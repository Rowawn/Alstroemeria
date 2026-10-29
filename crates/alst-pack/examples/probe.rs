//! Helper for the `build-pack` skill (SPEC §10.6): prints short JSON summaries so the model never reads whole pages.
//!
//!   probe article <url>...     title, word count, first 300 characters, ok/error for each page
//!   probe rss <feed-url>       the feed's latest episodes (at most 20): guid, title, minutes, hasTranscript
//!   probe build <manifest.json> <out.alpack> [items-dir]   validate with the app's own reader and zip
//!   probe validate <file.alpack>                           open a pack the way the app does

use alst_pack::manifest;
use alst_text::import::{article, rss};
use serde_json::json;
use std::io::Write;
use std::path::Path;

fn minutes(d: &str) -> Option<u32> {
    let parts: Vec<u32> = d.split(':').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    let secs = match parts.as_slice() {
        [s] => *s,
        [m, s] => m * 60 + s,
        [h, m, s] => h * 3600 + m * 60 + s,
        _ => return None,
    };
    Some((secs + 30) / 60)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = match args.first().map(String::as_str) {
        Some("article") => json!(args[1..].iter().map(|u| probe_article(u)).collect::<Vec<_>>()),
        Some("rss") if args.len() == 2 => probe_rss(&args[1]),
        Some("build") if args.len() >= 3 => build(Path::new(&args[1]), Path::new(&args[2]), args.get(3).map(Path::new)),
        Some("validate") if args.len() == 2 => match alst_pack::Pack::open(Path::new(&args[1])) {
            Ok(p) => json!({ "ok": true, "items": p.manifest.items.len(), "vocab": p.manifest.vocab.len(), "title": p.manifest.title }),
            Err(e) => json!({ "ok": false, "error": e.to_string() }),
        },
        _ => {
            eprintln!("usage: probe article <url>... | rss <feed> | build <manifest.json> <out.alpack> [items-dir] | validate <pack>");
            std::process::exit(2);
        }
    };
    println!("{out}");
}

fn probe_article(url: &str) -> serde_json::Value {
    match article::fetch(url) {
        Ok(w) => {
            let text: String = w.parts.iter().map(|p| p.text.as_str()).collect::<Vec<_>>().join("\n\n");
            let words = text.split_whitespace().count();
            let excerpt: String = text.chars().take(300).collect::<String>().replace('\n', " ");
            json!({ "url": url, "ok": true, "title": w.title, "words": words, "excerpt": excerpt })
        }
        Err(e) => json!({ "url": url, "ok": false, "error": e.to_string() }),
    }
}

fn probe_rss(url: &str) -> serde_json::Value {
    match rss::fetch_feed(url) {
        Ok(f) => json!({
            "ok": true,
            "feed": f.title,
            "episodes": f.episodes.iter().take(20).map(|e| json!({
                "guid": e.guid,
                "title": e.title,
                "minutes": e.duration.as_deref().and_then(minutes),
                "hasTranscript": e.best_transcript().is_some(),
                "hasAudio": e.audio_url.is_some(),
            })).collect::<Vec<_>>(),
        }),
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    }
}

fn collect(dir: &Path, base: &Path, out: &mut Vec<(String, std::path::PathBuf)>) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_dir() {
            collect(&p, base, out)?;
        } else {
            out.push((p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"), p));
        }
    }
    Ok(())
}

fn build(manifest_path: &Path, out: &Path, items_dir: Option<&Path>) -> serde_json::Value {
    let run = || -> Result<serde_json::Value, String> {
        let raw = std::fs::read(manifest_path).map_err(|e| e.to_string())?;
        let mut files: Vec<(String, std::path::PathBuf)> = Vec::new();
        if let Some(d) = items_dir.filter(|d| d.is_dir()) {
            collect(d, d.parent().unwrap_or(d), &mut files).map_err(|e| e.to_string())?;
        }
        let m = manifest::parse(&raw, &|p| files.iter().any(|(n, _)| n == p)).map_err(|e| e.to_string())?;
        let mut zip = zip::ZipWriter::new(std::fs::File::create(out).map_err(|e| e.to_string())?);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("manifest.json", opts).map_err(|e| e.to_string())?;
        zip.write_all(&raw).map_err(|e| e.to_string())?;
        for (name, path) in &files {
            zip.start_file(name, opts).map_err(|e| e.to_string())?;
            zip.write_all(&std::fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        }
        zip.finish().map_err(|e| e.to_string())?;
        // Open it again exactly as the app will.
        alst_pack::Pack::open(out).map_err(|e| e.to_string())?;
        let size = std::fs::metadata(out).map_err(|e| e.to_string())?.len();
        Ok(json!({ "ok": true, "items": m.items.len(), "vocab": m.vocab.len(), "bytes": size, "path": out }))
    };
    run().unwrap_or_else(|e| json!({ "ok": false, "error": e }))
}
