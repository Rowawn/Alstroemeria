mod common;

use alst_pack::{Error, Pack};
use common::{tmp, zip_files, zip_fixture};
use std::io::Write;
use std::path::Path;

fn err(p: &Path) -> String {
    Pack::open(p).unwrap_err().to_string()
}

fn manifest_with(items: &str) -> String {
    format!(r#"{{"format":"alstroemeria-pack","formatVersion":1,"id":"x","title":"T","language":"es","items":{items}}}"#)
}

#[test]
fn valid_pack_opens() {
    let p = Pack::open(&zip_fixture("valid")).unwrap();
    assert_eq!((p.manifest.items.len(), p.manifest.vocab.len()), (3, 2));
    assert_eq!(p.manifest.language, "es");
    assert!(p.manifest.items.iter().all(|i| i.license_ok()));
    assert!(p.read("items/t1.md", 1000).unwrap().starts_with(b"# Motor"));
}

#[test]
fn unknown_format_version_is_refused() {
    assert!(matches!(Pack::open(&zip_fixture("version2")), Err(Error::Version(v)) if v == "2"));
}

#[test]
fn bad_url_is_refused() {
    assert!(err(&zip_fixture("bad-url")).contains("http or https"));
}

#[test]
fn embedded_item_with_other_license_opens_but_is_flagged() {
    let p = Pack::open(&zip_fixture("license-warning")).unwrap();
    assert!(!p.manifest.items[0].license_ok());
}

#[test]
fn path_traversal_and_absolute_paths_are_refused() {
    let m = manifest_with("[]");
    for evil in ["../evil.txt", "items/../../evil.txt", "/etc/passwd", "items\\..\\x"] {
        let p = zip_files("evil", &[("manifest.json", m.as_bytes()), (evil, b"x")]);
        assert!(err(&p).contains("unsafe path"), "{evil}");
    }
}

#[test]
fn embedded_path_outside_items_or_missing_is_refused() {
    let m = manifest_with(r#"[{"id":"t","kind":"text","title":"t","content":{"mode":"embedded","path":"manifest.json"}}]"#);
    assert!(err(&zip_files("outside", &[("manifest.json", m.as_bytes())])).contains("non-items path"));
    let m = manifest_with(r#"[{"id":"t","kind":"text","title":"t","content":{"mode":"embedded","path":"items/none.md"}}]"#);
    assert!(err(&zip_files("missing", &[("manifest.json", m.as_bytes())])).contains("not in the pack"));
}

#[test]
fn symlinks_are_refused() {
    let path = tmp("link.alpack");
    let mut w = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    w.start_file("manifest.json", opts).unwrap();
    w.write_all(manifest_with("[]").as_bytes()).unwrap();
    w.add_symlink("items/link", "/etc/passwd", opts).unwrap();
    w.finish().unwrap();
    assert!(err(&path).contains("symbolic links"));
}

#[test]
fn limits() {
    // Declared size over 200 MB (highly compressible zeros).
    let m = manifest_with("[]");
    let big = vec![0u8; 201 * 1024 * 1024];
    let p = zip_files("big", &[("manifest.json", m.as_bytes()), ("items/big.bin", &big)]);
    assert!(err(&p).contains("too large"));
    // Manifest over 1 MB.
    let pad = format!(r#"{{"format":"alstroemeria-pack","formatVersion":1,"id":"x","title":"T","language":"es","items":[],"description":"{}"}}"#, "a".repeat(1 << 20));
    assert!(err(&zip_files("bigmanifest", &[("manifest.json", pad.as_bytes())])).contains("too large"));
    // More than 200 items.
    let items: Vec<String> = (0..201).map(|i| format!(r#"{{"id":"i{i}","kind":"article","title":"t","source":{{"url":"https://e.com/{i}"}},"content":{{"mode":"fetch","strategy":"article"}}}}"#)).collect();
    assert!(err(&zip_files("items", &[("manifest.json", manifest_with(&format!("[{}]", items.join(","))).as_bytes())])).contains("too many items"));
    // More than 2000 vocabulary entries.
    let vocab: Vec<String> = (0..2001).map(|i| format!(r#"{{"text":"w{i}"}}"#)).collect();
    let m = format!(r#"{{"format":"alstroemeria-pack","formatVersion":1,"id":"x","title":"T","language":"es","items":[],"vocab":[{}]}}"#, vocab.join(","));
    assert!(err(&zip_files("vocab", &[("manifest.json", m.as_bytes())])).contains("vocabulary"));
}

#[test]
fn duplicate_item_ids_and_unknown_kinds_are_refused() {
    let it = r#"{"id":"a","kind":"article","title":"t","source":{"url":"https://e.com"},"content":{"mode":"fetch","strategy":"article"}}"#;
    assert!(err(&zip_files("dup", &[("manifest.json", manifest_with(&format!("[{it},{it}]")).as_bytes())])).contains("repeated"));
    let it = r#"{"id":"a","kind":"poem","title":"t","source":{"url":"https://e.com"},"content":{"mode":"fetch","strategy":"article"}}"#;
    assert!(err(&zip_files("kind", &[("manifest.json", manifest_with(&format!("[{it}]")).as_bytes())])).contains("unknown kind"));
}

#[test]
fn not_a_zip_and_no_manifest() {
    let p = tmp("junk.alpack");
    std::fs::write(&p, b"hello").unwrap();
    assert!(err(&p).contains("zip"));
    assert!(err(&zip_files("nomanifest", &[("items/a.md", b"x")])).contains("manifest.json not found"));
}
