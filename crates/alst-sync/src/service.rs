//! The sync service: a TCP listener, outgoing sessions, and both kinds of pairing (SPEC §9.3, §9.4).

use crate::noise::{self, KeyPair, NoiseTransport, Pattern, fingerprint, hex, unhex};
use crate::peers::{self, Peer};
use crate::proto::{Msg, Transport, unexpected};
use crate::session::{self, Outcome, Role, SessionConfig};
use crate::{Error, Result};
use alst_core::Store;
use alst_core::media::MediaDir;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// First byte of every connection (plaintext): which handshake follows.
const MODE_SYNC: u8 = 0;
const MODE_CODE: u8 = 1;
const MODE_QR: u8 = 2;
/// Fetch one file from a paired device on demand, ignoring the media policy (SPEC §9.6).
const MODE_FETCH: u8 = 3;

const PAIRING_WINDOW: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const IO_TIMEOUT: Duration = Duration::from_secs(60);

/// A device that finished pairing.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedDevice {
    pub site_id: String,
    pub name: String,
    pub public_key: String,
}

/// The contents of the QR code on the pairing screen (SPEC §9.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QrPayload {
    pub v: u32,
    pub name: String,
    pub addrs: Vec<String>,
    pub port: u16,
    pub pk: String,
    pub secret: String,
}

impl QrPayload {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn parse(text: &str) -> Result<Self> {
        let q: QrPayload = serde_json::from_str(text).map_err(|_| Error::Refused("That is not a pairing code.".into()))?;
        if q.v != 1 {
            return Err(Error::Refused("That pairing code is from a different version of the app.".into()));
        }
        Ok(q)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingInfo {
    pub qr: QrPayload,
    /// The QR payload as text, ready to render.
    pub qr_text: String,
    /// Unix ms when pairing mode ends.
    pub expires_at: i64,
}

/// What the app wants to hear about. All methods have empty defaults.
pub trait Observer: Send + Sync {
    /// A session with this device began / ended (`Err` carries a message for the user).
    fn sync_started(&self, _site_id: &str) {}
    fn sync_finished(&self, _site_id: &str, _result: &std::result::Result<Outcome, String>) {}
    /// Tables changed by an applied batch: refresh what shows them.
    fn applied(&self, _tables: &BTreeSet<String>) {}
    /// Pairing by code reached the comparison step; call `Service::answer_pairing` when the user decides.
    fn pair_code(&self, _peer_name: &str, _code: &str) {}
    fn paired(&self, _result: &std::result::Result<PairedDevice, String>) {}
    /// Asset downloads progressed (`done` of `total` files).
    fn assets(&self, _done: usize, _total: usize) {}
}

struct Pairing {
    secret: [u8; 16],
    deadline: Instant,
}

struct Inner {
    store: Arc<Mutex<Store>>,
    media: Option<MediaDir>,
    observer: Arc<dyn Observer>,
    keys: KeyPair,
    port: u16,
    pairing: Mutex<Option<Pairing>>,
    answer: Mutex<Option<Sender<bool>>>,
    busy: Mutex<HashSet<String>>,
    stop: AtomicBool,
}

#[derive(Clone)]
pub struct Service {
    inner: Arc<Inner>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn msg_of(e: &Error) -> String {
    match e {
        Error::Closed => "The other device closed the connection (is it paired with this device?).".into(),
        other => other.to_string(),
    }
}

impl Service {
    /// Bind a random port and start accepting connections.
    pub fn start(store: Arc<Mutex<Store>>, media: Option<MediaDir>, observer: Arc<dyn Observer>) -> Result<Service> {
        let keys = peers::device_keypair(&lock(&store))?;
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        let port = listener.local_addr()?.port();
        let inner = Arc::new(Inner {
            store,
            media,
            observer,
            keys,
            port,
            pairing: Mutex::new(None),
            answer: Mutex::new(None),
            busy: Mutex::new(HashSet::new()),
            stop: AtomicBool::new(false),
        });
        let svc = Service { inner: inner.clone() };
        std::thread::Builder::new().name("alst-sync-accept".into()).spawn(move || {
            for conn in listener.incoming() {
                if inner.stop.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = conn else { continue };
                let inner = inner.clone();
                let _ = std::thread::Builder::new().name("alst-sync-conn".into()).spawn(move || {
                    let _ = handle_incoming(&inner, stream);
                });
            }
        })?;
        Ok(svc)
    }

