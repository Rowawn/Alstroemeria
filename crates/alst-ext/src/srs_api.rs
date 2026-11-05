//! `alst.srs.*` (SPEC §11.1): the host API the SRS module (and any extension with `srs:read` / `srs:write`) uses.
//! Everything here is a thin layer over `alst-srs`; the permission is checked per method.

use crate::manifest::Manifest;
use crate::perm::{Need, allows};
use crate::{Error, Result};
use alst_core::Store;
use alst_core::media::MediaDir;
use alst_srs::anki::{self, ImportOptions};
use alst_srs::concept::{self, Mined};
use alst_srs::fsrs::{CardState, Queue, Rating};
use alst_srs::pool::{Segmenter, Tok};
use alst_srs::{render, replay, sched};
use alst_text::LanguageDef;
use rusqlite::{params, params_from_iter, types::Value as Sql};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// What the shell provides for imports and exports.
pub struct SrsEnv<'a> {
    pub media: &'a MediaDir,
    /// Scratch space for extracted collections.
    pub scratch: &'a Path,
    /// Where exported `.apkg` files are written.
    pub export_dir: &'a Path,
}

/// Word tokens of a language's tokenizer, keyed by term key (SPEC §16.3).
pub struct LangSeg<'a>(pub Option<&'a LanguageDef>);

impl Segmenter for LangSeg<'_> {
    fn tokens(&self, text: &str) -> Vec<Tok> {
        match self.0 {
            Some(l) => l.tokenize_flat(text).into_iter().filter(|t| t.is_word).map(|t| Tok { key: t.norm, start: t.start, end: t.end }).collect(),
            None => alst_srs::pool::SimpleSegmenter.tokens(text),
        }
    }
}

fn core(e: alst_core::Error) -> Error {
    Error::Invalid(e.to_string())
}

fn params_of<T: for<'de> Deserialize<'de>>(p: &Value) -> Result<T> {
    serde_json::from_value(p.clone()).map_err(|e| Error::Invalid(format!("bad parameters: {e}")))
}

fn need(m: &Manifest, n: Need, what: &str) -> Result<()> {
    if allows(m, &n) { Ok(()) } else { Err(Error::Permission(format!("'{}' needs the {what} permission", m.id))) }
}

fn now_ms() -> i64 {
    alst_core::hlc::wall_ms() as i64
}

fn sql(e: rusqlite::Error) -> Error {
    Error::Invalid(e.to_string())
}

fn lang_for<'a>(languages: &'a [LanguageDef], id: &str) -> Option<&'a LanguageDef> {
    languages.iter().find(|l| l.id == id)
}

/// The language of a card (through its deck), for the segmenter.
fn card_language(store: &Store, card_id: &str) -> Option<String> {
    store.read_conn().query_row("SELECT d.language FROM cards c JOIN decks d ON d.id = c.deck_id WHERE c.id = ?1", [card_id], |r| r.get(0)).ok().flatten()
}

fn t(s: &str) -> Sql {
    Sql::Text(s.to_string())
}

fn i(n: i64) -> Sql {
    Sql::Integer(n)
}

fn state_json(s: &CardState) -> Value {
    json!({"queue": s.queue.as_str(), "due": s.due, "stability": s.stability, "difficulty": s.difficulty, "step": s.step, "reps": s.reps, "lapses": s.lapses, "lastReview": s.last_review})
}

fn plain(html: &str) -> String {
    alst_srs::template::strip_html(html).chars().take(120).collect()
}

