//! Multi-word term matching (SPEC §5.3) and the "units" the reader renders.

use crate::lang::Token;
use serde::Serialize;
use std::collections::HashMap;

/// What the store knows about a term, keyed elsewhere by `text_lc`.
#[derive(Debug, Clone, PartialEq)]
pub struct TermState {
    pub id: String,
    pub status: i64,
}

#[derive(Default, Debug)]
struct Node {
    children: HashMap<String, Node>,
    terminal: bool,
}

/// Trie over normalized word sequences; longest match wins.
#[derive(Default, Debug)]
pub struct Matcher {
    root: Node,
    max_words: usize,
}

impl Matcher {
    /// `keys` are `text_lc` values of multi-word terms: normalized words joined by single spaces.
    pub fn new<'a>(keys: impl IntoIterator<Item = &'a str>) -> Self {
        let mut m = Matcher::default();
        for key in keys {
            let words: Vec<&str> = key.split(' ').collect();
            if words.len() < 2 {
                continue;
            }
            m.max_words = m.max_words.max(words.len());
            let mut node = &mut m.root;
            for w in words {
                node = node.children.entry(w.to_string()).or_default();
            }
            node.terminal = true;
        }
        m
    }

    /// Number of words in the longest term that is a prefix of `norms` (at least 2), if any.
    pub fn longest(&self, norms: &[&str]) -> Option<usize> {
        let mut node = &self.root;
        let mut best = None;
        for (n, w) in norms.iter().enumerate() {
            match node.children.get(*w) {
                Some(next) => {
                    node = next;
                    if node.terminal {
                        best = Some(n + 1);
                    }
                }
                None => break,
            }
        }
        best
    }
}

/// One renderable piece of a part: a word, a multi-word match, or non-word text.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Unit {
    /// Source text (for a multi-word unit, including the whitespace between its words).
    pub text: String,
    pub is_word: bool,
    /// `text_lc` for words, `None` otherwise.
    pub key: Option<String>,
    pub word_count: usize,
    /// 0 = new/unknown when the word has no term row; `None` for non-words.
    pub status: Option<i64>,
    pub term_id: Option<String>,
    pub sentence: usize,
    pub start: usize,
    pub end: usize,
    /// First and last token index (inclusive) covered by this unit, as in `PartView.tokens`.
    pub token_range: (usize, usize),
}

/// A token as sent to the UI: the §5.2 token plus the sentence it belongs to.
#[derive(Debug, Clone, Serialize)]
pub struct ViewToken {
    #[serde(flatten)]
    pub token: Token,
    pub sentence: usize,
}

/// Flatten sentences into one token list, indexed by `Token::i`.
pub fn flatten(sentences: &[Vec<Token>]) -> Vec<ViewToken> {
    sentences
        .iter()
        .enumerate()
        .flat_map(|(si, sent)| sent.iter().map(move |t| ViewToken { token: t.clone(), sentence: si }))
        .collect()
}

fn is_space(t: &Token) -> bool {
    !t.is_word && t.text.chars().all(char::is_whitespace)
}

/// Turn tokenized sentences into units, greedily matching multi-word terms and attaching term state.
pub fn build_units(sentences: &[Vec<Token>], matcher: &Matcher, terms: &HashMap<String, TermState>) -> Vec<Unit> {
    let mut units = Vec::new();
    for (si, sent) in sentences.iter().enumerate() {
        let mut k = 0;
        while k < sent.len() {
            let tok = &sent[k];
            if !tok.is_word {
                units.push(Unit {
                    text: tok.text.clone(),
                    is_word: false,
                    key: None,
                    word_count: 0,
                    status: None,
                    term_id: None,
                    sentence: si,
                    start: tok.start,
                    end: tok.end,
                    token_range: (tok.i, tok.i),
                });
                k += 1;
                continue;
            }
            // Candidate chain: this word and following words separated only by whitespace.
            let mut norms = vec![tok.norm.as_str()];
            let mut ends = vec![k];
            let mut j = k + 1;
            while norms.len() < matcher.max_words && j < sent.len() {
                if is_space(&sent[j]) && j + 1 < sent.len() && sent[j + 1].is_word {
                    norms.push(sent[j + 1].norm.as_str());
                    ends.push(j + 1);
                    j += 2;
                } else {
                    break;
                }
            }
            let n = matcher.longest(&norms).unwrap_or(1);
            let last = ends[n - 1];
            let key = norms[..n].join(" ");
            let state = terms.get(&key);
            units.push(Unit {
                text: sent[k..=last].iter().map(|t| t.text.as_str()).collect(),
                is_word: true,
                word_count: n,
                status: Some(state.map_or(0, |s| s.status)),
                term_id: state.map(|s| s.id.clone()),
                key: Some(key),
                sentence: si,
                start: tok.start,
                end: sent[last].end,
                token_range: (tok.i, sent[last].i),
            });
            k = last + 1;
        }
    }
    units
}
