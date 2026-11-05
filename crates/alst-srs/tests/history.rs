mod common;
use alst_srs::fsrs::{Queue, Rating};
use alst_srs::replay;
use common::*;

fn setup() -> (alst_core::Store, String) {
    let mut s = alst_srs::open_in_memory().unwrap();
    note_type(&mut s);
    config(&mut s, "c", 20, 200, false);
    deck(&mut s, "d", "D", "classic", "c");
    let id = note(&mut s, "n", "d", 1).remove(0);
    (s, id)
}

#[test]
fn undo_restores_the_card_and_replay_skips_the_undone_review() {
    let (mut s, id) = setup();
    replay::answer(&mut s, &id, Rating::Good, 1000, None, T0, 0).unwrap();
    let after_one = replay::card_state(&s, &id).unwrap().unwrap();
    replay::answer(&mut s, &id, Rating::Easy, 1000, None, T0 + 60_000, 0).unwrap();
    assert_ne!(replay::card_state(&s, &id).unwrap().unwrap(), after_one);
    assert!(replay::undo(&mut s, &id).unwrap());
    assert_eq!(replay::card_state(&s, &id).unwrap().unwrap(), after_one);
    assert!(replay::undo(&mut s, &id).unwrap());
    assert_eq!(replay::card_state(&s, &id).unwrap().unwrap().queue, Queue::New);
    assert!(!replay::undo(&mut s, &id).unwrap(), "nothing left to undo");
    // The undone reviews no longer count against today's limit.
    let plan = alst_srs::sched::plan(s.read_conn(), T0 + 120_000, 0).unwrap();
    assert_eq!(plan.items.len(), 1);
    assert_eq!(s.events_query(Some("review.undo"), None, 10).unwrap().len(), 2);
}

#[test]
fn the_cache_is_a_function_of_the_events() {
    let (mut s, id) = setup();
    let mut now = T0;
    for r in [Rating::Good, Rating::Good, Rating::Good, Rating::Again, Rating::Good, Rating::Hard] {
        replay::answer(&mut s, &id, r, 800, None, now, 0).unwrap();
        now += 3 * DAY;
    }
    let cached = replay::card_state(&s, &id).unwrap().unwrap();
    s.set_card_cache(&id, &alst_core::srs::CardCache { queue: "new".into(), due: 0, stability: None, difficulty: None, step: 0, reps: 0, lapses: 0, last_review: None }).unwrap();
    replay::replay_card(&mut s, &id).unwrap();
    assert_eq!(replay::card_state(&s, &id).unwrap().unwrap(), cached);
    assert_eq!(cached.reps, 6);
    assert_eq!(cached.lapses, 1);
}

#[test]
fn a_state_snapshot_is_the_baseline_for_later_reviews() {
    let (mut s, id) = setup();
    s.emit_event_id(
        "review.state",
        None,
        Some(("card", &id)),
        &serde_json::json!({"at": T0, "queue": "review", "due": T0 + 10 * DAY, "stability": 30.0, "difficulty": 5.0, "reps": 12, "lapses": 2, "last_review": T0 - 20 * DAY}),
    )
    .unwrap();
    replay::replay_card(&mut s, &id).unwrap();
    let st = replay::card_state(&s, &id).unwrap().unwrap();
    assert_eq!((st.queue, st.stability, st.reps), (Queue::Review, Some(30.0), 12));
    replay::answer(&mut s, &id, Rating::Good, 900, None, T0 + 10 * DAY, 0).unwrap();
    let st = replay::card_state(&s, &id).unwrap().unwrap();
    assert_eq!((st.reps, st.lapses), (13, 2));
    assert!(st.stability.unwrap() > 30.0);
}
