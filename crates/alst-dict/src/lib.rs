//! Yomitan-format dictionary import and lookup (SPEC §7). Data lives in the device-local
//! `local_dict_*` tables created by `alst-core` migration 0002.
//!
//! Written from the public format description; no Yomitan code is used (it is GPL-3.0).
//! Not handled yet: `term_meta_bank` (frequency, pitch), `kanji_bank`, media, deinflection.

use rusqlite::{Connection, params};
use serde::Serialize;
use serde_json::Value;
use std::io::{Cursor, Read};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a Yomitan dictionary: {0}")]
    Format(String),
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DictInfo {
    pub id: i64,
    pub title: String,
    pub revision: Option<String>,
    pub language: String,
    pub enabled: bool,
    pub entry_count: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LookupResult {
    pub dict_id: i64,
    pub dict_title: String,
    pub term: String,
    pub reading: String,
    pub tags: String,
    pub score: i64,
    pub glossary: Vec<String>,
}

const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;

fn fmt(msg: impl std::fmt::Display) -> Error {
    Error::Format(msg.to_string())
}

/// Import a Yomitan dictionary zip for `language`. All-or-nothing: a failure leaves nothing behind.
pub fn import(conn: &mut Connection, zip_bytes: &[u8], language: &str, now_ms: i64) -> Result<DictInfo> {
    let mut zip = zip::ZipArchive::new(Cursor::new(zip_bytes)).map_err(fmt)?;
    let index: Value = serde_json::from_slice(&read_file(&mut zip, "index.json")?).map_err(fmt)?;
    let title = index["title"].as_str().filter(|t| !t.is_empty()).ok_or_else(|| fmt("index.json has no title"))?.to_string();
    let format = index["format"].as_i64().or_else(|| index["version"].as_i64()).unwrap_or(3);
    let revision = index["revision"].as_str().map(String::from);

    let mut banks: Vec<String> = zip
        .file_names()
        .filter(|n| n.rsplit('/').next().is_some_and(|f| f.starts_with("term_bank_") && f.ends_with(".json")))
        .map(String::from)
        .collect();
    if banks.is_empty() {
        return Err(fmt("no term_bank_*.json files (only term dictionaries are supported)"));
    }
    banks.sort_by_key(|n| bank_number(n));

    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO local_dict_dicts (title, revision, language, imported_at) VALUES (?1, ?2, ?3, ?4)",
        params![title, revision, language, now_ms],
    )?;
    let dict_id = tx.last_insert_rowid();
    let mut count = 0i64;
    {
        let mut insert = tx.prepare(
            "INSERT INTO local_dict_entries (dict_id, term, term_lc, reading, tags, score, glossary)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for bank in &banks {
            let rows: Vec<Vec<Value>> = serde_json::from_slice(&read_file(&mut zip, bank)?).map_err(fmt)?;
            for row in rows {
                let Some(term) = row.first().and_then(Value::as_str).filter(|t| !t.is_empty()) else { continue };
                let reading = row.get(1).and_then(Value::as_str).unwrap_or("");
                let tags = row.get(2).and_then(Value::as_str).unwrap_or("");
                let score = row.get(4).and_then(Value::as_i64).unwrap_or(0);
                let items: &[Value] = if format == 1 {
                    row.get(5..).unwrap_or(&[])
                } else {
                    row.get(5).and_then(Value::as_array).map_or(&[], Vec::as_slice)
                };
                let glossary: Vec<String> = items.iter().map(flatten).filter(|g| !g.is_empty()).collect();
                if glossary.is_empty() {
                    continue;
                }
                insert.execute(params![
                    dict_id,
                    term,
                    term.to_lowercase(),
                    reading,
                    tags,
                    score,
                    Value::from(glossary).to_string()
                ])?;
                count += 1;
            }
        }
    }
    tx.execute("UPDATE local_dict_dicts SET entry_count = ?1 WHERE id = ?2", params![count, dict_id])?;
    tx.commit()?;
    Ok(DictInfo { id: dict_id, title, revision, language: language.to_string(), enabled: true, entry_count: count })
}

fn bank_number(name: &str) -> u64 {
    name.chars().filter(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0)
}

fn read_file<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, name: &str) -> Result<Vec<u8>> {
    let f = zip.by_name(name).map_err(|_| fmt(format!("missing {name}")))?;
    let mut buf = Vec::new();
    f.take(MAX_FILE_BYTES).read_to_end(&mut buf).map_err(fmt)?;
    Ok(buf)
}

