//! Google Cloud Storage remote cache backend.
//!
//! Stores artifacts in a GCS bucket. Requires application default
//! credentials or a service account key for authentication.
//!
//! ## Object layout
//!
//! ```text
//! gs://<bucket>/<prefix>/<first-2-hex>/<full-hash>
//! ```
//!
//! ## Design notes
//!
//! Like the S3 backend, this uses direct HTTP (JSON API) rather than a
//! full GCP SDK to keep the dependency tree small. This is a
//! **configuration struct and trait stub** — transport is not yet wired.

use std::path::Path;
use std::pin::Pin;

use ox_core::model::ContentHash;
use ox_core::traits::remote_cache::{RemoteCache, RemoteCacheError};

/// Configuration for a Google Cloud Storage remote cache backend.
#[derive(Debug, Clone)]
pub struct GcsCache {
    /// GCS bucket name.
    bucket: String,
    /// Object name prefix within the bucket.
    prefix: String,
}

impl GcsCache {
    /// Create a new GCS cache backend.
    ///
    /// `prefix` is prepended to all object names. Include a trailing `/`
    /// if you want a directory-like namespace.
    pub fn new(bucket: impl Into<String>, prefix: impl Into<String>) -> Self {
        Self {
            bucket: bucket.into(),
            prefix: prefix.into(),
        }
    }

    /// Return the GCS bucket name.
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// Return the object name prefix.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Compute the full GCS object name for a content hash.
    fn object_name(&self, key: &ContentHash) -> String {
        let hex = key.as_str();
        let prefix = &hex[..2.min(hex.len())];
        format!("{}{prefix}/{hex}", self.prefix)
    }
}

impl RemoteCache for GcsCache {
    fn fetch<'a>(
        &'a self,
        key: &'a ContentHash,
        _dest: &'a Path,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<bool, RemoteCacheError>> + Send + 'a>>
    {
        Box::pin(async move {
            let _object_name = self.object_name(key);
            // TODO: implement GCS JSON API download
            Err(RemoteCacheError::Unavailable(
                "GCS transport not yet implemented".into(),
            ))
        })
    }

    fn store<'a>(
        &'a self,
        key: &'a ContentHash,
        _source: &'a Path,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), RemoteCacheError>> + Send + 'a>> {
        Box::pin(async move {
            let _object_name = self.object_name(key);
            // TODO: implement GCS JSON API upload
            Err(RemoteCacheError::Unavailable(
                "GCS transport not yet implemented".into(),
            ))
        })
    }

    fn exists<'a>(
        &'a self,
        key: &'a ContentHash,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<bool, RemoteCacheError>> + Send + 'a>>
    {
        Box::pin(async move {
            let _object_name = self.object_name(key);
            // TODO: implement GCS JSON API HEAD/metadata check
            Err(RemoteCacheError::Unavailable(
                "GCS transport not yet implemented".into(),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_name_layout() {
        let cache = GcsCache::new("my-bucket", "oxymake/cache/");
        let key = ContentHash::from_hex(
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
        )
        .unwrap();
        let name = cache.object_name(&key);
        assert_eq!(
            name,
            "oxymake/cache/ab/abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
        );
    }

    #[test]
    fn object_name_with_empty_prefix() {
        let cache = GcsCache::new("bucket", "");
        let key = ContentHash::from_hex("ff".repeat(32)).unwrap();
        let name = cache.object_name(&key);
        assert!(name.starts_with("ff/"));
    }

    #[tokio::test]
    async fn fetch_returns_unavailable() {
        let cache = GcsCache::new("bucket", "prefix/");
        let key = ContentHash::from_hex("aabb".repeat(16)).unwrap();
        let result = cache.fetch(&key, Path::new("/tmp/nope")).await;
        assert!(matches!(result, Err(RemoteCacheError::Unavailable(_))));
    }
}
