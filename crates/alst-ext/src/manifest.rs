//! Extension manifest (`extension.json`, SPEC §6.2 and §6.9.2): types and validation.

use crate::perm::Permission;
use crate::{Error, Result};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Version of the host API this build implements (SPEC §6.9.7).
pub const API_VERSION: &str = "1.0.0";

pub const PLATFORMS: &[&str] = &["windows", "macos", "linux", "android", "ios"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Language,
    Addon,
    Module,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeType {
    Declarative,
    Js,
    Wasm,
    Native,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Runtime {
    #[serde(rename = "type")]
    pub ty: RuntimeType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecoratorDecl {
    pub id: String,
    #[serde(default = "default_priority")]
    pub priority: i64,
}

fn default_priority() -> i64 {
    50
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanelDecl {
    pub id: String,
    pub title: String,
    pub entry: String,
    #[serde(default = "default_placement")]
    pub placement: String,
}

fn default_placement() -> String {
    "sidebar".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandDecl {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// Contribution declarations. The host ignores the kinds listed in [`Contributes::unsupported`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Contributes {
    #[serde(default)]
    pub decorators: Vec<DecoratorDecl>,
    #[serde(default)]
    pub panels: Vec<PanelDecl>,
    #[serde(default)]
    pub commands: Vec<CommandDecl>,
    #[serde(default)]
    pub lookup_providers: Vec<Value>,
    #[serde(default)]
    pub importers: Vec<Value>,
    #[serde(default)]
    pub exporters: Vec<Value>,
    #[serde(default)]
    pub activities: Vec<Value>,
    /// JSON Schema object for the extension's settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<Value>,
}

impl Contributes {
    /// Declared contribution kinds that M3 doesn't implement (SPEC §6.9.1).
    pub fn unsupported(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        for (name, n) in [
            ("lookupProviders", self.lookup_providers.len()),
            ("importers", self.importers.len()),
            ("exporters", self.exporters.len()),
            ("activities", self.activities.len()),
        ] {
            if n > 0 {
                v.push(name);
            }
        }
        v
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub kind: Kind,
    pub license: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub api_version: String,
    /// `kind: language` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Language ids or `*`; add-ons and modules only.
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default)]
    pub provides: Vec<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub optional: Vec<String>,
    pub runtime: Runtime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platforms: Option<Vec<String>>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub contributes: Contributes,
    /// Language extensions: read by `alst-text` from the raw manifest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declarative: Option<Value>,
}

fn is_ident(s: &str, extra: &str) -> bool {
    let mut c = s.chars();
    matches!(c.next(), Some(f) if f.is_ascii_lowercase() || f.is_ascii_digit())
        && s.len() <= 64
        && c.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || extra.contains(ch))
}

/// Relative path inside an extension folder: no absolute paths, no `..`, no backslashes.
pub fn is_safe_path(p: &str) -> bool {
    !p.is_empty()
        && !p.starts_with('/')
        && !p.contains('\\')
        && !p.contains('\0')
        && !p.as_bytes().get(1).is_some_and(|c| *c == b':')
        && p.split('/').all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

fn is_capability(s: &str, allow_wildcard: bool) -> bool {
    match s.split_once('.') {
        Some((scope, name)) => {
            (is_ident(scope, "-_") || (allow_wildcard && scope == "*")) && !name.is_empty() && is_ident(name, "-_.")
        }
        None => false,
    }
}

impl Manifest {
    pub fn parse(json: &str) -> Result<Self> {
        let m: Manifest = serde_json::from_str(json).map_err(|e| Error::Manifest(e.to_string()))?;
        m.validate()?;
        Ok(m)
    }

    pub fn semver(&self) -> Version {
        Version::parse(&self.version).expect("validated")
    }

    pub fn permissions(&self) -> Vec<Permission> {
        self.permissions.iter().filter_map(|p| Permission::parse(p).ok()).collect()
    }

    /// Languages this extension can serve: its own for a language extension, else `targets`.
    pub fn targets_language(&self, lang: &str) -> bool {
        match self.kind {
            Kind::Language => self.language.as_deref() == Some(lang),
            _ => self.targets.iter().any(|t| t == "*" || t == lang),
        }
    }

    pub fn available_on(&self, platform: &str) -> bool {
        self.platforms.as_ref().is_none_or(|p| p.iter().any(|x| x == platform))
    }

    pub fn validate(&self) -> Result<()> {
        let bad = |m: String| Err(Error::Manifest(m));
        if !is_ident(&self.id, "-_.") {
            return bad(format!("invalid id '{}' (lowercase letters, digits, '.', '-', '_'; at most 64)", self.id));
        }
        if self.name.trim().is_empty() {
            return bad("name is required".into());
        }
        if Version::parse(&self.version).is_err() {
            return bad(format!("version '{}' is not a semantic version", self.version));
        }
        if self.license.trim().is_empty() {
            return bad("license (an SPDX id) is required".into());
        }
        if VersionReq::parse(&self.api_version).is_err() {
            return bad(format!("apiVersion '{}' is not a valid version range", self.api_version));
        }
        match self.kind {
            Kind::Language => {
                if !self.language.as_deref().is_some_and(|l| is_ident(l, "-_")) {
                    return bad("a language extension needs a \"language\" id".into());
                }
                if !self.targets.is_empty() {
                    return bad("a language extension uses \"language\", not \"targets\"".into());
                }
            }
            _ => {
                if self.targets.is_empty() {
                    return bad("targets is required: language ids, or \"*\" for every language".into());
                }
                for t in &self.targets {
                    if t != "*" && !is_ident(t, "-_") {
                        return bad(format!("invalid target '{t}'"));
                    }
                }
            }
        }
        match (self.runtime.ty, &self.runtime.entry) {
            (RuntimeType::Declarative, _) => {
                if self.kind == Kind::Language && self.declarative.is_none() {
                    return bad("a declarative language extension needs a \"declarative\" section".into());
                }
            }
            (_, None) => return bad("runtime.entry is required for js, wasm and native runtimes".into()),
            (RuntimeType::Native, Some(_)) => {
                if self.platforms.is_none() {
                    return bad("native extensions must list their platforms".into());
                }
            }
            (_, Some(e)) => {
                if !is_safe_path(e) {
                    return bad(format!("runtime.entry '{e}' is not a safe relative path"));
                }
            }
        }
        if let Some(ps) = &self.platforms {
            for p in ps {
                if !PLATFORMS.contains(&p.as_str()) {
                    return bad(format!("unknown platform '{p}'"));
                }
            }
        }
        for c in &self.provides {
            if !is_capability(c, false) {
                return bad(format!("invalid capability '{c}' in provides"));
            }
        }
        for c in self.requires.iter().chain(&self.optional) {
            if !is_capability(c, true) {
                return bad(format!("invalid capability '{c}'"));
            }
        }
        for p in &self.permissions {
            Permission::parse(p).map_err(|e| Error::Manifest(e.to_string()))?;
        }
        self.validate_contributes()
    }

    fn validate_contributes(&self) -> Result<()> {
        let bad = |m: String| Err(Error::Manifest(m));
        let c = &self.contributes;
        let mut seen = std::collections::HashSet::new();
        for d in &c.decorators {
            if !is_ident(&d.id, "-_") || !seen.insert(("decorator", d.id.as_str())) {
                return bad(format!("invalid or duplicate decorator id '{}'", d.id));
            }
        }
        for p in &c.panels {
            if !is_ident(&p.id, "-_") || !seen.insert(("panel", p.id.as_str())) {
                return bad(format!("invalid or duplicate panel id '{}'", p.id));
            }
            if !is_safe_path(&p.entry) {
                return bad(format!("panel '{}' entry '{}' is not a safe relative path", p.id, p.entry));
            }
            if p.placement != "sidebar" {
                return bad(format!("panel '{}': placement '{}' is not supported (only \"sidebar\")", p.id, p.placement));
            }
        }
        for cmd in &c.commands {
            if !is_ident(&cmd.id, "-_.") || !seen.insert(("command", cmd.id.as_str())) {
                return bad(format!("invalid or duplicate command id '{}'", cmd.id));
            }
            if let Some(ctx) = &cmd.context {
                if !["reader", "term", "global"].contains(&ctx.as_str()) {
                    return bad(format!("command '{}': unknown context '{ctx}'", cmd.id));
                }
            }
        }
        if let Some(s) = &c.settings {
            validate_settings_schema(s)?;
        }
        let runs_code = matches!(self.runtime.ty, RuntimeType::Js | RuntimeType::Wasm);
        if !runs_code && (!c.decorators.is_empty() || !c.panels.is_empty() || !c.commands.is_empty()) {
            return bad("decorators, panels and commands need a js or wasm runtime".into());
        }
        Ok(())
    }
}

/// The settings schema subset the Extensions screen can render (SPEC §6.9.7).
fn validate_settings_schema(s: &Value) -> Result<()> {
    let bad = |m: String| Err(Error::Manifest(m));
    let Some(obj) = s.as_object() else { return bad("contributes.settings must be an object".into()) };
    if obj.get("type").and_then(Value::as_str).is_some_and(|t| t != "object") {
        return bad("contributes.settings must have type \"object\"".into());
    }
    let props = obj.get("properties").and_then(Value::as_object);
    for (name, p) in props.into_iter().flatten() {
        match p.get("type").and_then(Value::as_str) {
            Some("boolean" | "number" | "string") => {}
            other => return bad(format!("setting '{name}': type {other:?} is not supported (boolean, number, string)")),
        }
        if p.get("enum").is_some() && p.get("type").and_then(Value::as_str) != Some("string") {
            return bad(format!("setting '{name}': enum is only supported on strings"));
        }
    }
    Ok(())
}

/// Defaults from the settings schema overlaid with stored values (unknown stored keys are dropped).
pub fn settings_with_defaults(manifest: &Manifest, stored: &Value) -> Value {
    let mut out = serde_json::Map::new();
    let props = manifest
        .contributes
        .settings
        .as_ref()
        .and_then(|s| s.get("properties"))
        .and_then(Value::as_object);
    for (name, p) in props.into_iter().flatten() {
        if let Some(v) = stored.get(name).filter(|v| value_fits(p, v)) {
            out.insert(name.clone(), v.clone());
        } else if let Some(d) = p.get("default") {
            out.insert(name.clone(), d.clone());
        }
    }
    Value::Object(out)
}

fn value_fits(schema: &Value, v: &Value) -> bool {
    let ok = match schema.get("type").and_then(Value::as_str) {
        Some("boolean") => v.is_boolean(),
        Some("number") => v.is_number(),
        Some("string") => v.is_string(),
        _ => false,
    };
    if !ok {
        return false;
    }
    match schema.get("enum").and_then(Value::as_array) {
        Some(allowed) => allowed.contains(v),
        None => true,
    }
}

/// Check a settings object against the manifest's schema; rejects unknown keys and wrong types.
pub fn validate_settings(manifest: &Manifest, settings: &Value) -> Result<()> {
    let Some(obj) = settings.as_object() else { return Err(Error::Invalid("settings must be an object".into())) };
    let props = manifest
        .contributes
        .settings
        .as_ref()
        .and_then(|s| s.get("properties"))
        .and_then(Value::as_object);
    for (k, v) in obj {
        match props.and_then(|p| p.get(k)) {
            None => return Err(Error::Invalid(format!("unknown setting '{k}'"))),
            Some(schema) if !value_fits(schema, v) => {
                return Err(Error::Invalid(format!("setting '{k}' has an invalid value")));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn base() -> Value {
        serde_json::json!({
            "id": "addon.test", "name": "Test", "version": "1.0.0", "kind": "addon", "license": "MIT",
            "apiVersion": "^1.0", "targets": ["*"],
            "runtime": { "type": "js", "entry": "main.js" }
        })
    }

    fn with(f: impl FnOnce(&mut Value)) -> Result<Manifest> {
        let mut v = base();
        f(&mut v);
        Manifest::parse(&v.to_string())
    }

    #[test]
    fn minimal_manifest_parses() {
        let m = with(|_| {}).unwrap();
        assert_eq!(m.id, "addon.test");
        assert!(m.available_on("android"));
        assert!(m.targets_language("fr"));
    }

    #[test]
    fn license_is_required() {
        assert!(with(|v| _ = v.as_object_mut().unwrap().remove("license")).is_err());
        assert!(with(|v| v["license"] = "  ".into()).is_err());
    }

    #[test]
    fn rejects_bad_fields() {
        assert!(with(|v| v["id"] = "Bad Id".into()).is_err());
        assert!(with(|v| v["version"] = "one".into()).is_err());
        assert!(with(|v| v["apiVersion"] = "wat".into()).is_err());
        assert!(with(|v| v["targets"] = serde_json::json!([])).is_err());
        assert!(with(|v| v["runtime"]["entry"] = "../x.js".into()).is_err());
        assert!(with(|v| v["runtime"]["entry"] = "/abs.js".into()).is_err());
        assert!(with(|v| v["runtime"] = serde_json::json!({ "type": "js" })).is_err());
        assert!(with(|v| v["platforms"] = serde_json::json!(["beos"])).is_err());
        assert!(with(|v| v["permissions"] = serde_json::json!(["root"])).is_err());
        assert!(with(|v| v["provides"] = serde_json::json!(["nodot"])).is_err());
        assert!(with(|v| v["provides"] = serde_json::json!(["*.x"])).is_err(), "wildcard only in requires");
        assert!(with(|v| v["requires"] = serde_json::json!(["*.x"])).is_ok());
        assert!(with(|v| v["unknownField"] = 1.into()).is_ok(), "forward compatible");
    }

    #[test]
    fn language_kind_rules() {
        let lang = |f: fn(&mut Value)| {
            let mut v = serde_json::json!({
                "id": "lang.xx", "name": "X", "version": "1.0.0", "kind": "language", "license": "MIT",
                "language": "xx", "apiVersion": "^1.0", "runtime": { "type": "declarative" },
                "declarative": { "tokenizer": { "engine": "regex" } }
            });
            f(&mut v);
            Manifest::parse(&v.to_string())
        };
        assert!(lang(|_| {}).is_ok());
        assert!(lang(|v| _ = v.as_object_mut().unwrap().remove("language")).is_err());
        assert!(lang(|v| _ = v.as_object_mut().unwrap().remove("declarative")).is_err());
        assert!(lang(|v| v["targets"] = serde_json::json!(["xx"])).is_err());
    }

    #[test]
    fn contributes_validation() {
        let m = with(|v| {
            v["contributes"] = serde_json::json!({
                "decorators": [{ "id": "d" }],
                "panels": [{ "id": "p", "title": "P", "entry": "panel.html" }],
                "commands": [{ "id": "c", "title": "C", "context": "reader" }],
                "importers": [{ "id": "x" }]
            })
        })
        .unwrap();
        assert_eq!(m.contributes.decorators[0].priority, 50);
        assert_eq!(m.contributes.unsupported(), vec!["importers"]);
        assert!(with(|v| v["contributes"] = serde_json::json!({ "panels": [{ "id": "p", "title": "P", "entry": "../p.html" }] })).is_err());
        assert!(with(|v| v["contributes"] = serde_json::json!({ "commands": [{ "id": "c", "title": "C", "context": "nowhere" }] })).is_err());
        assert!(with(|v| v["contributes"] = serde_json::json!({ "decorators": [{ "id": "d" }, { "id": "d" }] })).is_err());
        assert!(with(|v| {
            v["runtime"] = serde_json::json!({ "type": "declarative" });
            v["contributes"] = serde_json::json!({ "commands": [{ "id": "c", "title": "C" }] });
        })
        .is_err());
    }

    #[test]
    fn settings_schema_and_defaults() {
        let m = with(|v| {
            v["contributes"] = serde_json::json!({ "settings": { "type": "object", "properties": {
                "flag": { "type": "boolean", "default": true },
                "mode": { "type": "string", "enum": ["a", "b"], "default": "a" },
                "n": { "type": "number" }
            }}})
        })
        .unwrap();
        let s = settings_with_defaults(&m, &serde_json::json!({ "mode": "b", "gone": 1, "n": "text" }));
        assert_eq!(s, serde_json::json!({ "flag": true, "mode": "b" }));
        assert!(validate_settings(&m, &serde_json::json!({ "mode": "b", "n": 3 })).is_ok());
        assert!(validate_settings(&m, &serde_json::json!({ "mode": "c" })).is_err());
        assert!(validate_settings(&m, &serde_json::json!({ "other": 1 })).is_err());
        assert!(with(|v| v["contributes"] = serde_json::json!({ "settings": { "properties": { "x": { "type": "array" } } } })).is_err());
    }

    #[test]
    fn safe_paths() {
        for ok in ["main.js", "a/b/c.wasm"] {
            assert!(is_safe_path(ok), "{ok}");
        }
        for bad in ["", "/x", "../x", "a/../b", "a//b", "a\\b", "C:/x", "./a"] {
            assert!(!is_safe_path(bad), "{bad}");
        }
    }
}
