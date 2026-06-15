//! Error types for the public API facade.
//!
//! [`ApiError`] unifies errors from all internal crates so callers only
//! need a single error type.

use std::path::PathBuf;

use thiserror::Error;

/// Unified error type for the public OxyMake API.
#[derive(Debug, Error)]
pub enum ApiError {
    /// Oxymakefile could not be read from disk.
    #[error("cannot read Oxymakefile `{path}`: {source}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },

    /// TOML parsing or semantic validation failed.
    #[error(transparent)]
    Parse(#[from] ox_core::error::ParseError),

    /// Workflow validation produced one or more errors.
    #[error("workflow validation failed: {}", .0.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; "))]
    Validation(Vec<ox_core::error::ParseError>),

    /// DAG construction or resolution failed.
    #[error(transparent)]
    Dag(#[from] ox_core::error::DagError),

    /// A cache operation failed.
    #[error(transparent)]
    Cache(#[from] ox_cache::CacheError),

    /// State database operation failed.
    #[error(transparent)]
    State(#[from] ox_state::error::StateError),

    /// Generic I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The builder was not fully configured.
    #[error("session builder incomplete: {0}")]
    Builder(String),
}
