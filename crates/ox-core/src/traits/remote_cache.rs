//! # Remote Cache Trait
//!
//! Defines the plugin interface for remote artifact caching. Implementations
//! allow sharing build artifacts across machines (CI runners, developer
//! workstations) by storing and retrieving output files keyed by their
//! content hash.
//!
//! Unlike the local cache check which only answers "is this cached?",
//! the remote cache trait moves actual artifacts: downloading them on fetch
//! and uploading them on store.
//!
//! Remote caches always use [`ContentHash`] keys — mtime is meaningless
//! across machines.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use crate::model::ContentHash;

/// A remote cache backend that can fetch and store build artifacts.
///
/// The trait is object-safe and `Send + Sync` so it can be wrapped in `Arc`
/// and shared across async tasks.
///
/// Cache keys are content hashes (blake3). Each key maps to a single artifact
/// file. Multi-file outputs are stored as one entry per file.
pub trait RemoteCache: Send + Sync {
    /// Fetch a cached artifact by its content hash.
    ///
    /// If the artifact exists in the remote cache, downloads it to `dest`
    /// and returns `Ok(true)`. If not found, returns `Ok(false)` without
    /// modifying `dest`. Returns `Err` on transport or I/O failures.
    ///
    /// # Integrity contract (mandatory)
    ///
    /// Implementations MUST re-verify the downloaded artifact: after the
    /// transfer, the blake3 hash of `dest` must equal `key`. On mismatch the
    /// implementation MUST delete `dest` and return `Ok(false)` (treat the
    /// entry as absent). A remote cache is a shared, writable namespace —
    /// without this post-transfer check, any party able to write to the
    /// backend (or a transport corruption) can poison every consumer's
    /// outputs with content that does not match its advertised hash.
    fn fetch<'a>(
        &'a self,
        key: &'a ContentHash,
        dest: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<bool, RemoteCacheError>> + Send + 'a>>;

    /// Store an artifact in the remote cache under the given content hash.
    ///
    /// Reads the file at `source` and uploads it. If an entry with the same
    /// key already exists, this is a no-op (content-addressed — same hash
    /// means same content).
    fn store<'a>(
        &'a self,
        key: &'a ContentHash,
        source: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<(), RemoteCacheError>> + Send + 'a>>;

    /// Check whether an artifact exists in the remote cache without
    /// downloading it.
    ///
    /// Returns `Ok(true)` if present, `Ok(false)` if absent, `Err` on
    /// transport failure.
    fn exists<'a>(
        &'a self,
        key: &'a ContentHash,
    ) -> Pin<Box<dyn Future<Output = Result<bool, RemoteCacheError>> + Send + 'a>>;
}

/// Errors that can occur during remote cache operations.
#[derive(Debug, thiserror::Error)]
pub enum RemoteCacheError {
    /// I/O error reading or writing the local file.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The remote backend returned a transport-level error (HTTP, gRPC, etc.).
    #[error("transport error: {message}")]
    Transport {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Authentication or authorization failure.
    #[error("auth error: {0}")]
    Auth(String),

    /// The remote backend is not configured or unreachable.
    #[error("backend unavailable: {0}")]
    Unavailable(String),
}

impl RemoteCacheError {
    /// Create a transport error with a source.
    pub fn transport(
        message: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Transport {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }

    /// Create a transport error without a source.
    pub fn transport_msg(message: impl Into<String>) -> Self {
        Self::Transport {
            message: message.into(),
            source: None,
        }
    }
}
