//! Error types for the JSON reporter.
//!
//! The JSON reporter is intentionally simple — it serializes events to JSON and
//! writes them to a stream. The only failure modes are I/O errors (the
//! underlying writer fails) and serialization errors (an `Event` variant cannot
//! be represented as JSON, which should never happen in practice since all
//! variants derive `Serialize`).

use thiserror::Error;

/// Errors that can occur during JSON event reporting.
///
/// In normal operation these are never surfaced to the user — `on_event` and
/// `finish` swallow errors with `.ok()` because a reporter must not crash the
/// build. The error type exists so that lower-level helpers can return
/// `Result<(), JsonReportError>` for testing and for callers who need
/// fine-grained control.
#[derive(Debug, Error)]
pub enum JsonReportError {
    /// The underlying writer returned an I/O error.
    ///
    /// This typically means stdout was closed (broken pipe) or the output file
    /// ran out of disk space.
    #[error("I/O error writing JSON event: {0}")]
    Io(#[from] std::io::Error),

    /// An event could not be serialized to JSON.
    ///
    /// This should never happen — all `Event` variants derive `Serialize` — but
    /// we handle it explicitly rather than panicking in production.
    #[error("JSON serialization error: {0}")]
    Serialize(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_error_displays() {
        let io_err = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe closed");
        let err = JsonReportError::Io(io_err);
        let msg = err.to_string();
        assert!(msg.contains("I/O error"));
        assert!(msg.contains("pipe closed"));
    }

    #[test]
    fn io_error_converts_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let err: JsonReportError = io_err.into();
        assert!(matches!(err, JsonReportError::Io(_)));
    }

    #[test]
    fn serialize_error_displays() {
        // Create a serde_json error by parsing invalid JSON
        let serde_err = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        let err = JsonReportError::Serialize(serde_err);
        let msg = err.to_string();
        assert!(msg.contains("JSON serialization error"));
    }
}
