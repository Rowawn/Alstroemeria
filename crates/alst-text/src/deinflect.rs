//! Data-driven deinflection (SPEC §16.5). A language ships its own rule file; nothing here is
//! specific to Japanese. Rules are our own grammar tables (no third-party rule data).
//!
//! A rule rewrites the end of a word to a more basic form:
//! `{ suffixIn, suffixOut, condIn, condOut, reason }`. The surface starts with every condition, so any
//! rule can start a chain; afterwards only rules whose `condIn` intersects the previous rule's `condOut`
//! may follow.

use crate::{Error, Result};
use serde::Deserialize;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuleJson {
    suffix_in: String,
    suffix_out: String,
    #[serde(default)]
    cond_in: Vec<String>,
    #[serde(default)]
    cond_out: Vec<String>,
    reason: String,
}

#[derive(Deserialize)]
struct FileJson {
    rules: Vec<RuleJson>,
}

#[derive(Debug, Clone)]
struct Rule {
    suffix_in: String,
    suffix_out: String,
    cond_in: u64,
    cond_out: u64,
    reason: String,
}

/// One possible dictionary form of a surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub text: String,
    /// Conditions of the rewritten word (`v1`, `v5`, `adj-i`, …); empty for the unmodified surface.
    pub conditions: Vec<String>,
    /// In conjugation order (innermost first), e.g. `["causative", "passive", "past"]`.
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Deinflector {
    names: Vec<String>,
    rules: Vec<Rule>,
    /// Rule indexes by the last character of `suffix_in`, in file order.
    by_last: HashMap<char, Vec<usize>>,
}

const MAX_DEPTH: usize = 10;
const MAX_CANDIDATES: usize = 1024;

impl Deinflector {
    pub fn from_json(json: &str) -> Result<Self> {
        let file: FileJson = serde_json::from_str(json).map_err(|e| Error::Manifest(format!("deinflection rules: {e}")))?;
        let mut names: Vec<String> = Vec::new();
        let bit = |n: &str, names: &mut Vec<String>| -> Result<u64> {
            let i = match names.iter().position(|x| x == n) {
                Some(i) => i,
                None => {
                    names.push(n.to_string());
                    names.len() - 1
                }
            };
            if i >= 64 {
                return Err(Error::Manifest("deinflection rules use more than 64 conditions".into()));
            }
            Ok(1u64 << i)
        };
        let mut rules = Vec::with_capacity(file.rules.len());
        for r in file.rules {
            if r.suffix_in.is_empty() {
                return Err(Error::Manifest("deinflection rule with empty suffixIn".into()));
            }
            let mut cond_in = 0;
            for c in &r.cond_in {
                cond_in |= bit(c, &mut names)?;
            }
            let mut cond_out = 0;
            for c in &r.cond_out {
                cond_out |= bit(c, &mut names)?;
            }
            // An empty condIn means "any".
            let cond_in = if r.cond_in.is_empty() { u64::MAX } else { cond_in };
            rules.push(Rule { suffix_in: r.suffix_in, suffix_out: r.suffix_out, cond_in, cond_out, reason: r.reason });
        }
        let mut by_last: HashMap<char, Vec<usize>> = HashMap::new();
        for (i, r) in rules.iter().enumerate() {
            by_last.entry(r.suffix_in.chars().next_back().unwrap()).or_default().push(i);
        }
        Ok(Self { names, rules, by_last })
    }

    fn names_of(&self, mask: u64) -> Vec<String> {
        self.names.iter().enumerate().filter(|(i, _)| mask >> i & 1 == 1).map(|(_, n)| n.clone()).collect()
    }

