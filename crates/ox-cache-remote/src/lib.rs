//! # ox-cache-remote — Remote Cache Backends
//!
//! Implements the [`RemoteCache`] trait from ox-core for sharing build
//! artifacts across machines. Provides three backends:
//!
//! - **Local directory** (`DirectoryCache`): Stores artifacts in a local
//!   directory tree, useful for testing and single-machine shared caches.
//! - **S3** (`S3Cache`): Amazon S3 and S3-compatible object stores.
//! - **GCS** (`GcsCache`): Google Cloud Storage.
//!
//! ## Crate responsibilities
//!
//! - Transport: upload/download artifacts to/from remote backends
//! - Key-to-path mapping: content hash → object key layout
//! - Retry and error classification for transient failures
//!
//! ## What this crate NEVER does
//!
//! - Cache key computation (that's ox-cache)
//! - Content hashing (that's ox-cache)
//! - Build scheduling or execution

pub mod directory;
pub mod gcs;
pub mod s3;

pub use directory::DirectoryCache;
pub use gcs::GcsCache;
pub use ox_core::traits::remote_cache::{RemoteCache, RemoteCacheError};
pub use s3::S3Cache;
