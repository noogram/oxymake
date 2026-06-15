//! Error types for the codec subsystem.

use std::path::PathBuf;

/// Errors that can occur during codec operations.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    /// No codec registered for the given file extension.
    #[error("no codec for extension {extension:?} (file: {path})")]
    UnknownExtension {
        /// The file extension that was not recognized.
        extension: String,
        /// The file path that triggered the lookup.
        path: PathBuf,
    },

    /// No format could be determined (no extension and no explicit hint).
    #[error("cannot detect format for {path}: no file extension and no format hint")]
    NoFormatDetectable {
        /// The path with no recognizable extension.
        path: PathBuf,
    },
}
