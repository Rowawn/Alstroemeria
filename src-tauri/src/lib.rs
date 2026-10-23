//! Thin Tauri shell: commands delegate to `alst-core`, `alst-text` and `alst-dict`.

use alst_core::{NewPart, NewWork, PartInfo, Store, Term, TermPatch, Work};
use alst_dict::{DictInfo, LookupResult};
use alst_text::export::{ExportRow, find_examples, to_tsv};
use alst_text::import::{self, ImportedWork};
use alst_text::reader::{ViewToken, build_units, flatten};
use alst_text::{LanguageDef, LookupUrl, Matcher, TermState, Unit};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard};
use tauri::ipc::{InvokeBody, Request};
use tauri::{Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

mod ext;
mod media;

pub(crate) struct AppState {
    store: Mutex<Store>,
    db_path: PathBuf,
    /// App data folder (`ALST_DATA_DIR` overrides it, for tests and the self-test).
    data_dir: PathBuf,
    /// Installed and bundled extensions. Lock order: `registry` before `store`.
    registry: RwLock<alst_ext::Registry>,
    ext_root: PathBuf,
    /// Content-addressed media files (SPEC §8).
    media: alst_core::media::MediaDir,
    /// Language definitions from enabled language extensions; rebuilt when extensions change.
    languages: RwLock<Vec<LanguageDef>>,
    /// SQLite rowid of the last event forwarded to the UI.
    event_cursor: AtomicI64,
}

type Res<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

impl AppState {
    fn store(&self) -> Res<MutexGuard<'_, Store>> {
        self.store.lock().map_err(err)
    }

    fn registry(&self) -> Res<RwLockReadGuard<'_, alst_ext::Registry>> {
        self.registry.read().map_err(err)
    }

    fn lang(&self, id: &str) -> Res<LanguageDef> {
        self.languages
            .read()
            .map_err(err)?
            .iter()
            .find(|l| l.id == id)
            .cloned()
            .ok_or_else(|| format!("language '{id}' is not installed"))
    }

    /// Send events written since the last call to the UI (`alst:event`), so listeners see core events live.
    fn forward_events(&self, app: &tauri::AppHandle, store: &Store) {
        let after = self.event_cursor.load(Ordering::SeqCst);
        if let Ok(rows) = store.events_after(after, 500) {
            for (rowid, ev) in rows {
                let _ = app.emit("alst:event", &ev);
                self.event_cursor.fetch_max(rowid, Ordering::SeqCst);
            }
        }
    }
}

/// Binary request body. Desktop sends it raw; Android's IPC falls back to JSON for typed arrays, as an
/// array of numbers or an object keyed by index, so accept those too.
pub(crate) fn raw_body<'a>(request: &'a Request<'_>) -> Res<std::borrow::Cow<'a, [u8]>> {
    use std::borrow::Cow;
    let bad = || "expected a binary body".to_string();
    match request.body() {
        InvokeBody::Raw(bytes) => Ok(Cow::Borrowed(bytes)),
        InvokeBody::Json(serde_json::Value::Array(a)) => {
            a.iter().map(|v| v.as_u64().and_then(|n| u8::try_from(n).ok()).ok_or_else(bad)).collect::<Res<Vec<u8>>>().map(Cow::Owned)
        }
        InvokeBody::Json(serde_json::Value::Object(o)) => {
            let mut items: Vec<(usize, u8)> = o
                .iter()
                .map(|(k, v)| Ok((k.parse::<usize>().map_err(|_| bad())?, v.as_u64().and_then(|n| u8::try_from(n).ok()).ok_or_else(bad)?)))
                .collect::<Res<_>>()?;
            items.sort_unstable_by_key(|(i, _)| *i);
            Ok(Cow::Owned(items.into_iter().map(|(_, b)| b).collect()))
        }
        _ => Err(bad()),
    }
}

fn header(request: &Request<'_>, name: &str) -> Res<String> {
    let v = request.headers().get(name).and_then(|v| v.to_str().ok()).ok_or_else(|| format!("missing header {name}"))?;
    Ok(percent_encoding::percent_decode_str(v).decode_utf8_lossy().into_owned())
}

