//! Canonical framed hashing for composite keys.
//!
//! Every composite hash in OxyMake (cache key, job spec hash, lockfile
//! hashes) is built from *framed fields*: each field is written as
//! `len(tag) ‖ tag ‖ presence ‖ len(value) ‖ value`. The length prefixes
//! make the encoding injective — bytes can never migrate across a field
//! boundary ("ab"+"c" vs "a"+"bc"), an absent optional field can never
//! collide with an empty or shifted one, and list elements can never be
//! re-split into a colliding concatenation.

use std::collections::BTreeMap;

use blake3::Hasher;

/// Feed a framed, tagged field into `hasher`.
///
/// Encoding: `len(tag) ‖ tag ‖ 0x01 ‖ len(value) ‖ value`, with lengths
/// as little-endian `u64`. The `0x01` presence byte keeps present fields
/// disjoint from absent ones (see [`update_absent_field`]).
pub fn update_field(hasher: &mut Hasher, tag: &str, value: &[u8]) {
    hasher.update(&(tag.len() as u64).to_le_bytes());
    hasher.update(tag.as_bytes());
    hasher.update(&[1u8]);
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value);
}

/// Feed an explicitly-absent tagged field into `hasher`.
///
/// Encoding: `len(tag) ‖ tag ‖ 0x00`. An absent field is distinct from a
/// present-but-empty one (`Some("")`), and from the field not being part
/// of the schema at all.
pub fn update_absent_field(hasher: &mut Hasher, tag: &str) {
    hasher.update(&(tag.len() as u64).to_le_bytes());
    hasher.update(tag.as_bytes());
    hasher.update(&[0u8]);
}

/// Feed an optional tagged field into `hasher` (presence-tagged).
pub fn update_opt_field(hasher: &mut Hasher, tag: &str, value: Option<&[u8]>) {
    match value {
        Some(v) => update_field(hasher, tag, v),
        None => update_absent_field(hasher, tag),
    }
}

/// Hash a string→string map as framed key/value pairs.
///
/// Pairs are iterated in `BTreeMap` (sorted) order; both key and value are
/// length-prefixed, so `{"ab": "c"}` and `{"a": "bc"}` hash differently.
/// Returns a 64-character lowercase BLAKE3 hex digest.
pub fn hash_kv_map(map: &BTreeMap<String, String>) -> String {
    let mut hasher = Hasher::new();
    hasher.update(&(map.len() as u64).to_le_bytes());
    for (k, v) in map {
        hasher.update(&(k.len() as u64).to_le_bytes());
        hasher.update(k.as_bytes());
        hasher.update(&(v.len() as u64).to_le_bytes());
        hasher.update(v.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(f: impl FnOnce(&mut Hasher)) -> String {
        let mut h = Hasher::new();
        f(&mut h);
        h.finalize().to_hex().to_string()
    }

    #[test]
    fn field_boundaries_are_framed() {
        let a = digest(|h| {
            update_field(h, "x", b"ab");
            update_field(h, "y", b"c");
        });
        let b = digest(|h| {
            update_field(h, "x", b"a");
            update_field(h, "y", b"bc");
        });
        assert_ne!(a, b);
    }

    #[test]
    fn absent_differs_from_empty() {
        let absent = digest(|h| update_opt_field(h, "x", None));
        let empty = digest(|h| update_opt_field(h, "x", Some(b"")));
        assert_ne!(absent, empty);
    }

    #[test]
    fn kv_map_boundaries_are_framed() {
        let mut a = BTreeMap::new();
        a.insert("ab".to_string(), "c".to_string());
        let mut b = BTreeMap::new();
        b.insert("a".to_string(), "bc".to_string());
        assert_ne!(hash_kv_map(&a), hash_kv_map(&b));
    }

    #[test]
    fn kv_map_pair_boundaries_are_framed() {
        // {"a": "b", "c": "d"} vs {"a": "bc", "": "d"}-style re-splits.
        let mut a = BTreeMap::new();
        a.insert("a".to_string(), "b".to_string());
        a.insert("c".to_string(), "d".to_string());
        let mut b = BTreeMap::new();
        b.insert("a".to_string(), "bc".to_string());
        b.insert(String::new(), "d".to_string());
        assert_ne!(hash_kv_map(&a), hash_kv_map(&b));
    }
}
