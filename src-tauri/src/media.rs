//! Media commands and the `alstmedia://` protocol (SPEC §8). Logic lives in `alst-core::media` and
//! `alst-text`; this only wires it to Tauri.

use crate::{AppState, Res, err, header, raw_body};
use alst_core::{Work, media as core_media};
use alst_pack::sources::{self, Origin, StoredMedia};
use alst_text::import::{rss, ytdlp};
use serde::Serialize;
use tauri::http::{Request as HttpRequest, Response, StatusCode};
use tauri::ipc::Request;
use tauri::{Manager, State, UriSchemeContext, Wry};

/// Largest slice answered to one `alstmedia://` request, so a plain GET of a big file can't fill memory.
const MAX_SLICE: u64 = 8 << 20;

// ---- upload and import -----------------------------------------------------------------------

#[tauri::command]
pub fn media_upload_begin(state: State<AppState>) -> Res<String> {
    state.media.begin().map_err(err)
}

/// Body: the next chunk. Header: `x-upload-id`.
#[tauri::command]
pub fn media_upload_chunk(state: State<AppState>, request: Request<'_>) -> Res<()> {
    let id = header(&request, "x-upload-id")?;
    state.media.append(&id, &raw_body(&request)?).map_err(err)
}

/// Finish an upload and import it with a subtitle or lyrics file. Body: the subtitle file's bytes.
/// Headers: `x-language`, `x-title` (may be empty), `x-upload-id`, `x-media-name`, `x-media-type`, `x-subtitle-name`.
#[tauri::command]
pub async fn import_media_file(state: State<'_, AppState>, request: Request<'_>) -> Res<Work> {
    let language = header(&request, "x-language")?;
    let title = header(&request, "x-title").unwrap_or_default();
    let upload = header(&request, "x-upload-id")?;
    let name = header(&request, "x-media-name").unwrap_or_default();
    let hint = header(&request, "x-media-type").unwrap_or_default();
    let sub_name = header(&request, "x-subtitle-name").unwrap_or_default();
    let sub_bytes = raw_body(&request)?.into_owned();
    let lang = state.lang(&language)?;
    let result = (|| {
        let mime = core_media::mime_for(&name, &hint).ok_or("that file is not audio or video")?;
        let (hash, size) = state.media.finish(&upload).map_err(err)?;
        let media = StoredMedia { hash, size, mime, name: Some(name.clone()) };
        sources::import_media_with_cues(&state.env(&lang), &media, &sub_name, &sub_bytes, &title, None, &Origin::default()).map_err(err)
    })();
    if result.is_err() {
        state.media.abort(&upload);
    }
    result
}

// ---- podcasts --------------------------------------------------------------------------------

#[tauri::command]
pub async fn podcast_feed(url: String) -> Res<rss::Feed> {
    tauri::async_runtime::spawn_blocking(move || rss::fetch_feed(&url)).await.map_err(err)?.map_err(err)
}

/// Import one episode: its audio goes to the media store and its transcript becomes the parts.
#[tauri::command]
pub async fn podcast_import(state: State<'_, AppState>, language: String, feed_url: String, guid: String) -> Res<Work> {
    let lang = state.lang(&language)?;
    let (store, media) = (state.store.clone(), state.media.clone());
    tauri::async_runtime::spawn_blocking(move || {
        let env = sources::Env { store: &store, media: &media, lang: &lang };
        sources::import_podcast_episode(&env, &feed_url, &guid, &Origin::default())
    })
    .await
    .map_err(err)?
    .map_err(err)
}

// ---- YouTube (desktop, user-installed yt-dlp) ---------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YtdlpStatus {
    available: bool,
    version: Option<String>,
}

#[tauri::command]
pub async fn ytdlp_status() -> YtdlpStatus {
    if cfg!(any(target_os = "android", target_os = "ios")) {
        return YtdlpStatus { available: false, version: None };
    }
    let version = tauri::async_runtime::spawn_blocking(|| ytdlp::find().and_then(|p| ytdlp::version(&p))).await.ok().flatten();
    YtdlpStatus { available: version.is_some(), version }
}

#[tauri::command]
pub async fn import_youtube(state: State<'_, AppState>, language: String, url: String) -> Res<Work> {
    let lang = state.lang(&language)?;
    let (store, media) = (state.store.clone(), state.media.clone());
    tauri::async_runtime::spawn_blocking(move || {
        let env = sources::Env { store: &store, media: &media, lang: &lang };
        sources::import_youtube(&env, &url, &Origin::default())
    })
    .await
    .map_err(err)?
    .map_err(err)
}

#[tauri::command]
pub fn log_listen(app: tauri::AppHandle, state: State<AppState>, part_id: String, seconds: f64) -> Res<()> {
    if !(seconds.is_finite() && seconds >= 1.0) {
        return Ok(());
    }
    let mut store = state.store()?;
    let part = store.get_part(&part_id).map_err(err)?;
    let work = store.get_work(&part.work_id).map_err(err)?;
    let payload = serde_json::json!({ "part_id": part_id, "seconds": seconds.round() });
    store.emit_event("media.listen", Some(&work.language), Some(("part", &part_id)), &payload).map_err(err)?;
    state.forward_events(&app, &store);
    Ok(())
}

// ---- alstmedia:// ----------------------------------------------------------------------------