    pub fn port(&self) -> u16 {
        self.inner.port
    }

    pub fn public_key(&self) -> &[u8] {
        &self.inner.keys.public
    }

    pub fn fingerprint(&self) -> String {
        fingerprint(&self.inner.keys.public)
    }

    /// Stop accepting connections (existing sessions finish).
    pub fn stop(&self) {
        self.inner.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, self.inner.port)), Duration::from_millis(500));
    }

    // ---- pairing mode ------------------------------------------------------------------------------

    /// Enter pairing mode for two minutes (accept unknown keys) and return what the QR code should say.
    pub fn start_pairing(&self) -> PairingInfo {
        let secret: [u8; 16] = *uuid::Uuid::new_v4().as_bytes();
        *lock(&self.inner.pairing) = Some(Pairing { secret, deadline: Instant::now() + PAIRING_WINDOW });
        let name = peers::device_name(&lock(&self.inner.store));
        let qr = QrPayload { v: 1, name, addrs: local_addrs(), port: self.inner.port, pk: hex(&self.inner.keys.public), secret: hex(&secret) };
        PairingInfo {
            qr_text: qr.to_json(),
            qr,
            expires_at: (alst_core::hlc::wall_ms() + PAIRING_WINDOW.as_millis() as u64) as i64,
        }
    }

    pub fn stop_pairing(&self) {
        *lock(&self.inner.pairing) = None;
    }

    pub fn pairing_active(&self) -> bool {
        lock(&self.inner.pairing).as_ref().is_some_and(|p| p.deadline > Instant::now())
    }

    /// The user's answer to the code comparison ("Codes match" or "They differ").
    pub fn answer_pairing(&self, accept: bool) {
        if let Some(tx) = lock(&self.inner.answer).take() {
            let _ = tx.send(accept);
        }
    }

    /// Pair with a device in pairing mode, found by discovery or typed in. Blocks until done.
    pub fn pair_by_code(&self, addrs: &[SocketAddr]) -> Result<PairedDevice> {
        let r = self.pair_connect(addrs, MODE_CODE, None, None);
        self.finish_pairing(r)
    }

    /// Pair with the device whose QR code was scanned. Blocks until done.
    pub fn pair_by_qr(&self, qr: &QrPayload) -> Result<PairedDevice> {
        let secret = unhex(&qr.secret).filter(|s| s.len() == 16).ok_or_else(|| Error::Refused("That pairing code is damaged.".into()))?;
        let pk = unhex(&qr.pk).ok_or_else(|| Error::Refused("That pairing code is damaged.".into()))?;
        let mut addrs: Vec<SocketAddr> = qr.addrs.iter().filter_map(|a| a.parse::<IpAddr>().ok()).map(|ip| SocketAddr::new(ip, qr.port)).collect();
        if addrs.is_empty() {
            return Err(Error::Refused("That pairing code has no address.".into()));
        }
        addrs.dedup();
        let r = self.pair_connect(&addrs, MODE_QR, Some(secret), Some(pk));
        self.finish_pairing(r)
    }

    fn finish_pairing(&self, r: Result<PairedDevice>) -> Result<PairedDevice> {
        self.inner.observer.paired(&r.as_ref().map(Clone::clone).map_err(msg_of));
        r
    }

