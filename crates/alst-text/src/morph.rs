//! Morphological engine (SPEC §16): Lindera segmentation plus word grouping for Japanese (IPADIC)
//! and Korean (ko-dic). Output is a list of byte-ranged groups; `lang.rs` turns them into tokens.

use crate::{Error, Result};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Morph {
    Ja,
    Ko,
}

impl Morph {
    pub fn from_engine(engine: &str) -> Option<Morph> {
        match engine {
            "lindera-ipadic" => Some(Morph::Ja),
            "lindera-ko-dic" => Some(Morph::Ko),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Morph::Ja => "ja",
            Morph::Ko => "ko",
        }
    }

    /// Id of the language data package holding this engine's dictionary (SPEC §17.3).
    pub fn langdata_id(self) -> &'static str {
        match self {
            Morph::Ja => "ja-ipadic",
            Morph::Ko => "ko-kodic",
        }
    }

    pub fn from_langdata_id(id: &str) -> Option<Morph> {
        [Morph::Ja, Morph::Ko].into_iter().find(|m| m.langdata_id() == id)
    }

    fn slot(self) -> usize {
        self as usize
    }

    /// Whether a dictionary was compiled in (the `embed-*` features, for tests and development).
    fn embedded(self) -> bool {
        match self {
            Morph::Ja => cfg!(feature = "embed-ja"),
            Morph::Ko => cfg!(feature = "embed-ko"),
        }
    }
}

/// Directory of installed language data per engine, set by [`set_dict_dir`].
static DICT_DIRS: std::sync::Mutex<[Option<std::path::PathBuf>; 2]> = std::sync::Mutex::new([None, None]);

/// Point an engine at an installed dictionary directory (or `None` to unset it). Drops any loaded dictionary.
pub fn set_dict_dir(kind: Morph, dir: Option<std::path::PathBuf>) {
    DICT_DIRS.lock().unwrap_or_else(|e| e.into_inner())[kind.slot()] = dir;
    engine::reset(kind);
}

fn dict_dir(kind: Morph) -> Option<std::path::PathBuf> {
    DICT_DIRS.lock().unwrap_or_else(|e| e.into_inner())[kind.slot()].clone()
}

/// Whether `kind` can tokenize right now: its language data is installed or a dictionary is compiled in.
pub fn available(kind: Morph) -> bool {
    cfg!(any(feature = "lang-ja", feature = "lang-ko")) && (dict_dir(kind).is_some() || kind.embedded())
}

/// One reader word (or a piece of non-word text) with its byte range in the analysed text.
#[derive(Debug, Clone)]
pub struct Group {
    pub start: usize,
    pub end: usize,
    pub word: bool,
    pub lemma: String,
    /// Annotations keyed by capability (`ja.reading`, `ja.pos`, `<lang>.morphemes`).
    pub ann: serde_json::Map<String, Value>,
}

/// One analyser morpheme, normalised across both dictionaries.
#[derive(Debug, Clone)]
struct M {
    start: usize,
    end: usize,
    surface: String,
    base: String,
    /// Dictionary POS path, e.g. `["動詞", "自立"]` or `["XSV"]`.
    pos: Vec<String>,
    /// Inflection form (ja `cform`), empty if none.
    form: String,
    reading: Option<String>,
    /// Whitespace separates this morpheme from the previous one.
    space_before: bool,
}

impl M {
    fn p1(&self) -> &str {
        self.pos.first().map_or("", String::as_str)
    }
    fn p2(&self) -> &str {
        self.pos.get(1).map_or("", String::as_str)
    }
    fn pos_label(&self) -> String {
        self.pos.join("-")
    }
}

pub fn katakana_to_hiragana(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{30A1}'..='\u{30F6}' => char::from_u32(c as u32 - 0x60).unwrap_or(c),
            _ => c,
        })
        .collect()
}

#[cfg(any(feature = "lang-ja", feature = "lang-ko"))]
mod engine {
    use super::*;
    use lindera::dictionary::load_dictionary;
    use lindera::mode::Mode;
    use lindera::segmenter::Segmenter;
    use std::borrow::Cow;
    use std::sync::{Arc, Mutex};

    /// Loaded segmenters by engine; loaded lazily, and only a successful load is kept so that data
    /// installed later is picked up without a restart.
    static SEGS: Mutex<[Option<Arc<Segmenter>>; 2]> = Mutex::new([None, None]);

    pub fn reset(kind: Morph) {
        SEGS.lock().unwrap_or_else(|e| e.into_inner())[kind.slot()] = None;
    }

