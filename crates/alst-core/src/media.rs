//! The content-addressed media store (SPEC §8): files live at `<root>/<ab>/<sha256>` with no extension,
//! so identical files are stored once. Metadata (mime, size, name) is the synced `media` row, written by
//! `Store::register_media`; this module only touches the file system.

use crate::{Error, Result};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct MediaDir {
    root: PathBuf,
}

fn io(e: std::io::Error) -> Error {
    Error::Invalid(format!("media: {e}"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn is_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl MediaDir {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// Where a file with this hash lives (it may not exist). `None` if `hash` isn't a sha256 hex digest,
    /// which keeps request paths from escaping the media directory.
    pub fn path(&self, hash: &str) -> Option<PathBuf> {
        is_hash(hash).then(|| self.root.join(&hash[..2]).join(hash))
    }

    pub fn exists(&self, hash: &str) -> bool {
        self.path(hash).is_some_and(|p| p.is_file())
    }

    /// Remove half-finished uploads (call at startup).
    pub fn clear_tmp(&self) {
        let _ = fs::remove_dir_all(self.tmp_dir());
    }

    fn tmp_path(&self, id: &str) -> Result<PathBuf> {
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err(Error::Invalid("bad upload id".into()));
        }
        Ok(self.tmp_dir().join(id))
    }

    /// Start a chunked upload; returns its id.
    pub fn begin(&self) -> Result<String> {
        fs::create_dir_all(self.tmp_dir()).map_err(io)?;
        let id = uuid::Uuid::new_v4().to_string();
        File::create(self.tmp_path(&id)?).map_err(io)?;
        Ok(id)
    }

    pub fn append(&self, id: &str, chunk: &[u8]) -> Result<()> {
        let mut f = fs::OpenOptions::new().append(true).open(self.tmp_path(id)?).map_err(|_| Error::Invalid("unknown upload".into()))?;
        f.write_all(chunk).map_err(io)
    }

    /// Drop a staged upload (after a failed import).
    pub fn abort(&self, id: &str) {
        if let Ok(p) = self.tmp_path(id) {
            let _ = fs::remove_file(p);
        }
    }

    /// Finish an upload: hash the staged file and move it into place. Returns `(hash, size)`.
    pub fn finish(&self, id: &str) -> Result<(String, u64)> {
        let tmp = self.tmp_path(id)?;
        let mut f = File::open(&tmp).map_err(|_| Error::Invalid("unknown upload".into()))?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        let mut size = 0u64;
        loop {
            let n = f.read(&mut buf).map_err(io)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            size += n as u64;
        }
        drop(f);
        self.commit(&tmp, hex(&hasher.finalize()), size)
    }

    /// Stream `reader` into the store, refusing more than `max` bytes. Returns `(hash, size)`.
    pub fn put_reader(&self, mut reader: impl Read, max: u64) -> Result<(String, u64)> {
        let id = self.begin()?;
        let tmp = self.tmp_path(&id)?;
        let result = (|| {
            let mut out = File::create(&tmp).map_err(io)?;
            let mut hasher = Sha256::new();
            let mut buf = vec![0u8; 1 << 20];
            let mut size = 0u64;
            loop {
                let n = reader.read(&mut buf).map_err(io)?;
                if n == 0 {
                    break;
                }
                size += n as u64;
                if size > max {
                    return Err(Error::Invalid(format!("file is larger than the {} MB limit", max >> 20)));
                }
                hasher.update(&buf[..n]);
                out.write_all(&buf[..n]).map_err(io)?;
            }
            Ok((hex(&hasher.finalize()), size))
        })();
        match result {
            Ok((hash, size)) => self.commit(&tmp, hash, size),
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                Err(e)
            }
        }
    }

    pub fn put_bytes(&self, bytes: &[u8], max: u64) -> Result<(String, u64)> {
        self.put_reader(bytes, max)
    }

    fn commit(&self, tmp: &Path, hash: String, size: u64) -> Result<(String, u64)> {
        let dest = self.path(&hash).expect("hex digest");
        if dest.is_file() {
            let _ = fs::remove_file(tmp);
        } else {
            fs::create_dir_all(dest.parent().expect("has parent")).map_err(io)?;
            fs::rename(tmp, &dest).map_err(io)?;
        }
        Ok((hash, size))
    }

    /// Read up to `len` bytes from `start`. Returns the bytes and the file's total size.
    pub fn read_range(&self, hash: &str, start: u64, len: u64) -> Result<(Vec<u8>, u64)> {
        let path = self.path(hash).ok_or_else(|| Error::Invalid("bad media id".into()))?;
        let mut f = File::open(path).map_err(|_| Error::Invalid("media file is missing on this device".into()))?;
        let total = f.metadata().map_err(io)?.len();
        if start >= total {
            return Ok((Vec::new(), total));
        }
        f.seek(SeekFrom::Start(start)).map_err(io)?;
        let mut out = Vec::with_capacity(len.min(total - start) as usize);
        f.take(len).read_to_end(&mut out).map_err(io)?;
        Ok((out, total))
    }
}

/// MIME type from a file name, falling back to the browser-reported type (`hint`). `None` if neither is
/// audio or video.
pub fn mime_for(name: &str, hint: &str) -> Option<String> {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    let by_ext = match ext.as_str() {
        "mp3" => "audio/mpeg",
        "m4a" | "m4b" => "audio/mp4",
        "aac" => "audio/aac",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/ogg; codecs=opus",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "weba" => "audio/webm",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "ogv" => "video/ogg",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        _ => "",
    };
    if !by_ext.is_empty() {
        return Some(by_ext.into());
    }
    let hint = hint.trim().to_lowercase();
    (hint.starts_with("audio/") || hint.starts_with("video/")).then_some(hint)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> (MediaDir, PathBuf) {
        let p = std::env::temp_dir().join(format!("alst-media-{}", uuid::Uuid::new_v4()));
        (MediaDir::new(&p), p)
    }

    const HELLO: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    #[test]
    fn chunked_upload_is_content_addressed_and_deduplicated() {
        let (m, root) = dir();
        let id = m.begin().unwrap();
        m.append(&id, b"hel").unwrap();
        m.append(&id, b"lo").unwrap();
        let (hash, size) = m.finish(&id).unwrap();
        assert_eq!((hash.as_str(), size), (HELLO, 5));
        assert_eq!(m.path(&hash).unwrap(), root.join("2c").join(HELLO));
        assert!(m.exists(&hash));
        // The same content again stores nothing new and leaves no staging file.
        let (h2, _) = m.put_bytes(b"hello", 100).unwrap();
        assert_eq!(h2, hash);
        assert_eq!(fs::read_dir(root.join("tmp")).unwrap().count(), 0);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn size_limit_cleans_up_and_bad_ids_are_rejected() {
        let (m, root) = dir();
        assert!(m.put_bytes(&[0u8; 50], 10).is_err());
        assert_eq!(fs::read_dir(root.join("tmp")).unwrap().count(), 0);
        assert!(m.append("../x", b"a").is_err());
        assert!(m.path("../../etc/passwd").is_none());
        assert!(m.read_range("zz", 0, 1).is_err());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn range_reads() {
        let (m, root) = dir();
        let (h, _) = m.put_bytes(b"0123456789", 100).unwrap();
        assert_eq!(m.read_range(&h, 2, 3).unwrap(), (b"234".to_vec(), 10));
        assert_eq!(m.read_range(&h, 8, 100).unwrap(), (b"89".to_vec(), 10));
        assert_eq!(m.read_range(&h, 10, 5).unwrap().0, Vec::<u8>::new());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn mime_by_extension_then_hint() {
        assert_eq!(mime_for("a.MP3", "").as_deref(), Some("audio/mpeg"));
        assert_eq!(mime_for("clip", "video/mp4").as_deref(), Some("video/mp4"));
        assert_eq!(mime_for("notes.txt", "text/plain"), None);
    }
}
