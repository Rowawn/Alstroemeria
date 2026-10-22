//! Web article import: fetch a URL and extract the main readable text.

use super::{ImportedPart, ImportedWork, PART_CHARS, html, split_parts};
use crate::{Error, Result};
use scraper::{ElementRef, Html, Selector};
use std::io::Read;
use std::time::Duration;

const MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Fetch `url` (http/https only) and extract the article.
pub fn fetch(url: &str) -> Result<ImportedWork> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(Error::Import("only http(s) URLs can be imported".into()));
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .user_agent("Mozilla/5.0 (compatible; Alstroemeria)")
        .build()
        .into();
    let mut resp = agent.get(url).call().map_err(|e| Error::Import(format!("fetch failed: {e}")))?;
    let content_type = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let mut bytes = Vec::new();
    resp.body_mut().as_reader().take(MAX_BYTES).read_to_end(&mut bytes).map_err(|e| Error::Import(e.to_string()))?;
    extract(&decode(&bytes, &content_type), url)
}

/// Decode with the charset from the Content-Type header or a `<meta charset>` sniff; UTF-8 otherwise.
fn decode(bytes: &[u8], content_type: &str) -> String {
    let from_header = content_type.to_lowercase().split("charset=").nth(1).map(|s| s.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_').to_string());
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(2048)]).to_lowercase();
    let from_meta = head.split("charset=").nth(1).map(|s| s.trim_start_matches(['"', '\'']).chars().take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_').collect::<String>());
    let label = from_header.or(from_meta).unwrap_or_default();
    match encoding_rs::Encoding::for_label(label.as_bytes()) {
        Some(enc) => enc.decode(bytes).0.into_owned(),
        None => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Extract title, author and body text from an HTML page.
pub fn extract(page: &str, url: &str) -> Result<ImportedWork> {
    let doc = Html::parse_document(page);
    let sel = |s: &str| Selector::parse(s).unwrap();
    let meta = |s: &str| doc.select(&sel(s)).next().and_then(|m| m.value().attr("content")).map(html::clean).filter(|s| !s.is_empty());
    let text_of = |s: &str| doc.select(&sel(s)).next().map(|e| html::clean(&e.text().collect::<String>())).filter(|s| !s.is_empty());

    let root = best_root(&doc);
    let mut paras = html::paragraphs(root);
    let title = meta(r#"meta[property="og:title"]"#)
        .or_else(|| text_of("h1"))
        .or_else(|| text_of("title"))
        .unwrap_or_else(|| url.to_string());
    // The extracted body often repeats the headline as its first paragraph.
    if paras.first().is_some_and(|p| *p == title) {
        paras.remove(0);
    }
    let body = paras.join("\n\n");
    if body.chars().count() < 80 {
        return Err(Error::Import("no readable article text found on that page".into()));
    }
    Ok(ImportedWork {
        title,
        kind: "article",
        author: meta(r#"meta[name="author"]"#),
        source_url: Some(url.to_string()),
        parts: split_parts(&body, PART_CHARS).into_iter().map(|text| ImportedPart { title: None, text }).collect(),
    })
}

fn text_len(e: ElementRef) -> usize {
    e.text().map(|t| t.trim().chars().count()).sum()
}

/// `<article>`, else `<main>`, else the container whose direct `<p>` children hold the most text.
fn best_root(doc: &Html) -> ElementRef<'_> {
    let pick = |s: &str, min: usize| {
        doc.select(&Selector::parse(s).unwrap()).max_by_key(|e| text_len(*e)).filter(|e| text_len(*e) >= min)
    };
    if let Some(e) = pick("article", 200).or_else(|| pick("main", 200)) {
        return e;
    }
    let p = Selector::parse("p").unwrap();
    let mut best: Option<(usize, ElementRef)> = None;
    for container in doc.select(&Selector::parse("div, section, td, body").unwrap()) {
        let score: usize = container
            .children()
            .filter_map(ElementRef::wrap)
            .filter(|c| p.matches(c))
            .map(text_len)
            .sum();
        if best.is_none_or(|(s, _)| score > s) {
            best = Some((score, container));
        }
    }
    best.map(|(_, e)| e).unwrap_or_else(|| doc.root_element())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    const PAGE: &str = r#"<html><head><title>Site | Noticias</title><meta name="author" content="Ana Pérez"></head>
<body><nav><a href="/">Inicio</a> <a href="/x">Menú largo de navegación</a></nav>
<div class="sidebar"><p>Suscríbete a nuestro boletín ahora mismo y recibe ofertas exclusivas todos los días.</p></div>
<div id="content"><h1>El taller de la esquina</h1>
<p>El taller de la esquina abrió sus puertas en 1985 y desde entonces repara coches de todas las marcas.</p>
<p>Los clientes dicen que el servicio es honesto &amp; rápido, y que los precios son justos para todos.</p></div>
<footer><p>Copyright 2026</p></footer></body></html>"#;

    #[test]
    fn extracts_main_text_without_boilerplate() {
        let w = extract(PAGE, "https://example.test/a").unwrap();
        assert_eq!(w.title, "El taller de la esquina");
        assert_eq!(w.author.as_deref(), Some("Ana Pérez"));
        assert_eq!(w.kind, "article");
        let text = &w.parts[0].text;
        assert!(text.starts_with("El taller de la esquina abrió"));
        assert!(text.contains("honesto & rápido"));
        assert!(!text.contains("Suscríbete") && !text.contains("Copyright") && !text.contains("Menú"));
        assert_eq!(text.matches("\n\n").count(), 1);
    }

    #[test]
    fn prefers_article_element() {
        let page = format!("<body><div><p>{}</p></div><article><h1>T</h1><p>{}</p></article></body>", "x ".repeat(300), "y ".repeat(120));
        let w = extract(&page, "https://example.test").unwrap();
        assert!(w.parts[0].text.starts_with('y'));
    }

    #[test]
    fn rejects_pages_without_text() {
        assert!(extract("<html><body><p>hola</p></body></html>", "https://e.test").is_err());
    }

    #[test]
    fn rejects_non_http_urls() {
        assert!(fetch("file:///etc/passwd").is_err());
    }

    #[test]
    fn decodes_latin1_from_header() {
        let bytes = [b'a', 0xF1, b'o']; // "año" in ISO-8859-1
        assert_eq!(decode(&bytes, "text/html; charset=ISO-8859-1"), "año");
    }

    #[test]
    fn fetches_from_a_local_server() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            let body = PAGE.as_bytes();
            let head = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            s.write_all(head.as_bytes()).unwrap();
            s.write_all(body).unwrap();
        });
        let w = fetch(&format!("http://127.0.0.1:{port}/a")).unwrap();
        handle.join().unwrap();
        assert_eq!(w.title, "El taller de la esquina");
        assert_eq!(w.source_url.as_deref(), Some(format!("http://127.0.0.1:{port}/a").as_str()));
    }
}
