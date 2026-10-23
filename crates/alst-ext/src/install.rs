//! `.alext` packages (SPEC §6.9.2): a zip of the extension folder with `extension.json` at its root.

use crate::manifest::{Manifest, is_safe_path};
use crate::{Error, Result};
use std::io::Read;
use std::path::Path;

pub const MAX_TOTAL_BYTES: u64 = 50 * 1024 * 1024;
const MAX_ENTRIES: usize = 5000;

/// A validated package held in memory, not yet written anywhere.
#[derive(Debug)]
pub struct Package {
    pub manifest: Manifest,
    pub raw_manifest: String,
    /// `(relative path, bytes)`; directories are implied.
    pub files: Vec<(String, Vec<u8>)>,
}

fn bad<T>(m: impl Into<String>) -> Result<T> {
    Err(Error::Package(m.into()))
}

/// Read and validate a `.alext`. Rejects absolute paths, `..`, backslashes, symlinks, too many or too large
/// entries, a missing or invalid manifest, and manifests that point at files the package doesn't contain.
pub fn read_package(bytes: &[u8]) -> Result<Package> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| Error::Package(format!("not a zip file: {e}")))?;
    if zip.len() > MAX_ENTRIES {
        return bad(format!("too many files ({} > {MAX_ENTRIES})", zip.len()));
    }
    let declared: u64 = (0..zip.len()).filter_map(|i| zip.by_index_raw(i).ok().map(|f| f.size())).sum();
    if declared > MAX_TOTAL_BYTES {
        return bad(format!("too large: {} MB uncompressed (limit {} MB)", declared / 1_048_576, MAX_TOTAL_BYTES / 1_048_576));
    }
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| Error::Package(e.to_string()))?;
        let name = f.name().to_string();
        let is_dir = name.ends_with('/');
        let path = name.trim_end_matches('/');
        if !is_safe_path(path) {
            return bad(format!("unsafe path in package: '{name}'"));
        }
        if f.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            return bad(format!("symbolic links are not allowed: '{name}'"));
        }
        if is_dir || path.starts_with("__MACOSX/") || path.rsplit('/').next() == Some(".DS_Store") {
            continue;
        }
        let mut buf = Vec::new();
        f.by_ref().take(MAX_TOTAL_BYTES + 1 - total).read_to_end(&mut buf)?;
        total += buf.len() as u64;
        if total > MAX_TOTAL_BYTES {
            return bad(format!("too large (limit {} MB)", MAX_TOTAL_BYTES / 1_048_576));
        }
        if files.iter().any(|(p, _)| p == path) {
            return bad(format!("duplicate entry '{path}'"));
        }
        files.push((path.to_string(), buf));
    }
    let Some((_, raw)) = files.iter().find(|(p, _)| p == "extension.json") else {
        let nested = files.iter().any(|(p, _)| p.ends_with("/extension.json"));
        return bad(if nested {
            "extension.json must be at the root of the zip, not inside a folder: zip the folder's contents"
        } else {
            "extension.json not found"
        });
    };
    let raw = String::from_utf8(raw.clone()).map_err(|_| Error::Package("extension.json is not UTF-8".into()))?;
    let manifest = Manifest::parse(&raw).map_err(|e| Error::Package(e.to_string()))?;
    let has = |p: &str| files.iter().any(|(f, _)| f == p);
    if let Some(entry) = &manifest.runtime.entry {
        if manifest.runtime.ty != crate::RuntimeType::Declarative && !has(entry) {
            return bad(format!("runtime.entry '{entry}' is not in the package"));
        }
    }
    for p in &manifest.contributes.panels {
        if !has(&p.entry) {
            return bad(format!("panel '{}' entry '{}' is not in the package", p.id, p.entry));
        }
    }
    Ok(Package { manifest, raw_manifest: raw, files })
}