    fn build(kind: Morph) -> Result<Segmenter> {
        let uri = match dict_dir(kind) {
            Some(dir) => dir.to_string_lossy().into_owned(),
            None if kind.embedded() => match kind {
                Morph::Ja => "embedded://ipadic".to_string(),
                Morph::Ko => "embedded://ko-dic".to_string(),
            },
            None => return Err(Error::LangData(kind.langdata_id().to_string())),
        };
        let dict = load_dictionary(&uri).map_err(|e| Error::Engine(format!("{} dictionary: {e}", kind.code())))?;
        Ok(Segmenter::new(Mode::Normal, dict, None))
    }

    fn segmenter(kind: Morph) -> Result<Arc<Segmenter>> {
        let mut segs = SEGS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = &segs[kind.slot()] {
            return Ok(s.clone());
        }
        let s = Arc::new(build(kind)?);
        segs[kind.slot()] = Some(s.clone());
        Ok(s)
    }

    /// Run Lindera, returning normalised morphemes in text order.
    pub fn segment(kind: Morph, text: &str) -> Result<Vec<M>> {
        let seg = segmenter(kind)?;
        let mut toks = seg.segment(Cow::Borrowed(text)).map_err(|e| Error::Import(format!("tokenizer: {e}")))?;
        let mut out: Vec<M> = Vec::with_capacity(toks.len());
        let mut prev_end = 0;
        for t in toks.iter_mut() {
            let (start, end) = (t.byte_start, t.byte_end);
            let surface = t.surface.to_string();
            let d: Vec<String> = t.details().iter().map(|s| s.to_string()).collect();
            let space_before = text[prev_end.min(start)..start].chars().any(char::is_whitespace);
            prev_end = end;
            match kind {
                Morph::Ja => out.push(ja_morpheme(start, end, surface, &d, space_before)),
                Morph::Ko => ko_morphemes(start, end, surface, &d, space_before, &mut out),
            }
        }
        Ok(out)
    }

    fn star(s: Option<&String>) -> Option<String> {
        s.filter(|s| !s.is_empty() && s.as_str() != "*").cloned()
    }

    fn ja_morpheme(start: usize, end: usize, surface: String, d: &[String], space_before: bool) -> M {
        if d.len() < 9 {
            // Unknown word without a full feature row: treat as a plain noun.
            return M { start, end, base: surface.clone(), surface, pos: vec!["名詞".into(), "一般".into()], form: String::new(), reading: None, space_before };
        }
        let pos: Vec<String> = d[..4].iter().filter(|s| s.as_str() != "*").cloned().collect();
        let base = star(d.get(6)).unwrap_or_else(|| surface.clone());
        M { start, end, base, surface, pos, form: d[5].clone(), reading: star(d.get(7)).map(|r| katakana_to_hiragana(&r)), space_before }
    }

    /// ko-dic rows: POS, class, jongseong, reading, type, first POS, last POS, expression.
    /// Inflected/pre-analysed tokens (`했` = `하/XSV + 았/EP`) are expanded using the expression.
    fn ko_morphemes(start: usize, end: usize, surface: String, d: &[String], space_before: bool, out: &mut Vec<M>) {
        let pos_col = d.first().cloned().unwrap_or_default();
        let kind = d.get(4).map_or("", String::as_str);
        let expr = d.get(7).map_or("", String::as_str);
        let expand = (kind == "Inflect" || kind == "Preanalysis") && expr != "*" && !expr.is_empty() && pos_col.contains('+');
        if expand {
            let parts: Vec<(String, String)> = expr
                .split('+')
                .filter_map(|p| {
                    let mut it = p.split('/');
                    Some((it.next()?.to_string(), it.next()?.to_string()))
                })
                .collect();
            if !parts.is_empty() {
                for (i, (form, pos)) in parts.iter().enumerate() {
                    out.push(M {
                        start,
                        end,
                        // Only the first piece carries the token's byte range for grouping purposes.
                        surface: if i == 0 { surface.clone() } else { String::new() },
                        base: form.clone(),
                        pos: vec![pos.clone()],
                        form: String::new(),
                        reading: None,
                        space_before: space_before && i == 0,
                    });
                }
                return;
            }
        }
        let pos = pos_col.split('+').next().unwrap_or("UNKNOWN").to_string();
        out.push(M { start, end, base: surface.clone(), surface, pos: vec![pos], form: String::new(), reading: None, space_before });
    }
}

