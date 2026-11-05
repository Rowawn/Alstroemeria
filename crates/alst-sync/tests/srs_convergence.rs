//! SPEC §11.6: devices that review the same cards offline keep every review and end with identical card state.
//! The card scheduling columns are a cache of replaying events, so they must agree after sync too.

use alst_core::Store;
use alst_core::srs::classic_card_id;
use alst_srs::fsrs::Rating;
use alst_srs::replay;
use alst_sync::proto::mem;
use alst_sync::session::{Role, SessionConfig, run};
use proptest::prelude::*;
use rusqlite::types::Value;
use std::sync::Mutex;

const TABLES: [&str; 8] = ["deck_configs", "decks", "note_types", "notes", "cards", "note_media", "concepts", "terms"];
const T0: i64 = 1_709_294_400_000;
const MIN: i64 = 60_000;

fn dump(s: &Mutex<Store>) -> String {
    let s = s.lock().unwrap();
    let c = s.read_conn();
    let mut out = String::new();
    for t in TABLES.iter().chain(["_changes", "events"].iter()) {
        let order = if *t == "_changes" { "tbl, row_id, col" } else { "id" };
        let mut stmt = c.prepare(&format!("SELECT * FROM {t} ORDER BY {order}")).unwrap();
        let n = stmt.column_count();
        let mut rows = stmt.query([]).unwrap();
        while let Some(r) = rows.next().unwrap() {
            out.push_str(t);
            for i in 0..n {
                out.push_str(&format!("|{:?}", r.get_ref(i).unwrap()));
            }
            out.push('\n');
        }
    }
    out
}

fn session(stores: &[Mutex<Store>], a: usize, b: usize, cut: Option<usize>, batch_rows: usize) -> bool {
    if a == b {
        return true;
    }
    let (ta, tb) = mem::pair(cut, None);
    let mut cfg = SessionConfig::new("dev");
    cfg.batch_rows = batch_rows;
    let (ra, rb) = std::thread::scope(|sc| {
        let ha = sc.spawn(|| {
            let mut ta = ta;
            run(&stores[a], &mut ta, Role::Initiator, &cfg)
        });
        let hb = sc.spawn(|| {
            let mut tb = tb;
            run(&stores[b], &mut tb, Role::Responder, &cfg)
        });
        (ha.join().unwrap(), hb.join().unwrap())
    });
    ra.is_ok() && rb.is_ok()
}

fn full_round(stores: &[Mutex<Store>], batch_rows: usize) {
    for a in 0..stores.len() {
        for b in 0..stores.len() {
            if a != b {
                assert!(session(stores, a, b, None, batch_rows), "full session {a}->{b} failed");
            }
        }
    }
}

fn t(s: &str) -> Value {
    Value::Text(s.into())
}

