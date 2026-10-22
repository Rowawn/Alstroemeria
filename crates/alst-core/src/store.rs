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
pub struct NewWork {
    pub language: String,
    pub title: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Work {
    pub id: String,
    pub language: String,
    pub title: String,
    pub kind: String,
    pub created_at: i64,
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

    /// Insert a work and its single part, recording every column in `_changes`.
    pub fn create_work(&mut self, new: NewWork) -> Result<Work> {
        if new.title.trim().is_empty() {
            return Err(Error::Invalid("title is required".into()));
        }
        let now = hlc::wall_ms();
        let stamp = self.clock.tick(now);
        let (work_id, part_id) = (new_id(), new_id());
        let created = now as i64;
        let word_count = new.text.split_whitespace().count() as i64;
        let site = self.site_id.clone();
        let tx = self.conn.transaction()?;

        let work_cols: Vec<(&str, Value)> = vec![
            ("created_at", created.into()),
            ("deleted", 0.into()),
            ("language", new.language.clone().into()),
            ("title", new.title.clone().into()),
            ("kind", "text".to_string().into()),
            ("tags", "[]".to_string().into()),
            ("archived", 0.into()),
            ("settings", "{}".to_string().into()),
        ];
        insert_row(&tx, "works", &work_id, &work_cols, stamp, &site)?;

        let part_cols: Vec<(&str, Value)> = vec![
            ("created_at", created.into()),
            ("deleted", 0.into()),
            ("work_id", work_id.clone().into()),
            ("idx", 0.into()),
            ("text", new.text.into()),
            ("word_count", word_count.into()),
        ];
        insert_row(&tx, "parts", &part_id, &part_cols, stamp, &site)?;
        tx.commit()?;

        Ok(Work { id: work_id, language: new.language, title: new.title, kind: "text".into(), created_at: created })
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
            "SELECT id, language, title, kind, created_at FROM works
             WHERE deleted = 0 AND archived = 0 ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Work { id: r.get(0)?, language: r.get(1)?, title: r.get(2)?, kind: r.get(3)?, created_at: r.get(4)? })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
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
        NewWork { language: "es".into(), title: "Hola".into(), text: "uno dos tres".into() }
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
}
