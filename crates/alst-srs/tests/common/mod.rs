#![allow(dead_code)]
use alst_core::Store;
use alst_core::srs::classic_card_id;
use rusqlite::types::Value;

pub const HOUR: i64 = 3_600_000;
pub const DAY: i64 = 86_400_000;
/// 2024-03-01 12:00 UTC.
pub const T0: i64 = 1_709_294_400_000;

pub fn t(s: &str) -> Value {
    Value::Text(s.to_string())
}

pub fn i(n: i64) -> Value {
    Value::Integer(n)
}

pub fn config(s: &mut Store, id: &str, new_per_day: i64, reviews_per_day: i64, bury: bool) {
    s.srs_put(
        "deck_configs",
        id,
        &[("name", t(id)), ("new_per_day", i(new_per_day)), ("reviews_per_day", i(reviews_per_day)), ("bury_siblings", i(bury as i64))],
    )
    .unwrap();
}

pub fn deck(s: &mut Store, id: &str, name: &str, kind: &str, config: &str) {
    s.srs_put("decks", id, &[("name", t(name)), ("kind", t(kind)), ("language", t("es")), ("config_id", t(config))]).unwrap();
}

pub fn note_type(s: &mut Store) {
    s.srs_put("note_types", "nt", &[("name", t("Basic")), ("fields", t(r#"["Front","Back"]"#)), ("templates", t(r#"[{"name":"Card 1","qfmt":"{{Front}}","afmt":"{{Back}}"}]"#))]).unwrap();
}

/// A note with `n_cards` cards (ord 0..n); returns the card ids.
pub fn note(s: &mut Store, id: &str, deck: &str, n_cards: i64) -> Vec<String> {
    s.srs_put("notes", id, &[("note_type_id", t("nt")), ("deck_id", t(deck)), ("fields", t(r#"["a","b"]"#))]).unwrap();
    (0..n_cards)
        .map(|ord| {
            let cid = classic_card_id(id, ord);
            s.srs_put("cards", &cid, &[("note_id", t(id)), ("deck_id", t(deck)), ("ord", i(ord))]).unwrap();
            cid
        })
        .collect()
}
