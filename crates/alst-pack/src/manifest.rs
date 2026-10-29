//! The `manifest.json` of a `.alpack` and its validation (SPEC §10.1, §10.3).

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const FORMAT: &str = "alstroemeria-pack";
pub const FORMAT_VERSION: u64 = 1;
pub const MAX_MANIFEST_BYTES: u64 = 1 << 20;
pub const MAX_ITEMS: usize = 200;
pub const MAX_VOCAB: usize = 2000;

pub const KINDS: [&str; 7] = ["article", "podcast", "video", "book", "text", "subtitle", "lyrics"];
pub const STRATEGIES: [&str; 4] = ["article", "rss-episode", "youtube-subs", "direct"];
/// Licences an embedded item may carry without a warning (SPEC §10.2).
pub const EMBED_LICENSES: [&str; 5] = ["CC0-1.0", "CC-BY-4.0", "CC-BY-SA-4.0", "PD", "own"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub format: String,
    pub format_version: u64,
    pub id: String,
    pub title: String,
    pub language: String,
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub created_by: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    pub items: Vec<Item>,
    #[serde(default)]
    pub vocab: Vec<VocabEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub source: Source,
    pub content: Content,
    #[serde(default)]
    pub estimate: Option<Estimate>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub attribution: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Content {
    /// `fetch` or `embedded`.
    pub mode: String,
    #[serde(default)]
    pub strategy: Option<String>,
    #[serde(default)]
    pub guid: Option<String>,
    #[serde(default)]
    pub transcript_url: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub media_path: Option<String>,
    #[serde(default)]
    pub timing_path: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Estimate {
    #[serde(default)]
    pub words: Option<u64>,
    #[serde(default)]
    pub minutes: Option<f64>,
    #[serde(default)]
    pub transcript: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VocabEntry {
    pub text: String,
    #[serde(default)]
    pub translation: String,
    #[serde(default)]
    pub reading: String,
    #[serde(default)]
    pub notes: String,
}

impl Item {
    /// Embedded items must carry an allowlisted licence; others get a warning and start unchecked.
    pub fn license_ok(&self) -> bool {
        self.content.mode != "embedded" || self.source.license.as_deref().is_some_and(|l| EMBED_LICENSES.contains(&l))
    }
}

/// A relative path inside the zip: no empty, absolute, `..`, backslash or drive-letter components.
pub fn is_safe_path(p: &str) -> bool {
    !p.is_empty()
        && !p.starts_with('/')
        && !p.contains('\\')
        && !p.contains(':')
        && !p.contains('\0')
        && p.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

pub fn is_http_url(u: &str) -> bool {
    let l = u.trim().to_ascii_lowercase();
    (l.starts_with("http://") || l.starts_with("https://")) && l.len() > 8 && !l.contains(char::is_whitespace)
}

fn bad<T>(m: impl Into<String>) -> Result<T> {
    Err(Error::Invalid(m.into()))
}

/// Parse and validate `manifest.json`. `has_file` tells whether a path exists in the zip.
pub fn parse(raw: &[u8], has_file: &dyn Fn(&str) -> bool) -> Result<Manifest> {
    if raw.len() as u64 > MAX_MANIFEST_BYTES {
        return bad("manifest.json is larger than 1 MB");
    }
    let value: serde_json::Value = serde_json::from_slice(raw).map_err(|e| Error::Invalid(format!("manifest.json: {e}")))?;
    if value.get("format").and_then(|v| v.as_str()) != Some(FORMAT) {
        return bad("not an Alstroemeria content pack (format must be \"alstroemeria-pack\")");
    }
    let version = value.get("formatVersion").and_then(|v| v.as_u64());
    if version != Some(FORMAT_VERSION) {
        return Err(Error::Version(version.map_or_else(|| "missing".into(), |v| v.to_string())));
    }
    let m: Manifest = serde_json::from_value(value).map_err(|e| Error::Invalid(format!("manifest.json: {e}")))?;
    if m.id.trim().is_empty() || m.id.len() > 100 {
        return bad("pack id is required");
    }
    if m.title.trim().is_empty() {
        return bad("pack title is required");
    }
    if m.language.trim().is_empty() {
        return bad("pack language is required");
    }
    if m.items.len() > MAX_ITEMS {
        return bad(format!("too many items ({} > {MAX_ITEMS})", m.items.len()));
    }
    if m.vocab.len() > MAX_VOCAB {
        return bad(format!("too many vocabulary entries ({} > {MAX_VOCAB})", m.vocab.len()));
    }
    if m.vocab.iter().any(|v| v.text.trim().is_empty()) {
        return bad("a vocabulary entry has no text");
    }
    let mut seen = HashSet::new();
    for it in &m.items {
        if it.id.trim().is_empty() || !seen.insert(it.id.as_str()) {
            return bad(format!("item id '{}' is empty or repeated", it.id));
        }
        check_item(it, has_file).map_err(|e| Error::Invalid(format!("item '{}': {e}", it.id)))?;
    }
    Ok(m)
}

fn check_item(it: &Item, has_file: &dyn Fn(&str) -> bool) -> std::result::Result<(), String> {
    if !KINDS.contains(&it.kind.as_str()) {
        return Err(format!("unknown kind '{}'", it.kind));
    }
    if it.title.trim().is_empty() {
        return Err("title is required".into());
    }
    let c = &it.content;
    match c.mode.as_str() {
        "fetch" => {
            let strategy = c.strategy.as_deref().unwrap_or("");
            if !STRATEGIES.contains(&strategy) {
                return Err(format!("unknown strategy '{strategy}'"));
            }
            match it.source.url.as_deref() {
                Some(u) if is_http_url(u) => {}
                _ => return Err("source.url must be an http or https URL".into()),
            }
            if strategy == "rss-episode" && c.guid.as_deref().is_none_or(|g| g.trim().is_empty()) {
                return Err("rss-episode needs a guid".into());
            }
            if let Some(t) = c.transcript_url.as_deref().filter(|t| !is_http_url(t)) {
                return Err(format!("transcriptUrl must be an http or https URL ('{t}')"));
            }
        }
        "embedded" => {
            let Some(path) = c.path.as_deref() else { return Err("embedded content needs a path".into()) };
            for p in [Some(path), c.media_path.as_deref(), c.timing_path.as_deref()].into_iter().flatten() {
                if !is_safe_path(p) || !p.starts_with("items/") {
                    return Err(format!("unsafe or non-items path '{p}'"));
                }
                if !has_file(p) {
                    return Err(format!("'{p}' is not in the pack"));
                }
            }
            if let Some(u) = it.source.url.as_deref().filter(|u| !u.is_empty() && !is_http_url(u)) {
                return Err(format!("source.url must be an http or https URL ('{u}')"));
            }
        }
        other => return Err(format!("unknown mode '{other}'")),
    }
    Ok(())
}
