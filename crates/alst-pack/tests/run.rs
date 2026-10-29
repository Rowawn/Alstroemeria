//! Planning and running an import against a local HTTP fixture server (SPEC §10.3).

mod common;

use alst_core::{Store, media::MediaDir};
use alst_pack::sources::Env;
use alst_pack::{Caps, ItemState, Pack, RunOptions, Status, plan, run};
use alst_text::LanguageDef;
use common::{tmp, zip_files};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const PAGE: &str = "<html><head><title>Site</title></head><body><article><h1>El motor</h1>\
<p>El motor de un coche tiene muchas piezas pequeñas que trabajan juntas todos los días sin parar.</p>\
<p>La bujía produce la chispa que enciende la mezcla de aire y combustible dentro del cilindro.</p></article></body></html>";
const SRT: &str = "1\n00:00:01,000 --> 00:00:03,000\nHola a todos.\n\n2\n00:00:03,500 --> 00:00:06,000\nHoy hablamos del motor.\n";

/// Serves fixed bodies; `/flaky` fails the first `flaky_failures` times; unknown paths are 404.
struct Server {
    base: String,
    flaky_hits: Arc<AtomicUsize>,
}

fn serve(flaky_failures: usize) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let feed = format!(
        r#"<?xml version="1.0"?><rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd" xmlns:p="https://podcastindex.org/namespace/1.0">
<channel><title>Charlas</title><item><title>Uno</title><guid>g1</guid><enclosure url="{base}/audio.mp3" type="audio/mpeg"/>
<p:transcript url="{base}/t.srt" type="application/x-subrip"/></item></channel></rss>"#
    );
    let mut routes: HashMap<&'static str, (&'static str, Vec<u8>)> = HashMap::new();
    routes.insert("/article", ("text/html; charset=utf-8", PAGE.as_bytes().to_vec()));
    routes.insert("/feed.xml", ("application/rss+xml", feed.into_bytes()));
    routes.insert("/audio.mp3", ("audio/mpeg", vec![7u8; 2048]));
    routes.insert("/t.srt", ("application/x-subrip", SRT.as_bytes().to_vec()));
    routes.insert("/notes.txt", ("text/plain; charset=utf-8", "Primera línea del texto.\n\nSegundo párrafo con más palabras.".as_bytes().to_vec()));
    routes.insert("/flaky.txt", ("text/plain; charset=utf-8", "Texto que llega al segundo intento.".as_bytes().to_vec()));
    let hits = Arc::new(AtomicUsize::new(0));
    let flaky_hits = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let (routes, hits) = (routes.clone(), hits.clone());
            std::thread::spawn(move || {
                let mut buf = [0u8; 2048];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                let flaky_fail = path == "/flaky.txt" && hits.fetch_add(1, Ordering::SeqCst) < flaky_failures;
                let (status, ct, body) = match routes.get(path.as_str()) {
                    Some(_) if flaky_fail => ("500 Internal Server Error", "text/plain", b"boom".to_vec()),
                    Some((ct, b)) => ("200 OK", *ct, b.clone()),
                    None => ("404 Not Found", "text/plain", b"nope".to_vec()),
                };
                let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(&body);
            });
        }
    });
    Server { base, flaky_hits }
}

fn es() -> LanguageDef {
    LanguageDef::from_manifest(include_str!("../../../extensions/lang-es/extension.json")).unwrap()
}

struct Ctx {
    store: Mutex<Store>,
    media: MediaDir,
    lang: LanguageDef,
}

impl Ctx {
    fn new() -> Ctx {
        Ctx { store: Mutex::new(Store::open_in_memory().unwrap()), media: MediaDir::new(tmp(&format!("media-{}", alst_core::store::new_id()))), lang: es() }
    }
    fn env(&self) -> Env<'_> {
        Env { store: &self.store, media: &self.media, lang: &self.lang }
    }
}

fn manifest(base: &str, items: &str) -> String {
    let items = items.replace("BASE", base);
    format!(
        r#"{{"format":"alstroemeria-pack","formatVersion":1,"id":"pack-1","title":"Automoción","language":"es","level":"B1","items":[{items}],
"vocab":[{{"text":"bujía","translation":"spark plug"}},{{"text":"freno de mano","translation":"handbrake"}}]}}"#
    )
}

