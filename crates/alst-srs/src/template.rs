//! Anki-style card templates (SPEC §11.8), written from the public template documentation: fields, conditional
//! sections, `{{FrontSide}}`, the special fields, cloze deletions, and the text/hint/furigana filters.
//! Output is HTML; it is rendered only inside the module's sandboxed frame.

use std::collections::{BTreeSet, HashSet};
use std::sync::Mutex;

/// What a template can see of one card.
pub struct Ctx<'a> {
    /// `(name, value)` in note-type order.
    pub fields: &'a [(String, String)],
    pub tags: &'a str,
    pub deck: &'a str,
    pub card_name: &'a str,
    pub note_type: &'a str,
    /// 0-based card ordinal; for cloze notes the cloze number is `ord + 1`.
    pub ord: usize,
    /// The rendered question, for `{{FrontSide}}` on the answer side.
    pub front: Option<&'a str>,
}

#[derive(Debug)]
enum Node {
    Text(String),
    Field(Vec<String>, String), // filters (outermost first), field name
    Section { inverse: bool, name: String, body: Vec<Node> },
}

static LOGGED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn log_once(msg: String) {
    let mut g = LOGGED.lock().unwrap();
    if g.get_or_insert_with(HashSet::new).insert(msg.clone()) {
        eprintln!("alst-srs template: {msg}");
    }
}

fn parse(tpl: &str) -> Vec<Node> {
    let mut stack: Vec<(Option<(bool, String)>, Vec<Node>)> = vec![(None, vec![])];
    let mut rest = tpl;
    while let Some(i) = rest.find("{{") {
        let (text, after) = rest.split_at(i);
        if !text.is_empty() {
            stack.last_mut().unwrap().1.push(Node::Text(text.to_string()));
        }
        let Some(j) = after.find("}}") else {
            stack.last_mut().unwrap().1.push(Node::Text(after.to_string()));
            rest = "";
            break;
        };
        let tag = after[2..j].trim();
        rest = &after[j + 2..];
        if let Some(name) = tag.strip_prefix('#').or_else(|| tag.strip_prefix('^')) {
            stack.push((Some((tag.starts_with('^'), name.trim().to_string())), vec![]));
        } else if let Some(name) = tag.strip_prefix('/') {
            if stack.len() > 1 && stack.last().and_then(|s| s.0.as_ref()).is_some_and(|(_, n)| n == name.trim()) {
                let (head, body) = stack.pop().unwrap();
                let (inverse, name) = head.unwrap();
                stack.last_mut().unwrap().1.push(Node::Section { inverse, name, body });
            } else {
                log_once(format!("unmatched {{{{/{name}}}}}"));
            }
        } else {
            let mut parts: Vec<&str> = tag.split(':').map(str::trim).collect();
            let field = parts.pop().unwrap_or("").to_string();
            stack.last_mut().unwrap().1.push(Node::Field(parts.into_iter().map(String::from).collect(), field));
        }
    }
    if !rest.is_empty() {
        stack.last_mut().unwrap().1.push(Node::Text(rest.to_string()));
    }
    // Unclosed sections: render their body as if closed.
    while stack.len() > 1 {
        let (head, body) = stack.pop().unwrap();
        let (inverse, name) = head.unwrap();
        log_once(format!("section {name} is not closed"));
        stack.last_mut().unwrap().1.push(Node::Section { inverse, name, body });
    }
    stack.pop().unwrap().1
}

fn lookup(ctx: &Ctx, name: &str) -> Option<String> {
    if let Some((_, v)) = ctx.fields.iter().find(|(n, _)| n == name) {
        return Some(v.clone());
    }
    Some(match name {
        "Tags" => ctx.tags.to_string(),
        "Deck" => ctx.deck.to_string(),
        "Subdeck" => ctx.deck.rsplit("::").next().unwrap_or("").to_string(),
        "Card" => ctx.card_name.to_string(),
        "Type" => ctx.note_type.to_string(),
        "FrontSide" => ctx.front.unwrap_or("").to_string(),
        _ => return None,
    })
}

/// Anki treats a field with only whitespace or line breaks as empty for conditionals.
fn is_empty(v: &str) -> bool {
    let mut s = v.to_string();
    for br in ["<br>", "<br/>", "<br />", "&nbsp;", "<div></div>"] {
        s = s.replace(br, "");
    }
    s.trim().is_empty()
}

