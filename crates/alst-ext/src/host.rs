//! The host side of the extension API (SPEC §6.9.4, §6.9.7): every call from an extension is
//! routed through [`call`], which looks up the extension's manifest and checks its permissions.
//! The UI host tags each call with the worker's extension id; extensions can't set it themselves.

use crate::manifest::{Contributes, Kind, Manifest, RuntimeType, settings_with_defaults};
use crate::net::{FetchRequest, fetch};
use crate::perm::{Need, Permission, allows};
use crate::registry::{Registry, Source};
use crate::resolve::{Candidate, Input, Resolved, UserState, resolve};
use crate::{Error, Result, STORAGE_MAX_KEYS, STORAGE_MAX_VALUE_BYTES};
use alst_core::{Store, TermPatch};
use alst_text::LanguageDef;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

/// Name of this platform in manifests' `platforms`.
pub fn platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        "android" => "android",
        "ios" => "ios",
        _ => "linux",
    }
}

fn core_err(e: alst_core::Error) -> Error {
    Error::Invalid(e.to_string())
}

// ---- views for the UI ------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct PermInfo {
    pub id: String,
    pub text: String,
    pub warning: bool,
}

pub fn perm_infos(m: &Manifest) -> Vec<PermInfo> {
    m.permissions
        .iter()
        .map(|p| PermInfo { id: p.clone(), text: Permission::describe(p), warning: Permission::is_warning(p) })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub kind: Kind,
    pub description: Option<String>,
    pub author: Option<String>,
    pub license: String,
    pub source: Source,
    pub runtime: RuntimeType,
    pub entry: Option<String>,
    pub targets: Vec<String>,
    pub language: Option<String>,
    pub provides: Vec<String>,
    pub requires: Vec<String>,
    pub optional: Vec<String>,
    pub permissions: Vec<PermInfo>,
    pub platforms: Option<Vec<String>>,
    pub contributes: Contributes,
    /// Declared contribution kinds this version of the host ignores.
    pub unsupported: Vec<&'static str>,
    pub enabled: bool,
    pub languages: Vec<String>,
    /// Stored settings merged over the schema defaults.
    pub settings: Value,
}

pub fn ext_info(m: &Manifest, source: Source, state: Option<&UserState>) -> ExtInfo {
    let default = UserState::default();
    let st = state.unwrap_or(&default);
    ExtInfo {
        id: m.id.clone(),
        name: m.name.clone(),
        version: m.version.clone(),
        kind: m.kind,
        description: m.description.clone(),
        author: m.author.clone(),
        license: m.license.clone(),
        source,
        runtime: m.runtime.ty,
        entry: m.runtime.entry.clone(),
        targets: m.targets.clone(),
        language: m.language.clone(),
        provides: m.provides.clone(),
        requires: m.requires.clone(),
        optional: m.optional.clone(),
        permissions: perm_infos(m),
        platforms: m.platforms.clone(),
        contributes: m.contributes.clone(),
        unsupported: m.contributes.unsupported(),
        enabled: st.enabled,
        languages: st.languages.clone(),
        settings: settings_with_defaults(m, &st.settings),
    }
}

pub fn user_states(store: &Store) -> Result<HashMap<String, UserState>> {
    Ok(store
        .ext_states()
        .map_err(core_err)?
        .into_iter()
        .map(|r| (r.id, UserState { enabled: r.enabled, languages: r.languages, settings: r.settings }))
        .collect())
}

#[derive(Debug, Serialize)]
pub struct ExtensionList {
    pub extensions: Vec<ExtInfo>,
    pub errors: Vec<crate::LoadError>,
    pub platform: &'static str,
}

pub fn list(registry: &Registry, store: &Store) -> Result<ExtensionList> {
    let states = user_states(store)?;
    Ok(ExtensionList {
        extensions: registry.all().map(|e| ext_info(&e.manifest, e.source, states.get(&e.manifest.id))).collect(),
        errors: registry.errors.clone(),
        platform: platform(),
    })
}

// ---- resolution ------------------------------------------------------------------------------

fn providers_key(lang: &str) -> String {
    format!("ext.providers.{lang}")
}

