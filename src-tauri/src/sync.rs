//! LAN sync wiring (SPEC §9): starts the `alst-sync` service and discovery, runs the sync triggers, and
//! exposes the Devices commands. The protocol and the data merge live in `alst-sync` and `alst-core`.

use crate::{AppState, Res, err};
use alst_sync::discovery::{Advert, Discovered, Discovery};
use alst_sync::peers::{self, MediaPolicy};
use alst_sync::service::{Observer, PairedDevice, QrPayload, Service};
use alst_sync::session::Outcome;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

/// Seconds the data must stay unchanged before a push, so a burst of edits is one sync (SPEC §9.4).
const DEBOUNCE: Duration = Duration::from_secs(30);
const PERIODIC: Duration = Duration::from_secs(600);
/// Don't start another background sync with one device sooner than this (mDNS re-announces often).
const COOLDOWN: Duration = Duration::from_secs(45);

#[derive(Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    /// `idle`, `syncing` or `error`.
    state: &'static str,
    message: Option<String>,
    last_synced_at: Option<i64>,
}

pub struct SyncCtx {
    app: AppHandle,
    service: Service,
    discovery: Option<Discovery>,
    status: Mutex<Status>,
    syncing: Mutex<HashSet<String>>,
    /// Devices the user asked to sync with: their failures are shown, background ones are quiet.
    manual: Mutex<HashSet<String>>,
    last_attempt: Mutex<HashMap<String, Instant>>,
    assets: Mutex<Option<(usize, usize)>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_ms() -> i64 {
    alst_core::hlc::wall_ms() as i64
}

struct Obs {
    app: AppHandle,
    ctx: std::sync::OnceLock<std::sync::Weak<SyncCtx>>,
}

impl Obs {
    fn ctx(&self) -> Option<Arc<SyncCtx>> {
        self.ctx.get().and_then(std::sync::Weak::upgrade)
    }
}

impl Observer for Obs {
    fn sync_started(&self, site_id: &str) {
        if let Some(c) = self.ctx() {
            lock(&c.syncing).insert(site_id.to_string());
            c.publish();
        }
    }

    fn sync_finished(&self, site_id: &str, result: &Result<Outcome, String>) {
        let Some(c) = self.ctx() else { return };
        lock(&c.syncing).remove(site_id);
        let manual = lock(&c.manual).remove(site_id);
        match result {
            Ok(o) => eprintln!("sync: with {} ok in {} ms (sent {} rows, received {} rows)", o.peer.name, o.elapsed_ms, o.sent_rows, o.received_rows),
            Err(m) => eprintln!("sync: with {site_id} failed: {m}"),
        }
        *lock(&c.assets) = None;
        {
            let mut st = lock(&c.status);
            match result {
                Ok(_) => {
                    st.state = "idle";
                    st.message = None;
                    st.last_synced_at = Some(now_ms());
                }
                // A device that simply isn't on the network is not an error worth showing.
                Err(m) if !manual && m.starts_with("Could not reach") => {}
                Err(m) => {
                    st.state = "error";
                    st.message = Some(m.clone());
                }
            }
        }
        c.publish();
        if result.is_ok() {
            // Archives that arrived are imported/installed in the background.
            let c = c.clone();
            std::thread::spawn(move || reconcile(&c));
        }
    }

    fn applied(&self, tables: &BTreeSet<String>) {
        let _ = self.app.emit("sync://applied", tables);
        // Extension choices decide the available languages and add-ons.
        if tables.contains("extension_state") || tables.contains("settings") {
            if let Some(state) = self.app.try_state::<AppState>() {
                let _ = crate::ext::reload(&state);
            }
        }
    }

    fn pair_code(&self, peer_name: &str, code: &str) {
        let _ = self.app.emit("sync://pair-code", serde_json::json!({ "peerName": peer_name, "code": code }));
    }

