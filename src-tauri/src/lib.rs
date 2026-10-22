//! Thin Tauri shell: commands delegate to `alst-core`, `alst-text` and `alst-dict`.

use alst_core::{NewPart, NewWork, PartInfo, Store, Term, TermPatch, Work};
use alst_dict::{DictInfo, LookupResult};
use alst_text::export::{ExportRow, find_examples, to_tsv};
use alst_text::import::{self, ImportedWork};
use alst_text::reader::build_units;
use alst_text::{LanguageDef, LookupUrl, Matcher, TermState, Unit};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use tauri::ipc::{InvokeBody, Request};
use tauri::{Manager, State};
use tauri_plugin_opener::OpenerExt;

struct AppState {
    store: Mutex<Store>,
    db_path: PathBuf,
    languages: Vec<LanguageDef>,
}

type Res<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

impl AppState {
    fn store(&self) -> Res<MutexGuard<'_, Store>> {
        self.store.lock().map_err(err)
    }

    fn lang(&self, id: &str) -> Res<&LanguageDef> {
        self.languages.iter().find(|l| l.id == id).ok_or_else(|| format!("language '{id}' is not installed"))
    }
}

fn raw_body<'a>(request: &'a Request<'_>) -> Res<&'a [u8]> {
    match request.body() {
        InvokeBody::Raw(bytes) => Ok(bytes),
        _ => Err("expected a binary body".into()),
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
        .map(|p| NewPart { word_count: lang.count_words(&p.text) as i64, title: p.title, text: p.text })
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
fn list_languages(state: State<AppState>) -> Vec<LanguageInfo> {
    state
        .languages
        .iter()
        .map(|l| LanguageInfo { id: l.id.clone(), name: l.name.clone(), rtl: l.rtl, lookup_urls: l.lookup_urls.clone() })
        .collect()
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
    let work = import::epub::import(raw_body(&request)?, &fallback).map_err(err)?;
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
}

/// Term lookup tables for a language: key → state, plus the multi-word matcher.
fn term_tables(store: &Store, language: &str) -> Res<(Matcher, HashMap<String, TermState>)> {
    let rows = store.term_states(language).map_err(err)?;
    let matcher = Matcher::new(rows.iter().filter(|r| r.word_count > 1).map(|r| r.text_lc.as_str()));
    let map = rows.into_iter().map(|r| (r.text_lc, TermState { id: r.id, status: r.status })).collect();
    Ok((matcher, map))
}

fn units_for(store: &Store, lang: &LanguageDef, text: &str) -> Res<Vec<Unit>> {
    let (matcher, map) = term_tables(store, &lang.id)?;
    Ok(build_units(&lang.tokenize(text), &matcher, &map))
}

#[tauri::command]
fn get_part_view(state: State<AppState>, part_id: String) -> Res<PartView> {
    let store = state.store()?;
    let part = store.get_part(&part_id).map_err(err)?;
    let work = store.get_work(&part.work_id).map_err(err)?;
    let lang = state.lang(&work.language)?;
    let units = units_for(&store, lang, &part.text)?;
    Ok(PartView { parts: store.list_parts(&work.id).map_err(err)?, work, part_id, rtl: lang.rtl, units })
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
fn open_term(state: State<AppState>, language: String, text: String, log_lookup: bool) -> Res<TermView> {
    let lang = state.lang(&language)?;
    let (key, word_count) = lang.term_key(&text).ok_or("no word in selection")?;
    let mut store = state.store()?;
    let term = store.find_term(&language, &key).map_err(err)?;
    let dict = alst_dict::lookup(store.local_conn(), &language, &key, 30).map_err(err)?;
    let subject = term.as_ref().map(|t| ("term", t.id.as_str()));
    if log_lookup {
        store.emit_event("term.lookup", Some(&language), subject, &serde_json::json!({ "source": "popup" })).map_err(err)?;
    }
    Ok(TermView { key, text: squash(&text), word_count, term, dict })
}

#[tauri::command]
fn save_term(
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
        return store.find_term(&language, &key).map_err(err)?.ok_or_else(|| "term vanished".into());
    }
    Ok(term)
}

/// Mark a part as read; optionally set its still-unknown words to "well known" (99).
#[tauri::command]
fn finish_part(state: State<AppState>, part_id: String, mark_known: bool, duration_ms: i64) -> Res<usize> {
    let mut store = state.store()?;
    let part = store.get_part(&part_id).map_err(err)?;
    let work = store.get_work(&part.work_id).map_err(err)?;
    let lang = state.lang(&work.language)?;
    let units = units_for(&store, lang, &part.text)?;
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
    if mark_known { store.create_terms(&work.language, &unknown, 99).map_err(err) } else { Ok(0) }
}

// ---- dictionaries ---------------------------------------------------------------------------

/// Body: the Yomitan .zip bytes. Header: `x-language`. Imports on a second connection so the
/// reader stays responsive while a large dictionary loads.
#[tauri::command]
async fn import_dictionary(state: State<'_, AppState>, request: Request<'_>) -> Res<DictInfo> {
    let language = header(&request, "x-language")?;
    state.lang(&language)?;
    let bytes = raw_body(&request)?.to_vec();
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
    let examples = find_examples(lang, texts.iter().map(String::as_str), &wanted);
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let db_path = dir.join("alstroemeria.db");
            let store = Store::open(&db_path)?;
            let languages = alst_text::bundled::languages()?;
            app.manage(AppState { store: Mutex::new(store), db_path, languages });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_languages,
            list_works,
            list_parts,
            create_work,
            import_epub,
            import_url,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