fn render_nodes(nodes: &[Node], ctx: &Ctx, out: &mut String) {
    for n in nodes {
        match n {
            Node::Text(t) => out.push_str(t),
            Node::Section { inverse, name, body } => {
                let has = lookup(ctx, name).is_some_and(|v| !is_empty(&v));
                if has != *inverse {
                    render_nodes(body, ctx, out);
                }
            }
            Node::Field(filters, name) => {
                let Some(mut v) = lookup(ctx, name) else {
                    out.push_str(&format!("<span class=\"alst-error\">[unknown field {}]</span>", escape(name)));
                    continue;
                };
                for f in filters.iter().rev() {
                    v = apply_filter(f, &v, ctx, name);
                }
                out.push_str(&sounds(&v));
            }
        }
    }
}

fn apply_filter(filter: &str, v: &str, ctx: &Ctx, field: &str) -> String {
    match filter {
        "text" => strip_html(v),
        "cloze" => cloze(v, ctx.ord as u32 + 1, ctx.front.is_none()),
        "hint" => hint(v, field),
        "furigana" => ruby(v, Ruby::Furigana),
        "kana" => ruby(v, Ruby::Kana),
        "kanji" => ruby(v, Ruby::Kanji),
        other => {
            log_once(format!("unknown filter '{other}' passes the field through"));
            v.to_string()
        }
    }
}

/// Render a template. `front` is `None` for the question side and the rendered question for the answer side.
pub fn render(tpl: &str, ctx: &Ctx) -> String {
    let mut out = String::new();
    render_nodes(&parse(tpl), ctx, &mut out);
    out
}

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Plain text of an HTML fragment (`{{text:}}`).
pub fn strip_html(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&amp;", "&")
}

/// `[sound:file]` becomes a play button the frame wires up (`data-sound` is the file name or an `alstclip:` reference).
fn sounds(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("[sound:") {
        let Some(j) = rest[i..].find(']') else { break };
        out.push_str(&rest[..i]);
        let file = &rest[i + 7..i + j];
        out.push_str(&format!("<a class=\"alst-sound\" href=\"#\" data-sound=\"{}\">&#9654;</a>", escape(file)));
        rest = &rest[i + j + 1..];
    }
    out.push_str(rest);
    out
}

fn hint(v: &str, field: &str) -> String {
    if is_empty(v) {
        return String::new();
    }
    let id = format!("hint-{}", field.chars().filter(|c| c.is_alphanumeric()).collect::<String>());
    format!(
        "<a class=\"hint\" href=\"#\" onclick=\"this.style.display='none';document.getElementById('{id}').style.display='block';return false;\">Show {}</a><div id=\"{id}\" class=\"hint\" style=\"display:none\">{v}</div>",
        escape(field)
    )
}

#[derive(Clone, Copy)]
enum Ruby {
    Furigana,
    Kana,
    Kanji,
}

/// Anki's `base[reading]` notation: the base is the run of non-space characters before `[`; a space ends it.
fn ruby(v: &str, mode: Ruby) -> String {
    let chars: Vec<char> = v.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut tag = false;
    let mut run_start = 0usize; // start of the current base candidate in `out`-chars terms (index into chars)
    let mut base_from = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '<' {
            tag = true;
        } else if c == '>' {
            tag = false;
        }
        if c == '[' && !tag && i > base_from
            && let Some(close) = chars[i + 1..].iter().position(|x| *x == ']') {
                let reading: String = chars[i + 1..i + 1 + close].iter().collect();
                if !reading.starts_with("sound:") {
                    let base: String = chars[base_from..i].iter().collect();
                    // Drop the base text already emitted for this run.
                    let emitted: String = chars[run_start..i].iter().collect();
                    out.truncate(out.len() - emitted.len());
                    out.push_str(&match mode {
                        Ruby::Furigana => format!("<ruby><rb>{base}</rb><rt>{reading}</rt></ruby>"),
                        Ruby::Kana => reading,
                        Ruby::Kanji => base,
                    });
                    i += close + 2;
                    run_start = i;
                    base_from = i;
                    continue;
                }
            }
        out.push(c);
        i += 1;
        if c == ' ' || c == '>' && !tag {
            base_from = i;
            run_start = i;
        }
    }
    out
}

// ---- cloze -------------------------------------------------------------------------------------