    fn paired(&self, result: &Result<PairedDevice, String>) {
        let _ = self.app.emit("sync://paired", result);
        if let (Some(c), Ok(p)) = (self.ctx(), result) {
            c.service.stop_pairing();
            if let Some(d) = &c.discovery {
                let _ = d.set_pairing(false);
            }
            let _ = c.app.emit("sync://state", c.overview_json());
            // The device that initiated pairing also starts the first sync; the other side will be called.
            let site = p.site_id.clone();
            let c2 = c.clone();
            std::thread::spawn(move || c2.sync(&site, false));
        }
    }

    fn assets(&self, done: usize, total: usize) {
        if let Some(c) = self.ctx() {
            *lock(&c.assets) = Some((done, total));
            let _ = self.app.emit("sync://assets", serde_json::json!({ "done": done, "total": total }));
        }
    }
}

impl SyncCtx {
    fn state(&self) -> Option<tauri::State<'_, AppState>> {
        self.app.try_state::<AppState>()
    }

    fn publish(&self) {
        {
            let busy = !lock(&self.syncing).is_empty();
            let mut st = lock(&self.status);
            if busy {
                st.state = "syncing";
            } else if st.state == "syncing" {
                st.state = "idle";
            }
        }
        let _ = self.app.emit("sync://state", self.overview_json());
    }

    fn hints(&self, site: &str) -> Vec<SocketAddr> {
        self.discovery.as_ref().map(|d| d.addrs_for(site)).unwrap_or_default()
    }

    /// Sync with one paired device now (blocking). `manual` means the user asked: show failures.
    fn sync(&self, site: &str, manual: bool) {
        if manual {
            lock(&self.manual).insert(site.to_string());
        }
        lock(&self.last_attempt).insert(site.to_string(), Instant::now());
        let _ = self.service.sync_with(site, &self.hints(site));
    }

    fn paired_sites(&self) -> Vec<String> {
        let Some(state) = self.state() else { return vec![] };
        let Ok(store) = state.store() else { return vec![] };
        peers::list_peers(&store).map(|v| v.into_iter().map(|p| p.site_id).collect()).unwrap_or_default()
    }

    fn sync_all(self: &Arc<Self>, only_discovered: bool) {
        for site in self.paired_sites() {
            if only_discovered && self.hints(&site).is_empty() {
                continue;
            }
            let ctx = self.clone();
            std::thread::spawn(move || ctx.sync(&site, false));
        }
    }

    fn overview_json(&self) -> serde_json::Value {
        let Some(state) = self.state() else { return serde_json::json!({}) };
        let Ok(store) = state.store() else { return serde_json::json!({}) };
        let nearby: Vec<Discovered> = self.discovery.as_ref().map(|d| d.list()).unwrap_or_default();
        let syncing = lock(&self.syncing).clone();
        let paired = peers::list_peers(&store).unwrap_or_default();
        let paired_sites: HashSet<&str> = paired.iter().map(|p| p.site_id.as_str()).collect();
        let peers_json: Vec<serde_json::Value> = paired
            .iter()
            .map(|p| {
                serde_json::json!({
                    "siteId": p.site_id, "name": p.name, "fingerprint": alst_sync::noise::fingerprint(&alst_sync::noise::unhex(&p.public_key).unwrap_or_default()),
                    "online": nearby.iter().any(|d| d.site_id == p.site_id),
                    "syncing": syncing.contains(&p.site_id), "lastSeen": p.last_seen, "addr": p.addr, "port": p.port, "mediaPolicy": p.media_policy,
                })
            })
            .collect();
        let others: Vec<&Discovered> = nearby.iter().filter(|d| !paired_sites.contains(d.site_id.as_str())).collect();
        serde_json::json!({
            "deviceName": peers::device_name(&store),
            "fingerprint": self.service.fingerprint(),
            "siteId": store.site_id(),
            "port": self.service.port(),
            "status": *lock(&self.status),
            "assets": *lock(&self.assets),
            "pairing": self.service.pairing_active(),
            "peers": peers_json,
            "nearby": others,
        })
    }
}