/// Write the package to `<root>/<id>/` through a temp folder and an atomic rename.
pub fn install(root: &Path, pkg: &Package) -> Result<()> {
    std::fs::create_dir_all(root)?;
    let id = &pkg.manifest.id;
    let tmp = root.join(format!(".tmp-{id}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let written = (|| -> Result<()> {
        for (path, data) in &pkg.files {
            let dest = tmp.join(path);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(dest, data)?;
        }
        Ok(())
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    let dest = root.join(id);
    let old = root.join(format!(".old-{id}-{}", std::process::id()));
    let had_old = dest.exists();
    if had_old {
        let _ = std::fs::remove_dir_all(&old);
        std::fs::rename(&dest, &old)?;
    }
    if let Err(e) = std::fs::rename(&tmp, &dest) {
        if had_old {
            let _ = std::fs::rename(&old, &dest);
        }
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e.into());
    }
    if had_old {
        let _ = std::fs::remove_dir_all(&old);
    }
    Ok(())
}

pub fn uninstall(root: &Path, id: &str) -> Result<()> {
    if !is_safe_path(id) || id.contains('/') || id.starts_with('.') {
        return Err(Error::Invalid(format!("invalid extension id '{id}'")));
    }
    let dir = root.join(id);
    if !dir.is_dir() {
        return Err(Error::Invalid(format!("extension '{id}' is not installed")));
    }
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    pub fn manifest_json() -> String {
        serde_json::json!({
            "id": "addon.pkg", "name": "Pkg", "version": "1.0.0", "kind": "addon", "license": "MIT",
            "apiVersion": "^1.0", "targets": ["*"], "runtime": { "type": "js", "entry": "main.js" },
            "contributes": { "panels": [{ "id": "p", "title": "P", "entry": "panel.html" }] }
        })
        .to_string()
    }

    pub fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut out));
            let opts = zip::write::SimpleFileOptions::default();
            for (name, data) in entries {
                if name.ends_with('/') {
                    w.add_directory(*name, opts).unwrap();
                } else {
                    w.start_file(*name, opts).unwrap();
                    w.write_all(data).unwrap();
                }
            }
            w.finish().unwrap();
        }
        out
    }

    fn good() -> Vec<(&'static str, Vec<u8>)> {
        vec![
            ("extension.json", manifest_json().into_bytes()),
            ("main.js", b"export function activate(){}".to_vec()),
            ("panel.html", b"<p>hi</p>".to_vec()),
        ]
    }

    fn pkg(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        zip_of(&entries.iter().map(|(n, d)| (*n, d.as_slice())).collect::<Vec<_>>())
    }

    fn err_of(bytes: &[u8]) -> String {
        read_package(bytes).unwrap_err().to_string()
    }

    #[test]
    fn reads_a_good_package() {
        let p = read_package(&pkg(&good())).unwrap();
        assert_eq!(p.manifest.id, "addon.pkg");
        assert_eq!(p.files.len(), 3);
    }

    #[test]
    fn rejects_zip_slip_and_absolute_paths() {
        for evil in ["../evil.js", "a/../../evil.js", "/etc/passwd", "a\\..\\b.js", "C:/x.js"] {
            let mut g = good();
            g.push((evil, b"x".to_vec()));
            assert!(err_of(&pkg(&g)).contains("unsafe path"), "{evil}");
        }
    }

    #[test]
    fn rejects_symlinks() {
        let mut out = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut out));
            let opts = zip::write::SimpleFileOptions::default();
            for (n, d) in good() {
                w.start_file(n, opts).unwrap();
                w.write_all(&d).unwrap();
            }
            w.add_symlink("link", "/etc/passwd", opts).unwrap();
            w.finish().unwrap();
        }
        assert!(err_of(&out).contains("symbolic links"));
    }

    #[test]
    fn rejects_oversized_packages() {
        let mut g = good();
        g.push(("big.bin", vec![0u8; (MAX_TOTAL_BYTES + 1) as usize]));
        assert!(err_of(&pkg(&g)).contains("too large"));
    }

    #[test]
    fn rejects_missing_or_nested_manifest_and_missing_files() {
        assert!(err_of(&pkg(&[("main.js", b"x".to_vec())])).contains("extension.json not found"));
        let nested: Vec<(&str, Vec<u8>)> = good().into_iter().map(|(n, d)| (Box::leak(format!("folder/{n}").into_boxed_str()) as &str, d)).collect();
        assert!(err_of(&pkg(&nested)).contains("root of the zip"));
        let mut g = good();
        g.retain(|(n, _)| *n != "main.js");
        assert!(err_of(&pkg(&g)).contains("runtime.entry"));
        let mut g = good();
        g.retain(|(n, _)| *n != "panel.html");
        assert!(err_of(&pkg(&g)).contains("panel"));
        assert!(err_of(b"not a zip").contains("not a zip"));
    }

    #[test]
    fn rejects_invalid_manifest() {
        let mut g = good();
        g[0].1 = br#"{"id":"x"}"#.to_vec();
        assert!(err_of(&pkg(&g)).contains("invalid manifest") || err_of(&pkg(&g)).contains("missing field"));
    }

    #[test]
    fn skips_directories_and_mac_junk() {
        let mut g = good();
        g.push(("__MACOSX/._main.js", b"junk".to_vec()));
        g.push(("sub/.DS_Store", b"junk".to_vec()));
        let p = read_package(&pkg(&g)).unwrap();
        assert_eq!(p.files.len(), 3);
        let with_dir = zip_of(&[("extension.json", manifest_json().as_bytes()), ("main.js", b"x"), ("panel.html", b"x"), ("assets/", b"")]);
        assert_eq!(read_package(&with_dir).unwrap().files.len(), 3);
    }

    #[test]
    fn install_replace_and_uninstall() {
        let root = std::env::temp_dir().join(format!("alst-ext-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let p = read_package(&pkg(&good())).unwrap();
        install(&root, &p).unwrap();
        assert!(root.join("addon.pkg/main.js").is_file());
        // Reinstall replaces cleanly and leaves no temp folders behind.
        install(&root, &p).unwrap();
        let names: Vec<String> = std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["addon.pkg"]);
        assert!(uninstall(&root, "../x").is_err());
        assert!(uninstall(&root, "nope").is_err());
        uninstall(&root, "addon.pkg").unwrap();
        assert!(!root.join("addon.pkg").exists());
        std::fs::remove_dir_all(&root).ok();
    }
}
