//! LAN sync (SPEC §9): the wire protocol, the session state machine, and the Noise/TCP transport.
//! Pure Rust, no Tauri. Everything that touches synced tables goes through `alst_core::Store`.

pub mod assets;
pub mod discovery;
pub mod noise;
pub mod peers;
pub mod proto;
pub mod service;
pub mod session;

pub use proto::{Msg, Transport};
pub use session::{Outcome, PeerInfo, Role, SessionConfig, run};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Store(#[from] alst_core::Error),
    #[error("connection: {0}")]
    Io(std::io::Error),
    #[error("protocol: {0}")]
    Protocol(String),
    /// The other side (or we) refused the session; the text is meant for the user.
    #[error("{0}")]
    Refused(String),
    #[error("connection closed")]
    Closed,
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        use std::io::ErrorKind::*;
        match e.kind() {
            UnexpectedEof | ConnectionReset | ConnectionAborted | BrokenPipe => Error::Closed,
            _ => Error::Io(e),
        }
    }
}
