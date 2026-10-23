//! Registry of bundled and installed extensions (SPEC §6.9.2).

use crate::manifest::{Manifest, is_safe_path};
use crate::{Error, Result};
use include_dir::{Dir, include_dir};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// First-party extensions compiled into the app (`extensions/*`).
pub static BUNDLED: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../extensions");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Bundled,
    Installed,
}

#[derive(Debug, Clone)]
enum Files {
    Embedded { dir: &'static Dir<'static> },
    Disk { root: PathBuf },
}

#[derive(Debug, Clone)]
pub struct Extension {
    pub manifest: Manifest,
    /// The manifest as written, for consumers (like `alst-text`) that read their own section.
    pub raw: String,
    pub source: Source,
    files: Files,
}

/// A folder or bundled entry that couldn't be loaded; shown on the Extensions screen.
#[derive(Debug, Clone, Serialize)]
pub struct LoadError {
    pub location: String,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct Registry {
    exts: BTreeMap<String, Extension>,
    pub errors: Vec<LoadError>,
}

impl Registry {
    /// `bundled` is normally [`BUNDLED`]; `installed_root` is `<app-data>/extensions`.
    pub fn load(bundled: &'static Dir<'static>, installed_root: Option<&Path>) -> Self {
        let mut reg = Registry::default();
        for dir in bundled.dirs() {
            let name = dir.path().display().to_string();
            let loaded = dir
                .get_file(dir.path().join("extension.json"))
                .ok_or_else(|| Error::Manifest("no extension.json".into()))
                .and_then(|f| {
                    let raw = f.contents_utf8().ok_or_else(|| Error::Manifest("not UTF-8".into()))?.to_string();
                    Ok((Manifest::parse(&raw)?, raw))
                });
            match loaded {
                Ok((manifest, raw)) => reg.add(Extension { manifest, raw, source: Source::Bundled, files: Files::Embedded { dir } }, &name),
                Err(e) => reg.errors.push(LoadError { location: format!("bundled/{name}"), message: e.to_string() }),
            }
        }
        if let Some(root) = installed_root {
            reg.scan_installed(root);
        }
        reg
    }

    fn scan_installed(&mut self, root: &Path) {
        let Ok(entries) = std::fs::read_dir(root) else { return };
        let mut dirs: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        for path in dirs {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if name.starts_with('.') {
                continue; // temp folders from an interrupted install
            }
            let loaded = std::fs::read_to_string(path.join("extension.json"))
                .map_err(Error::from)
                .and_then(|raw| Ok((Manifest::parse(&raw)?, raw)))
                .and_then(|(m, raw)| {
                    if m.id == name { Ok((m, raw)) } else { Err(Error::Manifest(format!("folder name '{name}' doesn't match id '{}'", m.id))) }
                });
            match loaded {
                Ok((manifest, raw)) => self.add(Extension { manifest, raw, source: Source::Installed, files: Files::Disk { root: path } }, &name),
                Err(e) => self.errors.push(LoadError { location: format!("installed/{name}"), message: e.to_string() }),
            }
        }
    }

