//! Importers (SPEC §8): plain text, EPUB, web articles. They return plain data;
//! the caller writes it through `alst-core::store`.

pub mod article;
pub mod epub;
mod html;
pub mod markdown;
pub mod media;
pub mod rss;
pub mod ytdlp;

use regex::Regex;

#[derive(Debug, Clone, PartialEq)]
pub struct ImportedPart {
    pub title: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportedWork {
    pub title: String,
    pub kind: &'static str,
    pub author: Option<String>,
    pub source_url: Option<String>,
    pub parts: Vec<ImportedPart>,
}

/// Default size of a part when splitting long text.
pub const PART_CHARS: usize = 5000;

/// Plain text or pasted text, split into parts.
pub fn from_text(title: &str, text: &str) -> ImportedWork {
    ImportedWork {
        title: title.trim().to_string(),
        kind: "text",
        author: None,
        source_url: None,
        parts: split_parts(text, PART_CHARS).into_iter().map(|text| ImportedPart { title: None, text }).collect(),
    }
}

/// Split on blank lines into parts of at most `max_chars` characters (paragraphs are kept whole
/// unless one alone exceeds the limit, in which case it is cut at a sentence or word boundary).
pub fn split_parts(text: &str, max_chars: usize) -> Vec<String> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let blank = Regex::new(r"\n[ \t]*\n").unwrap();
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_len = 0;
    for para in blank.split(&text).map(str::trim).filter(|p| !p.is_empty()) {
        for piece in cut_paragraph(para, max_chars) {
            let len = piece.chars().count();
            if cur_len > 0 && cur_len + 2 + len > max_chars {
                parts.push(std::mem::take(&mut cur));
                cur_len = 0;
            }
            if cur_len > 0 {
                cur.push_str("\n\n");
                cur_len += 2;
            }
            cur.push_str(&piece);
            cur_len += len;
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
}

fn cut_paragraph(para: &str, max_chars: usize) -> Vec<String> {
    if para.chars().count() <= max_chars {
        return vec![para.to_string()];
    }
    let mut out = Vec::new();
    let mut rest = para;
    while rest.chars().count() > max_chars {
        let limit = rest.char_indices().nth(max_chars).map_or(rest.len(), |(i, _)| i);
        let window = &rest[..limit];
        let cut = window
            .rmatch_indices(['.', '!', '?', '…'])
            .map(|(i, m)| i + m.len())
            .find(|&i| window[i..].starts_with(char::is_whitespace))
            .or_else(|| window.rfind(char::is_whitespace))
            .filter(|&i| i > 0)
            .unwrap_or(limit);
        out.push(rest[..cut].trim().to_string());
        rest = rest[cut..].trim_start();
    }
    if !rest.is_empty() {
        out.push(rest.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_paragraph_boundaries() {
        let text = "uno uno\n\ndos dos\n\ntres tres";
        assert_eq!(split_parts(text, 100), vec!["uno uno\n\ndos dos\n\ntres tres"]);
        assert_eq!(split_parts(text, 10), vec!["uno uno", "dos dos", "tres tres"]);
    }

    #[test]
    fn cuts_oversized_paragraph_at_sentence_end() {
        let text = "Hola mundo. Adiós mundo. Otra vez.";
        let parts = split_parts(text, 20);
        assert_eq!(parts, vec!["Hola mundo.", "Adiós mundo.", "Otra vez."]);
        assert_eq!(parts.join(" "), text);
    }

    #[test]
    fn cuts_without_punctuation_at_whitespace_and_never_loses_text() {
        let text = "a ".repeat(50);
        let parts = split_parts(&text, 15);
        assert!(parts.iter().all(|p| p.chars().count() <= 15));
        assert_eq!(parts.join(" ").split_whitespace().count(), 50);
    }

    #[test]
    fn empty_and_crlf() {
        assert!(split_parts("  \n\n ", 10).is_empty());
        assert_eq!(split_parts("a\r\n\r\nb", 100), vec!["a\n\nb"]);
    }
}
