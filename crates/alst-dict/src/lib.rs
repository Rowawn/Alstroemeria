//! Yomitan-format dictionary import and lookup (SPEC §7). Data lives in the device-local
//! `local_dict_*` tables created by `alst-core` migration 0002.
//!
//! Written from the public format description; no Yomitan code is used (it is GPL-3.0).
//! Imports term banks (with `rules`) and `term_meta_bank` frequency/pitch data (SPEC §16.7);
//! `lookup_scan` (SPEC §16.6) finds dictionary forms of conjugated text via `alst-text::deinflect`.
//! Not handled: `kanji_bank`, media.

use alst_text::deinflect::Deinflector;
use rusqlite::{Connection, params};
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
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
    /// False for dictionaries imported before `rules` were stored: re-import to enable conjugation lookup.
    pub rules_imported: bool,
    pub meta_count: i64,
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
            "INSERT INTO local_dict_entries (dict_id, term, term_lc, reading, tags, score, glossary, rules)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for bank in &banks {
            let rows: Vec<Vec<Value>> = serde_json::from_slice(&read_file(&mut zip, bank)?).map_err(fmt)?;
            for row in rows {
                let Some(term) = row.first().and_then(Value::as_str).filter(|t| !t.is_empty()) else { continue };
                let reading = row.get(1).and_then(Value::as_str).unwrap_or("");
                let tags = row.get(2).and_then(Value::as_str).unwrap_or("");
                let rules = normalize_rules(row.get(3).and_then(Value::as_str).unwrap_or(""));
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
                    Value::from(glossary).to_string(),
                    rules
                ])?;
                count += 1;
            }
        }
    }
    let meta_count = import_meta(&tx, &mut zip, dict_id)?;
    tx.execute(
        "UPDATE local_dict_dicts SET entry_count = ?1, rules_imported = 1 WHERE id = ?2",
        params![count, dict_id],
    )?;
    tx.commit()?;
    Ok(DictInfo { id: dict_id, title, revision, language: language.to_string(), enabled: true, entry_count: count, rules_imported: true, meta_count })
}

/// Collapse JMdict-style rule tags to the condition names the deinflector uses (`v5k` → `v5`, …).
fn normalize_rules(rules: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for t in rules.split_whitespace() {
        let n = match t {
            t if t.starts_with("v5") => "v5",
            "v1-s" => "v1",
            t if t.starts_with("vs-") => "vs",
            "adj-ix" => "adj-i",
            t => t,
        };
        if !out.contains(&n) {
            out.push(n);
        }
    }
    out.join(" ")
}

/// Import `term_meta_bank_*.json` rows: `[term, mode, data]` with mode `freq` or `pitch` (others ignored).
/// Stored data: freq → `{value, display?}`, pitch → `{pitches: [{position, nasal?, devoice?}]}`; the reading
/// (when the bank gives one) goes in its own column.
fn import_meta<R: Read + std::io::Seek>(tx: &rusqlite::Transaction, zip: &mut zip::ZipArchive<R>, dict_id: i64) -> Result<i64> {
    let mut banks: Vec<String> = zip
        .file_names()
        .filter(|n| n.rsplit('/').next().is_some_and(|f| f.starts_with("term_meta_bank_") && f.ends_with(".json")))
        .map(String::from)
        .collect();
    banks.sort_by_key(|n| bank_number(n));
    let mut insert = tx.prepare("INSERT INTO local_dict_meta (dict_id, term, reading, mode, data) VALUES (?1, ?2, ?3, ?4, ?5)")?;
    let mut count = 0;
    for bank in &banks {
        let rows: Vec<Vec<Value>> = serde_json::from_slice(&read_file(zip, bank)?).map_err(fmt)?;
        for row in rows {
            let (Some(term), Some(mode), Some(data)) = (row.first().and_then(Value::as_str), row.get(1).and_then(Value::as_str), row.get(2)) else {
                continue;
            };
            let parsed = match mode {
                "freq" => parse_freq(data),
                "pitch" => parse_pitch(data),
                _ => None,
            };
            if let Some((reading, data)) = parsed.filter(|_| !term.is_empty()) {
                insert.execute(params![dict_id, term, reading, mode, data.to_string()])?;
                count += 1;
            }
        }
    }
    Ok(count)
}