    fn pair_connect(&self, addrs: &[SocketAddr], mode: u8, secret: Option<Vec<u8>>, expect_pk: Option<Vec<u8>>) -> Result<PairedDevice> {
        let (stream, addr) = connect_any(addrs)?;
        let mut stream = stream;
        stream.write_all(&[mode])?;
        let pattern = if mode == MODE_QR { Pattern::XxPsk3 } else { Pattern::Xx };
        let hs = noise::handshake_initiator(stream, &self.inner.keys, pattern, secret.as_deref())?;
        if let Some(pk) = expect_pk
            && pk != hs.remote_static
        {
            return Err(Error::Refused("That is not the device in the pairing code.".into()));
        }
        let mut t = hs.transport;
        let (site_id, name, port) = my_identity(&self.inner);
        t.send(&Msg::PairInfo { site_id, name, port })?;
        let Msg::PairInfo { site_id: theirs, name: their_name, port: their_port } = t.recv()? else {
            return Err(Error::Protocol("expected pair info".into()));
        };
        let accept = if mode == MODE_CODE { self.ask_user(&their_name, &hs.code) } else { true };
        conclude(&self.inner, &mut t, accept, &theirs, &their_name, &hs.remote_static, Some(addr.ip()), their_port)
    }

    fn ask_user(&self, peer_name: &str, code: &str) -> bool {
        ask_user(&self.inner, peer_name, code)
    }

    // ---- sync --------------------------------------------------------------------------------------

    /// Sync with a paired device now. `hints` are addresses from discovery; the last known address is tried too.
    /// Blocks until the session (and asset transfer) ends.
    pub fn sync_with(&self, site_id: &str, hints: &[SocketAddr]) -> Result<Outcome> {
        let peer = peers::peer_by_site(&lock(&self.inner.store), site_id)?.ok_or_else(|| Error::Refused("That device is not paired.".into()))?;
        let key = peer.public_key.clone();
        if !lock(&self.inner.busy).insert(key.clone()) {
            return Err(Error::Refused("Already syncing with that device.".into()));
        }
        self.inner.observer.sync_started(&peer.site_id);
        let result = outgoing_session(&self.inner, &peer, hints);
        lock(&self.inner.busy).remove(&key);
        self.inner.observer.sync_finished(&peer.site_id, &result.as_ref().map(Clone::clone).map_err(msg_of));
        result
    }

    /// `sync_with` on a background thread.
    pub fn sync_in_background(&self, site_id: String, hints: Vec<SocketAddr>) {
        let svc = self.clone();
        let _ = std::thread::Builder::new().name("alst-sync-out".into()).spawn(move || {
            let _ = svc.sync_with(&site_id, &hints);
        });
    }

    /// "Download now": fetch one media file from the first reachable paired device that has it, whatever the
    /// media policy says. `hints` maps a device to addresses found by discovery. Returns whether it arrived.
    pub fn download_media(&self, hash: &str, hints: &std::collections::HashMap<String, Vec<SocketAddr>>) -> Result<bool> {
        let media = self.inner.media.as_ref().ok_or_else(|| Error::Refused("This device has no media store.".into()))?;
        if media.exists(hash) {
            return Ok(true);
        }
        let all = peers::list_peers(&lock(&self.inner.store))?;
        let mut last = Error::Refused("No paired device is reachable.".into());
        for peer in all {
            let addrs = candidates(&peer, hints.get(&peer.site_id).map_or(&[][..], Vec::as_slice));
            let Ok((mut stream, _)) = connect_any(&addrs) else { continue };
            let attempt = (|| -> Result<bool> {
                stream.set_read_timeout(Some(IO_TIMEOUT))?;
                stream.write_all(&[MODE_FETCH])?;
                let hs = noise::handshake_initiator(stream, &self.inner.keys, Pattern::Xx, None)?;
                if hex(&hs.remote_static) != peer.public_key {
                    return Ok(false);
                }
                let mut t = hs.transport;
                t.send(&Msg::Want { hashes: vec![hash.to_string()] })?;
                let sizes = lock(&self.inner.store).media_info(hash)?.map(|r| (hash.to_string(), r.size)).into_iter().collect();
                let got = crate::assets::receive(media, &mut t, &[hash.to_string()], &sizes, &*self.inner.observer)?;
                Ok(!got.is_empty())
            })();
            match attempt {
                Ok(true) => return Ok(true),
                Ok(false) => {}
                Err(e) => last = e,
            }
        }
        match last {
            Error::Refused(_) => Ok(false),
            other => Err(other),
        }
    }

