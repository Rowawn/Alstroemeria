//! Language data commands (SPEC §17.3): status, download with progress, import from file, install
//! from the media store. The logic (pins, checksums, unzip, resume) lives in `alst_text::langdata`.

use crate::{AppState, Res, err};
use alst_text::langdata::{self, Status};
use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{Emitter, State};

/// Largest package we will store (the pinned size is checked too).
const MAX_PACKAGE: u64 = 512 << 20;

#[derive(Serialize, Clone)]
struct Progress {
    id: String,
    done: u64,
    total: u64,
}

/// Keep a verified package in the media store (so paired devices can fetch it, SPEC §9.6) and record its row.
fn keep_as_media(state: &AppState, zip: &std::path::Path, id: &str) -> Res<()> {
    let file = std::fs::File::open(zip).map_err(err)?;
    let (hash, size) = state.media.put_reader(file, MAX_PACKAGE).map_err(err)?;
    state.store()?.register_media(&hash, langdata::MIME, size as i64, Some(&format!("langdata-{id}.zip"))).map_err(err)?;
    Ok(())
}

#[tauri::command]
pub fn langdata_status(state: State<AppState>) -> Vec<Status> {
    langdata::status(&state.data_dir)
}

/// Download and install the pinned package `id`. Progress arrives as `langdata://progress`.
#[tauri::command]
pub async fn langdata_download(app: tauri::AppHandle, state: State<'_, AppState>, id: String) -> Res<()> {
    let pin = langdata::pinned_for(&id).ok_or("unknown language data")?;
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut jobs = state.langdata_jobs.lock().map_err(err)?;
        if jobs.contains_key(&id) {
            return Err("already downloading".into());
        }
        jobs.insert(id.clone(), cancel.clone());
    }
    let dir = state.data_dir.clone();
    let emit_app = app.clone();
    let pin2 = pin.clone();
    let result = tauri::async_runtime::spawn_blocking(move || -> Res<std::path::PathBuf> {
        let mut last = 0u64;
        let mut progress = |done: u64, total: u64| {
            // Roughly 1% steps keep the event stream light.
            if done == total || done >= last + total / 100 {
                last = done;
                let _ = emit_app.emit("langdata://progress", Progress { id: pin2.id.clone(), done, total });
            }
        };
        let zip = langdata::download(&dir, &pin2, &mut progress, &cancel).map_err(err)?;
        langdata::install_zip(&dir, &zip).map_err(err)?;
        Ok(zip)
    })
    .await
    .map_err(err);
    state.langdata_jobs.lock().map_err(err)?.remove(&id);
    let zip = result??;
    let kept = keep_as_media(&state, &zip, &id);
    let _ = std::fs::remove_file(&zip);
    let _ = app.emit("langdata://changed", &id);
    kept
}

#[tauri::command]
pub fn langdata_cancel(state: State<AppState>, id: String) {
    if let Some(flag) = state.langdata_jobs.lock().ok().and_then(|j| j.get(&id).cloned()) {
        flag.store(true, Ordering::Relaxed);
    }
}

/// Install a package the UI uploaded with `media_upload_*` ("Import from file"). Returns the package id.
#[tauri::command]
pub async fn langdata_import(app: tauri::AppHandle, state: State<'_, AppState>, upload_id: String) -> Res<String> {
    let (hash, _) = state.media.finish(&upload_id).map_err(err).inspect_err(|_| state.media.abort(&upload_id))?;
    let path = state.media.path(&hash).ok_or("upload failed")?;
    let dir = state.data_dir.clone();
    let p = path.clone();
    let id = tauri::async_runtime::spawn_blocking(move || langdata::install_zip(&dir, &p)).await.map_err(err)?;
    match id {
        Ok(id) => {
            let name = format!("langdata-{id}.zip");
            state.store()?.register_media(&hash, langdata::MIME, std::fs::metadata(&path).map_or(0, |m| m.len() as i64), Some(&name)).map_err(err)?;
            let _ = app.emit("langdata://changed", &id);
            Ok(id)
        }
        Err(e) => {
            // Not a package we accept: don't leave the upload in the media store unless something else uses it.
            if state.store()?.media_info(&hash).map_err(err)?.is_none() {
                let _ = std::fs::remove_file(path);
            }
            Err(err(e))
        }
    }
}

/// Point the tokenizers at installed packages, and install any whose zip is already in the media store
/// (it arrived from a paired device). Cheap when nothing changed. Returns whether something was installed.
pub fn reconcile(state: &AppState) -> bool {
    langdata::refresh(&state.data_dir);
    let mut installed_any = false;
    for pin in langdata::pinned() {
        if langdata::installed(&state.data_dir, &pin.id).is_some() {
            continue;
        }
        let Some(zip) = state.media.path(&pin.sha256).filter(|p| p.is_file()) else { continue };
        match langdata::install_zip(&state.data_dir, &zip) {
            Ok(_) => installed_any = true,
            Err(e) => eprintln!("langdata: {}: {e}", pin.id),
        }
    }
    installed_any
}
