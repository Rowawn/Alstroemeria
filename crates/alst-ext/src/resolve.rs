//! Capability resolution (SPEC §6.9.3): which extensions run for a language, in what order.

use crate::manifest::{API_VERSION, Kind, Manifest, RuntimeType};
use semver::{Version, VersionReq};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Capabilities the host itself provides (SPEC §6.9.3).
pub const CORE_CAPABILITIES: &[&str] =
    &["core.tokenize", "core.dict", "core.terms", "core.events", "core.storage", "core.net", "core.ui"];

/// User choices stored for one extension (`extension_state`).
#[derive(Debug, Clone, PartialEq)]
pub struct UserState {
    pub enabled: bool,
    /// Languages the user enabled it for; empty means every language it targets.
    pub languages: Vec<String>,
    pub settings: serde_json::Value,
}

impl Default for UserState {
    fn default() -> Self {
        Self { enabled: true, languages: vec![], settings: serde_json::json!({}) }
    }
}

pub struct Candidate<'a> {
    pub manifest: &'a Manifest,
    pub bundled: bool,
}

pub struct Input<'a> {
    pub language: &'a str,
    pub platform: &'a str,
    pub candidates: Vec<Candidate<'a>>,
    pub state: &'a HashMap<String, UserState>,
    /// `ext.providers.<lang>`: capability → extension id.
    pub providers: &'a BTreeMap<String, String>,
    /// `ext.decoratorOrder.<lang>`: extension ids, first wins.
    pub decorator_order: &'a [String],
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Disabled {
    pub id: String,
    pub reason: String,
}

/// A capability with several installed providers; the user picks one (SPEC §6.3).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProviderChoice {
    pub capability: String,
    pub providers: Vec<String>,
    pub chosen: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolved {
    /// Active extensions in run order (language extensions first, then dependency order).
    pub active: Vec<String>,
    /// Active extensions that run code, ordered so providers precede their dependents.
    pub annotator_order: Vec<String>,
    pub decorator_order: Vec<String>,
    pub disabled: Vec<Disabled>,
    pub provider_choices: Vec<ProviderChoice>,
    pub fingerprint: String,
}

/// Capabilities an extension provides for `lang`, including the implicit ones of a language extension.
fn provided(m: &Manifest, lang: &str) -> BTreeSet<String> {
    let mut p: BTreeSet<String> = m.provides.iter().cloned().collect();
    if m.kind == Kind::Language {
        p.insert(format!("{lang}.tokenize"));
        p.insert(format!("{lang}.norm"));
    }
    p
}

fn expand(cap: &str, lang: &str) -> String {
    match cap.strip_prefix("*.") {
        Some(name) => format!("{lang}.{name}"),
        None => cap.to_string(),
    }
}

fn needs(m: &Manifest, lang: &str) -> BTreeSet<String> {
    m.requires.iter().map(|c| expand(c, lang)).collect()
}

fn decorator_priority(m: &Manifest) -> i64 {
    m.contributes.decorators.iter().map(|d| d.priority).max().unwrap_or(50)
}