fn decorator_key(lang: &str) -> String {
    format!("ext.decoratorOrder.{lang}")
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedView {
    pub language: String,
    /// Active extensions in run order.
    pub active: Vec<ExtInfo>,
    pub annotator_order: Vec<String>,
    pub decorator_order: Vec<String>,
    pub disabled: Vec<DisabledView>,
    pub provider_choices: Vec<crate::resolve::ProviderChoice>,
    pub fingerprint: String,
}

#[derive(Debug, Serialize)]
pub struct DisabledView {
    pub id: String,
    pub name: String,
    pub reason: String,
}

pub fn resolve_language(registry: &Registry, store: &Store, language: &str, platform: &str) -> Result<(Resolved, ResolvedView)> {
    let states = user_states(store)?;
    let providers: BTreeMap<String, String> = store
        .get_setting(&providers_key(language))
        .map_err(core_err)?
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    let order: Vec<String> = store
        .get_setting(&decorator_key(language))
        .map_err(core_err)?
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    let candidates =
        registry.all().map(|e| Candidate { manifest: &e.manifest, bundled: e.source == Source::Bundled }).collect();
    let r = resolve(&Input { language, platform, candidates, state: &states, providers: &providers, decorator_order: &order });
    let info = |id: &str| registry.get(id).map(|e| ext_info(&e.manifest, e.source, states.get(id)));
    let view = ResolvedView {
        language: language.to_string(),
        active: r.active.iter().filter_map(|id| info(id)).collect(),
        annotator_order: r.annotator_order.clone(),
        decorator_order: r.decorator_order.clone(),
        disabled: r
            .disabled
            .iter()
            .map(|d| DisabledView {
                id: d.id.clone(),
                name: registry.get(&d.id).map_or_else(|| d.id.clone(), |e| e.manifest.name.clone()),
                reason: d.reason.clone(),
            })
            .collect(),
        provider_choices: r.provider_choices.clone(),
        fingerprint: r.fingerprint.clone(),
    };
    Ok((r, view))
}

/// Languages from active-or-not language extensions: the registry is the source of truth for what is installed.
pub fn language_defs(registry: &Registry, store: &Store) -> Result<Vec<LanguageDef>> {
    let states = user_states(store)?;
    registry
        .all()
        .filter(|e| e.manifest.kind == Kind::Language && states.get(&e.manifest.id).is_none_or(|s| s.enabled))
        .map(|e| LanguageDef::from_manifest(&e.raw).map_err(|err| Error::Manifest(format!("{}: {err}", e.manifest.id))))
        .collect()
}

pub fn set_provider(registry: &Registry, store: &mut Store, language: &str, capability: &str, ext_id: &str) -> Result<()> {
    let ext = registry.get(ext_id).ok_or_else(|| Error::Invalid(format!("no extension '{ext_id}'")))?;
    let lang_provides = |c: &str| {
        ext.manifest.provides.iter().any(|p| p == c) || (ext.manifest.kind == Kind::Language && [format!("{language}.tokenize"), format!("{language}.norm")].contains(&c.to_string()))
    };
    if !lang_provides(capability) {
        return Err(Error::Invalid(format!("'{ext_id}' doesn't provide {capability}")));
    }
    let key = providers_key(language);
    let mut map: BTreeMap<String, String> =
        store.get_setting(&key).map_err(core_err)?.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    map.insert(capability.to_string(), ext_id.to_string());
    store.set_setting(&key, &json!(map)).map_err(core_err)
}

pub fn set_decorator_order(store: &mut Store, language: &str, ids: &[String]) -> Result<()> {
    store.set_setting(&decorator_key(language), &json!(ids)).map_err(core_err)
}

// ---- host API calls --------------------------------------------------------------------------

pub struct HostCtx<'a> {
    pub registry: &'a Registry,
    pub store: &'a mut Store,
    pub languages: &'a [LanguageDef],
}

fn params<T: for<'de> Deserialize<'de>>(p: &Value) -> Result<T> {
    serde_json::from_value(p.clone()).map_err(|e| Error::Invalid(format!("bad parameters: {e}")))
}

fn need(m: &Manifest, n: Need, what: &str) -> Result<()> {
    if allows(m, &n) {
        Ok(())
    } else {
        Err(Error::Permission(format!("'{}' needs the {what} permission", m.id)))
    }
}

fn manifest_of<'a>(registry: &'a Registry, id: &str) -> Result<&'a Manifest> {
    registry.get(id).map(|e| &e.manifest).ok_or_else(|| Error::Invalid(format!("unknown extension '{id}'")))
}

