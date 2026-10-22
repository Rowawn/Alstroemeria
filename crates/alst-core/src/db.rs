//! SQLite open/create and the migration runner.

use crate::Result;
use rusqlite::Connection;
use std::path::Path;

/// Migrations in order. Never edit an applied one; add a new numbered file.
const MIGRATIONS: &[(i64, &str)] = &[(1, include_str!("../migrations/0001_core.sql"))];

pub fn open(path: &Path) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| crate::Error::Invalid(e.to_string()))?;
    }
    init(Connection::open(path)?)
}

pub fn open_in_memory() -> Result<Connection> {
    init(Connection::open_in_memory()?)
}

fn init(mut conn: Connection) -> Result<Connection> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    migrate(&mut conn)?;
    Ok(conn)
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (version, sql) in MIGRATIONS.iter().filter(|(v, _)| *v > current) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_create_core_tables_and_are_idempotent() {
        let mut conn = open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        for t in [
            "works", "parts", "concepts", "terms", "term_parents", "media", "extension_state",
            "ext_data", "settings", "events", "_changes", "local_settings", "local_peers",
        ] {
            let n: i64 = conn
                .query_row("SELECT count(*) FROM sqlite_master WHERE name = ?1", [t], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 1, "missing table {t}");
        }
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, MIGRATIONS.last().unwrap().0);
    }
}
