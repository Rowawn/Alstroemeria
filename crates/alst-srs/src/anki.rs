//! Anki package import and export (SPEC §11.8), written from the public description of the legacy collection
//! layout (`collection.anki21` / `collection.anki2`: SQLite tables `col`, `notes`, `cards`, `revlog`, with
//! note types, decks and deck options as JSON in `col`) and from inspecting exported files. Not derived from
//! Anki or AnkiDroid code.

use crate::concept;
use crate::fsrs::{self, CardState, Queue};
use crate::pool::Segmenter;
use crate::render;
use crate::replay;
use crate::sched;
use crate::{Error, Result};
use alst_core::media::MediaDir;
use alst_core::srs::{CardEvent, NewEvent, classic_card_id};
use alst_core::store::derive_id;
use alst_core::Store;
use rusqlite::{Connection, OpenFlags, params, types::Value};
use serde::Serialize;
use serde_json::{Value as J, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const NEW_LAYOUT_MESSAGE: &str = "Export again from Anki with 'Support older Anki versions' ticked.";
const MAX_MEDIA_FILE: u64 = 300 * 1024 * 1024;
const MAX_COLLECTION: u64 = 4 * 1024 * 1024 * 1024;

pub struct ImportOptions<'a> {
    pub media: &'a MediaDir,
    /// Scratch directory for the extracted collection.
    pub tmp_dir: &'a Path,
    pub now_ms: i64,
    /// Local offset east of UTC, used for the day boundary when imported reviews are replayed.
    pub tz_offset_min: i64,
    /// Language to put on the imported decks (the app's decks carry one; Anki's do not).
    pub language: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub decks: usize,
    pub note_types: usize,
    pub notes_new: usize,
    pub notes_updated: usize,
    pub cards: usize,
    pub reviews: usize,
    pub states: usize,
    pub media: usize,
}

fn t(s: &str) -> Value {
    Value::Text(s.to_string())
}

fn i(n: i64) -> Value {
    Value::Integer(n)
}

fn bad(msg: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("Anki package: {msg}"))
}

fn read_entry(z: &mut zip::ZipArchive<std::fs::File>, name: &str, max: u64) -> Result<Vec<u8>> {
    let e = z.by_name(name).map_err(|_| bad(format!("missing {name}")))?;
    if e.size() > max {
        return Err(bad(format!("{name} is too large")));
    }
    let mut buf = Vec::with_capacity(e.size() as usize);
    e.take(max + 1).read_to_end(&mut buf).map_err(bad)?;
    if buf.len() as u64 > max {
        return Err(bad(format!("{name} is too large")));
    }
    Ok(buf)
}

