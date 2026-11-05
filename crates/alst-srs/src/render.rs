//! Rendering cards for the review screen (SPEC §11.4, §11.8): classic cards from their note type's templates,
//! concept cards from the built-in recognition and production templates.

use crate::concept::{self, ConceptCard};
use crate::pool::{Example, Segmenter, utf16_slice};
use crate::template::{self, Ctx, escape};
use crate::{Error, Result};
use alst_core::Store;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rendered {
    pub front: String,
    pub back: String,
    pub css: String,
    /// "classic" or "concept"
    pub kind: String,
    pub deck: String,
    pub note_id: Option<String>,
    /// Concept cards: the example shown, to send back with the answer.
    pub example: Option<Example>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TemplateDef {
    pub name: String,
    pub qfmt: String,
    pub afmt: String,
}

pub const CONCEPT_CSS: &str = ".card{font-family:system-ui,sans-serif;font-size:1.2em;text-align:center}.word{font-size:1.8em}\
.sentence b,.sentence .target{color:#c0392b}.blank{border-bottom:2px solid currentColor;padding:0 .8em}.reading,.source{opacity:.7;font-size:.85em}";

pub const RECOGNITION: (&str, &str) = (
    "{{#Sentence}}<div class=\"sentence\">{{Sentence}}</div>{{/Sentence}}{{^Sentence}}<div class=\"word\">{{Word}}</div>{{/Sentence}}",
    "{{FrontSide}}<hr id=answer><div class=\"word\">{{Word}}</div>{{#Reading}}<div class=\"reading\">{{Reading}}</div>{{/Reading}}\
<div class=\"meaning\">{{Meaning}}</div>{{Audio}}{{#Source}}<div class=\"source\">{{Source}} {{Open}}</div>{{/Source}}",
);

pub const PRODUCTION: (&str, &str) = (
    "<div class=\"meaning\">{{Meaning}}</div>{{#SentenceBlank}}<div class=\"sentence\">{{SentenceBlank}}</div>{{/SentenceBlank}}",
    "{{FrontSide}}<hr id=answer><div class=\"word\">{{Word}}</div>{{#Reading}}<div class=\"reading\">{{Reading}}</div>{{/Reading}}\
{{#Sentence}}<div class=\"sentence\">{{Sentence}}</div>{{/Sentence}}{{Audio}}{{#Source}}<div class=\"source\">{{Source}} {{Open}}</div>{{/Source}}",
);

/// The sentence as HTML with the target word wrapped (`<b>` or a blank).
fn mark_target(ex: &Example, blank: bool) -> String {
    match ex.target {
        Some((a, b)) => {
            let units = ex.text.encode_utf16().count();
            let word = escape(&utf16_slice(&ex.text, a, b));
            let mid = if blank { "<span class=\"blank\">&nbsp;</span>".to_string() } else { format!("<b>{word}</b>") };
            format!("{}{}{}", escape(&utf16_slice(&ex.text, 0, a)), mid, escape(&utf16_slice(&ex.text, b, units)))
        }
        None => escape(&ex.text),
    }
}

/// Render a concept card with the template of its direction.
pub fn render_concept(cc: &ConceptCard, deck: &str) -> Rendered {
    let ex = cc.example.as_ref();
    let mut fields: Vec<(String, String)> = vec![
        ("Word".into(), escape(&cc.term)),
        ("Reading".into(), escape(cc.reading.as_deref().unwrap_or(""))),
        ("Meaning".into(), escape(cc.meaning.as_deref().unwrap_or(""))),
        ("Sentence".into(), ex.map(|e| mark_target(e, false)).unwrap_or_default()),
        ("SentenceBlank".into(), ex.map(|e| mark_target(e, true)).unwrap_or_default()),
        ("Source".into(), ex.and_then(|e| e.source.clone()).map(|s| escape(&s)).unwrap_or_else(|| if ex.is_some_and(|e| e.mined) { "mined".into() } else { String::new() })),
        ("Audio".into(), String::new()),
        ("Open".into(), String::new()),
    ];
    if let Some(e) = ex {
        if let (Some(h), Some((s, t))) = (&e.media_hash, e.cue) {
            fields[6].1 = format!("[sound:alstclip:{h}:{s}:{t}]");
        }
        if !e.part_id.starts_with("note:") {
            fields[7].1 = format!("<a class=\"alst-open\" href=\"#\" data-part=\"{}\" data-start=\"{}\">Open in reader</a>", escape(&e.part_id), e.start);
        }
    }
    let (qfmt, afmt) = if cc.direction == "production" { PRODUCTION } else { RECOGNITION };
    let (name, kind) = if cc.direction == "production" { ("Production", "Concept production") } else { ("Recognition", "Concept recognition") };
    let front = template::render(qfmt, &Ctx { fields: &fields, tags: "", deck, card_name: name, note_type: kind, ord: 0, front: None });
    let back = template::render(afmt, &Ctx { fields: &fields, tags: "", deck, card_name: name, note_type: kind, ord: 0, front: Some(&front) });
    Rendered { front, back, css: CONCEPT_CSS.into(), kind: "concept".into(), deck: deck.into(), note_id: None, example: cc.example.clone() }
}

/// The card ordinals a note yields: a standard type one per template whose question side renders something with
/// a non-empty field; a cloze type one per cloze number found in its fields.
pub fn card_ords(kind: &str, fields: &[(String, String)], templates: &[TemplateDef]) -> Vec<usize> {
    if kind == "cloze" {
        let nums: std::collections::BTreeSet<u32> = fields.iter().flat_map(|(_, v)| template::cloze_numbers(v)).collect();
        return nums.into_iter().map(|n| n as usize - 1).collect();
    }
    templates
        .iter()
        .enumerate()
        .filter(|(_, t)| {
            // A template yields a card when a field it uses (outside `{{^…}}` sections) is non-empty.
            let used = used_fields(&t.qfmt);
            used.iter().any(|f| fields.iter().any(|(n, v)| n == f && !v.trim().is_empty()))
        })
        .map(|(i, _)| i)
        .collect()
}

/// Field names a template reads in plain (non-negated) positions.
fn used_fields(tpl: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = tpl;
    while let Some(i) = rest.find("{{") {
        let Some(j) = rest[i..].find("}}") else { break };
        let tag = rest[i + 2..i + j].trim();
        rest = &rest[i + j + 2..];
        if tag.starts_with('^') || tag.starts_with('/') {
            continue;
        }
        let name = tag.trim_start_matches('#').rsplit(':').next().unwrap_or("").trim();
        if !name.is_empty() && !matches!(name, "FrontSide" | "Tags" | "Deck" | "Subdeck" | "Card" | "Type") {
            out.push(name.to_string());
        }
    }
    out
}

/// Render any card for review.
pub fn render_card(store: &Store, card_id: &str, seg: &dyn Segmenter) -> Result<Rendered> {
    let conn = store.read_conn();
    let (note_id, concept_id, deck_id, ord): (Option<String>, Option<String>, String, i64) = conn
        .query_row("SELECT note_id, concept_id, deck_id, ord FROM cards WHERE id = ?1 AND deleted = 0", [card_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .map_err(|_| Error::Invalid(format!("no card {card_id}")))?;
    let deck: String = conn.query_row("SELECT name FROM decks WHERE id = ?1", [&deck_id], |r| r.get(0)).unwrap_or_default();
    if let (None, Some(_)) = (&note_id, &concept_id) {
        return Ok(render_concept(&concept::concept_card(store, card_id, seg)?, &deck));
    }
    let note_id = note_id.ok_or_else(|| Error::Invalid(format!("card {card_id} has no note or concept")))?;
    let (nt, fields_json, tags): (String, String, String) =
        conn.query_row("SELECT note_type_id, fields, tags FROM notes WHERE id = ?1", [&note_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    let (kind, names, templates, css, nt_name): (String, String, String, String, String) = conn.query_row(
        "SELECT kind, fields, templates, css, name FROM note_types WHERE id = ?1",
        [&nt],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )?;
    let names: Vec<String> = serde_json::from_str(&names).unwrap_or_default();
    let values: Vec<String> = serde_json::from_str(&fields_json).unwrap_or_default();
    let fields: Vec<(String, String)> = names.into_iter().enumerate().map(|(i, n)| (n, values.get(i).cloned().unwrap_or_default())).collect();
    let templates: Vec<TemplateDef> = serde_json::from_str(&templates).unwrap_or_default();
    let tags: Vec<String> = serde_json::from_str(&tags).unwrap_or_default();
    let tags = tags.join(" ");
    // Cloze types have one template used for every cloze card.
    let tpl = if kind == "cloze" { templates.first() } else { templates.get(ord as usize) }.ok_or_else(|| Error::Invalid(format!("note type {nt_name} has no template {ord}")))?;
    let o = ord as usize;
    let front = template::render(&tpl.qfmt, &Ctx { fields: &fields, tags: &tags, deck: &deck, card_name: &tpl.name, note_type: &nt_name, ord: o, front: None });
    let back = template::render(&tpl.afmt, &Ctx { fields: &fields, tags: &tags, deck: &deck, card_name: &tpl.name, note_type: &nt_name, ord: o, front: Some(&front) });
    Ok(Rendered { front, back, css: css + if kind == "cloze" { ".cloze{font-weight:bold;color:#1f6feb}" } else { "" }, kind: "classic".into(), deck, note_id: Some(note_id), example: None })
}
