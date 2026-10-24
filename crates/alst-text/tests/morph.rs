//! Golden tests for the Japanese/Korean engines (SPEC §16.2) and term keys (§16.3).
//! Regenerate with `UPDATE_GOLDEN=1 cargo test -p alst-text --test morph`, then review the diff.
#![cfg(all(feature = "lang-ja", feature = "lang-ko"))]

use alst_text::{LanguageDef, Token};
use std::path::PathBuf;

fn dir(lang: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(lang)
}

fn ja() -> LanguageDef {
    LanguageDef::from_manifest(include_str!("../../../extensions/lang-ja/extension.json")).unwrap()
}

fn ko() -> LanguageDef {
    LanguageDef::from_manifest(include_str!("../../../extensions/lang-ko/extension.json")).unwrap()
}

/// `surface|lemma|role|reading|morphemes` for words, `-surface` for everything else.
fn line(t: &Token) -> String {
    if !t.is_word {
        return format!("-{}", t.text.replace('\n', "\\n"));
    }
    let code = |k: &str| t.ann.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let lang = if t.ann.contains_key("ja.pos") { "ja" } else { "ko" };
    let m = t.ann[&format!("{lang}.morphemes")].as_array().unwrap();
    let parts: Vec<String> = m.iter().map(|x| x["surface"].as_str().unwrap().to_string()).collect();
    format!("{}|{}|{}|{}|{}", t.text, t.lemma.clone().unwrap(), code(&format!("{lang}.pos")), code("ja.reading"), parts.join("+"))
}

fn golden(l: &LanguageDef, lang: &str, name: &str) {
    let text = std::fs::read_to_string(dir(lang).join(format!("{name}.txt"))).unwrap();
    let flat = l.tokenize_flat(&text);
    assert_eq!(flat.iter().map(|t| t.text.as_str()).collect::<String>(), text, "tokens reassemble the source");
    let units: Vec<u16> = text.encode_utf16().collect();
    for t in &flat {
        assert_eq!(String::from_utf16(&units[t.start..t.end]).unwrap(), t.text, "UTF-16 offsets");
    }
    let out: String = flat.iter().map(|t| line(t) + "\n").collect();
    let path = dir(lang).join(format!("{name}.golden"));
    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::write(&path, &out).unwrap();
        return;
    }
    assert_eq!(out, std::fs::read_to_string(&path).unwrap_or_default(), "golden mismatch: {}", path.display());
}

#[test]
fn ja_examples_golden() {
    golden(&ja(), "ja", "examples");
}
#[test]
fn ja_prose_golden() {
    golden(&ja(), "ja", "prose");
}
#[test]
fn ko_examples_golden() {
    golden(&ko(), "ko", "examples");
}
#[test]
fn ko_prose_golden() {
    golden(&ko(), "ko", "prose");
}

/// The words of `text` as `(surface, lemma)`.
fn words(l: &LanguageDef, text: &str) -> Vec<(String, String)> {
    l.tokenize_flat(text).into_iter().filter(|t| t.is_word).map(|t| (t.text, t.lemma.unwrap())).collect()
}

fn one(l: &LanguageDef, text: &str, lemma: &str) {
    let w = words(l, text);
    assert_eq!(w, vec![(text.to_string(), lemma.to_string())], "{text}");
}

#[test]
fn ja_groups_verb_chains() {
    let l = ja();
    one(&l, "食べさせられた", "食べる");
    one(&l, "食べなかった", "食べる");
    one(&l, "食べている", "食べる");
    one(&l, "勉強している", "勉強する");
    one(&l, "読まされていた", "読む");
    one(&l, "書いてしまった", "書く");
    one(&l, "食べたくない", "食べる");
    one(&l, "美味しそうだ", "美味しい");
    one(&l, "高かった", "高い");
    one(&l, "来なかった", "来る");
}

#[test]
fn ja_particles_and_nouns_stay_separate() {
    let l = ja();
    let w: Vec<String> = words(&l, "私は学校へ行った").into_iter().map(|(s, _)| s).collect();
    assert_eq!(w, ["私", "は", "学校", "へ", "行った"]);
    let w: Vec<String> = words(&l, "本を読んで").into_iter().map(|(s, _)| s).collect();
    assert_eq!(w, ["本", "を", "読んで"]);
    // copula forms group with each other but not with the noun
    let w: Vec<String> = words(&l, "学生でした").into_iter().map(|(s, _)| s).collect();
    assert_eq!(w, ["学生", "でした"]);
}

#[test]
fn ja_polite_prefix_joins_noun_and_readings_are_hiragana() {
    let l = ja();
    let t = l.tokenize_flat("お茶");
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].lemma.as_deref(), Some("お茶"));
    assert_eq!(t[0].ann["ja.reading"], "おちゃ");
    let t = l.tokenize_flat("食べさせられた");
    assert_eq!(t[0].ann["ja.reading"], "たべさせられた");
    assert_eq!(t[0].ann["ja.pos"], "verb");
    let m = t[0].ann["ja.morphemes"].as_array().unwrap();
    assert_eq!(m.len(), 4);
    assert_eq!(m[0]["base"], "食べる");
    assert_eq!(m[0]["pos"], "動詞-自立");
    assert_eq!(t[0].norm, "食べる");
}

