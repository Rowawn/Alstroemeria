//! Fetch-and-store orchestration shared by the import commands and the pack importer (SPEC §10.3): one path
//! per source type. Everything here blocks; callers run it off the UI thread. Rows are written through
//! `alst-core::store`.

use crate::{Error, Result};
use alst_core::media::{self as core_media, MediaDir};
use alst_core::{NewPart, NewWork, Store, Work, WorkPack};
use alst_text::LanguageDef;
use alst_text::cues;
use alst_text::import::media::{self as import_media, MediaPart, MediaWork};
use alst_text::import::{ImportedWork, article, epub, markdown, rss, ytdlp};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

/// Largest media file we import (bounds disk use and a runaway download).
pub const MAX_MEDIA_BYTES: u64 = 2 << 30;
const MAX_TEXT_BYTES: u64 = 16 << 20;

/// What an import needs: the store, the media directory and the work's language.
pub struct Env<'a> {
    pub store: &'a Mutex<Store>,
    pub media: &'a MediaDir,
    pub lang: &'a LanguageDef,
}

/// Where a work came from in a pack, and what the pack says about it (title overrides the importer's).
#[derive(Debug, Clone, Default)]
pub struct Origin {
    pub pack: Option<WorkPack>,
    pub title: Option<String>,
}

/// A media file already in the media store.
#[derive(Debug, Clone)]
pub struct StoredMedia {
    pub hash: String,
    pub size: u64,
    pub mime: String,
    pub name: Option<String>,
}

impl Env<'_> {
    pub fn store(&self) -> Result<MutexGuard<'_, Store>> {
        self.store.lock().map_err(|_| Error::Invalid("the library is busy".into()))
    }

    /// Fail with `langdata:<id>` when the language needs data that isn't installed.
    pub fn check_lang(&self) -> Result<()> {
        match self.lang.langdata_missing() {
            Some(id) => Err(Error::LangData(id.into())),
            None => Ok(()),
        }
    }

    fn new_parts(&self, parts: Vec<(Option<String>, String, Option<String>)>, media_hash: Option<&str>) -> Vec<NewPart> {
        parts
            .into_iter()
            .map(|(title, text, timing)| NewPart {
                word_count: self.lang.count_words(&text) as i64,
                title,
                text,
                media_hash: media_hash.map(String::from),
                timing,
            })
            .collect()
    }

    fn create(&self, new: NewWork, origin: &Origin) -> Result<Work> {
        let mut new = new;
        if let Some(t) = origin.title.as_deref().filter(|t| !t.trim().is_empty()) {
            new.title = t.trim().to_string();
        }
        Ok(self.store()?.create_work_in_pack(new, origin.pack.as_ref())?)
    }

    /// Store a text work (article, EPUB, plain text, Markdown, YouTube subtitles).
    pub fn store_text(&self, w: ImportedWork, origin: &Origin) -> Result<Work> {
        self.check_lang()?;
        let parts = self.new_parts(w.parts.into_iter().map(|p| (p.title, p.text, None)).collect(), None);
        self.create(NewWork { language: self.lang.id.clone(), title: w.title, kind: w.kind.into(), author: w.author, source_url: w.source_url, parts }, origin)
    }

    /// Store a work whose parts play `media_hash`; parts without times get none.
    pub fn store_media(&self, w: MediaWork, media_hash: Option<&str>, origin: &Origin) -> Result<Work> {
        self.check_lang()?;
        let parts = self.new_parts(w.parts.into_iter().map(|p| (p.title, p.text, p.timing)).collect(), media_hash);
        self.create(NewWork { language: self.lang.id.clone(), title: w.title, kind: w.kind.into(), author: w.author, source_url: w.source_url, parts }, origin)
    }

    pub fn register_media(&self, m: &StoredMedia) -> Result<()> {
        self.store()?.register_media(&m.hash, &m.mime, m.size as i64, m.name.as_deref())?;
        Ok(())
    }
}

fn fail(m: impl Into<String>) -> Error {
    Error::Invalid(m.into())
}

// ---- the sources -------------------------------------------------------------------------------

