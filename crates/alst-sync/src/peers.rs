//! Device identity and the paired-peer list. Both live in device-local tables (`local_settings`,
//! `local_peers`), so nothing here is synced (SPEC §9.3).

use crate::noise::{KeyPair, generate_keypair, hex, unhex};
use crate::{Error, Result};
use alst_core::Store;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

/// What this device downloads from a peer (SPEC §9.6). `max_mb` 0 means no limit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaPolicy {
    pub audio: bool,
    pub video: bool,
    pub images: bool,
    pub max_mb: u64,
}

impl Default for MediaPolicy {
    /// Desktop takes everything; Android skips video and caps file size.
    fn default() -> Self {
        if cfg!(target_os = "android") {
            MediaPolicy { audio: true, video: false, images: true, max_mb: 500 }
        } else {
            MediaPolicy { audio: true, video: true, images: true, max_mb: 0 }
        }
    }
}

impl MediaPolicy {
    /// Whether a file of this type and size is fetched automatically.
    pub fn allows(&self, mime: &str, size: i64) -> bool {
        let kind_ok = if mime.starts_with("audio/") {
            self.audio
        } else if mime.starts_with("video/") {
            self.video
        } else if mime.starts_with("image/") {
            self.images
        } else {
            true
        };
        kind_ok && (self.max_mb == 0 || size <= (self.max_mb * 1024 * 1024) as i64)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub site_id: String,
    pub name: String,
    pub public_key: String,
    pub addr: Option<String>,
    pub port: Option<u16>,
    pub media_policy: MediaPolicy,
    pub paired_at: Option<i64>,
    pub last_seen: Option<i64>,
    pub last_vector: serde_json::Value,
}

fn setting(store: &Store, key: &str) -> Option<String> {
    store.local_conn().query_row("SELECT value FROM local_settings WHERE key = ?1", [key], |r| r.get(0)).ok()
}

fn set_setting(store: &Store, key: &str, value: &str) -> Result<()> {
    store
        .local_conn()
        .execute("INSERT INTO local_settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2", params![key, value])
        .map_err(alst_core::Error::from)?;
    Ok(())
}

/// This device's static key pair, generated on first use and never synced.
pub fn device_keypair(store: &Store) -> Result<KeyPair> {
    if let (Some(private), Some(public)) = (setting(store, "noise_private"), setting(store, "noise_public"))
        && let (Some(private), Some(public)) = (unhex(&private), unhex(&public))
    {
        return Ok(KeyPair { private, public });
    }
    let kp = generate_keypair()?;
    set_setting(store, "noise_private", &hex(&kp.private))?;
    set_setting(store, "noise_public", &hex(&kp.public))?;
    Ok(kp)
}

pub fn device_name(store: &Store) -> String {
    setting(store, "device_name").filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
        let host = std::env::var("HOSTNAME").or_else(|_| std::env::var("COMPUTERNAME")).unwrap_or_default();
        if host.is_empty() { format!("Device {}", &store.site_id()[..4]) } else { host }
    })
}

pub fn set_device_name(store: &Store, name: &str) -> Result<()> {
    set_setting(store, "device_name", name.trim())
}

fn peer_row(r: &rusqlite::Row) -> rusqlite::Result<Peer> {
    let policy: String = r.get(6)?;
    let vector: String = r.get(9)?;
    Ok(Peer {
        site_id: r.get(0)?,
        name: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
        public_key: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        addr: r.get(3)?,
        port: r.get::<_, Option<i64>>(4)?.map(|p| p as u16),
        paired_at: r.get(5)?,
        media_policy: serde_json::from_str(&policy).unwrap_or_default(),
        last_seen: r.get(7)?,
        last_vector: serde_json::from_str(&vector).unwrap_or(serde_json::json!({})),
    })
}

const COLS: &str = "site_id, name, public_key, addr, port, paired_at, media_policy, last_seen, 0, last_vector";

