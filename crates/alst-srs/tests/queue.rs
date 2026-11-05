mod common;
use alst_srs::fsrs::{Queue, Rating};
use alst_srs::{replay, sched};
use common::*;

fn setup() -> alst_core::Store {
    let mut s = alst_srs::open_in_memory().unwrap();
    note_type(&mut s);
    s
}

#[test]
fn the_day_starts_at_the_configured_local_hour() {
    // UTC-8, day starts 04:00 local = 12:00 UTC.
    let c = sched::Clock { tz_offset_min: -480, day_starts_hour: 4 };
    let noon_utc = T0; // 12:00 UTC exactly at a boundary
    assert_eq!(c.today_start(noon_utc), noon_utc);
    assert_eq!(c.today_start(noon_utc - 1), noon_utc - DAY);
    assert_eq!(c.next_day_start(noon_utc), noon_utc + DAY);
    assert_eq!(c.day_index(noon_utc + 1) - c.day_index(noon_utc - 1), 1);
}

#[test]
fn new_cards_follow_the_daily_limit_of_every_deck_in_the_tree() {
    let mut s = setup();
    config(&mut s, "parent", 5, 200, false);
    config(&mut s, "kid", 20, 200, false);
    deck(&mut s, "p", "Spanish", "classic", "parent");
    deck(&mut s, "a", "Spanish::A", "classic", "kid");
    deck(&mut s, "b", "Spanish::B", "classic", "kid");
    for n in 0..10 {
        note(&mut s, &format!("na{n}"), "a", 1);
        note(&mut s, &format!("nb{n}"), "b", 1);
    }
    let plan = sched::plan(s.read_conn(), T0, 0).unwrap();
    assert_eq!(plan.items.len(), 5, "the parent's limit caps the tree");
    let root = plan.counts.iter().find(|c| c.name == "Spanish").unwrap();
    assert_eq!(root.new, 5);
    let a = plan.counts.iter().find(|c| c.name == "Spanish::A").unwrap();
    assert_eq!(a.new + plan.counts.iter().find(|c| c.name == "Spanish::B").unwrap().new, 5);
    // Answering a new card uses up one of today's five.
    let first = plan.items[0].card_id.clone();
    replay::answer(&mut s, &first, Rating::Good, 3000, None, T0, 0).unwrap();
    let plan = sched::plan(s.read_conn(), T0 + 1000, 0).unwrap();
    let new_left = plan.items.iter().filter(|i| i.queue == Queue::New).count();
    assert_eq!(new_left, 4);
    // Next day the allowance is back.
    let plan = sched::plan(s.read_conn(), T0 + DAY + 1000, 0).unwrap();
    assert!(plan.items.iter().filter(|i| i.queue == Queue::New).count() >= 5);
}

#[test]
fn review_limit_and_due_dates() {
    let mut s = setup();
    config(&mut s, "c", 20, 2, false);
    deck(&mut s, "d", "D", "classic", "c");
    let ids: Vec<String> = (0..4).flat_map(|n| note(&mut s, &format!("n{n}"), "d", 1)).collect();
    // Make every card a mature review card by answering Easy twice over time, then look from later.
    for id in &ids {
        replay::answer(&mut s, id, Rating::Easy, 1000, None, T0, 0).unwrap();
    }
    let st = replay::card_state(&s, &ids[0]).unwrap().unwrap();
    assert_eq!(st.queue, Queue::Review);
    assert!(st.due > T0 + DAY, "easy goes days out");
    let before = sched::plan(s.read_conn(), T0 + HOUR, 0).unwrap();
    assert!(before.items.is_empty(), "nothing due today");
    let far = sched::plan(s.read_conn(), st.due + 2 * DAY, 0).unwrap();
    assert_eq!(far.items.len(), 2, "the review limit caps due reviews at 2 of 4");
    assert!(far.items.iter().all(|i| i.queue == Queue::Review));
}

#[test]
fn siblings_are_buried_until_the_next_day_and_learning_cards_stay() {
    let mut s = setup();
    config(&mut s, "c", 20, 200, true);
    deck(&mut s, "d", "D", "classic", "c");
    let ids = note(&mut s, "n", "d", 2);
    let plan = sched::plan(s.read_conn(), T0, 0).unwrap();
    assert_eq!(plan.items.len(), 1, "only one sibling is offered");
    let shown = plan.items[0].card_id.clone();
    replay::answer(&mut s, &shown, Rating::Good, 1000, None, T0, 0).unwrap(); // step 2 of learning
    let plan = sched::plan(s.read_conn(), T0 + 11 * 60_000, 0).unwrap();
    let got: Vec<&str> = plan.items.iter().map(|i| i.card_id.as_str()).collect();
    assert_eq!(got, vec![shown.as_str()], "the learning card comes back, its sibling stays buried");
    let other = ids.iter().find(|x| **x != shown).unwrap();
    // Next day the learning card is offered first and its sibling still waits; once it graduates the
    // sibling is buried for that day too, and appears the day after.
    let d2 = T0 + DAY + 11 * 60_000;
    let plan = sched::plan(s.read_conn(), d2, 0).unwrap();
    assert_eq!(plan.items.iter().map(|i| i.card_id.as_str()).collect::<Vec<_>>(), vec![shown.as_str()]);
    replay::answer(&mut s, &shown, Rating::Easy, 1000, None, d2, 0).unwrap();
    assert!(sched::plan(s.read_conn(), d2 + HOUR, 0).unwrap().items.is_empty());
    let plan = sched::plan(s.read_conn(), d2 + DAY, 0).unwrap();
    assert!(plan.items.iter().any(|i| &i.card_id == other), "buried only for the day");
}

#[test]
fn suspended_and_manually_buried_cards_are_skipped() {
    let mut s = setup();
    config(&mut s, "c", 20, 200, false);
    deck(&mut s, "d", "D", "classic", "c");
    let a = note(&mut s, "a", "d", 1).remove(0);
    let b = note(&mut s, "b", "d", 1).remove(0);
    s.srs_update("cards", &a, &[("suspended", i(1))]).unwrap();
    s.srs_update("cards", &b, &[("buried_until", i(T0 + HOUR))]).unwrap();
    assert!(sched::plan(s.read_conn(), T0, 0).unwrap().items.is_empty());
    assert_eq!(sched::plan(s.read_conn(), T0 + 2 * HOUR, 0).unwrap().items.len(), 1);
}

#[test]
fn the_policy_follows_the_deck_kind_and_answers_are_recorded() {
    assert_eq!(sched::policy_for("concept").name(), "concept");
    assert_eq!(sched::policy_for("classic").name(), "card");
    let mut s = setup();
    config(&mut s, "c", 20, 200, false);
    deck(&mut s, "d", "D", "concept", "c");
    let id = note(&mut s, "n", "d", 1).remove(0);
    replay::answer(&mut s, &id, Rating::Good, 1500, Some(("part", 7)), T0, 60).unwrap();
    let ev = s.events_query(Some("review"), None, 10).unwrap();
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].payload["policy"], "concept");
    assert_eq!(ev[0].payload["example"]["start"], 7);
    assert_eq!(ev[0].payload["before"]["queue"], "new");
}