pub fn import_article(env: &Env, url: &str, origin: &Origin) -> Result<Work> {
    env.check_lang()?;
    env.store_text(article::fetch(url.trim())?, origin)
}

/// One podcast episode: its audio goes to the media store and its transcript becomes the parts. The feed is
/// fetched again so only data from the feed itself is trusted.
pub fn import_podcast_episode(env: &Env, feed_url: &str, guid: &str, origin: &Origin) -> Result<Work> {
    env.check_lang()?;
    let feed = rss::fetch_feed(feed_url)?;
    let ep = feed.episodes.iter().find(|e| e.guid == guid).cloned().ok_or_else(|| fail("episode not found in the feed"))?;
    let t = ep.best_transcript().ok_or_else(|| fail("this episode has no transcript in the feed"))?.clone();
    let audio = ep.audio_url.clone().ok_or_else(|| fail("this episode has no audio file"))?;
    let bytes = rss::fetch_bytes(&t.url, MAX_TEXT_BYTES, Duration::from_secs(60))?;
    let parts: Vec<MediaPart> = rss::transcript_parts(&t, &bytes)?;
    let reader = rss::open(&audio, Duration::from_secs(60 * 60))?;
    let (hash, size) = env.media.put_reader(reader, MAX_MEDIA_BYTES)?;
    let clean = audio.split(['?', '#']).next().unwrap_or("");
    let mime = core_media::mime_for(clean, ep.audio_type.as_deref().unwrap_or("")).unwrap_or_else(|| "audio/mpeg".into());
    env.register_media(&StoredMedia { hash: hash.clone(), size, mime: mime.clone(), name: clean.rsplit('/').next().map(String::from) })?;
    let work = MediaWork {
        title: ep.title.clone(),
        kind: if mime.starts_with("video/") { "video" } else { "audio" },
        author: Some(feed.title.clone()),
        source_url: ep.link.clone().or(ep.audio_url.clone()),
        parts,
    };
    env.store_media(work, Some(&hash), origin)
}

/// Subtitles of a YouTube video through the user's yt-dlp (desktop only).
pub fn import_youtube(env: &Env, url: &str, origin: &Origin) -> Result<Work> {
    if !youtube_available() {
        return Err(fail("YouTube import is only available on desktop with yt-dlp installed"));
    }
    env.check_lang()?;
    let bin = ytdlp::find().ok_or_else(|| fail("yt-dlp was not found; install it first"))?;
    let work = ytdlp::import(&bin, url.trim(), &env.lang.id)?;
    env.store_text(work, origin)
}

/// Whether this device can import YouTube subtitles (SPEC §8): desktop with a yt-dlp binary.
pub fn youtube_available() -> bool {
    !cfg!(any(target_os = "android", target_os = "ios")) && ytdlp::find().is_some()
}

/// A media file already in the media store plus a subtitle, lyrics or timed-transcript file.
pub fn import_media_with_cues(env: &Env, media: &StoredMedia, cue_name: &str, cue_bytes: &[u8], title: &str, source_url: Option<String>, origin: &Origin) -> Result<Work> {
    env.check_lang()?;
    let file = cues::parse(cue_name, cue_bytes, false)?;
    let kind = if cue_name.to_lowercase().ends_with(".lrc") {
        "lyrics"
    } else if media.mime.starts_with("video/") {
        "video"
    } else {
        "audio"
    };
    let name = media.name.clone().unwrap_or_default();
    let title = if title.trim().is_empty() && file.title.is_none() { name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s).to_string() } else { title.to_string() };
    env.register_media(media)?;
    env.store_media(import_media::from_cue_file(&title, kind, file, source_url), Some(&media.hash), origin)
}

/// A media file with a transcript that has no times (plain text): the player plus plain text.
pub fn import_media_with_text(env: &Env, media: &StoredMedia, text: &str, title: &str, source_url: Option<String>, origin: &Origin) -> Result<Work> {
    env.check_lang()?;
    let parts = import_media::text_parts(text);
    if parts.is_empty() {
        return Err(fail("the transcript is empty"));
    }
    env.register_media(media)?;
    let kind = if media.mime.starts_with("video/") { "video" } else { "audio" };
    env.store_media(MediaWork { title: title.to_string(), kind, author: None, source_url, parts }, Some(&media.hash), origin)
}

