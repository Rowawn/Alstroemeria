//! Podcast RSS (SPEC §8): list a feed's episodes and find each one's audio and transcript.

use super::media::{self, MediaPart};
use crate::cues::{self, Cue};
use crate::{Error, Result};
use roxmltree::{Document, Node};
use serde::Serialize;
use std::io::Read;
use std::time::Duration;

const PODCAST_NS: &str = "podcastindex.org/namespace";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptRef {
    pub url: String,
    pub mime: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Episode {
    pub guid: String,
    pub title: String,
    pub published: Option<String>,
    pub duration: Option<String>,
    pub audio_url: Option<String>,
    pub audio_type: Option<String>,
    pub link: Option<String>,
    pub transcripts: Vec<TranscriptRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Feed {
    pub title: String,
    pub author: Option<String>,
    pub episodes: Vec<Episode>,
}

fn text(n: Node) -> Option<String> {
    let t = n.text()?.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn child<'a>(n: Node<'a, 'a>, local: &str) -> Option<Node<'a, 'a>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == local)
}

pub fn parse_feed(xml: &str) -> Result<Feed> {
    let xml = xml.trim_start_matches('\u{feff}');
    let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let doc = Document::parse_with_options(xml, opts).map_err(|e| Error::Import(format!("not a valid RSS feed: {e}")))?;
    let channel = doc.descendants().find(|n| n.is_element() && n.tag_name().name() == "channel").ok_or_else(|| Error::Import("no <channel> in feed".into()))?;
    let title = child(channel, "title").and_then(text).unwrap_or_else(|| "Podcast".into());
    let author = channel.children().find(|c| c.is_element() && c.tag_name().name() == "author").and_then(text);
    let mut episodes = Vec::new();
    for item in channel.children().filter(|c| c.is_element() && c.tag_name().name() == "item") {
        let enclosure = child(item, "enclosure");
        let audio_url = enclosure.and_then(|e| e.attribute("url")).map(str::to_string);
        let audio_type = enclosure.and_then(|e| e.attribute("type")).map(str::to_string);
        let transcripts = item
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "transcript" && c.tag_name().namespace().is_some_and(|ns| ns.contains(PODCAST_NS)))
            .filter_map(|c| Some(TranscriptRef { url: c.attribute("url")?.to_string(), mime: c.attribute("type").unwrap_or("").to_lowercase() }))
            .collect();
        let etitle = child(item, "title").and_then(text).unwrap_or_else(|| "Episode".into());
        let guid = child(item, "guid").and_then(text).or_else(|| audio_url.clone()).unwrap_or_else(|| etitle.clone());
        episodes.push(Episode {
            guid,
            title: etitle,
            published: child(item, "pubDate").and_then(text),
            duration: child(item, "duration").and_then(text),
            audio_url,
            audio_type,
            link: child(item, "link").and_then(text),
            transcripts,
        });
    }
    if episodes.is_empty() {
        return Err(Error::Import("the feed has no episodes".into()));
    }
    Ok(Feed { title, author, episodes })
}

impl Episode {
    /// The best transcript we can read: SRT/VTT/JSON (timed) before text/HTML (untimed).
    pub fn best_transcript(&self) -> Option<&TranscriptRef> {
        let rank = |t: &TranscriptRef| match kind_of(t) {
            TranscriptKind::Srt | TranscriptKind::Vtt => 0,
            TranscriptKind::Json => 1,
            TranscriptKind::Html => 3,
            TranscriptKind::Text => 2,
            TranscriptKind::Unknown => 9,
        };
        self.transcripts.iter().filter(|t| rank(t) < 9).min_by_key(|t| rank(t))
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum TranscriptKind {
    Srt,
    Vtt,
    Json,
    Html,
    Text,
    Unknown,
}

fn kind_of(t: &TranscriptRef) -> TranscriptKind {
    let url = t.url.split(['?', '#']).next().unwrap_or("").to_lowercase();
    match t.mime.as_str() {
        "application/x-subrip" | "application/srt" | "text/srt" => TranscriptKind::Srt,
        "text/vtt" => TranscriptKind::Vtt,
        "application/json" => TranscriptKind::Json,
        "text/html" => TranscriptKind::Html,
        "text/plain" => TranscriptKind::Text,
        _ if url.ends_with(".srt") => TranscriptKind::Srt,
        _ if url.ends_with(".vtt") => TranscriptKind::Vtt,
        _ if url.ends_with(".json") => TranscriptKind::Json,
        _ if url.ends_with(".html") || url.ends_with(".htm") => TranscriptKind::Html,
        _ if url.ends_with(".txt") => TranscriptKind::Text,
        _ => TranscriptKind::Unknown,
    }
}

/// Parts for a downloaded transcript of the kind `t` declared.
pub fn transcript_parts(t: &TranscriptRef, bytes: &[u8]) -> Result<Vec<MediaPart>> {
    match kind_of(t) {
        TranscriptKind::Srt | TranscriptKind::Vtt => Ok(media::cue_parts(&cues::parse(&t.url, bytes, false)?.cues)),
        TranscriptKind::Json => Ok(media::cue_parts(&json_cues(&cues::decode(bytes))?)),
        TranscriptKind::Html => {
            let page = scraper::Html::parse_document(&cues::decode(bytes));
            let paras = super::html::paragraphs(page.root_element());
            non_empty(media::text_parts(&paras.join("\n\n")))
        }
        TranscriptKind::Text => non_empty(media::text_parts(&cues::decode(bytes))),
        TranscriptKind::Unknown => Err(Error::Import("unsupported transcript format".into())),
    }
}

fn non_empty(parts: Vec<MediaPart>) -> Result<Vec<MediaPart>> {
    if parts.is_empty() { Err(Error::Import("the transcript is empty".into())) } else { Ok(parts) }
}

/// Podcasting 2.0 JSON transcript: `{"segments":[{"startTime":1.5,"endTime":3,"body":"…"}]}`.
fn json_cues(s: &str) -> Result<Vec<Cue>> {
    let v: serde_json::Value = serde_json::from_str(s).map_err(|e| Error::Import(format!("bad JSON transcript: {e}")))?;
    let secs = |x: Option<&serde_json::Value>| x.and_then(|x| x.as_f64()).map(|f| (f * 1000.0).round() as i64);
    let mut out: Vec<Cue> = Vec::new();
    for seg in v.get("segments").and_then(|s| s.as_array()).into_iter().flatten() {
        let (Some(start), Some(body)) = (secs(seg.get("startTime")), seg.get("body").and_then(|b| b.as_str())) else { continue };
        let body = body.split_whitespace().collect::<Vec<_>>().join(" ");
        if body.is_empty() {
            continue;
        }
        let end = secs(seg.get("endTime")).unwrap_or(start + 3000).max(start);
        out.push(Cue { start, end, text: body });
    }
    if out.is_empty() {
        return Err(Error::Import("no segments in the JSON transcript".into()));
    }
    Ok(out)
}

// ---- network ---------------------------------------------------------------------------------

/// Open an http(s) URL for reading. `timeout` bounds the whole request.
pub fn open(url: &str, timeout: Duration) -> Result<Box<dyn Read + Send>> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(Error::Import("only http(s) URLs can be imported".into()));
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .user_agent("Mozilla/5.0 (compatible; Alstroemeria)")
        .build()
        .into();
    let resp = agent.get(url).call().map_err(|e| Error::Import(format!("download failed: {e}")))?;
    Ok(Box::new(resp.into_body().into_reader()))
}

pub fn fetch_bytes(url: &str, max: u64, timeout: Duration) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    open(url, timeout)?.take(max).read_to_end(&mut out).map_err(|e| Error::Import(e.to_string()))?;
    Ok(out)
}

pub fn fetch_feed(url: &str) -> Result<Feed> {
    let bytes = fetch_bytes(url.trim(), 16 * 1024 * 1024, Duration::from_secs(30))?;
    parse_feed(&cues::decode(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd" xmlns:p="https://podcastindex.org/namespace/1.0">
<channel><title>Charlas</title><itunes:author>Ana</itunes:author>
<item><title>Uno</title><guid>g1</guid><pubDate>Mon, 01 Jan 2024 10:00:00 GMT</pubDate><itunes:duration>12:34</itunes:duration>
 <enclosure url="https://x.test/1.mp3" type="audio/mpeg" length="5"/>
 <p:transcript url="https://x.test/1.html" type="text/html"/>
 <p:transcript url="https://x.test/1.srt" type="application/x-subrip"/></item>
<item><title>Dos</title><enclosure url="https://x.test/2.mp3" type="audio/mpeg"/></item>
</channel></rss>"#;

    #[test]
    fn parses_episodes_enclosures_and_transcripts() {
        let f = parse_feed(FEED).unwrap();
        assert_eq!((f.title.as_str(), f.author.as_deref(), f.episodes.len()), ("Charlas", Some("Ana"), 2));
        let e = &f.episodes[0];
        assert_eq!((e.guid.as_str(), e.audio_url.as_deref(), e.duration.as_deref()), ("g1", Some("https://x.test/1.mp3"), Some("12:34")));
        assert_eq!(e.best_transcript().unwrap().url, "https://x.test/1.srt");
        assert_eq!(f.episodes[1].guid, "https://x.test/2.mp3");
        assert!(f.episodes[1].best_transcript().is_none());
        assert!(parse_feed("<rss><channel><title>x</title></channel></rss>").is_err());
        assert!(parse_feed("not xml").is_err());
    }

    #[test]
    fn transcript_formats() {
        let r = |url: &str, mime: &str| TranscriptRef { url: url.into(), mime: mime.into() };
        let srt = transcript_parts(&r("a.srt", ""), b"1\n00:00:01,000 --> 00:00:02,000\nhola\n").unwrap();
        assert!(srt[0].timing.is_some());
        let json = br#"{"version":"1.0.0","segments":[{"speaker":"A","startTime":1.5,"endTime":3,"body":"hola  mundo"},{"startTime":4,"body":""}]}"#;
        let j = transcript_parts(&r("a", "application/json"), json).unwrap();
        assert_eq!(j[0].text, "hola mundo");
        assert!(j[0].timing.as_deref().unwrap().contains("\"start\":1500"));
        let h = transcript_parts(&r("a", "text/html"), b"<html><body><p>Uno dos.</p><p>Tres.</p></body></html>").unwrap();
        assert!(h[0].timing.is_none() && h[0].text.contains("Tres."));
        assert!(transcript_parts(&r("a.bin", ""), b"x").is_err());
    }
}
