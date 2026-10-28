//! Language data packages (SPEC §17.3): compiled Lindera dictionaries that are downloaded, imported from
//! a file or fetched from a paired device instead of being compiled into the app.
//!
//! A package is a zip holding `langdata.json` (`{id, engine, version, lindera, files}`), the dictionary
//! files and the dictionary's licence. The app pins the packages it accepts in `langdata.json` next to
//! this crate's manifest (id, version, url, sha256 of the zip, size); anything else is refused.

use crate::morph::{self, Morph};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Media type of a package stored in the media store, so peers can recognise it (SPEC §9.6).
pub const MIME: &str = "application/x-alstroemeria-langdata";

/// One entry of the pinned list.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Pinned {
    pub id: String,
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

/// `langdata.json` inside a package.
#[derive(Debug, Deserialize)]
pub struct PackageManifest {
    pub id: String,
    pub engine: String,
    pub version: String,
    #[serde(default)]
    pub lindera: String,
    pub files: Vec<String>,
}

/// What Settings → Languages shows for one package.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub id: String,
    pub label: &'static str,
    pub version: String,
    pub size: u64,
    pub installed: bool,
    /// The pin is still a placeholder (no release has published this package), so it can't be downloaded.
    pub published: bool,
}

pub fn pinned() -> Vec<Pinned> {
    serde_json::from_str(include_str!("../langdata.json")).expect("langdata.json is valid")
}

pub fn pinned_for(id: &str) -> Option<Pinned> {
    pinned().into_iter().find(|p| p.id == id)
}

/// A pin whose hash is all zeros is a placeholder from before the package was published.
fn is_published(p: &Pinned) -> bool {
    p.sha256.chars().any(|c| c != '0')
}

pub fn label(id: &str) -> &'static str {
    match Morph::from_langdata_id(id) {
        Some(Morph::Ja) => "Japanese",
        Some(Morph::Ko) => "Korean",
        None => "this language",
    }
}

fn root(data: &Path) -> PathBuf {
    data.join("langdata")
}

/// `<data>/langdata/<id>/<version>/`
pub fn install_dir(data: &Path, id: &str, version: &str) -> PathBuf {
    root(data).join(id).join(version)
}

/// The installed directory of the pinned version of `id`, if complete.
pub fn installed(data: &Path, id: &str) -> Option<PathBuf> {
    let p = pinned_for(id)?;
    let dir = install_dir(data, &p.id, &p.version);
    let m: PackageManifest = serde_json::from_slice(&fs::read(dir.join("langdata.json")).ok()?).ok()?;
    (m.id == p.id && m.version == p.version && m.files.iter().all(|f| dir.join(f).is_file())).then_some(dir)
}

pub fn status(data: &Path) -> Vec<Status> {
    pinned()
        .into_iter()
        .map(|p| Status {
            label: label(&p.id),
            installed: installed(data, &p.id).is_some(),
            published: is_published(&p),
            id: p.id,
            version: p.version,
            size: p.size,
        })
        .collect()
}

/// Point the tokenizers at every installed package. Call at start and after an install.
pub fn refresh(data: &Path) {
    for p in pinned() {
        if let Some(kind) = Morph::from_langdata_id(&p.id) {
            morph::set_dict_dir(kind, installed(data, &p.id));
        }
    }
}

