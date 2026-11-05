//! Alstroemeria SRS core (SPEC §11): FSRS-6 scheduling, policies, queues, review history and replay.
//! Pure Rust on top of `alst-core::store`; no Tauri.

pub mod anki;
pub mod concept;
pub mod fsrs;
pub mod pool;
pub mod render;
pub mod replay;
pub mod sched;
pub mod template;

pub use alst_core::{Error, Result};
pub use replay::attach;

/// Open an in-memory store with the replay hook installed (tests).
pub fn open_in_memory() -> Result<alst_core::Store> {
    let mut s = alst_core::Store::open_in_memory()?;
    attach(&mut s);
    Ok(s)
}
