//! Running an import (SPEC §10.3 steps 3–5): items one at a time, a failed item doesn't stop the rest.

use crate::manifest::Item;
use crate::plan::seed_terms;
use crate::sources::{self, Env, MAX_MEDIA_BYTES, Origin, StoredMedia};
use crate::{Error, Pack, Result};
use alst_core::{PackRow, SeedCounts, WorkPack};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub item_id: String,
    /// 1-based position among the items being imported, and how many there are.
    pub index: usize,
    pub total: usize,
    pub state: ItemState,
    pub error: Option<String>,
    pub work_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ItemState {
    Started,
    Done,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemFailure {
    pub item_id: String,
    pub error: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// `(item id, work id)` of the works created in this run.
    pub imported: Vec<(String, String)>,
    /// Items that already had a work (here or, after sync, on another device).
    pub skipped: Vec<String>,
    pub failed: Vec<ItemFailure>,
    pub vocab: Option<SeedCounts>,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Ids of the checked items.
    pub items: Vec<String>,
    pub seed_vocab: bool,
}

/// Import the chosen items. Cancelling stops before the next item (a download in flight finishes first).
pub fn run(env: &Env, pack: &Pack, opts: &RunOptions, cancel: &AtomicBool, on_progress: &dyn Fn(&Progress)) -> Result<Report> {
    env.check_lang()?;
    let m = &pack.manifest;
    {
        let mut s = env.store()?;
        s.put_pack(&PackRow {
            id: m.id.clone(),
            title: m.title.clone(),
            language: m.language.clone(),
            topic: m.topic.clone(),
            level: m.level.clone(),
            description: m.description.clone(),
            created_by: m.created_by.clone(),
            version_at: m.created_at.clone(),
            manifest: serde_json::to_string(m).map_err(|e| Error::Invalid(e.to_string()))?,
        })?;
    }
    let chosen: Vec<&Item> = m.items.iter().filter(|i| opts.items.contains(&i.id)).collect();
    let total = chosen.len();
    let mut report = Report::default();
    let emit = |item: &Item, index: usize, state: ItemState, error: Option<String>, work_id: Option<String>| {
        on_progress(&Progress { item_id: item.id.clone(), index, total, state, error, work_id });
    };
    for (n, item) in chosen.into_iter().enumerate() {
        let index = n + 1;
        if cancel.load(Ordering::SeqCst) {
            report.cancelled = true;
            break;
        }
        // Checked again here: a sync may have brought the item in since the review screen.
        if env.store()?.imported_pack_items(&m.id)?.contains(&item.id) {
            report.skipped.push(item.id.clone());
            emit(item, index, ItemState::Skipped, None, None);
            continue;
        }
        emit(item, index, ItemState::Started, None, None);
        match import_item(env, pack, item) {
            Ok(work_id) => {
                report.imported.push((item.id.clone(), work_id.clone()));
                emit(item, index, ItemState::Done, None, Some(work_id));
            }
            Err(e) => {
                let msg = e.to_string();
                // Language data missing fails every item the same way: stop and let the UI prompt.
                if let Error::LangData(_) = e {
                    return Err(e);
                }
                report.failed.push(ItemFailure { item_id: item.id.clone(), error: msg.clone() });
                emit(item, index, ItemState::Failed, Some(msg), None);
            }
        }
    }
    if opts.seed_vocab && !cancel.load(Ordering::SeqCst) {
        let terms = seed_terms(m, env.lang);
        report.vocab = Some(env.store()?.seed_terms(&m.language, &terms)?);
    }
    Ok(report)
}

fn import_item(env: &Env, pack: &Pack, item: &Item) -> Result<String> {
    let origin = Origin {
        pack: Some(WorkPack {
            pack_id: pack.manifest.id.clone(),
            pack_item: item.id.clone(),
            license: item.source.license.clone(),
            attribution: item.source.attribution.clone(),
        }),
        title: Some(item.title.clone()),
    };
    let c = &item.content;
    let url = item.source.url.as_deref().unwrap_or("");
    let work = match c.mode.as_str() {
        "fetch" => match c.strategy.as_deref().unwrap_or("") {
            "article" => sources::import_article(env, url, &origin)?,
            "rss-episode" => sources::import_podcast_episode(env, url, c.guid.as_deref().unwrap_or(""), &origin)?,
            "youtube-subs" => sources::import_youtube(env, url, &origin)?,
            "direct" => sources::import_direct(env, url, c.transcript_url.as_deref(), &origin)?,
            other => return Err(Error::Invalid(format!("unknown strategy '{other}'"))),
        },
        "embedded" => import_embedded(env, pack, item, &origin)?,
        other => return Err(Error::Invalid(format!("unknown mode '{other}'"))),
    };
    Ok(work.id)
}

fn import_embedded(env: &Env, pack: &Pack, item: &Item, origin: &Origin) -> Result<alst_core::Work> {
    const MAX_TEXT: u64 = 16 << 20;
    let c = &item.content;
    let path = c.path.as_deref().ok_or_else(|| Error::Invalid("embedded content needs a path".into()))?;
    let source_url = item.source.url.clone();
    let Some(media_path) = c.media_path.as_deref() else {
        return sources::import_file_bytes(env, path, &pack.read(path, MAX_TEXT)?, source_url, origin);
    };
    // Media from the zip into the media store, then the transcript: a cue file (`timingPath`, or `path` when it
    // is one), else the plain text of `path`.
    let tmp_id = env.media.begin()?;
    let result = (|| {
        let tmp = env.media.staged_path(&tmp_id)?;
        pack.extract(media_path, &tmp, MAX_MEDIA_BYTES)?;
        let (hash, size) = env.media.finish(&tmp_id)?;
        let mime = alst_core::media::mime_for(media_path, "").ok_or_else(|| Error::Invalid(format!("'{media_path}' is not audio or video")))?;
        let media = StoredMedia { hash, size, mime, name: media_path.rsplit('/').next().map(String::from) };
        let cue_path = c.timing_path.as_deref().or_else(|| sources::is_cue_ext(&sources::extension(path)).then_some(path));
        match cue_path {
            Some(cp) => sources::import_media_with_cues(env, &media, cp, &pack.read(cp, MAX_TEXT)?, "", source_url, origin),
            None => {
                let text = alst_text::cues::decode(&pack.read(path, MAX_TEXT)?);
                let text = if matches!(sources::extension(path).as_str(), "md" | "markdown") { alst_text::import::markdown::to_text(&text) } else { text };
                sources::import_media_with_text(env, &media, &text, "", source_url, origin)
            }
        }
    })();
    if result.is_err() {
        env.media.abort(&tmp_id);
    }
    result
}
