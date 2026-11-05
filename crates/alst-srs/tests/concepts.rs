mod common;
use alst_core::{NewWork, TermPatch};
use alst_srs::concept::{self, Mined};
use alst_srs::fsrs::Rating;
use alst_srs::pool::{self, SimpleSegmenter, split_sentences, utf16_slice};
use alst_srs::sched;
use common::*;
use serde_json::json;

fn term(s: &mut alst_core::Store, text: &str, status: i64, lemma: Option<&str>) -> String {
    let p = TermPatch { status: Some(status), lemma: lemma.map(String::from), translation: Some(format!("{text}-tr")), ..Default::default() };
    s.save_term("es", text, text, 1, &p).unwrap().id
}

fn set(s: &mut alst_core::Store, k: &str, v: &str) {
    s.set_setting(k, &json!(v)).unwrap();
}

#[test]
fn sentences_split_with_utf16_offsets() {
    let text = "Hola, amigo. ¿Cómo estás? Muy bien!\nUna línea 3.5 más… Fin";
    let got: Vec<String> = split_sentences(text).into_iter().map(|(a, b)| utf16_slice(text, a, b)).collect();
    assert_eq!(got, vec!["Hola, amigo.", "¿Cómo estás?", "Muy bien!", "Una línea 3.5 más…", "Fin"]);
    let emoji = "😀 uno. dos tres.";
    let s = split_sentences(emoji);
    assert_eq!(utf16_slice(emoji, s[1].0, s[1].1), "dos tres.");
}

fn library(s: &mut alst_core::Store) {
    // Work A: 5 sentences with "gato"; work B: 1. One too short, one too long.
    let a = "El gato come pescado fresco hoy. El gato duerme en la casa grande. Mi gato juega con una pelota roja. \
             Vi un gato negro en la calle. Un gato viejo vive aquí hace años. Gato malo.";
    let long = format!("El gato {}.", vec!["muy"; 40].join(" "));
    s.create_work(NewWork::text("es", "A", &format!("{a} {long}"))).unwrap();
    s.create_work(NewWork::text("es", "B", "Ese gato de mi vecino es muy simpático.")).unwrap();
}

