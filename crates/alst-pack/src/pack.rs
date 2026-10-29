//! An opened `.alpack`: the zip stays on disk; `items/` files are read on demand.

use crate::manifest::{self, Manifest, is_safe_path};
use crate::{Error, Result};
use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const MAX_PACK_BYTES: u64 = 200 * 1024 * 1024;
const MAX_ENTRIES: usize = 1000;

#[derive(Debug)]
pub struct Pack {
    pub manifest: Manifest,
    path: PathBuf,
}

fn bad<T>(m: impl Into<String>) -> Result<T> {
    Err(Error::Invalid(m.into()))
}

impl Pack {
    /// Open and validate. Rejects non-zips, files over 200 MB (compressed or declared), too many entries,
    /// absolute paths, `..`, backslashes, symlinks, duplicate names and an invalid manifest.
    pub fn open(path: &Path) -> Result<Pack> {
        if std::fs::metadata(path)?.len() > MAX_PACK_BYTES {
            return bad("pack is larger than 200 MB");
        }
        let mut zip = zip::ZipArchive::new(File::open(path)?).map_err(|e| Error::Invalid(format!("not a pack (zip): {e}")))?;
        if zip.len() > MAX_ENTRIES {
            return bad(format!("too many files in the pack ({} > {MAX_ENTRIES})", zip.len()));
        }
        let mut names = HashSet::new();
        let mut declared = 0u64;
        for i in 0..zip.len() {
            let f = zip.by_index_raw(i).map_err(|e| Error::Invalid(e.to_string()))?;
            let name = f.name().to_string();
            let p = name.trim_end_matches('/');
            if !is_safe_path(p) {
                return bad(format!("unsafe path in pack: '{name}'"));
            }
            if f.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
                return bad(format!("symbolic links are not allowed: '{name}'"));
            }
            declared += f.size();
            if !name.ends_with('/') && !names.insert(p.to_string()) {
                return bad(format!("duplicate entry '{p}'"));
            }
        }
        if declared > MAX_PACK_BYTES {
            return bad(format!("pack is too large: {} MB uncompressed (limit 200 MB)", declared / 1_048_576));
        }
        if !names.contains("manifest.json") {
            return bad("manifest.json not found at the root of the pack");
        }
        let raw = read_entry(&mut zip, "manifest.json", manifest::MAX_MANIFEST_BYTES)?;
        let manifest = manifest::parse(&raw, &|p| names.contains(p))?;
        Ok(Pack { manifest, path: path.to_path_buf() })
    }

    /// Bytes of an entry under `items/`; `max` bounds what is read.
    pub fn read(&self, path: &str, max: u64) -> Result<Vec<u8>> {
        if !is_safe_path(path) || !path.starts_with("items/") {
            return bad(format!("unsafe path '{path}'"));
        }
        let mut zip = zip::ZipArchive::new(File::open(&self.path)?).map_err(|e| Error::Invalid(e.to_string()))?;
        read_entry(&mut zip, path, max)
    }

    /// Copy an entry to `dest` (for large media files), bounded by `max`.
    pub fn extract(&self, path: &str, dest: &Path, max: u64) -> Result<u64> {
        if !is_safe_path(path) || !path.starts_with("items/") {
            return bad(format!("unsafe path '{path}'"));
        }
        let mut zip = zip::ZipArchive::new(File::open(&self.path)?).map_err(|e| Error::Invalid(e.to_string()))?;
        let f = zip.by_name(path).map_err(|_| Error::Invalid(format!("'{path}' is not in the pack")))?;
        let mut out = File::create(dest)?;
        let n = std::io::copy(&mut f.take(max + 1), &mut out)?;
        if n > max {
            return bad(format!("'{path}' is too large"));
        }
        Ok(n)
    }
}

fn read_entry<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, name: &str, max: u64) -> Result<Vec<u8>> {
    let f = zip.by_name(name).map_err(|_| Error::Invalid(format!("'{name}' is not in the pack")))?;
    let mut buf = Vec::new();
    f.take(max + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > max {
        return bad(format!("'{name}' is too large"));
    }
    Ok(buf)
}