/// Reduce one glossary item (string, text/image/structured-content object) to plain text.
fn flatten(item: &Value) -> String {
    let mut out = String::new();
    flatten_into(item, &mut out);
    out.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

fn flatten_into(v: &Value, out: &mut String) {
    match v {
        Value::String(s) => out.push_str(s),
        Value::Array(a) => a.iter().for_each(|x| flatten_into(x, out)),
        Value::Object(o) => {
            if o.get("type").and_then(Value::as_str) == Some("image") {
                return;
            }
            if let Some(Value::String(t)) = o.get("text") {
                out.push_str(t);
            }
            let block = matches!(
                o.get("tag").and_then(Value::as_str),
                Some("div" | "li" | "p" | "ul" | "ol" | "tr" | "br" | "table" | "details" | "summary")
            );
            if block {
                out.push('\n');
            }
            if let Some(c) = o.get("content") {
                flatten_into(c, out);
            }
            if block {
                out.push('\n');
            }
        }
        _ => {}
    }
}

pub fn list(conn: &Connection) -> Result<Vec<DictInfo>> {
    let mut stmt =
        conn.prepare("SELECT id, title, revision, language, enabled, entry_count FROM local_dict_dicts ORDER BY id")?;
    let rows = stmt.query_map([], |r| {
        Ok(DictInfo {
            id: r.get(0)?,
            title: r.get(1)?,
            revision: r.get(2)?,
            language: r.get(3)?,
            enabled: r.get::<_, i64>(4)? != 0,
            entry_count: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn delete(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM local_dict_dicts WHERE id = ?1", [id])?; // entries cascade
    Ok(())
}

pub fn set_enabled(conn: &Connection, id: i64, enabled: bool) -> Result<()> {
    conn.execute("UPDATE local_dict_dicts SET enabled = ?1 WHERE id = ?2", params![enabled as i64, id])?;
    Ok(())
}

/// Exact headword (case-insensitive) or reading matches in the enabled dictionaries of `language`,
/// in dictionary order, then by score.
pub fn lookup(conn: &Connection, language: &str, query: &str, limit: usize) -> Result<Vec<LookupResult>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(vec![]);
    }
    let mut stmt = conn.prepare(
        "SELECT d.id, d.title, e.term, e.reading, e.tags, e.score, e.glossary
         FROM local_dict_entries e JOIN local_dict_dicts d ON d.id = e.dict_id
         WHERE d.enabled = 1 AND d.language = ?1 AND (e.term_lc = ?2 OR e.reading = ?3)
         ORDER BY d.id, e.score DESC, e.id LIMIT ?4",
    )?;
    let rows = stmt.query_map(params![language, q.to_lowercase(), q, limit as i64], |r| {
        let glossary: String = r.get(6)?;
        Ok(LookupResult {
            dict_id: r.get(0)?,
            dict_title: r.get(1)?,
            term: r.get(2)?,
            reading: r.get(3)?,
            tags: r.get(4)?,
            score: r.get(5)?,
            glossary: serde_json::from_str(&glossary).unwrap_or_default(),
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::{ZipWriter, write::SimpleFileOptions};

    fn make_zip(files: &[(&str, &str)]) -> Vec<u8> {
        let mut z = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, body) in files {
            z.start_file(*name, SimpleFileOptions::default()).unwrap();
            z.write_all(body.as_bytes()).unwrap();
        }
        z.finish().unwrap().into_inner()
    }

    fn v3() -> Vec<u8> {
        make_zip(&[
            ("index.json", r#"{"title":"Mini ES-EN","format":3,"revision":"r1"}"#),
            (
                "term_bank_1.json",
                r#"[["Gato","","n m",[],5,["cat","male cat"],1,""],
                    ["gato","","n m",[],9,[{"type":"structured-content","content":[{"tag":"ul","content":[{"tag":"li","content":"jack (tool)"},{"tag":"li","content":["car ","jack"]}]}]}],2,""],
                    ["vacío","","",[],0,[],3,""]]"#,
            ),
            ("term_bank_2.json", r#"[["perro","","n",[],1,[{"type":"text","text":"dog"},{"type":"image","path":"x.png"}],4,""]]"#),
            ("term_meta_bank_1.json", "[]"),
        ])
    }

    #[test]
    fn imports_and_looks_up() {
        let mut conn = alst_core::db::open_in_memory().unwrap();
        let info = import(&mut conn, &v3(), "es", 1).unwrap();
        assert_eq!((info.title.as_str(), info.entry_count), ("Mini ES-EN", 3)); // empty glossary skipped
        let r = lookup(&conn, "es", "  GATO ", 10).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].glossary, vec!["jack (tool)\ncar jack"], "higher score first, structured content flattened");
        assert_eq!(r[1].glossary, vec!["cat", "male cat"]);
        assert_eq!(lookup(&conn, "es", "perro", 10).unwrap()[0].glossary, vec!["dog"]);
        assert!(lookup(&conn, "es", "nada", 10).unwrap().is_empty());
        assert!(lookup(&conn, "fr", "gato", 10).unwrap().is_empty(), "language filter");
    }

    #[test]
    fn format_1_rows_and_reading_lookup() {
        let mut conn = alst_core::db::open_in_memory().unwrap();
        let z = make_zip(&[
            ("index.json", r#"{"title":"Old","format":1}"#),
            ("term_bank_1.json", r#"[["漢字","かんじ","n","",0,"kanji","Chinese characters"]]"#),
        ]);
        import(&mut conn, &z, "ja", 1).unwrap();
        let r = lookup(&conn, "ja", "かんじ", 5).unwrap();
        assert_eq!((r[0].term.as_str(), r[0].glossary.clone()), ("漢字", vec!["kanji".to_string(), "Chinese characters".into()]));
    }

    #[test]
    fn disable_and_delete() {
        let mut conn = alst_core::db::open_in_memory().unwrap();
        let info = import(&mut conn, &v3(), "es", 1).unwrap();
        set_enabled(&conn, info.id, false).unwrap();
        assert!(lookup(&conn, "es", "gato", 5).unwrap().is_empty());
        assert!(!list(&conn).unwrap()[0].enabled);
        delete(&conn, info.id).unwrap();
        let n: i64 = conn.query_row("SELECT count(*) FROM local_dict_entries", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn bad_archives_leave_nothing_behind() {
        let mut conn = alst_core::db::open_in_memory().unwrap();
        assert!(import(&mut conn, b"nope", "es", 1).is_err());
        assert!(import(&mut conn, &make_zip(&[("index.json", r#"{"title":"x"}"#)]), "es", 1).is_err());
        let broken = make_zip(&[("index.json", r#"{"title":"x"}"#), ("term_bank_1.json", "not json")]);
        assert!(import(&mut conn, &broken, "es", 1).is_err());
        assert!(list(&conn).unwrap().is_empty());
    }
}
