//! Golden tests (SPEC §13): fixture text → expected tokens / units JSON.
//! Regenerate with `UPDATE_GOLDEN=1 cargo test -p alst-text --test golden`, then review the diff.

use alst_text::reader::build_units;
use alst_text::{LanguageDef, Matcher, TermState, bundled};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;

fn fixtures(lang: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(lang)
}

fn lang(id: &str) -> LanguageDef {
    bundled::languages().unwrap().into_iter().find(|l| l.id == id).unwrap()
}

fn check<T: Serialize>(path: PathBuf, actual: &T) {
    let json = serde_json::to_string_pretty(actual).unwrap() + "\n";
    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::write(&path, &json).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing golden {}", path.display()));
    assert_eq!(json, expected, "golden mismatch: {}", path.display());
}

#[test]
fn es_tokens_basic() {
    let dir = fixtures("es");
    let text = std::fs::read_to_string(dir.join("tokens-basic.txt")).unwrap();
    check(dir.join("tokens-basic.json"), &lang("es").tokenize(&text));
}

#[test]
fn es_tokens_reassemble_source_and_offsets_are_utf16() {
    let l = lang("es");
    let text = std::fs::read_to_string(fixtures("es").join("tokens-basic.txt")).unwrap();
    let flat = l.tokenize_flat(&text);
    assert_eq!(flat.iter().map(|t| t.text.as_str()).collect::<String>(), text);
    let units: Vec<u16> = text.encode_utf16().collect();
    for t in &flat {
        assert_eq!(String::from_utf16(&units[t.start..t.end]).unwrap(), t.text);
    }
    let sentences: usize = l.tokenize(&text).len();
    assert_eq!(l.tokenize(&text).concat().len(), flat.len());
    assert!(sentences >= 6);
}

#[derive(Serialize)]
struct UnitGolden {
    text: String,
    key: Option<String>,
    words: usize,
    status: Option<i64>,
    sentence: usize,
}

#[test]
fn es_multiword_matching() {
    let dir = fixtures("es");
    let l = lang("es");
    let text = std::fs::read_to_string(dir.join("match-basic.txt")).unwrap();
    let mut terms: HashMap<String, TermState> = HashMap::new();
    for line in std::fs::read_to_string(dir.join("match-basic.terms")).unwrap().lines().filter(|l| !l.starts_with('#')) {
        let f: Vec<&str> = line.split('\t').collect();
        terms.insert(f[0].to_string(), TermState { id: f[1].to_string(), status: f[2].parse().unwrap() });
    }
    let matcher = Matcher::new(terms.keys().map(String::as_str));
    let units = build_units(&l.tokenize(&text), &matcher, &terms);
    let golden: Vec<UnitGolden> = units
        .into_iter()
        .filter(|u| u.is_word)
        .map(|u| UnitGolden { text: u.text, key: u.key, words: u.word_count, status: u.status, sentence: u.sentence })
        .collect();
    check(dir.join("match-basic.json"), &golden);
}

#[test]
fn term_keys() {
    let l = lang("es");
    assert_eq!(l.term_key("  Por   Favor "), Some(("por favor".into(), 2)));
    assert_eq!(l.term_key("D’Artagnan"), Some(("d'artagnan".into(), 1)));
    assert_eq!(l.term_key("¿?"), None);
}
