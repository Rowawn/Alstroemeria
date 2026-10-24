//! Loopback tests for transport and pairing (SPEC §9.3): two services in one process over real TCP.

use alst_core::{Store, TermPatch};
use alst_sync::peers;
use alst_sync::service::{Observer, QrPayload, Service};
use std::net::SocketAddr;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct Obs {
    codes: Mutex<Sender<String>>,
}

impl Observer for Obs {
    fn pair_code(&self, _peer: &str, code: &str) {
        self.codes.lock().unwrap().send(code.to_string()).unwrap();
    }
}

struct Dev {
    store: Arc<Mutex<Store>>,
    svc: Service,
    codes: Receiver<String>,
}

impl Dev {
    fn new(name: &str) -> Dev {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        peers::set_device_name(&store.lock().unwrap(), name).unwrap();
        let (tx, codes) = channel();
        let svc = Service::start(store.clone(), None, Arc::new(Obs { codes: Mutex::new(tx) })).unwrap();
        Dev { store, svc, codes }
    }

    fn addr(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.svc.port()))
    }

    fn site(&self) -> String {
        self.store.lock().unwrap().site_id().to_string()
    }

    fn peers(&self) -> usize {
        peers::list_peers(&self.store.lock().unwrap()).unwrap().len()
    }

    fn term(&self, w: &str) {
        self.store.lock().unwrap().save_term("es", w, w, 1, &TermPatch { status: Some(2), ..Default::default() }).unwrap();
    }

    fn has_term(&self, w: &str) -> bool {
        self.store.lock().unwrap().find_term("es", w).unwrap().is_some()
    }
}

/// Pair `b` to `a` by code; both users accept (or not).
fn pair_by_code(a: &Dev, b: &Dev, a_accepts: bool, b_accepts: bool) -> (Result<(), String>, Result<(), String>, String, String) {
    a.svc.start_pairing();
    std::thread::scope(|sc| {
        let (svc, addr) = (b.svc.clone(), a.addr());
        let hb = sc.spawn(move || svc.pair_by_code(&[addr]).map(|_| ()).map_err(|e| e.to_string()));
        let code_a = a.codes.recv_timeout(Duration::from_secs(10)).unwrap();
        let code_b = b.codes.recv_timeout(Duration::from_secs(10)).unwrap();
        a.svc.answer_pairing(a_accepts);
        b.svc.answer_pairing(b_accepts);
        let rb = hb.join().unwrap();
        // A's side finishes on its own thread; wait for it to settle.
        std::thread::sleep(Duration::from_millis(300));
        let ra = if a.peers() == 1 { Ok(()) } else { Err("not paired".to_string()) };
        (ra, rb, code_a, code_b)
    })
}

#[test]
fn codes_match_on_both_ends_and_both_pin_each_other() {
    let (a, b) = (Dev::new("Mac"), Dev::new("Phone"));
    let (ra, rb, ca, cb) = pair_by_code(&a, &b, true, true);
    assert_eq!(ca, cb);
    assert_eq!(ca.len(), 6);
    assert!(ca.bytes().all(|c| c.is_ascii_digit()));
    ra.unwrap();
    rb.unwrap();
    assert_eq!((a.peers(), b.peers()), (1, 1));
    let pa = peers::list_peers(&a.store.lock().unwrap()).unwrap();
    assert_eq!((pa[0].site_id.as_str(), pa[0].name.as_str()), (b.site().as_str(), "Phone"));
    assert_eq!(pa[0].public_key, alst_sync::noise::hex(b.svc.public_key()));
}

#[test]
fn a_mismatch_pins_nothing() {
    let (a, b) = (Dev::new("Mac"), Dev::new("Phone"));
    let (_, rb, _, _) = pair_by_code(&a, &b, true, false);
    assert!(rb.is_err());
    assert_eq!((a.peers(), b.peers()), (0, 0));
}

#[test]
fn a_device_not_in_pairing_mode_is_not_paired() {
    let (a, b) = (Dev::new("Mac"), Dev::new("Phone"));
    assert!(b.svc.pair_by_code(&[a.addr()]).is_err());
    assert_eq!((a.peers(), b.peers()), (0, 0));
}

