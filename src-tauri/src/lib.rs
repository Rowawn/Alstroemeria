//! Thin Tauri shell: commands delegate to `alst-core`, `alst-text` and `alst-dict`.

use alst_core::{PartInfo, Store, Term, TermPatch, Work};
use alst_dict::{DictInfo, LookupResult};
use alst_text::export::{ExportRow, find_examples, to_tsv};
use alst_text::import::{self, ImportedWork};
use alst_text::reader::{ViewToken, build_units, flatten};
use alst_text::{LanguageDef, LookupUrl, Matcher, TermState, Unit};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock, RwLockReadGuard};
use tauri::ipc::{InvokeBody, Request};
use tauri::{Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

mod ext;
mod media;
mod langdata;
mod pack;
mod sync;
mod update;

pub(crate) struct AppState {
    store: Arc<Mutex<Store>>,
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
    /// Deinflection rules by language (SPEC §16.5), rebuilt with `languages`.
    deinflectors: RwLock<HashMap<String, alst_text::deinflect::Deinflector>>,
    /// SQLite rowid of the last event forwarded to the UI.
    event_cursor: AtomicI64,
    /// LAN sync (SPEC §9); `None` until the service has started, or if it couldn't.
    sync: OnceLock<Arc<sync::SyncCtx>>,
    /// Language data downloads in progress, with their cancel flags (SPEC §17.3).
    langdata_jobs: Mutex<HashMap<String, Arc<std::sync::atomic::AtomicBool>>>,
    /// Content packs open on the review screen, by handle (SPEC §10.3).
    packs: Mutex<HashMap<String, pack::OpenPack>>,
    /// A `.alpack` the OS asked us to open before the UI could take it.
    pending_pack: Mutex<Option<String>>,
}

type Res<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

impl AppState {
    fn store(&self) -> Res<MutexGuard<'_, Store>> {
        self.store.lock().map_err(err)
    }

    /// What the shared importers need (`alst-pack::sources`).
    fn env<'a>(&'a self, lang: &'a LanguageDef) -> alst_pack::sources::Env<'a> {
        alst_pack::sources::Env { store: &self.store, media: &self.media, lang }
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

    /// Fail with `langdata:<id>` (the UI shows the download prompt) when `lang` needs language data that isn't installed.
    fn require_langdata(&self, lang: &LanguageDef) -> Res<()> {
        match lang.langdata_missing() {
            Some(id) => Err(format!("langdata:{id}")),
            None => Ok(()),
        }
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
    state.env(&lang).store_text(w, &alst_pack::sources::Origin::default()).map_err(err)
}

// ---- languages and library ------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LanguageInfo {
    id: String,
    name: String,
    rtl: bool,
    lookup_urls: Vec<LookupUrl>,
    /// Words are keyed by lemma (SPEC §16.3).
    lemma_keys: bool,
    /// Clicked text is looked up with `lookup_scan` (SPEC §16.6).
    scan_lookup: bool,
    /// No spaces between words (affects reader CSS).
    spaceless: bool,
}

#[tauri::command]
fn list_languages(state: State<AppState>) -> Res<Vec<LanguageInfo>> {
    let rules = state.deinflectors.read().map_err(err)?;
    Ok(state
        .languages
        .read()
        .map_err(err)?
        .iter()
        .map(|l| LanguageInfo {
            id: l.id.clone(),
            name: l.name.clone(),
            rtl: l.rtl,
            lookup_urls: l.lookup_urls.clone(),
            lemma_keys: l.term_key == alst_text::TermKey::Lemma,
            scan_lookup: rules.contains_key(&l.id),
            spaceless: l.spaceless(),
        })
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
    /// Name of the paired device the file was imported on, when the file isn't here and that device is known.
    on_device: Option<String>,
}

/// Term lookup tables for a language: key → state, plus the multi-word matcher.
fn term_tables(store: &Store, lang: &LanguageDef) -> Res<(Matcher, HashMap<String, TermState>)> {
    let rows = store.term_states(&lang.id).map_err(err)?;
    let matcher = lang.matcher(rows.iter().filter(|r| r.word_count > 1).map(|r| r.text_lc.as_str()));
    let map = rows.into_iter().map(|r| (r.text_lc, TermState { id: r.id, status: r.status })).collect();
    Ok((matcher, map))
}

fn units_for(store: &Store, lang: &LanguageDef, part_id: &str, text: &str) -> Res<Vec<Unit>> {
    Ok(view_for(store, lang, part_id, text)?.0)
}

/// Tokenized sentences of a part, from `local_token_cache` for languages where analysis is slow (SPEC §5.3).
/// The cache key covers the language definition and the part's text, so edits and extension updates recompute.
fn sentences_for(store: &Store, lang: &LanguageDef, part_id: &str, text: &str) -> Vec<Vec<alst_text::Token>> {
    if !lang.cacheable() {
        return lang.tokenize(text);
    }
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    let fingerprint = format!("{}:{h:016x}:{}", lang.fingerprint, text.len());
    if let Some(sentences) = store.token_cache_get(part_id, &fingerprint).and_then(|j| serde_json::from_str(&j).ok()) {
        return sentences;
    }
    let sentences = lang.tokenize(text);
    if let Ok(json) = serde_json::to_string(&sentences) {
        store.token_cache_put(part_id, &fingerprint, &json, 40);
    }
    sentences
}

fn view_for(store: &Store, lang: &LanguageDef, part_id: &str, text: &str) -> Res<(Vec<Unit>, Vec<ViewToken>)> {
    let (matcher, map) = term_tables(store, lang)?;
    let sentences = sentences_for(store, lang, part_id, text);
    Ok((build_units(&sentences, &matcher, &map), flatten(&sentences)))
}

#[tauri::command]
fn get_part_view(state: State<AppState>, part_id: String) -> Res<PartView> {
    let store = state.store()?;
    let part = store.get_part(&part_id).map_err(err)?;
    let work = store.get_work(&part.work_id).map_err(err)?;
    let lang = state.lang(&work.language)?;
    state.require_langdata(&lang)?;
    let (units, tokens) = view_for(&store, &lang, &part.id, &part.text)?;
    let media = match &part.media_hash {
        Some(h) => store.media_info(h).map_err(err)?.map(|m| {
            let present = state.media.exists(&m.id);
            let on_device = if present {
                None
            } else {
                store.media_origin(&m.id).ok().flatten().and_then(|site| alst_sync::peers::peer_by_site(&store, &site).ok().flatten()).map(|p| p.name)
            };
            MediaView { present, on_device, video: m.mime.starts_with("video/"), hash: m.id, mime: m.mime }
        }),
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
    /// The term as stored: the lemma for lemma-keyed languages (SPEC §16.3).
    text: String,
    /// The text as it appeared in the reader, when it differs from `text`.
    seen_as: Option<String>,
    word_count: usize,
    term: Option<Term>,
    dict: Vec<LookupResult>,
}

/// Open the editor for a word or selection: the stored term (if any) and dictionary results.
/// Logs a `term.lookup` event when `log_lookup` (the first open, not when the selection is adjusted).
#[tauri::command]
fn open_term(app: tauri::AppHandle, state: State<AppState>, language: String, text: String, log_lookup: bool) -> Res<TermView> {
    let lang = state.lang(&language)?;
    let info = lang.term_info(&text).ok_or("no word in selection")?;
    let (key, word_count) = (info.key, info.word_count);
    let mut store = state.store()?;
    let term = store.find_term(&language, &key).map_err(err)?;
    let dict = alst_dict::lookup(store.local_conn(), &language, &key, 30).map_err(err)?;
    let subject = term.as_ref().map(|t| ("term", t.id.as_str()));
    if log_lookup {
        store.emit_event("term.lookup", Some(&language), subject, &serde_json::json!({ "source": "popup" })).map_err(err)?;
        state.forward_events(&app, &store);
    }
    let seen = squash(&text);
    let seen_as = (seen != info.display).then_some(seen);
    Ok(TermView { key, text: info.display, seen_as, word_count, term, dict })
}

#[tauri::command]
fn save_term(
    app: tauri::AppHandle,
    state: State<AppState>,
    language: String,
    text: String,
    patch: TermPatch,
    parents: Option<Vec<String>>,
    key: Option<(String, usize)>,
) -> Res<Term> {
    let lang = state.lang(&language)?;
    // The editor passes the key `open_term` computed, so a lemma that re-tokenizes differently can't fork the term.
    let (key, wc) = match key {
        Some(k) => k,
        None => lang.term_key(&text).ok_or("no word in term")?,
    };
    let mut store = state.store()?;
    let term = store.save_term(&language, &squash(&text), &key, wc as i64, &patch).map_err(err)?;
    if let Some(parents) = parents {
        let mut ids = Vec::new();
        for p in parents.iter().filter(|p| !p.trim().is_empty()) {
            let pinfo = lang.term_info(p).ok_or_else(|| format!("'{p}' has no word"))?;
            let parent = store.save_term(&language, &pinfo.display, &pinfo.key, pinfo.word_count as i64, &TermPatch::default()).map_err(err)?;
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
    let units = units_for(&store, &lang, &part.id, &part.text)?;
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
    let (path, store, media) = (state.db_path.clone(), state.store.clone(), state.media.clone());
    let info = tauri::async_runtime::spawn_blocking(move || -> Res<DictInfo> {
        let mut conn = alst_core::db::open(&path).map_err(err)?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        let mut info = alst_dict::import(&mut conn, &bytes, &language, now).map_err(err)?;
        // Keep the archive and record it as a synced row so other devices can import it too (SPEC §9.6).
        let (hash, size) = media.put_bytes(&bytes, 512 << 20).map_err(err)?;
        let mut s = store.lock().map_err(err)?;
        s.register_media(&hash, "application/zip", size as i64, Some(&format!("{}.zip", info.title))).map_err(err)?;
        s.put_dictionary(&hash, &info.title, info.revision.as_deref(), &language).map_err(err)?;
        alst_dict::set_source(&conn, info.id, &hash).map_err(err)?;
        info.source_id = Some(hash);
        Ok(info)
    })
    .await
    .map_err(err)??;
    Ok(info)
}

#[tauri::command]
fn list_dictionaries(state: State<AppState>) -> Res<Vec<DictInfo>> {
    alst_dict::list(state.store()?.local_conn()).map_err(err)
}

#[tauri::command]
fn delete_dictionary(state: State<AppState>, id: i64) -> Res<()> {
    let mut store = state.store()?;
    if let Some(source) = alst_dict::source_of(store.local_conn(), id).map_err(err)? {
        store.delete_dictionary(&source).map_err(err)?;
    }
    alst_dict::delete(store.local_conn(), id).map_err(err)
}

#[tauri::command]
fn set_dictionary_enabled(state: State<AppState>, id: i64, enabled: bool) -> Res<()> {
    alst_dict::set_enabled(state.store()?.local_conn(), id, enabled).map_err(err)
}

/// Dictionary entries for the start of `text` with conjugation handling (SPEC §16.6).
#[tauri::command]
fn lookup_scan(state: State<AppState>, language: String, text: String, max_len: Option<usize>) -> Res<Vec<alst_dict::ScanResult>> {
    let rules = state.deinflectors.read().map_err(err)?;
    alst_dict::lookup_scan(state.store()?.local_conn(), &language, rules.get(&language), &text, max_len.unwrap_or(20).min(60)).map_err(err)
}

#[tauri::command]
fn dict_meta(state: State<AppState>, language: String, term: String, reading: Option<String>, mode: String) -> Res<Vec<alst_dict::MetaResult>> {
    alst_dict::meta(state.store()?.local_conn(), &language, &term, reading.as_deref(), &mode).map_err(err)
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
    let builder = tauri::Builder::default().plugin(tauri_plugin_opener::init());
    // QR pairing scans with the camera (SPEC §9.3); phones only.
    #[cfg(mobile)]
    let builder = builder.plugin(tauri_plugin_barcode_scanner::init());
    // Desktop self-update (SPEC §17.4).
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    builder
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
            let deinflectors = alst_ext::host::deinflectors(&registry, &store)?;
            let event_cursor = AtomicI64::new(store.last_event_rowid()?);
            let media = alst_core::media::MediaDir::new(dir.join("media"));
            media.clear_tmp();
            app.manage(AppState {
                media,
                store: Arc::new(Mutex::new(store)),
                db_path,
                data_dir: dir,
                registry: RwLock::new(registry),
                ext_root,
                languages: RwLock::new(languages),
                deinflectors: RwLock::new(deinflectors),
                event_cursor,
                sync: OnceLock::new(),
                langdata_jobs: Mutex::new(HashMap::new()),
                packs: Mutex::new(HashMap::new()),
                pending_pack: Mutex::new(None),
            });
            // Windows and Linux pass the file to open on the command line (macOS sends `RunEvent::Opened`).
            #[cfg(desktop)]
            if let (Some(state), Some(arg)) = (app.try_state::<AppState>(), std::env::args_os().nth(1)) {
                pack::offer_path(app.handle(), PathBuf::from(arg), &state);
            }
            if let Some(state) = app.try_state::<AppState>() {
                langdata::reconcile(&state);
            }
            sync::start(app.handle());
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
            pack::pack_open_path,
            pack::pack_open_upload,
            pack::pack_plan,
            pack::pack_run,
            pack::pack_cancel,
            pack::pack_close,
            pack::list_packs,
            pack::take_pending_pack,
            get_part_view,
            open_term,
            save_term,
            finish_part,
            import_dictionary,
            list_dictionaries,
            delete_dictionary,
            set_dictionary_enabled,
            lookup_dict,
            lookup_scan,
            dict_meta,
            open_url,
            export_tsv,
            update::update_info,
            update::update_set_auto,
            update::update_check,
            update::update_install,
            langdata::langdata_status,
            langdata::langdata_download,
            langdata::langdata_cancel,
            langdata::langdata_import,
            sync::sync_overview,
            sync::set_device_name,
            sync::pair_start,
            sync::pair_stop,
            sync::pair_by_code,
            sync::pair_by_qr,
            sync::pair_answer,
            sync::sync_now,
            sync::unpair,
            sync::set_media_policy,
            sync::set_peer_addr,
            sync::download_media,
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
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, _event| {
            // macOS: a double-clicked `.alpack` arrives as an Opened event.
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            if let tauri::RunEvent::Opened { urls } = &_event {
                if let Some(state) = _app.try_state::<AppState>() {
                    for path in urls.iter().filter_map(|u| u.to_file_path().ok()) {
                        pack::offer_path(_app, path, &state);
                    }
                }
            }
        });
}
