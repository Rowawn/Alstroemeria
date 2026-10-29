//! Opening a pack for review: what will happen to each item and to the seed vocabulary (SPEC §10.3 step 1).

use crate::manifest::{Item, Manifest};
use crate::sources;
use crate::{Pack, Result};
use alst_core::{SeedCounts, SeedTerm, Store};
use alst_text::LanguageDef;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Ready,
    /// Needs a desktop (YouTube subtitles on a device without yt-dlp); it will sync here after a desktop imports it.
    NeedsDesktop,
    AlreadyImported,
    LicenseWarning,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub tags: Vec<String>,
    /// `mode` or `mode/strategy`, e.g. `fetch/article`.
    pub how: String,
    pub domain: Option<String>,
    pub license: Option<String>,
    pub attribution: Option<String>,
    pub words: Option<u64>,
    pub minutes: Option<f64>,
    pub transcript: Option<bool>,
    pub status: Status,
    /// Whether the review screen starts with this item checked.
    pub checked: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub id: String,
    pub title: String,
    pub language: String,
    pub topic: Option<String>,
    pub level: Option<String>,
    pub description: Option<String>,
    pub created_by: Option<String>,
    pub created_at: Option<String>,
    pub items: Vec<PlanItem>,
    pub vocab_total: usize,
    pub vocab: SeedCounts,
    /// The pack's language isn't installed: nothing can be imported.
    pub language_installed: bool,
}

/// What this device can do (so the plan is testable without a yt-dlp binary).
#[derive(Debug, Clone, Copy)]
pub struct Caps {
    pub youtube: bool,
}

impl Caps {
    pub fn detect() -> Caps {
        Caps { youtube: sources::youtube_available() }
    }
}

pub fn domain(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?;
    let host = host.split(':').next()?.to_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    (!host.is_empty()).then_some(host)
}

/// Seed entries with the keys `lang` computes for them; entries with no word in them are dropped.
pub fn seed_terms(m: &Manifest, lang: &LanguageDef) -> Vec<SeedTerm> {
    m.vocab
        .iter()
        .filter_map(|v| {
            let info = lang.term_info(&v.text)?;
            Some(SeedTerm {
                text: info.display,
                text_lc: info.key,
                word_count: info.word_count as i64,
                translation: v.translation.clone(),
                reading: v.reading.clone(),
                notes: v.notes.clone(),
            })
        })
        .collect()
}

fn status_of(it: &Item, done: &std::collections::HashSet<String>, caps: Caps) -> Status {
    if done.contains(&it.id) {
        Status::AlreadyImported
    } else if it.content.mode == "fetch" && it.content.strategy.as_deref() == Some("youtube-subs") && !caps.youtube {
        Status::NeedsDesktop
    } else if !it.license_ok() {
        Status::LicenseWarning
    } else {
        Status::Ready
    }
}

/// Nothing is fetched or written. `lang` is `None` when the pack's language isn't installed.
pub fn plan(pack: &Pack, store: &Store, lang: Option<&LanguageDef>, caps: Caps) -> Result<Plan> {
    let m = &pack.manifest;
    let done = store.imported_pack_items(&m.id)?;
    let items = m
        .items
        .iter()
        .map(|it| {
            let status = status_of(it, &done, caps);
            PlanItem {
                id: it.id.clone(),
                kind: it.kind.clone(),
                title: it.title.clone(),
                tags: it.tags.clone(),
                how: match &it.content.strategy {
                    Some(s) => format!("{}/{s}", it.content.mode),
                    None => it.content.mode.clone(),
                },
                domain: it.source.url.as_deref().and_then(domain),
                license: it.source.license.clone(),
                attribution: it.source.attribution.clone(),
                words: it.estimate.as_ref().and_then(|e| e.words),
                minutes: it.estimate.as_ref().and_then(|e| e.minutes),
                transcript: it.estimate.as_ref().and_then(|e| e.transcript),
                status,
                checked: status == Status::Ready,
            }
        })
        .collect();
    let (vocab, vocab_total) = match lang {
        Some(l) => {
            let terms = seed_terms(m, l);
            (store.seed_counts(&m.language, &terms)?, terms.len())
        }
        None => (SeedCounts::default(), m.vocab.len()),
    };
    Ok(Plan {
        id: m.id.clone(),
        title: m.title.clone(),
        language: m.language.clone(),
        topic: m.topic.clone(),
        level: m.level.clone(),
        description: m.description.clone(),
        created_by: m.created_by.clone(),
        created_at: m.created_at.clone(),
        items,
        vocab_total,
        vocab,
        language_installed: lang.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains() {
        assert_eq!(domain("https://www.Example.com:8080/a?b#c").as_deref(), Some("example.com"));
        assert_eq!(domain("http://user@feeds.x.org/rss").as_deref(), Some("feeds.x.org"));
        assert_eq!(domain("nope"), None);
    }
}
