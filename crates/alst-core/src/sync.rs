//! Store API for sync (SPEC §9.2): the vector clock, ordered change and event streams, and `apply`,
//! which merges a remote batch into `_changes` and rebuilds the touched rows from it.

use crate::store::Store;
use crate::{Error, Result, hlc};
use rusqlite::{Connection, Transaction, params, types::Value};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Highest HLC known per site: "I have everything from `site` up to `hlc`".
pub type Vector = BTreeMap<String, i64>;

/// One column clock from `_changes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub tbl: String,
    pub row: String,
    pub col: String,
    pub value: Option<String>,
    pub hlc: i64,
    pub site: String,
}

/// One row of `events`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventEntry {
    pub id: String,
    pub hlc: i64,
    pub site: String,
    pub ty: String,
    pub language: Option<String>,
    pub subject_type: Option<String>,
    pub subject_id: Option<String>,
    pub payload: String,
}

/// A slice of the change and event streams, ordered by `(hlc, site)`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Batch {
    pub changes: Vec<Change>,
    pub events: Vec<EventEntry>,
}

impl Batch {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty() && self.events.is_empty()
    }

    /// Entries per site: the highest `hlc` in the batch.
    fn per_site_max(&self) -> Vector {
        let mut v = Vector::new();
        let items = self.changes.iter().map(|c| (&c.site, c.hlc)).chain(self.events.iter().map(|e| (&e.site, e.hlc)));
        for (site, h) in items {
            let slot = v.entry(site.clone()).or_insert(0);
            *slot = (*slot).max(h);
        }
        v
    }
}

/// Where a sender is in streaming `changes + events` to one peer. Starts at the peer's vector.
#[derive(Debug, Clone)]
pub struct Cursor {
    vector: Vector,
    floor: i64,
}

impl Cursor {
    pub fn new(peer_vector: Vector) -> Self {
        Cursor { vector: peer_vector, floor: 0 }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TableInfo {
    cols: BTreeSet<String>,
    /// NOT NULL columns without a default (other than `id`): a row can't be inserted without them.
    required: Vec<String>,
}

pub(crate) type Tables = HashMap<String, TableInfo>;

/// Synced tables: not `local_*`, `_changes`, `events`; they carry `created_at` and `deleted`.
pub(crate) fn discover(conn: &Connection) -> Result<Tables> {
    let names: Vec<String> = {
        let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")?;
        stmt.query_map([], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?
    };
    let mut out = Tables::new();
    for name in names {
        if name.starts_with("local_") || name.starts_with('_') || name == "events" {
            continue;
        }
        let mut cols = BTreeSet::new();
        let mut required = Vec::new();
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({name})"))?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, i64>(3)?, r.get::<_, Option<String>>(4)?)))?;
        for row in rows {
            let (col, notnull, default) = row?;
            if notnull != 0 && default.is_none() && col != "id" {
                required.push(col.clone());
            }
            cols.insert(col);
        }
        if cols.contains("id") && cols.contains("deleted") && cols.contains("created_at") {
            out.insert(name, TableInfo { cols, required });
        }
    }
    Ok(out)
}

fn change_row(r: &rusqlite::Row) -> rusqlite::Result<Change> {
    Ok(Change { tbl: r.get(0)?, row: r.get(1)?, col: r.get(2)?, value: r.get(3)?, hlc: r.get(4)?, site: r.get(5)? })
}

fn event_row(r: &rusqlite::Row) -> rusqlite::Result<EventEntry> {
    Ok(EventEntry {
        id: r.get(0)?,
        hlc: r.get(1)?,
        site: r.get(2)?,
        ty: r.get(3)?,
        language: r.get(4)?,
        subject_type: r.get(5)?,
        subject_id: r.get(6)?,
        payload: r.get(7)?,
    })
}

fn vector_json(v: &Vector) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "{}".into())
}

/// Bump the vector entry for `site` (never lowers it).
pub(crate) fn bump_clock(conn: &Connection, site: &str, stamp: u64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO local_sync_clock (site_id, hlc) VALUES (?1, ?2)
         ON CONFLICT(site_id) DO UPDATE SET hlc = excluded.hlc WHERE excluded.hlc > local_sync_clock.hlc",
        params![site, stamp as i64],
    )?;
    Ok(())
}