fn mime_for_file(name: &str) -> String {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" => "audio/ogg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn step_string(minutes: f64) -> String {
    if minutes >= 1440.0 && (minutes / 1440.0).fract() == 0.0 {
        format!("{}d", (minutes / 1440.0) as i64)
    } else if minutes >= 60.0 && (minutes / 60.0).fract() == 0.0 {
        format!("{}h", (minutes / 60.0) as i64)
    } else if minutes.fract() == 0.0 {
        format!("{}m", minutes as i64)
    } else {
        format!("{}s", (minutes * 60.0).round() as i64)
    }
}

fn steps_json(v: Option<&J>, default: &[&str]) -> String {
    let steps: Vec<String> = match v.and_then(J::as_array) {
        Some(a) => a.iter().filter_map(J::as_f64).map(step_string).collect(),
        None => default.iter().map(|s| s.to_string()).collect(),
    };
    serde_json::to_string(&steps).unwrap()
}

struct Parsed {
    models: J,
    decks: J,
    dconf: J,
    crt: i64,
    rollover: i64,
}

fn parse_col(conn: &Connection) -> Result<Parsed> {
    let (crt, conf, models, decks, dconf): (i64, String, String, String, String) =
        conn.query_row("SELECT crt, conf, models, decks, dconf FROM col LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
            .map_err(|e| bad(format!("no collection row ({e})")))?;
    let j = |s: &str, what: &str| serde_json::from_str::<J>(s).map_err(|_| bad(format!("{what} is not JSON; this looks like the newer layout")));
    let conf = j(&conf, "col.conf").unwrap_or(J::Null);
    Ok(Parsed { models: j(&models, "note types")?, decks: j(&decks, "decks")?, dconf: j(&dconf, "deck options")?, crt, rollover: conf["rollover"].as_i64().unwrap_or(4) })
}

/// Import a `.apkg` or `.colpkg` (legacy layout only).
pub fn import_apkg(store: &mut Store, path: &Path, opts: &ImportOptions) -> Result<ImportReport> {
    let file = std::fs::File::open(path).map_err(bad)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| bad(format!("not a zip file ({e})")))?;
    let names: HashSet<String> = zip.file_names().map(String::from).collect();
    let entry = if names.contains("collection.anki21") {
        "collection.anki21"
    } else if names.contains("collection.anki2") {
        "collection.anki2"
    } else if names.contains("collection.anki21b") {
        return Err(Error::Invalid(NEW_LAYOUT_MESSAGE.into()));
    } else {
        return Err(bad("no collection found"));
    };
    std::fs::create_dir_all(opts.tmp_dir).map_err(bad)?;
    let db_path: PathBuf = opts.tmp_dir.join(format!("import-{}.sqlite", alst_core::store::new_id()));
    std::fs::write(&db_path, read_entry(&mut zip, entry, MAX_COLLECTION)?).map_err(bad)?;
    let result = (|| {
        let conn = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(bad)?;
        import_collection(store, &conn, &mut zip, opts)
    })();
    for ext in ["", "-wal", "-shm", "-journal"] {
        let mut p = db_path.clone().into_os_string();
        p.push(ext);
        let _ = std::fs::remove_file(p);
    }
    result
}

struct CardRow {
    id: i64,
    nid: i64,
    did: i64,
    ord: i64,
    ty: i64,
    queue: i64,
    due: i64,
    ivl: i64,
    reps: i64,
    lapses: i64,
    odid: i64,
    modified: i64,
    data: String,
}

fn import_collection(store: &mut Store, conn: &Connection, zip: &mut zip::ZipArchive<std::fs::File>, opts: &ImportOptions) -> Result<ImportReport> {
    let col = parse_col(conn)?;
    let mut rep = ImportReport::default();
    let lang = opts.language.clone();

    // Deck options.
    let mut config_ids: HashMap<String, String> = HashMap::new();
    let mut rows: Vec<alst_core::srs::SrsRow<'_>> = vec![];
    if let Some(map) = col.dconf.as_object() {
        for (cid, c) in map {
            let name = c["name"].as_str().unwrap_or("Imported").to_string();
            let hash = derive_id("deck_configs", &["hash", &c.to_string()]);
            let id = derive_id("deck_configs", &["anki", &name, &hash]);
            let params: Option<Vec<f64>> = ["fsrsParams6", "fsrsParams5", "fsrsWeights"].iter().find_map(|k| c[*k].as_array()).map(|a| a.iter().filter_map(J::as_f64).collect());
            let params = params.filter(|p| p.len() == 19 || p.len() == 21).map(|p| serde_json::to_string(&fsrs::normalize_params(&p).to_vec()).unwrap());
            rows.push((
                "deck_configs",
                id.clone(),
                vec![
                    ("name", t(&name)),
                    ("new_per_day", i(c["new"]["perDay"].as_i64().unwrap_or(20))),
                    ("reviews_per_day", i(c["rev"]["perDay"].as_i64().unwrap_or(200))),
                    ("desired_retention", Value::Real(c["desiredRetention"].as_f64().unwrap_or(0.9).clamp(0.7, 0.99))),
                    ("learning_steps", t(&steps_json(c["new"].get("delays"), &["1m", "10m"]))),
                    ("relearning_steps", t(&steps_json(c["lapse"].get("delays"), &["10m"]))),
                    ("max_interval_days", i(c["rev"]["maxIvl"].as_i64().unwrap_or(36500).max(1))),
                    ("fsrs_params", params.map(Value::Text).unwrap_or(Value::Null)),
                    ("bury_siblings", i((c["new"]["bury"].as_bool().unwrap_or(true) || c["rev"]["bury"].as_bool().unwrap_or(true)) as i64)),
                    ("day_starts_hour", i(col.rollover.clamp(0, 23))),
                ],
            ));
            config_ids.insert(cid.clone(), id);
        }
    }
    let default_config = config_ids.values().next().cloned();

    // Decks (filtered decks hold other decks' cards temporarily; they are not imported).
    let mut deck_ids: HashMap<i64, String> = HashMap::new();
    let used_decks: HashSet<i64> = {
        let mut stmt = conn.prepare("SELECT did, odid FROM cards").map_err(bad)?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))).map_err(bad)?;
        rows.filter_map(|r| r.ok()).map(|(d, o)| if o != 0 { o } else { d }).collect()
    };
    if let Some(map) = col.decks.as_object() {
        for (k, d) in map {
            let did: i64 = d["id"].as_i64().or_else(|| k.parse().ok()).unwrap_or(0);
            if d["dyn"].as_i64().unwrap_or(0) == 1 {
                continue;
            }
            let name = d["name"].as_str().unwrap_or("Imported").to_string();
            if did == 1 && !used_decks.contains(&1) {
                continue;
            }
            let id = derive_id("decks", &["anki", &name]);
            let conf = config_ids.get(&d["conf"].as_i64().map(|c| c.to_string()).unwrap_or_default()).cloned().or_else(|| default_config.clone());
            rows.push((
                "decks",
                id.clone(),
                vec![
                    ("name", t(&name)),
                    ("kind", t("classic")),
                    ("language", lang.as_deref().map(t).unwrap_or(Value::Null)),
                    ("config_id", conf.as_deref().map(t).unwrap_or(Value::Null)),
                    ("anki_id", i(did)),
                    ("description", t(d["desc"].as_str().unwrap_or(""))),
                ],
            ));
            deck_ids.insert(did, id);
            rep.decks += 1;
        }
    }

    // Note types.
    let mut model_ids: HashMap<i64, String> = HashMap::new();
    if let Some(map) = col.models.as_object() {
        for (k, m) in map {
            let mid: i64 = m["id"].as_str().and_then(|s| s.parse().ok()).or_else(|| m["id"].as_i64()).or_else(|| k.parse().ok()).unwrap_or(0);
            let sorted = |key: &str| -> Vec<J> {
                let mut v: Vec<J> = m[key].as_array().cloned().unwrap_or_default();
                v.sort_by_key(|x| x["ord"].as_i64().unwrap_or(0));
                v
            };
            let fields: Vec<&str> = Vec::new();
            let _ = fields;
            let field_names: Vec<String> = sorted("flds").iter().map(|f| f["name"].as_str().unwrap_or("").to_string()).collect();
            let templates: Vec<J> = sorted("tmpls").iter().map(|x| json!({"name": x["name"], "qfmt": x["qfmt"], "afmt": x["afmt"]})).collect();
            let id = derive_id("note_types", &["anki", &mid.to_string()]);
            rows.push((
                "note_types",
                id.clone(),
                vec![
                    ("name", t(m["name"].as_str().unwrap_or("Imported"))),
                    ("kind", t(if m["type"].as_i64() == Some(1) { "cloze" } else { "standard" })),
                    ("fields", t(&serde_json::to_string(&field_names).unwrap())),
                    ("templates", t(&serde_json::to_string(&templates).unwrap())),
                    ("css", t(m["css"].as_str().unwrap_or(""))),
                    ("anki_id", i(mid)),
                ],
            ));
            model_ids.insert(mid, id);
            rep.note_types += 1;
        }
    }
    store.srs_put_many(&rows)?;

    // Cards first (a note's deck is its first card's deck).
    let cards: Vec<CardRow> = {
        let mut stmt = conn
            .prepare("SELECT id, nid, did, ord, type, queue, due, ivl, reps, lapses, odid, mod, data FROM cards ORDER BY id")
            .map_err(bad)?;
        stmt.query_map([], |r| {
            Ok(CardRow { id: r.get(0)?, nid: r.get(1)?, did: r.get(2)?, ord: r.get(3)?, ty: r.get(4)?, queue: r.get(5)?, due: r.get(6)?, ivl: r.get(7)?, reps: r.get(8)?, lapses: r.get(9)?, odid: r.get(10)?, modified: r.get(11)?, data: r.get::<_, Option<String>>(12)?.unwrap_or_default() })
        })
        .map_err(bad)?
        .collect::<std::result::Result<_, _>>()
        .map_err(bad)?
    };
    let deck_of = |c: &CardRow| deck_ids.get(&if c.odid != 0 { c.odid } else { c.did }).cloned();
    let mut note_deck: HashMap<i64, String> = HashMap::new();
    for c in &cards {
        if let Some(d) = deck_of(c) {
            note_deck.entry(c.nid).or_insert(d);
        }
    }

    // Notes.
    let mut note_ids: HashMap<i64, String> = HashMap::new();
    let mut rows: Vec<alst_core::srs::SrsRow<'_>> = vec![];
    {
        let mut stmt = conn.prepare("SELECT id, guid, mid, tags, flds FROM notes ORDER BY id").map_err(bad)?;
        let notes = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?)))
            .map_err(bad)?;
        for n in notes {
            let (nid, guid, mid, tags, flds) = n.map_err(bad)?;
            let (Some(deck), Some(nt)) = (note_deck.get(&nid), model_ids.get(&mid)) else { continue };
            let key = if guid.is_empty() { format!("id{nid}") } else { guid.clone() };
            let id = derive_id("notes", &["anki", &key]);
            let existing: i64 = store.read_conn().query_row("SELECT count(*) FROM notes WHERE id = ?1 AND deleted = 0", [&id], |r| r.get(0)).unwrap_or(0);
            if existing > 0 {
                rep.notes_updated += 1;
            } else {
                rep.notes_new += 1;
            }
            let fields: Vec<&str> = flds.split('\u{1f}').collect();
            let tags: Vec<&str> = tags.split_whitespace().collect();
            rows.push((
                "notes",
                id.clone(),
                vec![
                    ("note_type_id", t(nt)),
                    ("deck_id", t(deck)),
                    ("fields", t(&serde_json::to_string(&fields).unwrap())),
                    ("tags", t(&serde_json::to_string(&tags).unwrap())),
                    ("guid", t(&guid)),
                    ("anki_id", i(nid)),
                ],
            ));
            note_ids.insert(nid, id);
        }
    }
    store.srs_put_many(&rows)?;

    let mut rows: Vec<alst_core::srs::SrsRow<'_>> = vec![];
    let mut card_ids: HashMap<i64, (String, String)> = HashMap::new();
    for c in &cards {
        let (Some(note), Some(deck)) = (note_ids.get(&c.nid), deck_of(c)) else { continue };
        let id = classic_card_id(note, c.ord);
        rows.push((
            "cards",
            id.clone(),
            vec![("note_id", t(note)), ("deck_id", t(&deck)), ("ord", i(c.ord)), ("suspended", i((c.queue == -1) as i64)), ("anki_id", i(c.id))],
        ));
        card_ids.insert(c.id, (id, deck));
        rep.cards += 1;
    }
    store.srs_put_many(&rows)?;

    // Media.
    let media_map: BTreeMap<String, String> = {
        let raw = read_entry(zip, "media", 64 * 1024 * 1024).unwrap_or_default();
        serde_json::from_slice::<BTreeMap<String, String>>(&raw).unwrap_or_default()
    };
    let mut rows: Vec<alst_core::srs::SrsRow<'_>> = vec![];
    for (num, name) in &media_map {
        if name.contains('/') || name.contains('\\') || name.contains("..") || name.is_empty() {
            continue;
        }
        let Ok(bytes) = read_entry(zip, num, MAX_MEDIA_FILE) else { continue };
        let (hash, size) = opts.media.put_bytes(&bytes, MAX_MEDIA_FILE)?;
        store.register_media(&hash, &mime_for_file(name), size as i64, Some(name))?;
        rows.push(("note_media", derive_id("note_media", &[name]), vec![("name", t(name)), ("media_hash", t(&hash))]));
        rep.media += 1;
    }
    store.srs_put_many(&rows)?;

    // Review history and state.
    let mut revlog: HashMap<i64, Vec<(i64, i64, i64, i64)>> = HashMap::new(); // cid -> (id, ease, time, type)
    {
        let mut stmt = conn.prepare("SELECT id, cid, ease, time, type FROM revlog ORDER BY id").map_err(bad)?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?))).map_err(bad)?;
        for r in rows {
            let (id, cid, ease, time, ty) = r.map_err(bad)?;
            if (1..=4).contains(&ease) && ty != 4 {
                revlog.entry(cid).or_default().push((id, ease, time, ty));
            }
        }
    }
    let mut events: Vec<NewEvent> = vec![];
    let crt_ms = col.crt * 1000;
    for c in &cards {
        let Some((card_id, deck_id)) = card_ids.get(&c.id) else { continue };
        let hist = revlog.get(&c.id).cloned().unwrap_or_default();
        let mut mine: Vec<NewEvent> = vec![];
        for (rid, ease, time, ty) in &hist {
            mine.push(NewEvent {
                id: derive_id("events", &["anki-review", card_id, &rid.to_string()]),
                ty: "review".into(),
                language: lang.clone(),
                subject_type: "card".into(),
                subject_id: card_id.clone(),
                payload: json!({"card_id": card_id, "rating": ease, "duration_ms": time, "at": rid, "tz": opts.tz_offset_min, "policy": "card",
                                 "before": {"queue": match ty { 1 => "review", 2 => "relearning", _ => "learning" }}, "import": true}),
            });
        }
        let last_at = hist.last().map(|h| h.0);
        let data: J = serde_json::from_str(&c.data).unwrap_or(J::Null);
        let (s_data, d_data) = (data["s"].as_f64().filter(|s| *s > 0.0), data["d"].as_f64());
        let queue = match c.ty {
            1 => Queue::Learning,
            2 => Queue::Review,
            3 => Queue::Relearning,
            _ => Queue::New,
        };
        // The schedule Anki had: review cards are due on a day counted from the collection's creation.
        let due_ms = match (queue, c.queue) {
            (Queue::Review, _) | (Queue::Relearning, 3) => crt_ms + c.due * 86_400_000,
            (Queue::Learning, 1) | (Queue::Relearning, 1) => c.due * 1000,
            _ => crt_ms + c.due * 86_400_000,
        };
        let at = last_at.map(|a| a + 1).unwrap_or_else(|| if c.modified > 0 { (c.modified * 1000).min(opts.now_ms) } else { c.id.min(opts.now_ms) });
        let snapshot = if queue == Queue::New {
            (!hist.is_empty()).then(|| CardState { reps: c.reps, lapses: c.lapses, ..CardState::new() })
        } else {
            // Memory state: from `data`, else by replaying the imported history, else from the interval.
            let cfg_id: Option<String> = store.read_conn().query_row("SELECT config_id FROM decks WHERE id = ?1", [deck_id], |r| r.get(0)).ok().flatten();
            let cfg = sched::load_config(store.read_conn(), cfg_id.as_deref())?;
            let folded = (!hist.is_empty() && s_data.is_none()).then(|| {
                let evs: Vec<CardEvent> = mine.iter().map(|e| CardEvent { id: e.id.clone(), hlc: 0, site: String::new(), ty: e.ty.clone(), payload: e.payload.clone() }).collect();
                replay::fold(card_id, evs, sched::policy_for("classic"), &cfg)
            });
            let (s, d) = match (s_data, d_data, &folded) {
                (Some(s), d, _) => (s, d.unwrap_or(5.0).clamp(1.0, 10.0)),
                (None, _, Some(f)) if f.stability.is_some() => (f.stability.unwrap(), f.difficulty.unwrap_or(5.0)),
                _ => ((c.ivl.max(1)) as f64, 5.0),
            };
            Some(CardState {
                queue,
                due: due_ms,
                stability: Some(s),
                difficulty: Some(d),
                step: 0,
                reps: c.reps,
                lapses: c.lapses,
                last_review: last_at.or(Some(due_ms - c.ivl.max(0) * 86_400_000)),
            })
        };
        if let Some(st) = snapshot {
            rep.states += 1;
            let payload = json!({"at": at, "queue": st.queue.as_str(), "due": st.due, "stability": st.stability, "difficulty": st.difficulty,
                                 "step": st.step, "reps": st.reps, "lapses": st.lapses, "last_review": st.last_review, "import": true});
            mine.push(NewEvent {
                id: derive_id("events", &["anki-state", card_id, &at.to_string(), &st.due.to_string(), &format!("{:?}", st.stability)]),
                ty: "review.state".into(),
                language: lang.clone(),
                subject_type: "card".into(),
                subject_id: card_id.clone(),
                payload,
            });
        }
        rep.reviews += hist.len();
        events.extend(mine);
    }
    for chunk in events.chunks(2000) {
        store.emit_events_batch(chunk)?;
    }
    let ids: Vec<String> = card_ids.values().map(|(c, _)| c.clone()).collect();
    store.replay_cards(&ids)?;
    Ok(rep)
}

