//! Day boundary, deck configs, scheduling policies and the due queue with daily limits (SPEC §11.3).

use crate::fsrs::{self, CardState, Config, DAY_MS, MINUTE_MS, Queue, Rating};
use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{HashMap, HashSet};

/// Anki shows a learning card up to 20 minutes before it is due, when nothing else is waiting.
pub const LEARN_AHEAD_MS: i64 = 20 * MINUTE_MS;

/// A deck config with everything that matters at review time.
#[derive(Debug, Clone)]
pub struct DeckCfg {
    pub sched: Config,
    pub new_per_day: i64,
    pub reviews_per_day: i64,
    pub bury_siblings: bool,
    pub day_starts_hour: i64,
}

impl Default for DeckCfg {
    fn default() -> Self {
        DeckCfg { sched: Config::default(), new_per_day: 20, reviews_per_day: 200, bury_siblings: true, day_starts_hour: 4 }
    }
}

/// Read a deck config row. A missing row gives the defaults.
pub fn load_config(conn: &Connection, config_id: Option<&str>) -> Result<DeckCfg> {
    let Some(id) = config_id else { return Ok(DeckCfg::default()) };
    let row = conn
        .query_row(
            "SELECT new_per_day, reviews_per_day, desired_retention, learning_steps, relearning_steps, max_interval_days,
                    fsrs_params, bury_siblings, day_starts_hour FROM deck_configs WHERE id = ?1 AND deleted = 0",
            [id],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, f64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, i64>(8)?,
                ))
            },
        )
        .optional()?;
    let Some((npd, rpd, retention, learn, relearn, max_ivl, params, bury, hour)) = row else { return Ok(DeckCfg::default()) };
    let p: Vec<f64> = params.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
    Ok(DeckCfg {
        sched: Config {
            params: fsrs::normalize_params(&p),
            desired_retention: retention.clamp(0.7, 0.99),
            learning_steps: fsrs::parse_steps(&learn),
            relearning_steps: fsrs::parse_steps(&relearn),
            max_interval: max_ivl.max(1),
            fuzz: true,
        },
        new_per_day: npd,
        reviews_per_day: rpd,
        bury_siblings: bury != 0,
        day_starts_hour: hour.clamp(0, 23),
    })
}

/// Local day boundaries: the scheduling day starts at `day_starts_hour` local time.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    pub tz_offset_min: i64,
    pub day_starts_hour: i64,
}

impl Clock {
    pub fn day_index(&self, ms: i64) -> i64 {
        (ms + self.tz_offset_min * MINUTE_MS - self.day_starts_hour * 3_600_000).div_euclid(DAY_MS)
    }

    pub fn day_start(&self, index: i64) -> i64 {
        index * DAY_MS + self.day_starts_hour * 3_600_000 - self.tz_offset_min * MINUTE_MS
    }

    /// Start of the scheduling day that contains `ms`.
    pub fn today_start(&self, ms: i64) -> i64 {
        self.day_start(self.day_index(ms))
    }

    pub fn next_day_start(&self, ms: i64) -> i64 {
        self.day_start(self.day_index(ms) + 1)
    }
}

/// A scheduling policy (SPEC §11.3). The deck kind picks it.
pub trait Policy: Sync {
    fn name(&self) -> &'static str;
    /// The card's state after `rating` at `now`.
    fn answer(&self, card_id: &str, state: &CardState, rating: Rating, now: i64, cfg: &DeckCfg, clock: &Clock) -> CardState;
}

fn fsrs_answer(card_id: &str, state: &CardState, rating: Rating, now: i64, cfg: &DeckCfg, clock: &Clock) -> CardState {
    let seed = fsrs::fuzz_seed(card_id, state.reps);
    let a = fsrs::review(state, rating, now, &cfg.sched, seed);
    let mut s = a.state;
    if let Some(days) = a.interval_days {
        s.due = clock.day_start(clock.day_index(now) + days);
    }
    s
}

/// FSRS per card: Anki decks and classic decks.
pub struct CardPolicy;
/// FSRS per concept card; the example rotation (`pool`) is chosen by the caller, scheduling is the same.
pub struct ConceptPolicy;

impl Policy for CardPolicy {
    fn name(&self) -> &'static str {
        "card"
    }
    fn answer(&self, id: &str, st: &CardState, r: Rating, now: i64, cfg: &DeckCfg, clock: &Clock) -> CardState {
        fsrs_answer(id, st, r, now, cfg, clock)
    }
}

impl Policy for ConceptPolicy {
    fn name(&self) -> &'static str {
        "concept"
    }
    fn answer(&self, id: &str, st: &CardState, r: Rating, now: i64, cfg: &DeckCfg, clock: &Clock) -> CardState {
        fsrs_answer(id, st, r, now, cfg, clock)
    }
}