fn store_import(state: &AppState, language: &str, w: ImportedWork) -> Res<Work> {
    let lang = state.lang(language)?;
    let parts = w
        .parts
        .into_iter()
        .map(|p| NewPart { word_count: lang.count_words(&p.text) as i64, title: p.title, text: p.text, media_hash: None, timing: None })
        .collect();
    state
        .store()?
        .create_work(NewWork {
            language: language.into(),
            title: w.title,
            kind: w.kind.into(),
            author: w.author,
            source_url: w.source_url,
            parts,
        })
        .map_err(err)
}

/// Store a work whose parts play `media_hash` (a media-store id); parts without times get none.
fn store_media_work(state: &AppState, language: &str, w: alst_text::import::media::MediaWork, media_hash: Option<&str>) -> Res<Work> {
    let lang = state.lang(language)?;
    let parts = w
        .parts
        .into_iter()
        .map(|p| NewPart {
            word_count: lang.count_words(&p.text) as i64,
            title: p.title,
            text: p.text,
            media_hash: media_hash.map(String::from),
            timing: p.timing,
        })
        .collect();
    state
        .store()?
        .create_work(NewWork { language: language.into(), title: w.title, kind: w.kind.into(), author: w.author, source_url: w.source_url, parts })
        .map_err(err)
}

// ---- languages and library ------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LanguageInfo {
    id: String,
    name: String,
    rtl: bool,
    lookup_urls: Vec<LookupUrl>,
}

#[tauri::command]
fn list_languages(state: State<AppState>) -> Res<Vec<LanguageInfo>> {
    Ok(state
        .languages
        .read()
        .map_err(err)?
        .iter()
        .map(|l| LanguageInfo { id: l.id.clone(), name: l.name.clone(), rtl: l.rtl, lookup_urls: l.lookup_urls.clone() })
        .collect())
}

#[tauri::command]
fn list_works(state: State<AppState>) -> Res<Vec<Work>> {
    state.store()?.list_works().map_err(err)
}

#[tauri::command]
fn list_parts(state: State<AppState>, work_id: String) -> Res<Vec<PartInfo>> {
    state.store()?.list_parts(&work_id).map_err(err)
}

#[tauri::command]
fn create_work(state: State<AppState>, language: String, title: String, text: String) -> Res<Work> {
    store_import(&state, &language, import::from_text(&title, &text))
}

/// Body: the .epub bytes. Headers: `x-language`, `x-filename` (percent-encoded).
#[tauri::command]
async fn import_epub(state: State<'_, AppState>, request: Request<'_>) -> Res<Work> {
    let language = header(&request, "x-language")?;
    let filename = header(&request, "x-filename").unwrap_or_default();
    let fallback = filename.trim_end_matches(".epub").to_string();
    let work = import::epub::import(&raw_body(&request)?, &fallback).map_err(err)?;
    store_import(&state, &language, work)
}

#[tauri::command]
async fn import_url(state: State<'_, AppState>, language: String, url: String) -> Res<Work> {
    state.lang(&language)?;
    let work = tauri::async_runtime::spawn_blocking(move || import::article::fetch(url.trim()))
        .await
        .map_err(err)?
        .map_err(err)?;
    store_import(&state, &language, work)
}

// ---- reader ---------------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PartView {
    work: Work,
    parts: Vec<PartInfo>,
    part_id: String,
    rtl: bool,
    units: Vec<Unit>,
    tokens: Vec<ViewToken>,
    /// The part's media file and cue list (SPEC §8); `timing` is `parts.timing` parsed.
    media: Option<MediaView>,
    timing: Option<serde_json::Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MediaView {
    hash: String,
    mime: String,
    video: bool,
    /// False when the file isn't on this device (e.g. synced metadata without the file).
    present: bool,
}

/// Term lookup tables for a language: key → state, plus the multi-word matcher.
fn term_tables(store: &Store, language: &str) -> Res<(Matcher, HashMap<String, TermState>)> {
    let rows = store.term_states(language).map_err(err)?;
    let matcher = Matcher::new(rows.iter().filter(|r| r.word_count > 1).map(|r| r.text_lc.as_str()));
    let map = rows.into_iter().map(|r| (r.text_lc, TermState { id: r.id, status: r.status })).collect();
    Ok((matcher, map))
}

fn units_for(store: &Store, lang: &LanguageDef, text: &str) -> Res<Vec<Unit>> {
    Ok(view_for(store, lang, text)?.0)
}