/// Dispatch a `srs.*` method (`name` is the part after `srs.`).
pub fn call(store: &mut Store, languages: &[LanguageDef], env: Option<&SrsEnv>, m: &Manifest, name: &str, p: &Value) -> Result<Value> {
    let tz = p.get("tz").and_then(Value::as_i64).unwrap_or(0).clamp(-14 * 60, 14 * 60);
    let seg_for = |lang: Option<String>| lang.and_then(|l| lang_for(languages, &l)).map(|l| l as &LanguageDef);
    match name {
        // ---- read -------------------------------------------------------------------------------
        "settings" => {
            need(m, Need::SrsRead, "srs:read")?;
            let s = concept::settings(store).map_err(core)?;
            Ok(json!({
                "cardCreation": if s.card_creation == concept::CardCreation::Automatic { "automatic" } else { "opt-in" },
                "statusLink": if s.status_link == concept::StatusLink::ReviewsOnly { "reviews-only" } else { "linked" },
                "productionCards": s.production_cards,
                "defaultDeckKind": if s.deck_kind == concept::DeckKind::Classic { "classic" } else { "concept" },
            }))
        }
        "decks" => {
            need(m, Need::SrsRead, "srs:read")?;
            let plan = sched::plan(store.read_conn(), now_ms(), tz).map_err(core)?;
            let mut out = vec![];
            for d in sched::decks(store.read_conn()).map_err(core)? {
                let c = plan.counts.iter().find(|c| c.deck_id == d.id);
                let (language, total, suspended): (Option<String>, i64, i64) = store
                    .read_conn()
                    .query_row("SELECT d.language, (SELECT count(*) FROM cards WHERE deck_id = d.id AND deleted = 0), (SELECT count(*) FROM cards WHERE deck_id = d.id AND deleted = 0 AND suspended != 0) FROM decks d WHERE d.id = ?1", [&d.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                    .map_err(sql)?;
                out.push(json!({"id": d.id, "name": d.name, "kind": d.kind, "language": language, "configId": d.config_id,
                                "new": c.map_or(0, |c| c.new), "learning": c.map_or(0, |c| c.learning), "review": c.map_or(0, |c| c.review), "total": total, "suspended": suspended}));
            }
            Ok(json!(out))
        }
        "queue" => {
            need(m, Need::SrsRead, "srs:read")?;
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                deck_id: Option<String>,
                limit: Option<usize>,
            }
            let P { deck_id, limit } = params_of(p)?;
            let items = match deck_id {
                Some(d) => sched::queue_for(store.read_conn(), &d, now_ms(), tz).map_err(core)?,
                None => sched::plan(store.read_conn(), now_ms(), tz).map_err(core)?.items,
            };
            Ok(json!(items.into_iter().take(limit.unwrap_or(50).min(500)).map(|q| json!({"cardId": q.card_id, "deckId": q.deck_id, "queue": q.queue.as_str(), "due": q.due})).collect::<Vec<_>>()))
        }
        "card" => {
            need(m, Need::SrsRead, "srs:read")?;
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                card_id: String,
            }
            let P { card_id } = params_of(p)?;
            let lang = card_language(store, &card_id);
            let seg = LangSeg(seg_for(lang));
            let rendered = render::render_card(store, &card_id, &seg).map_err(core)?;
            let row = replay::card_row(store, &card_id).map_err(core)?.ok_or_else(|| Error::Invalid(format!("no card {card_id}")))?;
            let state = replay::card_state(store, &card_id).map_err(core)?.unwrap_or_default();
            // When each answer would bring the card back (no fuzz, for the button labels).
            let mut cfg = sched::load_config(store.read_conn(), row.config_id.as_deref()).map_err(core)?;
            cfg.sched.fuzz = false;
            let clock = sched::Clock { tz_offset_min: tz, day_starts_hour: cfg.day_starts_hour };
            let now = now_ms();
            let policy = sched::policy_for(&row.deck_kind);
            let next: Vec<Value> = [Rating::Again, Rating::Hard, Rating::Good, Rating::Easy]
                .iter()
                .map(|r| {
                    let s = policy.answer(&card_id, &state, *r, now, &cfg, &clock);
                    json!({"rating": *r as i64, "queue": s.queue.as_str(), "due": s.due, "inMs": s.due - now})
                })
                .collect();
            let (susp,): (i64,) = store.read_conn().query_row("SELECT suspended FROM cards WHERE id = ?1", [&card_id], |r| Ok((r.get(0)?,))).map_err(sql)?;
            // Media referenced by name in the card's HTML, resolved to store hashes.
            let mut media = serde_json::Map::new();
            for html in [&rendered.front, &rendered.back] {
                for name in media_names(html) {
                    let h: Option<String> = store.read_conn().query_row("SELECT media_hash FROM note_media WHERE id = ?1 AND deleted = 0", [alst_core::store::derive_id("note_media", &[&name])], |r| r.get(0)).ok();
                    if let Some(h) = h {
                        media.insert(name, Value::String(h));
                    }
                }
            }
            Ok(json!({"cardId": card_id, "rendered": rendered, "state": state_json(&state), "next": next, "suspended": susp != 0, "media": media}))
        }
        "browse" => {
            need(m, Need::SrsRead, "srs:read")?;
            #[derive(Deserialize, Default)]
            #[serde(rename_all = "camelCase", default)]
            struct P {
                text: String,
                deck_id: Option<String>,
                tag: Option<String>,
                state: Option<String>,
                suspended: Option<bool>,
                limit: Option<i64>,
                offset: Option<i64>,
            }
            let P { text, deck_id, tag, state, suspended, limit, offset } = params_of(p)?;
            let mut where_ = vec!["c.deleted = 0".to_string()];
            let mut args: Vec<Sql> = vec![];
            if !text.trim().is_empty() {
                where_.push("(COALESCE(n.fields, '') LIKE ? OR COALESCE(k.key, '') LIKE ? OR COALESCE(tm.translation, '') LIKE ?)".into());
                let like = format!("%{}%", text.trim().replace('%', ""));
                args.extend([t(&like), t(&like), t(&like)]);
            }
            if let Some(d) = deck_id {
                where_.push("(d.id = ? OR d.name LIKE (SELECT name FROM decks WHERE id = ?) || '::%')".into());
                args.extend([t(&d), t(&d)]);
            }
            if let Some(tag) = tag.filter(|t| !t.is_empty()) {
                where_.push("n.tags LIKE ?".into());
                args.push(t(&format!("%\"{}\"%", tag.replace('"', ""))));
            }
            if let Some(s) = state.filter(|s| !s.is_empty()) {
                where_.push("c.queue = ?".into());
                args.push(t(&s));
            }
            if let Some(s) = suspended {
                where_.push(if s { "c.suspended != 0".into() } else { "c.suspended = 0".to_string() });
            }
            let q = format!(
                "SELECT c.id, c.note_id, c.deck_id, d.name, c.queue, c.due, c.suspended, c.stability, n.fields, n.tags, k.key, tm.translation, c.direction
                 FROM cards c JOIN decks d ON d.id = c.deck_id LEFT JOIN notes n ON n.id = c.note_id LEFT JOIN concepts k ON k.id = c.concept_id
                 LEFT JOIN terms tm ON tm.concept_id = k.id AND tm.text_lc = k.key AND tm.deleted = 0
                 WHERE {} ORDER BY c.created_at, c.id LIMIT {} OFFSET {}",
                where_.join(" AND "),
                limit.unwrap_or(100).clamp(1, 500),
                offset.unwrap_or(0).max(0)
            );
            let conn = store.read_conn();
            let mut stmt = conn.prepare(&q).map_err(sql)?;
            let rows = stmt
                .query_map(params_from_iter(args), |r| {
                    let fields: Option<String> = r.get(8)?;
                    let key: Option<String> = r.get(10)?;
                    let first = fields.and_then(|f| serde_json::from_str::<Vec<String>>(&f).ok()).and_then(|v| v.into_iter().next()).map(|f| plain(&f));
                    Ok(json!({
                        "cardId": r.get::<_, String>(0)?, "noteId": r.get::<_, Option<String>>(1)?, "deckId": r.get::<_, String>(2)?, "deck": r.get::<_, String>(3)?,
                        "queue": r.get::<_, String>(4)?, "due": r.get::<_, i64>(5)?, "suspended": r.get::<_, i64>(6)? != 0, "stability": r.get::<_, Option<f64>>(7)?,
                        "title": first.or(key).unwrap_or_default(), "tags": r.get::<_, Option<String>>(9)?.and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or(json!([])),
                        "meaning": r.get::<_, Option<String>>(11)?, "direction": r.get::<_, Option<String>>(12)?,
                    }))
                })
                .map_err(sql)?;
            Ok(json!(rows.collect::<std::result::Result<Vec<_>, _>>().map_err(sql)?))
        }
        "note" => {
            need(m, Need::SrsRead, "srs:read")?;
            let id = p.get("noteId").and_then(Value::as_str).ok_or_else(|| Error::Invalid("noteId is required".into()))?;
            let conn = store.read_conn();
            let (nt, fields, tags, deck, guid): (String, String, String, String, Option<String>) = conn
                .query_row("SELECT note_type_id, fields, tags, deck_id, guid FROM notes WHERE id = ?1 AND deleted = 0", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
                .map_err(|_| Error::Invalid(format!("no note {id}")))?;
            let (ntname, names): (String, String) = conn.query_row("SELECT name, fields FROM note_types WHERE id = ?1", [&nt], |r| Ok((r.get(0)?, r.get(1)?))).map_err(sql)?;
            let mut stmt = conn.prepare("SELECT id, ord, queue FROM cards WHERE note_id = ?1 AND deleted = 0 ORDER BY ord").map_err(sql)?;
            let cards: Vec<Value> = stmt.query_map([id], |r| Ok(json!({"id": r.get::<_, String>(0)?, "ord": r.get::<_, i64>(1)?, "queue": r.get::<_, String>(2)?}))).map_err(sql)?.filter_map(|r| r.ok()).collect();
            Ok(json!({"id": id, "noteTypeId": nt, "noteType": ntname, "fieldNames": serde_json::from_str::<Value>(&names).unwrap_or(json!([])),
                      "fields": serde_json::from_str::<Value>(&fields).unwrap_or(json!([])), "tags": serde_json::from_str::<Value>(&tags).unwrap_or(json!([])), "deckId": deck, "guid": guid, "cards": cards}))
        }
        "noteTypes" => {
            need(m, Need::SrsRead, "srs:read")?;
            let conn = store.read_conn();
            let mut stmt = conn.prepare("SELECT id, name, kind, fields FROM note_types WHERE deleted = 0 ORDER BY name").map_err(sql)?;
            let rows = stmt.query_map([], |r| Ok(json!({"id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?, "kind": r.get::<_, String>(2)?,
                "fields": serde_json::from_str::<Value>(&r.get::<_, String>(3)?).unwrap_or(json!([]))}))).map_err(sql)?;
            Ok(json!(rows.collect::<std::result::Result<Vec<_>, _>>().map_err(sql)?))
        }
        "stats" => {
            need(m, Need::SrsRead, "srs:read")?;
            Ok(stats(store, now_ms(), tz)?)
        }
        "refreshPool" => {
            need(m, Need::SrsRead, "srs:read")?;
            let lang = p.get("language").and_then(Value::as_str).ok_or_else(|| Error::Invalid("language is required".into()))?;
            let seg = LangSeg(lang_for(languages, lang));
            let n = alst_srs::pool::refresh_language(store, lang, &seg).map_err(core)?;
            Ok(json!({"rows": n, "upToDate": n.is_none()}))
        }
        "learningCount" => {
            need(m, Need::SrsRead, "srs:read")?;
            let lang = p.get("language").and_then(Value::as_str).ok_or_else(|| Error::Invalid("language is required".into()))?;
            Ok(json!({"count": concept::learning_terms_without_cards(store, lang).map_err(core)?.len()}))
        }
        "deckConfig" => {
            need(m, Need::SrsRead, "srs:read")?;
            let deck = p.get("deckId").and_then(Value::as_str).ok_or_else(|| Error::Invalid("deckId is required".into()))?;
            let conn = store.read_conn();
            let cid: Option<String> = conn.query_row("SELECT config_id FROM decks WHERE id = ?1", [deck], |r| r.get(0)).map_err(sql)?;
            let cid = cid.ok_or_else(|| Error::Invalid("deck has no options".into()))?;
            conn.query_row(
                "SELECT id, name, new_per_day, reviews_per_day, desired_retention, learning_steps, relearning_steps, max_interval_days, bury_siblings, day_starts_hour FROM deck_configs WHERE id = ?1",
                [&cid],
                |r| Ok(json!({"id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?, "newPerDay": r.get::<_, i64>(2)?, "reviewsPerDay": r.get::<_, i64>(3)?,
                    "desiredRetention": r.get::<_, f64>(4)?, "learningSteps": serde_json::from_str::<Value>(&r.get::<_, String>(5)?).unwrap_or(json!([])),
                    "relearningSteps": serde_json::from_str::<Value>(&r.get::<_, String>(6)?).unwrap_or(json!([])), "maxIntervalDays": r.get::<_, i64>(7)?,
                    "burySiblings": r.get::<_, i64>(8)? != 0, "dayStartsHour": r.get::<_, i64>(9)?})),
            )
            .map_err(sql)
        }
        // ---- write ------------------------------------------------------------------------------
        "answer" => {
            need(m, Need::SrsWrite, "srs:write")?;
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct Ex {
                part_id: String,
                start: i64,
            }
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                card_id: String,
                rating: i64,
                #[serde(default)]
                duration_ms: i64,
                example: Option<Ex>,
            }
            let P { card_id, rating, duration_ms, example } = params_of(p)?;
            let rating = Rating::from_i64(rating).ok_or_else(|| Error::Invalid("rating must be 1-4".into()))?;
            let a = concept::answer_card(store, &card_id, rating, duration_ms.clamp(0, 3_600_000), example.as_ref().map(|e| (e.part_id.as_str(), e.start)), now_ms(), tz).map_err(core)?;
            Ok(json!({"reviewId": a.review_id, "state": state_json(&a.state)}))
        }
        "undo" => {
            need(m, Need::SrsWrite, "srs:write")?;
            let card_id = p.get("cardId").and_then(Value::as_str).ok_or_else(|| Error::Invalid("cardId is required".into()))?;
            let undone = replay::undo(store, card_id).map_err(core)?;
            // The status follows the restored stability.
            concept::sync_status_from_reviews(store, card_id).map_err(core)?;
            Ok(json!({"undone": undone}))
        }
        "addTerm" => {
            need(m, Need::SrsWrite, "srs:write")?;
            let only_auto = p.get("onlyIfAutomatic").and_then(Value::as_bool).unwrap_or(false);
            let id = resolve_term(store, languages, p)?;
            Ok(json!({"termId": id, "created": concept::add_term(store, &id, only_auto).map_err(core)?}))
        }
        "addAllLearning" => {
            need(m, Need::SrsWrite, "srs:write")?;
            let lang = p.get("language").and_then(Value::as_str).ok_or_else(|| Error::Invalid("language is required".into()))?;
            Ok(json!({"terms": concept::add_all_learning(store, lang).map_err(core)?}))
        }
        "onTermStatus" => {
            need(m, Need::SrsWrite, "srs:write")?;
            let id = p.get("termId").and_then(Value::as_str).ok_or_else(|| Error::Invalid("termId is required".into()))?;
            let to = p.get("to").and_then(Value::as_i64).ok_or_else(|| Error::Invalid("to is required".into()))?;
            concept::handle_status_change(store, id, to).map_err(core)?;
            Ok(Value::Null)
        }
        "mine" => {
            need(m, Need::SrsWrite, "srs:write")?;
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", default)]
            #[derive(Default)]
            struct P {
                term_id: String,
                language: String,
                text: String,
                sentence: String,
                target: String,
                meaning: String,
                sentence_meaning: String,
                audio: String,
                source_label: String,
                source: Value,
                tags: Vec<String>,
                deck_id: Option<String>,
            }
            let q: P = params_of(p)?;
            if q.sentence.trim().is_empty() {
                return Err(Error::Invalid("a sentence is required".into()));
            }
            let term_id = resolve_term(store, languages, p)?;
            let _ = (&q.language, &q.text);
            let (note, cards) = concept::mine_sentence(
                store,
                &Mined { term_id, sentence: q.sentence, target: q.target, meaning: q.meaning, sentence_meaning: q.sentence_meaning, audio: q.audio, source_label: q.source_label, source: q.source, tags: q.tags, deck_id: q.deck_id },
            )
            .map_err(core)?;
            Ok(json!({"noteId": note, "cardIds": cards}))
        }
        "editNote" => {
            need(m, Need::SrsWrite, "srs:write")?;
            let id = p.get("noteId").and_then(Value::as_str).ok_or_else(|| Error::Invalid("noteId is required".into()))?;
            let mut cols: Vec<(&str, Sql)> = vec![];
            if let Some(f) = p.get("fields").filter(|f| f.is_array()) {
                cols.push(("fields", t(&f.to_string())));
            }
            if let Some(f) = p.get("tags").filter(|f| f.is_array()) {
                cols.push(("tags", t(&f.to_string())));
            }
            if cols.is_empty() {
                return Ok(Value::Null);
            }
            store.srs_update("notes", id, &cols).map_err(core)?;
            Ok(Value::Null)
        }
        "suspend" => {
            need(m, Need::SrsWrite, "srs:write")?;
            let ids = strings(p, "cardIds")?;
            let on = p.get("suspended").and_then(Value::as_bool).unwrap_or(true);
            for id in ids {
                store.srs_update("cards", &id, &[("suspended", i(on as i64))]).map_err(core)?;
            }
            Ok(Value::Null)
        }
        "suspendDeck" => {
            need(m, Need::SrsWrite, "srs:write")?;
            let deck = p.get("deckId").and_then(Value::as_str).ok_or_else(|| Error::Invalid("deckId is required".into()))?;
            let on = p.get("suspended").and_then(Value::as_bool).unwrap_or(true);
            // Only the user's own suspension moves; a status-link suspension (2) stays with the word's status (SPEC §11.7).
            let (from, to) = if on { (0, 1) } else { (1, 0) };
            let ids = card_ids(store, deck, Some(from))?;
            for id in ids {
                store.srs_update("cards", &id, &[("suspended", i(to))]).map_err(core)?;
            }
            Ok(Value::Null)
        }
        "deleteDeck" => {
            need(m, Need::SrsWrite, "srs:write")?;
            let deck = p.get("deckId").and_then(Value::as_str).ok_or_else(|| Error::Invalid("deckId is required".into()))?;
            let notes: Vec<String> = {
                let mut stmt = store.read_conn().prepare("SELECT id FROM notes WHERE deck_id = ?1 AND deleted = 0").map_err(sql)?;
                let rows = stmt.query_map([deck], |r| r.get(0)).map_err(sql)?;
                rows.collect::<std::result::Result<_, _>>().map_err(sql)?
            };
            for id in card_ids(store, deck, None)? {
                store.srs_delete("cards", &id).map_err(core)?;
            }
            for n in notes {
                store.srs_delete("notes", &n).map_err(core)?;
            }
            store.srs_delete("decks", deck).map_err(core)?;
            Ok(Value::Null)
        }
        "moveCards" => {
            need(m, Need::SrsWrite, "srs:write")?;
            let ids = strings(p, "cardIds")?;
            let deck = p.get("deckId").and_then(Value::as_str).ok_or_else(|| Error::Invalid("deckId is required".into()))?;
            for id in ids {
                store.srs_update("cards", &id, &[("deck_id", t(deck))]).map_err(core)?;
                let note: Option<String> = store.read_conn().query_row("SELECT note_id FROM cards WHERE id = ?1", [&id], |r| r.get(0)).map_err(sql)?;
                if let Some(n) = note {
                    store.srs_update("notes", &n, &[("deck_id", t(deck))]).map_err(core)?;
                }
            }
            Ok(Value::Null)
        }
        "deleteCards" => {
            need(m, Need::SrsWrite, "srs:write")?;
            let ids = strings(p, "cardIds")?;
            for id in &ids {
                let note: Option<String> = store.read_conn().query_row("SELECT note_id FROM cards WHERE id = ?1", [id], |r| r.get(0)).map_err(sql)?;
                store.srs_delete("cards", id).map_err(core)?;
                if let Some(n) = note {
                    let left: i64 = store.read_conn().query_row("SELECT count(*) FROM cards WHERE note_id = ?1 AND deleted = 0", [&n], |r| r.get(0)).map_err(sql)?;
                    if left == 0 {
                        store.srs_delete("notes", &n).map_err(core)?;
                    }
                }
            }
            Ok(Value::Null)
        }
        "createDeck" => {
            need(m, Need::SrsWrite, "srs:write")?;
            let name = p.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty() && n.len() <= 100).ok_or_else(|| Error::Invalid("a deck name of 1-100 characters is required".into()))?;
            let kind = if p.get("kind").and_then(Value::as_str) == Some("concept") { "concept" } else { "classic" };
            let language = p.get("language").and_then(Value::as_str).map(String::from);
            let id = alst_core::store::derive_id("decks", &["user", name]);
            let config = concept::ensure_config(store, language.as_deref().unwrap_or("any")).map_err(core)?;
            store
                .srs_put("decks", &id, &[("name", t(name)), ("kind", t(kind)), ("language", language.as_deref().map(t).unwrap_or(Sql::Null)), ("config_id", t(&config))])
                .map_err(core)?;
            Ok(json!({"id": id}))
        }
        "setDeckConfig" => {
            need(m, Need::SrsWrite, "srs:write")?;
            let id = p.get("configId").and_then(Value::as_str).ok_or_else(|| Error::Invalid("configId is required".into()))?;
            let mut cols: Vec<(&str, Sql)> = vec![];
            let int = |k: &str, lo: i64, hi: i64| p.get(k).and_then(Value::as_i64).map(|v| v.clamp(lo, hi));
            if let Some(v) = int("newPerDay", 0, 9999) { cols.push(("new_per_day", i(v))); }
            if let Some(v) = int("reviewsPerDay", 0, 9999) { cols.push(("reviews_per_day", i(v))); }
            if let Some(v) = int("maxIntervalDays", 1, 36500) { cols.push(("max_interval_days", i(v))); }
            if let Some(v) = int("dayStartsHour", 0, 23) { cols.push(("day_starts_hour", i(v))); }
            if let Some(v) = p.get("burySiblings").and_then(Value::as_bool) { cols.push(("bury_siblings", i(v as i64))); }
            if let Some(v) = p.get("desiredRetention").and_then(Value::as_f64) { cols.push(("desired_retention", Sql::Real(v.clamp(0.7, 0.99)))); }
            for (k, col) in [("learningSteps", "learning_steps"), ("relearningSteps", "relearning_steps")] {
                if let Some(a) = p.get(k).and_then(Value::as_array) {
                    let steps: Vec<String> = a.iter().filter_map(Value::as_str).map(String::from).collect();
                    if steps.iter().any(|s| alst_srs::fsrs::parse_step(s).is_none()) {
                        return Err(Error::Invalid(format!("{k}: use values like 1m, 10m, 1h, 1d")));
                    }
                    cols.push((col, t(&serde_json::to_string(&steps).unwrap())));
                }
            }
            if !cols.is_empty() {
                store.srs_update("deck_configs", id, &cols).map_err(core)?;
                // New options change the schedule of replayed cards.
                let ids: Vec<String> = {
                    let conn = store.read_conn();
                    let mut stmt = conn.prepare("SELECT c.id FROM cards c JOIN decks d ON d.id = c.deck_id WHERE d.config_id = ?1 AND c.queue != 'new'").map_err(sql)?;
                    stmt.query_map([id], |r| r.get(0)).map_err(sql)?.filter_map(|r| r.ok()).collect()
                };
                store.replay_cards(&ids).map_err(core)?;
            }
            Ok(Value::Null)
        }
        "setSetting" => {
            need(m, Need::SrsWrite, "srs:write")?;
            let key = p.get("key").and_then(Value::as_str).unwrap_or("");
            let value = p.get("value").cloned().unwrap_or(Value::Null);
            let ok = match key {
                "srs.cardCreation" => matches!(value.as_str(), Some("opt-in" | "automatic")),
                "srs.statusLink" => matches!(value.as_str(), Some("linked" | "reviews-only")),
                "srs.defaultDeckKind" => matches!(value.as_str(), Some("classic" | "concept")),
                "srs.productionCards" => value.is_boolean(),
                _ => false,
            };
            if !ok {
                return Err(Error::Invalid(format!("not an SRS setting or bad value: {key}")));
            }
            store.set_setting(key, &value).map_err(core)?;
            Ok(Value::Null)
        }
        "importApkg" => {
            need(m, Need::SrsWrite, "srs:write")?;
            let env = env.ok_or_else(|| Error::Invalid("import is not available here".into()))?;
            let upload = p.get("uploadId").and_then(Value::as_str).ok_or_else(|| Error::Invalid("uploadId is required".into()))?;
            let language = p.get("language").and_then(Value::as_str).map(String::from);
            let path = env.media.staged_path(upload).map_err(core)?;
            let result = anki::import_apkg(store, &path, &ImportOptions { media: env.media, tmp_dir: env.scratch, now_ms: now_ms(), tz_offset_min: tz, language });
            env.media.abort(upload);
            Ok(json!(result.map_err(core)?))
        }
        "exportApkg" => {
            need(m, Need::SrsRead, "srs:read")?;
            let env = env.ok_or_else(|| Error::Invalid("export is not available here".into()))?;
            let decks = strings(p, "deckIds")?;
            std::fs::create_dir_all(env.export_dir).map_err(|e| Error::Invalid(e.to_string()))?;
            let lang = decks.first().and_then(|d| store.read_conn().query_row("SELECT language FROM decks WHERE id = ?1", [d], |r| r.get::<_, Option<String>>(0)).ok().flatten());
            let seg = LangSeg(seg_for(lang));
            let stamp = now_ms();
            let path: PathBuf = env.export_dir.join(format!("alstroemeria-{stamp}.apkg"));
            let rep = anki::export_apkg(store, &decks, &path, env.media, &seg, stamp, tz).map_err(core)?;
            Ok(json!({"path": path.display().to_string(), "report": rep}))
        }
        _ => Err(Error::Invalid(format!("unknown method srs.{name}"))),
    }
}