// ---- export ------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExportReport {
    pub decks: usize,
    pub notes: usize,
    pub cards: usize,
    pub reviews: usize,
    pub media: usize,
    /// Concept cards written as plain notes (lossy).
    pub concept_cards: usize,
}

const SCHEMA: &str = "
CREATE TABLE col (id integer primary key, crt integer not null, mod integer not null, scm integer not null, ver integer not null,
  dty integer not null, usn integer not null, ls integer not null, conf text not null, models text not null, decks text not null,
  dconf text not null, tags text not null);
CREATE TABLE notes (id integer primary key, guid text not null, mid integer not null, mod integer not null, usn integer not null,
  tags text not null, flds text not null, sfld integer not null, csum integer not null, flags integer not null, data text not null);
CREATE TABLE cards (id integer primary key, nid integer not null, did integer not null, ord integer not null, mod integer not null,
  usn integer not null, type integer not null, queue integer not null, due integer not null, ivl integer not null, factor integer not null,
  reps integer not null, lapses integer not null, left integer not null, odue integer not null, odid integer not null, flags integer not null,
  data text not null);
CREATE TABLE revlog (id integer primary key, cid integer not null, usn integer not null, ease integer not null, ivl integer not null,
  lastIvl integer not null, factor integer not null, time integer not null, type integer not null);
