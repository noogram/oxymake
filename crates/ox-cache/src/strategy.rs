//! Cache validation strategies.
//!
//! Controls how output files are verified against cached entries.
//! See ADR-006 for the full design rationale.

use std::fmt;
use std::str::FromStr;

/// How output files are validated against cache entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CacheValidation {
    /// Check mtime + size only. O(1) stat calls per output.
    /// Fast but can miss same-size edits within the same second, and never
    /// verifies content: a corrupted output with a newer mtime is served as
    /// a hit. Matches Make/Snakemake behavior. **Opt-in only** — on a
    /// shared or multi-user cache, prefer `MtimeHash` or `ContentHash`.
    Mtime,

    /// Check mtime + size first; if they differ, compute BLAKE3 hash.
    /// Fast on unchanged files, correct on changed ones.
    ///
    /// This is the default: it closes the same-size-corruption hole of
    /// `Mtime` while costing ~zero on genuinely unchanged files
    /// (security premortem; amends ADR-006).
    #[default]
    MtimeHash,

    /// Always compute BLAKE3 hash, ignoring mtime.
    /// Guarantees bit-exact correctness.
    ContentHash,
}

impl fmt::Display for CacheValidation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mtime => write!(f, "mtime"),
            Self::MtimeHash => write!(f, "mtime+hash"),
            Self::ContentHash => write!(f, "hash"),
        }
    }
}

impl FromStr for CacheValidation {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "mtime" => Ok(Self::Mtime),
            "mtime+hash" | "mtime-hash" => Ok(Self::MtimeHash),
            "hash" | "content-hash" => Ok(Self::ContentHash),
            other => Err(format!(
                "unknown cache validation strategy '{other}': expected mtime, mtime+hash, or hash"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_mtime_hash() {
        // Security: the default must verify content when metadata changes.
        // Plain `Mtime` never hashes and is opt-in only (ADR-006 amendment).
        assert_eq!(CacheValidation::default(), CacheValidation::MtimeHash);
    }

    #[test]
    fn display_round_trips() {
        for strategy in [
            CacheValidation::Mtime,
            CacheValidation::MtimeHash,
            CacheValidation::ContentHash,
        ] {
            let s = strategy.to_string();
            let parsed: CacheValidation = s.parse().unwrap();
            assert_eq!(parsed, strategy);
        }
    }

    #[test]
    fn parse_aliases() {
        assert_eq!(
            "mtime-hash".parse::<CacheValidation>().unwrap(),
            CacheValidation::MtimeHash
        );
        assert_eq!(
            "content-hash".parse::<CacheValidation>().unwrap(),
            CacheValidation::ContentHash
        );
    }

    #[test]
    fn parse_invalid() {
        assert!("foo".parse::<CacheValidation>().is_err());
    }
}
