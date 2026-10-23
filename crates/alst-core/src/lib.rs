//! Alstroemeria core: database, migrations, HLC, change-capturing write layer.

pub mod db;
pub mod hlc;
pub mod media;
pub mod store;

pub use store::{EventRow, ExtStateRow, MediaRow, NewPart, NewWork, Part, PartInfo, STATUSES, Store, Term, TermPatch, TermRef, TermStateRow, Work};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;
