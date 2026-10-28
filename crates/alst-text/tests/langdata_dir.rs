//! Loads a Lindera dictionary from an installed language data folder (SPEC §17.3).
//! Needs a package unzipped by hand, so it is skipped unless `ALST_TEST_LANGDATA_JA` names that folder:
//!   unzip dist/langdata/langdata-ja-ipadic-1.zip -d /tmp/ja && ALST_TEST_LANGDATA_JA=/tmp/ja cargo test -p alst-text --test langdata_dir
#![cfg(feature = "lang-ja")]

use alst_text::LanguageDef;
use alst_text::morph::{self, Morph};

#[test]
fn tokenizes_from_a_directory() {
    let lang = LanguageDef::from_manifest(include_str!("../../../extensions/lang-ja/extension.json")).unwrap();
    #[cfg(not(feature = "embed-ja"))]
    {
        assert_eq!(lang.langdata_missing(), Some("ja-ipadic"));
        assert!(matches!(morph::analyze(Morph::Ja, "猫"), Err(alst_text::Error::LangData(_))));
    }
    let Ok(dir) = std::env::var("ALST_TEST_LANGDATA_JA") else { return };
    morph::set_dict_dir(Morph::Ja, Some(dir.into()));
    assert!(morph::available(Morph::Ja));
    let words: Vec<String> = lang.tokenize_flat("私は猫が好きです。").into_iter().filter(|t| t.is_word).map(|t| t.text).collect();
    assert!(words.contains(&"猫".to_string()) && words.contains(&"好き".to_string()), "{words:?}");
}