CREATE TABLE graves (usn integer not null, oid integer not null, type integer not null);
CREATE INDEX ix_notes_usn on notes (usn); CREATE INDEX ix_cards_usn on cards (usn); CREATE INDEX ix_revlog_usn on revlog (usn);
CREATE INDEX ix_cards_nid on cards (nid); CREATE INDEX ix_cards_sched on cards (did, queue, due); CREATE INDEX ix_revlog_cid on revlog (cid);
CREATE INDEX ix_notes_csum on notes (csum);
";

/// Numeric ids for the exported rows: the Anki id the row came from, else a fresh increasing millisecond-like id.
struct Ids {
    used: HashSet<i64>,
    next: i64,
}

impl Ids {
    fn take(&mut self, want: Option<i64>) -> i64 {
        if let Some(w) = want.filter(|w| *w > 0 && !self.used.contains(w)) {
            self.used.insert(w);
            return w;
        }
        while self.used.contains(&self.next) {
            self.next += 1;
        }
        self.used.insert(self.next);
        self.next
    }
}

fn field_checksum(s: &str) -> i64 {
    // The first field's text, hashed to a 32-bit number; Anki recomputes it, it only has to be a number.
    let mut h: u32 = 2166136261;
    for b in render_text(s).bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    h as i64
}