impl Store {
    /// `local_sync_clock` as a map site → hlc.
    pub fn vector(&self) -> Result<Vector> {
        let mut stmt = self.conn.prepare("SELECT site_id, hlc FROM local_sync_clock")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// This device's clock for the session hello: the later of its HLC and its wall clock.
    pub fn hlc_now(&self) -> i64 {
        self.clock.last().max(hlc::pack(hlc::wall_ms(), 0)) as i64
    }

    /// `_changes` entries newer than `vector`, ordered by `(hlc, site, tbl, row, col)`. A batch never splits
    /// entries that share one `(site, hlc)`; it may exceed `max_rows` to finish that group.
    pub fn changes_since(&self, vector: &Vector, max_rows: usize) -> Result<Vec<Change>> {
        self.changes_page(vector, 0, max_rows)
    }

    /// The same rule for `events`.
    pub fn events_since(&self, vector: &Vector, max_rows: usize) -> Result<Vec<EventEntry>> {
        self.events_page(vector, 0, max_rows)
    }

    fn changes_page(&self, vector: &Vector, floor: i64, max_rows: usize) -> Result<Vec<Change>> {
        let vj = vector_json(vector);
        let mut stmt = self.conn.prepare_cached(
            "SELECT tbl, row_id, col, value, hlc, site_id FROM _changes
             WHERE hlc >= ?2 AND hlc > COALESCE((SELECT value FROM json_each(?1) WHERE key = _changes.site_id), 0)
             ORDER BY hlc, site_id, tbl, row_id, col LIMIT ?3",
        )?;
        let mut out: Vec<Change> =
            stmt.query_map(params![vj, floor, max_rows as i64], change_row)?.collect::<std::result::Result<_, _>>()?;
        if out.len() >= max_rows
            && let Some(last) = out.last().cloned()
        {
            let mut rest = self.conn.prepare_cached(
                "SELECT tbl, row_id, col, value, hlc, site_id FROM _changes
                 WHERE hlc = ?1 AND site_id = ?2 AND (tbl, row_id, col) > (?3, ?4, ?5) ORDER BY tbl, row_id, col",
            )?;
            let more = rest.query_map(params![last.hlc, last.site, last.tbl, last.row, last.col], change_row)?;
            for c in more {
                out.push(c?);
            }
        }
        Ok(out)
    }

    fn events_page(&self, vector: &Vector, floor: i64, max_rows: usize) -> Result<Vec<EventEntry>> {
        let vj = vector_json(vector);
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, hlc, site_id, type, language, subject_type, subject_id, payload FROM events
             WHERE hlc >= ?2 AND hlc > COALESCE((SELECT value FROM json_each(?1) WHERE key = events.site_id), 0)
             ORDER BY hlc, site_id, id LIMIT ?3",
        )?;
        let mut out: Vec<EventEntry> =
            stmt.query_map(params![vj, floor, max_rows as i64], event_row)?.collect::<std::result::Result<_, _>>()?;
        if out.len() >= max_rows
            && let Some(last) = out.last().cloned()
        {
            let mut rest = self.conn.prepare_cached(
                "SELECT id, hlc, site_id, type, language, subject_type, subject_id, payload FROM events
                 WHERE hlc = ?1 AND site_id = ?2 AND id > ?3 ORDER BY id",
            )?;
            let more = rest.query_map(params![last.hlc, last.site, last.id], event_row)?;
            for e in more {
                out.push(e?);
            }
        }
        Ok(out)
    }

