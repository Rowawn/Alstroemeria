//! The write layer for synced tables (SPEC §4.1, §4.4). Every write records
//! per-column clocks in `_changes`; nothing else may write synced tables.

use crate::{Error, Result, hlc};
use rusqlite::{Connection, Transaction, params, types::Value};
use serde::Serialize;
use std::path::Path;

pub struct Store {
    conn: Connection,
    clock: hlc::Hlc,
    site_id: String,
}

#[derive(Debug, Clone)]
pub struct NewPart {
    pub title: Option<String>,
    pub text: String,
    pub word_count: i64,
}

#[derive(Debug, Clone)]
pub struct NewWork {
    pub language: String,
    pub title: String,
    pub kind: String,
    pub author: Option<String>,
    pub source_url: Option<String>,
    pub parts: Vec<NewPart>,
}

impl NewWork {
    /// A single-part plain-text work.
    pub fn text(language: &str, title: &str, text: &str) -> Self {
        NewWork {
            language: language.into(),
            title: title.into(),
            kind: "text".into(),
            author: None,
            source_url: None,
            parts: vec![NewPart { title: None, text: text.into(), word_count: text.split_whitespace().count() as i64 }],
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Work {
    pub id: String,
    pub language: String,
    pub title: String,
    pub kind: String,
    pub author: Option<String>,
    pub source_url: Option<String>,
    pub created_at: i64,
    pub part_count: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    pub id: String,
    pub work_id: String,
    pub idx: i64,
    pub title: Option<String>,
    pub text: String,
    pub word_count: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartInfo {
    pub id: String,
    pub idx: i64,
    pub title: Option<String>,
    pub word_count: i64,
}

/// Valid term statuses (SPEC §4.2).
pub const STATUSES: [i64; 8] = [0, 1, 2, 3, 4, 5, 98, 99];

#[derive(Debug, Clone, Serialize)]
pub struct TermRef {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Term {
    pub id: String,
    pub language: String,
    pub text: String,
    pub text_lc: String,
    pub lemma: Option<String>,
    pub status: i64,
    pub translation: Option<String>,
    pub reading: Option<String>,
    pub notes: Option<String>,
    pub word_count: i64,
    pub parents: Vec<TermRef>,
}

/// Fields to change on a term; `None` leaves the column alone.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct TermPatch {
    pub status: Option<i64>,
    pub translation: Option<String>,
    pub reading: Option<String>,
    pub notes: Option<String>,
    pub lemma: Option<String>,
}

/// Minimal term info the reader needs to color tokens.
#[derive(Debug, Clone)]
pub struct TermStateRow {
    pub id: String,
    pub text_lc: String,
    pub status: i64,
    pub word_count: i64,
}

pub fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_conn(crate::db::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::from_conn(crate::db::open_in_memory()?)
    }

    fn from_conn(conn: Connection) -> Result<Self> {
        let site_id = match conn
            .query_row("SELECT value FROM local_settings WHERE key = 'site_id'", [], |r| {
                r.get::<_, String>(0)
            })
            .ok()
        {
            Some(s) => s,
            None => {
                let s = uuid::Uuid::new_v4().simple().to_string();
                conn.execute("INSERT INTO local_settings(key, value) VALUES ('site_id', ?1)", [&s])?;
                s
            }
        };
        let last: i64 = conn.query_row("SELECT COALESCE(MAX(hlc), 0) FROM _changes", [], |r| r.get(0))?;
        Ok(Self { conn, clock: hlc::Hlc::new(last as u64), site_id })
    }

    pub fn site_id(&self) -> &str {
        &self.site_id
    }

    /// Insert a work and its parts, recording every column in `_changes`.
    pub fn create_work(&mut self, new: NewWork) -> Result<Work> {
        if new.title.trim().is_empty() {
            return Err(Error::Invalid("title is required".into()));
        }
        if new.parts.is_empty() {
            return Err(Error::Invalid("nothing to import: no text".into()));
        }
        let now = hlc::wall_ms();
        let stamp = self.clock.tick(now);
        let work_id = new_id();
        let created = now as i64;
        let site = self.site_id.clone();
        let tx = self.conn.transaction()?;

        let mut work_cols: Vec<(&str, Value)> = vec![
            ("created_at", created.into()),
            ("deleted", 0.into()),
            ("language", new.language.clone().into()),
            ("title", new.title.clone().into()),
            ("kind", new.kind.clone().into()),
            ("tags", "[]".to_string().into()),
            ("archived", 0.into()),
            ("settings", "{}".to_string().into()),
        ];
        if let Some(a) = &new.author {
            work_cols.push(("author", a.clone().into()));
        }
        if let Some(u) = &new.source_url {
            work_cols.push(("source_url", u.clone().into()));
        }
        insert_row(&tx, "works", &work_id, &work_cols, stamp, &site)?;

        for (idx, part) in new.parts.iter().enumerate() {
            let mut cols: Vec<(&str, Value)> = vec![
                ("created_at", created.into()),
                ("deleted", 0.into()),
                ("work_id", work_id.clone().into()),
                ("idx", (idx as i64).into()),
                ("text", part.text.clone().into()),
                ("word_count", part.word_count.into()),
            ];
            if let Some(t) = &part.title {
                cols.push(("title", t.clone().into()));
            }
            insert_row(&tx, "parts", &new_id(), &cols, stamp, &site)?;
        }
        tx.commit()?;

        Ok(Work {
            id: work_id,
            language: new.language,
            title: new.title,
            kind: new.kind,
            author: new.author,
            source_url: new.source_url,
            created_at: created,
            part_count: new.parts.len() as i64,
        })
    }

    /// Update one column of a work; the change is recorded with a fresh clock.
    pub fn update_work(&mut self, id: &str, col: WorkCol, value: Value) -> Result<()> {
        let stamp = self.clock.tick(hlc::wall_ms());
        let site = self.site_id.clone();
        let tx = self.conn.transaction()?;
        let n = tx.execute(&format!("UPDATE works SET {} = ?1 WHERE id = ?2", col.name()), params![value, id])?;
        if n == 0 {
            return Err(Error::Invalid(format!("no work {id}")));
        }
        record_change(&tx, "works", id, col.name(), &value, stamp, &site)?;
        tx.commit()?;
        Ok(())
    }

    pub fn list_works(&self) -> Result<Vec<Work>> {
        let mut stmt = self.conn.prepare(
            "SELECT w.id, w.language, w.title, w.kind, w.author, w.source_url, w.created_at,
                    (SELECT count(*) FROM parts p WHERE p.work_id = w.id AND p.deleted = 0)
             FROM works w WHERE w.deleted = 0 AND w.archived = 0 ORDER BY w.created_at DESC, w.id DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Work {
                id: r.get(0)?,
                language: r.get(1)?,
                title: r.get(2)?,
                kind: r.get(3)?,
                author: r.get(4)?,
                source_url: r.get(5)?,
                created_at: r.get(6)?,
                part_count: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn get_work(&self, id: &str) -> Result<Work> {
        self.list_works()?
            .into_iter()
            .find(|w| w.id == id)
            .ok_or_else(|| Error::Invalid(format!("no work {id}")))
    }

    pub fn list_parts(&self, work_id: &str) -> Result<Vec<PartInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, idx, title, word_count FROM parts WHERE work_id = ?1 AND deleted = 0 ORDER BY idx",
        )?;
        let rows = stmt.query_map([work_id], |r| {
            Ok(PartInfo { id: r.get(0)?, idx: r.get(1)?, title: r.get(2)?, word_count: r.get(3)? })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn get_part(&self, id: &str) -> Result<Part> {
        self.conn
            .query_row(
                "SELECT id, work_id, idx, title, text, word_count FROM parts WHERE id = ?1 AND deleted = 0",
                [id],
                |r| {
                    Ok(Part {
                        id: r.get(0)?,
                        work_id: r.get(1)?,
                        idx: r.get(2)?,
                        title: r.get(3)?,
                        text: r.get(4)?,
                        word_count: r.get(5)?,
                    })
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Error::Invalid(format!("no part {id}")),
                e => e.into(),
            })
    }

    /// Texts of all parts of works in `language`, for example-sentence search.
    pub fn part_texts(&self, language: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.text FROM parts p JOIN works w ON w.id = p.work_id
             WHERE w.language = ?1 AND w.deleted = 0 AND p.deleted = 0 ORDER BY w.created_at, p.idx",
        )?;
        let rows = stmt.query_map([language], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    // ---- terms ---------------------------------------------------------------------------------

    /// Create the term if `(language, text_lc)` has no live row, otherwise change the patched columns
    /// that differ. A status change emits a `term.status` event in the same transaction.
    pub fn save_term(&mut self, language: &str, text: &str, text_lc: &str, word_count: i64, patch: &TermPatch) -> Result<Term> {
        validate_patch(patch)?;
        let stamp = self.clock.tick(hlc::wall_ms());
        let site = self.site_id.clone();
        let tx = self.conn.transaction()?;
        let id = upsert_term(&tx, &site, stamp, language, text, text_lc, word_count, patch, false)?;
        tx.commit()?;
        self.term_by_id(&id)
    }

    /// Create any missing terms with `status` (existing terms are left alone). Returns how many were created.
    pub fn create_terms(&mut self, language: &str, items: &[(String, String, i64)], status: i64) -> Result<usize> {
        let patch = TermPatch { status: Some(status), ..Default::default() };
        validate_patch(&patch)?;
        let stamp = self.clock.tick(hlc::wall_ms());
        let site = self.site_id.clone();
        let tx = self.conn.transaction()?;
        let mut created = 0;
        for (text, text_lc, wc) in items {
            if find_term(&tx, language, text_lc)?.is_none() {
                upsert_term(&tx, &site, stamp, language, text, text_lc, *wc, &patch, true)?;
                created += 1;
            }
        }
        tx.commit()?;
        Ok(created)
    }

    pub fn find_term(&self, language: &str, text_lc: &str) -> Result<Option<Term>> {
        match find_term(&self.conn, language, text_lc)? {
            Some(id) => Ok(Some(self.term_by_id(&id)?)),
            None => Ok(None),
        }
    }

    fn term_by_id(&self, id: &str) -> Result<Term> {
        let mut term = self.conn.query_row(
            "SELECT id, language, text, text_lc, lemma, status, translation, reading, notes, word_count
             FROM terms WHERE id = ?1",
            [id],
            |r| {
                Ok(Term {
                    id: r.get(0)?,
                    language: r.get(1)?,
                    text: r.get(2)?,
                    text_lc: r.get(3)?,
                    lemma: r.get(4)?,
                    status: r.get(5)?,
                    translation: r.get(6)?,
                    reading: r.get(7)?,
                    notes: r.get(8)?,
                    word_count: r.get(9)?,
                    parents: vec![],
                })
            },
        )?;
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.text FROM term_parents tp JOIN terms t ON t.id = tp.parent_id
             WHERE tp.term_id = ?1 AND tp.deleted = 0 AND t.deleted = 0 ORDER BY tp.created_at, tp.id",
        )?;
        term.parents = stmt
            .query_map([id], |r| Ok(TermRef { id: r.get(0)?, text: r.get(1)? }))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(term)
    }

    /// Replace the parent set of a term. Removed links are tombstoned, re-added ones revived.
    pub fn set_term_parents(&mut self, term_id: &str, parent_ids: &[String]) -> Result<()> {
        if parent_ids.iter().any(|p| p == term_id) {
            return Err(Error::Invalid("a term cannot be its own parent".into()));
        }
        let stamp = self.clock.tick(hlc::wall_ms());
        let site = self.site_id.clone();
        let tx = self.conn.transaction()?;
        let existing: Vec<(String, String, i64)> = {
            let mut stmt = tx.prepare("SELECT id, parent_id, deleted FROM term_parents WHERE term_id = ?1")?;
            stmt.query_map([term_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<std::result::Result<_, _>>()?
        };
        for (id, parent, deleted) in &existing {
            let wanted = parent_ids.contains(parent);
            if wanted && *deleted == 1 {
                update_cols(&tx, "term_parents", id, &[("deleted", 0.into())], stamp, &site)?;
            } else if !wanted && *deleted == 0 {
                update_cols(&tx, "term_parents", id, &[("deleted", 1.into())], stamp, &site)?;
            }
        }
        for parent in parent_ids {
            if !existing.iter().any(|(_, p, _)| p == parent) {
                let cols: Vec<(&str, Value)> = vec![
                    ("created_at", (hlc::wall_ms() as i64).into()),
                    ("deleted", 0.into()),
                    ("term_id", term_id.to_string().into()),
                    ("parent_id", parent.clone().into()),
                ];
                insert_row(&tx, "term_parents", &new_id(), &cols, stamp, &site)?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Every live term of a language, for coloring tokens (`text_lc` → state).
    pub fn term_states(&self, language: &str) -> Result<Vec<TermStateRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, text_lc, status, word_count FROM terms WHERE language = ?1 AND deleted = 0 ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map([language], |r| {
            Ok(TermStateRow { id: r.get(0)?, text_lc: r.get(1)?, status: r.get(2)?, word_count: r.get(3)? })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Terms with `min <= status <= max`, oldest first, with parents.
    pub fn list_terms(&self, language: &str, min: i64, max: i64) -> Result<Vec<Term>> {
        let ids: Vec<String> = {
            let mut stmt = self.conn.prepare(
                "SELECT id FROM terms WHERE language = ?1 AND deleted = 0 AND status BETWEEN ?2 AND ?3
                 ORDER BY created_at, id",
            )?;
            stmt.query_map(params![language, min, max], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?
        };
        ids.iter().map(|id| self.term_by_id(id)).collect()
    }

    // ---- events --------------------------------------------------------------------------------

    /// Append to the event log (SPEC §4.3).
    pub fn emit_event(
        &mut self,
        ty: &str,
        language: Option<&str>,
        subject: Option<(&str, &str)>,
        payload: &serde_json::Value,
    ) -> Result<()> {
        let stamp = self.clock.tick(hlc::wall_ms());
        insert_event(&self.conn, &self.site_id, stamp, ty, language, subject, payload)
    }

    /// Connection for device-local `local_*` tables (dictionaries). Never use it for synced tables.
    pub fn local_conn(&self) -> &Connection {
        &self.conn
    }

    #[cfg(test)]
    fn conn(&self) -> &Connection {
        &self.conn
    }
}

/// Columns of `works` that may be updated through the store.
#[derive(Debug, Clone, Copy)]
pub enum WorkCol {
    Title,
    Author,
    Archived,
    Deleted,
}

impl WorkCol {
    fn name(self) -> &'static str {
        match self {
            WorkCol::Title => "title",
            WorkCol::Author => "author",
            WorkCol::Archived => "archived",
            WorkCol::Deleted => "deleted",
        }
    }
}

fn validate_patch(p: &TermPatch) -> Result<()> {
    match p.status {
        Some(s) if !STATUSES.contains(&s) => Err(Error::Invalid(format!("invalid status {s}"))),
        _ => Ok(()),
    }
}

fn find_term(conn: &Connection, language: &str, text_lc: &str) -> Result<Option<String>> {
    // Oldest live row wins if two devices created the same term concurrently.
    Ok(conn
        .query_row(
            "SELECT id FROM terms WHERE language = ?1 AND text_lc = ?2 AND deleted = 0 ORDER BY created_at, id LIMIT 1",
            params![language, text_lc],
            |r| r.get(0),
        )
        .ok())
}

#[allow(clippy::too_many_arguments)]
fn upsert_term(
    tx: &Transaction,
    site: &str,
    stamp: u64,
    language: &str,
    text: &str,
    text_lc: &str,
    word_count: i64,
    patch: &TermPatch,
    create_only: bool,
) -> Result<String> {
    let opt = |v: &Option<String>| -> Value { v.clone().map_or(Value::Null, Value::Text) };
    match find_term(tx, language, text_lc)? {
        None => {
            let id = new_id();
            let status = patch.status.unwrap_or(0);
            let mut cols: Vec<(&str, Value)> = vec![
                ("created_at", (hlc::wall_ms() as i64).into()),
                ("deleted", 0.into()),
                ("language", language.to_string().into()),
                ("text", text.to_string().into()),
                ("text_lc", text_lc.to_string().into()),
                ("status", status.into()),
                ("tags", "[]".to_string().into()),
                ("word_count", word_count.into()),
            ];
            for (name, v) in [
                ("translation", &patch.translation),
                ("reading", &patch.reading),
                ("notes", &patch.notes),
                ("lemma", &patch.lemma),
            ] {
                if v.is_some() {
                    cols.push((name, opt(v)));
                }
            }
            insert_row(tx, "terms", &id, &cols, stamp, site)?;
            if status != 0 {
                let payload = serde_json::json!({ "from": 0, "to": status });
                insert_event(tx, site, stamp, "term.status", Some(language), Some(("term", &id)), &payload)?;
            }
            Ok(id)
        }
        Some(id) => {
            if create_only {
                return Ok(id);
            }
            let (status, translation, reading, notes, lemma): (i64, Option<String>, Option<String>, Option<String>, Option<String>) =
                tx.query_row("SELECT status, translation, reading, notes, lemma FROM terms WHERE id = ?1", [&id], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                })?;
            let mut changes: Vec<(&str, Value)> = Vec::new();
            if let Some(s) = patch.status.filter(|s| *s != status) {
                changes.push(("status", s.into()));
            }
            for (name, new, old) in [
                ("translation", &patch.translation, &translation),
                ("reading", &patch.reading, &reading),
                ("notes", &patch.notes, &notes),
                ("lemma", &patch.lemma, &lemma),
            ] {
                if new.is_some() && new != old {
                    changes.push((name, opt(new)));
                }
            }
            if !changes.is_empty() {
                update_cols(tx, "terms", &id, &changes, stamp, site)?;
            }
            if let Some(to) = patch.status.filter(|s| *s != status) {
                let payload = serde_json::json!({ "from": status, "to": to });
                insert_event(tx, site, stamp, "term.status", Some(language), Some(("term", &id)), &payload)?;
            }
            Ok(id)
        }
    }
}

fn insert_event(
    conn: &Connection,
    site: &str,
    stamp: u64,
    ty: &str,
    language: Option<&str>,
    subject: Option<(&str, &str)>,
    payload: &serde_json::Value,
) -> Result<()> {
    conn.execute(
        "INSERT INTO events (id, hlc, site_id, type, language, subject_type, subject_id, payload)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![new_id(), stamp as i64, site, ty, language, subject.map(|s| s.0), subject.map(|s| s.1), payload.to_string()],
    )?;
    Ok(())
}

/// Update several columns of one row and record each in `_changes`.
fn update_cols(tx: &Transaction, tbl: &str, id: &str, cols: &[(&str, Value)], stamp: u64, site: &str) -> Result<()> {
    for (col, v) in cols {
        let n = tx.execute(&format!("UPDATE {tbl} SET {col} = ?1 WHERE id = ?2"), params![v, id])?;
        if n == 0 {
            return Err(Error::Invalid(format!("no {tbl} row {id}")));
        }
        record_change(tx, tbl, id, col, v, stamp, site)?;
    }
    Ok(())
}

fn insert_row(tx: &Transaction, tbl: &str, id: &str, cols: &[(&str, Value)], stamp: u64, site: &str) -> Result<()> {
    let names: Vec<&str> = cols.iter().map(|(n, _)| *n).collect();
    let marks: Vec<String> = (2..cols.len() + 2).map(|i| format!("?{i}")).collect();
    let sql = format!("INSERT INTO {tbl} (id, {}) VALUES (?1, {})", names.join(", "), marks.join(", "));
    let mut args: Vec<Value> = vec![id.to_string().into()];
    args.extend(cols.iter().map(|(_, v)| v.clone()));
    tx.execute(&sql, rusqlite::params_from_iter(args))?;
    for (col, v) in cols {
        record_change(tx, tbl, id, col, v, stamp, site)?;
    }
    Ok(())
}

/// Upsert the column clock; the write wins only if `(hlc, site_id)` is newer.
fn record_change(tx: &Transaction, tbl: &str, row: &str, col: &str, v: &Value, stamp: u64, site: &str) -> Result<()> {
    tx.execute(
        "INSERT INTO _changes (tbl, row_id, col, value, hlc, site_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(tbl, row_id, col) DO UPDATE SET value = excluded.value, hlc = excluded.hlc, site_id = excluded.site_id
         WHERE (excluded.hlc, excluded.site_id) > (_changes.hlc, _changes.site_id)",
        params![tbl, row, col, v, stamp as i64, site],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> NewWork {
        NewWork::text("es", "Hola", "uno dos tres")
    }

    #[test]
    fn site_id_is_stable_across_opens() {
        let dir = std::env::temp_dir().join(format!("alst-test-{}", new_id()));
        let path = dir.join("a.db");
        let a = Store::open(&path).unwrap().site_id().to_string();
        let b = Store::open(&path).unwrap().site_id().to_string();
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn create_and_list_work() {
        let mut s = Store::open_in_memory().unwrap();
        let w = s.create_work(sample()).unwrap();
        let list = s.list_works().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, w.id);
        let wc: i64 = s.conn().query_row("SELECT word_count FROM parts WHERE work_id = ?1", [&w.id], |r| r.get(0)).unwrap();
        assert_eq!(wc, 3);
    }

    #[test]
    fn create_records_changes_for_every_column() {
        let mut s = Store::open_in_memory().unwrap();
        let w = s.create_work(sample()).unwrap();
        let (n, title): (i64, String) = s
            .conn()
            .query_row(
                "SELECT count(*), max(CASE WHEN col='title' THEN value END) FROM _changes WHERE tbl='works' AND row_id=?1",
                [&w.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(n, 8);
        assert_eq!(title, "Hola");
        let parts: i64 = s.conn().query_row("SELECT count(*) FROM _changes WHERE tbl='parts'", [], |r| r.get(0)).unwrap();
        assert_eq!(parts, 6);
    }

    #[test]
    fn update_advances_the_column_clock() {
        let mut s = Store::open_in_memory().unwrap();
        let w = s.create_work(sample()).unwrap();
        let clock = |s: &Store| -> i64 {
            s.conn()
                .query_row("SELECT hlc FROM _changes WHERE tbl='works' AND row_id=?1 AND col='title'", [&w.id], |r| r.get(0))
                .unwrap()
        };
        let before = clock(&s);
        s.update_work(&w.id, WorkCol::Title, "Adiós".to_string().into()).unwrap();
        assert!(clock(&s) > before);
        assert_eq!(s.list_works().unwrap()[0].title, "Adiós");
    }

    #[test]
    fn older_remote_change_does_not_overwrite_newer() {
        let mut s = Store::open_in_memory().unwrap();
        let w = s.create_work(sample()).unwrap();
        let tx = s.conn.transaction().unwrap();
        record_change(&tx, "works", &w.id, "title", &"stale".to_string().into(), 1, "zzz").unwrap();
        tx.commit().unwrap();
        let v: String = s
            .conn()
            .query_row("SELECT value FROM _changes WHERE row_id=?1 AND col='title'", [&w.id], |r| r.get(0))
            .unwrap();
        assert_eq!(v, "Hola");
    }

    #[test]
    fn empty_title_rejected() {
        let mut s = Store::open_in_memory().unwrap();
        let mut n = sample();
        n.title = "  ".into();
        assert!(s.create_work(n).is_err());
    }

    #[test]
    fn work_with_several_parts() {
        let mut s = Store::open_in_memory().unwrap();
        let mut n = sample();
        n.kind = "epub".into();
        n.author = Some("Ana".into());
        n.parts = (0..3)
            .map(|i| NewPart { title: Some(format!("Cap {i}")), text: format!("texto {i}"), word_count: 2 })
            .collect();
        let w = s.create_work(n).unwrap();
        assert_eq!(w.part_count, 3);
        let parts = s.list_parts(&w.id).unwrap();
        assert_eq!(parts.iter().map(|p| p.idx).collect::<Vec<_>>(), vec![0, 1, 2]);
        let p = s.get_part(&parts[1].id).unwrap();
        assert_eq!((p.title.as_deref(), p.text.as_str()), (Some("Cap 1"), "texto 1"));
        assert_eq!(s.get_work(&w.id).unwrap().author.as_deref(), Some("Ana"));
        let mut empty = sample();
        empty.parts.clear();
        assert!(s.create_work(empty).is_err());
    }

    fn event_count(s: &Store, ty: &str) -> i64 {
        s.conn().query_row("SELECT count(*) FROM events WHERE type = ?1", [ty], |r| r.get(0)).unwrap()
    }

    #[test]
    fn save_term_creates_then_updates_per_column() {
        let mut s = Store::open_in_memory().unwrap();
        let patch = TermPatch { status: Some(2), translation: Some("hello".into()), ..Default::default() };
        let t = s.save_term("es", "Hola", "hola", 1, &patch).unwrap();
        assert_eq!((t.status, t.translation.as_deref(), t.text.as_str()), (2, Some("hello"), "Hola"));

        let clock = |s: &Store, col: &str| -> i64 {
            s.conn()
                .query_row("SELECT hlc FROM _changes WHERE tbl='terms' AND row_id=?1 AND col=?2", params![t.id, col], |r| r.get(0))
                .unwrap()
        };
        let (status_clock, tr_clock) = (clock(&s, "status"), clock(&s, "translation"));
        // Editing only notes leaves the other columns' clocks alone, and doesn't create a second row.
        let t2 = s.save_term("es", "HOLA", "hola", 1, &TermPatch { notes: Some("greeting".into()), ..Default::default() }).unwrap();
        assert_eq!(t2.id, t.id);
        assert_eq!(t2.text, "Hola");
        assert_eq!((t2.status, t2.notes.as_deref()), (2, Some("greeting")));
        assert_eq!((clock(&s, "status"), clock(&s, "translation")), (status_clock, tr_clock));
        assert_eq!(s.term_states("es").unwrap().len(), 1);
    }

    #[test]
    fn status_changes_emit_events_with_from_and_to() {
        let mut s = Store::open_in_memory().unwrap();
        let st = |n| TermPatch { status: Some(n), ..Default::default() };
        s.save_term("es", "gato", "gato", 1, &st(1)).unwrap();
        s.save_term("es", "gato", "gato", 1, &st(1)).unwrap(); // unchanged: no event
        s.save_term("es", "gato", "gato", 1, &st(99)).unwrap();
        assert_eq!(event_count(&s, "term.status"), 2);
        let payloads: Vec<String> = {
            let mut q = s.conn().prepare("SELECT payload FROM events WHERE type='term.status' ORDER BY hlc, id").unwrap();
            q.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
        };
        assert_eq!(payloads, vec![r#"{"from":0,"to":1}"#, r#"{"from":1,"to":99}"#]);
        // Saving only a translation on a brand-new term creates status 0 without an event.
        s.save_term("es", "perro", "perro", 1, &TermPatch { translation: Some("dog".into()), ..Default::default() }).unwrap();
        assert_eq!(event_count(&s, "term.status"), 2);
    }

    #[test]
    fn invalid_status_rejected() {
        let mut s = Store::open_in_memory().unwrap();
        for bad in [6, 50, -1, 100] {
            let p = TermPatch { status: Some(bad), ..Default::default() };
            assert!(s.save_term("es", "x", "x", 1, &p).is_err());
        }
        assert!(s.term_states("es").unwrap().is_empty());
    }

    #[test]
    fn parents_are_replaced_with_tombstones_and_revived() {
        let mut s = Store::open_in_memory().unwrap();
        let mk = |s: &mut Store, w: &str| s.save_term("es", w, w, 1, &TermPatch::default()).unwrap().id;
        let (child, a, b) = (mk(&mut s, "corriendo"), mk(&mut s, "correr"), mk(&mut s, "corrida"));
        s.set_term_parents(&child, &[a.clone(), b.clone()]).unwrap();
        assert_eq!(s.find_term("es", "corriendo").unwrap().unwrap().parents.len(), 2);
        s.set_term_parents(&child, &[a.clone()]).unwrap();
        let t = s.find_term("es", "corriendo").unwrap().unwrap();
        assert_eq!(t.parents.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(), vec!["correr"]);
        s.set_term_parents(&child, &[a.clone(), b]).unwrap();
        assert_eq!(s.find_term("es", "corriendo").unwrap().unwrap().parents.len(), 2);
        let rows: i64 = s.conn().query_row("SELECT count(*) FROM term_parents", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 2, "revived rather than duplicated");
        assert!(s.set_term_parents(&child, &[child.clone()]).is_err());
    }

    #[test]
    fn create_terms_skips_existing_and_logs_events() {
        let mut s = Store::open_in_memory().unwrap();
        s.save_term("es", "uno", "uno", 1, &TermPatch { status: Some(3), ..Default::default() }).unwrap();
        let items: Vec<(String, String, i64)> =
            ["uno", "dos", "tres"].iter().map(|w| (w.to_string(), w.to_string(), 1)).collect();
        assert_eq!(s.create_terms("es", &items, 99).unwrap(), 2);
        let by: std::collections::HashMap<_, _> = s.term_states("es").unwrap().into_iter().map(|t| (t.text_lc, t.status)).collect();
        assert_eq!((by["uno"], by["dos"], by["tres"]), (3, 99, 99));
        assert_eq!(s.list_terms("es", 99, 99).unwrap().len(), 2);
    }

    #[test]
    fn emit_event_appends() {
        let mut s = Store::open_in_memory().unwrap();
        s.emit_event("part.read", Some("es"), Some(("part", "p1")), &serde_json::json!({"words_read": 10})).unwrap();
        assert_eq!(event_count(&s, "part.read"), 1);
    }
}