pub fn list_peers(store: &Store) -> Result<Vec<Peer>> {
    let c = store.local_conn();
    let mut stmt = c.prepare(&format!("SELECT {COLS} FROM local_peers WHERE public_key IS NOT NULL ORDER BY paired_at, site_id")).map_err(alst_core::Error::from)?;
    let rows = stmt.query_map([], peer_row).map_err(alst_core::Error::from)?;
    Ok(rows.collect::<std::result::Result<_, _>>().map_err(alst_core::Error::from)?)
}

pub fn peer_by_key(store: &Store, public_key: &[u8]) -> Result<Option<Peer>> {
    Ok(store
        .local_conn()
        .query_row(&format!("SELECT {COLS} FROM local_peers WHERE public_key = ?1"), [hex(public_key)], peer_row)
        .optional()
        .map_err(alst_core::Error::from)?)
}

pub fn peer_by_site(store: &Store, site_id: &str) -> Result<Option<Peer>> {
    Ok(store
        .local_conn()
        .query_row(&format!("SELECT {COLS} FROM local_peers WHERE site_id = ?1 AND public_key IS NOT NULL"), [site_id], peer_row)
        .optional()
        .map_err(alst_core::Error::from)?)
}

/// Pin a device. Pairing the same device again replaces its key and address.
pub fn add_peer(store: &Store, site_id: &str, name: &str, public_key: &[u8], addr: Option<&str>, port: Option<u16>) -> Result<()> {
    let policy = serde_json::to_string(&MediaPolicy::default()).unwrap_or_else(|_| "{}".into());
    store
        .local_conn()
        .execute(
            "INSERT INTO local_peers (site_id, name, public_key, addr, port, media_policy, paired_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(site_id) DO UPDATE SET name = ?2, public_key = ?3, addr = ?4, port = ?5, paired_at = ?7",
            params![site_id, name, hex(public_key), addr, port.map(i64::from), policy, alst_core::hlc::wall_ms() as i64],
        )
        .map_err(alst_core::Error::from)?;
    Ok(())
}

/// Unpair: removes the peer on this device only.
pub fn remove_peer(store: &Store, site_id: &str) -> Result<()> {
    store.local_conn().execute("DELETE FROM local_peers WHERE site_id = ?1", [site_id]).map_err(alst_core::Error::from)?;
    Ok(())
}

pub fn set_media_policy(store: &Store, site_id: &str, policy: &MediaPolicy) -> Result<()> {
    let json = serde_json::to_string(policy).map_err(|e| Error::Protocol(e.to_string()))?;
    store
        .local_conn()
        .execute("UPDATE local_peers SET media_policy = ?1 WHERE site_id = ?2", params![json, site_id])
        .map_err(alst_core::Error::from)?;
    Ok(())
}

/// Remember when and where a peer was last reached, and what it had.
pub fn touch_peer(store: &Store, site_id: &str, addr: Option<&str>, port: Option<u16>, vector: &alst_core::sync::Vector) -> Result<()> {
    store
        .local_conn()
        .execute(
            "UPDATE local_peers SET last_seen = ?1, last_vector = ?2, addr = COALESCE(?3, addr), port = COALESCE(?4, port) WHERE site_id = ?5",
            params![alst_core::hlc::wall_ms() as i64, serde_json::to_string(vector).unwrap_or_else(|_| "{}".into()), addr, port.map(i64::from), site_id],
        )
        .map_err(alst_core::Error::from)?;
    Ok(())
}

/// Set the address to try for a peer when discovery doesn't find it (typed in by hand).
pub fn set_peer_addr(store: &Store, site_id: &str, addr: &str, port: u16) -> Result<()> {
    store
        .local_conn()
        .execute("UPDATE local_peers SET addr = ?1, port = ?2 WHERE site_id = ?3", params![addr, i64::from(port), site_id])
        .map_err(alst_core::Error::from)?;
    Ok(())
}