pub fn resolve(input: &Input) -> Resolved {
    let lang = input.language;
    let host_version = Version::parse(API_VERSION).expect("API_VERSION");
    let mut disabled: Vec<Disabled> = Vec::new();
    let mut why = |id: &str, reason: String| disabled.push(Disabled { id: id.to_string(), reason });

    // Rule 1: user choice, language, platform, API version, runtime.
    let mut live: BTreeMap<&str, &Candidate> = BTreeMap::new();
    for c in &input.candidates {
        let m = c.manifest;
        let st = input.state.get(&m.id);
        if st.is_some_and(|s| !s.enabled) {
            continue;
        }
        if !m.targets_language(lang) {
            continue;
        }
        if m.kind != Kind::Language && st.is_some_and(|s| !s.languages.is_empty() && !s.languages.iter().any(|l| l == lang)) {
            continue;
        }
        if !m.available_on(input.platform) {
            why(&m.id, format!("not available on {}", input.platform));
            continue;
        }
        if !VersionReq::parse(&m.api_version).is_ok_and(|r| r.matches(&host_version)) {
            why(&m.id, format!("needs host API {}, this app provides {API_VERSION}", m.api_version));
            continue;
        }
        if m.runtime.ty == RuntimeType::Native {
            why(&m.id, "native extensions are not supported yet".into());
            continue;
        }
        live.insert(&m.id, c);
    }

    // Rule 3: one provider per capability.
    let mut by_cap: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for (id, c) in &live {
        for cap in provided(c.manifest, lang) {
            by_cap.entry(cap).or_default().push(id);
        }
    }
    let mut losers: BTreeMap<&str, String> = BTreeMap::new();
    let mut provider_choices = Vec::new();
    for (cap, ids) in &by_cap {
        if ids.len() < 2 {
            continue;
        }
        let chosen = input
            .providers
            .get(cap)
            .and_then(|want| ids.iter().find(|i| *i == want))
            .or_else(|| ids.iter().find(|i| live[**i].bundled))
            .unwrap_or(&ids[0]);
        provider_choices.push(ProviderChoice {
            capability: cap.clone(),
            providers: ids.iter().map(|i| i.to_string()).collect(),
            chosen: chosen.to_string(),
        });
        for id in ids.iter().filter(|i| *i != chosen) {
            losers.entry(id).or_insert_with(|| format!("another provider is selected for {cap}"));
        }
    }
    for (id, reason) in losers {
        live.remove(id);
        why(id, reason);
    }

    // Rules 4 and 5, repeated until stable: unmet requirements, then cycles.
    loop {
        let mut changed = false;
        loop {
            let have: BTreeSet<String> = live
                .values()
                .flat_map(|c| provided(c.manifest, lang))
                .chain(CORE_CAPABILITIES.iter().map(|s| s.to_string()))
                .collect();
            let unmet: Vec<(&str, String)> = live
                .iter()
                .filter_map(|(id, c)| needs(c.manifest, lang).into_iter().find(|n| !have.contains(n)).map(|n| (*id, n)))
                .collect();
            if unmet.is_empty() {
                break;
            }
            changed = true;
            for (id, cap) in unmet {
                live.remove(id);
                why(id, format!("requires {cap}, which no active extension provides"));
            }
        }
        let cyc = cycles(&live, lang);
        if !cyc.is_empty() {
            changed = true;
            for (id, path) in cyc {
                live.remove(id.as_str());
                why(&id, format!("dependency cycle: {path}"));
            }
        }
        if !changed {
            break;
        }
    }

    let order = topo_order(&live, lang);
    // Language extensions provide the base data every other extension builds on: run them first.
    let mut active: Vec<String> = order.iter().filter(|id| live[id.as_str()].manifest.kind == Kind::Language).cloned().collect();
    active.extend(order.iter().filter(|id| live[id.as_str()].manifest.kind != Kind::Language).cloned());
    let annotator_order: Vec<String> = order
        .iter()
        .filter(|id| matches!(live[id.as_str()].manifest.runtime.ty, RuntimeType::Js | RuntimeType::Wasm))
        .cloned()
        .collect();

    // Rule 6: decorators.
    let mut decorators: Vec<&str> = live
        .iter()
        .filter(|(_, c)| !c.manifest.contributes.decorators.is_empty())
        .map(|(id, _)| *id)
        .collect();
    decorators.sort_by(|a, b| {
        let rank = |id: &str| input.decorator_order.iter().position(|x| x == id).unwrap_or(usize::MAX);
        rank(a)
            .cmp(&rank(b))
            .then(decorator_priority(live[b].manifest).cmp(&decorator_priority(live[a].manifest)))
            .then(a.cmp(b))
    });

    disabled.sort_by(|a, b| a.id.cmp(&b.id));
    let fingerprint = fingerprint(&active, &live, input.state);
    Resolved {
        active,
        annotator_order,
        decorator_order: decorators.into_iter().map(String::from).collect(),
        disabled,
        provider_choices,
        fingerprint,
    }
}