fn respond(status: StatusCode, mime: &str, body: Vec<u8>, extra: &[(&str, String)]) -> Response<Vec<u8>> {
    let mut b = Response::builder()
        .status(status)
        .header("Content-Type", mime)
        .header("Accept-Ranges", "bytes")
        .header("Access-Control-Allow-Origin", "*")
        .header("Access-Control-Allow-Headers", "range")
        .header("Access-Control-Expose-Headers", "content-range, content-length, accept-ranges")
        .header("Cross-Origin-Resource-Policy", "cross-origin")
        .header("X-Content-Type-Options", "nosniff");
    for (k, v) in extra {
        b = b.header(*k, v);
    }
    b.body(body).expect("static response")
}

fn plain(status: StatusCode, msg: &str) -> Response<Vec<u8>> {
    respond(status, "text/plain", msg.as_bytes().to_vec(), &[])
}

/// `bytes=a-b`, `bytes=a-`, `bytes=-n` → (start, inclusive end or None).
fn parse_range(h: &str, total: u64) -> Option<(u64, Option<u64>)> {
    let spec = h.trim().strip_prefix("bytes=")?.split(',').next()?.trim();
    let (a, b) = spec.split_once('-')?;
    if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        return Some((total.saturating_sub(n), None));
    }
    Some((a.parse().ok()?, b.parse().ok()))
}

#[derive(Debug, PartialEq)]
struct Slice {
    start: u64,
    /// Inclusive.
    end: u64,
    partial: bool,
}

/// What to send for a `Range` header (or none) on a file of `total` bytes; `None` = not satisfiable.
/// A request without a Range still gets a bounded slice (206), which players accept.
fn plan(range_header: Option<&str>, total: u64) -> Option<Slice> {
    let range = range_header.and_then(|h| parse_range(h, total));
    let (start, end) = match range {
        Some((s, e)) => (s, e.unwrap_or(u64::MAX).min(total.saturating_sub(1))),
        None => (0, total.saturating_sub(1)),
    };
    if total == 0 || start >= total || start > end {
        return None;
    }
    let end = end.min(start + MAX_SLICE - 1);
    Some(Slice { start, end, partial: range.is_some() || end != total - 1 })
}

/// `alstmedia://localhost/<hash>` on macOS/Linux, `http://alstmedia.localhost/<hash>` on Windows/Android.
pub fn serve(ctx: UriSchemeContext<'_, Wry>, request: HttpRequest<Vec<u8>>) -> Response<Vec<u8>> {
    if request.method() == tauri::http::Method::OPTIONS {
        return respond(StatusCode::NO_CONTENT, "text/plain", vec![], &[]);
    }
    let Some(state) = ctx.app_handle().try_state::<AppState>() else {
        return plain(StatusCode::SERVICE_UNAVAILABLE, "starting");
    };
    let hash = request.uri().path().rsplit('/').next().unwrap_or("").to_string();
    if !core_media::is_hash(&hash) {
        return plain(StatusCode::BAD_REQUEST, "bad media id");
    }
    let Ok(Some(info)) = state.store.lock().map_err(|_| ()).and_then(|s| s.media_info(&hash).map_err(|_| ())) else {
        return plain(StatusCode::NOT_FOUND, "unknown media");
    };
    let total = match state.media.path(&hash).and_then(|p| std::fs::metadata(p).ok()) {
        Some(m) => m.len(),
        None => return plain(StatusCode::NOT_FOUND, "media file is missing on this device"),
    };
    let header = request.headers().get("range").and_then(|v| v.to_str().ok());
    let Some(slice) = plan(header, total) else {
        return respond(StatusCode::RANGE_NOT_SATISFIABLE, &info.mime, vec![], &[("Content-Range", format!("bytes */{total}"))]);
    };
    match state.media.read_range(&hash, slice.start, slice.end - slice.start + 1) {
        Ok((bytes, _)) => {
            let mut extra = vec![("Content-Length", bytes.len().to_string())];
            if slice.partial {
                extra.push(("Content-Range", format!("bytes {}-{}/{total}", slice.start, slice.end)));
            }
            respond(if slice.partial { StatusCode::PARTIAL_CONTENT } else { StatusCode::OK }, &info.mime, bytes, &extra)
        }
        Err(e) => plain(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_SLICE, Slice, parse_range, plan};

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, Some(99))));
        assert_eq!(parse_range("bytes=500-", 1000), Some((500, None)));
        assert_eq!(parse_range("bytes=-100", 1000), Some((900, None)));
        assert_eq!(parse_range("items=1-2", 1000), None);
    }

    #[test]
    fn plans() {
        assert_eq!(plan(None, 1000), Some(Slice { start: 0, end: 999, partial: false }));
        assert_eq!(plan(Some("bytes=0-99"), 1000), Some(Slice { start: 0, end: 99, partial: true }));
        assert_eq!(plan(Some("bytes=990-"), 1000), Some(Slice { start: 990, end: 999, partial: true }));
        assert_eq!(plan(Some("bytes=900-5000"), 1000), Some(Slice { start: 900, end: 999, partial: true }));
        assert_eq!(plan(Some("bytes=1000-"), 1000), None);
        assert_eq!(plan(Some("bytes=5-2"), 1000), None);
        assert_eq!(plan(None, 0), None);
        // Big files are cut to a bounded slice even for open-ended and range-less requests.
        let big = 100 << 20;
        assert_eq!(plan(None, big), Some(Slice { start: 0, end: MAX_SLICE - 1, partial: true }));
        assert_eq!(plan(Some("bytes=10-"), big), Some(Slice { start: 10, end: 10 + MAX_SLICE - 1, partial: true }));
    }
}
