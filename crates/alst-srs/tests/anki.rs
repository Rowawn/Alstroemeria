mod common;
use alst_core::{NewWork, Store, TermPatch};
use alst_core::media::MediaDir;
use alst_srs::anki::{self, ImportOptions, NEW_LAYOUT_MESSAGE};
use alst_srs::concept;
use alst_srs::fsrs::{Queue, Rating};
use alst_srs::pool::{self, SimpleSegmenter};
use alst_srs::{render, replay};
use common::*;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name)
}

struct Env {
    dir: PathBuf,
    media: MediaDir,
}

impl Env {
    fn new() -> Env {
        let dir = std::env::temp_dir().join(format!("alst-anki-{}", alst_core::store::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        Env { media: MediaDir::new(dir.join("media")), dir }
    }
    fn opts(&self) -> ImportOptions<'_> {
        ImportOptions { media: &self.media, tmp_dir: &self.dir, now_ms: 1_710_000_000_000, tz_offset_min: 0, language: Some("es".into()) }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn count(s: &Store, sql: &str) -> i64 {
    s.read_conn().query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn basic_deck_notes_cards_tags_decks_and_media() {
    let env = Env::new();
    let mut s = alst_srs::open_in_memory().unwrap();
    let rep = anki::import_apkg(&mut s, &fixture("basic.apkg"), &env.opts()).unwrap();
    assert_eq!((rep.notes_new, rep.cards, rep.media), (3, 5, 2), "{rep:?}");
    let decks: Vec<String> = alst_srs::sched::decks(s.read_conn()).unwrap().into_iter().map(|d| d.name).collect();
    assert_eq!(decks, vec!["Spanish::Verbs"], "the empty Default deck is not imported");
    let tags: String = s.read_conn().query_row("SELECT tags FROM notes WHERE guid = 'guid-hablar'", [], |r| r.get(0)).unwrap();
    assert_eq!(tags, r#"["verb","regular"]"#);
    assert_eq!(count(&s, "SELECT count(*) FROM cards c JOIN notes n ON n.id = c.note_id WHERE n.guid = 'guid-solo'"), 1, "an empty Back gives no reverse card");
    // Media: files are in the store and the names resolve.
    assert_eq!(count(&s, "SELECT count(*) FROM note_media"), 2);
    let hash: String = s.read_conn().query_row("SELECT media_hash FROM note_media WHERE name = 'cat.png'", [], |r| r.get(0)).unwrap();
    assert!(env.media.exists(&hash));
    // Cards render from the imported note type, with the sound as a play button and the image kept.
    let card: String = s.read_conn().query_row("SELECT c.id FROM cards c JOIN notes n ON n.id = c.note_id WHERE n.guid = 'guid-gato' AND c.ord = 1", [], |r| r.get(0)).unwrap();
    let r = render::render_card(&s, &card, &SimpleSegmenter).unwrap();
    assert!(r.front.contains("cat") && r.front.contains("data-sound=\"hola.mp3\""), "{}", r.front);
    assert!(r.back.contains("<img src=\"cat.png\">"), "{}", r.back);
    // New cards come up for review under the deck's options.
    assert!(!alst_srs::sched::plan(s.read_conn(), T0, 0).unwrap().items.is_empty());
}

#[test]
fn cloze_notes_make_one_card_per_number_and_render() {
    let env = Env::new();
    let mut s = alst_srs::open_in_memory().unwrap();
    let rep = anki::import_apkg(&mut s, &fixture("cloze.apkg"), &env.opts()).unwrap();
    assert_eq!((rep.notes_new, rep.cards), (2, 3));
    let kind: String = s.read_conn().query_row("SELECT kind FROM note_types", [], |r| r.get(0)).unwrap();
    assert_eq!(kind, "cloze");
    let c2: String = s.read_conn().query_row("SELECT c.id FROM cards c JOIN notes n ON n.id = c.note_id WHERE n.guid = 'guid-capital' AND c.ord = 1", [], |r| r.get(0)).unwrap();
    let r = render::render_card(&s, &c2, &SimpleSegmenter).unwrap();
    assert!(r.front.starts_with("Madrid is the capital of ") && r.front.contains("[...]"), "{}", r.front);
    assert!(r.back.contains("<span class=\"cloze\">Spain</span>"));
}

#[test]
fn review_history_fsrs_state_and_suspension_come_across() {
    let env = Env::new();
    let mut s = alst_srs::open_in_memory().unwrap();
    let rep = anki::import_apkg(&mut s, &fixture("history.apkg"), &env.opts()).unwrap();
    assert_eq!((rep.cards, rep.reviews), (4, 6), "{rep:?}");
    assert_eq!(count(&s, "SELECT count(*) FROM events WHERE type = 'review'"), 6);
    let card = |guid: &str| -> String { s.read_conn().query_row("SELECT c.id FROM cards c JOIN notes n ON n.id = c.note_id WHERE n.guid = ?1", [guid], |r| r.get(0)).unwrap() };
    // Card with history only: state comes from replaying the five reviews; the due date is Anki's.
    let st = replay::card_state(&s, &card("guid-h0")).unwrap().unwrap();
    assert_eq!(st.queue, Queue::Review);
    assert_eq!((st.reps, st.lapses), (5, 1));
    assert!(st.stability.unwrap() > 1.0);
    let want = (1_700_000_000 + 10 * 86400) * 1000; // the fixture's due date: ten days after its reference time
    assert!((st.due - want).abs() < 86_400_000, "due {} vs {}", st.due, want);
    // Card with FSRS memory state in `data`: taken as is.
    let st = replay::card_state(&s, &card("guid-h1")).unwrap().unwrap();
    assert_eq!((st.stability, st.difficulty, st.reps), (Some(14.5), Some(4.2), 7));
    // Suspended and new.
    let susp: i64 = s.read_conn().query_row("SELECT suspended FROM cards WHERE id = ?1", [card("guid-h2")], |r| r.get(0)).unwrap();
    assert_eq!(susp, 1);
    assert_eq!(replay::card_state(&s, &card("guid-h3")).unwrap().unwrap().queue, Queue::New);
    // A review in the app continues from the imported state.
    let id = card("guid-h1");
    replay::answer(&mut s, &id, Rating::Good, 1000, None, st.due, 0).unwrap();
    let after = replay::card_state(&s, &id).unwrap().unwrap();
    assert_eq!(after.reps, 8);
    assert!(after.stability.unwrap() > 14.5);
}

#[test]
fn importing_again_updates_notes_by_guid_and_keeps_history() {
    let env = Env::new();
    let mut s = alst_srs::open_in_memory().unwrap();
    anki::import_apkg(&mut s, &fixture("history.apkg"), &env.opts()).unwrap();
    let id: String = s.read_conn().query_row("SELECT c.id FROM cards c JOIN notes n ON n.id = c.note_id WHERE n.guid = 'guid-h1'", [], |r| r.get(0)).unwrap();
    let st = replay::card_state(&s, &id).unwrap().unwrap();
    replay::answer(&mut s, &id, Rating::Again, 1000, None, st.due + 3_600_000, 0).unwrap();
    let before = (count(&s, "SELECT count(*) FROM events"), count(&s, "SELECT count(*) FROM notes"), count(&s, "SELECT count(*) FROM cards"));
    let state_before = replay::card_state(&s, &id).unwrap().unwrap();
    // Edit a field locally; the re-import overwrites it from the file (same guid) but adds nothing twice.
    let note: String = s.read_conn().query_row("SELECT id FROM notes WHERE guid = 'guid-h1'", [], |r| r.get(0)).unwrap();
    s.srs_update("notes", &note, &[("fields", t(r#"["edited","edited"]"#))]).unwrap();
    let rep = anki::import_apkg(&mut s, &fixture("history.apkg"), &env.opts()).unwrap();
    assert_eq!((rep.notes_new, rep.notes_updated), (0, 4));
    assert_eq!((count(&s, "SELECT count(*) FROM events"), count(&s, "SELECT count(*) FROM notes"), count(&s, "SELECT count(*) FROM cards")), before);
    let f: String = s.read_conn().query_row("SELECT fields FROM notes WHERE id = ?1", [&note], |r| r.get(0)).unwrap();
    assert_eq!(f, r#"["dos","two"]"#);
    assert_eq!(replay::card_state(&s, &id).unwrap().unwrap(), state_before, "the in-app review after import is kept");
}

#[test]
fn a_package_with_only_the_new_layout_is_refused_with_advice() {
    let env = Env::new();
    let mut s = alst_srs::open_in_memory().unwrap();
    let err = anki::import_apkg(&mut s, &fixture("new-layout.apkg"), &env.opts()).unwrap_err().to_string();
    assert_eq!(err, NEW_LAYOUT_MESSAGE);
    assert_eq!(err, "Export again from Anki with 'Support older Anki versions' ticked.");
    assert_eq!(count(&s, "SELECT count(*) FROM decks"), 0);
}

#[test]
fn not_a_package_is_an_error_not_a_crash() {
    let env = Env::new();
    let mut s = alst_srs::open_in_memory().unwrap();
    let p = env.dir.join("junk.apkg");
    std::fs::write(&p, b"definitely not a zip").unwrap();
    assert!(anki::import_apkg(&mut s, &p, &env.opts()).is_err());
}

#[test]
fn export_then_import_round_trips_notes_media_and_history() {
    let env = Env::new();
    let mut a = alst_srs::open_in_memory().unwrap();
    anki::import_apkg(&mut a, &fixture("basic.apkg"), &env.opts()).unwrap();
    anki::import_apkg(&mut a, &fixture("history.apkg"), &env.opts()).unwrap();
    // Review a card in the app so there is history of our own.
    let card: String = a.read_conn().query_row("SELECT c.id FROM cards c JOIN notes n ON n.id = c.note_id WHERE n.guid = 'guid-hablar' AND c.ord = 0", [], |r| r.get(0)).unwrap();
    replay::answer(&mut a, &card, Rating::Good, 2000, None, T0, 0).unwrap();
    replay::answer(&mut a, &card, Rating::Good, 2000, None, T0 + 11 * 60_000, 0).unwrap();
    let decks: Vec<String> = alst_srs::sched::decks(a.read_conn()).unwrap().into_iter().map(|d| d.id).collect();
    let out = env.dir.join("out.apkg");
    let rep = anki::export_apkg(&a, &decks, &out, &env.media, &SimpleSegmenter, 1_710_000_000_000, 0).unwrap();
    assert_eq!((rep.notes, rep.cards, rep.media), (7, 9, 2), "{rep:?}");
    assert_eq!(rep.reviews, 8, "6 imported + 2 in the app");

    let env2 = Env::new();
    let mut b = alst_srs::open_in_memory().unwrap();
    let r2 = anki::import_apkg(&mut b, &out, &env2.opts()).unwrap();
    assert_eq!((r2.notes_new, r2.cards, r2.media, r2.reviews), (7, 9, 2, 8), "{r2:?}");
    for guid in ["guid-hablar", "guid-gato", "guid-solo", "guid-h0", "guid-h1"] {
        let get = |s: &Store| -> (String, String) { s.read_conn().query_row("SELECT fields, tags FROM notes WHERE guid = ?1", [guid], |r| Ok((r.get(0)?, r.get(1)?))).unwrap() };
        assert_eq!(get(&a), get(&b), "{guid}");
    }
    // Schedules survive: the same card has the same state on the other side.
    let find = |s: &Store, guid: &str, ord: i64| -> String { s.read_conn().query_row("SELECT c.id FROM cards c JOIN notes n ON n.id = c.note_id WHERE n.guid = ?1 AND c.ord = ?2", rusqlite::params![guid, ord], |r| r.get(0)).unwrap() };
    for (guid, ord) in [("guid-hablar", 0), ("guid-h0", 0), ("guid-h1", 0), ("guid-h2", 0), ("guid-h3", 0)] {
        let (sa, sb) = (replay::card_state(&a, &find(&a, guid, ord)).unwrap().unwrap(), replay::card_state(&b, &find(&b, guid, ord)).unwrap().unwrap());
        assert_eq!((sa.queue, sa.reps, sa.lapses), (sb.queue, sb.reps, sb.lapses), "{guid}");
        assert!((sa.stability.unwrap_or(0.0) - sb.stability.unwrap_or(0.0)).abs() < 1e-6, "{guid}: {:?} vs {:?}", sa.stability, sb.stability);
        assert_eq!(sa.due / 86_400_000, sb.due / 86_400_000, "{guid} due day");
    }
    let susp: i64 = b.read_conn().query_row("SELECT suspended FROM cards WHERE id = ?1", [find(&b, "guid-h2", 0)], |r| r.get(0)).unwrap();
    assert_eq!(susp, 1);
    assert_eq!(count(&b, "SELECT count(*) FROM note_media"), 2);
    assert_eq!(count(&b, "SELECT count(*) FROM decks WHERE name = 'Spanish::Verbs'"), 1);
}

#[test]
fn concept_decks_export_as_plain_notes_with_the_best_sentence() {
    let env = Env::new();
    let mut a = alst_srs::open_in_memory().unwrap();
    a.set_setting("srs.productionCards", &serde_json::json!(true)).unwrap();
    a.create_work(NewWork::text("es", "Cuento", "Ayer vi un gato negro en la calle.")).unwrap();
    let id = a.save_term("es", "gato", "gato", 1, &TermPatch { status: Some(1), translation: Some("cat".into()), ..Default::default() }).unwrap().id;
    let cid = concept::concept_for_term(&mut a, &id).unwrap();
    pool::refresh(&mut a, &[cid], &SimpleSegmenter).unwrap();
    concept::add_term(&mut a, &id, false).unwrap();
    let decks: Vec<String> = alst_srs::sched::decks(a.read_conn()).unwrap().into_iter().map(|d| d.id).collect();
    let out = env.dir.join("concepts.apkg");
    let rep = anki::export_apkg(&a, &decks, &out, &env.media, &SimpleSegmenter, 1_710_000_000_000, 0).unwrap();
    assert_eq!((rep.concept_cards, rep.notes, rep.cards), (2, 2, 2));
    let env2 = Env::new();
    let mut b = alst_srs::open_in_memory().unwrap();
    anki::import_apkg(&mut b, &out, &env2.opts()).unwrap();
    let fields: Vec<String> = {
        let mut stmt = b.read_conn().prepare("SELECT fields FROM notes ORDER BY id").unwrap();
        stmt.query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
    };
    assert!(fields.iter().any(|f| f.contains("vi un <b>gato</b> negro")), "{fields:?}");
    assert!(fields.iter().any(|f| f.contains("cat")));
}