fn io(e: std::io::Error) -> Error {
    Error::Import(e.to_string())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = File::open(path).map_err(io)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(io)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// The pinned package whose zip has this sha256 (the media-store id of a stored package is that hash).
pub fn pinned_by_hash(sha256: &str) -> Option<Pinned> {
    pinned().into_iter().find(|p| is_published(p) && p.sha256 == sha256)
}

/// Install the zip at `zip_path`. Refused unless its sha256 is pinned. Returns the package id.
/// Extracts beside the final folder and renames it into place, so a failure leaves nothing half-installed.
pub fn install_zip(data: &Path, zip_path: &Path) -> Result<String> {
    let id = install_zip_with(data, zip_path, &pinned())?;
    refresh(data);
    Ok(id)
}

fn install_zip_with(data: &Path, zip_path: &Path, pins: &[Pinned]) -> Result<String> {
    let sum = sha256_file(zip_path)?;
    let pin = pins
        .iter()
        .find(|p| is_published(p) && p.sha256 == sum)
        .ok_or_else(|| Error::Import("this is not a language data package that this version of the app accepts".into()))?;
    let file = File::open(zip_path).map_err(io)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| Error::Import(format!("not a zip file: {e}")))?;
    let manifest: PackageManifest = {
        let mut f = zip.by_name("langdata.json").map_err(|_| Error::Import("package has no langdata.json".into()))?;
        let mut s = String::new();
        f.read_to_string(&mut s).map_err(io)?;
        serde_json::from_str(&s).map_err(|e| Error::Import(format!("bad langdata.json: {e}")))?
    };
    if manifest.id != pin.id || manifest.version != pin.version {
        return Err(Error::Import("package contents do not match its pinned id and version".into()));
    }
    let dest = install_dir(data, &pin.id, &pin.version);
    let tmp = dest.with_extension("installing");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).map_err(io)?;
    let result = (|| -> Result<()> {
        for i in 0..zip.len() {
            let mut f = zip.by_index(i).map_err(|e| Error::Import(e.to_string()))?;
            if f.is_dir() {
                continue;
            }
            // Only plain top-level names: no subfolders, no `..`, no absolute paths.
            let name = f.enclosed_name().ok_or_else(|| Error::Import("unsafe path in package".into()))?;
            let mut comps = name.components();
            let (Some(std::path::Component::Normal(leaf)), None) = (comps.next(), comps.next()) else {
                return Err(Error::Import("unexpected folder in package".into()));
            };
            let mut out = File::create(tmp.join(leaf)).map_err(io)?;
            std::io::copy(&mut f, &mut out).map_err(io)?;
        }
        for f in &manifest.files {
            if !tmp.join(f).is_file() {
                return Err(Error::Import(format!("package is missing {f}")));
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&tmp);
        return Err(e);
    }
    // Retire older versions and any previous copy of this one.
    if let Ok(entries) = fs::read_dir(root(data).join(&pin.id)) {
        for e in entries.flatten().filter(|e| e.path() != tmp) {
            let _ = fs::remove_dir_all(e.path());
        }
    }
    fs::rename(&tmp, &dest).map_err(io)?;
    Ok(pin.id.clone())
}

/// Where a partial download of `pin` lives.
pub fn partial_path(data: &Path, pin: &Pinned) -> PathBuf {
    root(data).join(".downloads").join(format!("{}-{}.part", pin.id, pin.version))
}

/// Download the pinned package into a partial file, resuming from what is there, and verify its sha256.
/// `progress(done, total)` is called as bytes arrive. Setting `cancel` stops with an error and keeps the
/// partial file so a later call resumes. Returns the path of the verified zip.
pub fn download(
    data: &Path,
    pin: &Pinned,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<PathBuf> {
    if !is_published(pin) {
        return Err(Error::Import("this language data has not been published yet; use \"Import from file\"".into()));
    }
    if !(pin.url.starts_with("https://") || pin.url.starts_with("http://")) {
        return Err(Error::Import("language data URL must be http(s)".into()));
    }
    let part = partial_path(data, pin);
    fs::create_dir_all(part.parent().expect("has parent")).map_err(io)?;
    let mut have = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if have > pin.size {
        let _ = fs::remove_file(&part);
        have = 0;
    }
    if have < pin.size {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .timeout_recv_body(Some(Duration::from_secs(60)))
            .http_status_as_error(false)
            .user_agent("Mozilla/5.0 (compatible; Alstroemeria)")
            .build()
            .into();
        let mut req = agent.get(&pin.url);
        if have > 0 {
            req = req.header("Range", format!("bytes={have}-"));
        }
        let resp = req.call().map_err(|e| Error::Import(format!("download failed: {e}")))?;
        let status = resp.status().as_u16();
        let mut out = match status {
            206 => fs::OpenOptions::new().append(true).open(&part).map_err(io)?,
            200 => {
                // The server ignored the Range header: start over.
                have = 0;
                File::create(&part).map_err(io)?
            }
            416 => {
                let _ = fs::remove_file(&part);
                return Err(Error::Import("download position was rejected; try again".into()));
            }
            s => return Err(Error::Import(format!("download failed: server answered {s}"))),
        };
        let mut body = resp.into_body().into_reader();
        let mut buf = vec![0u8; 64 * 1024];
        progress(have, pin.size);
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Import("download cancelled".into()));
            }
            let n = body.read(&mut buf).map_err(|e| Error::Import(format!("download interrupted: {e}")))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n]).map_err(io)?;
            have += n as u64;
            if have > pin.size {
                drop(out);
                let _ = fs::remove_file(&part);
                return Err(Error::Import("download is larger than expected".into()));
            }
            progress(have, pin.size);
        }
        out.flush().map_err(io)?;
    }
    if have != pin.size {
        return Err(Error::Import("download ended early; try again to resume".into()));
    }
    if sha256_file(&part)? != pin.sha256 {
        let _ = fs::remove_file(&part);
        return Err(Error::Import("downloaded file failed its checksum".into()));
    }
    Ok(part)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::Arc;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("alst-langdata-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn pin_for(bytes: &[u8], url: String) -> Pinned {
        Pinned { id: "ja-ipadic".into(), version: "1".into(), url, sha256: hex(&Sha256::digest(bytes)), size: bytes.len() as u64 }
    }

    /// Tiny HTTP server: answers `GET` with `Range` support. `cut_first` drops the first connection
    /// after that many body bytes, to simulate an interrupted download.
    fn serve(body: Arc<Vec<u8>>, cut_first: Option<usize>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/pkg.zip", l.local_addr().unwrap());
        std::thread::spawn(move || {
            let mut first = true;
            for s in l.incoming() {
                let Ok(mut s) = s else { break };
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut start = 0usize;
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        start = v.trim().trim_end_matches('-').parse().unwrap_or(0);
                    }
                }
                let slice = &body[start..];
                let head = if start > 0 {
                    format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{}/{}\r\nConnection: close\r\n\r\n", slice.len(), body.len() - 1, body.len())
                } else {
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", slice.len())
                };
                let _ = s.write_all(head.as_bytes());
                if first && let Some(n) = cut_first {
                    let _ = s.write_all(&slice[..n.min(slice.len())]);
                } else {
                    let _ = s.write_all(slice);
                }
                first = false;
            }
        });
        url
    }

    #[test]
    fn download_verifies_checksum() {
        let data = tmp("dl");
        let body: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let url = serve(Arc::new(body.clone()), None);
        let pin = pin_for(&body, url);
        let mut last = (0, 0);
        let path = download(&data, &pin, &mut |d, t| last = (d, t), &AtomicBool::new(false)).unwrap();
        assert_eq!(fs::read(path).unwrap(), body);
        assert_eq!(last, (body.len() as u64, body.len() as u64));
        // A wrong pin is refused and the file removed.
        let mut bad = pin.clone();
        bad.sha256 = hex(&Sha256::digest(b"other"));
        let _ = fs::remove_file(partial_path(&data, &pin));
        assert!(download(&data, &bad, &mut |_, _| {}, &AtomicBool::new(false)).unwrap_err().to_string().contains("checksum"));
        assert!(!partial_path(&data, &bad).exists());
    }

    #[test]
    fn download_resumes_after_interruption() {
        let data = tmp("resume");
        let body: Vec<u8> = (0..400_000u32).map(|i| (i % 253) as u8).collect();
        let url = serve(Arc::new(body.clone()), Some(100_000));
        let pin = pin_for(&body, url);
        let err = download(&data, &pin, &mut |_, _| {}, &AtomicBool::new(false)).unwrap_err().to_string();
        assert!(err.contains("interrupted") || err.contains("early"), "{err}");
        let partial = fs::metadata(partial_path(&data, &pin)).unwrap().len();
        assert!(partial > 0 && partial < body.len() as u64);
        let path = download(&data, &pin, &mut |_, _| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(fs::read(path).unwrap(), body);
    }

    #[test]
    fn download_can_be_cancelled_and_keeps_the_partial() {
        let data = tmp("cancel");
        let body: Vec<u8> = vec![7; 500_000];
        let url = serve(Arc::new(body.clone()), None);
        let pin = pin_for(&body, url);
        let cancel = AtomicBool::new(false);
        let err = download(&data, &pin, &mut |d, _| if d > 100_000 { cancel.store(true, Ordering::Relaxed) }, &cancel).unwrap_err();
        assert!(err.to_string().contains("cancelled"));
        assert!(partial_path(&data, &pin).exists());
    }

    #[test]
    fn placeholder_pins_cannot_download() {
        let pin = pinned_for("ja-ipadic").unwrap();
        if !is_published(&pin) {
            let e = download(&tmp("ph"), &pin, &mut |_, _| {}, &AtomicBool::new(false)).unwrap_err();
            assert!(e.to_string().contains("not been published"));
        }
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut w = zip::ZipWriter::new(File::create(path).unwrap());
        for (name, body) in entries {
            w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(body).unwrap();
        }
        w.finish().unwrap();
    }

    #[test]
    fn install_unpacks_a_pinned_package_and_replaces_old_versions() {
        let data = tmp("ok");
        let manifest = br#"{"id":"ja-ipadic","engine":"lindera-ipadic","version":"1","lindera":"6.2.0","files":["dict.trie"]}"#;
        let zip_path = data.join("p.zip");
        write_zip(&zip_path, &[("langdata.json", manifest), ("dict.trie", b"xyz"), ("LICENSE.txt", b"lic")]);
        let pin = pin_for(&fs::read(&zip_path).unwrap(), String::new());
        // An older version on disk goes away.
        fs::create_dir_all(install_dir(&data, "ja-ipadic", "0")).unwrap();
        assert_eq!(install_zip_with(&data, &zip_path, std::slice::from_ref(&pin)).unwrap(), "ja-ipadic");
        let dir = install_dir(&data, "ja-ipadic", "1");
        assert_eq!(fs::read(dir.join("dict.trie")).unwrap(), b"xyz");
        assert!(!install_dir(&data, "ja-ipadic", "0").exists());
        assert!(!dir.with_extension("installing").exists());
    }

    #[test]
    fn install_refuses_paths_that_escape_and_leaves_nothing_behind() {
        let data = tmp("evil");
        let manifest = br#"{"id":"ja-ipadic","engine":"lindera-ipadic","version":"1","files":["dict.trie"]}"#;
        for bad in ["../evil.txt", "sub/dict.trie"] {
            let zip_path = data.join("p.zip");
            write_zip(&zip_path, &[("langdata.json", manifest), ("dict.trie", b"x"), (bad, b"y")]);
            let pin = pin_for(&fs::read(&zip_path).unwrap(), String::new());
            assert!(install_zip_with(&data, &zip_path, &[pin]).is_err(), "{bad}");
            assert!(!install_dir(&data, "ja-ipadic", "1").exists());
            assert!(!data.join("evil.txt").exists());
        }
    }

    #[test]
    fn install_refuses_a_package_missing_listed_files() {
        let data = tmp("missing");
        let manifest = br#"{"id":"ja-ipadic","engine":"lindera-ipadic","version":"1","files":["dict.trie","dict.words"]}"#;
        let zip_path = data.join("p.zip");
        write_zip(&zip_path, &[("langdata.json", manifest), ("dict.trie", b"x")]);
        let pin = pin_for(&fs::read(&zip_path).unwrap(), String::new());
        assert!(install_zip_with(&data, &zip_path, &[pin]).unwrap_err().to_string().contains("missing dict.words"));
    }

    #[test]
    fn install_rejects_unpinned_and_unsafe_zips() {
        let data = tmp("inst");
        let zip_path = data.join("x.zip");
        {
            let mut w = zip::ZipWriter::new(File::create(&zip_path).unwrap());
            w.start_file("langdata.json", zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(b"{}").unwrap();
            w.finish().unwrap();
        }
        assert!(install_zip(&data, &zip_path).unwrap_err().to_string().contains("not a language data package"));
    }
}