    /// Same id twice: an installed version overrides the bundled one only if it is newer.
    fn add(&mut self, ext: Extension, location: &str) {
        match self.exts.get(&ext.manifest.id) {
            None => {
                self.exts.insert(ext.manifest.id.clone(), ext);
            }
            Some(old) => {
                let newer = ext.source == Source::Installed && old.source == Source::Bundled && ext.manifest.semver() > old.manifest.semver();
                if newer {
                    self.exts.insert(ext.manifest.id.clone(), ext);
                } else if ext.source == Source::Bundled {
                    self.errors.push(LoadError { location: location.into(), message: format!("duplicate id '{}'", ext.manifest.id) });
                }
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<&Extension> {
        self.exts.get(id)
    }

    /// All extensions ordered by id.
    pub fn all(&self) -> impl Iterator<Item = &Extension> {
        self.exts.values()
    }

    /// A file of an extension, with its MIME type. Rejects unsafe paths and anything that resolves
    /// outside the extension folder (symlinks).
    pub fn read_file(&self, id: &str, path: &str) -> Result<(Vec<u8>, &'static str)> {
        let ext = self.get(id).ok_or_else(|| Error::Invalid(format!("no extension '{id}'")))?;
        if !is_safe_path(path) {
            return Err(Error::Invalid(format!("invalid path '{path}'")));
        }
        let bytes = match &ext.files {
            Files::Embedded { dir } => dir
                .get_file(dir.path().join(path))
                .map(|f| f.contents().to_vec())
                .ok_or_else(|| Error::Invalid(format!("no file '{path}' in '{id}'")))?,
            Files::Disk { root } => {
                let full = root.join(path).canonicalize()?;
                if !full.starts_with(root.canonicalize()?) {
                    return Err(Error::Invalid(format!("'{path}' is outside the extension folder")));
                }
                std::fs::read(full)?
            }
        };
        Ok((bytes, mime_for(path)))
    }
}

pub fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "js" | "mjs" => "text/javascript",
        "wasm" => "application/wasm",
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "txt" | "md" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::{install, read_package, tests::zip_of};
    use semver::Version;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("alst-reg-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    fn manifest(id: &str, version: &str) -> String {
        serde_json::json!({
            "id": id, "name": id, "version": version, "kind": "addon", "license": "MIT",
            "apiVersion": "^1.0", "targets": ["*"], "runtime": { "type": "js", "entry": "main.js" }
        })
        .to_string()
    }

    fn install_zip(root: &Path, id: &str, version: &str) {
        let bytes = zip_of(&[("extension.json", manifest(id, version).as_bytes()), ("main.js", b"// js")]);
        install(root, &read_package(&bytes).unwrap()).unwrap();
    }

    #[test]
    fn bundled_first_party_manifests_are_valid() {
        let reg = Registry::load(&BUNDLED, None);
        assert!(reg.errors.is_empty(), "{:?}", reg.errors);
        let es = reg.get("lang.es").expect("lang.es is bundled");
        assert_eq!(es.manifest.language.as_deref(), Some("es"));
        assert_eq!(es.manifest.license, "MIT");
        assert!(reg.all().all(|e| e.source == Source::Bundled));
        // Every first-party extension carries its own LICENSE (SPEC §14).
        for e in reg.all() {
            assert!(reg.read_file(&e.manifest.id, "LICENSE").is_ok(), "{} has no LICENSE", e.manifest.id);
        }
    }

    #[test]
    fn installed_extensions_load_and_serve_files() {
        let root = tmp("serve");
        install_zip(&root, "addon.one", "1.0.0");
        let reg = Registry::load(&BUNDLED, Some(&root));
        let (bytes, mime) = reg.read_file("addon.one", "main.js").unwrap();
        assert_eq!((bytes.as_slice(), mime), (&b"// js"[..], "text/javascript"));
        assert!(reg.read_file("addon.one", "../addon.one/main.js").is_err());
        assert!(reg.read_file("addon.one", "/etc/passwd").is_err());
        assert!(reg.read_file("addon.one", "nope.js").is_err());
        assert!(reg.read_file("nope", "main.js").is_err());
        assert_eq!(reg.get("addon.one").unwrap().source, Source::Installed);
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_escape_the_extension_folder() {
        let root = tmp("symlink");
        install_zip(&root, "addon.link", "1.0.0");
        let secret = root.join("secret.txt");
        std::fs::write(&secret, "secret").unwrap();
        std::os::unix::fs::symlink(&secret, root.join("addon.link/leak.txt")).unwrap();
        let reg = Registry::load(&BUNDLED, Some(&root));
        assert!(reg.read_file("addon.link", "leak.txt").is_err());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn broken_folders_are_reported_not_fatal() {
        let root = tmp("broken");
        install_zip(&root, "addon.ok", "1.0.0");
        std::fs::create_dir_all(root.join("junk")).unwrap();
        std::fs::write(root.join("junk/extension.json"), "{ nope").unwrap();
        std::fs::create_dir_all(root.join("renamed")).unwrap();
        std::fs::write(root.join("renamed/extension.json"), manifest("addon.other", "1.0.0")).unwrap();
        std::fs::create_dir_all(root.join(".tmp-half")).unwrap();
        let reg = Registry::load(&BUNDLED, Some(&root));
        assert!(reg.get("addon.ok").is_some());
        assert_eq!(reg.errors.len(), 2, "{:?}", reg.errors);
        assert!(reg.errors.iter().any(|e| e.location == "installed/junk"));
        assert!(reg.errors.iter().any(|e| e.message.contains("doesn't match")));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn newer_installed_version_overrides_bundled_but_older_does_not() {
        let root = tmp("override");
        let bundled_version = Registry::load(&BUNDLED, None).get("lang.es").unwrap().manifest.semver();
        let newer = Version::new(bundled_version.major + 1, 0, 0);
        let m = |v: &Version| {
            serde_json::json!({
                "id": "lang.es", "name": "es2", "version": v.to_string(), "kind": "language", "license": "MIT",
                "language": "es", "apiVersion": "^1.0", "runtime": { "type": "declarative" }, "declarative": {}
            })
            .to_string()
        };
        let put = |v: &Version| {
            let bytes = zip_of(&[("extension.json", m(v).as_bytes())]);
            install(&root, &read_package(&bytes).unwrap()).unwrap();
        };
        put(&newer);
        let reg = Registry::load(&BUNDLED, Some(&root));
        assert_eq!(reg.get("lang.es").unwrap().source, Source::Installed);
        put(&Version::new(0, 0, 1));
        let reg = Registry::load(&BUNDLED, Some(&root));
        assert_eq!(reg.get("lang.es").unwrap().source, Source::Bundled);
        std::fs::remove_dir_all(root).ok();
    }
}

#[cfg(test)]
mod schema_tests {
    use super::*;

    /// `schemas/extension.schema.json` must keep up with the manifest types: it declares every field
    /// the bundled manifests use and requires the fields validation requires.
    #[test]
    fn schema_covers_bundled_manifests() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../../schemas/extension.schema.json")).expect("schema is JSON");
        let props = schema["properties"].as_object().unwrap();
        let required: Vec<&str> = schema["required"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        for field in ["id", "name", "version", "kind", "license", "apiVersion", "runtime"] {
            assert!(required.contains(&field), "schema should require {field}");
        }
        for ext in Registry::load(&BUNDLED, None).all() {
            let raw: serde_json::Value = serde_json::from_str(&ext.raw).unwrap();
            for key in raw.as_object().unwrap().keys() {
                assert!(props.contains_key(key), "{}: field '{key}' missing from the schema", ext.manifest.id);
            }
            for key in &required {
                assert!(raw.get(key).is_some(), "{}: missing required '{key}'", ext.manifest.id);
            }
        }
    }
}
