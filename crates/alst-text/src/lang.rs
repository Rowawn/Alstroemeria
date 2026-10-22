//! Declarative language definition (SPEC §6.7) and the `regex` tokenizer engine (SPEC §5.2).

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
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenizerCfg {
    engine: String,
    #[serde(default)]
    word_chars: String,
}

/// A token as in SPEC §5.2. Offsets are UTF-16 code units so they match JS strings.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Token {
    pub i: usize,
    pub text: String,
    pub is_word: bool,
    pub norm: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lemma: Option<String>,
    pub start: usize,
    pub end: usize,
    pub ann: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct LanguageDef {
    pub id: String,
    pub name: String,
    pub rtl: bool,
    pub lookup_urls: Vec<LookupUrl>,
    word: Regex,
    sentence_end: HashSet<char>,
    exceptions: Vec<String>,
    substitutions: Vec<(String, String)>,
}

/// Characters that may follow a sentence end and still belong to that sentence.
const CLOSERS: &str = "\"'»”’›)]}";

impl LanguageDef {
    pub fn from_manifest(json: &str) -> Result<Self> {
        let m: Manifest = serde_json::from_str(json).map_err(|e| Error::Manifest(e.to_string()))?;
        let t = &m.declarative.tokenizer;
        if t.engine != "regex" {
            return Err(Error::Engine(t.engine.clone()));
        }
        let word = Regex::new(&format!("[{}]+", t.word_chars)).map_err(|e| Error::Manifest(e.to_string()))?;
        Ok(Self {
            id: m.language,
            name: m.name,
            rtl: m.declarative.rtl,
            lookup_urls: m.declarative.lookup_urls,
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
        let norms: Vec<String> =
            self.tokenize_flat(text).into_iter().filter(|t| t.is_word).map(|t| t.norm).collect();
        if norms.is_empty() { None } else { Some((norms.join(" "), norms.len())) }
    }

    pub fn count_words(&self, text: &str) -> usize {
        self.tokenize_flat(text).iter().filter(|t| t.is_word).count()
    }

    /// Flat token list (no sentence structure).
    pub fn tokenize_flat(&self, text: &str) -> Vec<Token> {
        let mut raw: Vec<(String, bool)> = Vec::new();
        let mut last = 0;
        for m in self.word.find_iter(text) {
            self.push_gap(&text[last..m.start()], &mut raw);
            self.push_word_run(m.as_str(), &mut raw);
            last = m.end();
        }
        self.push_gap(&text[last..], &mut raw);

        let mut pos = 0;
        raw.into_iter()
            .enumerate()
            .map(|(i, (text, is_word))| {
                let start = pos;
                pos += text.encode_utf16().count();
                let norm = if is_word { self.normalize(&text) } else { text.clone() };
                Token { i, text, is_word, norm, lemma: None, start, end: pos, ann: Default::default() }
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
        let followed_ok = match tokens.get(idx + 1) {
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
    fn push_word_run(&self, run: &str, out: &mut Vec<(String, bool)>) {
        let trimmed_start = run.trim_start_matches(|c: char| !c.is_alphanumeric());
        let lead = &run[..run.len() - trimmed_start.len()];
        let core = trimmed_start.trim_end_matches(|c: char| !c.is_alphanumeric());
        let tail = &trimmed_start[core.len()..];
        self.push_gap(lead, out);
        if !core.is_empty() {
            out.push((core.to_string(), true));
        }
        self.push_gap(tail, out);
    }

    /// Non-word text: whitespace runs stay together, every other char is its own token.
    fn push_gap(&self, gap: &str, out: &mut Vec<(String, bool)>) {
        let mut ws = String::new();
        for c in gap.chars() {
            if c.is_whitespace() {
                ws.push(c);
            } else {
                if !ws.is_empty() {
                    out.push((std::mem::take(&mut ws), false));
                }
                out.push((c.to_string(), false));
            }
        }
        if !ws.is_empty() {
            out.push((ws, false));
        }
    }
}
