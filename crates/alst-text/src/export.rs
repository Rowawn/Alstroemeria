//! Example sentences for export (SPEC §11): find the first sentence containing each wanted term.

use crate::{LanguageDef, reader};
use std::collections::{HashMap, HashSet};

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// For every key in `wanted` (a `text_lc`), the first sentence in `texts` that contains it, as HTML
/// with the term in `<b>`. Multi-word keys are matched with the same greedy matcher as the reader.
pub fn find_examples<'a>(
    lang: &LanguageDef,
    texts: impl IntoIterator<Item = &'a str>,
    wanted: &HashSet<String>,
) -> HashMap<String, String> {
    let matcher = lang.matcher(wanted.iter().map(String::as_str));
    let none = HashMap::new();
    let mut found: HashMap<String, String> = HashMap::new();
    for text in texts {
        if found.len() == wanted.len() {
            break;
        }
        let sentences = lang.tokenize(text);
        let units = reader::build_units(&sentences, &matcher, &none);
        // Units are in sentence order; group by sentence index.
        let mut by_sentence: HashMap<usize, Vec<&reader::Unit>> = HashMap::new();
        for u in &units {
            by_sentence.entry(u.sentence).or_default().push(u);
        }
        for si in 0..sentences.len() {
            let Some(units) = by_sentence.get(&si) else { continue };
            for hit in units.iter().filter(|u| u.key.as_ref().is_some_and(|k| wanted.contains(k))) {
                let key = hit.key.clone().unwrap();
                if found.contains_key(&key) {
                    continue;
                }
                let html: String = units
                    .iter()
                    .map(|u| {
                        let t = escape_html(&u.text);
                        if std::ptr::eq(*u, *hit) { format!("<b>{t}</b>") } else { t }
                    })
                    .collect();
                found.insert(key, html.trim().to_string());
            }
        }
    }
    found
}

/// One row of the Anki export.
#[derive(Debug, Clone)]
pub struct ExportRow {
    pub text: String,
    pub translation: String,
    pub notes: String,
    pub parents: Vec<String>,
    /// HTML, with the term in `<b>`; empty if no example was found.
    pub sentence: String,
    pub status: i64,
}

fn tsv_field(s: &str) -> String {
    s.replace('\t', " ").replace("\r\n", "<br>").replace(['\n', '\r'], "<br>")
}

/// Anki-importable TSV (text file import, with the `#` header directives Anki 2.1.54+ reads).
/// Columns: Term, Translation, Notes, Parents, Sentence, Tags.
pub fn to_tsv(language: &str, rows: &[ExportRow]) -> String {
    let mut out = String::from("#separator:tab\n#html:true\n#tags column:6\n");
    for r in rows {
        let fields = [
            tsv_field(&escape_html(&r.text)),
            tsv_field(&escape_html(&r.translation)),
            tsv_field(&escape_html(&r.notes)),
            tsv_field(&escape_html(&r.parents.join(", "))),
            tsv_field(&r.sentence),
            format!("alstroemeria {language} status{}", r.status),
        ];
        out.push_str(&fields.join("\t"));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_first_sentence_with_bold_term() {
        let es = LanguageDef::from_manifest(include_str!("../../../extensions/lang-es/extension.json")).unwrap();
        let wanted: HashSet<String> = ["taller", "por favor", "ausente"].iter().map(|s| s.to_string()).collect();
        let texts = ["Hola. Voy al Taller <hoy>.", "Dime, por\nfavor, qué pasó. El taller cerró."];
        let found = find_examples(&es, texts, &wanted);
        assert_eq!(found["taller"], "Voy al <b>Taller</b> &lt;hoy&gt;.");
        assert_eq!(found["por favor"], "Dime, <b>por\nfavor</b>, qué pasó.");
        assert!(!found.contains_key("ausente"));
    }

    #[test]
    fn tsv_escapes_and_has_headers() {
        let rows = [ExportRow {
            text: "a\tb".into(),
            translation: "x & y".into(),
            notes: "line1\nline2".into(),
            parents: vec!["p1".into(), "p2".into()],
            sentence: "una <b>a</b> frase".into(),
            status: 2,
        }];
        let tsv = to_tsv("es", &rows);
        let lines: Vec<&str> = tsv.lines().collect();
        assert_eq!(&lines[..3], ["#separator:tab", "#html:true", "#tags column:6"]);
        assert_eq!(lines[3], "a b\tx &amp; y\tline1<br>line2\tp1, p2\tuna <b>a</b> frase\talstroemeria es status2");
        assert_eq!(lines.len(), 4);
    }
}
