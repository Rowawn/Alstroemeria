//! One-time re-key (SPEC §9.1): rows created before M6 have UUIDv7 ids; rewrite them to derived ids so
//! every device agrees on the id of a natural-key row. Runs inside one transaction at first start of an
//! M6 build, before any sync, so independent devices produce the same ids.

use crate::Result;
use crate::store::derive_id;
use rusqlite::Connection;
use std::collections::HashMap;

/// Move every row whose id is not the derived one, merging rows that map to the same id (a deleted
/// term and its re-creation): the live row wins, else the newer `created_at`; the other's row and
/// `_changes` are dropped. References (`terms.concept_id`, `term_parents.*`, `events.subject_id`) and
/// their `_changes.value` follow.
pub(crate) fn rekey_v1(conn: &mut Connection) -> Result<()> {
    let done: Option<String> =
        conn.query_row("SELECT value FROM local_settings WHERE key = 'rekey_v1'", [], |r| r.get(0)).ok();
    if done.as_deref() == Some("1") {
        return Ok(());
    }
    let tx = conn.transaction()?;
    tx.execute_batch(
        "PRAGMA defer_foreign_keys = ON;
         CREATE TEMP TABLE rk (tbl TEXT NOT NULL, old TEXT NOT NULL, new TEXT NOT NULL, keep INTEGER NOT NULL, PRIMARY KEY (tbl, old));",
    )?;

    // concepts, then terms (which reference concepts), then term_parents (which reference terms), then ext_data.
    plan(&tx, "concepts", "SELECT id, deleted, created_at, language, kind, key FROM concepts", 3)?;
    apply(&tx, "concepts")?;
    tx.execute_batch(
        "UPDATE terms SET concept_id = (SELECT new FROM rk WHERE tbl='concepts' AND old = concept_id)
           WHERE concept_id IN (SELECT old FROM rk WHERE tbl='concepts');
         UPDATE _changes SET value = (SELECT new FROM rk WHERE tbl='concepts' AND old = _changes.value)
           WHERE tbl='terms' AND col='concept_id' AND value IN (SELECT old FROM rk WHERE tbl='concepts');
         UPDATE events SET subject_id = (SELECT new FROM rk WHERE tbl='concepts' AND old = subject_id)
           WHERE subject_type='concept' AND subject_id IN (SELECT old FROM rk WHERE tbl='concepts');",
    )?;

    plan(&tx, "terms", "SELECT id, deleted, created_at, language, text_lc FROM terms", 2)?;
    apply(&tx, "terms")?;
    for col in ["term_id", "parent_id"] {
        tx.execute_batch(&format!(
            "UPDATE term_parents SET {col} = (SELECT new FROM rk WHERE tbl='terms' AND old = {col})
               WHERE {col} IN (SELECT old FROM rk WHERE tbl='terms');
             UPDATE _changes SET value = (SELECT new FROM rk WHERE tbl='terms' AND old = _changes.value)
               WHERE tbl='term_parents' AND col='{col}' AND value IN (SELECT old FROM rk WHERE tbl='terms');"
        ))?;
    }
    tx.execute_batch(
        "UPDATE events SET subject_id = (SELECT new FROM rk WHERE tbl='terms' AND old = subject_id)
           WHERE subject_type='term' AND subject_id IN (SELECT old FROM rk WHERE tbl='terms');",
    )?;

    plan(&tx, "term_parents", "SELECT id, deleted, created_at, term_id, parent_id FROM term_parents", 2)?;
    apply(&tx, "term_parents")?;
    plan(&tx, "ext_data", "SELECT id, deleted, created_at, extension_id, key FROM ext_data", 2)?;
    apply(&tx, "ext_data")?;

    tx.execute("INSERT OR REPLACE INTO local_settings (key, value) VALUES ('rekey_v1', '1')", [])?;
    tx.execute_batch("DROP TABLE rk")?;
    tx.commit()?;
    Ok(())
}

/// Fill `rk` for `tbl`: `sql` yields `id, deleted, created_at` then `nkeys` natural-key columns.
fn plan(tx: &rusqlite::Transaction, tbl: &str, sql: &str, nkeys: usize) -> Result<()> {
    let mut groups: HashMap<String, Vec<(String, i64, i64)>> = HashMap::new();
    let mut stmt = tx.prepare(sql)?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let old: String = r.get(0)?;
        let keys: Vec<String> = (0..nkeys).map(|i| r.get::<_, String>(3 + i)).collect::<std::result::Result<_, _>>()?;
        let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        groups.entry(derive_id(tbl, &refs)).or_default().push((old, r.get(1)?, r.get(2)?));
    }
    drop(rows);
    drop(stmt);
    let mut ins = tx.prepare("INSERT INTO rk (tbl, old, new, keep) VALUES (?1, ?2, ?3, ?4)")?;
    for (new, mut rows) in groups {
        // Winner: live first, then newest, then highest id (deterministic).
        rows.sort_by(|a, b| a.1.cmp(&b.1).then(b.2.cmp(&a.2)).then(b.0.cmp(&a.0)));
        for (i, (old, _, _)) in rows.iter().enumerate() {
            ins.execute(rusqlite::params![tbl, old, new, i64::from(i == 0)])?;
        }
    }
    Ok(())
}

