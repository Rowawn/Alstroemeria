//! First-sync benchmark over loopback with a generated library (SPEC §9; M6 gap closed in M7).
//! Slow, so ignored by default:
//!   cargo test -p alst-sync --release --test bench -- --ignored --nocapture
//! Prints the time to generate the library, the first sync, and the throughput of the media part.

use alst_core::media::MediaDir;
use alst_core::{NewPart, NewWork, Store, TermPatch};
use alst_sync::peers;
use alst_sync::service::{Observer, Service};
use std::net::SocketAddr;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TERMS: usize = 20_000;
const PARTS: usize = 300;
const MEDIA_FILES: usize = 20;
const MEDIA_FILE_BYTES: usize = 10 * 1024 * 1024; // 200 MB in all

struct Obs(Mutex<std::sync::mpsc::Sender<String>>);
impl Observer for Obs {
    fn pair_code(&self, _: &str, code: &str) {
        self.0.lock().unwrap().send(code.to_string()).unwrap();
    }
}

struct Dev {
    store: Arc<Mutex<Store>>,
    media: MediaDir,
    svc: Service,
    codes: Receiver<String>,
}

fn dev(name: &str) -> Dev {
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    peers::set_device_name(&store.lock().unwrap(), name).unwrap();
    let media = MediaDir::new(std::env::temp_dir().join(format!("alst-bench-{}", alst_core::store::new_id())));
    let (tx, codes) = channel();
    let svc = Service::start(store.clone(), Some(media.clone()), Arc::new(Obs(Mutex::new(tx)))).unwrap();
    Dev { store, media, svc, codes }
}

/// Cheap deterministic pseudo-random bytes, so files differ and don't compress away.
fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E3779B97F4A7C15) | 1;
    let mut v = Vec::with_capacity(len);
    while v.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.extend_from_slice(&x.to_le_bytes());
    }
    v.truncate(len);
    v
}

#[test]
#[ignore = "benchmark: run explicitly with --ignored --nocapture"]
fn first_sync_of_a_large_library() {
    let (a, b) = (dev("Mac"), dev("Phone"));
    let t = Instant::now();
    {
        let mut s = a.store.lock().unwrap();
        for i in 0..TERMS {
            let w = format!("palabra{i}");
            s.save_term("es", &w, &w, 1, &TermPatch { status: Some(2), ..Default::default() }).unwrap();
        }
        let per_work = 10;
        for w in 0..PARTS / per_work {
            let parts = (0..per_work)
                .map(|p| {
                    let text = format!("Capítulo {w}-{p}. ").repeat(180); // about 3 KB
                    NewPart { title: Some(format!("Parte {p}")), word_count: 360, text, media_hash: None, timing: None }
                })
                .collect();
            s.create_work(NewWork { language: "es".into(), title: format!("Libro {w}"), kind: "text".into(), author: None, source_url: None, parts }).unwrap();
        }
    }
    for i in 0..MEDIA_FILES {
        let (hash, size) = a.media.put_bytes(&noise(i as u64 + 1, MEDIA_FILE_BYTES), 1 << 30).unwrap();
        a.store.lock().unwrap().register_media(&hash, "audio/mpeg", size as i64, Some("bench.mp3")).unwrap();
    }
    let generated = t.elapsed();

    a.svc.start_pairing();
    let (svc, addr) = (b.svc.clone(), SocketAddr::from(([127, 0, 0, 1], a.svc.port())));
    let h = std::thread::spawn(move || svc.pair_by_code(&[addr]).unwrap());
    a.codes.recv_timeout(Duration::from_secs(10)).unwrap();
    b.codes.recv_timeout(Duration::from_secs(10)).unwrap();
    a.svc.answer_pairing(true);
    b.svc.answer_pairing(true);
    h.join().unwrap();
    std::thread::sleep(Duration::from_millis(300));

    let site = a.store.lock().unwrap().site_id().to_string();
    let t = Instant::now();
    b.svc.sync_with(&site, &[addr]).unwrap();
    let total = t.elapsed();
    let received = MEDIA_FILES * MEDIA_FILE_BYTES;
    assert_eq!(b.store.lock().unwrap().media_list().unwrap().len(), MEDIA_FILES);
    assert!(b.store.lock().unwrap().find_term("es", &format!("palabra{}", TERMS - 1)).unwrap().is_some());
    println!(
        "BENCH library: {TERMS} terms, {PARTS} parts, {} MB media (generated in {:.1}s)",
        received >> 20,
        generated.as_secs_f64()
    );
    println!("BENCH first sync, data + media, loopback: {:.2}s ({:.0} MB/s overall for the media)", total.as_secs_f64(), received as f64 / 1e6 / total.as_secs_f64());
    let t2 = Instant::now();
    b.svc.sync_with(&site, &[addr]).unwrap();
    println!("BENCH second sync (nothing new): {} ms", t2.elapsed().as_millis());
}