/// Check that the extension exists and is enabled; returns its manifest.
fn active_manifest<'a>(registry: &'a Registry, store: &Store, id: &str) -> Result<&'a Manifest> {
    let m = manifest_of(registry, id)?;
    if !user_states(store)?.get(id).is_none_or(|s| s.enabled) {
        return Err(Error::Permission(format!("extension '{id}' is disabled")));
    }
    Ok(m)
}

/// Methods that don't touch the store and may be slow: the shell runs these off the main lock.
pub fn is_net_call(method: &str) -> bool {
    method == "net.fetch"
}

/// `alst.net.fetch` for `ext_id` (blocking; call from a worker thread).
pub fn net_fetch(manifest: &Manifest, p: &Value) -> Result<Value> {
    let req: FetchRequest = params(p)?;
    serde_json::to_value(fetch(manifest, &req)?).map_err(|e| Error::Net(e.to_string()))
}

pub fn manifest_for_call(registry: &Registry, store: &Store, ext_id: &str) -> Result<Manifest> {
    active_manifest(registry, store, ext_id).cloned()
}

pub fn call(ctx: &mut HostCtx, ext_id: &str, method: &str, p: &Value) -> Result<Value> {
    let m = active_manifest(ctx.registry, ctx.store, ext_id)?.clone();
    match method {
        "settings.get" => {
            let states = user_states(ctx.store)?;
            let stored = states.get(ext_id).map(|s| s.settings.clone()).unwrap_or_else(|| json!({}));
            Ok(settings_with_defaults(&m, &stored))
        }
        "storage.get" => {
            let k: KeyParam = params(p)?;
            Ok(ctx.store.ext_data_get(ext_id, &k.key).map_err(core_err)?.unwrap_or(Value::Null))
        }
        "storage.keys" => Ok(json!(ctx.store.ext_data_keys(ext_id).map_err(core_err)?)),
        "storage.set" => {
            #[derive(Deserialize)]
            struct P {
                key: String,
                value: Value,
            }
            let P { key, value } = params(p)?;
            check_key(&key)?;
            if value.to_string().len() > STORAGE_MAX_VALUE_BYTES {
                return Err(Error::Invalid(format!("storage value for '{key}' exceeds {} KB", STORAGE_MAX_VALUE_BYTES / 1024)));
            }
            let keys = ctx.store.ext_data_keys(ext_id).map_err(core_err)?;
            if keys.len() >= STORAGE_MAX_KEYS && !keys.contains(&key) {
                return Err(Error::Invalid(format!("storage is limited to {STORAGE_MAX_KEYS} keys")));
            }
            ctx.store.ext_data_set(ext_id, &key, &value).map_err(core_err)?;
            Ok(Value::Null)
        }
        "storage.delete" => {
            let k: KeyParam = params(p)?;
            Ok(json!(ctx.store.ext_data_delete(ext_id, &k.key).map_err(core_err)?))
        }
        "terms.get" => {
            need(&m, Need::TermsRead, "terms:read")?;
            #[derive(Deserialize)]
            struct P {
                language: String,
                ids: Option<Vec<String>>,
                texts: Option<Vec<String>>,
            }
            let P { language, ids, texts } = params(p)?;
            let mut out = Vec::new();
            for id in ids.unwrap_or_default().iter().take(500) {
                if let Ok(t) = ctx.store.get_term(id) {
                    out.push(t);
                }
            }
            if let Some(texts) = texts {
                let lang = lang_of(ctx.languages, &language)?;
                for text in texts.iter().take(500) {
                    if let Some((key, _)) = lang.term_key(text) {
                        if let Some(t) = ctx.store.find_term(&language, &key).map_err(core_err)? {
                            out.push(t);
                        }
                    }
                }
            }
            Ok(json!(out))
        }
        "terms.search" => {
            need(&m, Need::TermsRead, "terms:read")?;
            #[derive(Deserialize)]
            struct P {
                language: String,
                #[serde(default)]
                query: String,
                status: Option<i64>,
            }
            let P { language, query, status } = params(p)?;
            Ok(json!(ctx.store.search_terms(&language, &query, status, 100).map_err(core_err)?))
        }
        "terms.update" => {
            need(&m, Need::TermsWrite, "terms:write")?;
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Patch {
                status: Option<i64>,
                translation: Option<String>,
                notes: Option<String>,
                reading: Option<String>,
            }
            #[derive(Deserialize)]
            struct P {
                id: String,
                patch: Patch,
            }
            let P { id, patch } = params(p)?;
            let t = ctx.store.get_term(&id).map_err(core_err)?;
            let patch = TermPatch { status: patch.status, translation: patch.translation, notes: patch.notes, reading: patch.reading, lemma: None };
            Ok(json!(ctx.store.save_term(&t.language, &t.text, &t.text_lc, t.word_count, &patch).map_err(core_err)?))
        }
        "events.emit" => {
            #[derive(Deserialize)]
            struct P {
                #[serde(rename = "type")]
                ty: String,
                #[serde(default)]
                payload: Value,
                language: Option<String>,
            }
            let P { ty, payload, language } = params(p)?;
            if ty.is_empty() || ty.len() > 64 || !ty.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) {
                return Err(Error::Invalid(format!("invalid event type '{ty}'")));
            }
            if payload.to_string().len() > STORAGE_MAX_VALUE_BYTES {
                return Err(Error::Invalid("event payload is too large".into()));
            }
            ctx.store.emit_event(&format!("{ext_id}/{ty}"), language.as_deref(), None, &payload).map_err(core_err)?;
            Ok(Value::Null)
        }
        "events.query" => {
            #[derive(Deserialize)]
            struct P {
                #[serde(rename = "type")]
                ty: Option<String>,
                since: Option<i64>,
                limit: Option<usize>,
            }
            let P { ty, since, limit } = params(p)?;
            let own = format!("{ext_id}/");
            let ty = match ty {
                Some(t) if t.starts_with(&own) => Some(t),
                Some(t) => {
                    need(&m, Need::EventsRead, "events:read")?;
                    Some(t)
                }
                None if allows(&m, &Need::EventsRead) => None,
                None => Some(format!("{own}*")),
            };
            Ok(json!(ctx.store.events_query(ty.as_deref(), since, limit.unwrap_or(100).min(1000)).map_err(core_err)?))
        }
        "dict.lookup" => {
            need(&m, Need::DictRead, "dict:read")?;
            #[derive(Deserialize)]
            struct P {
                language: String,
                query: String,
            }
            let P { language, query } = params(p)?;
            let key = lang_of(ctx.languages, &language).ok().and_then(|l| l.term_key(&query)).map_or(query, |(k, _)| k);
            Ok(json!(alst_dict::lookup(ctx.store.local_conn(), &language, &key, 30).map_err(|e| Error::Invalid(e.to_string()))?))
        }
        other => Err(Error::Invalid(format!("unknown host method '{other}'"))),
    }
}