/// The policy for a deck kind (`classic` or `concept`).
pub fn policy_for(kind: &str) -> &'static dyn Policy {
    if kind == "concept" { &ConceptPolicy } else { &CardPolicy }
}

#[derive(Debug, Clone)]
pub struct DeckInfo {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub config_id: Option<String>,
}

pub fn decks(conn: &Connection) -> Result<Vec<DeckInfo>> {
    let mut stmt = conn.prepare("SELECT id, name, kind, config_id FROM decks WHERE deleted = 0 ORDER BY name, id")?;
    let rows = stmt.query_map([], |r| Ok(DeckInfo { id: r.get(0)?, name: r.get(1)?, kind: r.get(2)?, config_id: r.get(3)? }))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// One card offered for review.
#[derive(Debug, Clone, PartialEq)]
pub struct QueueItem {
    pub card_id: String,
    pub deck_id: String,
    pub queue: Queue,
    pub due: i64,
}

/// Counts for the deck list, aggregated over each deck's tree.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeckCounts {
    pub deck_id: String,
    pub name: String,
    pub new: i64,
    pub learning: i64,
    pub review: i64,
}

#[derive(Debug, Default)]
pub struct Plan {
    pub items: Vec<QueueItem>,
    pub counts: Vec<DeckCounts>,
}

fn is_under(name: &str, ancestor: &str) -> bool {
    name == ancestor || (name.len() > ancestor.len() && name.starts_with(ancestor) && name[ancestor.len()..].starts_with("::"))
}

/// What was answered today, for limits and sibling burying (undone reviews don't count).
struct Done {
    new: HashMap<String, i64>,
    review: HashMap<String, i64>,
    answered: HashSet<String>,
    groups: HashSet<String>,
}

fn done_today(conn: &Connection, since: i64) -> Result<Done> {
    let mut d = Done { new: HashMap::new(), review: HashMap::new(), answered: HashSet::new(), groups: HashSet::new() };
    let mut stmt = conn.prepare(
        "SELECT c.id, c.deck_id, COALESCE(c.note_id, c.concept_id, c.id), json_extract(e.payload, '$.before.queue')
         FROM events e JOIN cards c ON c.id = e.subject_id
         WHERE e.type = 'review' AND COALESCE(json_extract(e.payload, '$.at'), e.hlc >> 16) >= ?1
           AND NOT EXISTS (SELECT 1 FROM events u WHERE u.type = 'review.undo' AND u.subject_id = e.subject_id
                           AND json_extract(u.payload, '$.review_id') = e.id)",
    )?;
    let rows = stmt.query_map([since], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?)))?;
    for row in rows {
        let (card, deck, group, q) = row?;
        match q.as_deref() {
            Some("new") => *d.new.entry(deck).or_insert(0) += 1,
            Some("review") => *d.review.entry(deck).or_insert(0) += 1,
            _ => {}
        }
        d.answered.insert(card);
        d.groups.insert(group);
    }
    Ok(d)
}