fn view_for(store: &Store, lang: &LanguageDef, text: &str) -> Res<(Vec<Unit>, Vec<ViewToken>)> {
    let (matcher, map) = term_tables(store, &lang.id)?;
    let sentences = lang.tokenize(text);
    Ok((build_units(&sentences, &matcher, &map), flatten(&sentences)))
}

#[tauri::command]
fn get_part_view(state: State<AppState>, part_id: String) -> Res<PartView> {
    let store = state.store()?;
    let part = store.get_part(&part_id).map_err(err)?;
    let work = store.get_work(&part.work_id).map_err(err)?;
    let lang = state.lang(&work.language)?;
    let (units, tokens) = view_for(&store, &lang, &part.text)?;
    let media = match &part.media_hash {
        Some(h) => store.media_info(h).map_err(err)?.map(|m| MediaView { present: state.media.exists(&m.id), video: m.mime.starts_with("video/"), hash: m.id, mime: m.mime }),
        None => None,
    };
    let timing = part.timing.as_deref().and_then(|t| serde_json::from_str(t).ok());
    Ok(PartView { parts: store.list_parts(&work.id).map_err(err)?, work, part_id, rtl: lang.rtl, units, tokens, media, timing })
}

fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TermView {
    key: String,
    text: String,
    word_count: usize,
    term: Option<Term>,
    dict: Vec<LookupResult>,
}

/// Open the editor for a word or selection: the stored term (if any) and dictionary results.
/// Logs a `term.lookup` event when `log_lookup` (the first open, not when the selection is adjusted).
#[tauri::command]
fn open_term(app: tauri::AppHandle, state: State<AppState>, language: String, text: String, log_lookup: bool) -> Res<TermView> {
    let lang = state.lang(&language)?;
    let (key, word_count) = lang.term_key(&text).ok_or("no word in selection")?;
    let mut store = state.store()?;
    let term = store.find_term(&language, &key).map_err(err)?;
    let dict = alst_dict::lookup(store.local_conn(), &language, &key, 30).map_err(err)?;
    let subject = term.as_ref().map(|t| ("term", t.id.as_str()));
    if log_lookup {
        store.emit_event("term.lookup", Some(&language), subject, &serde_json::json!({ "source": "popup" })).map_err(err)?;
        state.forward_events(&app, &store);
    }
    Ok(TermView { key, text: squash(&text), word_count, term, dict })
}

#[tauri::command]
fn save_term(
    app: tauri::AppHandle,
    state: State<AppState>,
    language: String,
    text: String,
    patch: TermPatch,
    parents: Option<Vec<String>>,
) -> Res<Term> {
    let lang = state.lang(&language)?;
    let (key, wc) = lang.term_key(&text).ok_or("no word in term")?;
    let mut store = state.store()?;
    let term = store.save_term(&language, &squash(&text), &key, wc as i64, &patch).map_err(err)?;
    if let Some(parents) = parents {
        let mut ids = Vec::new();
        for p in parents.iter().filter(|p| !p.trim().is_empty()) {
            let (pkey, pwc) = lang.term_key(p).ok_or_else(|| format!("'{p}' has no word"))?;
            let parent = store.save_term(&language, &squash(p), &pkey, pwc as i64, &TermPatch::default()).map_err(err)?;
            if parent.id != term.id && !ids.contains(&parent.id) {
                ids.push(parent.id);
            }
        }
        store.set_term_parents(&term.id, &ids).map_err(err)?;
        state.forward_events(&app, &store);
        return store.find_term(&language, &key).map_err(err)?.ok_or_else(|| "term vanished".into());
    }
    state.forward_events(&app, &store);
    Ok(term)
}

/// Mark a part as read; optionally set its still-unknown words to "well known" (99).
#[tauri::command]
fn finish_part(app: tauri::AppHandle, state: State<AppState>, part_id: String, mark_known: bool, duration_ms: i64) -> Res<usize> {
    let mut store = state.store()?;
    let part = store.get_part(&part_id).map_err(err)?;
    let work = store.get_work(&part.work_id).map_err(err)?;
    let lang = state.lang(&work.language)?;
    let units = units_for(&store, &lang, &part.text)?;
    let mut seen = HashSet::new();
    let unknown: Vec<(String, String, i64)> = units
        .iter()
        .filter(|u| u.is_word && u.term_id.is_none())
        .filter_map(|u| {
            let key = u.key.clone()?;
            seen.insert(key.clone()).then(|| (squash(&u.text), key, u.word_count as i64))
        })
        .collect();
    let words = units.iter().map(|u| u.word_count.max(usize::from(u.is_word))).sum::<usize>();
    let payload = serde_json::json!({
        "words_read": words,
        "unknown_count": unknown.len(),
        "duration_ms": duration_ms.max(0),
    });
    store.emit_event("part.read", Some(&work.language), Some(("part", &part_id)), &payload).map_err(err)?;
    let created = if mark_known { store.create_terms(&work.language, &unknown, 99).map_err(err)? } else { 0 };
    state.forward_events(&app, &store);
    Ok(created)
}

