mod common;
use alst_core::NewWork;
use alst_srs::concept::{self, Mined};
use alst_srs::pool::{self, SimpleSegmenter};
use alst_srs::render::{self, TemplateDef, card_ords};
use common::*;
use serde_json::json;

fn td(name: &str, q: &str) -> TemplateDef {
    TemplateDef { name: name.into(), qfmt: q.into(), afmt: "{{FrontSide}}".into() }
}

#[test]
fn standard_notes_yield_a_card_per_template_with_content_and_cloze_per_number() {
    let f = |a: &str, b: &str| vec![("Front".to_string(), a.to_string()), ("Back".to_string(), b.to_string())];
    let both = [td("F", "{{Front}}"), td("R", "{{Back}}")];
    assert_eq!(card_ords("standard", &f("a", "b"), &both), vec![0, 1]);
    assert_eq!(card_ords("standard", &f("a", ""), &both), vec![0], "the reverse card needs Back");
    assert_eq!(card_ords("standard", &f("a", "b"), &[td("F", "{{#Back}}{{Front}}{{/Back}}")]), vec![0]);
    let cloze = vec![("Text".to_string(), "{{c1::a}} {{c3::b}} {{c3::c}}".to_string())];
    assert_eq!(card_ords("cloze", &cloze, &[td("Cloze", "{{cloze:Text}}")]), vec![0, 2]);
}

#[test]
fn a_classic_card_renders_front_and_back_from_its_note_type() {
    let mut s = alst_srs::open_in_memory().unwrap();
    note_type(&mut s);
    config(&mut s, "c", 20, 200, true);
    deck(&mut s, "d", "Spanish::Basics", "classic", "c");
    s.srs_put("notes", "n", &[("note_type_id", t("nt")), ("deck_id", t("d")), ("fields", t(r#"["hola","<i>hello</i>"]"#)), ("tags", t(r#"["greeting"]"#))]).unwrap();
    s.srs_put("cards", "k", &[("note_id", t("n")), ("deck_id", t("d")), ("ord", i(0))]).unwrap();
    s.srs_update("note_types", "nt", &[("templates", t(r#"[{"name":"Card 1","qfmt":"{{Front}}<br>{{Tags}}","afmt":"{{FrontSide}}<hr>{{Back}} in {{Deck}}"}]"#))]).unwrap();
    let r = render::render_card(&s, "k", &SimpleSegmenter).unwrap();
    assert_eq!(r.front, "hola<br>greeting");
    assert_eq!(r.back, "hola<br>greeting<hr><i>hello</i> in Spanish::Basics");
    assert_eq!(r.kind, "classic");
}

#[test]
fn concept_cards_render_recognition_and_production_with_the_example() {
    let mut s = alst_srs::open_in_memory().unwrap();
    s.set_setting("srs.productionCards", &json!(true)).unwrap();
    s.create_work(NewWork::text("es", "Cuento", "Ayer vi un gato negro en la calle & corrió.")).unwrap();
    let id = s.save_term("es", "gato", "gato", 1, &alst_core::TermPatch { status: Some(1), translation: Some("cat".into()), ..Default::default() }).unwrap().id;
    let cid = concept::concept_for_term(&mut s, &id).unwrap();
    pool::refresh(&mut s, &[cid.clone()], &SimpleSegmenter).unwrap();
    let cards = concept::add_term(&mut s, &id, false).unwrap();
    assert_eq!(cards.len(), 2);
    let rec = render::render_card(&s, &alst_core::srs::concept_card_id(&cid, "recognition"), &SimpleSegmenter).unwrap();
    assert!(rec.front.contains("vi un <b>gato</b> negro"), "{}", rec.front);
    assert!(rec.back.contains("cat") && rec.back.contains("Cuento") && rec.back.contains("Open in reader") && rec.back.contains("&amp;"));
    assert_eq!(rec.example.as_ref().map(|e| e.mined), Some(false));
    let prod = render::render_card(&s, &alst_core::srs::concept_card_id(&cid, "production"), &SimpleSegmenter).unwrap();
    assert!(prod.front.contains("cat") && prod.front.contains("class=\"blank\"") && !prod.front.contains("gato"), "{}", prod.front);
    assert!(prod.back.contains("<div class=\"word\">gato</div>"));
}

#[test]
fn a_concept_card_without_a_sentence_shows_the_word_and_a_mined_sentence_carries_its_clip() {
    let mut s = alst_srs::open_in_memory().unwrap();
    let id = s.save_term("es", "perro", "perro", 1, &alst_core::TermPatch { status: Some(1), translation: Some("dog".into()), ..Default::default() }).unwrap().id;
    let card = concept::add_term(&mut s, &id, false).unwrap().remove(0);
    let r = render::render_card(&s, &card, &SimpleSegmenter).unwrap();
    assert_eq!(r.front, "<div class=\"word\">perro</div>");
    let m = Mined {
        term_id: id.clone(),
        sentence: "El perro ladra.".into(),
        target: "perro".into(),
        meaning: "dog".into(),
        source: json!({"workId": "w", "partId": "p", "start": 0, "end": 15, "cue": {"mediaHash": "ab12", "start": 1000, "end": 2500}}),
        ..Default::default()
    };
    concept::mine_sentence(&mut s, &m).unwrap();
    let r = render::render_card(&s, &card, &SimpleSegmenter).unwrap();
    assert!(r.front.contains("El <b>perro</b> ladra."));
    assert!(r.back.contains("data-sound=\"alstclip:ab12:1000:2500\""), "{}", r.back);
}

#[test]
fn the_sentence_note_type_renders_its_fields() {
    let mut s = alst_srs::open_in_memory().unwrap();
    let id = s.save_term("es", "gato", "gato", 1, &alst_core::TermPatch { status: Some(1), ..Default::default() }).unwrap().id;
    s.set_setting("srs.defaultDeckKind", &json!("classic")).unwrap();
    let m = Mined { term_id: id, sentence: "Mi <b>gato</b> duerme.".into(), target: "gato".into(), meaning: "cat".into(), sentence_meaning: "My cat sleeps.".into(), source: json!({}), ..Default::default() };
    let (_, cards) = concept::mine_sentence(&mut s, &m).unwrap();
    let r = render::render_card(&s, &cards[0], &SimpleSegmenter).unwrap();
    assert!(r.front.contains("Mi <b>gato</b> duerme."));
    assert!(r.back.contains("cat") && r.back.contains("My cat sleeps."));
}
