//! Store functions for the SRS tables (SPEC §11.2, §11.6). Scheduling math lives in `alst-srs`; this
//! module only writes rows (with change capture) and the card cache, and runs the replay hook after `apply`.

use crate::store::{Store, derive_id};
use crate::{Error, Result, hlc};
use rusqlite::{params, types::Value};

/// Synced SRS tables that `srs_put` and `srs_delete` accept.
const TABLES: [&str; 6] = ["deck_configs", "decks", "note_types", "notes", "cards", "note_media"];

/// One row for `srs_put_many`: table, id, columns.
pub type SrsRow<'a> = (&'a str, String, Vec<(&'a str, Value)>);

/// Called after `apply` with the ids of cards whose review events or rows changed.
pub type ReplayHook = Box<dyn FnMut(&mut Store, &[String]) -> Result<()> + Send>;

/// The cached scheduling columns of a card (written by replay, never captured in `_changes`).
#[derive(Debug, Clone, PartialEq)]
pub struct CardCache {
    pub queue: String,
    pub due: i64,
    pub stability: Option<f64>,
    pub difficulty: Option<f64>,
    pub step: i64,
    pub reps: i64,
    pub lapses: i64,
    pub last_review: Option<i64>,
}

/// One SRS event of a card, in storage order.
#[derive(Debug, Clone)]
pub struct CardEvent {
    pub id: String,
    pub hlc: i64,
    pub site: String,
    pub ty: String,
    pub payload: serde_json::Value,
}

/// An event to insert with a caller-chosen id (imports), so importing twice adds nothing.
#[derive(Debug, Clone)]
pub struct NewEvent {
    pub id: String,
    pub ty: String,
    pub language: Option<String>,
    pub subject_type: String,
    pub subject_id: String,
    pub payload: serde_json::Value,
}

/// Id of a concept card (SPEC §11.2).
pub fn concept_card_id(concept_id: &str, direction: &str) -> String {
    derive_id("cards", &[concept_id, direction])
}

/// Id of a classic card.
pub fn classic_card_id(note_id: &str, ord: i64) -> String {
    derive_id("cards", &[note_id, &ord.to_string()])
}

pub fn concept_id(language: &str, kind: &str, key: &str) -> String {
    derive_id("concepts", &[language, kind, key])
}

fn check(tbl: &str) -> Result<()> {
    if TABLES.contains(&tbl) { Ok(()) } else { Err(Error::Invalid(format!("not an SRS table: {tbl}"))) }
}

impl Store {
    /// Install the hook that replays card history after `apply` (set by `alst_srs::attach`).
    pub fn set_replay_hook(&mut self, hook: ReplayHook) {
        self.replay_hook = Some(hook);
    }

    /// Insert or update one row of an SRS table, recording each column in `_changes`. Creating a row whose
    /// id is a tombstone revives it. `created_at` and `deleted` are managed here.
    pub fn srs_put(&mut self, tbl: &str, id: &str, cols: &[(&str, Value)]) -> Result<()> {
        check(tbl)?;
        let stamp = self.clock.tick(hlc::wall_ms());
        let site = self.site_id().to_string();
        let tx = self.conn.transaction()?;
        let exists: bool = tx.query_row(&format!("SELECT count(*) FROM {tbl} WHERE id = ?1"), [id], |r| Ok(r.get::<_, i64>(0)? > 0))?;
        let mut all: Vec<(&str, Value)> = Vec::with_capacity(cols.len() + 2);
        if !exists {
            all.push(("created_at", (hlc::wall_ms() as i64).into()));
        }
        all.push(("deleted", 0.into()));
        all.extend(cols.iter().cloned());
        if exists {
            crate::store::update_cols(&tx, tbl, id, &all, stamp, &site)?;
        } else {
            crate::store::insert_row(&tx, tbl, id, &all, stamp, &site)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Several `srs_put` calls in one transaction (imports).
    pub fn srs_put_many(&mut self, rows: &[SrsRow<'_>]) -> Result<()> {
        for (tbl, _, _) in rows {
            check(tbl)?;
        }
        let stamp = self.clock.tick(hlc::wall_ms());
        let site = self.site_id().to_string();
        let tx = self.conn.transaction()?;
        for (tbl, id, cols) in rows {
            let exists: bool = tx.query_row(&format!("SELECT count(*) FROM {tbl} WHERE id = ?1"), [id], |r| Ok(r.get::<_, i64>(0)? > 0))?;
            let mut all: Vec<(&str, Value)> = Vec::with_capacity(cols.len() + 2);
            if !exists {
                all.push(("created_at", (hlc::wall_ms() as i64).into()));
            }
            all.push(("deleted", 0.into()));
            all.extend(cols.iter().cloned());
            if exists {
                crate::store::update_cols(&tx, tbl, id, &all, stamp, &site)?;
            } else {
                crate::store::insert_row(&tx, tbl, id, &all, stamp, &site)?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Find or create the concept for a key (SPEC §11.2; id derived from language, kind, key).
    pub fn ensure_concept(&mut self, language: &str, kind: &str, key: &str) -> Result<String> {
        let id = concept_id(language, kind, key);
        let stamp = self.clock.tick(hlc::wall_ms());
        let site = self.site_id().to_string();
        let tx = self.conn.transaction()?;
        let state: Option<i64> = tx.query_row("SELECT deleted FROM concepts WHERE id = ?1", [&id], |r| r.get(0)).ok();
        match state {
            Some(0) => {}
            Some(_) => crate::store::update_cols(&tx, "concepts", &id, &[("deleted", 0.into())], stamp, &site)?,
            None => crate::store::insert_row(
                &tx,
                "concepts",
                &id,
                &[
                    ("created_at", (hlc::wall_ms() as i64).into()),
                    ("deleted", 0.into()),
                    ("language", language.to_string().into()),
                    ("kind", kind.to_string().into()),
                    ("key", key.to_string().into()),
                    ("meta", "{}".to_string().into()),
                ],
                stamp,
                &site,
            )?,
        }
        tx.commit()?;
        Ok(id)
    }

    /// Point a term at its concept (`terms.concept_id`).
    pub fn set_term_concept(&mut self, term_id: &str, concept_id: &str) -> Result<()> {
        let stamp = self.clock.tick(hlc::wall_ms());
        let site = self.site_id().to_string();
        let tx = self.conn.transaction()?;
        crate::store::update_cols(&tx, "terms", term_id, &[("concept_id", concept_id.to_string().into())], stamp, &site)?;
        tx.commit()?;
        Ok(())
    }

    /// Edit some columns of an existing SRS row.
    pub fn srs_update(&mut self, tbl: &str, id: &str, cols: &[(&str, Value)]) -> Result<()> {
        check(tbl)?;
        let stamp = self.clock.tick(hlc::wall_ms());
        let site = self.site_id().to_string();
        let tx = self.conn.transaction()?;
        crate::store::update_cols(&tx, tbl, id, cols, stamp, &site)?;
        tx.commit()?;
        Ok(())
    }

    /// Tombstone an SRS row.
    pub fn srs_delete(&mut self, tbl: &str, id: &str) -> Result<()> {
        self.srs_update(tbl, id, &[("deleted", 1.into())])
    }

    /// Write a card's cached scheduling columns. Not change-captured: every device derives the same values by
    /// replaying the card's events (SPEC §11.6). Returns false if the card does not exist.
    pub fn set_card_cache(&mut self, id: &str, c: &CardCache) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE cards SET queue = ?2, due = ?3, stability = ?4, difficulty = ?5, step = ?6, reps = ?7, lapses = ?8,
                              last_review = ?9 WHERE id = ?1",
            params![id, c.queue, c.due, c.stability, c.difficulty, c.step, c.reps, c.lapses, c.last_review],
        )?;
        Ok(n > 0)
    }

    /// Append an event and return its id (`emit_event` does not).
    pub fn emit_event_id(
        &mut self,
        ty: &str,
        language: Option<&str>,
        subject: Option<(&str, &str)>,
        payload: &serde_json::Value,
    ) -> Result<String> {
        let stamp = self.clock.tick(hlc::wall_ms());
        let id = crate::store::new_id();
        crate::sync::bump_clock(&self.conn, self.site_id(), stamp)?;
        self.conn.execute(
            "INSERT INTO events (id, hlc, site_id, type, language, subject_type, subject_id, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, stamp as i64, self.site_id(), ty, language, subject.map(|s| s.0), subject.map(|s| s.1), payload.to_string()],
        )?;
        Ok(id)
    }

    /// Insert events in one transaction, skipping ids that exist. Returns how many were new. Each gets its own
    /// HLC so they sync like any event; replay orders them by the payload's `at`, not by the HLC.
    pub fn emit_events_batch(&mut self, items: &[NewEvent]) -> Result<usize> {
        let site = self.site_id().to_string();
        let tx = self.conn.transaction()?;
        let mut n = 0;
        let mut last = 0u64;
        for e in items {
            let stamp = self.clock.tick(hlc::wall_ms());
            last = stamp;
            let k = tx.execute(
                "INSERT OR IGNORE INTO events (id, hlc, site_id, type, language, subject_type, subject_id, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![e.id, stamp as i64, site, e.ty, e.language, e.subject_type, e.subject_id, e.payload.to_string()],
            )?;
            n += k;
        }
        if last > 0 {
            crate::sync::bump_clock(&tx, &site, last)?;
        }
        tx.commit()?;
        Ok(n)
    }

    /// Review, undo and state events of one card (unordered; the replay sorts them).
    pub fn card_events(&self, card_id: &str) -> Result<Vec<CardEvent>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, hlc, site_id, type, payload FROM events
             WHERE subject_type = 'card' AND subject_id = ?1 AND type IN ('review', 'review.undo', 'review.state')",
        )?;
        let rows = stmt.query_map([card_id], |r| {
            let payload: String = r.get(4)?;
            Ok(CardEvent {
                id: r.get(0)?,
                hlc: r.get(1)?,
                site: r.get(2)?,
                ty: r.get(3)?,
                payload: serde_json::from_str(&payload).unwrap_or(serde_json::Value::Null),
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Read access for queries on any table (SRS screens, browse, stats). Writes must go through the store.
    pub fn read_conn(&self) -> &rusqlite::Connection {
        &self.conn
    }

    /// Run the replay hook for these cards (no-op when none is installed).
    pub fn replay_cards(&mut self, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        if let Some(mut hook) = self.replay_hook.take() {
            let r = hook(self, ids);
            self.replay_hook = Some(hook);
            r
        } else {
            Ok(())
        }
    }
}
