//! SPEC §9.5: N devices make random writes interleaved with random pairwise sessions (some cut off mid-batch).
//! After a final round of full syncs every device has identical synced tables, `_changes` and `events`.

use alst_core::{NewWork, PackRow, SeedTerm, Store, WorkPack, TermPatch, store::derive_id, store::term_id, store::WorkCol};
use alst_sync::proto::mem;
use alst_sync::session::{Role, SessionConfig, run};
use proptest::prelude::*;
use std::sync::Mutex;

const SYNCED: [&str; 12] = [
    "works", "parts", "concepts", "terms", "term_parents", "media", "extension_state", "ext_data", "settings", "dictionaries",
    "extension_packages", "packs",
];

fn dump(s: &Mutex<Store>) -> String {
    let s = s.lock().unwrap();
    let c = s.local_conn();
    let mut out = String::new();
    for t in SYNCED.iter().chain(["_changes", "events"].iter()) {
        let order = if *t == "_changes" { "tbl, row_id, col" } else { "id" };
        let mut stmt = c.prepare(&format!("SELECT * FROM {t} ORDER BY {order}")).unwrap();
        let n = stmt.column_count();
        let mut rows = stmt.query([]).unwrap();
        while let Some(r) = rows.next().unwrap() {
            out.push_str(t);
            for i in 0..n {
                out.push_str(&format!("|{:?}", r.get_ref(i).unwrap()));
            }
            out.push('\n');
        }
    }
    out
}

#[derive(Debug, Clone)]
enum Op {
    Term { d: usize, lang: usize, word: usize, status: usize, tr: Option<u8> },
    DelTerm { d: usize, lang: usize, word: usize },
    Parent { d: usize, lang: usize, child: usize, parent: usize },
    Work { d: usize, parts: usize },
    Rename { d: usize, n: u8 },
    ExtSet { d: usize, key: usize, val: u8 },
    ExtDel { d: usize, key: usize },
    Setting { d: usize, key: usize, val: u8 },
    Event { d: usize },
    Media { d: usize, h: usize },
    Dict { d: usize, h: usize },
    DelDict { d: usize, h: usize },
    Package { d: usize, ext: usize, v: u8 },
    Session { a: usize, b: usize, cut: Option<u8> },
}

fn op_strategy(n: usize) -> impl Strategy<Value = Op> {
    let d = 0..n;
    prop_oneof![
        4 => (d.clone(), 0..2usize, 0..4usize, 0..6usize, proptest::option::of(0..3u8)).prop_map(|(d, lang, word, status, tr)| Op::Term { d, lang, word, status, tr }),
        2 => (d.clone(), 0..2usize, 0..4usize).prop_map(|(d, lang, word)| Op::DelTerm { d, lang, word }),
        2 => (d.clone(), 0..2usize, 0..4usize, 0..4usize).prop_map(|(d, lang, child, parent)| Op::Parent { d, lang, child, parent }),
        1 => (d.clone(), 1..3usize).prop_map(|(d, parts)| Op::Work { d, parts }),
        1 => (d.clone(), 0..3u8).prop_map(|(d, n)| Op::Rename { d, n }),
        2 => (d.clone(), 0..3usize, 0..3u8).prop_map(|(d, key, val)| Op::ExtSet { d, key, val }),
        1 => (d.clone(), 0..3usize).prop_map(|(d, key)| Op::ExtDel { d, key }),
        1 => (d.clone(), 0..2usize, 0..3u8).prop_map(|(d, key, val)| Op::Setting { d, key, val }),
        1 => d.clone().prop_map(|d| Op::Event { d }),
        1 => (d.clone(), 0..3usize).prop_map(|(d, h)| Op::Media { d, h }),
        1 => (d.clone(), 0..2usize).prop_map(|(d, h)| Op::Dict { d, h }),
        1 => (d.clone(), 0..2usize).prop_map(|(d, h)| Op::DelDict { d, h }),
        1 => (d.clone(), 0..2usize, 0..3u8).prop_map(|(d, ext, v)| Op::Package { d, ext, v }),
        4 => (d.clone(), d, proptest::option::of(0..12u8)).prop_map(|(a, b, cut)| Op::Session { a, b, cut }),
    ]
}

