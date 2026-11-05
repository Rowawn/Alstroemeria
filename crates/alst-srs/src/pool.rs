//! The example pool for concept cards (SPEC §11.4): candidate sentences from mined notes and from the library,
//! scored for comprehensibility, and the rotation that shows the least recently shown one.

use crate::Result;
use alst_core::Store;
use rusqlite::params;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

pub const MIN_WORDS: usize = 4;
pub const MAX_WORDS: usize = 30;
pub const PER_WORK: usize = 3;
pub const POOL_SIZE: usize = 20;
/// Added to the score of a mined sentence so it always ranks above library sentences.
pub const MINED_BONUS: f64 = 10.0;

/// A word in a sentence: its term key and its range in UTF-16 code units (like reader tokens).
#[derive(Debug, Clone, PartialEq)]
pub struct Tok {
    pub key: String,
    pub start: usize,
    pub end: usize,
}

/// Splits text into word tokens with their term keys (SPEC §16.3). The app passes one built on the language's
/// tokenizer; `SimpleSegmenter` is the fallback for tests and for languages with spaces.
pub trait Segmenter {
    fn tokens(&self, text: &str) -> Vec<Tok>;
}

/// Words are runs of alphanumeric characters; the key is the lowercase word.
pub struct SimpleSegmenter;

impl Segmenter for SimpleSegmenter {
    fn tokens(&self, text: &str) -> Vec<Tok> {
        let mut out = vec![];
        let mut pos = 0usize;
        let mut cur: Option<(usize, String)> = None;
        for ch in text.chars() {
            let len = ch.len_utf16();
            if ch.is_alphanumeric() || ch == '\'' && cur.is_some() {
                match &mut cur {
                    Some((_, w)) => w.extend(ch.to_lowercase()),
                    None => cur = Some((pos, ch.to_lowercase().collect())),
                }
            } else if let Some((start, w)) = cur.take() {
                out.push(Tok { key: w, start, end: pos });
            }
            pos += len;
        }
        if let Some((start, w)) = cur {
            out.push(Tok { key: w, start, end: pos });
        }
        out
    }
}

/// Slice of `text` by UTF-16 code unit offsets.
pub fn utf16_slice(text: &str, a: usize, b: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    let (a, b) = (a.min(units.len()), b.min(units.len()));
    String::from_utf16_lossy(&units[a..b.max(a)])
}

/// Sentences of a text as `(start, end)` in UTF-16 units, trimmed. Breaks after `. ! ? … 。！？` (and runs of them,
/// closing quotes) and at newlines.
pub fn split_sentences(text: &str) -> Vec<(usize, usize)> {
    let chars: Vec<(usize, char)> = {
        let mut pos = 0;
        text.chars().map(|c| { let p = pos; pos += c.len_utf16(); (p, c) }).collect()
    };
    let total: usize = text.encode_utf16().count();
    let mut out = vec![];
    let mut start = 0usize;
    let push = |out: &mut Vec<(usize, usize)>, a: usize, b: usize| {
        let seg: Vec<&(usize, char)> = chars.iter().filter(|(p, _)| *p >= a && *p < b).collect();
        let first = seg.iter().find(|(_, c)| !c.is_whitespace());
        let last = seg.iter().rev().find(|(_, c)| !c.is_whitespace());
        if let (Some((fa, _)), Some((la, lc))) = (first, last) {
            out.push((*fa, *la + lc.len_utf16()));
        }
    };
    let mut i = 0;
    while i < chars.len() {
        let (p, c) = chars[i];
        let end_mark = matches!(c, '.' | '!' | '?' | '…' | '。' | '！' | '？' | '\n');
        if end_mark {
            let mut j = i + 1;
            while j < chars.len() && matches!(chars[j].1, '.' | '!' | '?' | '…' | '。' | '！' | '？' | '"' | '\'' | '”' | '’' | '»' | '」' | ')') {
                j += 1;
            }
            // "3.5" or "e.g." style: a dot between digits is not a break.
            let digit_dot = c == '.' && i > 0 && chars[i - 1].1.is_ascii_digit() && chars.get(i + 1).is_some_and(|x| x.1.is_ascii_digit());
            if !digit_dot {
                let end = chars.get(j).map(|x| x.0).unwrap_or(total);
                push(&mut out, start, end);
                start = end;
                i = j;
                continue;
            }
        }
        let _ = p;
        i += 1;
    }
    push(&mut out, start, total);
    out
}

