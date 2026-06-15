//! # Storage Trait
//!
//! Defines the plugin interface for file storage backends. Determines
//! where output files live and how they are accessed.
//!
//! Built-in: `LocalStorage` (ox-storage-local)
//! Phase 2+: S3, GCS, SSH

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::model::ContentHash;

/// Metadata about a file in storage.
#[derive(Debug, Clone)]
pub struct FileMeta {
    /// File size in bytes.
    pub size: u64,
    /// Last modification time.
    pub mtime: SystemTime,
    /// Whether this is a regular file (vs directory, symlink).
    pub is_file: bool,
}

/// The storage trait — plugin interface for file backends.
///
/// Storage backends handle file existence checks, content hashing,
/// staging, and directory listing. The scheduler and cache use this
/// trait to interact with files without knowing where they live.
pub trait Storage: Send + Sync {
    /// Error type specific to this storage backend.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Check if a path exists and get its metadata.
    fn stat(
        &self,
        path: &Path,
    ) -> impl Future<Output = Result<Option<FileMeta>, Self::Error>> + Send;

    /// Compute content hash (blake3) of a file.
    fn content_hash(
        &self,
        path: &Path,
    ) -> impl Future<Output = Result<ContentHash, Self::Error>> + Send;

    /// List directory contents (for wildcard expansion via `glob()`).
    fn list_dir(
        &self,
        path: &Path,
    ) -> impl Future<Output = Result<Vec<PathBuf>, Self::Error>> + Send;

    /// Stage input files to a location accessible by the executor.
    ///
    /// For local storage, this is a no-op (files are already local).
    /// For remote storage (S3), this downloads files to a staging area.
    fn stage_in(
        &self,
        paths: &[PathBuf],
        target_dir: &Path,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Collect output files from the executor's workspace.
    ///
    /// For local storage, this may be a rename/move.
    /// For remote storage, this uploads files.
    fn stage_out(
        &self,
        paths: &[PathBuf],
        source_dir: &Path,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Delete a file (used for invalidation and partial output cleanup).
    fn delete(&self, path: &Path) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
