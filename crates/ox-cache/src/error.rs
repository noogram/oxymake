//! Error types for the ox-cache crate.

/// Errors that can occur during cache operations.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    /// An I/O error occurred while reading/writing files or the manifest.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The cache manifest file is corrupted or cannot be deserialized.
    #[error("cache manifest corrupted: {0}")]
    Manifest(String),

    /// A content hash did not match the expected value.
    #[error("hash mismatch for {path}: expected {expected}, got {actual}")]
    HashMismatch {
        path: String,
        expected: String,
        actual: String,
    },
}