#[derive(Deserialize)]
struct KeyParam {
    key: String,
}

fn check_key(key: &str) -> Result<()> {
    if key.is_empty() || key.len() > 256 {
        Err(Error::Invalid("storage keys are 1-256 characters".into()))
    } else {
        Ok(())
    }
}

fn lang_of<'a>(languages: &'a [LanguageDef], id: &str) -> Result<&'a LanguageDef> {
    languages.iter().find(|l| l.id == id).ok_or_else(|| Error::Invalid(format!("language '{id}' is not installed")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::{install, read_package, tests::zip_of};
    use crate::manifest::validate_settings;
    use crate::registry::BUNDLED;
    use std::path::PathBuf;

    struct Fixture {
        root: PathBuf,
        registry: Registry,
        store: Store,
        languages: Vec<LanguageDef>,
    }

    impl Fixture {
        fn new(tag: &str, exts: &[(&str, Value)]) -> Self {
            let root = std::env::temp_dir().join(format!("alst-host-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            for (id, extra) in exts {
                let mut m = json!({
                    "id": id, "name": id, "version": "1.0.0", "kind": "addon", "license": "MIT",
                    "apiVersion": "^1.0", "targets": ["*"], "runtime": { "type": "js", "entry": "main.js" }
                });
                for (k, v) in extra.as_object().unwrap() {
                    m[k] = v.clone();
                }
                let bytes = zip_of(&[("extension.json", m.to_string().as_bytes()), ("main.js", b"")]);
                install(&root, &read_package(&bytes).unwrap()).unwrap();
            }
            let registry = Registry::load(&BUNDLED, Some(&root));
            let store = Store::open_in_memory().unwrap();
            let languages = language_defs(&registry, &store).unwrap();
            Self { root, registry, store, languages }
        }

        fn call(&mut self, ext: &str, method: &str, p: Value) -> Result<Value> {
            call(&mut HostCtx { registry: &self.registry, store: &mut self.store, languages: &self.languages }, ext, method, &p)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn denied(r: Result<Value>) -> bool {
        matches!(r, Err(Error::Permission(_)))
    }

    #[test]
    fn permissions_are_enforced_per_method() {
        let mut f = Fixture::new(
            "perms",
            &[
                ("a.none", json!({})),
                ("a.read", json!({ "permissions": ["terms:read", "events:read", "dict:read"] })),
                ("a.write", json!({ "permissions": ["terms:write"] })),
            ],
        );
        f.store.save_term("es", "gato", "gato", 1, &TermPatch { status: Some(2), ..Default::default() }).unwrap();
        let id = f.store.find_term("es", "gato").unwrap().unwrap().id;

        for (m, p) in [
            ("terms.get", json!({ "language": "es", "texts": ["gato"] })),
            ("terms.search", json!({ "language": "es" })),
            ("terms.update", json!({ "id": id, "patch": { "status": 3 } })),
            ("dict.lookup", json!({ "language": "es", "query": "gato" })),
            ("events.query", json!({ "type": "term.status" })),
        ] {
            assert!(denied(f.call("a.none", m, p.clone())), "a.none {m}");
        }
        assert_eq!(f.call("a.read", "terms.get", json!({ "language": "es", "texts": ["GATO"] })).unwrap()[0]["status"], 2);
        assert_eq!(f.call("a.read", "terms.search", json!({ "language": "es", "query": "ga" })).unwrap().as_array().unwrap().len(), 1);
        assert!(f.call("a.read", "dict.lookup", json!({ "language": "es", "query": "gato" })).unwrap().as_array().unwrap().is_empty());
        assert_eq!(f.call("a.read", "events.query", json!({ "type": "term.status" })).unwrap().as_array().unwrap().len(), 1);
        assert!(denied(f.call("a.read", "terms.update", json!({ "id": id, "patch": { "status": 3 } }))));
        let updated = f.call("a.write", "terms.update", json!({ "id": id, "patch": { "status": 3, "translation": "cat" } })).unwrap();
        assert_eq!((updated["status"].clone(), updated["translation"].clone()), (json!(3), json!("cat")));
        assert!(f.call("a.write", "terms.update", json!({ "id": id, "patch": { "lemma": "x" } })).is_err(), "unknown patch fields rejected");
        assert!(f.call("a.write", "terms.update", json!({ "id": id, "patch": { "status": 7 } })).is_err());
        assert!(f.call("a.write", "terms.get", json!({ "language": "es", "ids": [id] })).is_ok(), "write implies read");
    }

    #[test]
    fn storage_is_namespaced_and_limited() {
        let mut f = Fixture::new("storage", &[("a.one", json!({})), ("a.two", json!({}))]);
        f.call("a.one", "storage.set", json!({ "key": "k", "value": { "n": 1 } })).unwrap();
        assert_eq!(f.call("a.one", "storage.get", json!({ "key": "k" })).unwrap(), json!({ "n": 1 }));
        assert_eq!(f.call("a.two", "storage.get", json!({ "key": "k" })).unwrap(), Value::Null);
        assert_eq!(f.call("a.one", "storage.keys", json!({})).unwrap(), json!(["k"]));
        assert_eq!(f.call("a.one", "storage.delete", json!({ "key": "k" })).unwrap(), json!(true));
        assert_eq!(f.call("a.one", "storage.get", json!({ "key": "k" })).unwrap(), Value::Null);
        let big = "x".repeat(STORAGE_MAX_VALUE_BYTES);
        assert!(f.call("a.one", "storage.set", json!({ "key": "big", "value": big })).is_err());
        for i in 0..STORAGE_MAX_KEYS {
            f.store.ext_data_set("a.one", &format!("k{i}"), &json!(i)).unwrap();
        }
        assert!(f.call("a.one", "storage.set", json!({ "key": "one-too-many", "value": 1 })).is_err());
        assert!(f.call("a.one", "storage.set", json!({ "key": "k5", "value": 1 })).is_ok(), "overwriting is fine at the cap");
        assert!(f.call("a.one", "storage.set", json!({ "key": "", "value": 1 })).is_err());
    }

    #[test]
    fn events_are_prefixed_and_own_events_always_readable() {
        let mut f = Fixture::new("events", &[("a.none", json!({})), ("a.read", json!({ "permissions": ["events:read"] }))]);
        f.call("a.none", "events.emit", json!({ "type": "seen", "payload": { "x": 1 } })).unwrap();
        assert!(f.call("a.none", "events.emit", json!({ "type": "bad type" })).is_err());
        let own = f.call("a.none", "events.query", json!({ "type": "a.none/seen" })).unwrap();
        assert_eq!(own.as_array().unwrap().len(), 1);
        assert_eq!(own[0]["type"], "a.none/seen");
        f.store.emit_event("part.read", Some("es"), None, &json!({})).unwrap();
        assert!(denied(f.call("a.none", "events.query", json!({ "type": "part.read" }))));
        assert!(denied(f.call("a.none", "events.query", json!({ "type": "a.read/seen" }))), "not another extension's");
        let unfiltered = f.call("a.none", "events.query", json!({})).unwrap();
        assert_eq!(unfiltered.as_array().unwrap().len(), 1, "without the permission, no type means own events only");
        assert_eq!(f.call("a.read", "events.query", json!({})).unwrap().as_array().unwrap().len(), 2);
    }

    #[test]
    fn unknown_disabled_and_unknown_methods_are_refused() {
        let mut f = Fixture::new("refuse", &[("a.one", json!({}))]);
        assert!(f.call("nobody", "storage.keys", json!({})).is_err());
        assert!(f.call("a.one", "storage.explode", json!({})).is_err());
        assert!(f.call("a.one", "storage.get", json!({ "wrong": 1 })).is_err());
        f.store.set_ext_state("a.one", Some(false), None, None).unwrap();
        assert!(denied(f.call("a.one", "storage.keys", json!({}))), "disabled extensions can't call the host");
    }

    #[test]
    fn settings_get_merges_defaults() {
        let schema = json!({ "contributes": { "settings": { "type": "object", "properties": { "n": { "type": "number", "default": 3 } } } } });
        let mut f = Fixture::new("settings", &[("a.s", schema)]);
        assert_eq!(f.call("a.s", "settings.get", json!({})).unwrap(), json!({ "n": 3 }));
        f.store.set_ext_state("a.s", None, Some(&json!({ "n": 9 })), None).unwrap();
        assert_eq!(f.call("a.s", "settings.get", json!({})).unwrap(), json!({ "n": 9 }));
        assert!(validate_settings(&f.registry.get("a.s").unwrap().manifest, &json!({ "n": "x" })).is_err());
    }

    #[test]
    fn resolve_uses_stored_state_providers_and_order() {
        let provides = |c: &str| json!({ "provides": [c] });
        let mut f = Fixture::new("resolve", &[("a.p1", provides("es.pos")), ("a.p2", provides("es.pos"))]);
        // Bundled first-party extensions are active too; look only at the fixtures.
        let mine = |r: &Resolved| r.active.iter().filter(|id| id.as_str() == "lang.es" || id.starts_with("a.")).cloned().collect::<Vec<_>>();
        let (r, view) = resolve_language(&f.registry, &f.store, "es", "macos").unwrap();
        assert_eq!(mine(&r), vec!["lang.es", "a.p1"]);
        assert_eq!(view.provider_choices[0].chosen, "a.p1");
        set_provider(&f.registry, &mut f.store, "es", "es.pos", "a.p2").unwrap();
        assert!(set_provider(&f.registry, &mut f.store, "es", "es.other", "a.p2").is_err());
        assert!(set_provider(&f.registry, &mut f.store, "es", "es.pos", "nope").is_err());
        let (r, _) = resolve_language(&f.registry, &f.store, "es", "macos").unwrap();
        assert_eq!(mine(&r), vec!["lang.es", "a.p2"]);
        f.store.set_ext_state("a.p2", Some(false), None, None).unwrap();
        let (r, _) = resolve_language(&f.registry, &f.store, "es", "macos").unwrap();
        assert_eq!(mine(&r), vec!["lang.es", "a.p1"], "disabling a chosen provider falls back");
        let list = list(&f.registry, &f.store).unwrap();
        assert!(list.extensions.iter().any(|e| e.id == "a.p2" && !e.enabled && e.source == Source::Installed));
    }

    #[test]
    fn language_defs_come_from_the_registry() {
        let mut f = Fixture::new("langs", &[]);
        assert_eq!(f.languages.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(), vec!["es"]);
        f.store.set_ext_state("lang.es", Some(false), None, None).unwrap();
        assert!(language_defs(&f.registry, &f.store).unwrap().is_empty());
    }
}
