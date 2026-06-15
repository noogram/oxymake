//! # ox-storage-local — Local Filesystem Storage
//!
//! This crate implements the `Storage` trait from ox-core using the local
//! filesystem. It handles reading, writing, and checking existence of
//! build artifacts on disk.
//!
//! ## Crate responsibilities
//!
//! - Implement `Storage` for local file paths
//! - Atomic writes (write-to-temp then rename) to prevent partial artifacts
//! - Directory creation and cleanup
//! - File metadata queries (mtime, size, existence)
//!
//! ## What this crate NEVER does
//!
//! - Remote or cloud storage
//! - Content hashing (that's ox-cache)
//! - Cache invalidation logic

pub mod error;
pub mod local;

#[cfg(test)]
mod tests {
    #[test]
    fn crate_compiles() {
        // Verify that the crate's module structure is valid.
        // Actual unit tests will be added when Storage trait is implemented.
    }
}