/// The term a call is about: `termId`, or `language` + `text` (found by term key, else created at status 1).
/// Live cards of a deck, optionally only those with one `suspended` value.
fn card_ids(store: &Store, deck: &str, suspended: Option<i64>) -> Result<Vec<String>> {
    let mut stmt = store
        .read_conn()
        .prepare("SELECT id FROM cards WHERE deck_id = ?1 AND deleted = 0 AND (?2 IS NULL OR suspended = ?2)")
        .map_err(sql)?;
    let rows = stmt.query_map(params![deck, suspended], |r| r.get(0)).map_err(sql)?;
    rows.collect::<std::result::Result<_, _>>().map_err(sql)
}

fn resolve_term(store: &mut Store, languages: &[LanguageDef], p: &Value) -> Result<String> {
    if let Some(id) = p.get("termId").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        return Ok(id.to_string());
    }
    let (Some(language), Some(text)) = (p.get("language").and_then(Value::as_str), p.get("text").and_then(Value::as_str)) else {
        return Err(Error::Invalid("termId, or language and text, is required".into()));
    };
    let lang = lang_for(languages, language).ok_or_else(|| Error::Invalid(format!("language '{language}' is not installed")))?;
    let info = lang.term_info(text).ok_or_else(|| Error::Invalid("that text has no words".into()))?;
    if let Some(t) = store.find_term(language, &info.key).map_err(core)? {
        return Ok(t.id);
    }
    let created = store
        .save_term(language, &info.display, &info.key, info.word_count as i64, &alst_core::TermPatch { status: Some(1), ..Default::default() })
        .map_err(core)?;
    Ok(created.id)
}