/// Provider → dependent edges over required capabilities: `edges[a]` = extensions that require something `a` provides.
fn edges<'a>(live: &BTreeMap<&'a str, &Candidate>, lang: &str, with_optional: bool) -> BTreeMap<&'a str, BTreeSet<&'a str>> {
    let mut providers: HashMap<String, Vec<&str>> = HashMap::new();
    for (id, c) in live {
        for cap in provided(c.manifest, lang) {
            providers.entry(cap).or_default().push(id);
        }
    }
    let mut out: BTreeMap<&str, BTreeSet<&str>> = live.keys().map(|k| (*k, BTreeSet::new())).collect();
    for (id, c) in live {
        let mut wanted = needs(c.manifest, lang);
        if with_optional {
            wanted.extend(c.manifest.optional.iter().map(|o| expand(o, lang)));
        }
        for cap in wanted {
            for p in providers.get(&cap).into_iter().flatten().filter(|p| *p != id) {
                out.get_mut(p).expect("live").insert(id);
            }
        }
        // Self-dependency counts as a cycle.
        if needs(c.manifest, lang).iter().any(|n| provided(c.manifest, lang).contains(n)) {
            out.get_mut(id).expect("live").insert(id);
        }
    }
    out
}

/// Members of every dependency cycle, with a readable path (`a → b → a`) starting at the smallest id.
fn cycles(live: &BTreeMap<&str, &Candidate>, lang: &str) -> Vec<(String, String)> {
    let g = edges(live, lang, false);
    let mut result = Vec::new();
    for start in g.keys() {
        // Shortest path from `start` back to itself.
        let mut prev: HashMap<&str, &str> = HashMap::new();
        let mut queue = std::collections::VecDeque::from([*start]);
        let mut found = false;
        'bfs: while let Some(n) = queue.pop_front() {
            for next in &g[n] {
                if next == start {
                    prev.insert("<end>", n);
                    found = true;
                    break 'bfs;
                }
                if !prev.contains_key(next) {
                    prev.insert(next, n);
                    queue.push_back(next);
                }
            }
        }
        if found {
            let mut path = vec![*start];
            let mut cur = prev["<end>"];
            let mut rev = vec![];
            while cur != *start {
                rev.push(cur);
                cur = prev[cur];
            }
            path.extend(rev.into_iter().rev());
            path.push(*start);
            result.push((start.to_string(), path.join(" → ")));
        }
    }
    result
}

/// Kahn's algorithm, ties broken by id. Optional dependencies add ordering edges only if they don't make a cycle.
fn topo_order(live: &BTreeMap<&str, &Candidate>, lang: &str) -> Vec<String> {
    let order = |g: &BTreeMap<&str, BTreeSet<&str>>| -> Option<Vec<String>> {
        let mut indeg: BTreeMap<&str, usize> = g.keys().map(|k| (*k, 0)).collect();
        for tos in g.values() {
            for t in tos {
                *indeg.get_mut(t).expect("node") += 1;
            }
        }
        let mut ready: BTreeSet<&str> = indeg.iter().filter(|(_, d)| **d == 0).map(|(k, _)| *k).collect();
        let mut out = Vec::new();
        while let Some(n) = ready.pop_first() {
            out.push(n.to_string());
            for t in &g[n] {
                let d = indeg.get_mut(t).expect("node");
                *d -= 1;
                if *d == 0 {
                    ready.insert(t);
                }
            }
        }
        (out.len() == g.len()).then_some(out)
    };
    order(&edges(live, lang, true))
        .or_else(|| order(&edges(live, lang, false)))
        .unwrap_or_else(|| live.keys().map(|k| k.to_string()).collect())
}