#[cfg(not(any(feature = "lang-ja", feature = "lang-ko")))]
mod engine {
    use super::*;
    pub fn segment(kind: Morph, _text: &str) -> Result<Vec<M>> {
        Err(Error::Engine(format!("{} support was not compiled in", kind.code())))
    }
    pub fn reset(_kind: Morph) {}
}

/// Analyse `text` into reader words and non-word runs (in order, covering only analysed spans;
/// the caller fills whitespace gaps between byte ranges).
pub fn analyze(kind: Morph, text: &str) -> Result<Vec<Group>> {
    let ms = engine::segment(kind, text)?;
    Ok(match kind {
        Morph::Ja => group_ja(&ms),
        Morph::Ko => group_ko(&ms),
    })
}

fn is_wordlike(s: &str) -> bool {
    s.chars().any(char::is_alphanumeric)
}

fn morph_json(m: &M) -> Value {
    let mut v = json!({ "surface": m.surface, "base": m.base, "pos": m.pos_label() });
    if let Some(r) = &m.reading {
        v["reading"] = json!(r);
    }
    v
}

fn finish(kind: Morph, ms: &[M], lemma: String, role: &str, reading: Option<String>) -> Group {
    let code = kind.code();
    let mut ann = serde_json::Map::new();
    ann.insert(format!("{code}.pos"), json!(role));
    ann.insert(format!("{code}.morphemes"), Value::Array(ms.iter().filter(|m| !m.surface.is_empty() || kind == Morph::Ko).map(morph_json).collect()));
    if let Some(r) = reading {
        ann.insert("ja.reading".into(), json!(r));
    }
    Group { start: ms[0].start, end: ms.last().unwrap().end, word: true, lemma, ann }
}

// ───────────────────────── Japanese ─────────────────────────

fn ja_role(m: &M) -> &'static str {
    match (m.p1(), m.p2()) {
        ("名詞", "固有名詞") => "name",
        ("名詞", "形容動詞語幹") => "adjective",
        ("名詞", _) => "noun",
        ("動詞", _) => "verb",
        ("形容詞", _) => "adjective",
        ("副詞", _) => "adverb",
        ("助詞", _) => "particle",
        _ => "function",
    }
}

/// Joined reading in hiragana; `None` if some kanji-bearing morpheme has no dictionary reading.
fn ja_reading(ms: &[M]) -> Option<String> {
    let mut out = String::new();
    for m in ms {
        match &m.reading {
            Some(r) => out.push_str(r),
            None if m.surface.chars().all(|c| !is_kanji(c)) => out.push_str(&katakana_to_hiragana(&m.surface)),
            None => return None,
        }
    }
    Some(out)
}

fn is_kanji(c: char) -> bool {
    matches!(c, '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}' | '\u{3005}')
}

const MAX_GROUP: usize = 8;

fn ja_is_suru(m: &M) -> bool {
    m.p1() == "動詞" && m.base == "する"
}

fn ja_absorbs(prev: &M, m: &M) -> bool {
    match m.p1() {
        "助動詞" => true,
        "動詞" => match m.p2() {
            "接尾" | "非自立" => true,
            // Causative/passive さ after a verb stem: 読ま|さ|れ|て.
            "自立" => ja_is_suru(m) && matches!(prev.form.as_str(), "未然形" | "未然レル接続" | "未然ウ接続") && prev.p1() == "動詞",
            _ => false,
        },
        "助詞" => matches!(m.p2(), "接続助詞" | "並立助詞") && matches!(m.surface.as_str(), "て" | "で" | "ば" | "たり" | "だり"),
        "名詞" => m.p2() == "接尾" && m.pos.get(2).is_some_and(|s| s == "助動詞語幹"),
        "形容詞" => matches!(m.p2(), "非自立" | "接尾"),
        _ => false,
    }
}

