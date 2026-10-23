//! Importers that produce timed parts: subtitle/lyrics files, podcast transcripts (SPEC §8).
//! Like the other importers they return plain data; the shell attaches the media file and stores it.

use super::{ImportedPart, ImportedWork, PART_CHARS, split_parts};
use crate::cues::{self, Cue, CueFile};

#[derive(Debug, Clone, PartialEq)]
pub struct MediaPart {
    pub title: Option<String>,
    pub text: String,
    /// `parts.timing` JSON, absent for a transcript without times.
    pub timing: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaWork {
    pub title: String,
    pub kind: &'static str,
    pub author: Option<String>,
    pub source_url: Option<String>,
    pub parts: Vec<MediaPart>,
}

/// Parts for a cue file. `kind` is chosen from the media (`audio`, `video`) or `lyrics` for LRC.
pub fn from_cue_file(title: &str, kind: &'static str, file: CueFile, source_url: Option<String>) -> MediaWork {
    MediaWork {
        title: file.title.clone().filter(|_| title.trim().is_empty()).unwrap_or_else(|| title.trim().to_string()),
        kind,
        author: file.author,
        source_url,
        parts: cue_parts(&file.cues),
    }
}

pub fn cue_parts(cues: &[Cue]) -> Vec<MediaPart> {
    cues::into_parts(cues, PART_CHARS)
        .into_iter()
        .map(|p| MediaPart { title: Some(p.title), text: p.text, timing: Some(serde_json::to_string(&p.timing).expect("timing serializes")) })
        .collect()
}

/// Plain text parts without times (for transcripts that have none).
pub fn text_parts(text: &str) -> Vec<MediaPart> {
    split_parts(text, PART_CHARS).into_iter().map(|text| MediaPart { title: None, text, timing: None }).collect()
}

/// A text-only work from cues (YouTube subtitles: no media to play, so times are dropped and the
/// cues become running text).
pub fn cues_as_text(title: &str, author: Option<String>, source_url: Option<String>, cues: &[Cue]) -> ImportedWork {
    let text = cues.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join(" ");
    ImportedWork {
        title: title.trim().to_string(),
        kind: "subtitle",
        author,
        source_url,
        parts: split_parts(&text, PART_CHARS).into_iter().map(|text| ImportedPart { title: None, text }).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lrc_title_is_used_only_when_none_given() {
        let f = cues::parse("a.lrc", b"[ti:Canci\xc3\xb3n]\n[ar:X]\n[00:01.00]hola\n", false).unwrap();
        let w = from_cue_file("", "lyrics", f.clone(), None);
        assert_eq!((w.title.as_str(), w.author.as_deref()), ("Canción", Some("X")));
        assert_eq!(from_cue_file("Mi título", "lyrics", f, None).title, "Mi título");
        assert!(w.parts[0].timing.as_deref().unwrap().contains("\"cues\""));
    }

    #[test]
    fn subtitle_text_work_joins_cues() {
        let cues = vec![Cue { start: 0, end: 1, text: "Hola.".into() }, Cue { start: 1, end: 2, text: "Adiós.".into() }];
        let w = cues_as_text("T", None, None, &cues);
        assert_eq!(w.parts[0].text, "Hola. Adiós.");
    }
}
