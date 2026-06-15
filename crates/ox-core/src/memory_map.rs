//! # Output Memory Map
//!
//! Concurrent map for in-memory output data, enabling Stage 2 in-memory
//! data transport between jobs on the critical path.
//!
//! Producers call `OutputMemoryMap::put` after execution to store their
//! output data. Consumers call `OutputMemoryMap::get` to retrieve it
//! without disk I/O. The scheduler manages insertion/eviction; the executor
//! performs lookups during `Executor::execute`.
//!
//! The map is keyed by the string representation of `OutputRef` (the same
//! key used by `MaterializationSet` in the scheduler state). Values are
//! `Arc<[u8]>` — a single heap allocation for the data (no inner `Vec`
//! header overhead, no double indirection).

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

/// Concurrent map from output keys to their in-memory data.
///
/// Thread-safe via `Arc<RwLock<HashMap>>`. Cloning is cheap (Arc bump).
///
/// ## Type choice: `Arc<[u8]>` over `Arc<Vec<u8>>`
///
/// `Arc<[u8]>` stores the length and data in a single allocation (fat
/// pointer), while `Arc<Vec<u8>>` adds an extra heap indirection through
/// the `Vec` header. Since we never resize after creation, the `Vec`
/// capacity/length bookkeeping is wasted. This also matches the
/// `memory_store` type in `Frontier` (ADR-011 Stage 1), eliminating copies when
/// draining data from the map into the scheduler.
///
/// # Key format
///
/// Keys are the string representation of `OutputRef`, matching the keys
/// used by `MaterializationSet` in the scheduler. For file outputs, this
/// is the file path string.
#[derive(Debug, Clone, Default)]
pub struct OutputMemoryMap {
    inner: Arc<RwLock<HashMap<String, Arc<[u8]>>>>,
}

impl OutputMemoryMap {
    /// Create a new empty memory map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Store output data in the memory map.
    ///
    /// Overwrites any existing entry for the same key.
    pub fn put(&self, key: String, data: Arc<[u8]>) {
        self.inner.write().insert(key, data);
    }

    /// Retrieve output data from the memory map.
    ///
    /// Returns `None` if the key is not present (output not in memory).
    pub fn get(&self, key: &str) -> Option<Arc<[u8]>> {
        self.inner.read().get(key).cloned()
    }

    /// Remove and return output data from the memory map.
    ///
    /// Used during eviction when `MaterializationSet::is_evictable()` is true.
    pub fn remove(&self, key: &str) -> Option<Arc<[u8]>> {
        self.inner.write().remove(key)
    }

    /// Number of entries currently in the map.
    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

    /// Whether the map is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.read().is_empty()
    }

    /// Total bytes of in-memory data (sum of all values).
    pub fn total_bytes(&self) -> usize {
        self.inner.read().values().map(|v| v.len()).sum()
    }

    /// Check if a key exists in the map without cloning the data.
    pub fn contains(&self, key: &str) -> bool {
        self.inner.read().contains_key(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_and_get() {
        let map = OutputMemoryMap::new();
        let data: Arc<[u8]> = Arc::from(vec![1u8, 2, 3, 4]);
        map.put("output/a.parquet".into(), data);

        let retrieved = map.get("output/a.parquet").unwrap();
        assert_eq!(&*retrieved, &[1, 2, 3, 4]);
    }

    #[test]
    fn get_missing_returns_none() {
        let map = OutputMemoryMap::new();
        assert!(map.get("nonexistent").is_none());
    }

    #[test]
    fn put_overwrites() {
        let map = OutputMemoryMap::new();
        map.put("key".into(), Arc::from(vec![1u8]));
        map.put("key".into(), Arc::from(vec![2u8, 3]));
        assert_eq!(&*map.get("key").unwrap(), &[2, 3]);
    }

    #[test]
    fn remove_returns_data() {
        let map = OutputMemoryMap::new();
        map.put("key".into(), Arc::from(vec![1u8, 2, 3]));
        let removed = map.remove("key").unwrap();
        assert_eq!(&*removed, &[1, 2, 3]);
        assert!(map.get("key").is_none());
    }

    #[test]
    fn remove_missing_returns_none() {
        let map = OutputMemoryMap::new();
        assert!(map.remove("nonexistent").is_none());
    }

    #[test]
    fn len_and_is_empty() {
        let map = OutputMemoryMap::new();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);

        map.put("a".into(), Arc::from(vec![1u8]));
        map.put("b".into(), Arc::from(vec![2u8]));
        assert_eq!(map.len(), 2);
        assert!(!map.is_empty());
    }

    #[test]
    fn total_bytes() {
        let map = OutputMemoryMap::new();
        map.put("a".into(), Arc::from(vec![1u8, 2, 3]));
        map.put("b".into(), Arc::from(vec![4u8, 5]));
        assert_eq!(map.total_bytes(), 5);
    }

    #[test]
    fn contains() {
        let map = OutputMemoryMap::new();
        map.put("present".into(), Arc::from(vec![1u8]));
        assert!(map.contains("present"));
        assert!(!map.contains("absent"));
    }

    #[test]
    fn clone_shares_data() {
        let map = OutputMemoryMap::new();
        map.put("key".into(), Arc::from(vec![1u8, 2, 3]));

        let cloned = map.clone();
        assert_eq!(&*cloned.get("key").unwrap(), &[1, 2, 3]);

        // Mutations through one handle are visible through the other.
        cloned.put("new_key".into(), Arc::from(vec![4u8, 5]));
        assert!(map.contains("new_key"));
    }

    #[test]
    fn default_is_empty() {
        let map = OutputMemoryMap::default();
        assert!(map.is_empty());
    }
}
