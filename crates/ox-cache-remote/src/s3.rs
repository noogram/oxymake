//! S3 remote cache backend.
//!
//! Stores artifacts in an Amazon S3 bucket (or S3-compatible service like
//! MinIO, R2, etc.). Requires the `AWS_*` environment variables or a
//! credentials file for authentication.
//!
//! ## Object layout
//!
//! ```text
//! s3://<bucket>/<prefix>/<first-2-hex>/<full-hash>
//! ```
//!
//! ## Design notes
//!
//! This backend uses HTTP requests directly (no AWS SDK dependency) to
//! keep the dependency tree small. A future version may optionally use
//! the AWS SDK for advanced features (STS, IAM roles).
//!
//! For now, this is a **configuration struct and trait stub** — the actual
//! HTTP transport is gated behind runtime availability of credentials.

use std::path::Path;
use std::pin::Pin;

use ox_core::model::ContentHash;
use ox_core::traits::remote_cache::{RemoteCache, RemoteCacheError};

/// Configuration for an S3 remote cache backend.
#[derive(Debug, Clone)]
pub struct S3Cache {
    /// S3 bucket name.
    bucket: String,
    /// Key prefix within the bucket (e.g., `"oxymake/cache/"`).
    prefix: String,
    /// AWS region (e.g., `"us-east-1"`).
    region: String,
    /// Optional custom endpoint for S3-compatible services.
    endpoint: Option<String>,
}

impl S3Cache {
    /// Create a new S3 cache backend.
    ///
    /// `prefix` is prepended to all object keys. Include a trailing `/`
    /// if you want a directory-like namespace.
    pub fn new(
        bucket: impl Into<String>,
        prefix: impl Into<String>,
        region: impl Into<String>,
    ) -> Self {
        Self {
            bucket: bucket.into(),
            prefix: prefix.into(),
            region: region.into(),
            endpoint: None,
        }
    }

    /// Set a custom endpoint URL for S3-compatible services (MinIO, R2, etc.).
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    /// Return the S3 bucket name.
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// Return the key prefix.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Return the AWS region.
    pub fn region(&self) -> &str {
        &self.region
    }

    /// Return the custom endpoint, if set.
    pub fn endpoint(&self) -> Option<&str> {
        self.endpoint.as_deref()
    }

    /// Compute the full S3 object key for a content hash.
    fn object_key(&self, key: &ContentHash) -> String {
        let hex = key.as_str();
        let prefix = &hex[..2.min(hex.len())];
        format!("{}{prefix}/{hex}", self.prefix)
    }
}

impl RemoteCache for S3Cache {
    fn fetch<'a>(
        &'a self,
        key: &'a ContentHash,
        _dest: &'a Path,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<bool, RemoteCacheError>> + Send + 'a>>
    {
        Box::pin(async move {
            let _object_key = self.object_key(key);
            // TODO: implement S3 GetObject via HTTP
            // For now, signal that the object was not found so callers
            // fall through to local execution.
            Err(RemoteCacheError::Unavailable(
                "S3 transport not yet implemented".into(),
            ))
        })
    }

    fn store<'a>(
        &'a self,
        key: &'a ContentHash,
        _source: &'a Path,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), RemoteCacheError>> + Send + 'a>> {
        Box::pin(async move {
            let _object_key = self.object_key(key);
            // TODO: implement S3 PutObject via HTTP
            Err(RemoteCacheError::Unavailable(
                "S3 transport not yet implemented".into(),
            ))
        })
    }

    fn exists<'a>(
        &'a self,
        key: &'a ContentHash,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<bool, RemoteCacheError>> + Send + 'a>>
    {
        Box::pin(async move {
            let _object_key = self.object_key(key);
            // TODO: implement S3 HeadObject via HTTP
            Err(RemoteCacheError::Unavailable(
                "S3 transport not yet implemented".into(),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_key_layout() {
        let cache = S3Cache::new("my-bucket", "oxymake/cache/", "us-east-1");
        let key = ContentHash::from_hex(
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
        )
        .unwrap();
        let obj_key = cache.object_key(&key);
        assert_eq!(
            obj_key,
            "oxymake/cache/ab/abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
        );
    }

    #[test]
    fn object_key_with_empty_prefix() {
        let cache = S3Cache::new("bucket", "", "us-west-2");
        let key = ContentHash::from_hex("ff".repeat(32)).unwrap();
        let obj_key = cache.object_key(&key);
        assert!(obj_key.starts_with("ff/"));
    }

    #[test]
    fn builder_with_endpoint() {
        let cache =
            S3Cache::new("bucket", "prefix/", "us-east-1").with_endpoint("http://localhost:9000");
        assert_eq!(cache.endpoint(), Some("http://localhost:9000"));
    }

    #[tokio::test]
    async fn fetch_returns_unavailable() {
        let cache = S3Cache::new("bucket", "prefix/", "us-east-1");
        let key = ContentHash::from_hex("aabb".repeat(16)).unwrap();
        let result = cache.fetch(&key, Path::new("/tmp/nope")).await;
        assert!(matches!(result, Err(RemoteCacheError::Unavailable(_))));
    }
}