    pub fn is_busy(&self, public_key_hex: &str) -> bool {
        lock(&self.inner.busy).contains(public_key_hex)
    }
}

fn my_identity(inner: &Inner) -> (String, String, u16) {
    let s = lock(&inner.store);
    (s.site_id().to_string(), peers::device_name(&s), inner.port)
}

fn ask_user(inner: &Inner, peer_name: &str, code: &str) -> bool {
    let (tx, rx) = channel();
    *lock(&inner.answer) = Some(tx);
    inner.observer.pair_code(peer_name, code);
    rx.recv_timeout(PAIRING_WINDOW).unwrap_or(false)
}

/// Exchange verdicts and, if both sides accepted, pin the other device.
#[allow(clippy::too_many_arguments)]
fn conclude(
    inner: &Inner,
    t: &mut NoiseTransport,
    accept: bool,
    site_id: &str,
    name: &str,
    public_key: &[u8],
    ip: Option<IpAddr>,
    port: u16,
) -> Result<PairedDevice> {
    t.send(&Msg::PairVerdict { accept })?;
    let theirs = match t.recv()? {
        Msg::PairVerdict { accept } => accept,
        other => return unexpected(&other, "verdict"),
    };
    if !(accept && theirs) {
        return Err(Error::Refused(if accept { format!("{name} did not accept the pairing.") } else { "Pairing cancelled.".into() }));
    }
    let addr = ip.map(|i| i.to_string());
    peers::add_peer(&lock(&inner.store), site_id, name, public_key, addr.as_deref(), Some(port))?;
    Ok(PairedDevice { site_id: site_id.into(), name: name.into(), public_key: hex(public_key) })
}

fn connect_any(addrs: &[SocketAddr]) -> Result<(TcpStream, SocketAddr)> {
    let mut last = Error::Refused("No address to connect to.".into());
    for a in addrs {
        match TcpStream::connect_timeout(a, CONNECT_TIMEOUT) {
            Ok(s) => {
                s.set_read_timeout(Some(PAIRING_WINDOW + Duration::from_secs(30)))?;
                s.set_write_timeout(Some(IO_TIMEOUT))?;
                s.set_nodelay(true).ok();
                return Ok((s, *a));
            }
            Err(e) => last = Error::Io(e),
        }
    }
    Err(match last {
        Error::Io(e) => Error::Refused(format!("Could not reach the other device ({e}).")),
        other => other,
    })
}

fn candidates(peer: &Peer, hints: &[SocketAddr]) -> Vec<SocketAddr> {
    let mut v: Vec<SocketAddr> = hints.to_vec();
    if let (Some(a), Some(p)) = (&peer.addr, peer.port)
        && let Ok(mut it) = (a.as_str(), p).to_socket_addrs()
        && let Some(sa) = it.next()
        && !v.contains(&sa)
    {
        v.push(sa);
    }
    v
}

fn outgoing_session(inner: &Inner, peer: &Peer, hints: &[SocketAddr]) -> Result<Outcome> {
    let (mut stream, addr) = connect_any(&candidates(peer, hints))?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.write_all(&[MODE_SYNC])?;
    let hs = noise::handshake_initiator(stream, &inner.keys, Pattern::Xx, None)?;
    if hex(&hs.remote_static) != peer.public_key {
        return Err(Error::Refused(format!("The device at that address is not {}.", peer.name)));
    }
    let mut t = hs.transport;
    run_session(inner, &mut t, Role::Initiator, Some(addr))
}

fn run_session(inner: &Inner, t: &mut NoiseTransport, role: Role, addr: Option<SocketAddr>) -> Result<Outcome> {
    let started = Instant::now();
    let name = peers::device_name(&lock(&inner.store));
    let cfg = SessionConfig::new(name);
    let mut out = session::run(&inner.store, t, role, &cfg)?;
    if !out.applied.is_empty() {
        inner.observer.applied(&out.applied);
    }
    // Assets (media files, dictionary and extension archives) follow the data (SPEC §9.6).
    crate::assets::exchange(inner.media.as_ref(), &inner.store, t, role, &out.peer.site_id, &*inner.observer)?;
    out.elapsed_ms = started.elapsed().as_millis() as u64;
    let s = lock(&inner.store);
    let remembered = addr.filter(|_| role == Role::Initiator);
    peers::touch_peer(&s, &out.peer.site_id, remembered.map(|a| a.ip().to_string()).as_deref(), remembered.map(|a| a.port()), &s.vector()?)?;
    drop(s);
    Ok(out)
}