struct ClozeSpan {
    num: u32,
    start: usize,
    end: usize,
    body_start: usize,
    body_end: usize,
    hint: Option<(usize, usize)>,
}

fn find_clozes(s: &str) -> Vec<ClozeSpan> {
    let b = s.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i + 4 < b.len() {
        if &b[i..i + 3] == b"{{c" && b[i + 3].is_ascii_digit() {
            let mut j = i + 3;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if s[j..].starts_with("::") {
                let body_start = j + 2;
                // Find the matching "}}" allowing nested "{{ ... }}".
                let mut depth = 1;
                let mut k = body_start;
                let mut first_sep: Option<usize> = None;
                let mut inner_depth = 0;
                while k + 1 < b.len() {
                    if &b[k..k + 2] == b"{{" {
                        depth += 1;
                        inner_depth += 1;
                        k += 2;
                    } else if &b[k..k + 2] == b"}}" {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                        inner_depth -= 1;
                        k += 2;
                    } else if &b[k..k + 2] == b"::" && inner_depth == 0 && first_sep.is_none() {
                        first_sep = Some(k);
                        k += 2;
                    } else {
                        k += 1;
                    }
                }
                if depth == 0 {
                    let num: u32 = s[i + 3..j].parse().unwrap_or(0);
                    let (body_end, hint) = match first_sep {
                        Some(p) => (p, Some((p + 2, k))),
                        None => (k, None),
                    };
                    out.push(ClozeSpan { num, start: i, end: k + 2, body_start, body_end, hint });
                    i = k + 2;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// Cloze numbers present in a field's text, sorted (nested clozes included).
pub fn cloze_numbers(s: &str) -> BTreeSet<u32> {
    let mut set = BTreeSet::new();
    fn walk(s: &str, set: &mut BTreeSet<u32>) {
        for c in find_clozes(s) {
            if c.num > 0 {
                set.insert(c.num);
            }
            walk(&s[c.body_start..c.body_end], set);
        }
    }
    walk(s, &mut set);
    set
}

/// `{{cloze:Field}}` for cloze number `active` on the question or answer side.
pub fn cloze(s: &str, active: u32, question: bool) -> String {
    let mut out = String::new();
    let mut last = 0;
    for c in find_clozes(s) {
        out.push_str(&s[last..c.start]);
        let inner = cloze(&s[c.body_start..c.body_end], active, question);
        if c.num == active {
            if question {
                let h = c.hint.map(|(a, b)| strip_html(&s[a..b])).filter(|h| !h.is_empty()).unwrap_or_else(|| "...".into());
                out.push_str(&format!("<span class=\"cloze\" data-cloze=\"{}\">[{}]</span>", escape(&strip_html(&inner)), escape(&h)));
            } else {
                out.push_str(&format!("<span class=\"cloze\">{inner}</span>"));
            }
        } else {
            out.push_str(&inner);
        }
        last = c.end;
    }
    out.push_str(&s[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(tpl: &str, fields: &[(&str, &str)]) -> String {
        let f: Vec<(String, String)> = fields.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        render(tpl, &Ctx { fields: &f, tags: "t1 t2", deck: "Spanish::Verbs", card_name: "Card 1", note_type: "Basic", ord: 0, front: None })
    }

    #[test]
    fn field_replacement() {
        assert_eq!(r("<b>{{Front}}</b> / {{Back}}", &[("Front", "hola"), ("Back", "hello <i>there</i>")]), "<b>hola</b> / hello <i>there</i>");
        assert!(r("{{Nope}}", &[]).contains("unknown field Nope"));
    }

    #[test]
    fn conditional_sections() {
        let f = [("A", "x"), ("B", ""), ("C", "<br>")];
        assert_eq!(r("{{#A}}has A{{/A}}|{{#B}}has B{{/B}}|{{#C}}has C{{/C}}", &f), "has A||");
        assert_eq!(r("{{^A}}no A{{/A}}|{{^B}}no B{{/B}}|{{^C}}no C{{/C}}", &f), "|no B|no C");
        assert_eq!(r("{{#A}}[{{#B}}b{{/B}}{{^B}}nb{{/B}}]{{/A}}", &f), "[nb]");
    }

    #[test]
    fn front_side_and_special_fields() {
        let f = vec![("Front".to_string(), "Q".to_string())];
        let mk = |front| Ctx { fields: &f, tags: "t1 t2", deck: "Spanish::Verbs", card_name: "Card 1", note_type: "Basic", ord: 0, front };
        let q = render("{{Front}}", &mk(None));
        assert_eq!(render("{{FrontSide}}<hr>A", &mk(Some(&q))), "Q<hr>A");
        assert_eq!(r("{{Tags}}|{{Deck}}|{{Subdeck}}|{{Card}}|{{Type}}", &[]), "t1 t2|Spanish::Verbs|Verbs|Card 1|Basic");
        assert_eq!(r("{{#Tags}}tagged{{/Tags}}", &[]), "tagged");
    }

    #[test]
    fn text_filter_strips_html() {
        assert_eq!(r("{{text:F}}", &[("F", "a <b>b</b>&nbsp;&amp; c")]), "a b & c");
    }

    #[test]
    fn hint_filter_hides_the_field_behind_a_link() {
        let out = r("{{hint:Tip}}", &[("Tip", "think of a cat")]);
        assert!(out.contains("Show Tip") && out.contains("display:none") && out.contains("think of a cat"));
        assert_eq!(r("{{hint:Tip}}", &[("Tip", "")]), "");
    }

    #[test]
    fn furigana_kana_and_kanji_filters() {
        let f = [("J", "日本語[にほんご]を 勉強[べんきょう]する")];
        assert_eq!(r("{{furigana:J}}", &f), "<ruby><rb>日本語</rb><rt>にほんご</rt></ruby>を <ruby><rb>勉強</rb><rt>べんきょう</rt></ruby>する");
        assert_eq!(r("{{kana:J}}", &f), "にほんごを べんきょうする");
        assert_eq!(r("{{kanji:J}}", &f), "日本語を 勉強する");
        assert_eq!(r("{{furigana:J}}", &[("J", "plain")]), "plain");
    }

    #[test]
    fn sound_tags_become_play_buttons() {
        let out = r("{{Audio}}", &[("Audio", "[sound:hola.mp3] x")]);
        assert!(out.contains("data-sound=\"hola.mp3\"") && out.contains("alst-sound") && !out.contains("[sound:"));
    }

    #[test]
    fn cloze_question_and_answer_with_numbers_and_hints() {
        let text = "{{c1::Madrid::city}} is the capital of {{c2::Spain}}, near {{c1::Toledo}}.";
        let q1 = cloze(text, 1, true);
        assert!(q1.contains("[city]") && q1.contains("[...]") && q1.contains("of Spain") && !q1.contains("Madrid</span>"));
        let a1 = cloze(text, 1, false);
        assert!(a1.contains("<span class=\"cloze\">Madrid</span>") && a1.contains("of Spain,"));
        let q2 = cloze(text, 2, true);
        assert!(q2.starts_with("Madrid is") && q2.contains("[...]"));
        assert_eq!(cloze_numbers(text).into_iter().collect::<Vec<_>>(), vec![1, 2]);
    }

    #[test]
    fn cloze_via_template_uses_the_card_ordinal_and_side() {
        let f: Vec<(String, String)> = vec![("Text".into(), "{{c1::a}} and {{c2::b}}".into())];
        let mk = |ord, front| Ctx { fields: &f, tags: "", deck: "D", card_name: "Cloze", note_type: "Cloze", ord, front };
        let q = render("{{cloze:Text}}", &mk(1, None));
        assert!(q.starts_with("a and ") && q.contains("[...]"));
        assert!(render("{{cloze:Text}}", &mk(1, Some(&q))).contains("<span class=\"cloze\">b</span>"));
    }

    #[test]
    fn nested_cloze() {
        let t = "{{c1::outer {{c2::inner}}}}";
        assert_eq!(cloze_numbers(t).len(), 2);
        let q2 = cloze(t, 2, true);
        assert!(q2.starts_with("outer ") && q2.contains("[...]"));
    }

    #[test]
    fn unknown_filters_pass_the_field_through() {
        assert_eq!(r("{{mystery:F}}", &[("F", "keep")]), "keep");
        assert_eq!(r("{{type:F}}", &[("F", "keep")]), "keep");
    }

    #[test]
    fn filters_chain_right_to_left() {
        assert_eq!(r("{{text:hint:F}}", &[("F", "")]), "");
        assert_eq!(r("{{text:kanji:F}}", &[("F", "<b>日本[にほん]</b>")]), "日本");
    }
}
