//! Text pipeline: declarative tokenizer engine, multi-word term matching, importers.
//! Pure logic: no database, no Tauri.

pub mod cues;
pub mod export;
pub mod import;
pub mod lang;
pub mod reader;

pub use lang::{LanguageDef, LookupUrl, Token};
pub use reader::{Matcher, TermState, Unit};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid language manifest: {0}")]
    Manifest(String),
    #[error("unsupported tokenizer engine '{0}'")]
    Engine(String),
    #[error("{0}")]
    Import(String),
}

pub type Result<T> = std::result::Result<T, Error>;
