//! One test per row of the SPEC §16.5 coverage table, plus the negative cases.

use alst_text::deinflect::Deinflector;

fn d() -> Deinflector {
    Deinflector::from_json(include_str!("../../../extensions/lang-ja/deinflect.json")).unwrap()
}

/// Assert that `surface` deinflects to `lemma` with exactly `reasons` (joined by ", ") and, if given,
/// ending in condition `cond`.
fn ok(surface: &str, lemma: &str, reasons: &str, cond: &str) {
    let found: Vec<_> = d().deinflect(surface).into_iter().filter(|c| c.text == lemma).collect();
    assert!(
        found.iter().any(|c| c.reasons.join(", ") == reasons && c.conditions.iter().any(|x| x == cond)),
        "{surface} → {lemma} [{reasons}] ({cond}); candidates for that lemma: {found:?}"
    );
}

#[test]
fn ichidan_basic() {
    ok("食べた", "食べる", "past", "v1");
    ok("食べない", "食べる", "negative", "v1");
    ok("食べなかった", "食べる", "negative, past", "v1");
}

#[test]
fn ichidan_te_and_progressive() {
    ok("食べて", "食べる", "te", "v1");
    ok("食べている", "食べる", "te, progressive", "v1");
    ok("食べてる", "食べる", "te, progressive", "v1");
}

#[test]
fn ichidan_passive_causative() {
    ok("食べられる", "食べる", "passive-potential", "v1");
    ok("食べさせる", "食べる", "causative", "v1");
    ok("食べさせられた", "食べる", "causative, passive, past", "v1");
}

#[test]
fn ichidan_polite() {
    ok("食べます", "食べる", "polite", "v1");
    ok("食べませんでした", "食べる", "polite, negative, past", "v1");
}

#[test]
fn ichidan_desire() {
    ok("食べたい", "食べる", "desire", "v1");
    ok("食べたくない", "食べる", "desire, negative", "v1");
}

#[test]
fn ichidan_moods() {
    ok("食べよう", "食べる", "volitional", "v1");
    ok("食べれば", "食べる", "ba-conditional", "v1");
    ok("食べたら", "食べる", "tara-conditional", "v1");
    ok("食べろ", "食べる", "imperative", "v1");
}

#[test]
fn godan_basic() {
    ok("書いた", "書く", "past", "v5");
    ok("書かない", "書く", "negative", "v5");
    ok("書ける", "書く", "potential", "v5");
    ok("書かれる", "書く", "passive", "v5");
}

#[test]
fn godan_past_by_ending() {
    for (s, l) in [("泳いだ", "泳ぐ"), ("話した", "話す"), ("待った", "待つ"), ("買った", "買う"), ("帰った", "帰る"), ("死んだ", "死ぬ"), ("遊んだ", "遊ぶ"), ("読んだ", "読む")] {
        ok(s, l, "past", "v5");
    }
}

#[test]
fn iku_irregular() {
    ok("行った", "行く", "past", "v5");
    ok("行って", "行く", "te", "v5");
}

#[test]
fn suru() {
    ok("した", "する", "past", "vs");
    ok("しない", "する", "negative", "vs");
    ok("させる", "する", "causative", "vs");
    ok("される", "する", "passive", "vs");
}

#[test]
fn suru_compounds_also_yield_the_bare_noun() {
    ok("勉強した", "勉強する", "past", "vs");
    // the noun itself, accepted by the dictionary only if it is tagged `vs`
    ok("勉強した", "勉強", "past", "vs");
}

#[test]
fn kuru() {
    ok("来た", "来る", "past", "vk");
    ok("来ない", "来る", "negative", "vk");
    ok("きた", "くる", "past", "vk");
    ok("こない", "くる", "negative", "vk");
}

#[test]
fn i_adjectives() {
    ok("高かった", "高い", "past", "adj-i");
    ok("高くない", "高い", "negative", "adj-i");
    ok("高くて", "高い", "te", "adj-i");
    ok("高ければ", "高い", "ba-conditional", "adj-i");
    ok("高そう", "高い", "appearance", "adj-i");
    ok("高さ", "高い", "noun (-sa)", "adj-i");
}

#[test]
fn yoi_irregular() {
    ok("よかった", "よい", "past", "adj-i");
    ok("よかった", "いい", "past", "adj-i");
    ok("良かった", "良い", "past", "adj-i");
    ok("よくない", "いい", "negative", "adj-i");
}

#[test]
fn short_causative_passive_progressive_past() {
    ok("読まされていた", "読む", "causative (short), passive, te, progressive, past", "v5");
}

#[test]
fn colloquial_negative_conditional() {
    ok("食べなきゃ", "食べる", "negative, conditional (colloquial)", "v1");
}

#[test]
fn te_auxiliaries() {
    ok("食べてしまった", "食べる", "te, completive, past", "v1");
    ok("書いておく", "書く", "te, preparatory", "v5");
}

#[test]
fn negatives_never_produce_a_bare_ichidan_stem() {
    for s in ["見た", "見ない", "見て", "見れば", "見られる", "見ます"] {
        let c = d().deinflect(s);
        assert!(c.iter().all(|c| c.text != "見"), "{s}: {c:?}");
        assert!(c.iter().any(|c| c.text == "見る"), "{s}");
    }
}

#[test]
fn godan_rows_stay_distinct_from_ichidan_by_condition() {
    // 帰れば is both 帰る (v1 reading) and 帰る (v5 reading); only the conditions tell them apart, so
    // dictionary validation (which checks `rules`) can accept the godan entry and reject the other.
    let c: Vec<_> = d().deinflect("帰れば").into_iter().filter(|c| c.text == "帰る").collect();
    let conds: Vec<_> = c.iter().flat_map(|c| c.conditions.clone()).collect();
    assert!(conds.contains(&"v1".to_string()) && conds.contains(&"v5".to_string()), "{c:?}");
    assert_eq!(c.len(), 2);
}

#[test]
fn surface_is_always_the_first_candidate() {
    let c = d().deinflect("猫");
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].text, "猫");
}