/// Device 0 builds a deck of two notes (a concept deck and a classic deck, 4 cards) and everyone syncs it.
fn stores_with_cards(n: usize, batch_rows: usize) -> (Vec<Mutex<Store>>, Vec<String>) {
    let st: Vec<Mutex<Store>> = (0..n).map(|_| Mutex::new(alst_srs::open_in_memory().unwrap())).collect();
    let mut ids = vec![];
    {
        let mut s = st[0].lock().unwrap();
        s.srs_put("deck_configs", "cfg", &[("name", t("c")), ("fsrs_params", Value::Null)]).unwrap();
        s.srs_put("decks", "dk", &[("name", t("D")), ("kind", t("classic")), ("language", t("es")), ("config_id", t("cfg"))]).unwrap();
        s.srs_put("decks", "dk2", &[("name", t("D::C")), ("kind", t("concept")), ("language", t("es")), ("config_id", t("cfg"))]).unwrap();
        s.srs_put("note_types", "nt", &[("name", t("Basic")), ("fields", t(r#"["a","b"]"#))]).unwrap();
        for (n, deck) in [("n1", "dk"), ("n2", "dk2")] {
            s.srs_put("notes", n, &[("note_type_id", t("nt")), ("deck_id", t(deck)), ("fields", t(r#"["x","y"]"#))]).unwrap();
            for ord in 0..2 {
                let id = classic_card_id(n, ord);
                s.srs_put("cards", &id, &[("note_id", t(n)), ("deck_id", t(deck)), ("ord", Value::Integer(ord))]).unwrap();
                ids.push(id);
            }
        }
    }
    full_round(&st, batch_rows);
    (st, ids)
}

#[derive(Debug, Clone)]
enum Op {
    Review { d: usize, card: usize, rating: i64, at_min: i64, tz: i64 },
    Undo { d: usize, card: usize },
    Suspend { d: usize, card: usize, on: bool },
    Session { a: usize, b: usize, cut: Option<u8> },
}

fn op_strategy(n: usize) -> impl Strategy<Value = Op> {
    prop_oneof![
        8 => (0..n, 0..4usize, 1..5i64, 0..60_000i64, prop::sample::select(vec![0i64, -480, 330])).prop_map(|(d, card, rating, at_min, tz)| Op::Review { d, card, rating, at_min, tz }),
        2 => (0..n, 0..4usize).prop_map(|(d, card)| Op::Undo { d, card }),
        1 => (0..n, 0..4usize, any::<bool>()).prop_map(|(d, card, on)| Op::Suspend { d, card, on }),
        4 => (0..n, 0..n, proptest::option::of(0..12u8)).prop_map(|(a, b, cut)| Op::Session { a, b, cut }),
    ]
}

fn apply(st: &[Mutex<Store>], ids: &[String], op: &Op, batch_rows: usize) {
    match op {
        Op::Review { d, card, rating, at_min, tz } => {
            let mut s = st[*d].lock().unwrap();
            replay::answer(&mut s, &ids[*card], Rating::from_i64(*rating).unwrap(), 1000, None, T0 + at_min * MIN, *tz).unwrap();
        }
        Op::Undo { d, card } => {
            let mut s = st[*d].lock().unwrap();
            replay::undo(&mut s, &ids[*card]).unwrap();
        }
        Op::Suspend { d, card, on } => {
            st[*d].lock().unwrap().srs_update("cards", &ids[*card], &[("suspended", Value::Integer(*on as i64))]).unwrap();
        }
        Op::Session { a, b, cut } => {
            session(st, *a, *b, cut.map(|c| c as usize), batch_rows);
        }
    }
}

fn assert_converged(st: &[Mutex<Store>]) -> String {
    let d0 = dump(&st[0]);
    for (i, s) in st.iter().enumerate().skip(1) {
        assert_eq!(dump(s), d0, "device {i} differs from device 0");
    }
    d0
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 40, max_shrink_iters: 200, ..ProptestConfig::default() })]

    #[test]
    fn offline_reviews_on_several_devices_converge_to_identical_card_state(
        n in 2..4usize, batch_rows in 1..9usize, ops in prop::collection::vec(op_strategy(4), 1..50),
    ) {
        let (st, ids) = stores_with_cards(n, batch_rows);
        for op in &ops {
            let op = match op {
                Op::Review { d, card, rating, at_min, tz } => Op::Review { d: d % n, card: *card, rating: *rating, at_min: *at_min, tz: *tz },
                Op::Undo { d, card } => Op::Undo { d: d % n, card: *card },
                Op::Suspend { d, card, on } => Op::Suspend { d: d % n, card: *card, on: *on },
                Op::Session { a, b, cut } => Op::Session { a: a % n, b: b % n, cut: *cut },
            };
            apply(&st, &ids, &op, batch_rows);
        }
        full_round(&st, batch_rows);
        full_round(&st, batch_rows);
        let before = assert_converged(&st);
        // The cache really is the replay of the events: replaying changes nothing anywhere.
        for s in &st {
            let mut s = s.lock().unwrap();
            for id in &ids { replay::replay_card(&mut s, id).unwrap(); }
        }
        let after = assert_converged(&st);
        prop_assert_eq!(&before, &after);
        // No review was lost: every device holds every review event.
        let reviews = st.iter().map(|s| s.lock().unwrap().events_query(Some("review"), None, 10_000).unwrap().len()).collect::<Vec<_>>();
        prop_assert!(reviews.iter().all(|r| *r == reviews[0]));
        full_round(&st, batch_rows);
        prop_assert_eq!(before, assert_converged(&st));
    }
}

#[test]
fn two_devices_review_the_same_card_offline_and_both_reviews_count() {
    let (st, ids) = stores_with_cards(2, 500);
    let id = &ids[0];
    {
        let mut a = st[0].lock().unwrap();
        replay::answer(&mut a, id, Rating::Good, 1000, None, T0, 0).unwrap();
        replay::answer(&mut a, id, Rating::Good, 1000, None, T0 + 11 * MIN, 0).unwrap();
    }
    {
        let mut b = st[1].lock().unwrap();
        replay::answer(&mut b, id, Rating::Again, 1000, None, T0 + 5 * MIN, 0).unwrap();
    }
    full_round(&st, 500);
    assert_converged(&st);
    for s in &st {
        let s = s.lock().unwrap();
        let state = replay::card_state(&s, id).unwrap().unwrap();
        assert_eq!(state.reps, 3, "all three reviews replayed in time order");
    }
}