/// FNV-1a 64 over the active set; stable across Rust versions (unlike `DefaultHasher`).
fn fingerprint(active: &[String], live: &BTreeMap<&str, &Candidate>, state: &HashMap<String, UserState>) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut feed = |s: &str| {
        for b in s.bytes().chain([0]) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100000001b3);
        }
    };
    for id in active {
        feed(id);
        feed(&live[id.as_str()].manifest.version);
        feed(&state.get(id).map(|s| s.settings.to_string()).unwrap_or_default());
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ext(id: &str, f: impl FnOnce(&mut serde_json::Value)) -> Manifest {
        let mut v = json!({
            "id": id, "name": id, "version": "1.0.0", "kind": "addon", "license": "MIT",
            "apiVersion": "^1.0", "targets": ["*"], "runtime": { "type": "js", "entry": "m.js" }
        });
        f(&mut v);
        Manifest::parse(&v.to_string()).unwrap()
    }

    fn lang_es() -> Manifest {
        Manifest::parse(
            &json!({
                "id": "lang.es", "name": "es", "version": "1.0.0", "kind": "language", "license": "MIT",
                "language": "es", "apiVersion": "^1.0", "runtime": { "type": "declarative" },
                "provides": ["es.tokenize"], "declarative": {}
            })
            .to_string(),
        )
        .unwrap()
    }

    fn run(ms: &[Manifest], lang: &str, tweak: impl FnOnce(&mut TestCfg)) -> Resolved {
        let mut cfg = TestCfg::default();
        tweak(&mut cfg);
        let candidates = ms.iter().map(|m| Candidate { manifest: m, bundled: cfg.bundled.contains(&m.id) }).collect();
        resolve(&Input {
            language: lang,
            platform: &cfg.platform,
            candidates,
            state: &cfg.state,
            providers: &cfg.providers,
            decorator_order: &cfg.decorator_order,
        })
    }

    struct TestCfg {
        platform: String,
        state: HashMap<String, UserState>,
        providers: BTreeMap<String, String>,
        decorator_order: Vec<String>,
        bundled: Vec<String>,
    }
    impl Default for TestCfg {
        fn default() -> Self {
            Self {
                platform: "macos".into(),
                state: HashMap::new(),
                providers: BTreeMap::new(),
                decorator_order: vec![],
                bundled: vec![],
            }
        }
    }

    fn reason<'a>(r: &'a Resolved, id: &str) -> &'a str {
        &r.disabled.iter().find(|d| d.id == id).unwrap_or_else(|| panic!("{id} not disabled: {:?}", r.disabled)).reason
    }

    #[test]
    fn language_and_wildcard_requires() {
        let ms = [lang_es(), ext("a", |v| v["requires"] = json!(["*.tokenize"]))];
        let r = run(&ms, "es", |_| {});
        assert_eq!(r.active, vec!["lang.es", "a"]);
        let r = run(&ms, "fr", |_| {});
        assert!(r.active.is_empty(), "language ext doesn't apply to fr; a is dropped for unmet requires: {r:?}");
        assert!(reason(&r, "a").contains("fr.tokenize"));
    }

    #[test]
    fn targets_and_user_languages_filter() {
        let ms = [ext("only-es", |v| v["targets"] = json!(["es"])), ext("all", |_| {})];
        assert_eq!(run(&ms, "fr", |_| {}).active, vec!["all"]);
        let r = run(&ms, "es", |c| {
            c.state.insert("all".into(), UserState { languages: vec!["fr".into()], ..Default::default() });
        });
        assert_eq!(r.active, vec!["only-es"]);
        let r = run(&ms, "es", |c| {
            c.state.insert("all".into(), UserState { enabled: false, ..Default::default() });
        });
        assert_eq!(r.active, vec!["only-es"]);
    }

    #[test]
    fn platform_and_api_version_filters() {
        let ms = [
            ext("desk", |v| v["platforms"] = json!(["windows", "macos", "linux"])),
            ext("future", |v| v["apiVersion"] = json!("^2.0")),
            ext("ok", |_| {}),
        ];
        let r = run(&ms, "es", |c| c.platform = "android".into());
        assert_eq!(r.active, vec!["ok"]);
        assert_eq!(reason(&r, "desk"), "not available on android");
        assert!(reason(&r, "future").contains("^2.0"));
        assert_eq!(run(&ms, "es", |_| {}).active, vec!["desk", "ok"]);
    }

    #[test]
    fn provider_choice_user_then_bundled_then_id() {
        let ms = [
            ext("p.b", |v| v["provides"] = json!(["es.pos"])),
            ext("p.a", |v| v["provides"] = json!(["es.pos"])),
            ext("p.c", |v| v["provides"] = json!(["es.pos"])),
        ];
        assert_eq!(run(&ms, "es", |_| {}).active, vec!["p.a"], "first by id");
        assert_eq!(run(&ms, "es", |c| c.bundled = vec!["p.c".into()]).active, vec!["p.c"], "bundled beats id");
        let r = run(&ms, "es", |c| {
            c.bundled = vec!["p.c".into()];
            c.providers.insert("es.pos".into(), "p.b".into());
        });
        assert_eq!(r.active, vec!["p.b"], "user beats bundled");
        assert!(reason(&r, "p.a").contains("another provider is selected"));
        assert_eq!(
            r.provider_choices,
            vec![ProviderChoice { capability: "es.pos".into(), providers: vec!["p.a".into(), "p.b".into(), "p.c".into()], chosen: "p.b".into() }]
        );
        // A stale user choice (not installed) falls back.
        let r = run(&ms, "es", |c| _ = c.providers.insert("es.pos".into(), "gone".into()));
        assert_eq!(r.active, vec!["p.a"]);
    }

    #[test]
    fn unmet_requires_cascade() {
        let ms = [
            ext("a", |v| v["requires"] = json!(["es.missing"])),
            ext("b", |v| {
                v["requires"] = json!(["es.froma"]);
                v["provides"] = json!(["es.fromb"]);
            }),
            ext("c", |v| v["requires"] = json!(["es.fromb"])),
            ext("d", |v| v["optional"] = json!(["es.froma", "es.nothing"])),
        ];
        let mut ms = Vec::from(ms);
        ms[0].provides = vec!["es.froma".into()];
        let r = run(&ms, "es", |_| {});
        assert_eq!(r.active, vec!["d"], "optional never disables");
        assert!(reason(&r, "a").contains("es.missing"));
        assert!(reason(&r, "b").contains("es.froma"));
        assert!(reason(&r, "c").contains("es.fromb"));
    }

    #[test]
    fn core_capabilities_satisfy_requires() {
        let ms = [ext("a", |v| v["requires"] = json!(["core.dict", "core.net"])), ext("b", |v| v["requires"] = json!(["core.nope"]))];
        assert_eq!(run(&ms, "es", |_| {}).active, vec!["a"]);
    }

    #[test]
    fn cycles_disable_every_member_with_a_path() {
        let ms = [
            ext("a", |v| {
                v["provides"] = json!(["es.a"]);
                v["requires"] = json!(["es.b"]);
            }),
            ext("b", |v| {
                v["provides"] = json!(["es.b"]);
                v["requires"] = json!(["es.a"]);
            }),
            ext("c", |v| v["requires"] = json!(["es.a"])),
            ext("free", |_| {}),
        ];
        let r = run(&ms, "es", |_| {});
        assert_eq!(r.active, vec!["free"]);
        assert_eq!(reason(&r, "a"), "dependency cycle: a → b → a");
        assert_eq!(reason(&r, "b"), "dependency cycle: b → a → b");
        assert!(reason(&r, "c").contains("requires es.a"));
        // A self-requirement is a cycle too.
        let s = [ext("s", |v| {
            v["provides"] = json!(["es.s"]);
            v["requires"] = json!(["es.s"]);
        })];
        assert!(run(&s, "es", |_| {}).active.is_empty());
    }

    #[test]
    fn order_is_topological_and_deterministic() {
        let ms = [
            ext("z.base", |v| v["provides"] = json!(["es.base"])),
            ext("a.top", |v| v["requires"] = json!(["es.mid"])),
            ext("m.mid", |v| {
                v["provides"] = json!(["es.mid"]);
                v["requires"] = json!(["es.base"]);
            }),
            ext("b.free", |_| {}),
        ];
        let r = run(&ms, "es", |_| {});
        assert_eq!(r.active, vec!["b.free", "z.base", "m.mid", "a.top"]);
        assert_eq!(r.annotator_order, r.active);
        let mut rev = Vec::from(ms);
        rev.reverse();
        assert_eq!(run(&rev, "es", |_| {}), r, "input order doesn't matter");
    }

    #[test]
    fn optional_orders_when_possible_and_is_ignored_in_a_cycle() {
        let ms = [
            ext("a.early", |v| v["optional"] = json!(["es.late"])),
            ext("z.late", |v| v["provides"] = json!(["es.late"])),
        ];
        assert_eq!(run(&ms, "es", |_| {}).active, vec!["z.late", "a.early"]);
        let ms = [
            ext("a", |v| {
                v["provides"] = json!(["es.a"]);
                v["optional"] = json!(["es.b"]);
            }),
            ext("b", |v| {
                v["provides"] = json!(["es.b"]);
                v["requires"] = json!(["es.a"]);
            }),
        ];
        assert_eq!(run(&ms, "es", |_| {}).active, vec!["a", "b"], "required edge wins");
    }

    #[test]
    fn language_extensions_run_first_and_code_only_extensions_annotate() {
        let ms = [ext("a", |_| {}), lang_es()];
        let r = run(&ms, "es", |_| {});
        assert_eq!(r.active, vec!["lang.es", "a"]);
        assert_eq!(r.annotator_order, vec!["a"], "declarative has no worker");
    }

    #[test]
    fn decorator_order_user_then_priority_then_id() {
        let dec = |p: i64| move |v: &mut serde_json::Value| v["contributes"] = json!({ "decorators": [{ "id": "d", "priority": p }] });
        let ms = [ext("c", dec(10)), ext("b", dec(90)), ext("a", dec(90)), ext("plain", |_| {})];
        assert_eq!(run(&ms, "es", |_| {}).decorator_order, vec!["a", "b", "c"]);
        let r = run(&ms, "es", |c| c.decorator_order = vec!["c".into(), "gone".into(), "b".into()]);
        assert_eq!(r.decorator_order, vec!["c", "b", "a"]);
    }

    #[test]
    fn fingerprint_changes_with_set_version_and_settings() {
        let ms = [ext("a", |_| {}), ext("b", |_| {})];
        let base = run(&ms, "es", |_| {}).fingerprint;
        assert_eq!(base, run(&ms, "es", |_| {}).fingerprint);
        assert_ne!(base, run(&ms, "es", |c| _ = c.state.insert("b".into(), UserState { enabled: false, ..Default::default() })).fingerprint);
        assert_ne!(
            base,
            run(&ms, "es", |c| _ = c.state.insert("a".into(), UserState { settings: json!({ "x": 1 }), ..Default::default() })).fingerprint
        );
        let newer = [ext("a", |v| v["version"] = json!("1.0.1")), ext("b", |_| {})];
        assert_ne!(base, run(&newer, "es", |_| {}).fingerprint);
    }

    #[test]
    fn native_runtime_is_disabled_with_a_reason() {
        let ms = [ext("n", |v| {
            v["runtime"] = json!({ "type": "native", "entry": "lib.so" });
            v["platforms"] = json!(["linux"]);
        })];
        let r = run(&ms, "es", |c| c.platform = "linux".into());
        assert!(reason(&r, "n").contains("native"));
    }
}
