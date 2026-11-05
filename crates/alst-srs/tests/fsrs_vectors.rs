//! FSRS-6 against the reference vectors (scripts/gen-fsrs-vectors.py, py-fsrs, fuzz off).

use alst_srs::fsrs::{CardState, Config, Queue, Rating, normalize_params, review};
use serde_json::Value;

#[test]
fn matches_the_reference_implementation() {
    let v: Value = serde_json::from_str(include_str!("data/fsrs-vectors.json")).unwrap();
    let t0: i64 = 1_709_294_400_000; // 2024-03-01T12:00:00Z
    let mut steps_checked = 0;
    for (si, seq) in v["sequences"].as_array().unwrap().iter().enumerate() {
        let params: Vec<f64> = seq["params"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
        let ms = |k: &str| -> Vec<i64> { seq[k].as_array().unwrap().iter().map(|x| x.as_i64().unwrap()).collect() };
        let cfg = Config {
            params: normalize_params(&params),
            desired_retention: seq["retention"].as_f64().unwrap(),
            learning_steps: ms("learning_steps"),
            relearning_steps: ms("relearning_steps"),
            max_interval: seq["max_interval"].as_i64().unwrap(),
            fuzz: false,
        };
        let mut st = CardState::new();
        for (i, (ev, ex)) in seq["events"].as_array().unwrap().iter().zip(seq["expect"].as_array().unwrap()).enumerate() {
            let now = t0 + ev["at"].as_i64().unwrap();
            st = review(&st, Rating::from_i64(ev["rating"].as_i64().unwrap()).unwrap(), now, &cfg, 0).state;
            let at = format!("sequence {si} step {i}");
            assert_eq!(st.queue, Queue::parse(ex["state"].as_str().unwrap()), "{at}: queue");
            assert_eq!(st.step, ex["step"].as_i64().unwrap(), "{at}: step");
            let close = |a: f64, b: f64| (a - b).abs() <= 1e-9 * b.abs().max(1.0);
            assert!(close(st.stability.unwrap(), ex["stability"].as_f64().unwrap()), "{at}: stability {:?} vs {}", st.stability, ex["stability"]);
            assert!(close(st.difficulty.unwrap(), ex["difficulty"].as_f64().unwrap()), "{at}: difficulty");
            assert_eq!(st.due, t0 + ex["due"].as_i64().unwrap(), "{at}: due");
            steps_checked += 1;
        }
    }
    assert!(steps_checked > 500, "{steps_checked}");
}
