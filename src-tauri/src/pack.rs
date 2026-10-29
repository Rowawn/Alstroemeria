//! Content pack commands (SPEC §10). Logic lives in `alst-pack`; this only wires it to Tauri.

use crate::{AppState, Res, err};
use alst_pack::sources::Env;
use alst_pack::{Caps, Pack, Plan, Report, RunOptions};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{Emitter, State};

/// A pack opened for review. `staged` is the upload file to delete when it is closed.
pub struct OpenPack {
    pack: Arc<Pack>,
    cancel: Arc<AtomicBool>,
    staged: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Opened {
    handle: String,
    plan: Plan,
}

fn plan_for(state: &AppState, pack: &Pack) -> Res<Plan> {
    let lang = state.lang(&pack.manifest.language).ok();
    let store = state.store()?;
    alst_pack::plan(pack, &store, lang.as_ref(), Caps::detect()).map_err(err)
}

fn register(state: &AppState, pack: Pack, staged: Option<String>) -> Res<Opened> {
    let plan = plan_for(state, &pack)?;
    let handle = alst_core::store::new_id();
    state.packs.lock().map_err(err)?.insert(handle.clone(), OpenPack { pack: Arc::new(pack), cancel: Arc::new(AtomicBool::new(false)), staged });
    Ok(Opened { handle, plan })
}

/// Open a `.alpack` on disk (file association, command line).
#[tauri::command]
pub async fn pack_open_path(state: State<'_, AppState>, path: String) -> Res<Opened> {
    let pack = tauri::async_runtime::spawn_blocking(move || Pack::open(Path::new(&path))).await.map_err(err)?.map_err(err)?;
    register(&state, pack, None)
}

/// Open a `.alpack` that was uploaded with `media_upload_begin` / `media_upload_chunk`.
#[tauri::command]
pub async fn pack_open_upload(state: State<'_, AppState>, upload_id: String) -> Res<Opened> {
    let path = state.media.staged_path(&upload_id).map_err(err)?;
    let opened = tauri::async_runtime::spawn_blocking(move || Pack::open(&path)).await.map_err(err)?;
    match opened {
        Ok(pack) => register(&state, pack, Some(upload_id)),
        Err(e) => {
            state.media.abort(&upload_id);
            Err(err(e))
        }
    }
}

/// The plan again (after an import, statuses have changed).
#[tauri::command]
pub fn pack_plan(state: State<AppState>, handle: String) -> Res<Plan> {
    let pack = state.packs.lock().map_err(err)?.get(&handle).map(|p| p.pack.clone()).ok_or("this pack is no longer open")?;
    plan_for(&state, &pack)
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProgressEvent {
    handle: String,
    #[serde(flatten)]
    progress: alst_pack::Progress,
}

/// Import the chosen items. Progress arrives as `alst:pack-progress`; the report lists failures.
#[tauri::command]
pub async fn pack_run(app: tauri::AppHandle, state: State<'_, AppState>, handle: String, items: Vec<String>, seed_vocab: bool) -> Res<Report> {
    let (pack, cancel) = {
        let packs = state.packs.lock().map_err(err)?;
        let p = packs.get(&handle).ok_or("this pack is no longer open")?;
        (p.pack.clone(), p.cancel.clone())
    };
    cancel.store(false, Ordering::SeqCst);
    let lang = state.lang(&pack.manifest.language)?;
    let (store, media) = (state.store.clone(), state.media.clone());
    let emit_app = app.clone();
    let h = handle.clone();
    let report = tauri::async_runtime::spawn_blocking(move || {
        let env = Env { store: &store, media: &media, lang: &lang };
        alst_pack::run(&env, &pack, &RunOptions { items, seed_vocab }, &cancel, &|p| {
            let _ = emit_app.emit("alst:pack-progress", ProgressEvent { handle: h.clone(), progress: p.clone() });
        })
    })
    .await
    .map_err(err)?
    .map_err(err)?;
    // The library may have new works and the sync service new rows to offer.
    let _ = app.emit("alst:library-changed", ());
    Ok(report)
}

#[tauri::command]
pub fn pack_cancel(state: State<AppState>, handle: String) -> Res<()> {
    if let Some(p) = state.packs.lock().map_err(err)?.get(&handle) {
        p.cancel.store(true, Ordering::SeqCst);
    }
    Ok(())
}

#[tauri::command]
pub fn pack_close(state: State<AppState>, handle: String) -> Res<()> {
    if let Some(p) = state.packs.lock().map_err(err)?.remove(&handle) {
        if let Some(id) = p.staged {
            state.media.abort(&id);
        }
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackInfo {
    id: String,
    title: String,
    language: String,
    topic: Option<String>,
    level: Option<String>,
    description: Option<String>,
    created_by: Option<String>,
    item_count: usize,
}

#[tauri::command]
pub fn list_packs(state: State<AppState>) -> Res<Vec<PackInfo>> {
    Ok(state
        .store()?
        .packs()
        .map_err(err)?
        .into_iter()
        .map(|p| {
            let item_count = serde_json::from_str::<serde_json::Value>(&p.manifest).ok().and_then(|m| m["items"].as_array().map(Vec::len)).unwrap_or(0);
            PackInfo { id: p.id, title: p.title, language: p.language, topic: p.topic, level: p.level, description: p.description, created_by: p.created_by, item_count }
        })
        .collect())
}

/// A `.alpack` path the app was started with or asked to open before the UI was ready (file association).
#[tauri::command]
pub fn take_pending_pack(state: State<AppState>) -> Option<String> {
    state.pending_pack.lock().ok()?.take()
}

/// Remember (and announce) an `.alpack` the OS asked us to open.
pub fn offer_path(app: &tauri::AppHandle, path: PathBuf, state: &AppState) {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("alpack")) {
        let p = path.to_string_lossy().into_owned();
        if let Ok(mut slot) = state.pending_pack.lock() {
            *slot = Some(p.clone());
        }
        let _ = app.emit("alst:open-pack", p);
    }
}