fn freq_value(v: &Value) -> Option<(i64, Option<String>)> {
    match v {
        Value::Number(n) => Some((n.as_f64()? as i64, None)),
        Value::Object(o) => Some((o.get("value")?.as_f64()? as i64, o.get("displayValue").and_then(Value::as_str).map(String::from))),
        _ => None,
    }
}

fn parse_freq(data: &Value) -> Option<(String, Value)> {
    let (reading, inner) = match data {
        Value::Object(o) if o.contains_key("frequency") => (o.get("reading").and_then(Value::as_str).unwrap_or("").to_string(), o.get("frequency")?),
        other => (String::new(), other),
    };
    let (value, display) = freq_value(inner)?;
    let mut d = serde_json::json!({ "value": value });
    if let Some(display) = display {
        d["display"] = display.into();
    }
    Some((reading, d))
}

fn parse_pitch(data: &Value) -> Option<(String, Value)> {
    let reading = data.get("reading")?.as_str()?.to_string();
    let pitches: Vec<Value> = data
        .get("pitches")?
        .as_array()?
        .iter()
        .filter_map(|p| {
            let mut o = serde_json::Map::new();
            o.insert("position".into(), p.get("position")?.clone());
            for k in ["nasal", "devoice"] {
                if let Some(v) = p.get(k).filter(|v| !v.is_null()) {
                    o.insert(k.into(), v.clone());
                }
            }
            Some(Value::Object(o))
        })
        .collect();
    if pitches.is_empty() {
        return None;
    }
    Some((reading.clone(), serde_json::json!({ "reading": reading, "pitches": pitches })))
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
        conn.prepare("SELECT id, title, revision, language, enabled, entry_count, rules_imported, (SELECT count(*) FROM local_dict_meta m WHERE m.dict_id = local_dict_dicts.id) FROM local_dict_dicts ORDER BY id")?;
    let rows = stmt.query_map([], |r| {
        Ok(DictInfo {
            id: r.get(0)?,
            title: r.get(1)?,
            revision: r.get(2)?,
            language: r.get(3)?,
            enabled: r.get::<_, i64>(4)? != 0,
            entry_count: r.get(5)?,
            rules_imported: r.get::<_, i64>(6)? != 0,
            meta_count: r.get(7)?,
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

/// A meta row for a term (`mode` = `freq` or `pitch`) from the enabled dictionaries of `language`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaResult {
    pub dict_id: i64,
    pub dict_title: String,
    pub term: String,
    pub reading: String,
    pub mode: String,
    pub data: Value,
}

/// Meta rows for `term` (SPEC §16.7). With `reading`, rows for that reading or for no particular reading.
pub fn meta(conn: &Connection, language: &str, term: &str, reading: Option<&str>, mode: &str) -> Result<Vec<MetaResult>> {
    let mut stmt = conn.prepare(
        "SELECT d.id, d.title, m.term, m.reading, m.mode, m.data
         FROM local_dict_meta m JOIN local_dict_dicts d ON d.id = m.dict_id
         WHERE d.enabled = 1 AND d.language = ?1 AND m.term = ?2 AND m.mode = ?3
           AND (?4 IS NULL OR m.reading = '' OR m.reading = ?4)
         ORDER BY d.id, m.id",
    )?;
    let rows = stmt.query_map(params![language, term, mode, reading], |r| {
        let data: String = r.get(5)?;
        Ok(MetaResult {
            dict_id: r.get(0)?,
            dict_title: r.get(1)?,
            term: r.get(2)?,
            reading: r.get(3)?,
            mode: r.get(4)?,
            data: serde_json::from_str(&data).unwrap_or(Value::Null),
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// One matched span of the scanned text (SPEC §16.6).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    /// The text that matched (a prefix of the scanned text).
    pub matched: String,
    /// Dictionary headword.
    pub lemma: String,
    pub reading: String,
    /// How `matched` inflects from the headword, in conjugation order.
    pub reasons: Vec<String>,
    pub entries: Vec<LookupResult>,
    /// Best (lowest) frequency rank over the enabled dictionaries, if any dictionary has one.
    pub frequency: Option<i64>,
}

fn kana_shift(s: &str, to_hiragana: bool) -> String {
    s.chars()
        .map(|c| match (c, to_hiragana) {
            ('\u{30A1}'..='\u{30F6}', true) => char::from_u32(c as u32 - 0x60).unwrap_or(c),
            ('\u{3041}'..='\u{3096}', false) => char::from_u32(c as u32 + 0x60).unwrap_or(c),
            _ => c,
        })
        .collect()
}

struct Row {
    result: LookupResult,
    rules: HashSet<String>,
}

fn query_form(conn: &Connection, language: &str, form: &str) -> Result<Vec<Row>> {
    let mut stmt = conn.prepare_cached(
        "SELECT d.id, d.title, e.term, e.reading, e.tags, e.score, e.glossary, e.rules
         FROM local_dict_entries e JOIN local_dict_dicts d ON d.id = e.dict_id
         WHERE d.enabled = 1 AND d.language = ?1 AND (e.term_lc = ?2 OR e.reading = ?2 OR e.reading = ?3 OR e.reading = ?4)
         ORDER BY d.id, e.score DESC, e.id LIMIT 60",
    )?;
    let lc = form.to_lowercase();
    let rows = stmt.query_map(params![language, lc, kana_shift(form, true), kana_shift(form, false)], |r| {
        let glossary: String = r.get(6)?;
        let rules: String = r.get(7)?;
        Ok(Row {
            result: LookupResult {
                dict_id: r.get(0)?,
                dict_title: r.get(1)?,
                term: r.get(2)?,
                reading: r.get(3)?,
                tags: r.get(4)?,
                score: r.get(5)?,
                glossary: serde_json::from_str(&glossary).unwrap_or_default(),
            },
            rules: rules.split_whitespace().map(String::from).collect(),
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

fn frequency(conn: &Connection, language: &str, term: &str, reading: &str) -> Result<Option<i64>> {
    let mut stmt = conn.prepare_cached(
        "SELECT min(json_extract(m.data, '$.value'))
         FROM local_dict_meta m JOIN local_dict_dicts d ON d.id = m.dict_id
         WHERE d.enabled = 1 AND d.language = ?1 AND m.term = ?2 AND m.mode = 'freq' AND (m.reading = '' OR m.reading = ?3)",
    )?;
    Ok(stmt.query_row(params![language, term, reading], |r| r.get::<_, Option<i64>>(0))?)
}

/// Find dictionary entries for the start of `text` (SPEC §16.6): every prefix from the longest
/// (at most `max_len` characters) down, tried as written, kana-normalised and deinflected, and
/// validated against the entries' `rules`. Stops after the longest prefix with results plus two
/// shorter ones. Sorted by match length, then frequency, then dictionary order.
pub fn lookup_scan(conn: &Connection, language: &str, deinflector: Option<&Deinflector>, text: &str, max_len: usize) -> Result<Vec<ScanResult>> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out: Vec<(ScanResult, i64)> = Vec::new(); // (result, first dict id)
    let mut cache: HashMap<String, Vec<Row>> = HashMap::new();
    let mut hits = 0;
    for len in (1..=chars.len().min(max_len)).rev() {
        let end = chars.get(len).map_or(text.len(), |c| c.0);
        let prefix = &text[..end];
        if prefix.chars().all(char::is_whitespace) {
            continue;
        }
        let mut forms: Vec<(String, Vec<String>, HashSet<String>, bool)> = Vec::new(); // text, reasons, conditions, deinflected
        let hira = kana_shift(prefix, true);
        for base in [prefix.to_string(), hira].iter().collect::<Vec<_>>() {
            match deinflector {
                Some(d) => {
                    for c in d.deinflect(base) {
                        let deinflected = !c.reasons.is_empty();
                        forms.push((c.text, c.reasons, c.conditions.into_iter().collect(), deinflected));
                    }
                }
                None => forms.push((base.clone(), vec![], HashSet::new(), false)),
            }
        }
        // (headword, reading) → result for this prefix
        let mut found: Vec<ScanResult> = Vec::new();
        let mut seen_forms: HashSet<(String, Vec<String>, Vec<String>)> = HashSet::new();
        for (form, reasons, conds, deinflected) in forms {
            let mut cond_key: Vec<String> = conds.iter().cloned().collect();
            cond_key.sort();
            if !seen_forms.insert((form.clone(), reasons.clone(), cond_key)) {
                continue;
            }
            if !cache.contains_key(&form) {
                cache.insert(form.clone(), query_form(conn, language, &form)?);
            }
            for row in &cache[&form] {
                if deinflected && !row.rules.iter().any(|r| conds.contains(r)) {
                    continue;
                }
                let key = (row.result.term.as_str(), row.result.reading.as_str());
                match found.iter_mut().find(|f| (f.lemma.as_str(), f.reading.as_str()) == key) {
                    Some(f) => {
                        if !f.entries.iter().any(|e| e.dict_id == row.result.dict_id && e.glossary == row.result.glossary) {
                            f.entries.push(row.result.clone());
                        }
                    }
                    None => found.push(ScanResult {
                        matched: prefix.to_string(),
                        lemma: row.result.term.clone(),
                        reading: row.result.reading.clone(),
                        reasons: reasons.clone(),
                        entries: vec![row.result.clone()],
                        frequency: None,
                    }),
                }
            }
        }
        if found.is_empty() {
            continue;
        }
        for mut f in found {
            f.frequency = frequency(conn, language, &f.lemma, &f.reading)?;
            let first_dict = f.entries.iter().map(|e| e.dict_id).min().unwrap_or(0);
            out.push((f, first_dict));
        }
        hits += 1;
        if hits >= 3 {
            break;
        }
    }
    out.sort_by(|(a, da), (b, db)| {
        b.matched
            .chars()
            .count()
            .cmp(&a.matched.chars().count())
            .then(a.frequency.unwrap_or(i64::MAX).cmp(&b.frequency.unwrap_or(i64::MAX)))
            .then(da.cmp(db))
    });
    Ok(out.into_iter().map(|(r, _)| r).collect())
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

    // ---- Japanese: a hand-made fixture dictionary (own wording, no JMdict data) --------------------------

    fn ja_dict() -> Vec<u8> {
        make_zip(&[
            ("index.json", r#"{"title":"Fixture JA","format":3,"revision":"t"}"#),
            (
                "term_bank_1.json",
                r#"[
                ["食べる","たべる","v1",   "v1",    10,["to eat"],1,""],
                ["書く","かく","v5",       "v5k",   10,["to write"],2,""],
                ["帰る","かえる","v5",     "v5r",   10,["to return home"],3,""],
                ["見る","みる","v1",       "v1",    10,["to see"],4,""],
                ["勉強","べんきょう","n",  "vs",    10,["study"],5,""],
                ["高い","たかい","adj",    "adj-i", 10,["tall","expensive"],6,""],
                ["良い","よい","adj",      "adj-i", 10,["good"],7,""],
                ["いい","いい","adj",      "adj-i", 9,["good (colloquial)"],8,""],
                ["行く","いく","v5",       "v5k-s", 10,["to go"],9,""],
                ["来る","くる","vk",       "vk",    10,["to come"],10,""],
                ["読む","よむ","v5",       "v5m",   10,["to read"],11,""],
                ["する","する","vs",       "vs-i",  10,["to do"],12,""],
                ["猫","ねこ","n",         "",      10,["cat"],13,""],
                ["ビール","びーる","n",    "",      10,["beer"],14,""],
                ["橋","はし","n",          "",      10,["bridge"],15,""],
                ["箸","はし","n",          "",      10,["chopsticks"],16,""],
                ["食","しょく","n",       "",      10,["food; eating"],17,""]
                ]"#,
            ),
            (
                "term_meta_bank_1.json",
                r#"[["橋","freq",{"reading":"はし","frequency":{"value":900,"displayValue":"900"}}],
                    ["箸","freq",{"reading":"はし","frequency":4000}],
                    ["猫","freq",1200],
                    ["橋","pitch",{"reading":"はし","pitches":[{"position":2}]}],
                    ["箸","pitch",{"reading":"はし","pitches":[{"position":1,"nasal":[1],"devoice":2}]}],
                    ["猫","ipa",{"reading":"ねこ","transcriptions":[]}]]"#,
            ),
        ])
    }

    fn rules() -> Deinflector {
        Deinflector::from_json(include_str!("../../../extensions/lang-ja/deinflect.json")).unwrap()
    }

    fn scan(conn: &Connection, text: &str) -> Vec<ScanResult> {
        lookup_scan(conn, "ja", Some(&rules()), text, 20).unwrap()
    }

    fn ja_conn() -> Connection {
        let mut conn = alst_core::db::open_in_memory().unwrap();
        let info = import(&mut conn, &ja_dict(), "ja", 1).unwrap();
        assert!(info.rules_imported);
        assert_eq!((info.entry_count, info.meta_count), (17, 5), "ipa rows are skipped");
        conn
    }

    #[test]
    fn rules_are_normalised_and_meta_imported() {
        let conn = ja_conn();
        let rules: String = conn.query_row("SELECT rules FROM local_dict_entries WHERE term = '行く'", [], |r| r.get(0)).unwrap();
        assert_eq!(rules, "v5");
        let rules: String = conn.query_row("SELECT rules FROM local_dict_entries WHERE term = 'する'", [], |r| r.get(0)).unwrap();
        assert_eq!(rules, "vs");
        let f = meta(&conn, "ja", "橋", Some("はし"), "freq").unwrap();
        assert_eq!(f[0].data["value"], 900);
        assert_eq!(f[0].data["display"], "900");
        assert_eq!(meta(&conn, "ja", "猫", None, "freq").unwrap()[0].data["value"], 1200);
        let p = meta(&conn, "ja", "箸", None, "pitch").unwrap();
        assert_eq!(p[0].data["pitches"][0]["position"], 1);
        assert_eq!(p[0].data["pitches"][0]["devoice"], 2);
        assert!(meta(&conn, "ja", "橋", Some("きょう"), "pitch").unwrap().is_empty(), "reading filter");
        assert!(meta(&conn, "es", "橋", None, "pitch").unwrap().is_empty(), "language filter");
        assert_eq!(list(&conn).unwrap()[0].meta_count, 5);
    }

    #[test]
    fn scan_finds_conjugated_forms_with_reasons() {
        let conn = ja_conn();
        let r = scan(&conn, "食べさせられたので");
        assert_eq!(r[0].matched, "食べさせられた");
        assert_eq!(r[0].lemma, "食べる");
        assert_eq!(r[0].reading, "たべる");
        assert_eq!(r[0].reasons, ["causative", "passive", "past"]);
        assert_eq!(r[0].entries[0].glossary, ["to eat"]);
        // a shorter prefix with its own result follows the longest
        assert!(r.iter().any(|x| x.matched == "食" && x.lemma == "食" && x.reasons.is_empty()));
        let r = scan(&conn, "読まされていた。");
        assert_eq!((r[0].lemma.as_str(), r[0].reasons.join(", ")), ("読む", "causative (short), passive, te, progressive, past".to_string()));
        let r = scan(&conn, "高くなかった");
        assert_eq!(r[0].lemma, "高い");
        let r = scan(&conn, "よかった");
        assert!(r.iter().any(|x| x.lemma == "良い") && r.iter().any(|x| x.lemma == "いい"));
        let r = scan(&conn, "来なかった");
        assert_eq!((r[0].lemma.as_str(), r[0].reasons.join(", ")), ("来る", "negative, past".to_string()));
        let r = scan(&conn, "行って");
        assert_eq!((r[0].lemma.as_str(), r[0].reasons.join(", ")), ("行く", "te".to_string()));
    }

    #[test]
    fn scan_validates_against_entry_rules() {
        let conn = ja_conn();
        // 帰れば: the ichidan reading (帰る v1) is rejected because the entry is godan only
        let r = scan(&conn, "帰れば");
        assert_eq!(r[0].lemma, "帰る");
        assert_eq!(r[0].reasons, ["ba-conditional"]);
        assert_eq!(r.iter().filter(|x| x.matched == "帰れば").count(), 1, "one result for the span: only the v5 reading validates");
        // 見る is ichidan: 見た finds it, and nothing finds a bare 見 headword that doesn't exist
        let r = scan(&conn, "見た");
        assert_eq!((r[0].lemma.as_str(), r[0].reasons.join(", ")), ("見る", "past".to_string()));
        assert!(r.iter().all(|x| x.lemma != "見"));
        // godan 書く does not answer to an ichidan-style rewrite (書きる) and 書いた finds it
        assert_eq!(scan(&conn, "書いた")[0].lemma, "書く");
        // a noun matches する-compounds only when tagged vs
        let r = scan(&conn, "勉強した");
        assert_eq!((r[0].matched.as_str(), r[0].lemma.as_str(), r[0].reasons.join(", ")), ("勉強した", "勉強", "past".to_string()));
        assert!(scan(&conn, "猫した").iter().all(|x| x.matched != "猫した"), "猫 is not tagged vs");
        // する itself
        assert_eq!(scan(&conn, "しなかった")[0].lemma, "する");
    }

    #[test]
    fn scan_normalises_kana_and_orders_by_length_then_frequency() {
        let conn = ja_conn();
        assert_eq!(scan(&conn, "ビール")[0].lemma, "ビール");
        assert_eq!(scan(&conn, "びーる")[0].lemma, "ビール", "hiragana query finds a katakana headword by reading");
        assert_eq!(scan(&conn, "ネコ")[0].lemma, "猫", "katakana query finds by hiragana reading");
        let r = scan(&conn, "はし");
        assert_eq!(r.iter().map(|x| x.lemma.as_str()).collect::<Vec<_>>(), ["橋", "箸"], "more frequent first");
        assert_eq!(r[0].frequency, Some(900));
    }

    #[test]
    fn scan_stops_after_longest_plus_two_shorter_and_respects_max_len() {
        let conn = ja_conn();
        let r = scan(&conn, "食べる");
        let lens: HashSet<usize> = r.iter().map(|x| x.matched.chars().count()).collect();
        assert_eq!(lens, HashSet::from([3, 1]), "食べる, then 食");
        assert!(lookup_scan(&conn, "ja", Some(&rules()), "食べさせられた", 2).unwrap().iter().all(|x| x.matched.chars().count() <= 2));
        assert!(scan(&conn, "   ").is_empty());
    }

    #[test]
    fn scan_without_deinflector_is_a_plain_lookup_and_old_dictionaries_lack_rules() {
        let conn = ja_conn();
        let r = lookup_scan(&conn, "ja", None, "食べた", 20).unwrap();
        assert!(r.iter().all(|x| x.matched != "食べた"));
        assert_eq!(r[0].matched, "食");
        // a dictionary imported before migration 0003: no rules, flagged for re-import
        conn.execute("UPDATE local_dict_entries SET rules = ''", []).unwrap();
        conn.execute("UPDATE local_dict_dicts SET rules_imported = 0", []).unwrap();
        assert!(!list(&conn).unwrap()[0].rules_imported);
        assert!(scan(&conn, "食べた").iter().all(|x| x.matched != "食べた"));
        assert_eq!(scan(&conn, "食べる")[0].lemma, "食べる", "undeflected forms still match");
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