/// Build today's queue for the whole collection and the per-deck counts, with daily limits per deck tree and
/// sibling burying (SPEC §11.3). `tz_offset_min` is the local offset east of UTC.
pub fn plan(conn: &Connection, now: i64, tz_offset_min: i64) -> Result<Plan> {
    let decks = decks(conn)?;
    let cfgs: HashMap<String, DeckCfg> = decks
        .iter()
        .map(|d| Ok((d.id.clone(), load_config(conn, d.config_id.as_deref())?)))
        .collect::<Result<_>>()?;
    let clock_of = |d: &DeckInfo| Clock { tz_offset_min, day_starts_hour: cfgs[&d.id].day_starts_hour };
    // Decks sharing one day start share one pass over today's events.
    let mut done_by_hour: HashMap<i64, Done> = HashMap::new();
    for d in &decks {
        let c = clock_of(d);
        if let std::collections::hash_map::Entry::Vacant(e) = done_by_hour.entry(c.day_starts_hour) {
            e.insert(done_today(conn, c.today_start(now))?);
        }
    }
    // Ancestors of every deck: itself and all decks whose name is a `::` prefix of its name.
    let ancestors: HashMap<&str, Vec<&DeckInfo>> =
        decks.iter().map(|d| (d.id.as_str(), decks.iter().filter(|a| is_under(&d.name, &a.name)).collect())).collect();
    // Remaining allowance per deck: its limit minus what its tree already did today.
    let mut left_new: HashMap<&str, i64> = HashMap::new();
    let mut left_rev: HashMap<&str, i64> = HashMap::new();
    for a in &decks {
        let done = &done_by_hour[&cfgs[&a.id].day_starts_hour];
        let tree: Vec<&DeckInfo> = decks.iter().filter(|d| is_under(&d.name, &a.name)).collect();
        let n: i64 = tree.iter().map(|d| done.new.get(&d.id).copied().unwrap_or(0)).sum();
        let r: i64 = tree.iter().map(|d| done.review.get(&d.id).copied().unwrap_or(0)).sum();
        left_new.insert(&a.id, (cfgs[&a.id].new_per_day - n).max(0));
        left_rev.insert(&a.id, (cfgs[&a.id].reviews_per_day - r).max(0));
    }

    let mut items: Vec<QueueItem> = Vec::new();
    let mut groups_offered: HashSet<String> = HashSet::new();
    let mut stmt = conn.prepare(
        "SELECT id, COALESCE(note_id, concept_id, id), queue, due FROM cards
         WHERE deck_id = ?1 AND deleted = 0 AND suspended = 0 AND buried_until <= ?2
           AND (note_id IS NULL OR EXISTS (SELECT 1 FROM notes n WHERE n.id = cards.note_id AND n.deleted = 0))
         ORDER BY created_at, ord, id",
    )?;
    for d in &decks {
        let cfg = &cfgs[&d.id];
        let done = &done_by_hour[&cfg.day_starts_hour];
        let clock = clock_of(d);
        let day_end = clock.next_day_start(now);
        let rows: Vec<(String, String, String, i64)> = stmt
            .query_map(params![d.id, now], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<std::result::Result<_, _>>()?;
        let mut learning = vec![];
        let mut review = vec![];
        let mut new = vec![];
        for (id, group, queue, due) in rows {
            let q = Queue::parse(&queue);
            let hidden = cfg.bury_siblings && !done.answered.contains(&id) && done.groups.contains(&group);
            if hidden {
                continue;
            }
            match q {
                Queue::Learning | Queue::Relearning if due <= now + LEARN_AHEAD_MS => learning.push((id, group, q, due)),
                Queue::Review if due < day_end => review.push((id, group, q, due)),
                Queue::New => new.push((id, group, q, due)),
                _ => {}
            }
        }
        learning.sort_by_key(|x| x.3);
        review.sort_by_key(|x| x.3);
        let anc = &ancestors[d.id.as_str()];
        let mut take = |list: Vec<(String, String, Queue, i64)>, left: &mut HashMap<&str, i64>, limited: bool, items: &mut Vec<QueueItem>| {
            for (id, group, q, due) in list {
                if cfg.bury_siblings && groups_offered.contains(&group) && q != Queue::Learning && q != Queue::Relearning {
                    continue;
                }
                if limited {
                    if anc.iter().any(|a| left.get(a.id.as_str()).copied().unwrap_or(0) <= 0) {
                        break;
                    }
                    for a in anc {
                        if let Some(l) = left.get_mut(a.id.as_str()) {
                            *l -= 1;
                        }
                    }
                }
                groups_offered.insert(group);
                items.push(QueueItem { card_id: id, deck_id: d.id.clone(), queue: q, due });
            }
        };
        take(learning, &mut left_rev.clone(), false, &mut items);
        take(review, &mut left_rev, true, &mut items);
        take(new, &mut left_new, true, &mut items);
    }

    // Order: learning first (by due), then reviews (by due), then new.
    let rank = |q: Queue| match q {
        Queue::Learning | Queue::Relearning => 0,
        Queue::Review => 1,
        Queue::New => 2,
    };
    items.sort_by(|a, b| rank(a.queue).cmp(&rank(b.queue)).then(a.due.cmp(&b.due)));
    let counts = decks
        .iter()
        .map(|d| {
            let mut c = DeckCounts { deck_id: d.id.clone(), name: d.name.clone(), ..Default::default() };
            for it in &items {
                let name = &decks.iter().find(|x| x.id == it.deck_id).map(|x| x.name.as_str()).unwrap_or("");
                if is_under(name, &d.name) {
                    match it.queue {
                        Queue::New => c.new += 1,
                        Queue::Review => c.review += 1,
                        _ => c.learning += 1,
                    }
                }
            }
            c
        })
        .collect();
    Ok(Plan { items, counts })
}

/// The queue for one deck and its children.
pub fn queue_for(conn: &Connection, deck_id: &str, now: i64, tz_offset_min: i64) -> Result<Vec<QueueItem>> {
    let all = decks(conn)?;
    let root = all.iter().find(|d| d.id == deck_id).ok_or_else(|| Error::Invalid(format!("no deck {deck_id}")))?;
    let tree: HashSet<&str> = all.iter().filter(|d| is_under(&d.name, &root.name)).map(|d| d.id.as_str()).collect();
    Ok(plan(conn, now, tz_offset_min)?.items.into_iter().filter(|i| tree.contains(i.deck_id.as_str())).collect())
}