#[test]
fn the_pool_keeps_comprehensible_sentences_with_limits() {
    let mut s = alst_srs::open_in_memory().unwrap();
    library(&mut s);
    for w in ["el", "come", "en", "la", "casa", "grande", "duerme"] {
        term(&mut s, w, 4, None);
    }
    let gato = term(&mut s, "gato", 1, None);
    let cid = concept::concept_for_term(&mut s, &gato).unwrap();
    let n = pool::refresh(&mut s, &[cid.clone()], &SimpleSegmenter).unwrap();
    assert_eq!(n, 4, "3 from work A (not the 2-word or 42-word ones) and 1 from B");
    let rows: Vec<(f64, String)> = {
        let mut stmt = s.read_conn().prepare("SELECT p.score, pt.text FROM local_example_pool p JOIN parts pt ON pt.id = p.part_id WHERE concept_id = ?1 ORDER BY score DESC").unwrap();
        stmt.query_map([&cid], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap()
    };
    assert_eq!(rows.len(), 4);
    let ex = pool::pick(&s, &cid, &SimpleSegmenter).unwrap().unwrap();
    assert!(ex.text.starts_with("El gato duerme en la casa grande"), "the sentence made of known words ranks first: {}", ex.text);
    assert_eq!(ex.target.map(|(a, b)| utf16_slice(&ex.text, a, b)), Some("gato".to_string()));
}

#[test]
fn rotation_shows_the_least_recently_shown_and_a_mined_sentence_goes_first() {
    let mut s = alst_srs::open_in_memory().unwrap();
    library(&mut s);
    let gato = term(&mut s, "gato", 1, None);
    let cid = concept::concept_for_term(&mut s, &gato).unwrap();
    pool::refresh(&mut s, &[cid.clone()], &SimpleSegmenter).unwrap();
    concept::add_term(&mut s, &gato, false).unwrap();
    let card = alst_core::srs::concept_card_id(&cid, "recognition");
    let mut seen = vec![];
    let mut now = T0;
    for _ in 0..8 {
        let ex = pool::pick(&s, &cid, &SimpleSegmenter).unwrap().unwrap();
        seen.push((ex.part_id.clone(), ex.start));
        concept::answer_card(&mut s, &card, Rating::Good, 900, Some((&ex.part_id, ex.start as i64)), now, 0).unwrap();
        now += 2 * DAY;
    }
    let distinct: std::collections::HashSet<_> = seen[..4].iter().collect();
    assert_eq!(distinct.len(), 4, "four reviews show four different sentences: {seen:?}");
    assert_eq!(&seen[..4], &seen[4..], "then the rotation starts over in the same order");
    // The rotation holds without the local column: it is rebuilt from review events (another device's view).
    s.local_conn().execute("UPDATE local_example_pool SET last_shown = 0", []).unwrap();
    let ex = pool::pick(&s, &cid, &SimpleSegmenter).unwrap().unwrap();
    assert_eq!((ex.part_id.clone(), ex.start), seen[0]);
    // A mined sentence joins the pool and is preferred.
    let mined = Mined {
        term_id: gato.clone(),
        sentence: "Mi <b>gato</b> se llama Pepe.".into(),
        target: "gato".into(),
        meaning: "cat".into(),
        source: json!({"workId": "w", "partId": "no-such-part", "start": 3, "end": 9}),
        ..Default::default()
    };
    let (note, cards) = concept::mine_sentence(&mut s, &mined).unwrap();
    assert!(cards.is_empty(), "the concept deck already has the concept's card");
    let (note2, _) = concept::mine_sentence(&mut s, &mined).unwrap();
    assert_eq!(note, note2, "the same mined sentence is one note");
    let ex = pool::pick(&s, &cid, &SimpleSegmenter).unwrap().unwrap();
    assert!(ex.mined && ex.text.contains("Pepe"));
    assert_eq!(ex.text, "Mi gato se llama Pepe.", "the stored markup is not part of the example text");
}

#[test]
fn a_concept_with_no_sentence_shows_the_word_alone() {
    let mut s = alst_srs::open_in_memory().unwrap();
    let id = term(&mut s, "perro", 1, None);
    let card = concept::add_term(&mut s, &id, false).unwrap().remove(0);
    let c = concept::concept_card(&s, &card, &SimpleSegmenter).unwrap();
    assert_eq!((c.term.as_str(), c.direction.as_str(), c.meaning.as_deref()), ("perro", "recognition", Some("perro-tr")));
    assert!(c.example.is_none());
}

#[test]
fn forms_of_a_word_share_one_concept_and_one_set_of_cards() {
    let mut s = alst_srs::open_in_memory().unwrap();
    let a = term(&mut s, "gato", 1, Some("gato"));
    let b = term(&mut s, "gatos", 2, Some("gato"));
    let ca = concept::concept_for_term(&mut s, &a).unwrap();
    let cb = concept::concept_for_term(&mut s, &b).unwrap();
    assert_eq!(ca, cb);
    assert_eq!(s.get_term(&b).unwrap().id, b);
    let linked: i64 = s.read_conn().query_row("SELECT count(*) FROM terms WHERE concept_id = ?1", [&ca], |r| r.get(0)).unwrap();
    assert_eq!(linked, 2);
    assert_eq!(concept::add_term(&mut s, &a, false).unwrap().len(), 1);
    assert!(concept::add_term(&mut s, &b, false).unwrap().is_empty(), "already has cards");
}

#[test]
fn card_creation_opt_in_by_default_and_automatic_on_request() {
    let mut s = alst_srs::open_in_memory().unwrap();
    let id = term(&mut s, "casa", 0, None);
    // Opt-in: a status change creates nothing.
    s.save_term("es", "casa", "casa", 1, &TermPatch { status: Some(2), ..Default::default() }).unwrap();
    concept::handle_status_change(&mut s, &id, 2).unwrap();
    assert!(s.read_conn().query_row::<i64, _, _>("SELECT count(*) FROM cards", [], |r| r.get(0)).unwrap() == 0);
    // Automatic: the same change creates the card; a status outside 1-5 does not.
    set(&mut s, "srs.cardCreation", "automatic");
    concept::handle_status_change(&mut s, &id, 2).unwrap();
    assert_eq!(s.read_conn().query_row::<i64, _, _>("SELECT count(*) FROM cards", [], |r| r.get(0)).unwrap(), 1);
    let ign = term(&mut s, "xyz", 98, None);
    concept::handle_status_change(&mut s, &ign, 98).unwrap();
    assert_eq!(s.read_conn().query_row::<i64, _, _>("SELECT count(*) FROM cards", [], |r| r.get(0)).unwrap(), 1);
    // The new-cards-per-day limit still applies at review time.
    let plan = sched::plan(s.read_conn(), T0, 0).unwrap();
    assert_eq!(plan.items.len(), 1);
}

#[test]
fn add_all_learning_counts_first_and_adds_once() {
    let mut s = alst_srs::open_in_memory().unwrap();
    for (w, st) in [("uno", 1), ("dos", 3), ("tres", 99), ("cuatro", 0)] {
        term(&mut s, w, st, None);
    }
    assert_eq!(concept::learning_terms_without_cards(&s, "es").unwrap().len(), 2);
    assert_eq!(concept::add_all_learning(&mut s, "es").unwrap(), 2);
    assert_eq!(concept::add_all_learning(&mut s, "es").unwrap(), 0);
}

#[test]
fn production_cards_and_classic_deck_kind_are_settings() {
    let mut s = alst_srs::open_in_memory().unwrap();
    s.set_setting("srs.productionCards", &json!(true)).unwrap();
    let a = term(&mut s, "uno", 1, None);
    assert_eq!(concept::add_term(&mut s, &a, false).unwrap().len(), 2);
    set(&mut s, "srs.defaultDeckKind", "classic");
    let b = term(&mut s, "dos", 1, None);
    let cards = concept::add_term(&mut s, &b, false).unwrap();
    assert_eq!(cards.len(), 1);
    let kind: String = s.read_conn().query_row("SELECT d.kind FROM cards c JOIN decks d ON d.id = c.deck_id WHERE c.id = ?1", [&cards[0]], |r| r.get(0)).unwrap();
    assert_eq!(kind, "classic");
}

fn card_for(s: &mut alst_core::Store, word: &str, status: i64) -> (String, String) {
    let id = term(s, word, status, None);
    let card = concept::add_term(s, &id, false).unwrap().remove(0);
    (id, card)
}

#[test]
fn reviews_move_the_status_in_both_modes_but_never_to_98_or_99() {
    for mode in ["linked", "reviews-only"] {
        let mut s = alst_srs::open_in_memory().unwrap();
        set(&mut s, "srs.statusLink", mode);
        let (id, card) = card_for(&mut s, "uno", 1);
        let mut now = T0;
        for _ in 0..8 {
            concept::answer_card(&mut s, &card, Rating::Easy, 800, None, now, 0).unwrap();
            now += 40 * DAY;
        }
        let st = s.get_term(&id).unwrap().status;
        assert_eq!(st, 5, "{mode}");
        concept::answer_card(&mut s, &card, Rating::Again, 800, None, now, 0).unwrap();
        assert!(s.get_term(&id).unwrap().status < 5, "{mode}: a lapse lowers stability and status");
        assert!(concept::status_for_stability(1e9) == 5 && concept::status_for_stability(0.5) == 1);
    }
    assert_eq!(
        [0.5, 1.0, 2.9, 3.0, 6.9, 7.0, 20.9, 21.0].map(concept::status_for_stability),
        [1, 2, 2, 3, 3, 4, 4, 5]
    );
}

#[test]
fn linked_mode_suspends_on_known_or_ignored_and_restores_but_reviews_only_does_not() {
    for (mode, linked) in [("linked", true), ("reviews-only", false)] {
        let mut s = alst_srs::open_in_memory().unwrap();
        set(&mut s, "srs.statusLink", mode);
        let (id, card) = card_for(&mut s, "uno", 2);
        let susp = |s: &alst_core::Store| -> i64 { s.read_conn().query_row("SELECT suspended FROM cards WHERE id = ?1", [&card], |r| r.get(0)).unwrap() };
        s.save_term("es", "uno", "uno", 1, &TermPatch { status: Some(99), ..Default::default() }).unwrap();
        concept::handle_status_change(&mut s, &id, 99).unwrap();
        assert_eq!(susp(&s), if linked { 2 } else { 0 }, "{mode}");
        s.save_term("es", "uno", "uno", 1, &TermPatch { status: Some(3), ..Default::default() }).unwrap();
        concept::handle_status_change(&mut s, &id, 3).unwrap();
        assert_eq!(susp(&s), 0, "{mode}");
        // A card the user suspended by hand stays suspended when the word moves to 1-5.
        s.srs_update("cards", &card, &[("suspended", i(1))]).unwrap();
        concept::handle_status_change(&mut s, &id, 4).unwrap();
        assert_eq!(susp(&s), 1, "{mode}");
    }
}

#[test]
fn reading_a_part_emits_an_exposure_event_for_known_concepts() {
    let mut s = alst_srs::open_in_memory().unwrap();
    let w = s.create_work(NewWork::text("es", "T", "El gato come. El perro duerme.")).unwrap();
    let part = s.list_parts(&w.id).unwrap()[0].id.clone();
    let (_, _) = card_for(&mut s, "gato", 1);
    term(&mut s, "luna", 1, None); // no concept: not in the event
    assert_eq!(concept::emit_exposure(&mut s, &part, &SimpleSegmenter).unwrap(), 1);
    let ev = s.events_query(Some("exposure"), None, 5).unwrap();
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].payload["concept_ids"].as_array().unwrap().len(), 1);
    assert_eq!(ev[0].payload["part_id"], part);
}