/// Import dictionaries and install extensions whose archives have arrived (SPEC §9.6).
fn reconcile(ctx: &Arc<SyncCtx>) {
    let Some(state) = ctx.state() else { return };
    let (dicts, deleted_dicts, pkgs, deleted_pkgs) = {
        let Ok(s) = state.store() else { return };
        (
            s.dictionaries().unwrap_or_default(),
            s.deleted_dictionary_ids().unwrap_or_default(),
            s.extension_packages().unwrap_or_default(),
            s.deleted_extension_package_ids().unwrap_or_default(),
        )
    };
    let mut changed: Vec<&str> = Vec::new();
    if let Ok(mut conn) = alst_core::db::open(&state.db_path) {
        match alst_dict::reconcile(&mut conn, &dicts, &deleted_dicts, &state.media) {
            Ok(r) if !r.imported.is_empty() || r.removed > 0 => changed.push("dictionaries"),
            Ok(_) => {}
            Err(e) => eprintln!("sync: dictionaries: {e}"),
        }
    }
    let r = {
        let Ok(reg) = state.registry() else { return };
        alst_ext::install::reconcile(&state.ext_root, &reg, &pkgs, &deleted_pkgs, &state.media)
    };
    for e in &r.errors {
        eprintln!("sync: extensions: {e}");
    }
    if !r.installed.is_empty() || !r.removed.is_empty() {
        if let Ok(mut s) = state.store() {
            for id in &r.installed {
                let _ = s.set_ext_state(id, Some(true), None, None);
            }
            for id in &r.removed {
                let _ = s.set_ext_state(id, Some(false), None, None);
            }
        }
        let _ = crate::ext::reload(&state);
        changed.push("extensions");
    }
    if !changed.is_empty() {
        let _ = ctx.app.emit("sync://applied", changed);
    }
}

fn default_device_name() -> String {
    if cfg!(target_os = "android") {
        return "Android phone".into();
    }
    let host = std::process::Command::new("hostname").output().ok().and_then(|o| String::from_utf8(o.stdout).ok()).unwrap_or_default();
    let host = host.trim().trim_end_matches(".local").to_string();
    if host.is_empty() { "This computer".into() } else { host }
}

/// Start the service and discovery, and the trigger loop. Failures disable sync but never the app.
pub fn start(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let obs = Arc::new(Obs { app: app.clone(), ctx: std::sync::OnceLock::new() });
    {
        let Ok(store) = state.store() else { return };
        if !store.local_conn().query_row("SELECT count(*) FROM local_settings WHERE key = 'device_name'", [], |r| Ok(r.get::<_, i64>(0)? > 0)).unwrap_or(true) {
            let _ = peers::set_device_name(&store, &default_device_name());
        }
    }
    let service = match Service::start(state.store.clone(), Some(state.media.clone()), obs.clone()) {
        Ok(s) => s,
        Err(e) => return eprintln!("sync: could not start: {e}"),
    };
    let (site_id, name, schema) = {
        let Ok(store) = state.store() else { return };
        (store.site_id().to_string(), peers::device_name(&store), store.schema_version())
    };
    let found_ctx: Arc<std::sync::OnceLock<std::sync::Weak<SyncCtx>>> = Arc::default();
    let on_found = {
        let found_ctx = found_ctx.clone();
        Arc::new(move |d: &Discovered| {
            let Some(ctx) = found_ctx.get().and_then(std::sync::Weak::upgrade) else { return };
            let _ = ctx.app.emit("sync://state", ctx.overview_json());
            // A paired device appeared: sync with it (SPEC §9.4), unless we just did.
            if !ctx.paired_sites().contains(&d.site_id) {
                return;
            }
            let recent = lock(&ctx.last_attempt).get(&d.site_id).is_some_and(|t| t.elapsed() < COOLDOWN);
            if !recent {
                let site = d.site_id.clone();
                std::thread::spawn(move || ctx.sync(&site, false));
            }
        })
    };
    let discovery = Discovery::start(
        Advert { site_id, name, fp: service.fingerprint(), port: service.port(), schema },
        on_found,
    )
    .map_err(|e| eprintln!("sync: discovery unavailable: {e}"))
    .ok();
    let ctx = Arc::new(SyncCtx {
        app: app.clone(),
        service,
        discovery,
        status: Mutex::new(Status { state: "idle", ..Default::default() }),
        syncing: Mutex::default(),
        manual: Mutex::default(),
        last_attempt: Mutex::default(),
        assets: Mutex::default(),
    });
    let _ = obs.ctx.set(Arc::downgrade(&ctx));
    let _ = found_ctx.set(Arc::downgrade(&ctx));
    let _ = state.sync.set(ctx.clone());
    std::thread::Builder::new().name("alst-sync-triggers".into()).spawn(move || triggers(ctx)).ok();
}