// ---- dictionaries ---------------------------------------------------------------------------

/// Body: the Yomitan .zip bytes. Header: `x-language`. Imports on a second connection so the
/// reader stays responsive while a large dictionary loads.
#[tauri::command]
async fn import_dictionary(state: State<'_, AppState>, request: Request<'_>) -> Res<DictInfo> {
    let language = header(&request, "x-language")?;
    state.lang(&language)?;
    let bytes = raw_body(&request)?.into_owned();
    let path = state.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut conn = alst_core::db::open(&path).map_err(err)?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        alst_dict::import(&mut conn, &bytes, &language, now).map_err(err)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
fn list_dictionaries(state: State<AppState>) -> Res<Vec<DictInfo>> {
    alst_dict::list(state.store()?.local_conn()).map_err(err)
}

#[tauri::command]
fn delete_dictionary(state: State<AppState>, id: i64) -> Res<()> {
    alst_dict::delete(state.store()?.local_conn(), id).map_err(err)
}

#[tauri::command]
fn set_dictionary_enabled(state: State<AppState>, id: i64, enabled: bool) -> Res<()> {
    alst_dict::set_enabled(state.store()?.local_conn(), id, enabled).map_err(err)
}

#[tauri::command]
fn lookup_dict(state: State<AppState>, language: String, query: String) -> Res<Vec<LookupResult>> {
    alst_dict::lookup(state.store()?.local_conn(), &language, &query, 30).map_err(err)
}

#[tauri::command]
fn open_url(app: tauri::AppHandle, url: String) -> Res<()> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("only http(s) links can be opened".into());
    }
    app.opener().open_url(url, None::<&str>).map_err(err)
}

// ---- export ---------------------------------------------------------------------------------

#[derive(Serialize)]
struct TsvExport {
    tsv: String,
    count: usize,
}

/// Terms with `min_status <= status <= max_status` as Anki TSV, with an example sentence from the library.
#[tauri::command]
fn export_tsv(state: State<AppState>, language: String, min_status: i64, max_status: i64) -> Res<TsvExport> {
    let lang = state.lang(&language)?;
    let store = state.store()?;
    let terms = store.list_terms(&language, min_status, max_status).map_err(err)?;
    let wanted: HashSet<String> = terms.iter().map(|t| t.text_lc.clone()).collect();
    let texts = store.part_texts(&language).map_err(err)?;
    let examples = find_examples(&lang, texts.iter().map(String::as_str), &wanted);
    let rows: Vec<ExportRow> = terms
        .iter()
        .map(|t| ExportRow {
            text: t.text.clone(),
            translation: t.translation.clone().unwrap_or_default(),
            notes: t.notes.clone().unwrap_or_default(),
            parents: t.parents.iter().map(|p| p.text.clone()).collect(),
            sentence: examples.get(&t.text_lc).cloned().unwrap_or_default(),
            status: t.status,
        })
        .collect();
    Ok(TsvExport { tsv: to_tsv(&language, &rows), count: rows.len() })
}