fn hash(i: usize) -> String {
    format!("{:064x}", i + 1)
}

fn write(s: &Mutex<Store>, op: &Op) {
    let mut s = s.lock().unwrap();
    let langs = ["es", "fr"];
    let words = ["uno", "dos", "tres", "cuatro"];
    match op {
        Op::Term { lang, word, status, tr, .. } => {
            let status = [0, 1, 2, 3, 98, 99][*status];
            let patch = TermPatch { status: Some(status), translation: tr.map(|t| format!("tr{t}")), ..Default::default() };
            s.save_term(langs[*lang], words[*word], words[*word], 1, &patch).unwrap();
        }
        Op::DelTerm { lang, word, .. } => {
            s.delete_term(langs[*lang], words[*word]).unwrap();
        }
        Op::Parent { lang, child, parent, .. } => {
            let (c, p) = (s.find_term(langs[*lang], words[*child]).unwrap(), s.find_term(langs[*lang], words[*parent]).unwrap());
            if let (Some(c), Some(p)) = (c, p) {
                if c.id != p.id {
                    s.set_term_parents(&c.id, &[p.id]).unwrap();
                }
            }
        }
        Op::Work { parts, .. } => {
            let mut w = NewWork::text("es", "Work", "texto uno");
            w.parts = (0..*parts).map(|i| NewWork::text("es", "x", &format!("parte {i}")).parts.remove(0)).collect();
            s.create_work(w).unwrap();
        }
        Op::Rename { n, .. } => {
            if let Some(w) = s.list_works().unwrap().first() {
                s.update_work(&w.id, WorkCol::Title, format!("title{n}").into()).unwrap();
            }
        }
        Op::ExtSet { key, val, .. } => s.ext_data_set("ext", &format!("k{key}"), &serde_json::json!(val)).unwrap(),
        Op::ExtDel { key, .. } => {
            s.ext_data_delete("ext", &format!("k{key}")).unwrap();
        }
        Op::Setting { key, val, .. } => s.set_setting(&format!("s{key}"), &serde_json::json!(val)).unwrap(),
        Op::Event { .. } => s.emit_event("part.read", Some("es"), None, &serde_json::json!({})).unwrap(),
        Op::Media { h, .. } => {
            s.register_media(&hash(*h), "audio/mpeg", 10, Some("a.mp3")).unwrap();
        }
        Op::Dict { h, .. } => s.put_dictionary(&hash(100 + *h), "Dict", Some("r1"), "es").unwrap(),
        Op::DelDict { h, .. } => {
            s.delete_dictionary(&hash(100 + *h)).unwrap();
        }
        Op::Package { ext, v, .. } => s.put_extension_package(&format!("ext{ext}"), &format!("1.{v}"), &hash(200 + *ext)).unwrap(),
        Op::Session { .. } => unreachable!(),
    }
}

/// One session between devices `a` and `b`; either end may be cut off after some messages.
fn session(stores: &[Mutex<Store>], a: usize, b: usize, cut: Option<usize>, batch_rows: usize) -> bool {
    if a == b {
        return true;
    }
    let (ta, tb) = mem::pair(cut, None);
    let mut cfg = SessionConfig::new("dev");
    cfg.batch_rows = batch_rows;
    let (ra, rb) = std::thread::scope(|sc| {
        let ha = sc.spawn(|| {
            let mut ta = ta;
            run(&stores[a], &mut ta, Role::Initiator, &cfg)
        });
        let hb = sc.spawn(|| {
            let mut tb = tb;
            run(&stores[b], &mut tb, Role::Responder, &cfg)
        });
        (ha.join().unwrap(), hb.join().unwrap())
    });
    if cut.is_none() && (ra.is_err() || rb.is_err()) {
        eprintln!("session errors: {:?} / {:?}", ra.as_ref().err(), rb.as_ref().err());
    }
    ra.is_ok() && rb.is_ok()
}

fn full_round(stores: &[Mutex<Store>], batch_rows: usize) {
    for a in 0..stores.len() {
        for b in 0..stores.len() {
            if a != b {
                assert!(session(stores, a, b, None, batch_rows), "full session {a}->{b} failed");
            }
        }
    }
}

