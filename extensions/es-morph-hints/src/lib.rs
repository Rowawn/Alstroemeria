//! Spanish morphology hints: WASM ABI v1 (SPEC §6.9.8).
//! Hooks take and return UTF-8 JSON in linear memory; the return value packs `(ptr << 32) | len`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---- ABI ------------------------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn alst_alloc(len: u32) -> u32 {
    Box::into_raw(vec![0u8; len as usize].into_boxed_slice()) as *mut u8 as u32
}

/// # Safety
/// `ptr`/`len` must come from `alst_alloc` or from a hook result, and be freed once.
#[no_mangle]
pub unsafe extern "C" fn alst_free(ptr: u32, len: u32) {
    drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr as *mut u8, len as usize)));
}

fn input<'a>(ptr: u32, len: u32) -> &'a [u8] {
    unsafe { std::slice::from_raw_parts(ptr as *const u8, len as usize) }
}

fn output(value: &impl Serialize) -> u64 {
    let bytes = serde_json::to_vec(value).unwrap_or_default().into_boxed_slice();
    let len = bytes.len() as u64;
    let ptr = Box::into_raw(bytes) as *mut u8 as u64;
    (ptr << 32) | len
}

#[no_mangle]
pub extern "C" fn alst_annotate(ptr: u32, len: u32) -> u64 {
    match serde_json::from_slice::<AnnotateInput>(input(ptr, len)) {
        Ok(i) => output(&annotate(&i)),
        Err(_) => 0,
    }
}

#[no_mangle]
pub extern "C" fn alst_decorate(ptr: u32, len: u32) -> u64 {
    match serde_json::from_slice::<DecorateInput>(input(ptr, len)) {
        Ok(i) => output(&decorate(&i)),
        Err(_) => 0,
    }
}

// ---- logic ----------------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Tok {
    #[serde(default)]
    is_word: bool,
    #[serde(default)]
    norm: String,
    #[serde(default)]
    ann: BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct AnnotateInput {
    tokens: Vec<Tok>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Unit {
    #[serde(default)]
    is_word: bool,
    #[serde(default)]
    word_count: usize,
    token_range: [usize; 2],
}

#[derive(Deserialize)]
struct Settings {
    #[serde(default = "yes")]
    infinitives: bool,
    #[serde(default = "yes")]
    gerunds: bool,
    #[serde(default = "yes")]
    participles: bool,
    #[serde(default = "yes")]
    adverbs: bool,
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Settings { infinitives: true, gerunds: true, participles: true, adverbs: true }
    }
}

#[derive(Deserialize)]
struct DecorateInput {
    units: Vec<Unit>,
    tokens: Vec<Tok>,
    #[serde(default)]
    settings: Option<Settings>,
}

#[derive(Serialize, PartialEq, Debug)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Hint {
    Underline { role: &'static str, style: &'static str },
    Tooltip { text: &'static str },
}

/// Common nouns and adjectives that end like infinitives.
const NOT_VERBS: &[&str] = &[
    "mujer", "taller", "lugar", "hogar", "mayor", "menor", "mejor", "peor", "color", "dolor", "amor", "calor", "valor", "motor",
    "señor", "doctor", "favor", "error", "temor", "sabor", "olor", "sudor", "pastor", "azúcar", "mar", "par",
];

fn classify(norm: &str) -> Option<&'static str> {
    let n = norm.chars().count();
    if n > 6 && norm.ends_with("mente") {
        Some("adverb")
    } else if n > 5 && (norm.ends_with("ando") || norm.ends_with("iendo") || norm.ends_with("yendo")) {
        Some("gerund")
    } else if n > 4 && (norm.ends_with("ado") || norm.ends_with("ido")) {
        Some("participle")
    } else if n > 3 && (norm.ends_with("ar") || norm.ends_with("er") || norm.ends_with("ir")) && !NOT_VERBS.contains(&norm) {
        Some("infinitive")
    } else {
        None
    }
}

fn annotate(i: &AnnotateInput) -> BTreeMap<usize, BTreeMap<&'static str, &'static str>> {
    let mut out = BTreeMap::new();
    for (idx, t) in i.tokens.iter().enumerate() {
        if let Some(kind) = t.is_word.then(|| classify(&t.norm)).flatten() {
            out.insert(idx, BTreeMap::from([("es.morph", kind)]));
        }
    }
    out
}

fn decorate(i: &DecorateInput) -> BTreeMap<usize, Vec<Hint>> {
    let s = i.settings.as_ref().map_or_else(Settings::default, |s| Settings {
        infinitives: s.infinitives,
        gerunds: s.gerunds,
        participles: s.participles,
        adverbs: s.adverbs,
    });
    let mut out = BTreeMap::new();
    for (idx, u) in i.units.iter().enumerate() {
        if !u.is_word || u.word_count != 1 {
            continue;
        }
        let Some(kind) = i.tokens.get(u.token_range[0]).and_then(|t| t.ann.get("es.morph")).and_then(|v| v.as_str()) else {
            continue;
        };
        let (on, role, style, label) = match kind {
            "infinitive" => (s.infinitives, "verb", "solid", "infinitive"),
            "gerund" => (s.gerunds, "verb", "dotted", "gerund"),
            "participle" => (s.participles, "verb", "wavy", "participle"),
            "adverb" => (s.adverbs, "adverb", "solid", "adverb in -mente"),
            _ => continue,
        };
        if on {
            out.insert(idx, vec![Hint::Underline { role, style }, Hint::Tooltip { text: label }]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_suffix() {
        assert_eq!(classify("hablar"), Some("infinitive"));
        assert_eq!(classify("comiendo"), Some("gerund"));
        assert_eq!(classify("hablando"), Some("gerund"));
        assert_eq!(classify("hablado"), Some("participle"));
        assert_eq!(classify("rápidamente"), Some("adverb"));
        assert_eq!(classify("mujer"), None);
        assert_eq!(classify("mar"), None);
        assert_eq!(classify("casa"), None);
    }

    // The pointer-packing ABI only works with 32-bit pointers, so it is exercised end to end from JS (wasm-shim.js).
    #[test]
    fn annotates_words_only() {
        let json = br#"{"tokens":[{"isWord":true,"norm":"corriendo"},{"isWord":false,"norm":" "}]}"#;
        let i: AnnotateInput = serde_json::from_slice(json).unwrap();
        assert_eq!(serde_json::to_string(&annotate(&i)).unwrap(), r#"{"0":{"es.morph":"gerund"}}"#);
    }

    #[test]
    fn decorate_respects_settings() {
        let json = r#"{"units":[{"isWord":true,"wordCount":1,"tokenRange":[0,0]}],
            "tokens":[{"isWord":true,"norm":"corriendo","ann":{"es.morph":"gerund"}}],
            "settings":{"gerunds":false}}"#;
        let i: DecorateInput = serde_json::from_str(json).unwrap();
        assert!(decorate(&i).is_empty());
        let on = json.replace(r#""settings":{"gerunds":false}"#, r#""settings":{"gerunds":true}"#);
        let i: DecorateInput = serde_json::from_str(&on).unwrap();
        assert_eq!(
            serde_json::to_string(&decorate(&i)).unwrap(),
            r#"{"0":[{"kind":"underline","role":"verb","style":"dotted"},{"kind":"tooltip","text":"gerund"}]}"#
        );
    }
}