/// How easy a sentence is to read now: the share of its other words that are known, with a small preference
/// for medium length. In 0..=1.
pub fn comprehensibility(other_keys: &[&str], known: &HashSet<String>, n_words: usize) -> f64 {
    let ratio = if other_keys.is_empty() {
        1.0
    } else {
        other_keys.iter().filter(|k| known.contains(**k)).count() as f64 / other_keys.len() as f64
    };
    let len_fit = match n_words {
        6..=18 => 1.0,
        n if n < 6 => 0.6 + 0.1 * (n as f64 - MIN_WORDS as f64).max(0.0),
        n => (1.0 - (n as f64 - 18.0) / 24.0).max(0.2),
    };
    0.7 * ratio + 0.3 * len_fit
}

/// A sentence chosen for a card.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Example {
    pub text: String,
    /// The target word inside `text`, UTF-16 offsets (None when it can't be located).
    pub target: Option<(usize, usize)>,
    pub part_id: String,
    pub start: usize,
    pub end: usize,
    pub mined: bool,
    pub work_id: Option<String>,
    pub source: Option<String>,
    pub media_hash: Option<String>,
    /// The cue (ms) that covers the sentence, when the part is timed.
    pub cue: Option<(i64, i64)>,
}

/// Known words for scoring: terms at stage 3-5 or well-known.
pub fn known_keys(store: &Store, language: &str) -> Result<HashSet<String>> {
    let mut stmt = store.read_conn().prepare("SELECT text_lc FROM terms WHERE language = ?1 AND deleted = 0 AND (status BETWEEN 3 AND 5 OR status = 99)")?;
    let rows = stmt.query_map([language], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// The keys by which each concept (with cards) is found in text: its term keys and lemmas.
fn concept_keys(store: &Store, ids: &[String]) -> Result<HashMap<String, Vec<String>>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    let mut stmt = store.read_conn().prepare(
        "SELECT concept_id, text_lc, lemma FROM terms WHERE concept_id = ?1 AND deleted = 0
         UNION SELECT id, key, NULL FROM concepts WHERE id = ?1",
    )?;
    for id in ids {
        let rows = stmt.query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)))?;
        for row in rows {
            let (cid, key, lemma) = row?;
            let e = map.entry(cid).or_default();
            for k in [Some(key), lemma].into_iter().flatten() {
                let k = k.trim().to_lowercase();
                if !k.is_empty() && !e.contains(&k) {
                    e.push(k);
                }
            }
        }
    }
    Ok(map)
}

struct Cand {
    work: String,
    part: String,
    start: usize,
    end: usize,
    score: f64,
}