fn stores(n: usize) -> Vec<Mutex<Store>> {
    (0..n).map(|_| Mutex::new(Store::open_in_memory().unwrap())).collect()
}

fn assert_converged(stores: &[Mutex<Store>]) -> String {
    let d0 = dump(&stores[0]);
    for (i, s) in stores.iter().enumerate().skip(1) {
        assert_eq!(dump(s), d0, "device {i} differs from device 0");
    }
    for s in stores {
        assert_eq!(s.lock().unwrap().pending_count().unwrap(), 0, "rows left pending");
    }
    d0
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, max_shrink_iters: 200, ..ProptestConfig::default() })]

    #[test]
    fn devices_converge_and_syncing_again_changes_nothing(n in 3..5usize, batch_rows in 1..9usize, ops in prop::collection::vec(op_strategy(4), 1..60)) {
        let st = stores(n);
        for op in &ops {
            match op {
                Op::Session { a, b, cut } => { session(&st, a % n, b % n, cut.map(|c| c as usize), batch_rows); }
                other => { let d = match other { Op::Term{d,..}|Op::DelTerm{d,..}|Op::Parent{d,..}|Op::Work{d,..}|Op::Rename{d,..}|Op::ExtSet{d,..}|Op::ExtDel{d,..}|Op::Setting{d,..}|Op::Event{d}|Op::Media{d,..}|Op::Dict{d,..}|Op::DelDict{d,..}|Op::Package{d,..} => *d, Op::Session{..} => unreachable!() }; write(&st[d % n], other); }
            }
        }
        full_round(&st, batch_rows);
        full_round(&st, batch_rows);
        let before = assert_converged(&st);
        full_round(&st, batch_rows);
        let after = assert_converged(&st);
        prop_assert_eq!(before, after);
    }
}

#[test]
fn a_cut_off_session_resumes_and_loses_nothing() {
    let st = stores(2);
    for i in 0..40 {
        let mut s = st[0].lock().unwrap();
        s.save_term("es", &format!("w{i}"), &format!("w{i}"), 1, &TermPatch { status: Some(2), ..Default::default() }).unwrap();
    }
    // Cut after a few messages: some batches land, the rest doesn't.
    assert!(!session(&st, 0, 1, Some(3), 7));
    let got = st[1].lock().unwrap().term_states("es").unwrap().len();
    assert!(got > 0 && got < 40, "partial progress, got {got}");
    assert!(session(&st, 0, 1, None, 7));
    assert_converged(&st);
    assert_eq!(st[1].lock().unwrap().term_states("es").unwrap().len(), 40);
}

