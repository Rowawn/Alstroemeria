//! Review history: answers append events, and a card's scheduling columns are a cache of replaying them in
//! `(at, hlc, site)` order (SPEC §11.6). Replay after sync makes devices that reviewed offline agree.

use crate::fsrs::{CardState, Queue, Rating};
use crate::sched::{self, Clock};
use crate::{Error, Result};
use alst_core::Store;
use alst_core::srs::{CardCache, CardEvent};
use rusqlite::OptionalExtension;
use serde_json::{Value, json};
use std::collections::HashSet;

/// Install the replay hook on a store. Every store that applies synced batches must call this.
pub fn attach(store: &mut Store) {
    store.set_replay_hook(Box::new(|s, ids| {
        for id in ids {
            replay_card(s, id)?;
        }
        Ok(())
    }));
}

fn cache_of(s: &CardState) -> CardCache {
    CardCache {
        queue: s.queue.as_str().to_string(),
        due: s.due,
        stability: s.stability,
        difficulty: s.difficulty,
        step: s.step,
        reps: s.reps,
        lapses: s.lapses,
        last_review: s.last_review,
    }
}

fn state_of(c: &CardCache) -> CardState {
    CardState {
        queue: Queue::parse(&c.queue),
        due: c.due,
        stability: c.stability,
        difficulty: c.difficulty,
        step: c.step,
        reps: c.reps,
        lapses: c.lapses,
        last_review: c.last_review,
    }
}

/// A card row as the scheduler needs it.
pub struct CardRow {
    pub id: String,
    pub deck_id: String,
    pub deck_kind: String,
    pub config_id: Option<String>,
    pub language: Option<String>,
    pub cache: CardCache,
}

pub fn card_row(store: &Store, id: &str) -> Result<Option<CardRow>> {
    Ok(store
        .read_conn()
        .query_row(
            "SELECT c.id, c.deck_id, COALESCE(d.kind, 'classic'), d.config_id, d.language, c.queue, c.due, c.stability,
                    c.difficulty, c.step, c.reps, c.lapses, c.last_review
             FROM cards c LEFT JOIN decks d ON d.id = c.deck_id WHERE c.id = ?1 AND c.deleted = 0",
            [id],
            |r| {
                Ok(CardRow {
                    id: r.get(0)?,
                    deck_id: r.get(1)?,
                    deck_kind: r.get(2)?,
                    config_id: r.get(3)?,
                    language: r.get(4)?,
                    cache: CardCache {
                        queue: r.get(5)?,
                        due: r.get(6)?,
                        stability: r.get(7)?,
                        difficulty: r.get(8)?,
                        step: r.get(9)?,
                        reps: r.get(10)?,
                        lapses: r.get(11)?,
                        last_review: r.get(12)?,
                    },
                })
            },
        )
        .optional()?)
}

fn at_of(e: &CardEvent) -> i64 {
    e.payload.get("at").and_then(Value::as_i64).unwrap_or((e.hlc >> 16).max(0))
}

/// The card's events in replay order, with undone reviews removed.
fn ordered(events: Vec<CardEvent>) -> Vec<CardEvent> {
    let undone: HashSet<String> = events
        .iter()
        .filter(|e| e.ty == "review.undo")
        .filter_map(|e| e.payload.get("review_id").and_then(Value::as_str).map(String::from))
        .collect();
    let mut live: Vec<CardEvent> = events.into_iter().filter(|e| e.ty != "review.undo" && !undone.contains(&e.id)).collect();
    live.sort_by(|a, b| (at_of(a), a.hlc, &a.site, &a.id).cmp(&(at_of(b), b.hlc, &b.site, &b.id)));
    live
}

fn snapshot(p: &Value) -> CardState {
    let f = |k: &str| p.get(k).and_then(Value::as_f64);
    let i = |k: &str| p.get(k).and_then(Value::as_i64).unwrap_or(0);
    CardState {
        queue: Queue::parse(p.get("queue").and_then(Value::as_str).unwrap_or("new")),
        due: i("due"),
        stability: f("stability"),
        difficulty: f("difficulty"),
        step: i("step"),
        reps: i("reps"),
        lapses: i("lapses"),
        last_review: p.get("last_review").and_then(Value::as_i64),
    }
}

