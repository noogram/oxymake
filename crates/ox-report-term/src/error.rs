//! Error types for the terminal reporter.

/// Errors that can occur during terminal reporting.
///
/// These are non-fatal — the reporter should degrade gracefully rather
/// than crash the build when output fails.
#[derive(Debug, thiserror::Error)]
pub enum TermReportError {
    /// Failed to write to the terminal.
    #[error("terminal write error: {0}")]
    Io(#[from] std::io::Error),
}