/// On start; 30 s after the last local write; every 10 minutes (SPEC §9.4). "A peer appears" is the discovery callback.
fn triggers(ctx: Arc<SyncCtx>) {
    std::thread::sleep(Duration::from_secs(3));
    ctx.sync_all(false);
    let mut own = own_clock(&ctx);
    let mut changed_at: Option<Instant> = None;
    let mut last_periodic = Instant::now();
    loop {
        std::thread::sleep(Duration::from_secs(5));
        let now = own_clock(&ctx);
        if now != own {
            own = now;
            changed_at = Some(Instant::now());
        }
        if changed_at.is_some_and(|t| t.elapsed() >= DEBOUNCE) {
            changed_at = None;
            ctx.sync_all(true);
        }
        if last_periodic.elapsed() >= PERIODIC {
            last_periodic = Instant::now();
            ctx.sync_all(false);
        }
    }
}

fn own_clock(ctx: &SyncCtx) -> i64 {
    let Some(state) = ctx.state() else { return 0 };
    let Ok(store) = state.store() else { return 0 };
    store.vector().ok().and_then(|v| v.get(store.site_id()).copied()).unwrap_or(0)
}

fn ctx(state: &AppState) -> Res<Arc<SyncCtx>> {
    state.sync.get().cloned().ok_or_else(|| "Sync is not available on this device.".to_string())
}

// ---- commands -------------------------------------------------------------------------------

#[tauri::command]
pub fn sync_overview(state: State<AppState>) -> Res<serde_json::Value> {
    Ok(ctx(&state)?.overview_json())
}

#[tauri::command]
pub fn set_device_name(state: State<AppState>, name: String) -> Res<()> {
    if name.trim().is_empty() {
        return Err("The name can't be empty.".into());
    }
    peers::set_device_name(&*state.store()?, &name).map_err(err)?;
    let c = ctx(&state)?;
    let _ = c.app.emit("sync://state", c.overview_json());
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairStart {
    svg: String,
    text: String,
    expires_at: i64,
    addrs: Vec<String>,
    port: u16,
}

/// Enter pairing mode and return the QR code (SPEC §9.3).
#[tauri::command]
pub fn pair_start(state: State<AppState>) -> Res<PairStart> {
    let c = ctx(&state)?;
    let info = c.service.start_pairing();
    if let Some(d) = &c.discovery {
        let _ = d.set_pairing(true);
    }
    let svg = qrcode::QrCode::new(info.qr_text.as_bytes())
        .map_err(err)?
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(240, 240)
        .quiet_zone(true)
        .build();
    // Leave pairing mode when the window ends.
    let c2 = c.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(125));
        if !c2.service.pairing_active() {
            if let Some(d) = &c2.discovery {
                let _ = d.set_pairing(false);
            }
            let _ = c2.app.emit("sync://state", c2.overview_json());
        }
    });
    Ok(PairStart { svg, text: info.qr_text, expires_at: info.expires_at, addrs: info.qr.addrs, port: info.qr.port })
}