/// Rebuild the pool rows for these concepts from every part of their language (SPEC §11.4): sentences of 4-30
/// words that contain one of the concept's keys, scored by comprehensibility, at most 3 per work, the best 20.
/// Rows for the concepts are replaced, keeping `last_shown` of sentences that stay.
pub fn refresh(store: &mut Store, concept_ids: &[String], seg: &dyn Segmenter) -> Result<usize> {
    if concept_ids.is_empty() {
        return Ok(0);
    }
    let keys = concept_keys(store, concept_ids)?;
    let langs: HashMap<String, String> = {
        let mut stmt = store.read_conn().prepare("SELECT id, language FROM concepts WHERE id = ?1")?;
        let mut m = HashMap::new();
        for id in concept_ids {
            if let Ok((i, l)) = stmt.query_row([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) {
                m.insert(i, l);
            }
        }
        m
    };
    let mut by_lang: HashMap<&str, Vec<&String>> = HashMap::new();
    for (c, l) in &langs {
        by_lang.entry(l.as_str()).or_default().push(c);
    }
    let mut cands: HashMap<String, Vec<Cand>> = HashMap::new();
    for (lang, cids) in by_lang {
        let known = known_keys(store, lang)?;
        let mut key_to: HashMap<&str, Vec<&str>> = HashMap::new();
        for c in &cids {
            for k in keys.get(*c).into_iter().flatten() {
                key_to.entry(k.as_str()).or_default().push(c.as_str());
            }
        }
        let parts: Vec<(String, String, String)> = {
            let mut stmt = store.read_conn().prepare(
                "SELECT p.id, p.work_id, p.text FROM parts p JOIN works w ON w.id = p.work_id
                 WHERE w.language = ?1 AND w.deleted = 0 AND p.deleted = 0",
            )?;
            stmt.query_map([lang], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<std::result::Result<_, _>>()?
        };
        for (part, work, text) in parts {
            for (a, b) in split_sentences(&text) {
                let sentence = utf16_slice(&text, a, b);
                let toks = seg.tokens(&sentence);
                if !(MIN_WORDS..=MAX_WORDS).contains(&toks.len()) {
                    continue;
                }
                let hits: HashSet<&str> = toks.iter().filter_map(|t| key_to.get(t.key.as_str())).flatten().copied().collect();
                for cid in hits {
                    let own = &keys[cid];
                    let others: Vec<&str> = toks.iter().map(|t| t.key.as_str()).filter(|k| !own.iter().any(|o| o == k)).collect();
                    let score = comprehensibility(&others, &known, toks.len());
                    cands.entry(cid.to_string()).or_default().push(Cand { work: work.clone(), part: part.clone(), start: a, end: b, score });
                }
            }
        }
    }
    let mut written = 0;
    let tx = store.local_conn();
    for cid in concept_ids {
        let mut list = cands.remove(cid).unwrap_or_default();
        list.sort_by(|x, y| y.score.partial_cmp(&x.score).unwrap().then(x.part.cmp(&y.part)).then(x.start.cmp(&y.start)));
        let mut per_work: HashMap<String, usize> = HashMap::new();
        let mut kept: Vec<Cand> = vec![];
        for c in list {
            let n = per_work.entry(c.work.clone()).or_insert(0);
            if *n < PER_WORK && kept.len() < POOL_SIZE {
                *n += 1;
                kept.push(c);
            }
        }
        let shown: HashMap<(String, i64), i64> = {
            let mut stmt = tx.prepare("SELECT part_id, start, last_shown FROM local_example_pool WHERE concept_id = ?1")?;
            stmt.query_map([cid], |r| Ok(((r.get(0)?, r.get(1)?), r.get(2)?)))?.collect::<std::result::Result<_, _>>()?
        };
        tx.execute("DELETE FROM local_example_pool WHERE concept_id = ?1", [cid])?;
        for c in kept {
            let ls = shown.get(&(c.part.clone(), c.start as i64)).copied().unwrap_or(0);
            tx.execute(
                "INSERT INTO local_example_pool (concept_id, part_id, start, end, score, last_shown) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![cid, c.part, c.start as i64, c.end as i64, c.score, ls],
            )?;
            written += 1;
        }
    }
    Ok(written)
}

/// When each example of a concept was last shown, from review events (so it holds across devices) merged with
/// the local pool column. Keyed by `(part_id, start)`.
fn last_shown(store: &Store, concept_id: &str) -> Result<HashMap<(String, i64), i64>> {
    let mut map: HashMap<(String, i64), i64> = HashMap::new();
    let mut stmt = store.read_conn().prepare(
        "SELECT json_extract(e.payload, '$.example.part_id'), json_extract(e.payload, '$.example.start'),
                COALESCE(json_extract(e.payload, '$.at'), e.hlc >> 16)
         FROM events e JOIN cards c ON c.id = e.subject_id
         WHERE e.type = 'review' AND c.concept_id = ?1 AND json_extract(e.payload, '$.example.part_id') IS NOT NULL",
    )?;
    for row in stmt.query_map([concept_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))? {
        let (p, s, at) = row?;
        let e = map.entry((p, s)).or_insert(0);
        *e = (*e).max(at);
    }
    let mut stmt = store.read_conn().prepare("SELECT part_id, start, last_shown FROM local_example_pool WHERE concept_id = ?1")?;
    for row in stmt.query_map([concept_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))? {
        let (p, s, at) = row?;
        let e = map.entry((p, s)).or_insert(0);
        *e = (*e).max(at);
    }
    Ok(map)
}

/// Candidate examples in rank order: mined sentences (best first), then pool rows by score.
fn ranked(store: &Store, concept_id: &str) -> Result<Vec<(Example, f64)>> {
    let mut out: Vec<(Example, f64)> = vec![];
    // Mined: notes of a sentence type linked to the concept, in decks that exist.
    let mut stmt = store.read_conn().prepare(
        "SELECT n.id, n.fields, n.source FROM notes n JOIN note_types t ON t.id = n.note_type_id
         WHERE n.concept_id = ?1 AND n.deleted = 0 AND t.builtin = 'sentence' ORDER BY n.created_at, n.id",
    )?;
    let rows: Vec<(String, String, Option<String>)> = stmt.query_map([concept_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<std::result::Result<_, _>>()?;
    for (note_id, fields, source) in rows {
        let f: Vec<String> = serde_json::from_str(&fields).unwrap_or_default();
        let src: serde_json::Value = source.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let text = crate::template::strip_html(f.first().map(String::as_str).unwrap_or_default());
        if text.is_empty() {
            continue;
        }
        let part_id = src["partId"].as_str().map(String::from).unwrap_or_else(|| format!("note:{note_id}"));
        out.push((
            Example {
                text,
                target: None,
                part_id,
                start: src["start"].as_u64().unwrap_or(0) as usize,
                end: src["end"].as_u64().unwrap_or(0) as usize,
                mined: true,
                work_id: src["workId"].as_str().map(String::from),
                source: None,
                media_hash: src["cue"]["mediaHash"].as_str().map(String::from),
                cue: src["cue"]["start"].as_i64().zip(src["cue"]["end"].as_i64()),
            },
            MINED_BONUS,
        ));
    }
    let mut stmt = store.read_conn().prepare(
        "SELECT p.part_id, p.start, p.end, p.score, pt.text, pt.work_id, pt.media_hash, pt.timing, w.title
         FROM local_example_pool p JOIN parts pt ON pt.id = p.part_id AND pt.deleted = 0
         JOIN works w ON w.id = pt.work_id WHERE p.concept_id = ?1 ORDER BY p.score DESC, p.part_id, p.start",
    )?;
    let rows = stmt.query_map([concept_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, f64>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, Option<String>>(6)?, r.get::<_, Option<String>>(7)?, r.get::<_, String>(8)?))
    })?;
    for row in rows {
        let (part_id, start, end, score, text, work_id, media_hash, timing, title) = row?;
        let (s, e) = (start as usize, end as usize);
        out.push((
            Example {
                text: utf16_slice(&text, s, e),
                target: None,
                part_id,
                start: s,
                end: e,
                mined: false,
                work_id: Some(work_id),
                source: Some(title),
                cue: timing.as_deref().and_then(|t| cue_at(t, s)),
                media_hash,
            },
            score,
        ));
    }
    Ok(out)
}

/// The cue (start, end ms) whose text range holds the UTF-16 offset.
pub fn cue_at(timing: &str, offset: usize) -> Option<(i64, i64)> {
    let v: serde_json::Value = serde_json::from_str(timing).ok()?;
    v["cues"].as_array()?.iter().find_map(|c| {
        let (a, b) = (c["a"].as_u64()? as usize, c["b"].as_u64()? as usize);
        (offset >= a && offset < b.max(a + 1)).then(|| (c["start"].as_i64().unwrap_or(0), c["end"].as_i64().unwrap_or(0)))
    })
}

/// The example for a concept card now: the candidate shown least recently (ties: highest rank). None when the
/// concept has no sentence, and the card shows the word alone. `seg` locates the target word in the text.
pub fn pick(store: &Store, concept_id: &str, seg: &dyn Segmenter) -> Result<Option<Example>> {
    let cands = ranked(store, concept_id)?;
    if cands.is_empty() {
        return Ok(None);
    }
    let shown = last_shown(store, concept_id)?;
    let when = |e: &Example| shown.get(&(e.part_id.clone(), e.start as i64)).copied().unwrap_or(0);
    let best = cands.into_iter().enumerate().min_by_key(|(rank, (e, _))| (when(e), *rank)).map(|(_, (e, _))| e);
    let Some(mut ex) = best else { return Ok(None) };
    let keys = concept_keys(store, &[concept_id.to_string()])?.remove(concept_id).unwrap_or_default();
    ex.target = seg.tokens(&ex.text).into_iter().find(|t| keys.contains(&t.key)).map(|t| (t.start, t.end));
    Ok(Some(ex))
}

/// Note that an example was shown (the local column; review events carry it across devices).
pub fn mark_shown(store: &Store, concept_id: &str, part_id: &str, start: i64, at: i64) -> Result<()> {
    store.local_conn().execute(
        "UPDATE local_example_pool SET last_shown = MAX(last_shown, ?4) WHERE concept_id = ?1 AND part_id = ?2 AND start = ?3",
        params![concept_id, part_id, start, at],
    )?;
    Ok(())
}

/// Refresh the pools of every concept that has a card in `language`, unless nothing changed since last time
/// (same parts and same cards). Returns the number of pool rows written, or `None` when it was up to date.
pub fn refresh_language(store: &mut Store, language: &str, seg: &dyn Segmenter) -> Result<Option<usize>> {
    let ids: Vec<String> = {
        let mut stmt = store.read_conn().prepare(
            "SELECT DISTINCT COALESCE(c.concept_id, n.concept_id) FROM cards c LEFT JOIN notes n ON n.id = c.note_id
             JOIN concepts k ON k.id = COALESCE(c.concept_id, n.concept_id)
             WHERE c.deleted = 0 AND k.language = ?1 AND k.deleted = 0",
        )?;
        stmt.query_map([language], |r| r.get::<_, String>(0))?.collect::<std::result::Result<_, _>>()?
    };
    let (parts, newest): (i64, i64) = store.read_conn().query_row(
        "SELECT count(*), COALESCE(max(p.created_at), 0) FROM parts p JOIN works w ON w.id = p.work_id WHERE w.language = ?1 AND p.deleted = 0 AND w.deleted = 0",
        [language],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let sig = format!("{parts}:{newest}:{}", ids.len());
    let key = format!("srs.pool.{language}");
    let last: Option<String> = store.read_conn().query_row("SELECT value FROM local_settings WHERE key = ?1", [&key], |r| r.get(0)).ok();
    if last.as_deref() == Some(sig.as_str()) {
        return Ok(None);
    }
    let n = refresh(store, &ids, seg)?;
    store.local_conn().execute("INSERT INTO local_settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![key, sig])?;
    Ok(Some(n))
}