#[test]
fn concurrent_edits_to_different_columns_both_survive_and_same_term_merges() {
    let st = stores(2);
    let p = |status| TermPatch { status: Some(status), ..Default::default() };
    st[0].lock().unwrap().save_term("es", "gato", "gato", 1, &p(1)).unwrap();
    st[1].lock().unwrap().save_term("es", "gato", "gato", 1, &p(1)).unwrap(); // same natural key, offline
    full_round(&st, 500);
    st[0].lock().unwrap().save_term("es", "gato", "gato", 1, &p(4)).unwrap();
    st[1].lock().unwrap().save_term("es", "gato", "gato", 1, &TermPatch { translation: Some("cat".into()), ..Default::default() }).unwrap();
    full_round(&st, 500);
    assert_converged(&st);
    for s in &st {
        let s = s.lock().unwrap();
        let t = s.find_term("es", "gato").unwrap().unwrap();
        assert_eq!((t.id.as_str(), t.status, t.translation.as_deref()), (term_id("es", "gato").as_str(), 4, Some("cat")));
        let n: i64 = s.local_conn().query_row("SELECT count(*) FROM terms", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }
    let _ = derive_id("x", &[]);
}

#[test]
fn a_delete_and_a_revive_on_different_devices_converge() {
    let st = stores(3);
    st[0].lock().unwrap().save_term("es", "x", "x", 1, &TermPatch { status: Some(1), ..Default::default() }).unwrap();
    full_round(&st, 500);
    st[1].lock().unwrap().delete_term("es", "x").unwrap();
    st[2].lock().unwrap().save_term("es", "x", "x", 1, &TermPatch { notes: Some("n".into()), ..Default::default() }).unwrap();
    full_round(&st, 500);
    full_round(&st, 500);
    assert_converged(&st);
}

fn file_stores(dir: &std::path::Path, n: usize) -> Vec<Mutex<Store>> {
    (0..n).map(|i| Mutex::new(Store::open(&dir.join(format!("d{i}.db"))).unwrap())).collect()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 16, max_shrink_iters: 100, ..ProptestConfig::default() })]

    /// The same offline writes synced in two different session orders end in the same state.
    #[test]
    fn the_order_of_sessions_does_not_matter(
        ops in prop::collection::vec(op_strategy(3), 1..40),
        sched in prop::collection::vec((0..3usize, 0..3usize), 0..10),
    ) {
        let base = std::env::temp_dir().join(format!("alst-order-{}", alst_core::store::new_id()));
        let (da, db) = (base.join("a"), base.join("b"));
        std::fs::create_dir_all(&da).unwrap();
        std::fs::create_dir_all(&db).unwrap();
        {
            let st = file_stores(&da, 3);
            for op in &ops {
                if let Op::Session { .. } = op { continue; }
                let d = match op { Op::Term{d,..}|Op::DelTerm{d,..}|Op::Parent{d,..}|Op::Work{d,..}|Op::Rename{d,..}|Op::ExtSet{d,..}|Op::ExtDel{d,..}|Op::Setting{d,..}|Op::Event{d}|Op::Media{d,..}|Op::Dict{d,..}|Op::DelDict{d,..}|Op::Package{d,..} => *d, Op::Session{..} => unreachable!() };
                write(&st[d % 3], op);
            }
        }
        for i in 0..3 {
            std::fs::copy(da.join(format!("d{i}.db")), db.join(format!("d{i}.db"))).unwrap();
        }
        let (a, b) = (file_stores(&da, 3), file_stores(&db, 3));
        for &(x, y) in &sched { session(&a, x, y, None, 5); }
        for &(x, y) in sched.iter().rev() { session(&b, y, x, None, 50); }
        full_round(&a, 5);
        full_round(&a, 5);
        full_round(&b, 50);
        full_round(&b, 50);
        let (ra, rb) = (assert_converged(&a), assert_converged(&b));
        std::fs::remove_dir_all(&base).ok();
        prop_assert_eq!(ra, rb);
    }
}

fn pack_row() -> PackRow {
    PackRow { id: "pack-1".into(), title: "Automoción".into(), language: "es".into(), topic: None, level: Some("B1".into()), description: None, created_by: None, version_at: None, manifest: "{}".into() }
}

fn seed(text: &str, tr: &str) -> SeedTerm {
    SeedTerm { text: text.into(), text_lc: text.into(), word_count: 1, translation: tr.into(), reading: String::new(), notes: String::new() }
}

/// Two devices import the same pack offline: the pack and the seed terms merge (derived ids), and after syncing
/// each device sees the other's items as imported, so a re-import adds nothing twice.
#[test]
fn two_devices_importing_the_same_pack_converge() {
    let st = stores(2);
    let item = |id: &str| WorkPack { pack_id: "pack-1".into(), pack_item: id.into(), license: Some("CC0-1.0".into()), attribution: Some("Ana".into()) };
    for (d, items) in [(0, ["a1", "a2"]), (1, ["a1", "a3"])] {
        let mut s = st[d].lock().unwrap();
        s.put_pack(&pack_row()).unwrap();
        s.seed_terms("es", &[seed("bujía", "spark plug"), seed("freno", "brake")]).unwrap();
        for it in items {
            s.create_work_in_pack(NewWork::text("es", it, "uno dos"), Some(&item(it))).unwrap();
        }
    }
    full_round(&st, 500);
    assert_converged(&st);
    for s in &st {
        let s = s.lock().unwrap();
        assert_eq!(s.packs().unwrap().len(), 1);
        let n: i64 = s.local_conn().query_row("SELECT count(*) FROM terms", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
        let done = s.imported_pack_items("pack-1").unwrap();
        assert!(["a1", "a2", "a3"].iter().all(|i| done.contains(*i)));
    }
}