/// The state a card reaches by replaying `events` (SPEC §11.6): snapshots reset it, undone reviews are skipped.
pub fn fold(card_id: &str, events: Vec<CardEvent>, policy: &dyn sched::Policy, cfg: &sched::DeckCfg) -> CardState {
    let mut state = CardState::new();
    for e in ordered(events) {
        match e.ty.as_str() {
            "review.state" => state = snapshot(&e.payload),
            "review" => {
                let Some(rating) = e.payload.get("rating").and_then(Value::as_i64).and_then(Rating::from_i64) else { continue };
                let tz = e.payload.get("tz").and_then(Value::as_i64).unwrap_or(0);
                let clock = Clock { tz_offset_min: tz, day_starts_hour: cfg.day_starts_hour };
                state = policy.answer(card_id, &state, rating, at_of(&e), cfg, &clock);
            }
            _ => {}
        }
    }
    state
}

/// Recompute one card's cache from its events and write it if it differs.
pub fn replay_card(store: &mut Store, card_id: &str) -> Result<()> {
    let Some(card) = card_row(store, card_id)? else { return Ok(()) };
    let cfg = sched::load_config(store.read_conn(), card.config_id.as_deref())?;
    let state = fold(card_id, store.card_events(card_id)?, sched::policy_for(&card.deck_kind), &cfg);
    let new = cache_of(&state);
    if new != card.cache {
        store.set_card_cache(card_id, &new)?;
    }
    Ok(())
}

/// What an answer did, for the review screen.
#[derive(Debug, Clone)]
pub struct Answered {
    pub review_id: String,
    pub state: CardState,
}

/// Record an answer (SPEC §11.6) and update the card's cache. `now` is unix ms, `tz_offset_min` the local offset.
pub fn answer(
    store: &mut Store,
    card_id: &str,
    rating: Rating,
    duration_ms: i64,
    example: Option<(&str, i64)>,
    now: i64,
    tz_offset_min: i64,
) -> Result<Answered> {
    let card = card_row(store, card_id)?.ok_or_else(|| Error::Invalid(format!("no card {card_id}")))?;
    let policy = sched::policy_for(&card.deck_kind);
    let mut payload = json!({
        "card_id": card_id,
        "rating": rating as i64,
        "duration_ms": duration_ms,
        "at": now,
        "tz": tz_offset_min,
        "policy": policy.name(),
        "before": {"queue": card.cache.queue, "s": card.cache.stability, "d": card.cache.difficulty, "due": card.cache.due},
    });
    if let Some((part, start)) = example {
        payload["example"] = json!({"part_id": part, "start": start});
    }
    let review_id = store.emit_event_id("review", card.language.as_deref(), Some(("card", card_id)), &payload)?;
    replay_card(store, card_id)?;
    let state = card_row(store, card_id)?.map(|c| state_of(&c.cache)).unwrap_or_default();
    Ok(Answered { review_id, state })
}

/// Undo the card's most recent answer by appending a `review.undo` event. Returns false if there is none.
pub fn undo(store: &mut Store, card_id: &str) -> Result<bool> {
    let Some(card) = card_row(store, card_id)? else { return Ok(false) };
    let reviews: Vec<CardEvent> = ordered(store.card_events(card_id)?).into_iter().filter(|e| e.ty == "review").collect();
    let Some(last) = reviews.last() else { return Ok(false) };
    store.emit_event_id("review.undo", card.language.as_deref(), Some(("card", card_id)), &json!({"review_id": last.id}))?;
    replay_card(store, card_id)?;
    Ok(true)
}

/// The state cached on the card (what the UI shows).
pub fn card_state(store: &Store, card_id: &str) -> Result<Option<CardState>> {
    Ok(card_row(store, card_id)?.map(|c| state_of(&c.cache)))
}
