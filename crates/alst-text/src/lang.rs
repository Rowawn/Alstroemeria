//! Declarative language definition (SPEC §6.7) and the `regex` tokenizer engine (SPEC §5.2).

use crate::morph::{self, Morph};
use crate::{Error, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LookupUrl {
    pub name: String,
    pub url: String,
}

#[derive(Deserialize)]
struct Manifest {
    name: String,
    language: String,
    declarative: Declarative,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Declarative {
    tokenizer: TokenizerCfg,
    #[serde(default)]
    sentence_end: String,
    #[serde(default)]
    exceptions: Vec<String>,
    #[serde(default)]
    char_substitutions: BTreeMap<String, String>,
    #[serde(default)]
    rtl: bool,
    #[serde(default)]
    lookup_urls: Vec<LookupUrl>,
    #[serde(default)]
    term_key: TermKey,
    /// Path (inside the extension) of a deinflection rule file, SPEC §16.5.
    #[serde(default)]
    deinflection: Option<String>,
}

/// What identifies a unit as a term (SPEC §16.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TermKey {
    #[default]
    Surface,
    Lemma,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenizerCfg {
    engine: String,
    #[serde(default)]
    word_chars: String,
}

/// A token as in SPEC §5.2. Offsets are UTF-16 code units so they match JS strings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Token {
    pub i: usize,
    pub text: String,
    pub is_word: bool,
    pub norm: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lemma: Option<String>,
    pub start: usize,
    pub end: usize,
    pub ann: serde_json::Map<String, serde_json::Value>,
}

/// See [`LanguageDef::term_info`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermInfo {
    pub key: String,
    pub word_count: usize,
    pub display: String,
}

fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Debug, Clone)]
pub struct LanguageDef {
    pub id: String,
    pub name: String,
    pub rtl: bool,
    pub lookup_urls: Vec<LookupUrl>,
    pub term_key: TermKey,
    /// Relative path of the deinflection rules inside the extension, if the language has any.
    pub deinflection: Option<String>,
    /// Identifies this definition and engine build, for caches keyed on tokenizer output (SPEC §5.3).
    pub fingerprint: String,
    /// `None` for the regex engine, else the morphological engine (ja/ko).
    morph: Option<Morph>,
    word: Regex,
    sentence_end: HashSet<char>,
    exceptions: Vec<String>,
    substitutions: Vec<(String, String)>,
}