fn strings(p: &Value, key: &str) -> Result<Vec<String>> {
    let a = p.get(key).and_then(Value::as_array).ok_or_else(|| Error::Invalid(format!("{key} is required")))?;
    Ok(a.iter().filter_map(Value::as_str).take(5000).map(String::from).collect())
}

/// File names in `src="…"` of HTML, for resolving media.
fn media_names(html: &str) -> Vec<String> {
    let mut out = vec![];
    for q in ["src=\"", "src='"] {
        let close = &q[q.len() - 1..];
        let mut rest = html;
        while let Some(p) = rest.find(q) {
            let after = &rest[p + q.len()..];
            match after.find(close) {
                Some(e) => {
                    out.push(after[..e].to_string());
                    rest = &after[e..];
                }
                None => break,
            }
        }
    }
    for part in html.split("data-sound=\"").skip(1) {
        if let Some(e) = part.find('"') {
            out.push(part[..e].to_string());
        }
    }
    out.retain(|n| !n.starts_with("alstclip:") && !n.is_empty());
    out.sort();
    out.dedup();
    out
}

/// Due forecast, reviews per day, true retention and card counts (SPEC §11.9).
fn stats(store: &Store, now: i64, tz: i64) -> Result<Value> {
    let conn = store.read_conn();
    let clock = sched::Clock { tz_offset_min: tz, day_starts_hour: 4 };
    let today = clock.day_index(now);
    let mut forecast = vec![0i64; 30];
    {
        let mut stmt = conn.prepare("SELECT due FROM cards WHERE deleted = 0 AND suspended = 0 AND queue IN ('review', 'learning', 'relearning')").map_err(sql)?;
        for due in stmt.query_map([], |r| r.get::<_, i64>(0)).map_err(sql)? {
            let d = (clock.day_index(due.map_err(sql)?) - today).max(0);
            if d < 30 {
                forecast[d as usize] += 1;
            }
        }
    }
    let since = clock.day_start(today - 29);
    let mut per_day = vec![0i64; 30];
    let (mut passed, mut total) = (0i64, 0i64);
    {
        let mut stmt = conn
            .prepare(
                "SELECT COALESCE(json_extract(e.payload, '$.at'), e.hlc >> 16), json_extract(e.payload, '$.rating'), json_extract(e.payload, '$.before.queue')
                 FROM events e WHERE e.type = 'review' AND COALESCE(json_extract(e.payload, '$.at'), e.hlc >> 16) >= ?1
                   AND NOT EXISTS (SELECT 1 FROM events u WHERE u.type = 'review.undo' AND u.subject_id = e.subject_id AND json_extract(u.payload, '$.review_id') = e.id)",
            )
            .map_err(sql)?;
        for r in stmt.query_map(params![since], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, Option<String>>(2)?))).map_err(sql)? {
            let (at, rating, before) = r.map_err(sql)?;
            let idx = clock.day_index(at) - (today - 29);
            if (0..30).contains(&idx) {
                per_day[idx as usize] += 1;
            }
            // True retention: answers to cards that were already in the review queue.
            if before.as_deref() == Some("review") {
                total += 1;
                if rating.unwrap_or(1) > 1 {
                    passed += 1;
                }
            }
        }
    }
    let count = |q: &str| -> i64 { conn.query_row(q, [], |r| r.get(0)).unwrap_or(0) };
    Ok(json!({
        "forecast": forecast,
        "reviewsPerDay": per_day,
        "retention": if total > 0 { json!(passed as f64 / total as f64) } else { Value::Null },
        "retentionSample": total,
        "cards": {
            "new": count("SELECT count(*) FROM cards WHERE deleted = 0 AND suspended = 0 AND queue = 'new'"),
            "learning": count("SELECT count(*) FROM cards WHERE deleted = 0 AND suspended = 0 AND queue IN ('learning', 'relearning')"),
            "review": count("SELECT count(*) FROM cards WHERE deleted = 0 AND suspended = 0 AND queue = 'review'"),
            "suspended": count("SELECT count(*) FROM cards WHERE deleted = 0 AND suspended != 0"),
        },
        "queue": { "new": Queue::New.as_str() },
    }))
}