    /// The next batch of changes and events for a peer, about `max_rows` of each, and advance `cursor` past it.
    /// Changes and events travel together and the cutoff is the same for both, so a peer that applies a batch
    /// has everything at or below the cutoff from both streams (the vector can't skip an unsent event).
    /// Returns an empty batch when the peer is up to date.
    pub fn next_batch(&self, cursor: &mut Cursor, max_rows: usize) -> Result<Batch> {
        let mut changes = self.changes_page(&cursor.vector, cursor.floor, max_rows)?;
        let mut events = self.events_page(&cursor.vector, cursor.floor, max_rows)?;
        // Either stream may be cut short of the rest; the batch ends at the earlier of the two cutoffs.
        let key = |h: i64, s: &str| (h, s.to_string());
        let mut cutoff: Option<(i64, String)> = None;
        if changes.len() >= max_rows && let Some(c) = changes.last() {
            cutoff = Some(key(c.hlc, &c.site));
        }
        if events.len() >= max_rows && let Some(e) = events.last() {
            let k = key(e.hlc, &e.site);
            cutoff = Some(cutoff.map_or(k.clone(), |c| c.min(k)));
        }
        if let Some(cut) = &cutoff {
            changes.retain(|c| key(c.hlc, &c.site) <= *cut);
            events.retain(|e| key(e.hlc, &e.site) <= *cut);
        }
        let batch = Batch { changes, events };
        for (site, h) in batch.per_site_max() {
            let slot = cursor.vector.entry(site).or_insert(0);
            *slot = (*slot).max(h);
        }
        if let Some((h, _)) = cutoff.or_else(|| batch.per_site_max().values().max().map(|h| (*h, String::new()))) {
            cursor.floor = cursor.floor.max(h);
        }
        Ok(batch)
    }

    /// Merge a remote batch in one transaction: upsert `_changes` where `(hlc, site)` wins, insert new events,
    /// advance the vector, then rebuild every touched row from `_changes` (SPEC §9.2). Returns the touched tables.
    pub fn apply(&mut self, batch: &Batch) -> Result<BTreeSet<String>> {
        for c in &batch.changes {
            let info = self.tables.get(&c.tbl).ok_or_else(|| Error::Invalid(format!("sync: unknown table {}", c.tbl)))?;
            if !info.cols.contains(&c.col) {
                return Err(Error::Invalid(format!("sync: unknown column {}.{}", c.tbl, c.col)));
            }
        }
        // Rows may reference rows that arrive in a later batch (a link whose term was last edited after it),
        // so references are not enforced while applying; they converge by the end of a session.
        self.conn.pragma_update(None, "foreign_keys", "OFF")?;
        let result = self.apply_inner(batch);
        self.conn.pragma_update(None, "foreign_keys", "ON")?;
        let (touched, newest) = result?;
        if newest > 0 {
            self.clock.observe(newest as u64);
        }
        Ok(touched)
    }

    fn apply_inner(&mut self, batch: &Batch) -> Result<(BTreeSet<String>, i64)> {
        let tx = self.conn.transaction()?;
        let mut touched = BTreeSet::new();
        let mut rows: BTreeSet<(String, String)> = BTreeSet::new();
        {
            let mut up = tx.prepare_cached(
                "INSERT INTO _changes (tbl, row_id, col, value, hlc, site_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(tbl, row_id, col) DO UPDATE SET value = excluded.value, hlc = excluded.hlc, site_id = excluded.site_id
                 WHERE (excluded.hlc, excluded.site_id) > (_changes.hlc, _changes.site_id)",
            )?;
            for c in &batch.changes {
                up.execute(params![c.tbl, c.row, c.col, c.value, c.hlc, c.site])?;
                rows.insert((c.tbl.clone(), c.row.clone()));
            }
        }
        {
            let mut ins = tx.prepare_cached(
                "INSERT OR IGNORE INTO events (id, hlc, site_id, type, language, subject_type, subject_id, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for e in &batch.events {
                let n = ins.execute(params![e.id, e.hlc, e.site, e.ty, e.language, e.subject_type, e.subject_id, e.payload])?;
                if n > 0 {
                    touched.insert("events".to_string());
                }
            }
        }
        let mut newest = 0;
        for (site, h) in batch.per_site_max() {
            bump_clock(&tx, &site, h as u64)?;
            newest = newest.max(h);
        }
        for (tbl, row) in &rows {
            if materialize(&tx, &self.tables, tbl, row)? {
                tx.execute("DELETE FROM local_sync_pending WHERE tbl = ?1 AND row_id = ?2", params![tbl, row])?;
            } else {
                tx.execute("INSERT OR IGNORE INTO local_sync_pending (tbl, row_id) VALUES (?1, ?2)", params![tbl, row])?;
            }
            touched.insert(tbl.clone());
        }
        // Rows that were waiting for a column may be complete now.
        let pending: Vec<(String, String)> = {
            let mut stmt = tx.prepare("SELECT tbl, row_id FROM local_sync_pending")?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<_, _>>()?
        };
        for (tbl, row) in pending {
            if !rows.contains(&(tbl.clone(), row.clone())) && materialize(&tx, &self.tables, &tbl, &row)? {
                tx.execute("DELETE FROM local_sync_pending WHERE tbl = ?1 AND row_id = ?2", params![tbl, row])?;
                touched.insert(tbl);
            }
        }
        tx.commit()?;
        Ok((touched, newest))
    }