/// FNV-1a over the manifest text, the engine and this crate's version: any of them changing invalidates caches.
fn fingerprint(manifest: &str, morph: Option<Morph>) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in manifest.bytes().chain(format!("|{morph:?}|{}|lindera-6.2.0", env!("CARGO_PKG_VERSION")).bytes()) {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// A token before offsets are assigned.
struct Raw {
    text: String,
    is_word: bool,
    lemma: Option<String>,
    ann: serde_json::Map<String, serde_json::Value>,
}

/// Characters that may follow a sentence end and still belong to that sentence.
const CLOSERS: &str = "\"'»”’›)]}」』）】";

impl LanguageDef {
    pub fn from_manifest(json: &str) -> Result<Self> {
        let m: Manifest = serde_json::from_str(json).map_err(|e| Error::Manifest(e.to_string()))?;
        let t = &m.declarative.tokenizer;
        let morph = if t.engine == "regex" {
            None
        } else {
            Some(Morph::from_engine(&t.engine).ok_or_else(|| Error::Engine(t.engine.clone()))?)
        };
        let word_chars = if morph.is_some() && t.word_chars.is_empty() { "a-zA-Z" } else { t.word_chars.as_str() };
        let word = Regex::new(&format!("[{word_chars}]+")).map_err(|e| Error::Manifest(e.to_string()))?;
        Ok(Self {
            id: m.language,
            name: m.name,
            rtl: m.declarative.rtl,
            lookup_urls: m.declarative.lookup_urls,
            term_key: m.declarative.term_key,
            deinflection: m.declarative.deinflection,
            fingerprint: fingerprint(json, morph),
            morph,
            word,
            sentence_end: m.declarative.sentence_end.chars().collect(),
            exceptions: m.declarative.exceptions.iter().map(|e| e.to_lowercase()).collect(),
            substitutions: m.declarative.char_substitutions.into_iter().collect(),
        })
    }

    /// Normalized lookup form: lowercase, then the manifest's character substitutions.
    pub fn normalize(&self, word: &str) -> String {
        let mut s = word.to_lowercase();
        for (from, to) in &self.substitutions {
            s = s.replace(from, to);
        }
        s
    }

    /// Key for a term typed or selected by the user: normalized words joined by one space,
    /// plus its word count. `None` if the text contains no words.
    pub fn term_key(&self, text: &str) -> Option<(String, usize)> {
        self.term_info(text).map(|t| (t.key, t.word_count))
    }

    /// The term for a typed or selected text: its key (§5.3), word count, and the text to show and
    /// store as the term (§16.3: the lemma for lemma-keyed languages, else the squashed text).
    pub fn term_info(&self, text: &str) -> Option<TermInfo> {
        let tokens = self.tokenize_flat(text);
        let first = tokens.iter().position(|t| t.is_word)?;
        let last = tokens.iter().rposition(|t| t.is_word)?;
        let word_count = tokens.iter().filter(|t| t.is_word).count();
        let key = tokens.iter().filter(|t| t.is_word).map(|t| t.norm.as_str()).collect::<Vec<_>>().join(" ");
        let display = match self.term_key {
            TermKey::Surface => squash(text),
            TermKey::Lemma => {
                let joined: String = tokens[first..=last]
                    .iter()
                    .map(|t| if t.is_word { t.lemma.as_deref().unwrap_or(&t.text) } else { t.text.as_str() })
                    .collect();
                squash(&joined)
            }
        };
        Some(TermInfo { key, word_count, display })
    }

    pub fn count_words(&self, text: &str) -> usize {
        self.tokenize_flat(text).iter().filter(|t| t.is_word).count()
    }

    /// Tokenizing is slow enough (morphological analysis) to be worth caching (SPEC §5.3).
    pub fn cacheable(&self) -> bool {
        self.morph.is_some()
    }

    /// Languages written without spaces between words (word chains match without whitespace).
    pub fn spaceless(&self) -> bool {
        self.morph == Some(Morph::Ja)
    }

    /// Multi-word matcher over `text_lc` keys, configured for this language.
    pub fn matcher<'a>(&self, keys: impl IntoIterator<Item = &'a str>) -> crate::Matcher {
        crate::Matcher::new(keys).with_spaceless(self.spaceless())
    }

    /// Flat token list (no sentence structure).
    pub fn tokenize_flat(&self, text: &str) -> Vec<Token> {
        let mut raw: Vec<Raw> = Vec::new();
        match self.morph {
            None => {
                let mut last = 0;
                for m in self.word.find_iter(text) {
                    self.push_gap(&text[last..m.start()], &mut raw);
                    self.push_word_run(m.as_str(), &mut raw);
                    last = m.end();
                }
                self.push_gap(&text[last..], &mut raw);
            }
            Some(kind) => {
                // An analyser failure degrades to plain text rather than losing the part.
                let groups = morph::analyze(kind, text).unwrap_or_default();
                let mut last = 0;
                for g in groups {
                    self.push_gap(&text[last..g.start], &mut raw);
                    if g.word {
                        raw.push(Raw { text: text[g.start..g.end].to_string(), is_word: true, lemma: Some(g.lemma), ann: g.ann });
                    } else {
                        self.push_gap(&text[g.start..g.end], &mut raw);
                    }
                    last = g.end;
                }
                self.push_gap(&text[last..], &mut raw);
            }
        }

        let mut pos = 0;
        raw.into_iter()
            .enumerate()
            .map(|(i, r)| {
                let start = pos;
                pos += r.text.encode_utf16().count();
                let norm = match (r.is_word, self.term_key, &r.lemma) {
                    (false, _, _) => r.text.clone(),
                    (true, TermKey::Lemma, Some(l)) => self.normalize(l),
                    _ => self.normalize(&r.text),
                };
                Token { i, text: r.text, is_word: r.is_word, norm, lemma: r.lemma, start, end: pos, ann: r.ann }
            })
            .collect()
    }

    /// Tokens grouped into sentences (SPEC §5.2).
    pub fn tokenize(&self, text: &str) -> Vec<Vec<Token>> {
        let tokens = self.tokenize_flat(text);
        let mut sentences: Vec<Vec<Token>> = Vec::new();
        let mut cur: Vec<Token> = Vec::new();
        let mut ended = false;
        for (idx, tok) in tokens.iter().enumerate() {
            let is_ws = !tok.is_word && tok.text.chars().all(char::is_whitespace);
            if ended && !is_ws && !self.is_end(&tok.text) && !CLOSERS.contains(tok.text.as_str()) {
                sentences.push(std::mem::take(&mut cur));
                ended = false;
            }
            cur.push(tok.clone());
            if is_ws {
                // A blank line always breaks; otherwise whitespace closes a sentence that already ended.
                if ended || tok.text.matches('\n').count() >= 2 {
                    sentences.push(std::mem::take(&mut cur));
                    ended = false;
                }
            } else if self.is_end(&tok.text) && self.ends_sentence(&tokens, idx) {
                ended = true;
            }
        }
        if !cur.is_empty() {
            sentences.push(cur);
        }
        sentences
    }

    fn is_end(&self, s: &str) -> bool {
        let mut c = s.chars();
        matches!((c.next(), c.next()), (Some(ch), None) if self.sentence_end.contains(&ch))
    }

    /// An end char counts only when followed by whitespace, a closer, another end char, or the end,
    /// and when it isn't the dot of a listed abbreviation.
    fn ends_sentence(&self, tokens: &[Token], idx: usize) -> bool {
        let followed_ok = self.spaceless() || match tokens.get(idx + 1) {
            None => true,
            Some(n) => {
                (!n.is_word && n.text.chars().all(char::is_whitespace))
                    || self.is_end(&n.text)
                    || CLOSERS.contains(n.text.as_str())
            }
        };
        if !followed_ok {
            return false;
        }
        if tokens[idx].text == "…" {
            // An ellipsis followed by a lowercase word continues the sentence.
            let next = tokens[idx + 1..].iter().find(|t| t.is_word || !(t.text.chars().all(char::is_whitespace) || CLOSERS.contains(t.text.as_str())));
            if next.is_some_and(|t| t.is_word && t.text.chars().next().is_some_and(char::is_lowercase)) {
                return false;
            }
        }
        if tokens[idx].text == "." {
            // Contiguous non-whitespace run ending at this dot, e.g. "EE.UU."
            let mut run = String::from(".");
            for t in tokens[..idx].iter().rev() {
                if !t.is_word && t.text.chars().all(char::is_whitespace) {
                    break;
                }
                run.insert_str(0, &t.text);
            }
            let run = run.to_lowercase();
            for ex in &self.exceptions {
                if run.ends_with(ex.as_str()) {
                    let before = run[..run.len() - ex.len()].chars().next_back();
                    if before.is_none_or(|c| !c.is_alphanumeric()) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Split a regex match into a word plus any leading/trailing joiners (`'`, `-`), which are punctuation.
    fn push_word_run(&self, run: &str, out: &mut Vec<Raw>) {
        let trimmed_start = run.trim_start_matches(|c: char| !c.is_alphanumeric());
        let lead = &run[..run.len() - trimmed_start.len()];
        let core = trimmed_start.trim_end_matches(|c: char| !c.is_alphanumeric());
        let tail = &trimmed_start[core.len()..];
        self.push_gap(lead, out);
        if !core.is_empty() {
            out.push(Raw { text: core.to_string(), is_word: true, lemma: None, ann: Default::default() });
        }
        self.push_gap(tail, out);
    }

    /// Non-word text: whitespace runs stay together, every other char is its own token.
    fn push_gap(&self, gap: &str, out: &mut Vec<Raw>) {
        let mut ws = String::new();
        for c in gap.chars() {
            if c.is_whitespace() {
                ws.push(c);
            } else {
                if !ws.is_empty() {
                    out.push(Raw { text: std::mem::take(&mut ws), is_word: false, lemma: None, ann: Default::default() });
                }
                out.push(Raw { text: c.to_string(), is_word: false, lemma: None, ann: Default::default() });
            }
        }
        if !ws.is_empty() {
            out.push(Raw { text: ws, is_word: false, lemma: None, ann: Default::default() });
        }
    }
}
