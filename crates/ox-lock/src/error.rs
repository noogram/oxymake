//! Error types for the lockfile crate.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum LockError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("TOML serialization error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),

    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("lockfile not found: {0}")]
    NotFound(String),
}