// ---- by file type (the `direct` and `embedded` strategies) ---------------------------------------------

/// File extension of a URL's path or a zip path, lowercase.
pub fn extension(name: &str) -> String {
    let path = name.split(['?', '#']).next().unwrap_or("");
    path.rsplit('/').next().and_then(|f| f.rsplit_once('.')).map(|(_, e)| e.to_lowercase()).unwrap_or_default()
}

pub fn is_cue_ext(ext: &str) -> bool {
    matches!(ext, "srt" | "vtt" | "lrc")
}

pub fn is_media_ext(ext: &str) -> bool {
    core_media::mime_for(&format!("x.{ext}"), "").is_some()
}

/// Text-like content that has already been read: plain text, Markdown, EPUB, or subtitles without media.
pub fn import_file_bytes(env: &Env, name: &str, bytes: &[u8], source_url: Option<String>, origin: &Origin) -> Result<Work> {
    env.check_lang()?;
    let ext = extension(name);
    let stem = name.split(['?', '#']).next().unwrap_or("").rsplit('/').next().unwrap_or("");
    let stem = stem.rsplit_once('.').map_or(stem, |(s, _)| s);
    let mut work = match ext.as_str() {
        "epub" => epub::import(bytes, stem)?,
        "md" | "markdown" => markdown::import(stem, &cues::decode(bytes)),
        "txt" => alst_text::import::from_text(stem, &cues::decode(bytes)),
        e if is_cue_ext(e) => {
            let file = cues::parse(name, bytes, false)?;
            let mut w = import_media::cues_as_text(stem, file.author.clone(), None, &file.cues);
            if e == "lrc" {
                w.kind = "lyrics";
            }
            w
        }
        "" => return Err(fail("cannot tell the file type: the address must end in .epub, .txt, .md, .srt, .vtt, .lrc or an audio or video extension")),
        _ => return Err(fail(format!("cannot import a .{ext} file"))),
    };
    if work.title.trim().is_empty() {
        work.title = stem.to_string();
    }
    if work.source_url.is_none() {
        work.source_url = source_url;
    }
    if work.parts.is_empty() {
        return Err(fail("nothing to import: no text"));
    }
    env.store_text(work, origin)
}

/// `fetch` / `direct`: choose the importer by the file type of `url`; audio or video needs `transcript_url`.
pub fn import_direct(env: &Env, url: &str, transcript_url: Option<&str>, origin: &Origin) -> Result<Work> {
    env.check_lang()?;
    let ext = extension(url);
    let get = |u: &str, max: u64, secs: u64| rss::fetch_bytes(u, max, Duration::from_secs(secs));
    if is_media_ext(&ext) {
        let t = transcript_url.ok_or_else(|| fail("an audio or video file needs a transcript URL"))?;
        let tbytes = get(t, MAX_TEXT_BYTES, 60)?;
        let reader = rss::open(url, Duration::from_secs(60 * 60))?;
        let (hash, size) = env.media.put_reader(reader, MAX_MEDIA_BYTES)?;
        let name = url.split(['?', '#']).next().and_then(|u| u.rsplit('/').next()).map(String::from);
        let mime = core_media::mime_for(url.split(['?', '#']).next().unwrap_or(""), "").unwrap_or_else(|| "audio/mpeg".into());
        let media = StoredMedia { hash, size, mime, name };
        let tname = t.split(['?', '#']).next().unwrap_or("");
        return if is_cue_ext(&extension(tname)) {
            import_media_with_cues(env, &media, tname, &tbytes, "", Some(url.into()), origin)
        } else {
            import_media_with_text(env, &media, &cues::decode(&tbytes), "", Some(url.into()), origin)
        };
    }
    let bytes = get(url, MAX_TEXT_BYTES, 60)?;
    import_file_bytes(env, url, &bytes, Some(url.into()), origin)
}
