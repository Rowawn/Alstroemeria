//! Subtitle and lyrics parsing (SPEC §8): SRT, WebVTT and LRC become a list of timed cues, and a cue list
//! becomes part text plus the `parts.timing` JSON (SPEC §4.2).

use crate::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub start: i64,
    pub end: i64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CueFile {
    pub title: Option<String>,
    pub author: Option<String>,
    pub cues: Vec<Cue>,
}

/// `parts.timing`: cue times in ms, `a`/`b` = UTF-16 code-unit range of the cue in the part text (the same offsets as tokens and reader units).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Timing {
    pub v: u32,
    pub cues: Vec<TimedCue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimedCue {
    pub start: i64,
    pub end: i64,
    pub a: usize,
    pub b: usize,
}

/// Decode a text file of unknown encoding: BOM (UTF-8/16), then UTF-8, then windows-1252.
pub fn decode(bytes: &[u8]) -> String {
    if let Some((enc, _)) = encoding_rs::Encoding::for_bom(bytes) {
        return enc.decode(bytes).0.into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::WINDOWS_1252.decode(bytes).0.into_owned(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Srt,
    Vtt,
    Lrc,
}

impl Format {
    /// Detect by file name, falling back to content.
    pub fn detect(filename: &str, text: &str) -> Option<Format> {
        let lower = filename.to_lowercase();
        let head = text.trim_start_matches('\u{feff}').trim_start();
        if lower.ends_with(".vtt") || head.starts_with("WEBVTT") {
            Some(Format::Vtt)
        } else if lower.ends_with(".srt") {
            Some(Format::Srt)
        } else if lower.ends_with(".lrc") {
            Some(Format::Lrc)
        } else if head.contains("-->") {
            Some(Format::Srt)
        } else if head.lines().take(20).any(|l| l.trim_start().starts_with('[') && parse_lrc_stamp(l.trim_start()).is_some()) {
            Some(Format::Lrc)
        } else {
            None
        }
    }
}

/// Parse a subtitle or lyrics file. `rolling` removes the repeated lines of rolling auto-captions (VTT).
pub fn parse(filename: &str, bytes: &[u8], rolling: bool) -> Result<CueFile> {
    let text = decode(bytes);
    let format = Format::detect(filename, &text).ok_or_else(|| Error::Import("not a recognised .srt, .vtt or .lrc file".into()))?;
    let file = match format {
        Format::Srt => parse_srt(&text),
        Format::Vtt => parse_vtt(&text, rolling),
        Format::Lrc => parse_lrc(&text),
    };
    if file.cues.is_empty() {
        return Err(Error::Import("no cues found in the subtitle file".into()));
    }
    Ok(file)
}

// ---- SRT / VTT -------------------------------------------------------------------------------

/// `00:01:02,500`, `01:02.500`, `1:02:03.5`. Returns ms.
fn parse_time(s: &str) -> Option<i64> {
    let s = s.trim().replace(',', ".");
    let (hms, frac) = match s.split_once('.') {
        Some((a, b)) => (a, b),
        None => (s.as_str(), ""),
    };
    let parts: Vec<&str> = hms.split(':').collect();
    let nums: Vec<i64> = parts.iter().map(|p| p.trim().parse::<i64>().ok()).collect::<Option<_>>()?;
    let secs = match nums.as_slice() {
        [m, s] => m * 60 + s,
        [h, m, s] => h * 3600 + m * 60 + s,
        _ => return None,
    };
    let ms = if frac.is_empty() {
        0
    } else {
        let digits: String = frac.chars().take_while(char::is_ascii_digit).take(3).collect();
        if digits.is_empty() {
            return None;
        }
        format!("{digits:0<3}").parse::<i64>().ok()?
    };
    Some(secs * 1000 + ms)
}

/// Parse a `start --> end [settings]` line.
fn parse_timing_line(line: &str) -> Option<(i64, i64)> {
    let (a, rest) = line.split_once("-->")?;
    let b = rest.trim().split_whitespace().next()?;
    Some((parse_time(a)?, parse_time(b)?))
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest.find(';').filter(|&e| e <= 8);
        let rep = end.and_then(|e| match &rest[1..e] {
            "amp" => Some("&".to_string()),
            "lt" => Some("<".into()),
            "gt" => Some(">".into()),
            "quot" => Some("\"".into()),
            "apos" => Some("'".into()),
            "nbsp" | "lrm" | "rlm" => Some(" ".into()),
            n if n.starts_with("#x") || n.starts_with("#X") => u32::from_str_radix(&n[2..], 16).ok().and_then(char::from_u32).map(String::from),
            n if n.starts_with('#') => n[1..].parse::<u32>().ok().and_then(char::from_u32).map(String::from),
            _ => None,
        });
        match (rep, end) {
            (Some(r), Some(e)) => {
                out.push_str(&r);
                rest = &rest[e + 1..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Remove markup (`<i>`, `<c.x>`, `<00:00:01.000>`, `{\an8}`), decode entities, collapse whitespace.
fn clean_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    let mut in_brace = false;
    for c in s.chars() {
        match c {
            '<' if !in_brace => in_tag = true,
            '>' if in_tag => in_tag = false,
            '{' if !in_tag => in_brace = true,
            '}' if in_brace => in_brace = false,
            _ if in_tag || in_brace => {}
            _ => out.push(c),
        }
    }
    decode_entities(&out).split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_blocks(text: &str, vtt: bool, rolling: bool) -> Vec<Cue> {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n").replace('\r', "\n");
    let mut cues: Vec<Cue> = Vec::new();
    let mut prev_lines: Vec<String> = Vec::new();
    for block in text.split("\n\n") {
        let lines: Vec<&str> = block.lines().collect();
        let Some(ti) = lines.iter().position(|l| l.contains("-->")) else { continue };
        if vtt && lines.first().is_some_and(|l| l.starts_with("NOTE") || l.starts_with("STYLE") || l.starts_with("REGION")) {
            continue;
        }
        let Some((start, end)) = parse_timing_line(lines[ti]) else { continue };
        let mut body: Vec<String> = lines[ti + 1..].iter().map(|l| clean_line(l)).filter(|l| !l.is_empty()).collect();
        if rolling {
            let before = body.clone();
            body.retain(|l| !prev_lines.contains(l));
            prev_lines = before;
        }
        if body.is_empty() {
            continue;
        }
        cues.push(Cue { start, end: end.max(start), text: body.join(" ") });
    }
    cues
}

pub fn parse_srt(text: &str) -> CueFile {
    CueFile { cues: sorted(parse_blocks(text, false, false)), ..Default::default() }
}

pub fn parse_vtt(text: &str, rolling: bool) -> CueFile {
    CueFile { cues: sorted(parse_blocks(text, true, rolling)), ..Default::default() }
}

fn sorted(mut cues: Vec<Cue>) -> Vec<Cue> {
    cues.sort_by_key(|c| c.start);
    cues
}

// ---- LRC -------------------------------------------------------------------------------------

/// `[mm:ss.xx]` at the start of `s`: (ms, rest).
fn parse_lrc_stamp(s: &str) -> Option<(i64, &str)> {
    let inner = s.strip_prefix('[')?;
    let close = inner.find(']')?;
    let t = &inner[..close];
    if !t.chars().next()?.is_ascii_digit() || !t.contains(':') {
        return None;
    }
    Some((parse_time(t)?, &inner[close + 1..]))
}

/// LRC lyrics. A cue ends where the next one starts (the last one gets 5 s); empty lines are gaps.
pub fn parse_lrc(text: &str) -> CueFile {
    let mut file = CueFile::default();
    let mut offset = 0i64;
    let mut lines: Vec<(i64, String)> = Vec::new();
    for raw in text.trim_start_matches('\u{feff}').lines() {
        let mut rest = raw.trim();
        let mut stamps = Vec::new();
        while let Some((t, r)) = parse_lrc_stamp(rest) {
            stamps.push(t);
            rest = r.trim_start();
        }
        if stamps.is_empty() {
            if let Some(tag) = rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                if let Some((k, v)) = tag.split_once(':') {
                    let v = v.trim();
                    match k.trim().to_lowercase().as_str() {
                        "ti" if !v.is_empty() => file.title = Some(v.into()),
                        "ar" if !v.is_empty() => file.author = Some(v.into()),
                        "offset" => offset = v.parse().unwrap_or(0),
                        _ => {}
                    }
                }
            }
            continue;
        }
        let line = clean_line(rest);
        for t in stamps {
            lines.push((t, line.clone()));
        }
    }
    lines.sort_by_key(|(t, _)| *t);
    for (i, (t, text)) in lines.iter().enumerate() {
        if text.is_empty() {
            continue;
        }
        let start = (t - offset).max(0);
        let end = lines.get(i + 1).map_or(start + 5000, |(n, _)| (n - offset).max(start));
        file.cues.push(Cue { start, end, text: text.clone() });
    }
    file
}

// ---- cues → parts ----------------------------------------------------------------------------

/// Text and timing of one part made of consecutive cues.
#[derive(Debug, Clone, PartialEq)]
pub struct CuePart {
    pub title: String,
    pub text: String,
    pub timing: Timing,
}

pub fn fmt_clock(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// Group cues into parts of about `max_chars` characters, cut at cue boundaries. Each part's text is its
/// cues joined by `\n`; `timing` records every cue's byte range in that text.
pub fn into_parts(cues: &[Cue], max_chars: usize) -> Vec<CuePart> {
    let mut parts = Vec::new();
    let mut i = 0;
    while i < cues.len() {
        let mut text = String::new();
        let mut timed = Vec::new();
        let mut chars = 0;
        let mut units = 0;
        let first = &cues[i];
        let mut last = first;
        while i < cues.len() && (timed.is_empty() || chars + cues[i].text.chars().count() < max_chars) {
            let c = &cues[i];
            if !text.is_empty() {
                text.push('\n');
                units += 1;
            }
            let a = units;
            text.push_str(&c.text);
            units += c.text.encode_utf16().count();
            chars += c.text.chars().count() + 1;
            timed.push(TimedCue { start: c.start, end: c.end, a, b: units });
            last = c;
            i += 1;
        }
        parts.push(CuePart { title: format!("{} – {}", fmt_clock(first.start), fmt_clock(last.end)), text, timing: Timing { v: 1, cues: timed } });
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRT: &str = "\u{feff}1\r\n00:00:01,000 --> 00:00:03,500\r\n<i>Hola,</i> ¿qué tal?\r\n\r\n2\r\n00:00:04,000 --> 00:00:06,250\r\nMuy bien,\r\ngracias &amp; tú.\r\n\r\n3\r\n00:01:00,5 --> 00:01:02,000\r\n{\\an8}Adiós\r\n";

    #[test]
    fn srt_basics() {
        let f = parse("a.srt", SRT.as_bytes(), false).unwrap();
        assert_eq!(f.cues.len(), 3);
        assert_eq!(f.cues[0], Cue { start: 1000, end: 3500, text: "Hola, ¿qué tal?".into() });
        assert_eq!(f.cues[1].text, "Muy bien, gracias & tú.");
        assert_eq!(f.cues[2].start, 60_500);
        assert_eq!(f.cues[2].text, "Adiós");
    }

    #[test]
    fn vtt_with_header_notes_settings_and_tags() {
        let vtt = "WEBVTT - title\n\nNOTE a comment\n\nintro\n00:01.000 --> 00:02.000 align:start position:0%\nHola <c.yellow>mundo</c>\n\n1:00:00.000 --> 1:00:01.250\n<00:00:01.000><c>uno</c><00:00:01.500> dos\n";
        let f = parse("x.vtt", vtt.as_bytes(), false).unwrap();
        assert_eq!(f.cues.len(), 2);
        assert_eq!((f.cues[0].start, f.cues[0].end, f.cues[0].text.as_str()), (1000, 2000, "Hola mundo"));
        assert_eq!((f.cues[1].start, f.cues[1].end, f.cues[1].text.as_str()), (3_600_000, 3_601_250, "uno dos"));
    }

    #[test]
    fn vtt_rolling_captions_dedupe() {
        let vtt = "WEBVTT\n\n00:00:00.000 --> 00:00:02.000\nhola a todos\n\n00:00:02.000 --> 00:00:02.010\nhola a todos\n\n00:00:02.010 --> 00:00:04.000\nhola a todos\nbienvenidos al canal\n\n00:00:04.000 --> 00:00:06.000\nbienvenidos al canal\nhoy hablamos\n";
        let f = parse("a.vtt", vtt.as_bytes(), true).unwrap();
        let t: Vec<&str> = f.cues.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(t, ["hola a todos", "bienvenidos al canal", "hoy hablamos"]);
        // Without the flag nothing is dropped.
        assert_eq!(parse("a.vtt", vtt.as_bytes(), false).unwrap().cues.len(), 4);
    }

    #[test]
    fn lrc_tags_offset_multiple_stamps_and_gaps() {
        let lrc = "[ti:Canción]\n[ar:Alguien]\n[offset:500]\n[00:01.00]Uno\n[00:03.50][01:00.00]<00:03.50>Dos <00:04.00>tres\n[00:06.00]\n[00:08.25]Cuatro\n";
        let f = parse("s.lrc", lrc.as_bytes(), false).unwrap();
        assert_eq!((f.title.as_deref(), f.author.as_deref()), (Some("Canción"), Some("Alguien")));
        let c: Vec<(i64, i64, &str)> = f.cues.iter().map(|c| (c.start, c.end, c.text.as_str())).collect();
        assert_eq!(c, [(500, 3000, "Uno"), (3000, 5500, "Dos tres"), (7750, 59_500, "Cuatro"), (59_500, 64_500, "Dos tres")]);
    }

    #[test]
    fn detects_format_by_content_and_rejects_garbage() {
        assert_eq!(Format::detect("noext", "WEBVTT\n"), Some(Format::Vtt));
        assert_eq!(Format::detect("noext", "1\n00:00:01,000 --> 00:00:02,000\nx"), Some(Format::Srt));
        assert_eq!(Format::detect("noext", "[00:01.00]hi"), Some(Format::Lrc));
        assert!(parse("a.txt", b"just words", false).is_err());
        assert!(parse("a.srt", b"1\nnothing\n", false).is_err());
    }

    #[test]
    fn decodes_latin1_and_utf16() {
        let latin1 = b"1\n00:00:01,000 --> 00:00:02,000\nni\xf1o\n";
        assert_eq!(parse("a.srt", latin1, false).unwrap().cues[0].text, "niño");
        let mut utf16 = vec![0xff, 0xfe];
        for u in "1\n00:00:01,000 --> 00:00:02,000\nñ\n".encode_utf16() {
            utf16.extend(u.to_le_bytes());
        }
        assert_eq!(parse("a.srt", &utf16, false).unwrap().cues[0].text, "ñ");
    }

    #[test]
    fn parts_record_utf16_offsets_and_split_at_cue_boundaries() {
        let mut cues: Vec<Cue> = (0..6).map(|i| Cue { start: i * 1000, end: i * 1000 + 900, text: format!("niño {i} ¿sí?") }).collect();
        cues[1].text = "😀 hola".into(); // astral char: 2 UTF-16 units, 4 bytes
        let one = into_parts(&cues, 1000);
        assert_eq!(one.len(), 1);
        let u16s: Vec<u16> = one[0].text.encode_utf16().collect();
        let t = &one[0].timing.cues[3];
        assert_eq!(String::from_utf16(&u16s[t.a..t.b]).unwrap(), "niño 3 ¿sí?");
        let t = &one[0].timing.cues[1];
        assert_eq!(String::from_utf16(&u16s[t.a..t.b]).unwrap(), "😀 hola");
        let many = into_parts(&cues, 30);
        assert!(many.len() > 1);
        assert_eq!(many.iter().map(|p| p.timing.cues.len()).sum::<usize>(), 6);
        // Times stay absolute and part titles show the range.
        assert_eq!(many[1].timing.cues[0].start, many[0].timing.cues.len() as i64 * 1000);
        assert!(many[0].title.starts_with("0:00 – "));
    }
}