fn render_text(s: &str) -> String {
    crate::template::strip_html(s)
}

/// Media file names a note's fields refer to: `[sound:x]`, `src="x"` and `src='x'`.
fn media_refs(fields: &[String]) -> Vec<String> {
    let mut out = vec![];
    for f in fields {
        let mut rest = f.as_str();
        while let Some(p) = rest.find("[sound:") {
            if let Some(e) = rest[p..].find(']') {
                out.push(rest[p + 7..p + e].to_string());
                rest = &rest[p + e + 1..];
            } else {
                break;
            }
        }
        for q in ["src=\"", "src='"] {
            let mut rest = f.as_str();
            let close = &q[q.len() - 1..];
            while let Some(p) = rest.find(q) {
                let after = &rest[p + q.len()..];
                if let Some(e) = after.find(close) {
                    out.push(after[..e].to_string());
                    rest = &after[e..];
                } else {
                    break;
                }
            }
        }
    }
    out.retain(|n| !n.starts_with("alstclip:") && !n.contains('/') && !n.is_empty());
    out.sort();
    out.dedup();
    out
}

/// Export decks (and their sub-decks) as a legacy-layout `.apkg`. Concept decks become classic notes, one per concept
/// and direction, with the pool's best sentence in the fields; the dialog tells the user this is lossy.
pub fn export_apkg(store: &Store, deck_ids: &[String], out: &Path, media: &MediaDir, seg: &dyn Segmenter, now_ms: i64, tz_offset_min: i64) -> Result<ExportReport> {
    let conn = store.read_conn();
    let all = sched::decks(conn)?;
    let wanted: Vec<&sched::DeckInfo> = all
        .iter()
        .filter(|d| deck_ids.iter().any(|id| all.iter().any(|r| &r.id == id && (r.name == d.name || d.name.starts_with(&format!("{}::", r.name))))))
        .collect();
    if wanted.is_empty() {
        return Err(Error::Invalid("no decks to export".into()));
    }
    let mut rep = ExportReport { decks: wanted.len(), ..Default::default() };
    let mut ids = Ids { used: HashSet::new(), next: now_ms };
    let day_hour = 4;
    let clock = sched::Clock { tz_offset_min, day_starts_hour: day_hour };

    // Deck options and decks.
    let mut dconf = serde_json::Map::new();
    dconf.insert("1".into(), json!({"id": 1, "name": "Default", "autoplay": true, "mod": 0, "usn": 0, "maxTaken": 60, "timer": 0, "replayq": true,
        "new": {"bury": true, "delays": [1, 10], "initialFactor": 2500, "ints": [1, 4, 0], "order": 1, "perDay": 20},
        "rev": {"bury": true, "ease4": 1.3, "fuzz": 0.05, "ivlFct": 1, "maxIvl": 36500, "perDay": 200, "hardFactor": 1.2},
        "lapse": {"delays": [10], "leechAction": 0, "leechFails": 8, "minInt": 1, "mult": 0}}));
    let mut decks_json = serde_json::Map::new();
    decks_json.insert("1".into(), json!({"id": 1, "name": "Default", "desc": "", "mod": 0, "usn": 0, "collapsed": false, "browserCollapsed": false, "dyn": 0, "conf": 1,
        "extendNew": 10, "extendRev": 50, "lrnToday": [0, 0], "newToday": [0, 0], "revToday": [0, 0], "timeToday": [0, 0]}));
    let mut deck_num: HashMap<String, i64> = HashMap::new();
    let mut conf_num: HashMap<String, i64> = HashMap::new();
    for d in &wanted {
        let (anki_id, desc): (Option<i64>, String) = conn.query_row("SELECT anki_id, description FROM decks WHERE id = ?1", [&d.id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let num = ids.take(anki_id);
        deck_num.insert(d.id.clone(), num);
        let conf_key = d.config_id.clone().unwrap_or_default();
        let conf = match conf_num.get(&conf_key) {
            Some(c) => *c,
            None => {
                let cfg = sched::load_config(conn, d.config_id.as_deref())?;
                let c = ids.take(None);
                let mins = |v: &[i64]| -> Vec<f64> { v.iter().map(|ms| *ms as f64 / 60_000.0).collect() };
                let name: String = d.config_id.as_deref().and_then(|c| conn.query_row("SELECT name FROM deck_configs WHERE id = ?1", [c], |r| r.get(0)).ok()).unwrap_or_else(|| "Default".into());
                let mut jc = json!({"id": c, "name": name, "autoplay": true, "mod": 0, "usn": 0, "maxTaken": 60, "timer": 0, "replayq": true,
                    "new": {"bury": cfg.bury_siblings, "delays": mins(&cfg.sched.learning_steps), "initialFactor": 2500, "ints": [1, 4, 0], "order": 1, "perDay": cfg.new_per_day},
                    "rev": {"bury": cfg.bury_siblings, "ease4": 1.3, "fuzz": 0.05, "ivlFct": 1, "maxIvl": cfg.sched.max_interval, "perDay": cfg.reviews_per_day, "hardFactor": 1.2},
                    "lapse": {"delays": mins(&cfg.sched.relearning_steps), "leechAction": 0, "leechFails": 8, "minInt": 1, "mult": 0},
                    "desiredRetention": cfg.sched.desired_retention});
                jc["fsrsParams6"] = json!(cfg.sched.params.to_vec());
                dconf.insert(c.to_string(), jc);
                conf_num.insert(conf_key, c);
                c
            }
        };
        decks_json.insert(num.to_string(), json!({"id": num, "name": d.name, "desc": desc, "mod": now_ms / 1000, "usn": -1, "collapsed": false, "browserCollapsed": false,
            "dyn": 0, "conf": conf, "extendNew": 10, "extendRev": 50, "lrnToday": [0, 0], "newToday": [0, 0], "revToday": [0, 0], "timeToday": [0, 0]}));
    }

    // Collect notes, cards and note types.
    struct OutNote {
        id: i64,
        guid: String,
        mid: i64,
        tags: String,
        fields: Vec<String>,
    }
    struct OutCard {
        id: i64,
        nid: i64,
        did: i64,
        ord: i64,
        suspended: bool,
        state: CardState,
        our_id: String,
    }
    let mut models = serde_json::Map::new();
    let mut model_num: HashMap<String, i64> = HashMap::new();
    let mut notes: Vec<OutNote> = vec![];
    let mut out_cards: Vec<OutCard> = vec![];
    let mut note_num: HashMap<String, i64> = HashMap::new();
    let mut concept_mid: Option<i64> = None;
    let first_deck_num = deck_num[&wanted[0].id];

    let mut stmt = conn.prepare(
        "SELECT c.id, c.note_id, c.concept_id, c.deck_id, c.ord, c.direction, c.suspended, c.anki_id, c.queue, c.due, c.stability, c.difficulty,
                c.step, c.reps, c.lapses, c.last_review FROM cards c WHERE c.deleted = 0 ORDER BY c.created_at, c.id",
    )?;
    type Raw = (String, Option<String>, Option<String>, String, i64, Option<String>, i64, Option<i64>, CardState);
    let raws: Vec<Raw> = stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                CardState {
                    queue: Queue::parse(&r.get::<_, String>(8)?),
                    due: r.get(9)?,
                    stability: r.get(10)?,
                    difficulty: r.get(11)?,
                    step: r.get(12)?,
                    reps: r.get(13)?,
                    lapses: r.get(14)?,
                    last_review: r.get(15)?,
                },
            ))
        })?
        .collect::<std::result::Result<_, _>>()?;
    drop(stmt);
    let mut media_names: Vec<String> = vec![];
    for (card_id, note_id, concept_id, deck_id, ord, direction, suspended, anki_id, state) in raws {
        let Some(&did) = deck_num.get(&deck_id) else { continue };
        if let (None, Some(_)) = (&note_id, &concept_id) {
            // Concept card → a classic Front/Back note.
            let mid = match concept_mid {
                Some(m) => m,
                None => {
                    let m = ids.take(None);
                    models.insert(m.to_string(), model_json(m, "Alstroemeria Concept", "standard", &["Front".into(), "Back".into()],
                        &[("Card 1".into(), "{{Front}}".into(), "{{FrontSide}}<hr id=answer>{{Back}}".into())], render::CONCEPT_CSS, first_deck_num, now_ms));
                    concept_mid = Some(m);
                    m
                }
            };
            let cc = concept::concept_card(store, &card_id, seg)?;
            let r = render::render_concept(&cc, "");
            let back = r.back.strip_prefix(&r.front).unwrap_or(&r.back).trim_start_matches("<hr id=answer>").to_string();
            let nid = ids.take(None);
            let fields = vec![r.front.clone(), back];
            media_names.extend(media_refs(&fields));
            notes.push(OutNote { id: nid, guid: derive_id("anki-guid", &[&card_id])[..10].to_string(), mid, tags: String::new(), fields });
            out_cards.push(OutCard { id: ids.take(None), nid, did, ord: 0, suspended: suspended != 0, state, our_id: card_id });
            rep.concept_cards += 1;
            continue;
        }
        let Some(note_id) = note_id else { continue };
        let nid = match note_num.get(&note_id) {
            Some(n) => *n,
            None => {
                let (nt, fields, tags, guid, nanki): (String, String, String, Option<String>, Option<i64>) =
                    conn.query_row("SELECT note_type_id, fields, tags, guid, anki_id FROM notes WHERE id = ?1 AND deleted = 0", [&note_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?;
                let mid = match model_num.get(&nt) {
                    Some(m) => *m,
                    None => {
                        let (name, kind, fnames, tmpls, css, manki): (String, String, String, String, String, Option<i64>) =
                            conn.query_row("SELECT name, kind, fields, templates, css, anki_id FROM note_types WHERE id = ?1", [&nt], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?;
                        let m = ids.take(manki);
                        let fnames: Vec<String> = serde_json::from_str(&fnames).unwrap_or_default();
                        let tl: Vec<render::TemplateDef> = serde_json::from_str(&tmpls).unwrap_or_default();
                        let tl: Vec<(String, String, String)> = tl.into_iter().map(|t| (t.name, t.qfmt, t.afmt)).collect();
                        models.insert(m.to_string(), model_json(m, &name, &kind, &fnames, &tl, &css, did, now_ms));
                        model_num.insert(nt.clone(), m);
                        m
                    }
                };
                let fields: Vec<String> = serde_json::from_str(&fields).unwrap_or_default();
                let tags: Vec<String> = serde_json::from_str(&tags).unwrap_or_default();
                media_names.extend(media_refs(&fields));
                let n = ids.take(nanki);
                let guid = guid.filter(|g| !g.is_empty()).unwrap_or_else(|| derive_id("anki-guid", &[&note_id])[..10].to_string());
                notes.push(OutNote { id: n, guid, mid, tags: tags.join(" "), fields });
                note_num.insert(note_id.clone(), n);
                n
            }
        };
        out_cards.push(OutCard { id: ids.take(anki_id), nid, did, ord, suspended: suspended != 0, state, our_id: card_id });
        let _ = (direction, concept_id);
    }
    if out_cards.is_empty() {
        return Err(Error::Invalid("the selected decks have no cards".into()));
    }
    // Review cards are due on a day counted from the collection's creation: start it before the earliest date used.
    let earliest = out_cards.iter().filter(|c| c.state.queue != Queue::New).flat_map(|c| [Some(c.state.due), c.state.last_review]).flatten().fold(now_ms, i64::min);
    let crt = clock.today_start(earliest) / 1000 - 86_400;
    rep.notes = notes.len();
    rep.cards = out_cards.len();

    // Write the collection.
    let tmp = out.with_extension("collection.tmp");
    let _ = std::fs::remove_file(&tmp);
    {
        let db = Connection::open(&tmp).map_err(bad)?;
        db.execute_batch(SCHEMA).map_err(bad)?;
        let conf = json!({"nextPos": 1, "estTimes": true, "activeDecks": [1], "sortType": "noteFld", "timeLim": 0, "sortBackwards": false,
                          "addToCur": true, "curDeck": 1, "newBury": true, "newSpread": 0, "dueCounts": true, "curModel": models.keys().next().cloned().unwrap_or_default(),
                          "collapseTime": 1200, "rollover": day_hour});
        db.execute(
            "INSERT INTO col VALUES (1, ?1, ?2, ?2, 11, 0, 0, 0, ?3, ?4, ?5, ?6, '{}')",
            params![crt, now_ms, conf.to_string(), J::Object(models).to_string(), J::Object(decks_json).to_string(), J::Object(dconf).to_string()],
        )
        .map_err(bad)?;
        let tx = db.unchecked_transaction().map_err(bad)?;
        for n in &notes {
            let first = n.fields.first().cloned().unwrap_or_default();
            let tags = if n.tags.is_empty() { String::new() } else { format!(" {} ", n.tags) };
            tx.execute(
                "INSERT INTO notes VALUES (?1, ?2, ?3, ?4, -1, ?5, ?6, ?7, ?8, 0, '')",
                params![n.id, n.guid, n.mid, now_ms / 1000, tags, n.fields.join("\u{1f}"), render_text(&first), field_checksum(&first)],
            )
            .map_err(bad)?;
        }
        let our_to_anki: HashMap<&str, i64> = out_cards.iter().map(|c| (c.our_id.as_str(), c.id)).collect();
        for c in &out_cards {
            let s = &c.state;
            let (ty, queue_code) = match s.queue {
                Queue::New => (0, 0),
                Queue::Learning => (1, 1),
                Queue::Review => (2, 2),
                Queue::Relearning => (3, 1),
            };
            let queue_code = if c.suspended { -1 } else { queue_code };
            let due = match s.queue {
                Queue::New => c.id % 100_000,
                Queue::Learning | Queue::Relearning => s.due / 1000,
                Queue::Review => ((s.due - crt * 1000) / 86_400_000).max(0),
            };
            let ivl = if s.queue == Queue::Review { s.stability.unwrap_or(1.0).round().max(1.0) as i64 } else { 0 };
            let data = match (s.stability, s.difficulty) {
                (Some(st), Some(d)) => json!({"pos": c.id % 100_000, "s": st, "d": d, "dr": 0.9}).to_string(),
                _ => String::new(),
            };
            tx.execute(
                "INSERT INTO cards VALUES (?1, ?2, ?3, ?4, ?5, -1, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0, 0, 0, 0, ?13)",
                params![c.id, c.nid, c.did, c.ord, now_ms / 1000, ty, queue_code, due, ivl, if ty == 2 { 2500 } else { 0 }, s.reps, s.lapses, data],
            )
            .map_err(bad)?;
        }
        // Revlog from the review events of exported cards (undone reviews skipped).
        let mut used_rev: HashSet<i64> = HashSet::new();
        for (our, anki) in &our_to_anki {
            let evs = store.card_events(our)?;
            let undone: HashSet<String> = evs.iter().filter(|e| e.ty == "review.undo").filter_map(|e| e.payload["review_id"].as_str().map(String::from)).collect();
            let mut revs: Vec<&CardEvent> = evs.iter().filter(|e| e.ty == "review" && !undone.contains(&e.id)).collect();
            revs.sort_by_key(|e| (e.payload["at"].as_i64().unwrap_or(e.hlc >> 16), e.hlc));
            for e in revs {
                let mut rid = e.payload["at"].as_i64().unwrap_or(e.hlc >> 16);
                while !used_rev.insert(rid) {
                    rid += 1;
                }
                let ty = match e.payload["before"]["queue"].as_str() {
                    Some("review") => 1,
                    Some("relearning") => 2,
                    _ => 0,
                };
                tx.execute(
                    "INSERT INTO revlog VALUES (?1, ?2, -1, ?3, 0, 0, 0, ?4, ?5)",
                    params![rid, anki, e.payload["rating"].as_i64().unwrap_or(3), e.payload["duration_ms"].as_i64().unwrap_or(0), ty],
                )
                .map_err(bad)?;
                rep.reviews += 1;
            }
        }
        tx.commit().map_err(bad)?;
        db.execute_batch("PRAGMA journal_mode = DELETE; VACUUM;").map_err(bad)?;
    }
    let db_bytes = std::fs::read(&tmp).map_err(bad)?;
    let _ = std::fs::remove_file(&tmp);
    let _ = std::fs::remove_file(tmp.with_extension("collection.tmp-wal"));

    // Media files that exist on this device.
    media_names.sort();
    media_names.dedup();
    let mut media_map = serde_json::Map::new();
    let mut files: Vec<(String, Vec<u8>)> = vec![];
    for name in media_names {
        let hash: Option<String> = conn.query_row("SELECT media_hash FROM note_media WHERE id = ?1 AND deleted = 0", [derive_id("note_media", &[&name])], |r| r.get(0)).ok();
        let Some(path) = hash.and_then(|h| media.path(&h)) else { continue };
        let Ok(bytes) = std::fs::read(path) else { continue };
        let idx = files.len().to_string();
        media_map.insert(idx.clone(), J::String(name));
        files.push((idx, bytes));
    }
    rep.media = files.len();

    let file = std::fs::File::create(out).map_err(bad)?;
    let mut z = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for name in ["collection.anki21", "collection.anki2"] {
        z.start_file(name, opts).map_err(bad)?;
        z.write_all(&db_bytes).map_err(bad)?;
    }
    z.start_file("media", opts).map_err(bad)?;
    z.write_all(J::Object(media_map).to_string().as_bytes()).map_err(bad)?;
    for (idx, bytes) in files {
        z.start_file(idx, zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)).map_err(bad)?;
        z.write_all(&bytes).map_err(bad)?;
    }
    z.finish().map_err(bad)?;
    Ok(rep)
}

#[allow(clippy::too_many_arguments)]
fn model_json(mid: i64, name: &str, kind: &str, fields: &[String], templates: &[(String, String, String)], css: &str, did: i64, now_ms: i64) -> J {
    let tmpls: Vec<J> = templates
        .iter()
        .enumerate()
        .map(|(o, (n, q, a))| json!({"name": n, "ord": o, "qfmt": q, "afmt": a, "bqfmt": "", "bafmt": "", "did": null, "bfont": "", "bsize": 0}))
        .collect();
    let flds: Vec<J> = fields.iter().enumerate().map(|(o, n)| json!({"name": n, "ord": o, "sticky": false, "rtl": false, "font": "Arial", "size": 20, "media": []})).collect();
    let req: Vec<J> = if kind == "cloze" {
        vec![]
    } else {
        templates
            .iter()
            .enumerate()
            .map(|(o, (_, q, _))| {
                let used: Vec<usize> = fields.iter().enumerate().filter(|(_, f)| q.contains(&format!("{{{{{f}}}}}")) || q.contains(&format!(":{f}}}}}"))).map(|(i, _)| i).collect();
                json!([o, "any", if used.is_empty() { vec![0] } else { used }])
            })
            .collect()
    };
    json!({"id": mid.to_string(), "name": name, "type": if kind == "cloze" { 1 } else { 0 }, "mod": now_ms / 1000, "usn": -1, "sortf": 0, "did": did,
           "tmpls": tmpls, "flds": flds, "css": css, "latexPre": "\\documentclass[12pt]{article}\n\\special{papersize=3in,5in}\n\\usepackage[utf8]{inputenc}\n\\usepackage{amssymb,amsmath}\n\\pagestyle{empty}\n\\setlength{\\parindent}{0in}\n\\begin{document}\n",
           "latexPost": "\\end{document}", "latexsvg": false, "req": req, "tags": [], "vers": []})
}
