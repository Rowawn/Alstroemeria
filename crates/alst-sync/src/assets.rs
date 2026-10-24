//! Asset exchange after the data sync (SPEC §9.6): `have`/`want`, then chunked transfer with a sha256
//! check before a file enters the media store. Media follows the receiving device's per-peer policy;
//! dictionary and extension archives always transfer.

use crate::peers;
use crate::proto::{Msg, Transport, unexpected};
use crate::service::Observer;
use crate::session::Role;
use crate::{Error, Result};
use alst_core::Store;
use alst_core::media::MediaDir;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

const CHUNK: u64 = 256 * 1024;
/// Hash lists are sent whole; this bounds one message at about 1 MB.
const MAX_LIST: usize = 12_000;

fn lock(store: &Mutex<Store>) -> std::sync::MutexGuard<'_, Store> {
    store.lock().unwrap_or_else(|e| e.into_inner())
}

fn io_err(e: alst_core::Error) -> Error {
    Error::Store(e)
}

/// Files this device has: media rows whose file is on disk.
fn present(media: Option<&MediaDir>, store: &Mutex<Store>) -> Result<Vec<String>> {
    let Some(media) = media else { return Ok(vec![]) };
    let rows = lock(store).media_list()?;
    let mut v: Vec<String> = rows.into_iter().filter(|r| media.exists(&r.id)).map(|r| r.id).collect();
    v.truncate(MAX_LIST);
    Ok(v)
}

/// What to ask a peer for: files it has, that we have a row for and lack, that our policy allows.
fn wants(media: Option<&MediaDir>, store: &Mutex<Store>, peer_site: &str, peer_have: &HashSet<String>) -> Result<Vec<String>> {
    let Some(media) = media else { return Ok(vec![]) };
    let s = lock(store);
    let policy = peers::peer_by_site(&s, peer_site)?.map(|p| p.media_policy).unwrap_or_default();
    let mut always: HashSet<String> = s.dictionaries()?.into_iter().map(|d| d.media_hash).collect();
    always.extend(s.extension_packages()?.into_iter().map(|e| e.media_hash));
    Ok(s.media_list()?
        .into_iter()
        .filter(|r| peer_have.contains(&r.id) && !media.exists(&r.id))
        .filter(|r| always.contains(&r.id) || policy.allows(&r.mime, r.size))
        .map(|r| r.id)
        .collect())
}

/// Send the files in `wanted` that we have, each followed by `FileEnd`, or `Missing`.
pub fn serve(media: Option<&MediaDir>, t: &mut dyn Transport, wanted: &[String]) -> Result<()> {
    for hash in wanted {
        let Some(media) = media.filter(|m| m.exists(hash)) else {
            t.send(&Msg::Missing { hash: hash.clone() })?;
            continue;
        };
        let mut offset = 0u64;
        loop {
            let (data, total) = media.read_range(hash, offset, CHUNK).map_err(io_err)?;
            if !data.is_empty() {
                let n = data.len() as u64;
                t.send(&Msg::Chunk { hash: hash.clone(), offset, data })?;
                offset += n;
            }
            if offset >= total {
                break;
            }
        }
        t.send(&Msg::FileEnd { hash: hash.clone() })?;
    }
    Ok(())
}

/// Receive every file in `expected` (until each has ended or been reported missing). Returns the hashes stored.
pub fn receive(
    media: &MediaDir,
    t: &mut dyn Transport,
    expected: &[String],
    sizes: &HashMap<String, i64>,
    obs: &dyn Observer,
) -> Result<Vec<String>> {
    let mut open: HashMap<String, (String, u64)> = HashMap::new(); // hash -> (upload id, bytes so far)
    let mut pending: HashSet<&str> = expected.iter().map(String::as_str).collect();
    let mut stored = Vec::new();
    let total = expected.len();
    let result = (|| -> Result<()> {
        while !pending.is_empty() {
            match t.recv()? {
                Msg::Chunk { hash, offset, data } => {
                    if !pending.contains(hash.as_str()) {
                        return Err(Error::Protocol("unexpected file".into()));
                    }
                    if !open.contains_key(&hash) {
                        open.insert(hash.clone(), (media.begin().map_err(io_err)?, 0));
                    }
                    let (id, got) = open.get_mut(&hash).expect("just inserted");
                    if offset != *got {
                        return Err(Error::Protocol("file chunk out of order".into()));
                    }
                    *got += data.len() as u64;
                    if sizes.get(&hash).is_some_and(|s| *got > *s as u64) {
                        return Err(Error::Protocol("file larger than announced".into()));
                    }
                    media.append(id, &data).map_err(io_err)?;
                }
                Msg::FileEnd { hash } => {
                    // A file with no chunks is an empty file.
                    let id = match open.remove(&hash) {
                        Some((id, _)) => id,
                        None if pending.contains(hash.as_str()) => media.begin().map_err(io_err)?,
                        None => return Err(Error::Protocol("unexpected file".into())),
                    };
                    let (got, _) = media.finish(&id).map_err(io_err)?;
                    if got != hash {
                        // Corrupt or tampered: it is stored under its real hash (harmless, content-addressed), never the claimed one.
                        return Err(Error::Protocol("file failed its checksum".into()));
                    }
                    pending.remove(hash.as_str());
                    stored.push(hash);
                    obs.assets(stored.len(), total);
                }
                Msg::Missing { hash } => {
                    pending.remove(hash.as_str());
                }
                other => return unexpected(&other, "file data"),
            }
        }
        Ok(())
    })();
    for (id, _) in open.values() {
        media.abort(id);
    }
    result.map(|()| stored)
}

fn sizes_of(store: &Mutex<Store>) -> HashMap<String, i64> {
    lock(store).media_list().map(|rows| rows.into_iter().map(|r| (r.id, r.size)).collect()).unwrap_or_default()
}

/// Run the asset part of a session. Both sides call this after the data phase; it is turn-based like it.
pub fn exchange(
    media: Option<&MediaDir>,
    store: &Mutex<Store>,
    t: &mut dyn Transport,
    role: Role,
    peer_site: &str,
    obs: &dyn Observer,
) -> Result<()> {
    let mine = Msg::Have { hashes: present(media, store)? };
    let theirs = match role {
        Role::Initiator => {
            t.send(&mine)?;
            t.recv()?
        }
        Role::Responder => {
            let m = t.recv()?;
            t.send(&mine)?;
            m
        }
    };
    let Msg::Have { hashes } = theirs else { return unexpected(&theirs, "have") };
    let ask = wants(media, store, peer_site, &hashes.into_iter().collect())?;
    let sizes = sizes_of(store);
    let recv_all = |t: &mut dyn Transport| -> Result<()> {
        if let Some(m) = media {
            receive(m, t, &ask, &sizes, obs)?;
        }
        Ok(())
    };
    let their_want = |t: &mut dyn Transport| -> Result<Vec<String>> {
        match t.recv()? {
            Msg::Want { hashes } => Ok(hashes),
            other => unexpected(&other, "want"),
        }
    };
    match role {
        Role::Initiator => {
            t.send(&Msg::Want { hashes: ask.clone() })?;
            recv_all(t)?;
            let w = their_want(t)?;
            serve(media, t, &w)?;
        }
        Role::Responder => {
            let w = their_want(t)?;
            serve(media, t, &w)?;
            t.send(&Msg::Want { hashes: ask.clone() })?;
            recv_all(t)?;
        }
    }
    Ok(())
}