/// Drop the losers of each merge, then rewrite the winners' ids (row and `_changes`).
fn apply(tx: &rusqlite::Transaction, tbl: &str) -> Result<()> {
    tx.execute_batch(&format!(
        "DELETE FROM {tbl} WHERE id IN (SELECT old FROM rk WHERE tbl='{tbl}' AND keep=0);
         DELETE FROM _changes WHERE tbl='{tbl}' AND row_id IN (SELECT old FROM rk WHERE tbl='{tbl}' AND keep=0);
         UPDATE {tbl} SET id = (SELECT new FROM rk WHERE tbl='{tbl}' AND old = {tbl}.id)
           WHERE id IN (SELECT old FROM rk WHERE tbl='{tbl}' AND keep=1 AND old <> new);
         UPDATE _changes SET row_id = (SELECT new FROM rk WHERE tbl='{tbl}' AND old = _changes.row_id)
           WHERE tbl='{tbl}' AND row_id IN (SELECT old FROM rk WHERE tbl='{tbl}' AND keep=1 AND old <> new);"
    ))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Store, TermPatch, derive_id, term_id};

    fn change(c: &Connection, tbl: &str, row: &str, col: &str, v: Option<&str>, hlc: i64) {
        c.execute("INSERT INTO _changes (tbl,row_id,col,value,hlc,site_id) VALUES (?1,?2,?3,?4,?5,'s1')", rusqlite::params![tbl, row, col, v, hlc])
            .unwrap();
    }

    fn legacy_term(c: &Connection, id: &str, text: &str, deleted: i64, created: i64, concept: Option<&str>) {
        c.execute(
            "INSERT INTO terms (id, created_at, deleted, language, text, text_lc, concept_id) VALUES (?1,?2,?3,'es',?4,?4,?5)",
            rusqlite::params![id, created, deleted, text, concept],
        )
        .unwrap();
        change(c, "terms", id, "text_lc", Some(text), created);
        change(c, "terms", id, "deleted", Some(&deleted.to_string()), created);
        if let Some(k) = concept {
            change(c, "terms", id, "concept_id", Some(k), created);
        }
    }

    #[test]
    fn rekey_rewrites_ids_references_changes_and_events_and_merges_a_recreated_term() {
        let mut c = crate::db::open_in_memory().unwrap();
        c.execute("INSERT INTO concepts (id, created_at, language, key) VALUES ('c-old', 1, 'es', 'gato')", []).unwrap();
        change(&c, "concepts", "c-old", "key", Some("gato"), 1);
        legacy_term(&c, "t-a", "gato", 1, 10, Some("c-old")); // deleted, then re-created as t-b
        legacy_term(&c, "t-b", "gato", 0, 20, Some("c-old"));
        legacy_term(&c, "t-c", "perro", 0, 15, None);
        for (id, term, parent, deleted) in [("l-1", "t-a", "t-c", 0), ("l-2", "t-b", "t-c", 0), ("l-3", "t-c", "t-b", 1)] {
            c.execute(
                "INSERT INTO term_parents (id, created_at, deleted, term_id, parent_id) VALUES (?1, 5, ?2, ?3, ?4)",
                rusqlite::params![id, deleted, term, parent],
            )
            .unwrap();
            change(&c, "term_parents", id, "term_id", Some(term), 5);
            change(&c, "term_parents", id, "parent_id", Some(parent), 5);
        }
        c.execute("INSERT INTO ext_data (id, created_at, extension_id, key, value) VALUES ('e-1', 1, 'x', 'k', '1')", []).unwrap();
        c.execute("INSERT INTO events (id, hlc, site_id, type, subject_type, subject_id) VALUES ('ev', 1, 's1', 'term.status', 'term', 't-a')", [])
            .unwrap();

        rekey_v1(&mut c).unwrap();

        fn ids(c: &Connection, sql: &str) -> Vec<String> {
            let mut q = c.prepare(sql).unwrap();
            q.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
        }
        let gato = term_id("es", "gato");
        let perro = term_id("es", "perro");
        let mut terms = ids(&c, "SELECT id FROM terms");
        terms.sort();
        let mut want = vec![gato.clone(), perro.clone()];
        want.sort();
        assert_eq!(terms, want, "the deleted twin is dropped, ids are derived");
        let live: i64 = c.query_row("SELECT deleted FROM terms WHERE id = ?1", [&gato], |r| r.get(0)).unwrap();
        assert_eq!(live, 0, "the live row won");
        assert_eq!(ids(&c, "SELECT DISTINCT row_id FROM _changes WHERE tbl = 'terms' ORDER BY 1").len(), 2, "no _changes for dropped rows");
        let concept = derive_id("concepts", &["es", "lexeme", "gato"]);
        assert_eq!(ids(&c, "SELECT id FROM concepts"), vec![concept.clone()]);
        assert_eq!(ids(&c, &format!("SELECT concept_id FROM terms WHERE id = '{gato}'")), vec![concept.clone()]);
        assert_eq!(ids(&c, &format!("SELECT value FROM _changes WHERE tbl='terms' AND row_id='{gato}' AND col='concept_id'")), vec![concept]);
        // l-1 and l-2 both became gato -> perro; the live one wins. l-3 is perro -> gato (deleted).
        let mut links = ids(&c, "SELECT id FROM term_parents");
        links.sort();
        let mut want = vec![derive_id("term_parents", &[&gato, &perro]), derive_id("term_parents", &[&perro, &gato])];
        want.sort();
        assert_eq!(links, want);
        assert_eq!(ids(&c, &format!("SELECT parent_id FROM term_parents WHERE id = '{}'", derive_id("term_parents", &[&gato, &perro]))), vec![perro.clone()]);
        assert_eq!(ids(&c, "SELECT subject_id FROM events"), vec![gato]);
        assert_eq!(ids(&c, "SELECT id FROM ext_data"), vec![derive_id("ext_data", &["x", "k"])]);
        let fk: i64 = c.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r.get(0)).unwrap();
        assert_eq!(fk, 0);
        let flag: String = c.query_row("SELECT value FROM local_settings WHERE key='rekey_v1'", [], |r| r.get(0)).unwrap();
        assert_eq!(flag, "1");
        // Running again changes nothing.
        let before = ids(&c, "SELECT id FROM terms ORDER BY id");
        rekey_v1(&mut c).unwrap();
        assert_eq!(ids(&c, "SELECT id FROM terms ORDER BY id"), before);
    }

    #[test]
    fn two_devices_with_the_same_legacy_term_agree_after_rekey() {
        let mk = || {
            let mut c = crate::db::open_in_memory().unwrap();
            legacy_term(&c, &crate::store::new_id(), "hola", 0, 10, None);
            rekey_v1(&mut c).unwrap();
            c.query_row("SELECT id FROM terms", [], |r| r.get::<_, String>(0)).unwrap()
        };
        assert_eq!(mk(), mk());
    }

    #[test]
    fn new_terms_use_derived_ids_and_deleted_ones_are_revived_with_fresh_columns() {
        let mut s = Store::open_in_memory().unwrap();
        let p = TermPatch { status: Some(3), translation: Some("hi".into()), ..Default::default() };
        let t = s.save_term("es", "Hola", "hola", 1, &p).unwrap();
        assert_eq!(t.id, term_id("es", "hola"));
        assert!(s.delete_term("es", "hola").unwrap());
        assert!(s.find_term("es", "hola").unwrap().is_none());
        let t2 = s.save_term("es", "hola", "hola", 1, &TermPatch { status: Some(1), ..Default::default() }).unwrap();
        assert_eq!(t2.id, t.id, "revived, not duplicated");
        assert_eq!((t2.status, t2.translation), (1, None), "nothing of the deleted row comes back");
        let n: i64 = s.local_conn().query_row("SELECT count(*) FROM terms", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        s.ext_data_set("e", "k", &serde_json::json!(1)).unwrap();
        let id: String = s.local_conn().query_row("SELECT id FROM ext_data", [], |r| r.get(0)).unwrap();
        assert_eq!(id, derive_id("ext_data", &["e", "k"]));
    }

    #[test]
    fn the_sync_clock_follows_every_local_write() {
        let mut s = Store::open_in_memory().unwrap();
        assert!(s.vector().unwrap().is_empty());
        s.save_term("es", "a", "a", 1, &TermPatch { status: Some(1), ..Default::default() }).unwrap();
        s.emit_event("part.read", None, None, &serde_json::json!({})).unwrap();
        let v = s.vector().unwrap();
        let max: i64 = s.local_conn().query_row("SELECT MAX(hlc) FROM (SELECT hlc FROM _changes UNION ALL SELECT hlc FROM events)", [], |r| r.get(0)).unwrap();
        assert_eq!(v.get(s.site_id()), Some(&max));
        assert_eq!(v.len(), 1);
    }
}