    /// Breadth-first rewrite of `surface` (SPEC §16.5): every matching rule, depth ≤ 10, deduped on
    /// (text, conditions). The first candidate is always the unmodified surface.
    pub fn deinflect(&self, surface: &str) -> Vec<Candidate> {
        struct Node {
            text: String,
            conds: u64,
            reasons: Vec<usize>,
        }
        let mut seen: HashSet<(String, u64)> = HashSet::new();
        let mut out: Vec<Candidate> = Vec::new();
        let mut queue: VecDeque<Node> = VecDeque::new();
        seen.insert((surface.to_string(), u64::MAX));
        out.push(Candidate { text: surface.to_string(), conditions: Vec::new(), reasons: Vec::new() });
        queue.push_back(Node { text: surface.to_string(), conds: u64::MAX, reasons: Vec::new() });
        while let Some(node) = queue.pop_front() {
            if node.reasons.len() >= MAX_DEPTH {
                continue;
            }
            let Some(last) = node.text.chars().next_back() else { continue };
            let Some(idxs) = self.by_last.get(&last) else { continue };
            for &ri in idxs {
                let r = &self.rules[ri];
                if r.cond_in & node.conds == 0 || !node.text.ends_with(&r.suffix_in) {
                    continue;
                }
                let stem = &node.text[..node.text.len() - r.suffix_in.len()];
                let text = format!("{stem}{}", r.suffix_out);
                if text.is_empty() || !seen.insert((text.clone(), r.cond_out)) {
                    continue;
                }
                let mut reasons = node.reasons.clone();
                reasons.push(ri);
                out.push(Candidate {
                    text: text.clone(),
                    conditions: self.names_of(r.cond_out),
                    // peeled outermost-first; report in conjugation order
                    reasons: reasons.iter().rev().map(|&i| self.rules[i].reason.clone()).collect(),
                });
                if out.len() >= MAX_CANDIDATES {
                    return out;
                }
                queue.push_back(Node { text, conds: r.cond_out, reasons });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULES: &str = r#"{"rules":[
      {"suffixIn":"た","suffixOut":"る","condIn":["past"],"condOut":["v1"],"reason":"past"},
      {"suffixIn":"ている","suffixOut":"て","condIn":["v1"],"condOut":["te"],"reason":"progressive"},
      {"suffixIn":"て","suffixOut":"る","condIn":["te"],"condOut":["v1"],"reason":"te"}
    ]}"#;

    #[test]
    fn chains_rules_and_reports_conjugation_order() {
        let d = Deinflector::from_json(RULES).unwrap();
        let c = d.deinflect("食べていた");
        assert_eq!(c[0].text, "食べていた");
        assert!(c[0].reasons.is_empty() && c[0].conditions.is_empty());
        let hit = c.iter().find(|c| c.text == "食べる").unwrap();
        assert_eq!(hit.reasons, ["te", "progressive", "past"]);
        assert_eq!(hit.conditions, ["v1"]);
    }

    #[test]
    fn conditions_gate_chaining() {
        // 'て→る' needs a te-form, which only the surface (all conditions) or 'ている→て' provides:
        // after 'た→る' the word is v1, so it cannot be rewritten by 'て→る' again.
        let d = Deinflector::from_json(RULES).unwrap();
        let c = d.deinflect("たた");
        assert!(c.iter().all(|c| c.reasons.len() <= 1), "{c:?}");
    }

    #[test]
    fn dedupes_on_text_and_conditions_first_rule_wins() {
        let d = Deinflector::from_json(
            r#"{"rules":[
              {"suffixIn":"た","suffixOut":"る","condOut":["v1"],"reason":"first"},
              {"suffixIn":"た","suffixOut":"る","condOut":["v1"],"reason":"second"}]}"#,
        )
        .unwrap();
        let c = d.deinflect("見た");
        assert_eq!(c.iter().filter(|c| c.text == "見る").count(), 1);
        assert_eq!(c[1].reasons, ["first"]);
    }

    #[test]
    fn depth_is_bounded_and_empty_results_dropped() {
        let d = Deinflector::from_json(r#"{"rules":[{"suffixIn":"ああ","suffixOut":"あ","condOut":["x"],"reason":"r"}]}"#).unwrap();
        let c = d.deinflect(&"あ".repeat(40));
        assert!(c.iter().map(|c| c.reasons.len()).max().unwrap() <= MAX_DEPTH);
        let d = Deinflector::from_json(r#"{"rules":[{"suffixIn":"した","suffixOut":"","condOut":["vs"],"reason":"past"}]}"#).unwrap();
        assert_eq!(d.deinflect("した").len(), 1, "an empty rewrite is not a candidate");
        assert_eq!(d.deinflect("勉強した")[1].text, "勉強");
    }

    #[test]
    fn bad_rule_files_are_rejected() {
        assert!(Deinflector::from_json("{").is_err());
        assert!(Deinflector::from_json(r#"{"rules":[{"suffixIn":"","suffixOut":"a","reason":"r"}]}"#).is_err());
    }
}