/// Write an export to the user's Downloads folder (desktop). Returns the path.
#[tauri::command]
fn save_export(app: tauri::AppHandle, filename: String, content: String) -> Res<String> {
    let dir = app.path().download_dir().map_err(|_| "no Downloads folder on this device; use Copy instead".to_string())?;
    let safe: String = filename.chars().map(|c| if c.is_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect();
    let path = dir.join(safe);
    std::fs::write(&path, content).map_err(err)?;
    Ok(path.display().to_string())
}

/// The on-device self-test (ui/src/lib/ext/selftest.ts) runs only if a file named `selftest` exists in the
/// app data folder, so it can be switched on without env vars on Android. The file's text selects what to leave
/// on screen afterwards (`reader` or `extensions`), for taking screenshots.
#[tauri::command]
fn selftest_enabled(state: State<AppState>) -> Option<String> {
    std::fs::read_to_string(state.data_dir.join("selftest")).ok().map(|s| s.trim().to_string())
}

#[tauri::command]
fn selftest_report(state: State<AppState>, name: String, ok: bool, detail: String) {
    let line = format!("SELFTEST {} {name}: {detail}", if ok { "PASS" } else { "FAIL" });
    eprintln!("{line}");
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(state.data_dir.join("selftest.log")) {
        let _ = writeln!(f, "{line}");
    }
}

/// A loopback HTTP server that counts connections, so the self-test can prove a sandboxed worker or panel made
/// none (a failed request alone can't tell "blocked by CSP" from "no network"). Returns its port.
#[tauri::command]
fn selftest_canary_start() -> Res<u16> {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    static HITS: std::sync::OnceLock<(u16, std::sync::Arc<std::sync::atomic::AtomicU32>)> = std::sync::OnceLock::new();
    let (port, _) = HITS.get_or_init(|| {
        let listener = TcpListener::bind("127.0.0.1:0").expect("canary bind");
        let port = listener.local_addr().expect("addr").port();
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter = hits.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                counter.fetch_add(1, Ordering::SeqCst as _);
                let mut conn = conn;
                let mut buf = [0u8; 512];
                let _ = conn.read(&mut buf);
                let _ = conn.write_all(b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: 2\r\nConnection: close\r\n\r\nhi");
            }
        });
        // Positive control: a connection from here must be counted, then the count is reset.
        let _ = TcpStream::connect(("127.0.0.1", port));
        std::thread::sleep(std::time::Duration::from_millis(200));
        hits.store(0, Ordering::SeqCst as _);
        (port, hits)
    });
    CANARY.set(HITS.get().expect("init").1.clone()).ok();
    Ok(*port)
}

static CANARY: std::sync::OnceLock<std::sync::Arc<std::sync::atomic::AtomicU32>> = std::sync::OnceLock::new();

#[tauri::command]
fn selftest_canary_hits() -> u32 {
    CANARY.get().map_or(0, |c| c.load(Ordering::SeqCst as _))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .register_uri_scheme_protocol("alstext", ext::serve)
        .register_uri_scheme_protocol("alstmedia", media::serve)
        .setup(|app| {
            let dir = match std::env::var_os("ALST_DATA_DIR") {
                Some(d) => PathBuf::from(d),
                None => app.path().app_data_dir()?,
            };
            let db_path = dir.join("alstroemeria.db");
            let ext_root = dir.join("extensions");
            let store = Store::open(&db_path)?;
            let registry = alst_ext::Registry::load(&alst_ext::registry::BUNDLED, Some(&ext_root));
            let languages = alst_ext::host::language_defs(&registry, &store)?;
            let event_cursor = AtomicI64::new(store.last_event_rowid()?);
            let media = alst_core::media::MediaDir::new(dir.join("media"));
            media.clear_tmp();
            app.manage(AppState {
                media,
                store: Mutex::new(store),
                db_path,
                data_dir: dir,
                registry: RwLock::new(registry),
                ext_root,
                languages: RwLock::new(languages),
                event_cursor,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_languages,
            list_works,
            list_parts,
            create_work,
            import_epub,
            import_url,
            media::media_upload_begin,
            media::media_upload_chunk,
            media::import_media_file,
            media::podcast_feed,
            media::podcast_import,
            media::ytdlp_status,
            media::import_youtube,
            media::log_listen,
            get_part_view,
            open_term,
            save_term,
            finish_part,
            import_dictionary,
            list_dictionaries,
            delete_dictionary,
            set_dictionary_enabled,
            lookup_dict,
            open_url,
            export_tsv,
            save_export,
            ext::list_extensions,
            ext::resolve_extensions,
            ext::inspect_extension,
            ext::install_extension,
            ext::uninstall_extension,
            ext::set_extension_enabled,
            ext::set_extension_languages,
            ext::set_extension_settings,
            ext::set_provider,
            ext::set_decorator_order,
            ext::ext_call,
            selftest_enabled,
            selftest_report,
            selftest_canary_start,
            selftest_canary_hits,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
