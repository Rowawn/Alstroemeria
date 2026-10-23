//! Extension system core (SPEC §6, §6.9): manifests, registry, capability resolution, install,
//! permissions. Pure Rust: no Tauri, no database.

pub mod host;
pub mod install;
pub mod manifest;
pub mod net;
pub mod perm;
pub mod registry;
pub mod resolve;

pub use manifest::{API_VERSION, Kind, Manifest, RuntimeType, settings_with_defaults, validate_settings};
pub use perm::{Need, Permission};
pub use registry::{Extension, LoadError, Registry, Source};
pub use resolve::{CORE_CAPABILITIES, Candidate, Disabled, Input, ProviderChoice, Resolved, UserState, resolve};

/// Limits for `alst.storage` (SPEC §6.9.4).
pub const STORAGE_MAX_VALUE_BYTES: usize = 64 * 1024;
pub const STORAGE_MAX_KEYS: usize = 1000;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid manifest: {0}")]
    Manifest(String),
    #[error("{0}")]
    Invalid(String),
    #[error("invalid package: {0}")]
    Package(String),
    #[error("permission denied: {0}")]
    Permission(String),
    #[error("network: {0}")]
    Net(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