#[tauri::command]
pub fn pair_stop(state: State<AppState>) -> Res<()> {
    let c = ctx(&state)?;
    c.service.stop_pairing();
    if let Some(d) = &c.discovery {
        let _ = d.set_pairing(false);
    }
    let _ = c.app.emit("sync://state", c.overview_json());
    Ok(())
}

/// Pair with a nearby device in pairing mode (by `site_id`), or one typed in as `host:port`.
#[tauri::command]
pub async fn pair_by_code(state: State<'_, AppState>, site_id: Option<String>, address: Option<String>) -> Res<PairedDevice> {
    let c = ctx(&state)?;
    let addrs: Vec<SocketAddr> = match (&site_id, &address) {
        (_, Some(a)) if !a.trim().is_empty() => a
            .trim()
            .to_socket_addrs()
            .map_err(|_| "Enter the address as host:port, for example 192.168.1.20:4000.".to_string())?
            .collect(),
        (Some(s), _) => c.hints(s),
        _ => return Err("Pick a device or enter its address.".into()),
    };
    if addrs.is_empty() {
        return Err("That device's address is not known yet. Make sure it is in pairing mode on the same Wi-Fi.".into());
    }
    tauri::async_runtime::spawn_blocking(move || c.service.pair_by_code(&addrs).map_err(|e| e.to_string())).await.map_err(err)?
}

/// Pair with the device whose QR code was scanned (`text` is the QR content).
#[tauri::command]
pub async fn pair_by_qr(state: State<'_, AppState>, text: String) -> Res<PairedDevice> {
    let c = ctx(&state)?;
    let qr = QrPayload::parse(&text).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || c.service.pair_by_qr(&qr).map_err(|e| e.to_string())).await.map_err(err)?
}

#[tauri::command]
pub fn pair_answer(state: State<AppState>, accept: bool) -> Res<()> {
    ctx(&state)?.service.answer_pairing(accept);
    Ok(())
}

/// "Sync now": one device, or all paired devices.
#[tauri::command]
pub fn sync_now(state: State<AppState>, site_id: Option<String>) -> Res<()> {
    let c = ctx(&state)?;
    let sites = match site_id {
        Some(s) => vec![s],
        None => c.paired_sites(),
    };
    for site in sites {
        let c = c.clone();
        std::thread::spawn(move || c.sync(&site, true));
    }
    Ok(())
}

#[tauri::command]
pub fn unpair(state: State<AppState>, site_id: String) -> Res<()> {
    peers::remove_peer(&*state.store()?, &site_id).map_err(err)?;
    let c = ctx(&state)?;
    let _ = c.app.emit("sync://state", c.overview_json());
    Ok(())
}

#[tauri::command]
pub fn set_media_policy(state: State<AppState>, site_id: String, policy: MediaPolicy) -> Res<()> {
    peers::set_media_policy(&*state.store()?, &site_id, &policy).map_err(err)?;
    let c = ctx(&state)?;
    let _ = c.app.emit("sync://state", c.overview_json());
    Ok(())
}

/// Remember an address typed by hand for a paired device (the fallback when mDNS doesn't work).
#[tauri::command]
pub fn set_peer_addr(state: State<AppState>, site_id: String, address: String) -> Res<()> {
    let sa = address.trim().to_socket_addrs().map_err(|_| "Enter the address as host:port.".to_string())?.next().ok_or("Enter the address as host:port.")?;
    peers::set_peer_addr(&*state.store()?, &site_id, &sa.ip().to_string(), sa.port()).map_err(err)
}

/// "Download now": fetch a media file from any reachable paired device, ignoring the media policy.
#[tauri::command]
pub async fn download_media(state: State<'_, AppState>, hash: String) -> Res<bool> {
    let c = ctx(&state)?;
    let hints: HashMap<String, Vec<SocketAddr>> = c.paired_sites().into_iter().map(|s| (s.clone(), c.hints(&s))).collect();
    tauri::async_runtime::spawn_blocking(move || c.service.download_media(&hash, &hints).map_err(|e| e.to_string())).await.map_err(err)?
}