const ARTICLE: &str = r#"{"id":"a1","kind":"article","title":"El motor","source":{"url":"BASE/article","license":"unknown","attribution":"Ejemplo"},"content":{"mode":"fetch","strategy":"article"}}"#;
const PODCAST: &str = r#"{"id":"p1","kind":"podcast","title":"Charla uno","source":{"url":"BASE/feed.xml","attribution":"Charlas"},"content":{"mode":"fetch","strategy":"rss-episode","guid":"g1"}}"#;
const DIRECT: &str = r#"{"id":"d1","kind":"text","title":"Notas","source":{"url":"BASE/notes.txt"},"content":{"mode":"fetch","strategy":"direct"}}"#;
const EMBEDDED: &str = r#"{"id":"e1","kind":"text","title":"Incrustado","source":{"license":"CC-BY-SA-4.0","attribution":"Wikipedia contributors"},"content":{"mode":"embedded","path":"items/e1.md"}}"#;
const EMBEDDED_MEDIA: &str = r#"{"id":"e2","kind":"podcast","title":"Audio incrustado","source":{"license":"own"},"content":{"mode":"embedded","path":"items/e2.srt","mediaPath":"items/e2.mp3"}}"#;
const BAD: &str = r#"{"id":"x1","kind":"article","title":"Roto","source":{"url":"BASE/missing"},"content":{"mode":"fetch","strategy":"article"}}"#;
const YOUTUBE: &str = r#"{"id":"y1","kind":"video","title":"Vídeo","source":{"url":"https://www.youtube.com/watch?v=abc"},"content":{"mode":"fetch","strategy":"youtube-subs"}}"#;
const FLAKY: &str = r#"{"id":"f1","kind":"text","title":"Intermitente","source":{"url":"BASE/flaky.txt"},"content":{"mode":"fetch","strategy":"direct"}}"#;

fn open(m: &str) -> Pack {
    let path = zip_files(
        &format!("run-{}", alst_core::store::new_id()),
        &[("manifest.json", m.as_bytes()), ("items/e1.md", b"# Motor\n\nEl motor tiene una buj\xc3\xada.\n"), ("items/e2.srt", SRT.as_bytes()), ("items/e2.mp3", &[3u8; 512])],
    );
    Pack::open(&path).unwrap()
}

fn all(p: &Pack) -> RunOptions {
    RunOptions { items: p.manifest.items.iter().map(|i| i.id.clone()).collect(), seed_vocab: true }
}

fn go(ctx: &Ctx, p: &Pack, o: &RunOptions) -> alst_pack::Report {
    run(&ctx.env(), p, o, &AtomicBool::new(false), &|_| {}).unwrap()
}

#[test]
fn plan_reports_what_will_happen_without_writing() {
    let srv = serve(0);
    let ctx = Ctx::new();
    let p = open(&manifest(&srv.base, &[ARTICLE, YOUTUBE, EMBEDDED].join(",")));
    let pl = plan(&p, &ctx.store.lock().unwrap(), Some(&ctx.lang), Caps { youtube: false }).unwrap();
    let st: Vec<_> = pl.items.iter().map(|i| (i.id.as_str(), i.status, i.checked)).collect();
    assert_eq!(st, [("a1", Status::Ready, true), ("y1", Status::NeedsDesktop, false), ("e1", Status::Ready, true)]);
    assert_eq!(pl.items[0].domain.as_deref(), Some("127.0.0.1"));
    assert_eq!((pl.vocab_total, pl.vocab.new, pl.vocab.fill), (2, 2, 0));
    assert!(ctx.store.lock().unwrap().list_works().unwrap().is_empty());
    assert!(ctx.store.lock().unwrap().packs().unwrap().is_empty());
    // With yt-dlp the video is ready.
    let pl = plan(&p, &ctx.store.lock().unwrap(), Some(&ctx.lang), Caps { youtube: true }).unwrap();
    assert_eq!(pl.items[1].status, Status::Ready);
}

#[test]
fn imports_every_source_type_with_pack_origin_and_attribution() {
    let srv = serve(0);
    let ctx = Ctx::new();
    let p = open(&manifest(&srv.base, &[ARTICLE, PODCAST, DIRECT, EMBEDDED, EMBEDDED_MEDIA].join(",")));
    let events = Mutex::new(Vec::new());
    let r = run(&ctx.env(), &p, &all(&p), &AtomicBool::new(false), &|e| events.lock().unwrap().push((e.item_id.clone(), e.state))).unwrap();
    assert!(r.failed.is_empty(), "{:?}", r.failed);
    assert_eq!(r.imported.len(), 5);
    let s = ctx.store.lock().unwrap();
    let works = s.list_works().unwrap();
    assert_eq!(works.len(), 5);
    let by = |item: &str| works.iter().find(|w| w.pack_item.as_deref() == Some(item)).unwrap();
    assert_eq!((by("a1").title.as_str(), by("a1").kind.as_str(), by("a1").attribution.as_deref(), by("a1").source_license.as_deref()), ("El motor", "article", Some("Ejemplo"), Some("unknown")));
    assert_eq!(by("p1").kind, "audio");
    assert_eq!(by("p1").title, "Charla uno");
    assert_eq!(by("e1").attribution.as_deref(), Some("Wikipedia contributors"));
    assert!(works.iter().all(|w| w.pack_id.as_deref() == Some("pack-1")));
    // The podcast and the embedded media keep their audio and cue timing.
    let parts = s.list_parts(&by("p1").id).unwrap();
    let part = s.get_part(&parts[0].id).unwrap();
    assert!(part.media_hash.is_some() && part.timing.is_some());
    let embedded = s.get_part(&s.list_parts(&by("e2").id).unwrap()[0].id).unwrap();
    assert!(embedded.timing.is_some() && ctx.media.exists(embedded.media_hash.as_deref().unwrap()));
    // Markdown became plain text.
    assert_eq!(s.get_part(&s.list_parts(&by("e1").id).unwrap()[0].id).unwrap().text, "Motor\n\nEl motor tiene una bujía.");
    assert_eq!(s.packs().unwrap().len(), 1);
    // Progress: every item started then finished, in order.
    let ev = events.lock().unwrap();
    assert_eq!(ev.iter().filter(|(_, st)| *st == ItemState::Done).count(), 5);
    assert_eq!(ev[0], ("a1".to_string(), ItemState::Started));
    // Seed vocab was created.
    assert_eq!(s.find_term("es", "bujía").unwrap().unwrap().translation.as_deref(), Some("spark plug"));
    assert_eq!(r.vocab.unwrap().new, 2);
}

