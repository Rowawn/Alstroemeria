//! Content packs (SPEC §10): read and validate a `.alpack`, plan an import, run it. No Tauri.

pub mod manifest;
mod pack;
pub mod plan;
pub mod run;
pub mod sources;

pub use manifest::{Content, Estimate, Item, Manifest, Source, VocabEntry};
pub use pack::{MAX_PACK_BYTES, Pack};
pub use plan::{Caps, Plan, PlanItem, Status, plan};
pub use run::{ItemFailure, ItemState, Progress, Report, RunOptions, run};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("this pack uses format version {0}, which this app does not understand; update the app")]
    Version(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// The language needs data that isn't installed; the UI shows the download prompt (`langdata:<id>`).
    #[error("langdata:{0}")]
    LangData(String),
    #[error("{0}")]
    Core(#[from] alst_core::Error),
    #[error("{0}")]
    Text(#[from] alst_text::Error),
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;