fn handle_incoming(inner: &Arc<Inner>, mut stream: TcpStream) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    stream.set_nodelay(true).ok();
    let remote_ip = stream.peer_addr().ok().map(|a| a.ip());
    let mut mode = [0u8; 1];
    stream.read_exact(&mut mode)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    match mode[0] {
        MODE_SYNC => {
            let hs = noise::handshake_responder(stream, &inner.keys, Pattern::Xx, None)?;
            let Some(peer) = peers::peer_by_key(&lock(&inner.store), &hs.remote_static)? else {
                return Ok(()); // an unknown key: drop the connection
            };
            if !lock(&inner.busy).insert(peer.public_key.clone()) {
                return Ok(());
            }
            inner.observer.sync_started(&peer.site_id);
            let mut t = hs.transport;
            let result = run_session(inner, &mut t, Role::Responder, None);
            lock(&inner.busy).remove(&peer.public_key);
            inner.observer.sync_finished(&peer.site_id, &result.map_err(|e| msg_of(&e)));
            Ok(())
        }
        MODE_CODE | MODE_QR => {
            let secret = {
                let mut p = lock(&inner.pairing);
                match p.as_ref() {
                    Some(pairing) if pairing.deadline > Instant::now() => {
                        let secret = pairing.secret;
                        if mode[0] == MODE_QR {
                            *p = None; // a QR secret is good for one pairing
                        }
                        secret
                    }
                    _ => return Ok(()),
                }
            };
            stream.set_read_timeout(Some(PAIRING_WINDOW + Duration::from_secs(30)))?;
            let result = pair_incoming(inner, stream, mode[0], &secret, remote_ip);
            if mode[0] == MODE_CODE && result.is_ok() {
                *lock(&inner.pairing) = None;
            }
            Service { inner: inner.clone() }.finish_pairing(result).map(|_| ())
        }
        MODE_FETCH => {
            let hs = noise::handshake_responder(stream, &inner.keys, Pattern::Xx, None)?;
            if peers::peer_by_key(&lock(&inner.store), &hs.remote_static)?.is_none() {
                return Ok(());
            }
            let mut t = hs.transport;
            if let Msg::Want { hashes } = t.recv()? {
                crate::assets::serve(inner.media.as_ref(), &mut t, &hashes)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn pair_incoming(inner: &Inner, stream: TcpStream, mode: u8, secret: &[u8; 16], ip: Option<IpAddr>) -> Result<PairedDevice> {
    let hs = if mode == MODE_QR {
        noise::handshake_responder(stream, &inner.keys, Pattern::XxPsk3, Some(secret))?
    } else {
        noise::handshake_responder(stream, &inner.keys, Pattern::Xx, None)?
    };
    let mut t = hs.transport;
    let Msg::PairInfo { site_id, name, port } = t.recv()? else { return Err(Error::Protocol("expected pair info".into())) };
    let (my_site, my_name, my_port) = my_identity(inner);
    t.send(&Msg::PairInfo { site_id: my_site, name: my_name, port: my_port })?;
    let accept = if mode == MODE_CODE { ask_user(inner, &name, &hs.code) } else { true };
    conclude(inner, &mut t, accept, &site_id, &name, &hs.remote_static, ip, port)
}

/// Non-loopback IPv4 addresses of this device, for the QR code and the mDNS advertisement.
pub fn local_addrs() -> Vec<String> {
    if_addrs::get_if_addrs()
        .map(|ifs| {
            ifs.into_iter()
                .filter(|i| !i.is_loopback())
                .filter_map(|i| match i.ip() {
                    IpAddr::V4(v4) if !v4.is_link_local() => Some(v4.to_string()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}