#[test]
fn a_failed_item_does_not_stop_the_rest_and_retry_imports_only_it() {
    let srv = serve(1);
    let ctx = Ctx::new();
    let p = open(&manifest(&srv.base, &[BAD, FLAKY, DIRECT].join(",")));
    let r = go(&ctx, &p, &all(&p));
    assert_eq!(r.imported.len(), 1, "{:?}", r.failed);
    let failed: Vec<_> = r.failed.iter().map(|f| f.item_id.as_str()).collect();
    assert_eq!(failed, ["x1", "f1"]);
    assert!(r.failed[0].error.contains("fetch failed"), "{}", r.failed[0].error);
    // Retry the failed ones: the flaky source now answers, the bad one fails again.
    let retry = RunOptions { items: failed.iter().map(|s| s.to_string()).collect(), seed_vocab: false };
    let r2 = go(&ctx, &p, &retry);
    assert_eq!((r2.imported.len(), r2.failed.len()), (1, 1), "{:?}", r2.failed);
    assert_eq!(ctx.store.lock().unwrap().list_works().unwrap().len(), 2);
    assert!(srv.flaky_hits.load(Ordering::SeqCst) >= 2);
}

#[test]
fn reimporting_skips_existing_items_and_a_new_version_adds_only_new_ones() {
    let srv = serve(0);
    let ctx = Ctx::new();
    let v1 = open(&manifest(&srv.base, &[ARTICLE, DIRECT].join(",")));
    assert_eq!(go(&ctx, &v1, &all(&v1)).imported.len(), 2);
    // Same pack again: the plan says already-imported and a forced run skips them.
    let pl = plan(&v1, &ctx.store.lock().unwrap(), Some(&ctx.lang), Caps { youtube: true }).unwrap();
    assert!(pl.items.iter().all(|i| i.status == Status::AlreadyImported && !i.checked));
    let again = go(&ctx, &v1, &all(&v1));
    assert_eq!((again.imported.len(), again.skipped.len()), (0, 2));
    // Version 2 has one more item.
    let v2 = open(&manifest(&srv.base, &[ARTICLE, DIRECT, EMBEDDED].join(",")));
    let r = go(&ctx, &v2, &all(&v2));
    assert_eq!((r.imported.len(), r.skipped.len()), (1, 2));
    assert_eq!(ctx.store.lock().unwrap().list_works().unwrap().len(), 3);
    assert_eq!(ctx.store.lock().unwrap().packs().unwrap().len(), 1);
}

#[test]
fn cancel_stops_before_the_next_item() {
    let srv = serve(0);
    let ctx = Ctx::new();
    let p = open(&manifest(&srv.base, &[ARTICLE, DIRECT, EMBEDDED].join(",")));
    let cancel = AtomicBool::new(false);
    let r = run(&ctx.env(), &p, &all(&p), &cancel, &|e| {
        if e.state == ItemState::Done {
            cancel.store(true, Ordering::SeqCst);
        }
    })
    .unwrap();
    assert!(r.cancelled);
    assert_eq!(r.imported.len(), 1);
    assert!(r.vocab.is_none());
}

#[test]
fn seeding_is_optional() {
    let srv = serve(0);
    let ctx = Ctx::new();
    let p = open(&manifest(&srv.base, EMBEDDED));
    let r = go(&ctx, &p, &RunOptions { items: vec!["e1".into()], seed_vocab: false });
    assert!(r.vocab.is_none());
    assert!(ctx.store.lock().unwrap().find_term("es", "bujía").unwrap().is_none());
}
