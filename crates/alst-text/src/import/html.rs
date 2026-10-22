//! HTML → paragraphs, shared by the EPUB and article importers.

use scraper::{ElementRef, Node};

const SKIP: &[&str] = &["script", "style", "nav", "footer", "aside", "form", "noscript", "svg", "template", "head", "button", "iframe"];
const BLOCK: &[&str] = &[
    "p", "div", "section", "article", "main", "body", "h1", "h2", "h3", "h4", "h5", "h6", "li", "ul", "ol",
    "blockquote", "table", "tr", "td", "th", "figure", "figcaption", "pre", "dd", "dt", "dl", "header", "hr", "br",
];
const BOILERPLATE: &[&str] = &[
    "comment", "sidebar", "share", "social", "advert", "cookie", "related", "menu", "promo", "newsletter",
    "subscribe", "breadcrumb", "toolbar",
];

pub fn clean(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_boilerplate(el: &ElementRef) -> bool {
    let v = el.value();
    let attrs = format!("{} {}", v.attr("class").unwrap_or(""), v.attr("id").unwrap_or("")).to_lowercase();
    BOILERPLATE.iter().any(|b| attrs.contains(b))
}

/// Paragraphs of readable text below `root`, in document order.
pub fn paragraphs(root: ElementRef) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    walk(root, &mut out, &mut cur, true);
    flush(&mut out, &mut cur);
    out
}

fn flush(out: &mut Vec<String>, cur: &mut String) {
    let p = clean(cur);
    cur.clear();
    if !p.is_empty() {
        out.push(p);
    }
}

fn walk(el: ElementRef, out: &mut Vec<String>, cur: &mut String, is_root: bool) {
    let name = el.value().name();
    if !is_root && (SKIP.contains(&name) || is_boilerplate(&el)) {
        return;
    }
    let block = BLOCK.contains(&name);
    if block {
        flush(out, cur);
    }
    for child in el.children() {
        match child.value() {
            Node::Text(t) => cur.push_str(t),
            Node::Element(_) => walk(ElementRef::wrap(child).unwrap(), out, cur, false),
            _ => {}
        }
    }
    if block {
        flush(out, cur);
    }
}
