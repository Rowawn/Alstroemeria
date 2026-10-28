//! Asset transfer (SPEC §9.6) between two paired services over TCP.

use alst_core::Store;
use alst_core::media::MediaDir;
use alst_sync::peers::{self, MediaPolicy};
use alst_sync::service::{Observer, Service};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
    codes: std::sync::mpsc::Receiver<String>,
}

fn dev(name: &str) -> Dev {
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    peers::set_device_name(&store.lock().unwrap(), name).unwrap();
    let dir = std::env::temp_dir().join(format!("alst-assets-{}", alst_core::store::new_id()));
    let media = MediaDir::new(&dir);
    let (tx, codes) = std::sync::mpsc::channel();
    let svc = Service::start(store.clone(), Some(media.clone()), Arc::new(Obs(Mutex::new(tx)))).unwrap();
    Dev { store, media, svc, codes }
}

impl Dev {
    fn addr(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.svc.port()))
    }
    fn site(&self) -> String {
        self.store.lock().unwrap().site_id().to_string()
    }
    /// Store a file and its media row.
    fn add(&self, bytes: &[u8], mime: &str) -> String {
        let (hash, size) = self.media.put_bytes(bytes, 1 << 30).unwrap();
        self.store.lock().unwrap().register_media(&hash, mime, size as i64, Some("f")).unwrap();
        hash
    }
}

fn pair(a: &Dev, b: &Dev) {
    a.svc.start_pairing();
    let svc = b.svc.clone();
    let addr = a.addr();
    let h = std::thread::spawn(move || svc.pair_by_code(&[addr]).unwrap());
    a.codes.recv_timeout(Duration::from_secs(10)).unwrap();
    b.codes.recv_timeout(Duration::from_secs(10)).unwrap();
    a.svc.answer_pairing(true);
    b.svc.answer_pairing(true);
    h.join().unwrap();
    std::thread::sleep(Duration::from_millis(300));
}

#[test]
fn files_move_by_policy_and_dictionaries_always_and_on_demand_ignores_policy() {
    let (a, b) = (dev("Mac"), dev("Phone"));
    pair(&a, &b);
    let audio = a.add(&vec![7u8; 700_000], "audio/mpeg"); // several chunks
    let video = a.add(&vec![9u8; 1000], "video/mp4");
    let empty = a.add(b"", "audio/mpeg");
    let dict = a.add(b"PK dictionary archive", "application/zip");
    a.store.lock().unwrap().put_dictionary(&dict, "Dict", None, "es").unwrap();
    // The phone takes audio but not video.
    peers::set_media_policy(
        &b.store.lock().unwrap(),
        &a.site(),
        &MediaPolicy { audio: true, video: false, images: true, max_mb: 0 },
    )
    .unwrap();
    b.svc.sync_with(&a.site(), &[a.addr()]).unwrap();
    assert!(b.media.exists(&audio) && b.media.exists(&dict) && b.media.exists(&empty));
    assert!(!b.media.exists(&video), "video is skipped by policy");
    assert_eq!(b.media.read_range(&audio, 0, 10).unwrap().1, 700_000);
    // "Download now" ignores the policy.
    assert!(b.svc.download_media(&video, &HashMap::from([(a.site(), vec![a.addr()])])).unwrap());
    assert!(b.media.exists(&video));
    // Syncing again moves nothing and doesn't fail.
    b.svc.sync_with(&a.site(), &[a.addr()]).unwrap();
}

#[test]
fn a_size_cap_skips_big_files() {
    let (a, b) = (dev("Mac"), dev("Phone"));
    pair(&a, &b);
    let big = a.add(&vec![1u8; 2 * 1024 * 1024], "audio/mpeg");
    peers::set_media_policy(&b.store.lock().unwrap(), &a.site(), &MediaPolicy { audio: true, video: true, images: true, max_mb: 1 }).unwrap();
    b.svc.sync_with(&a.site(), &[a.addr()]).unwrap();
    assert!(!b.media.exists(&big));
}

#[test]
fn a_file_that_fails_its_checksum_is_not_stored() {
    use alst_sync::proto::{Msg, Transport, mem};
    let dir = std::env::temp_dir().join(format!("alst-bad-{}", alst_core::store::new_id()));
    let media = MediaDir::new(&dir);
    let claimed = "ab".repeat(32);
    let (mut a, mut b) = mem::pair(None, None);
    a.send(&Msg::Chunk { hash: claimed.clone(), offset: 0, data: b"not what it says".to_vec() }).unwrap();
    a.send(&Msg::FileEnd { hash: claimed.clone() }).unwrap();
    struct Quiet;
    impl Observer for Quiet {}
    let err = alst_sync::assets::receive(&media, &mut b, &[claimed.clone()], &HashMap::new(), &Quiet).unwrap_err();
    assert!(err.to_string().contains("checksum"), "{err}");
    assert!(!media.exists(&claimed));
}

/// The Android defaults (SPEC §9.6): video off, files over 500 MB skipped. The size rule goes by the synced
/// row's `size`, so rows can claim a big size without storing a big file.
#[test]
fn android_defaults_skip_video_and_files_over_500_mb() {
    let (a, b) = (dev("Mac"), dev("Phone"));
    pair(&a, &b);
    let android = MediaPolicy { audio: true, video: false, images: true, max_mb: 500 };
    assert_eq!(android, MediaPolicy { audio: true, video: false, images: true, max_mb: 500 });
    let add_sized = |bytes: &[u8], mime: &str, claimed_mb: i64| {
        let (hash, _) = a.media.put_bytes(bytes, 1 << 30).unwrap();
        a.store.lock().unwrap().register_media(&hash, mime, claimed_mb * 1024 * 1024, Some("f")).unwrap();
        hash
    };
    let small_audio = add_sized(b"small audio", "audio/mpeg", 3);
    let edge_audio = add_sized(b"exactly at the cap", "audio/mpeg", 500);
    let huge_audio = add_sized(b"over the cap", "audio/mpeg", 501);
    let video = add_sized(b"a short video", "video/mp4", 1);
    let image = add_sized(b"an image", "image/png", 1);
    peers::set_media_policy(&b.store.lock().unwrap(), &a.site(), &android).unwrap();
    b.svc.sync_with(&a.site(), &[a.addr()]).unwrap();
    assert!(b.media.exists(&small_audio) && b.media.exists(&edge_audio) && b.media.exists(&image));
    assert!(!b.media.exists(&huge_audio), "over 500 MB is skipped");
    assert!(!b.media.exists(&video), "video is off");
    // "Download now" still gets them.
    let hints = HashMap::from([(a.site(), vec![a.addr()])]);
    assert!(b.svc.download_media(&huge_audio, &hints).unwrap() && b.svc.download_media(&video, &hints).unwrap());
}

/// A language data package is always transferred, whatever the media policy (SPEC §17.3).
#[test]
fn language_data_packages_ignore_the_media_policy() {
    let (a, b) = (dev("Mac"), dev("Phone"));
    pair(&a, &b);
    let pkg = a.add(&vec![5u8; 600_000], "application/x-alstroemeria-langdata");
    peers::set_media_policy(&b.store.lock().unwrap(), &a.site(), &MediaPolicy { audio: false, video: false, images: false, max_mb: 1 }).unwrap();
    b.svc.sync_with(&a.site(), &[a.addr()]).unwrap();
    assert!(b.media.exists(&pkg));
}