#[test]
fn qr_pairing_needs_no_code_and_a_wrong_secret_fails() {
    let (a, b) = (Dev::new("Mac"), Dev::new("Phone"));
    let info = a.svc.start_pairing();
    let mut wrong: QrPayload = QrPayload::parse(&info.qr_text).unwrap();
    wrong.addrs = vec!["127.0.0.1".into()];
    let mut bad = wrong.clone();
    bad.secret = "00".repeat(16);
    assert!(b.svc.pair_by_qr(&bad).is_err());
    assert_eq!((a.peers(), b.peers()), (0, 0), "a wrong PSK pins nothing");
    // The failed attempt used up the single-use secret; a new QR is needed.
    let info = a.svc.start_pairing();
    let mut qr = QrPayload::parse(&info.qr_text).unwrap();
    qr.addrs = vec!["127.0.0.1".into()];
    let mut impostor = qr.clone();
    impostor.pk = alst_sync::noise::hex(&[7u8; 32]);
    assert!(b.svc.pair_by_qr(&impostor).is_err(), "the key in the QR must match the device");
    let info = a.svc.start_pairing();
    let mut qr = QrPayload::parse(&info.qr_text).unwrap();
    qr.addrs = vec!["127.0.0.1".into()];
    let paired = b.svc.pair_by_qr(&qr).unwrap();
    assert_eq!(paired.site_id, a.site());
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!((a.peers(), b.peers()), (1, 1));
    assert!(b.codes.try_recv().is_err() && a.codes.try_recv().is_err(), "no code comparison");
}

#[test]
fn a_pinned_peer_syncs_both_ways_and_an_unknown_key_is_refused() {
    let (a, b, c) = (Dev::new("Mac"), Dev::new("Phone"), Dev::new("Stranger"));
    pair_by_code(&a, &b, true, true).0.unwrap();
    a.term("gato");
    b.term("perro");
    let out = b.svc.sync_with(&a.site(), &[a.addr()]).unwrap();
    assert!(out.applied.contains("terms"));
    assert!(a.has_term("perro") && b.has_term("gato"));
    // A third device that was never paired is dropped before any data moves.
    c.term("secreto");
    let stranger_peer = {
        // Give the stranger a record of A so it tries to connect, with A's real key.
        peers::add_peer(&c.store.lock().unwrap(), &a.site(), "Mac", a.svc.public_key(), Some("127.0.0.1"), Some(a.addr().port())).unwrap();
        a.site()
    };
    let err = c.svc.sync_with(&stranger_peer, &[a.addr()]).unwrap_err();
    assert!(!a.has_term("secreto"), "{err}");
    assert!(err.to_string().contains("closed"), "{err}");
    // Unpairing on one side stops syncing from that side's peer.
    peers::remove_peer(&a.store.lock().unwrap(), &b.site()).unwrap();
    assert!(b.svc.sync_with(&a.site(), &[a.addr()]).is_err());
}

#[test]
fn mdns_advertises_and_finds_a_device_and_its_pairing_flag() {
    use alst_sync::discovery::{Advert, Discovery};
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let id_a = "a".repeat(32);
    let id_b = "b".repeat(32);
    let a = Discovery::start(
        Advert { site_id: id_a.clone(), name: "Mac".into(), fp: "0011223344556677".into(), port: 4242, schema: 5 },
        Arc::new(|_| {}),
    )
    .unwrap();
    let b = Discovery::start(
        Advert { site_id: id_b, name: "Phone".into(), fp: "8899".into(), port: 4343, schema: 5 },
        Arc::new(move |d| {
            let _ = tx.lock().unwrap().send(d.clone());
        }),
    )
    .unwrap();
    a.set_pairing(true).unwrap();
    let mut seen = None;
    while let Ok(d) = rx.recv_timeout(Duration::from_secs(10)) {
        if d.site_id == id_a && d.pairing {
            seen = Some(d);
            break;
        }
    }
    let d = seen.expect("device found over mDNS with pair=1");
    assert_eq!((d.name.as_str(), d.port, d.schema, d.fp.as_str()), ("Mac", 4242, 5, "0011223344556677"));
    assert!(!d.addrs.is_empty());
    assert!(b.list().iter().any(|x| x.site_id == id_a));
    a.stop();
    b.stop();
}
