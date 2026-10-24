//! Tokenizing time for a ~5,000-character part (SPEC §16.1 measurement). `cargo run --release -p alst-text --example bench`
use alst_text::LanguageDef;
use std::time::Instant;

fn main() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    for (id, manifest, prose) in [
        ("ja", include_str!("../../../extensions/lang-ja/extension.json"), "ja/prose.txt"),
        ("ko", include_str!("../../../extensions/lang-ko/extension.json"), "ko/prose.txt"),
    ] {
        let l = LanguageDef::from_manifest(manifest).unwrap();
        let base = std::fs::read_to_string(format!("{dir}/{prose}")).unwrap();
        let mut text = String::new();
        while text.chars().count() < 5000 {
            text.push_str(&base);
        }
        let t0 = Instant::now();
        let first = l.tokenize(&text[..30]).len(); // loads the dictionary
        let load = t0.elapsed();
        let mut best = u128::MAX;
        for _ in 0..5 {
            let t = Instant::now();
            let s = l.tokenize(&text);
            best = best.min(t.elapsed().as_millis());
            assert!(!s.is_empty());
        }
        println!("{id}: {} chars; first call (dictionary load) {load:?} [{first}]; best of 5 full tokenize: {best} ms", text.chars().count());
    }
}