fn group_ja(ms: &[M]) -> Vec<Group> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < ms.len() {
        let m = &ms[i];
        if m.p1() == "記号" || !is_wordlike(&m.surface) {
            out.push(Group { start: m.start, end: m.end, word: false, lemma: m.surface.clone(), ann: Default::default() });
            i += 1;
            continue;
        }
        let mut j = i + 1; // exclusive end
        let mut lemma = m.base.clone();
        let mut role = ja_role(m);
        let mut grouping = false;
        if m.p1() == "接頭詞" && m.p2() == "名詞接続" && ms.get(j).is_some_and(|n| n.p1() == "名詞" && !n.space_before) {
            // お茶, ご飯: a polite prefix belongs to the noun that follows.
            lemma = format!("{}{}", m.surface, ms[j].base);
            role = ja_role(&ms[j]);
            j += 1;
            // fall through: the noun itself may head a する verb below
            if ms[j - 1].p2() == "サ変接続" && ms.get(j).is_some_and(ja_is_suru) {
                lemma.push_str("する");
                role = "verb";
                grouping = true;
                j += 1;
            }
        } else if m.p1() == "名詞" && m.p2() == "サ変接続" && ms.get(j).is_some_and(|n| ja_is_suru(n) && !n.space_before) {
            lemma = format!("{}する", m.surface);
            role = "verb";
            grouping = true;
            j += 1;
        } else if matches!((m.p1(), m.p2()), ("動詞", _) | ("形容詞", _) | ("助動詞", _)) {
            grouping = true;
        }
        if grouping {
            while j < ms.len() && j - i < MAX_GROUP && !ms[j].space_before && ja_absorbs(&ms[j - 1], &ms[j]) {
                j += 1;
            }
        }
        let span = &ms[i..j];
        let reading = ja_reading(span);
        out.push(finish(Morph::Ja, span, lemma, role, reading));
        i = j;
    }
    out
}

// ───────────────────────── Korean ─────────────────────────

fn ko_is_symbol(p: &str) -> bool {
    matches!(p, "SF" | "SE" | "SSO" | "SSC" | "SC" | "SY" | "SS" | "SP" | "SO" | "SW" | "SB")
}

fn ko_is_noun(p: &str) -> bool {
    matches!(p, "NNG" | "NNP" | "NNB" | "NNBC" | "NR" | "NP" | "XR" | "SL" | "SH" | "SN" | "UNKNOWN")
}

fn ko_role(p: &str) -> &'static str {
    match p {
        "NNP" => "name",
        "VV" | "VX" => "verb",
        "VA" | "XSA" => "adjective",
        "MAG" => "adverb",
        p if p.starts_with('J') => "particle",
        p if ko_is_noun(p) => "noun",
        _ => "function",
    }
}

fn group_ko(ms: &[M]) -> Vec<Group> {
    // Several morphemes can share one analyser token (same byte range); keep each token whole.
    let mut spans: Vec<(usize, usize)> = Vec::new(); // morpheme index ranges per token
    let mut i = 0;
    while i < ms.len() {
        let mut j = i + 1;
        while j < ms.len() && ms[j].start == ms[i].start && ms[j].end == ms[i].end {
            j += 1;
        }
        spans.push((i, j));
        i = j;
    }
    let first = |t: usize| ms[spans[t].0].p1();
    let adjacent = |t: usize| t < spans.len() && !ms[spans[t].0].space_before;
    let tok_stem = |t: usize| ms[spans[t].0].base.clone();

    let mut out = Vec::new();
    let mut t = 0;
    while t < spans.len() {
        let (a, b) = spans[t];
        let head = first(t).to_string();
        if ko_is_symbol(&head) {
            let m = &ms[a];
            out.push(Group { start: m.start, end: ms[b - 1].end, word: false, lemma: m.surface.clone(), ann: Default::default() });
            t += 1;
            continue;
        }
        let mut end_t = t + 1; // exclusive token index
        let lemma;
        let mut role = ko_role(&head);
        if matches!(head.as_str(), "VV" | "VA" | "VX" | "VCP" | "VCN") {
            lemma = format!("{}다", tok_stem(t));
            if head == "VCP" || head == "VCN" {
                role = "function";
            }
        } else if (ko_is_noun(&head)) && adjacent(t + 1) && matches!(first(t + 1), "XSV" | "XSA") {
            lemma = format!("{}{}다", ms[a].surface.trim(), tok_stem(t + 1));
            role = if first(t + 1) == "XSV" { "verb" } else { "adjective" };
            end_t = t + 2;
        } else if ko_is_noun(&head) {
            let mut l = ms[a].surface.clone();
            while adjacent(end_t) && first(end_t) == "XSN" {
                end_t += 1;
            }
            if end_t == t + 1 {
                l = ms[a].surface.clone();
            }
            lemma = l;
        } else {
            lemma = ms[a].surface.clone();
        }
        // A noun that kept its XSN suffix (들) has no verbal absorption.
        let verbal = matches!(role, "verb" | "adjective") || head == "VCP" || head == "VCN";
        if verbal {
            while adjacent(end_t) && end_t - t < MAX_GROUP && matches!(first(end_t), "EP" | "EF" | "EC" | "ETN" | "ETM" | "XSV" | "XSA") {
                end_t += 1;
            }
        }
        let span = &ms[a..spans[end_t - 1].1];
        out.push(finish(Morph::Ko, span, lemma, role, None));
        t = end_t;
    }
    out
}
