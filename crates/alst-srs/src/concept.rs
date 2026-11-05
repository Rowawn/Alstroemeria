//! Concepts, card creation and the status link (SPEC §11.2, §11.4, §11.5, §11.7).

use crate::fsrs::Rating;
use crate::pool::{self, Example, Segmenter};
use crate::replay::{self, Answered};
use crate::{Error, Result};
use alst_core::srs::{classic_card_id, concept_card_id};
use alst_core::store::derive_id;
use alst_core::{Store, TermPatch};
use rusqlite::{OptionalExtension, params, types::Value};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardCreation {
    OptIn,
    Automatic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLink {
    Linked,
    ReviewsOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeckKind {
    Classic,
    Concept,
}

/// The `srs.*` settings (SPEC §11.5, §11.7, §11.9).
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    pub card_creation: CardCreation,
    pub status_link: StatusLink,
    pub production_cards: bool,
    pub deck_kind: DeckKind,
}

pub fn settings(store: &Store) -> Result<Settings> {
    let text = |k: &str| store.get_setting(k).ok().flatten().and_then(|v| v.as_str().map(String::from));
    Ok(Settings {
        card_creation: if text("srs.cardCreation").as_deref() == Some("automatic") { CardCreation::Automatic } else { CardCreation::OptIn },
        status_link: if text("srs.statusLink").as_deref() == Some("reviews-only") { StatusLink::ReviewsOnly } else { StatusLink::Linked },
        production_cards: store.get_setting("srs.productionCards")?.and_then(|v| v.as_bool()).unwrap_or(false),
        deck_kind: if text("srs.defaultDeckKind").as_deref() == Some("classic") { DeckKind::Classic } else { DeckKind::Concept },
    })
}

fn t(s: &str) -> Value {
    Value::Text(s.to_string())
}

fn i(n: i64) -> Value {
    Value::Integer(n)
}

fn exists(store: &Store, tbl: &str, id: &str) -> Result<bool> {
    Ok(store.read_conn().query_row(&format!("SELECT count(*) FROM {tbl} WHERE id = ?1 AND deleted = 0"), [id], |r| Ok(r.get::<_, i64>(0)? > 0))?)
}

/// The built-in note types (SPEC §11.4, §11.5), with templates in the Anki language (§11.8).
pub const BUILTIN: [(&str, &str, &str, &str, &str); 2] = [
    (
        "vocab",
        "Alstroemeria Vocabulary",
        r#"["Term","Reading","Meaning","Notes"]"#,
        r#"[{"name":"Term to meaning","qfmt":"<div class=\"term\">{{Term}}</div>","afmt":"{{FrontSide}}<hr id=answer>{{#Reading}}<div class=\"reading\">{{Reading}}</div>{{/Reading}}<div class=\"meaning\">{{Meaning}}</div>{{#Notes}}<div class=\"notes\">{{Notes}}</div>{{/Notes}}"}]"#,
        ".card{font-family:system-ui,sans-serif;font-size:1.2em;text-align:center}.term{font-size:1.8em}.reading,.notes{opacity:.7}",
    ),
    (
        "sentence",
        "Alstroemeria Sentence",
        r#"["Sentence","Target","Meaning","SentenceMeaning","Audio","Source"]"#,
        r#"[{"name":"Sentence","qfmt":"<div class=\"sentence\">{{Sentence}}</div>","afmt":"{{FrontSide}}<hr id=answer><div class=\"target\">{{Target}}</div><div class=\"meaning\">{{Meaning}}</div>{{#SentenceMeaning}}<div class=\"tr\">{{SentenceMeaning}}</div>{{/SentenceMeaning}}{{Audio}}{{#Source}}<div class=\"source\">{{Source}}</div>{{/Source}}"}]"#,
        ".card{font-family:system-ui,sans-serif;font-size:1.2em;text-align:center}.sentence b{color:#c0392b}.tr,.source{opacity:.7;font-size:.85em}",
    ),
];

/// Make sure a built-in note type exists; returns its id.
pub fn ensure_builtin(store: &mut Store, key: &str) -> Result<String> {
    let id = derive_id("note_types", &[key]);
    if !exists(store, "note_types", &id)? {
        let (_, name, fields, templates, css) = BUILTIN.iter().find(|b| b.0 == key).ok_or_else(|| Error::Invalid(format!("no built-in note type {key}")))?;
        let kind = "standard";
        store.srs_put("note_types", &id, &[("name", t(name)), ("kind", t(kind)), ("fields", t(fields)), ("templates", t(templates)), ("css", t(css)), ("builtin", t(key))])?;
    }
    Ok(id)
}

/// The app's default config for a language (derived id, so devices agree).
pub fn ensure_config(store: &mut Store, language: &str) -> Result<String> {
    let id = derive_id("deck_configs", &[language]);
    if !exists(store, "deck_configs", &id)? {
        store.srs_put("deck_configs", &id, &[("name", t(&format!("Default ({language})")))])?;
    }
    Ok(id)
}

/// The app-made deck of a language and kind (derived id).
pub fn ensure_deck(store: &mut Store, language: &str, kind: DeckKind) -> Result<String> {
    let k = if kind == DeckKind::Concept { "concept" } else { "classic" };
    let id = derive_id("decks", &[language, k]);
    if !exists(store, "decks", &id)? {
        let config = ensure_config(store, language)?;
        let name = if kind == DeckKind::Concept { format!("Vocabulary ({language})") } else { format!("Vocabulary cards ({language})") };
        store.srs_put("decks", &id, &[("name", t(&name)), ("kind", t(k)), ("language", t(language)), ("config_id", t(&config))])?;
    }
    Ok(id)
}

/// The key a term's concept is found by: its lemma, else its text key (SPEC §16.3).
pub fn concept_key(lemma: Option<&str>, text_lc: &str) -> String {
    lemma.map(str::trim).filter(|l| !l.is_empty()).unwrap_or(text_lc).to_lowercase()
}

/// Find or create the concept of a term and point the term (and other forms with the same key) at it.
pub fn concept_for_term(store: &mut Store, term_id: &str) -> Result<String> {
    let term = store.get_term(term_id)?;
    let key = concept_key(term.lemma.as_deref(), &term.text_lc);
    let cid = store.ensure_concept(&term.language, "lexeme", &key)?;
    let others: Vec<String> = {
        let mut stmt = store.read_conn().prepare(
            "SELECT id FROM terms WHERE language = ?1 AND deleted = 0 AND (text_lc = ?2 OR lower(lemma) = ?2)
             AND COALESCE(concept_id, '') != ?3",
        )?;
        stmt.query_map(params![term.language, key, cid], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?
    };
    for id in others {
        store.set_term_concept(&id, &cid)?;
    }
    Ok(cid)
}

/// Cards (ids) that exist for a term's concept, live and not tombstoned.
pub fn cards_of_concept(store: &Store, concept_id: &str) -> Result<Vec<String>> {
    let mut stmt = store.read_conn().prepare(
        "SELECT c.id FROM cards c LEFT JOIN notes n ON n.id = c.note_id
         WHERE c.deleted = 0 AND (c.concept_id = ?1 OR (n.concept_id = ?1 AND n.deleted = 0)) ORDER BY c.created_at, c.id",
    )?;
    Ok(stmt.query_map([concept_id], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?)
}

/// "Add to review" (SPEC §11.5): the term's concept gets its card(s) in the app's deck. Existing cards are
/// kept. With `only_if_automatic` the call does nothing unless `srs.cardCreation` is `automatic` and the term has
/// status 1-5 and no card yet (the module's status listener uses this). Returns the ids of the cards created.
pub fn add_term(store: &mut Store, term_id: &str, only_if_automatic: bool) -> Result<Vec<String>> {
    let s = settings(store)?;
    let term = store.get_term(term_id)?;
    if only_if_automatic {
        if s.card_creation != CardCreation::Automatic || !(1..=5).contains(&term.status) {
            return Ok(vec![]);
        }
        let cid = concept_for_term(store, term_id)?;
        if !cards_of_concept(store, &cid)?.is_empty() {
            return Ok(vec![]);
        }
    }
    let cid = concept_for_term(store, term_id)?;
    let mut created = vec![];
    match s.deck_kind {
        DeckKind::Concept => {
            let deck = ensure_deck(store, &term.language, DeckKind::Concept)?;
            let dirs: &[(&str, i64)] = if s.production_cards { &[("recognition", 0), ("production", 1)] } else { &[("recognition", 0)] };
            for (dir, ord) in dirs {
                let id = concept_card_id(&cid, dir);
                if !exists(store, "cards", &id)? {
                    store.srs_put("cards", &id, &[("concept_id", t(&cid)), ("deck_id", t(&deck)), ("ord", i(*ord)), ("direction", t(dir))])?;
                    created.push(id);
                }
            }
        }
        DeckKind::Classic => {
            let deck = ensure_deck(store, &term.language, DeckKind::Classic)?;
            let nt = ensure_builtin(store, "vocab")?;
            let note = derive_id("notes", &["vocab", &cid]);
            let fields = json!([term.text, term.reading.clone().unwrap_or_default(), term.translation.clone().unwrap_or_default(), term.notes.clone().unwrap_or_default()]);
            if !exists(store, "notes", &note)? {
                store.srs_put(
                    "notes",
                    &note,
                    &[("note_type_id", t(&nt)), ("deck_id", t(&deck)), ("fields", t(&fields.to_string())), ("concept_id", t(&cid)), ("term_id", t(term_id)), ("tags", t("[]"))],
                )?;
            }
            let id = classic_card_id(&note, 0);
            if !exists(store, "cards", &id)? {
                store.srs_put("cards", &id, &[("note_id", t(&note)), ("deck_id", t(&deck)), ("ord", i(0))])?;
                created.push(id);
            }
        }
    }
    Ok(created)
}

/// Terms at status 1-5 that have no card yet (the "Add all learning words" preview and its work list).
pub fn learning_terms_without_cards(store: &Store, language: &str) -> Result<Vec<String>> {
    let mut stmt = store.read_conn().prepare(
        "SELECT t.id FROM terms t WHERE t.language = ?1 AND t.deleted = 0 AND t.status BETWEEN 1 AND 5
           AND NOT EXISTS (SELECT 1 FROM cards c LEFT JOIN notes n ON n.id = c.note_id
                           WHERE c.deleted = 0 AND t.concept_id IS NOT NULL AND (c.concept_id = t.concept_id OR n.concept_id = t.concept_id))
         ORDER BY t.created_at, t.id",
    )?;
    Ok(stmt.query_map([language], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?)
}

/// "Add all learning words": creates cards for every such term; returns how many terms got cards. Terms that
/// share a concept (forms of one word) get one set of cards.
pub fn add_all_learning(store: &mut Store, language: &str) -> Result<usize> {
    let mut n = 0;
    for id in learning_terms_without_cards(store, language)? {
        if !add_term(store, &id, false)?.is_empty() {
            n += 1;
        }
    }
    Ok(n)
}

/// A mined sentence (SPEC §11.5): the note of type "Alstroemeria Sentence" linked to the term's concept.
#[derive(Debug, Clone, Default)]
pub struct Mined {
    pub term_id: String,
    pub sentence: String,
    pub target: String,
    pub meaning: String,
    pub sentence_meaning: String,
    pub audio: String,
    pub source_label: String,
    /// `{workId, partId, start, end, cue?}`
    pub source: serde_json::Value,
    pub tags: Vec<String>,
    pub deck_id: Option<String>,
}

/// Save a mined sentence. In a concept deck it joins the concept's pool; in a classic deck it is a card with this
/// fixed sentence. Mining the same sentence for the same word twice is the same note. Returns `(note_id, card_ids)`.
pub fn mine_sentence(store: &mut Store, m: &Mined) -> Result<(String, Vec<String>)> {
    let term = store.get_term(&m.term_id)?;
    let cid = concept_for_term(store, &m.term_id)?;
    let nt = ensure_builtin(store, "sentence")?;
    let deck = match &m.deck_id {
        Some(d) => d.clone(),
        None => ensure_deck(store, &term.language, settings(store)?.deck_kind)?,
    };
    let kind: String = store.read_conn().query_row("SELECT kind FROM decks WHERE id = ?1 AND deleted = 0", [&deck], |r| r.get(0)).map_err(|_| Error::Invalid(format!("no deck {deck}")))?;
    let note = derive_id("notes", &["sentence", &cid, &m.sentence, &m.target]);
    let fields = json!([m.sentence, m.target, m.meaning, m.sentence_meaning, m.audio, m.source_label]);
    store.srs_put(
        "notes",
        &note,
        &[
            ("note_type_id", t(&nt)),
            ("deck_id", t(&deck)),
            ("fields", t(&fields.to_string())),
            ("tags", t(&serde_json::to_string(&m.tags).unwrap())),
            ("concept_id", t(&cid)),
            ("term_id", t(&m.term_id)),
            ("source", t(&m.source.to_string())),
        ],
    )?;
    let mut cards = vec![];
    if kind == "concept" {
        // The sentence feeds the concept's pool; the concept's own cards carry the review.
        let dirs = if settings(store)?.production_cards { vec![("recognition", 0), ("production", 1)] } else { vec![("recognition", 0)] };
        for (dir, ord) in dirs {
            let id = concept_card_id(&cid, dir);
            if !exists(store, "cards", &id)? {
                store.srs_put("cards", &id, &[("concept_id", t(&cid)), ("deck_id", t(&deck)), ("ord", i(ord)), ("direction", t(dir))])?;
                cards.push(id);
            }
        }
    } else {
        let id = classic_card_id(&note, 0);
        if !exists(store, "cards", &id)? {
            store.srs_put("cards", &id, &[("note_id", t(&note)), ("deck_id", t(&deck)), ("ord", i(0))])?;
            cards.push(id);
        }
    }
    Ok((note, cards))
}

/// Status from stability (SPEC §11.7).
pub fn status_for_stability(s: f64) -> i64 {
    match s {
        s if s < 1.0 => 1,
        s if s < 3.0 => 2,
        s if s < 7.0 => 3,
        s if s < 21.0 => 4,
        _ => 5,
    }
}

fn concept_of_card(store: &Store, card_id: &str) -> Result<Option<String>> {
    Ok(store
        .read_conn()
        .query_row("SELECT COALESCE(c.concept_id, n.concept_id) FROM cards c LEFT JOIN notes n ON n.id = c.note_id WHERE c.id = ?1", [card_id], |r| r.get::<_, Option<String>>(0))
        .optional()?
        .flatten())
}

/// After a review the terms of the card's concept follow its stability (SPEC §11.7, both modes). Never sets 98/99
/// and leaves a term that has one of those alone. Returns the `(term_id, new_status)` changes.
pub fn sync_status_from_reviews(store: &mut Store, card_id: &str) -> Result<Vec<(String, i64)>> {
    let Some(cid) = concept_of_card(store, card_id)? else { return Ok(vec![]) };
    let stab: Option<f64> = store.read_conn().query_row(
        "SELECT MAX(c.stability) FROM cards c LEFT JOIN notes n ON n.id = c.note_id
         WHERE c.deleted = 0 AND (c.concept_id = ?1 OR n.concept_id = ?1)",
        [&cid],
        |r| r.get(0),
    )?;
    let Some(stab) = stab else { return Ok(vec![]) };
    let status = status_for_stability(stab);
    let terms: Vec<(String, String, String, i64, i64)> = {
        let mut stmt = store.read_conn().prepare(
            "SELECT language, text, text_lc, word_count, status FROM terms WHERE concept_id = ?1 AND deleted = 0 AND status NOT IN (98, 99)",
        )?;
        stmt.query_map([&cid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect::<std::result::Result<_, _>>()?
    };
    let mut changed = vec![];
    for (lang, text, lc, wc, old) in terms {
        if old != status {
            let term = store.save_term(&lang, &text, &lc, wc, &TermPatch { status: Some(status), ..Default::default() })?;
            changed.push((term.id, status));
        }
    }
    Ok(changed)
}

/// Reaction to a term's status change made on this device (SPEC §11.5, §11.7): in `linked` mode, 98/99 suspends the
/// concept's cards (suspended = 2, "by status") and 1-5 releases those; in `automatic` creation mode a term that moves
/// to 1-5 gets its cards. Cards the user suspended by hand (suspended = 1) are never touched.
pub fn handle_status_change(store: &mut Store, term_id: &str, to: i64) -> Result<()> {
    let s = settings(store)?;
    let term = store.get_term(term_id)?;
    let cid = {
        let key = concept_key(term.lemma.as_deref(), &term.text_lc);
        let id = alst_core::srs::concept_id(&term.language, "lexeme", &key);
        if exists(store, "concepts", &id)? { Some(id) } else { None }
    };
    if s.status_link == StatusLink::Linked
        && let Some(cid) = &cid
    {
        let (from, set) = if to == 98 || to == 99 { (0, 2) } else if (1..=5).contains(&to) { (2, 0) } else { return after_status(store, term_id, to, &s) };
        for card in cards_of_concept(store, cid)? {
            let cur: i64 = store.read_conn().query_row("SELECT suspended FROM cards WHERE id = ?1", [&card], |r| r.get(0))?;
            if cur == from {
                store.srs_update("cards", &card, &[("suspended", i(set))])?;
            }
        }
    }
    after_status(store, term_id, to, &s)
}

fn after_status(store: &mut Store, term_id: &str, _to: i64, s: &Settings) -> Result<()> {
    if s.card_creation == CardCreation::Automatic {
        add_term(store, term_id, true)?;
    }
    Ok(())
}

/// A concept card ready to show (SPEC §11.4).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConceptCard {
    pub card_id: String,
    pub concept_id: String,
    pub direction: String,
    pub language: String,
    pub term: String,
    pub reading: Option<String>,
    pub meaning: Option<String>,
    pub example: Option<Example>,
}

/// The content of a concept card now: the concept's main term and the example the rotation picks.
pub fn concept_card(store: &Store, card_id: &str, seg: &dyn Segmenter) -> Result<ConceptCard> {
    let (cid, dir): (String, Option<String>) = store
        .read_conn()
        .query_row("SELECT concept_id, direction FROM cards WHERE id = ?1 AND concept_id IS NOT NULL", [card_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|_| Error::Invalid(format!("{card_id} is not a concept card")))?;
    let (lang, key): (String, String) = store.read_conn().query_row("SELECT language, key FROM concepts WHERE id = ?1", [&cid], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let row: Option<(String, Option<String>, Option<String>)> = store
        .read_conn()
        .query_row(
            "SELECT text, reading, translation FROM terms WHERE concept_id = ?1 AND deleted = 0
             ORDER BY (text_lc = ?2) DESC, (translation IS NOT NULL AND translation != '') DESC, word_count, created_at LIMIT 1",
            params![cid, key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (term, reading, meaning) = row.unwrap_or((key, None, None));
    Ok(ConceptCard { card_id: card_id.to_string(), concept_id: cid.clone(), direction: dir.unwrap_or_else(|| "recognition".into()), language: lang, term, reading, meaning, example: pool::pick(store, &cid, seg)? })
}

/// Record an answer for a card (concept or classic): history event, cache, example rotation and the status link.
pub fn answer_card(
    store: &mut Store,
    card_id: &str,
    rating: Rating,
    duration_ms: i64,
    example: Option<(&str, i64)>,
    now: i64,
    tz_offset_min: i64,
) -> Result<Answered> {
    let a = replay::answer(store, card_id, rating, duration_ms, example, now, tz_offset_min)?;
    if let (Some((part, start)), Some(cid)) = (example, concept_of_card(store, card_id)?) {
        pool::mark_shown(store, &cid, part, start, now)?;
    }
    sync_status_from_reviews(store, card_id)?;
    Ok(a)
}

/// An `exposure` event for a part that was read (SPEC §11.6): the concepts of its known terms. No effect on scheduling.
pub fn emit_exposure(store: &mut Store, part_id: &str, seg: &dyn Segmenter) -> Result<usize> {
    let (text, lang): (String, String) = store.read_conn().query_row(
        "SELECT p.text, w.language FROM parts p JOIN works w ON w.id = p.work_id WHERE p.id = ?1",
        [part_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let keys: std::collections::HashSet<String> = seg.tokens(&text).into_iter().map(|t| t.key).collect();
    let mut ids = std::collections::BTreeSet::new();
    let mut stmt = store.read_conn().prepare("SELECT concept_id FROM terms WHERE language = ?1 AND text_lc = ?2 AND concept_id IS NOT NULL AND deleted = 0")?;
    for k in &keys {
        for id in stmt.query_map(params![lang, k], |r| r.get::<_, String>(0))? {
            ids.insert(id?);
        }
    }
    drop(stmt);
    if ids.is_empty() {
        return Ok(0);
    }
    let n = ids.len();
    store.emit_event("exposure", Some(&lang), Some(("part", part_id)), &json!({"concept_ids": ids, "part_id": part_id}))?;
    Ok(n)
}
