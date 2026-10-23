//! YouTube subtitles through a `yt-dlp` binary the user installed (SPEC §8). Desktop only: the shell never
//! offers this on mobile. We only download subtitles, never the video.

use super::media::cues_as_text;
use super::ImportedWork;
use crate::cues;
use crate::{Error, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Look for `yt-dlp` on `PATH` and in the usual install folders (a GUI app on macOS does not get the
/// shell's `PATH`, so Homebrew's folders must be tried explicitly).
pub fn find() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    for d in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/snap/bin"] {
        dirs.push(PathBuf::from(d));
    }
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = PathBuf::from(home);
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join("bin"));
    }
    dirs.into_iter().map(|d| d.join(exe)).find(|p| p.is_file())
}

/// `yt-dlp --version`, or `None` if it doesn't run.
pub fn version(bin: &Path) -> Option<String> {
    let out = Command::new(bin).arg("--version").stdin(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Download the subtitles of `url` in `language` and make a text work. Manual subtitles are preferred;
/// automatic ones are used when there are none (their repeated rolling lines are removed).
pub fn import(bin: &Path, url: &str, language: &str) -> Result<ImportedWork> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(Error::Import("enter a YouTube link (https://…)".into()));
    }
    if !language.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(Error::Import("bad language code".into()));
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("alst-ytdlp-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| Error::Import(e.to_string()))?;
    let result = run(bin, url, language, &dir);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn run(bin: &Path, url: &str, language: &str, dir: &Path) -> Result<ImportedWork> {
    let mut child = Command::new(bin)
        .args(["--skip-download", "--write-subs", "--write-auto-subs", "--sub-format", "vtt", "--write-info-json", "--no-playlist", "--no-warnings", "--sub-langs"])
        .arg(format!("{language}.*,{language}"))
        .arg("-o")
        .arg(dir.join("%(id)s.%(ext)s"))
        .arg("--")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Import(format!("could not run yt-dlp: {e}")))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait().map_err(|e| Error::Import(e.to_string()))? {
            Some(s) => break s,
            None if started.elapsed() > Duration::from_secs(120) => {
                let _ = child.kill();
                return Err(Error::Import("yt-dlp timed out".into()));
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    };
    if !status.success() {
        let mut err = String::new();
        if let Some(mut s) = child.stderr.take() {
            use std::io::Read;
            let _ = s.read_to_string(&mut err);
        }
        let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("unknown error");
        return Err(Error::Import(format!("yt-dlp failed: {last}")));
    }
    let mut vtt: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| Error::Import(e.to_string()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "vtt"))
        .collect();
    // Prefer the plain `<id>.<lang>.vtt` over variants such as `<lang>-orig`.
    vtt.sort_by_key(|p| p.file_name().map_or(usize::MAX, |n| n.len()));
    let sub = vtt.first().ok_or_else(|| Error::Import(format!("this video has no '{language}' subtitles")))?;
    let info: serde_json::Value = std::fs::read_dir(dir)
        .ok()
        .and_then(|d| d.flatten().map(|e| e.path()).find(|p| p.to_string_lossy().ends_with(".info.json")))
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let manual = info.get("subtitles").and_then(|s| s.as_object()).is_some_and(|m| m.keys().any(|k| k == language || k.starts_with(&format!("{language}-"))));
    let bytes = std::fs::read(sub).map_err(|e| Error::Import(e.to_string()))?;
    let file = cues::parse("x.vtt", &bytes, !manual)?;
    let title = info.get("title").and_then(|t| t.as_str()).unwrap_or("YouTube video");
    let author = info.get("uploader").or_else(|| info.get("channel")).and_then(|t| t.as_str()).map(str::to_string);
    Ok(cues_as_text(title, author, Some(url.to_string()), &file.cues))
}
