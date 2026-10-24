//! The sync session state machine (SPEC §9.4). One connection syncs both directions, one direction at a
//! time (the initiator sends first, then the responder), so neither side can block on a full pipe.

use crate::proto::{Msg, PROTOCOL, Transport, unexpected};
use crate::{Error, Result};
use alst_core::Store;
use alst_core::sync::{Cursor, Vector, skew_ms};
use std::collections::BTreeSet;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Initiator,
    Responder,
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// This device's name, shown to the peer.
    pub name: String,
    /// Rows per batch (about; a batch finishes its last `(site, hlc)` group).
    pub batch_rows: usize,
    /// Largest accepted difference between the two clocks.
    pub max_skew_ms: i64,
}

impl SessionConfig {
    pub fn new(name: impl Into<String>) -> Self {
        SessionConfig { name: name.into(), batch_rows: 500, max_skew_ms: 3_600_000 }
    }
}

#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub site_id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub peer: PeerInfo,
    /// Tables that changed here; the UI refreshes what it shows (`sync://applied`).
    pub applied: BTreeSet<String>,
    pub sent_rows: usize,
    pub received_rows: usize,
    pub peer_vector: Vector,
    /// Wall time of the whole session including assets (set by the service).
    pub elapsed_ms: u64,
}

fn lock(store: &Mutex<Store>) -> Result<std::sync::MutexGuard<'_, Store>> {
    store.lock().map_err(|_| Error::Protocol("store lock poisoned".into()))
}

/// Run the data part of a session (hello, changes, events). Assets are exchanged afterwards by the caller.
pub fn run(store: &Mutex<Store>, t: &mut dyn Transport, role: Role, cfg: &SessionConfig) -> Result<Outcome> {
    let (peer, peer_vector) = hello(store, t, role, cfg)?;
    let mut out = Outcome { peer, applied: BTreeSet::new(), sent_rows: 0, received_rows: 0, peer_vector: peer_vector.clone(), elapsed_ms: 0 };
    match role {
        Role::Initiator => {
            send_phase(store, t, cfg, peer_vector, &mut out)?;
            recv_phase(store, t, &mut out)?;
        }
        Role::Responder => {
            recv_phase(store, t, &mut out)?;
            send_phase(store, t, cfg, peer_vector, &mut out)?;
        }
    }
    Ok(out)
}

fn my_hello(store: &Mutex<Store>, cfg: &SessionConfig) -> Result<Msg> {
    let s = lock(store)?;
    Ok(Msg::Hello {
        protocol: PROTOCOL,
        site_id: s.site_id().to_string(),
        name: cfg.name.clone(),
        schema_version: s.schema_version(),
        hlc_now: s.hlc_now(),
        vector: s.vector()?,
    })
}

/// Exchange hellos and validate the peer's. Returns who it is and its vector.
fn hello(store: &Mutex<Store>, t: &mut dyn Transport, role: Role, cfg: &SessionConfig) -> Result<(PeerInfo, Vector)> {
    let mine = my_hello(store, cfg)?;
    let theirs = match role {
        Role::Initiator => {
            t.send(&mine)?;
            t.recv()?
        }
        Role::Responder => t.recv()?,
    };
    let Msg::Hello { protocol, site_id, name, schema_version, hlc_now, vector } = theirs else {
        return unexpected(&theirs, "hello");
    };
    let verdict = {
        let s = lock(store)?;
        if protocol != PROTOCOL || schema_version != s.schema_version() {
            Some(if schema_version > s.schema_version() {
                format!("This device needs to be updated to sync with {name}.")
            } else {
                format!("{name} needs to be updated to sync with this device.")
            })
        } else if site_id == s.site_id() {
            Some("That is this same device.".to_string())
        } else if skew_ms(hlc_now).abs() > cfg.max_skew_ms {
            Some(format!("The clock on {name} is off. Set both devices to automatic time."))
        } else {
            None
        }
    };
    if let Some(reason) = verdict {
        let _ = t.send(&Msg::Refuse { reason: reason.clone() });
        return Err(Error::Refused(reason));
    }
    if role == Role::Responder {
        t.send(&mine)?;
    }
    Ok((PeerInfo { site_id, name }, vector))
}

fn send_phase(store: &Mutex<Store>, t: &mut dyn Transport, cfg: &SessionConfig, peer_vector: Vector, out: &mut Outcome) -> Result<()> {
    let mut cursor = Cursor::new(peer_vector);
    loop {
        let batch = lock(store)?.next_batch(&mut cursor, cfg.batch_rows)?;
        if batch.is_empty() {
            break;
        }
        out.sent_rows += batch.changes.len() + batch.events.len();
        t.send(&Msg::Delta(batch))?;
        match t.recv()? {
            Msg::Ack => {}
            other => return unexpected(&other, "ack"),
        }
    }
    t.send(&Msg::Done)
}

fn recv_phase(store: &Mutex<Store>, t: &mut dyn Transport, out: &mut Outcome) -> Result<()> {
    loop {
        match t.recv()? {
            Msg::Delta(batch) => {
                out.received_rows += batch.changes.len() + batch.events.len();
                let touched = lock(store)?.apply(&batch)?;
                out.applied.extend(touched);
                t.send(&Msg::Ack)?;
            }
            Msg::Done => return Ok(()),
            other => return unexpected(&other, "delta or done"),
        }
    }
}
