//! Error types for the state persistence layer.
//!
//! [`StateError`] is the single error type for all `ox-state` operations.
//! It covers database errors (from `rusqlite`), migration failures, and
//! I/O errors encountered when opening or creating the database file.

/// Errors that can occur during state persistence operations.
///
/// ```
/// use ox_state::error::StateError;
///
/// let err = StateError::Migration {
///     from: 0,
///     to: 1,
///     reason: "table already exists".into(),
/// };
/// assert!(format!("{err}").contains("migration failed"));
/// ```
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    /// A `rusqlite` error occurred during a database operation.
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),

    /// The state database file is corrupt (SQLITE_CORRUPT / SQLITE_NOTADB).
    ///
    /// state.db is a regenerable cache, not the source of truth — the
    /// message must teach the user the escape hatch instead of leaving
    /// them with a raw SQLite error. WAL on NFS (the announced HPC
    /// cluster target) is a known corruption vector.
    #[error(
        "state database at {path} is corrupt — it is a regenerable cache, \
         run `ox clean --state` to start fresh (underlying error: {source})"
    )]
    Corrupt {
        /// Path of the corrupt database file.
        path: String,
        /// The underlying SQLite error that revealed the corruption.
        source: rusqlite::Error,
    },

    /// A schema migration failed.
    #[error("migration failed from v{from} to v{to}: {reason}")]
    Migration {
        /// Schema version before the failed migration.
        from: u32,
        /// Target schema version.
        to: u32,
        /// Human-readable reason for the failure.
        reason: String,
    },

    /// An I/O error occurred (e.g., creating the database file).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
