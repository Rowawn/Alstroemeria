//! Markdown to plain text for embedded pack items (SPEC §10.2): headings and paragraphs become
//! paragraphs; links keep their text, and images, HTML, code fences and markup are dropped.

use super::{ImportedPart, ImportedWork, PART_CHARS, split_parts};
use regex::Regex;

pub fn to_text(md: &str) -> String {
    let image = Regex::new(r"!\[[^\]]*\]\([^)]*\)").unwrap();
    let link = Regex::new(r"\[([^\]]*)\]\([^)]*\)").unwrap();
    let html = Regex::new(r"<[^>\n]*>").unwrap();
    let list = Regex::new(r"^(?:[-*+]|\d+[.)])\s+").unwrap();
    let rule = Regex::new(r"^(?:[-*_]\s*){3,}$").unwrap();
    let mut paras: Vec<String> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut fenced = false;
    fn flush(paras: &mut Vec<String>, cur: &mut Vec<String>) {
        if !cur.is_empty() {
            paras.push(cur.join(" "));
            cur.clear();
        }
    }
    for raw in md.replace("\r\n", "\n").replace('\r', "\n").lines() {
        let line = raw.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
            flush(&mut paras, &mut cur);
            continue;
        }
        if fenced {
            continue;
        }
        if line.is_empty() || rule.is_match(line) {
            flush(&mut paras, &mut cur);
            continue;
        }
        let heading = line.starts_with('#');
        let mut s = line.trim_start_matches('#').trim_start_matches('>').trim().to_string();
        s = list.replace(&s, "").into_owned();
        s = image.replace_all(&s, "").into_owned();
        s = link.replace_all(&s, "$1").into_owned();
        s = html.replace_all(&s, "").into_owned();
        s = s.replace(['*', '`'], "");
        let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
        if s.is_empty() {
            continue;
        }
        if heading {
            flush(&mut paras, &mut cur);
            paras.push(s);
        } else {
            cur.push(s);
        }
    }
    flush(&mut paras, &mut cur);
    paras.join("\n\n")
}

/// A `text` work from Markdown.
pub fn import(title: &str, md: &str) -> ImportedWork {
    ImportedWork {
        title: title.trim().to_string(),
        kind: "text",
        author: None,
        source_url: None,
        parts: split_parts(&to_text(md), PART_CHARS).into_iter().map(|text| ImportedPart { title: None, text }).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_and_paragraphs_survive_everything_else_goes() {
        let md = "# Motor\n\nEl **motor** tiene una [bujía](https://x.org/b).\nSegunda línea.\n\n![foto](a.png)\n\n- uno\n- dos\n\n<div>hola</div>\n\n```\ncódigo\n```\n\n> cita\n";
        assert_eq!(to_text(md), "Motor\n\nEl motor tiene una bujía. Segunda línea.\n\nuno dos\n\nhola\n\ncita");
    }

    #[test]
    fn empty_input_gives_no_parts() {
        assert!(import("t", " \n\n").parts.is_empty());
    }
}
