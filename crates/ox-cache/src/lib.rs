//! # ox-cache — Content-Addressable Cache
//!
//! This crate decides whether a job can be skipped by comparing BLAKE3
//! content hashes of inputs and outputs against previously recorded builds.
//!
//! ## Crate responsibilities
//!
//! - BLAKE3 hashing of file contents and command strings
//! - Cache key computation from the full set of job inputs
//! - Hit/miss determination: does a stored result match the current inputs?
//! - Cache storage and retrieval via a JSON manifest
//!
//! ## What this crate NEVER does
//!
//! - File copying or artifact management (delegates to Storage trait)
//! - Build scheduling or execution

pub mod error;
pub mod hash;
pub mod key;
pub mod lookup;
pub mod materialization;
pub mod strategy;

// Convenience re-exports.
pub use error::CacheError;
pub use hash::hash_file;
pub use key::{
    CACHE_KEY_FORMAT_VERSION, CacheKeySpec, compute_cache_key, current_platform,
    env_spec_content_hash,
};
pub use lookup::{CacheEntry, CacheHitStatus, CacheStore};
pub use materialization::{ComputationKeyStrategy, ContentAddressedStrategy, ExternalRefStrategy};
pub use strategy::CacheValidation;
