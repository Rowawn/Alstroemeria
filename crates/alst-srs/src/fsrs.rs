//! FSRS-6 scheduling (SPEC §11.3), implemented from the published algorithm: forgetting curve
//! `R(t, S) = (1 + F t / S)^-d`, `F = 0.9^(-1/d) - 1`, stability and difficulty updates, and the
//! learning/relearning step ladder as Anki has it. Checked against vectors from py-fsrs (MIT).

use serde::{Deserialize, Serialize};

pub const DAY_MS: i64 = 86_400_000;
pub const MINUTE_MS: i64 = 60_000;
const S_MIN: f64 = 0.001;

pub const DEFAULT_PARAMS: [f64; 21] = [
    0.212, 1.2931, 2.3065, 8.2956, 6.4133, 0.8334, 3.0194, 0.001, 1.8722, 0.1666, 0.796, 1.4835, 0.0614, 0.2629, 1.6483,
    0.6014, 1.8729, 0.5425, 0.0912, 0.0658, 0.1542,
];

const LOWER: [f64; 21] = [
    S_MIN, S_MIN, S_MIN, S_MIN, 1.0, 0.001, 0.001, 0.001, 0.0, 0.0, 0.001, 0.001, 0.001, 0.001, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.1,
];
const UPPER: [f64; 21] = [
    100.0, 100.0, 100.0, 100.0, 10.0, 4.0, 4.0, 0.75, 4.5, 0.8, 3.5, 5.0, 0.25, 0.9, 4.0, 1.0, 6.0, 2.0, 2.0, 0.8, 0.8,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rating {
    Again = 1,
    Hard = 2,
    Good = 3,
    Easy = 4,
}

impl Rating {
    pub fn from_i64(n: i64) -> Option<Rating> {
        Some(match n {
            1 => Rating::Again,
            2 => Rating::Hard,
            3 => Rating::Good,
            4 => Rating::Easy,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Queue {
    New,
    Learning,
    Review,
    Relearning,
}

impl Queue {
    pub fn as_str(self) -> &'static str {
        match self {
            Queue::New => "new",
            Queue::Learning => "learning",
            Queue::Review => "review",
            Queue::Relearning => "relearning",
        }
    }

    pub fn parse(s: &str) -> Queue {
        match s {
            "learning" => Queue::Learning,
            "review" => Queue::Review,
            "relearning" => Queue::Relearning,
            _ => Queue::New,
        }
    }
}

/// The scheduling state of one card (the cache columns of `cards`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardState {
    pub queue: Queue,
    /// Unix ms. Review cards: start of the due day. Learning: the due moment.
    pub due: i64,
    pub stability: Option<f64>,
    pub difficulty: Option<f64>,
    pub step: i64,
    pub reps: i64,
    pub lapses: i64,
    pub last_review: Option<i64>,
}

impl CardState {
    pub fn new() -> Self {
        CardState { queue: Queue::New, due: 0, stability: None, difficulty: None, step: 0, reps: 0, lapses: 0, last_review: None }
    }
}

impl Default for CardState {
    fn default() -> Self {
        Self::new()
    }
}

/// Everything the scheduler reads from a deck config.
#[derive(Debug, Clone)]
pub struct Config {
    pub params: [f64; 21],
    pub desired_retention: f64,
    pub learning_steps: Vec<i64>,
    pub relearning_steps: Vec<i64>,
    pub max_interval: i64,
    pub fuzz: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            params: DEFAULT_PARAMS,
            desired_retention: 0.9,
            learning_steps: vec![MINUTE_MS, 10 * MINUTE_MS],
            relearning_steps: vec![10 * MINUTE_MS],
            max_interval: 36500,
            fuzz: true,
        }
    }
}

/// "1m", "10m", "2h", "1d" (or a bare number of minutes) → ms.
pub fn parse_step(s: &str) -> Option<i64> {
    let s = s.trim();
    let (num, unit) = match s.char_indices().find(|(_, c)| c.is_alphabetic()) {
        Some((i, _)) => (&s[..i], &s[i..]),
        None => (s, "m"),
    };
    let n: f64 = num.trim().parse().ok()?;
    let unit_ms = match unit {
        "s" => 1000.0,
        "m" => MINUTE_MS as f64,
        "h" => 3_600_000.0,
        "d" => DAY_MS as f64,
        _ => return None,
    };
    (n >= 0.0).then(|| (n * unit_ms).round() as i64)
}

pub fn parse_steps(json: &str) -> Vec<i64> {
    serde_json::from_str::<Vec<String>>(json).map(|v| v.iter().filter_map(|s| parse_step(s)).collect()).unwrap_or_default()
}

/// Parameters from a deck config or an Anki import: 21 numbers as is; a 19-number FSRS-5 set is extended the
/// way FSRS-6 defines (no short-term exponent, decay 0.5, which reproduces FSRS-5's curve); out-of-range
/// values are clamped. Anything else falls back to the defaults.
pub fn normalize_params(p: &[f64]) -> [f64; 21] {
    let mut out = DEFAULT_PARAMS;
    match p.len() {
        21 => out.copy_from_slice(p),
        19 => {
            out[..19].copy_from_slice(p);
            out[19] = 0.0;
            out[20] = 0.5;
        }
        _ => return DEFAULT_PARAMS,
    }
    for i in 0..21 {
        if !out[i].is_finite() {
            return DEFAULT_PARAMS;
        }
        out[i] = out[i].clamp(LOWER[i], UPPER[i]);
    }
    out
}

fn decay(p: &[f64; 21]) -> f64 {
    -p[20]
}

fn factor(p: &[f64; 21]) -> f64 {
    0.9f64.powf(1.0 / decay(p)) - 1.0
}

/// Probability of recall `elapsed_days` after a review that left stability `s`.
pub fn retrievability(p: &[f64; 21], s: f64, elapsed_days: f64) -> f64 {
    (1.0 + factor(p) * elapsed_days.max(0.0) / s).powf(decay(p))
}

fn clamp_d(d: f64) -> f64 {
    d.clamp(1.0, 10.0)
}

fn clamp_s(s: f64) -> f64 {
    s.max(S_MIN)
}

fn init_s(p: &[f64; 21], r: Rating) -> f64 {
    clamp_s(p[r as usize - 1])
}

fn init_d(p: &[f64; 21], r: Rating, clamp: bool) -> f64 {
    let d = p[4] - (p[5] * (r as i64 - 1) as f64).exp() + 1.0;
    if clamp { clamp_d(d) } else { d }
}

fn next_interval(p: &[f64; 21], cfg: &Config, s: f64) -> i64 {
    let ivl = (s / factor(p)) * (cfg.desired_retention.powf(1.0 / decay(p)) - 1.0);
    (ivl.round_ties_even() as i64).max(1).min(cfg.max_interval)
}

fn short_term_s(p: &[f64; 21], s: f64, r: Rating) -> f64 {
    let mut inc = (p[17] * (r as i64 as f64 - 3.0 + p[18])).exp() * s.powf(-p[19]);
    if r != Rating::Again {
        inc = inc.max(1.0);
    }
    clamp_s(s * inc)
}

fn next_d(p: &[f64; 21], d: f64, r: Rating) -> f64 {
    let delta = -(p[6] * (r as i64 as f64 - 3.0));
    let arg2 = d + (10.0 - d) * delta / 9.0;
    clamp_d(p[7] * init_d(p, Rating::Easy, false) + (1.0 - p[7]) * arg2)
}

fn next_s(p: &[f64; 21], d: f64, s: f64, r: f64, rating: Rating) -> f64 {
    let v = if rating == Rating::Again {
        let long = p[11] * d.powf(-p[12]) * ((s + 1.0).powf(p[13]) - 1.0) * ((1.0 - r) * p[14]).exp();
        let short = s / (p[17] * p[18]).exp();
        long.min(short)
    } else {
        let hard = if rating == Rating::Hard { p[15] } else { 1.0 };
        let easy = if rating == Rating::Easy { p[16] } else { 1.0 };
        s * (1.0 + p[8].exp() * (11.0 - d) * s.powf(-p[9]) * (((1.0 - r) * p[10]).exp() - 1.0) * hard * easy)
    };
    clamp_s(v)
}

/// A float in [0, 1) from a seed (splitmix64), so replays on every device fuzz an interval the same way.
pub fn unit_from_seed(seed: u64) -> f64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// Seed for a card's n-th review: FNV-1a of the id, mixed with the count.
pub fn fuzz_seed(card_id: &str, reps: i64) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in card_id.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
    }
    h ^ (reps as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// The fuzz window (min, max days) for an interval, per the algorithm's three ranges.
pub fn fuzz_range(days: i64, max_interval: i64) -> (i64, i64) {
    const RANGES: [(f64, f64, f64); 3] = [(2.5, 7.0, 0.15), (7.0, 20.0, 0.1), (20.0, f64::INFINITY, 0.05)];
    let mut delta = 1.0;
    for (start, end, f) in RANGES {
        delta += f * ((days as f64).min(end) - start).max(0.0);
    }
    let lo = ((days as f64 - delta).round_ties_even() as i64).max(2);
    let hi = ((days as f64 + delta).round_ties_even() as i64).min(max_interval);
    (lo.min(hi), hi)
}

fn fuzzed(days: i64, max_interval: i64, u: f64) -> i64 {
    if (days as f64) < 2.5 {
        return days;
    }
    let (lo, hi) = fuzz_range(days, max_interval);
    // Uniform over lo..=hi (the reference rounds `lo + u * (hi - lo + 1)`, which can land one past `hi`).
    (lo + (u * (hi - lo + 1) as f64).floor() as i64).min(hi)
}

/// What one answer did.
#[derive(Debug, Clone)]
pub struct Answer {
    pub state: CardState,
    /// Interval in whole days when the card (re)entered or stayed in the review queue.
    pub interval_days: Option<i64>,
}

/// Apply one rating at `now` (unix ms). `seed` drives interval fuzz (used when `cfg.fuzz`). The review
/// card's `due` here is `now + interval`; the caller snaps it to a day boundary (`sched::snap_due`).
pub fn review(state: &CardState, rating: Rating, now: i64, cfg: &Config, seed: u64) -> Answer {
    let p = &cfg.params;
    let mut c = state.clone();
    let days_since = c.last_review.map(|l| ((now - l) / DAY_MS).max(0));
    let stage = if c.queue == Queue::New { Queue::Learning } else { c.queue };
    let steps = if stage == Queue::Relearning { &cfg.relearning_steps } else { &cfg.learning_steps };

    let (s, d);
    match (c.stability, c.difficulty) {
        (Some(s0), Some(d0)) => {
            let same_day = days_since.is_some_and(|d| d < 1);
            if same_day {
                s = short_term_s(p, s0, rating);
            } else {
                let elapsed = days_since.unwrap_or(0) as f64;
                s = next_s(p, d0, s0, retrievability(p, s0, elapsed), rating);
            }
            d = next_d(p, d0, rating);
        }
        _ => {
            s = init_s(p, rating);
            d = init_d(p, rating, true);
        }
    }
    c.stability = Some(s);
    c.difficulty = Some(d);
    c.reps += 1;
    c.last_review = Some(now);

    let to_review = |c: &mut CardState| -> i64 {
        c.queue = Queue::Review;
        c.step = 0;
        next_interval(p, cfg, s)
    };
    let mut ivl_days: Option<i64> = None;
    let mut step_ms: Option<i64> = None;

    match stage {
        Queue::Review => match rating {
            Rating::Again => {
                c.lapses += 1;
                if cfg.relearning_steps.is_empty() {
                    ivl_days = Some(to_review(&mut c));
                } else {
                    c.queue = Queue::Relearning;
                    c.step = 0;
                    step_ms = Some(cfg.relearning_steps[0]);
                }
            }
            _ => ivl_days = Some(to_review(&mut c)),
        },
        _ => {
            c.queue = stage;
            let n = steps.len() as i64;
            if n == 0 || (c.step >= n && rating != Rating::Again) {
                ivl_days = Some(to_review(&mut c));
            } else {
                match rating {
                    Rating::Again => {
                        c.step = 0;
                        step_ms = Some(steps[0]);
                    }
                    Rating::Hard => {
                        let i = c.step as usize;
                        step_ms = Some(if c.step == 0 && n == 1 {
                            (steps[0] as f64 * 1.5) as i64
                        } else if c.step == 0 && n >= 2 {
                            (steps[0] + steps[1]) / 2
                        } else {
                            steps[i]
                        });
                    }
                    Rating::Good => {
                        if c.step + 1 == n {
                            ivl_days = Some(to_review(&mut c));
                        } else {
                            c.step += 1;
                            step_ms = Some(steps[c.step as usize]);
                        }
                    }
                    Rating::Easy => ivl_days = Some(to_review(&mut c)),
                }
            }
        }
    }

    if let Some(mut days) = ivl_days {
        if cfg.fuzz {
            days = fuzzed(days, cfg.max_interval, unit_from_seed(seed));
        }
        ivl_days = Some(days);
        c.due = now + days * DAY_MS;
    } else {
        c.due = now + step_ms.unwrap_or(0);
    }
    Answer { state: c, interval_days: ivl_days }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_parse() {
        assert_eq!(parse_step("1m"), Some(60_000));
        assert_eq!(parse_step("10m"), Some(600_000));
        assert_eq!(parse_step("2h"), Some(7_200_000));
        assert_eq!(parse_step("1d"), Some(DAY_MS));
        assert_eq!(parse_step("5"), Some(300_000));
        assert_eq!(parse_step("x"), None);
        assert_eq!(parse_steps(r#"["1m","10m"]"#), vec![60_000, 600_000]);
    }

    #[test]
    fn fsrs5_parameters_are_extended_to_fsrs6() {
        let five: Vec<f64> = DEFAULT_PARAMS[..19].to_vec();
        let p = normalize_params(&five);
        assert_eq!((p[19], p[20]), (0.0, 0.5));
        assert_eq!(&p[..19], &five[..]);
        assert_eq!(normalize_params(&[1.0; 5]), DEFAULT_PARAMS);
        assert_eq!(normalize_params(&DEFAULT_PARAMS), DEFAULT_PARAMS);
        let mut wild = DEFAULT_PARAMS;
        wild[0] = 500.0;
        assert_eq!(normalize_params(&wild)[0], 100.0);
    }

    #[test]
    fn fuzz_stays_in_its_window_and_is_deterministic() {
        assert_eq!(fuzz_range(3, 36500), (2, 4));
        assert_eq!(fuzz_range(30, 36500), (27, 33));
        assert_eq!(fuzz_range(100, 100), (93, 100));
        for days in [3, 5, 8, 20, 45, 400] {
            let (lo, hi) = fuzz_range(days, 36500);
            let mut seen = std::collections::BTreeSet::new();
            for k in 0..400u64 {
                let v = fuzzed(days, 36500, unit_from_seed(fuzz_seed("card", k as i64)));
                assert!((lo..=hi).contains(&v), "{days}: {v} outside {lo}..{hi}");
                seen.insert(v);
            }
            assert!(seen.len() > 1, "{days}: fuzz never varied");
        }
        assert_eq!(fuzzed(2, 36500, 0.99), 2, "short intervals are not fuzzed");
        assert_eq!(unit_from_seed(fuzz_seed("a", 3)), unit_from_seed(fuzz_seed("a", 3)));
        assert_ne!(unit_from_seed(fuzz_seed("a", 3)), unit_from_seed(fuzz_seed("a", 4)));
    }

    #[test]
    fn retrievability_is_ninety_percent_at_the_stability() {
        let r = retrievability(&DEFAULT_PARAMS, 10.0, 10.0);
        assert!((r - 0.9).abs() < 1e-9);
    }
}