#[test]
fn ja_roles_and_sentences() {
    let l = ja();
    let t = l.tokenize_flat("美しい花をゆっくり見た。");
    let roles: Vec<(&str, &str)> = t.iter().filter(|x| x.is_word).map(|x| (x.text.as_str(), x.ann["ja.pos"].as_str().unwrap())).collect();
    assert_eq!(roles, [("美しい", "adjective"), ("花", "noun"), ("を", "particle"), ("ゆっくり", "adverb"), ("見た", "verb")]);
    let s = l.tokenize("今日は晴れだ。明日は雨？「本当」と言った。");
    assert_eq!(s.len(), 3, "{:?}", s.iter().map(|x| x.iter().map(|t| t.text.as_str()).collect::<String>()).collect::<Vec<_>>());
}

#[test]
fn ja_term_key_follows_lemma() {
    let l = ja();
    for s in ["食べた", "食べない", "食べさせられた"] {
        assert_eq!(l.term_key(s), Some(("食べる".into(), 1)), "{s}");
    }
    assert_eq!(l.term_key("。"), None);
}

#[test]
fn ja_multiword_terms_match_without_spaces() {
    use alst_text::TermState;
    use std::collections::HashMap;
    let l = ja();
    // 気 + が + する as a three-word term, keyed by lemmas joined with a space
    let key = "気 が する".to_string();
    let mut terms = HashMap::new();
    terms.insert(key.clone(), TermState { id: "t1".into(), status: 2 });
    let m = l.matcher([key.as_str()]);
    let units = alst_text::reader::build_units(&l.tokenize("変な気がした。"), &m, &terms);
    let hit = units.iter().find(|u| u.word_count == 3).expect("multi-word match");
    assert_eq!(hit.text, "気がした");
    assert_eq!(hit.status, Some(2));
}

#[test]
fn ko_groups_and_lemmas() {
    let l = ko();
    let cases = [
        ("공부했어요", "공부하다"),
        ("먹었습니다", "먹다"),
        ("예뻐요", "예쁘다"),
        ("갔다", "가다"),
        ("감사합니다", "감사하다"),
        ("조용하다", "조용하다"),
        ("친구들", "친구"),
    ];
    for (s, lemma) in cases {
        one(&l, s, lemma);
    }
}

#[test]
fn ko_particles_split_and_eojeol_boundaries_hold() {
    let l = ko();
    let w: Vec<(String, String)> = words(&l, "학교에서 친구들이 공부했어요");
    assert_eq!(
        w,
        [("학교", "학교"), ("에서", "에서"), ("친구들", "친구"), ("이", "이"), ("공부했어요", "공부하다")].map(|(a, b)| (a.to_string(), b.to_string()))
    );
    let w: Vec<(String, String)> = words(&l, "학생이에요");
    assert_eq!(w, [("학생", "학생"), ("이에요", "이다")].map(|(a, b)| (a.to_string(), b.to_string())));
    // a verb never absorbs across whitespace
    let w = words(&l, "먹고 있어요");
    assert_eq!(w.len(), 2);
}

#[test]
fn ko_roles_and_term_keys() {
    let l = ko();
    let t = l.tokenize_flat("친구들이 예뻐요");
    let roles: Vec<&str> = t.iter().filter(|x| x.is_word).map(|x| x.ann["ko.pos"].as_str().unwrap()).collect();
    assert_eq!(roles, ["noun", "particle", "adjective"]);
    assert_eq!(l.term_key("먹었습니다"), Some(("먹다".into(), 1)));
    assert!(l.tokenize_flat("친구들")[0].ann.get("ja.reading").is_none());
}

#[test]
fn term_info_shows_the_lemma_for_lemma_keyed_languages() {
    let l = ja();
    let t = l.term_info("食べさせられた").unwrap();
    assert_eq!((t.key.as_str(), t.display.as_str(), t.word_count), ("食べる", "食べる", 1));
    let t = l.term_info("気がした").unwrap();
    assert_eq!((t.key.as_str(), t.display.as_str(), t.word_count), ("気 が する", "気がする", 3));
    let t = ko().term_info("먹었습니다").unwrap();
    assert_eq!((t.key.as_str(), t.display.as_str()), ("먹다", "먹다"));
    // surface-keyed languages keep the squashed text
    let es = LanguageDef::from_manifest(include_str!("../../../extensions/lang-es/extension.json")).unwrap();
    assert_eq!(es.term_info("  Por   Favor ").unwrap().display, "Por Favor");
}

#[test]
fn tokens_survive_a_json_round_trip_for_the_token_cache() {
    for (l, text) in [(ja(), "食べさせられた猫は、昨日、お茶を飲んでいた。"), (ko(), "학교에서 친구들이 공부했어요.")] {
        let sentences = l.tokenize(text);
        let back: Vec<Vec<Token>> = serde_json::from_str(&serde_json::to_string(&sentences).unwrap()).unwrap();
        assert_eq!(sentences, back);
        assert!(l.cacheable() && !l.fingerprint.is_empty());
    }
    let es = LanguageDef::from_manifest(include_str!("../../../extensions/lang-es/extension.json")).unwrap();
    assert!(!es.cacheable());
    assert_ne!(es.fingerprint, ja().fingerprint);
}
