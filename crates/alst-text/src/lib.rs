//! Text pipeline: declarative tokenizer engine, multi-word term matching, importers.
//! Pure logic: no database, no Tauri.

pub mod cues;
pub mod deinflect;
pub mod export;
pub mod import;
pub mod lang;
pub mod langdata;
pub mod morph;
pub mod reader;

pub use lang::{LanguageDef, LookupUrl, TermInfo, TermKey, Token};
pub use reader::{Matcher, TermState, Unit};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid language manifest: {0}")]
    Manifest(String),
    #[error("unsupported tokenizer engine '{0}'")]
    Engine(String),
    /// The language data package with this id (SPEC §17.3) isn't installed.
    #[error("language data '{0}' is not installed")]
    LangData(String),
    #[error("{0}")]
    Import(String),
}

pub type Result<T> = std::result::Result<T, Error>;
