//! EPUB import: spine items become parts (SPEC §8). Images are not imported yet (needs the media store, M4).

use super::{ImportedPart, ImportedWork, html, split_parts};
use crate::{Error, Result};
use scraper::{Html, Selector};
use std::collections::HashMap;
use std::io::{Cursor, Read};
use zip::ZipArchive;

/// Chapters longer than this are split into several parts.
const CHAPTER_SPLIT: usize = 15_000;
const SPLIT_TO: usize = 8_000;
const MIN_CHARS: usize = 30;

fn err(msg: impl std::fmt::Display) -> Error {
    Error::Import(format!("EPUB: {msg}"))
}

pub fn import(bytes: &[u8], fallback_title: &str) -> Result<ImportedWork> {
    let mut zip = ZipArchive::new(Cursor::new(bytes)).map_err(err)?;
    let container = read_string(&mut zip, "META-INF/container.xml")?;
    let cdoc = roxmltree::Document::parse(&container).map_err(err)?;
    let opf_path = cdoc
        .descendants()
        .find(|n| n.has_tag_name("rootfile"))
        .and_then(|n| n.attribute("full-path"))
        .ok_or_else(|| err("no rootfile in container.xml"))?
        .to_string();
    let opf = read_string(&mut zip, &opf_path)?;
    let odoc = roxmltree::Document::parse(&opf).map_err(err)?;

    let meta = |tag: &str| {
        odoc.descendants()
            .find(|n| n.has_tag_name(tag))
            .and_then(|n| n.text())
            .map(html::clean)
            .filter(|s| !s.is_empty())
    };
    let title = meta("title").unwrap_or_else(|| fallback_title.to_string());
    let author = meta("creator");

    // id -> (href, is_nav)
    let manifest: HashMap<&str, (&str, bool)> = odoc
        .descendants()
        .filter(|n| n.has_tag_name("item"))
        .filter_map(|n| {
            let nav = n.attribute("properties").is_some_and(|p| p.split_whitespace().any(|x| x == "nav"));
            Some((n.attribute("id")?, (n.attribute("href")?, nav)))
        })
        .collect();
    let base = opf_path.rsplit_once('/').map_or("", |(d, _)| d);

    let mut parts = Vec::new();
    for itemref in odoc.descendants().filter(|n| n.has_tag_name("itemref")) {
        let Some(&(href, nav)) = itemref.attribute("idref").and_then(|id| manifest.get(id)) else { continue };
        if nav {
            continue;
        }
        let path = resolve(base, href);
        let Ok(xhtml) = read_string(&mut zip, &path) else { continue };
        let doc = Html::parse_document(&xhtml);
        let paras = html::paragraphs(doc.root_element());
        let body = paras.join("\n\n");
        if body.chars().count() < MIN_CHARS {
            continue;
        }
        let heading = ["h1", "h2", "h3", "title"].iter().find_map(|s| {
            doc.select(&Selector::parse(s).unwrap())
                .next()
                .map(|e| html::clean(&e.text().collect::<String>()))
                .filter(|t| !t.is_empty())
        });
        let chapter_title = heading.unwrap_or_else(|| format!("Part {}", parts.len() + 1));
        if body.chars().count() > CHAPTER_SPLIT {
            let pieces = split_parts(&body, SPLIT_TO);
            let n = pieces.len();
            for (i, text) in pieces.into_iter().enumerate() {
                parts.push(ImportedPart { title: Some(format!("{chapter_title} ({}/{n})", i + 1)), text });
            }
        } else {
            parts.push(ImportedPart { title: Some(chapter_title), text: body });
        }
    }
    if parts.is_empty() {
        return Err(err("no readable chapters found"));
    }
    Ok(ImportedWork { title, kind: "epub", author, source_url: None, parts })
}

fn read_string<R: Read + std::io::Seek>(zip: &mut ZipArchive<R>, path: &str) -> Result<String> {
    let mut f = zip.by_name(path).map_err(|_| err(format!("missing file {path}")))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).map_err(err)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Resolve a manifest href against the OPF folder: percent-decode, drop fragments, collapse `..`.
fn resolve(base: &str, href: &str) -> String {
    let href = href.split('#').next().unwrap_or("");
    let mut segs: Vec<String> = if base.is_empty() { vec![] } else { base.split('/').map(String::from).collect() };
    for seg in href.split('/') {
        match seg {
            "." | "" => {}
            ".." => {
                segs.pop();
            }
            s => segs.push(percent_decode(s)),
        }
    }
    segs.join("/")
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

    fn build_epub() -> Vec<u8> {
        let mut z = ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let mut add = |name: &str, body: &str| {
            z.start_file(name, opts).unwrap();
            z.write_all(body.as_bytes()).unwrap();
        };
        add("mimetype", "application/epub+zip");
        add("META-INF/container.xml", r#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#);
        add("OEBPS/content.opf", r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Cuentos breves</dc:title><dc:creator>A. Autor</dc:creator></metadata>
<manifest><item id="nav" href="nav.xhtml" properties="nav" media-type="application/xhtml+xml"/><item id="c1" href="text/cap%201.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="text/cap2.xhtml" media-type="application/xhtml+xml"/><item id="cover" href="cover.xhtml" media-type="application/xhtml+xml"/></manifest>
<spine><itemref idref="cover"/><itemref idref="nav"/><itemref idref="c1"/><itemref idref="c2"/></spine></package>"#);
        add("OEBPS/nav.xhtml", "<html><body><nav><ol><li>Uno</li></ol></nav></body></html>");
        add("OEBPS/cover.xhtml", "<html><body><img src='c.jpg'/></body></html>");
        add("OEBPS/text/cap 1.xhtml", "<html><head><title>x</title></head><body><h1>El comienzo</h1><p>Había una vez un coche muy viejo.</p><p>Nadie lo quería arreglar.</p></body></html>");
        add("OEBPS/text/cap2.xhtml", "<html><body><h2>El final</h2><p>Pero un mecánico lo arregló con paciencia y cariño.</p></body></html>");
        z.finish().unwrap().into_inner()
    }

    #[test]
    fn chapters_become_parts() {
        let w = import(&build_epub(), "fallback").unwrap();
        assert_eq!(w.title, "Cuentos breves");
        assert_eq!(w.author.as_deref(), Some("A. Autor"));
        assert_eq!(w.kind, "epub");
        let titles: Vec<_> = w.parts.iter().map(|p| p.title.clone().unwrap()).collect();
        assert_eq!(titles, vec!["El comienzo", "El final"]);
        assert_eq!(w.parts[0].text, "El comienzo\n\nHabía una vez un coche muy viejo.\n\nNadie lo quería arreglar.");
    }

    #[test]
    fn rejects_non_epub() {
        assert!(import(b"not a zip", "x").is_err());
    }

    #[test]
    fn resolves_paths() {
        assert_eq!(resolve("OEBPS", "../img/a%20b.png#x"), "img/a b.png");
        assert_eq!(resolve("", "a/./b.xhtml"), "a/b.xhtml");
    }
}