    /// Rows received but not yet complete (testing and diagnostics).
    pub fn pending_count(&self) -> Result<i64> {
        Ok(self.conn.query_row("SELECT count(*) FROM local_sync_pending", [], |r| r.get(0))?)
    }
}

/// Rebuild one row from its `_changes` entries. Returns false when the row doesn't exist and lacks a
/// required column, so it stays pending.
fn materialize(tx: &Transaction, tables: &Tables, tbl: &str, row: &str) -> Result<bool> {
    let info = tables.get(tbl).ok_or_else(|| Error::Invalid(format!("sync: unknown table {tbl}")))?;
    let entries: Vec<(String, Value)> = {
        let mut stmt = tx.prepare_cached("SELECT col, value FROM _changes WHERE tbl = ?1 AND row_id = ?2 ORDER BY col")?;
        stmt.query_map(params![tbl, row], |r| {
            let v: Option<String> = r.get(1)?;
            Ok((r.get::<_, String>(0)?, v.map_or(Value::Null, Value::Text)))
        })?
        .collect::<std::result::Result<_, _>>()?
    };
    let exists: bool = tx.query_row(&format!("SELECT count(*) FROM {tbl} WHERE id = ?1"), [row], |r| Ok(r.get::<_, i64>(0)? > 0))?;
    if exists {
        if entries.is_empty() {
            return Ok(true);
        }
        let sets: Vec<String> = entries.iter().enumerate().map(|(i, (c, _))| format!("{c} = ?{}", i + 2)).collect();
        let mut args: Vec<Value> = vec![row.to_string().into()];
        args.extend(entries.into_iter().map(|(_, v)| v));
        tx.prepare_cached(&format!("UPDATE {tbl} SET {} WHERE id = ?1", sets.join(", ")))?
            .execute(rusqlite::params_from_iter(args))?;
        return Ok(true);
    }
    if !info.required.iter().all(|c| entries.iter().any(|(n, _)| n == c)) {
        return Ok(false);
    }
    let names: Vec<&str> = entries.iter().map(|(c, _)| c.as_str()).collect();
    let marks: Vec<String> = (2..entries.len() + 2).map(|i| format!("?{i}")).collect();
    let mut args: Vec<Value> = vec![row.to_string().into()];
    args.extend(entries.iter().map(|(_, v)| v.clone()));
    tx.prepare_cached(&format!("INSERT INTO {tbl} (id, {}) VALUES (?1, {})", names.join(", "), marks.join(", ")))?
        .execute(rusqlite::params_from_iter(args))?;
    Ok(true)
}

/// Peers' physical clocks vs ours, for the session's clock-skew check.
pub fn skew_ms(remote_hlc: i64) -> i64 {
    hlc::physical_ms(remote_hlc.max(0) as u64) as i64 - hlc::wall_ms() as i64
}
