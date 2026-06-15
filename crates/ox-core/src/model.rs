//! Core data model for OxyMake.
//!
//! This module defines all the fundamental types used throughout the OxyMake
//! engine: rules, jobs, events, output references, and supporting enums.
//! These types are the shared vocabulary between the parser, resolver,
//! scheduler, executor, and reporter layers.
//!
//! # Design principles
//!
//! - **Newtypes for IDs**: [`RuleName`], [`JobId`], and [`GateId`] wrap `String`
//!   to prevent accidental mixing of identifiers.
//! - **Exhaustive enums**: All variants are explicit — no catch-all `Other(String)`.
//! - **Serialize everything**: Types that appear in `--json` output derive `Serialize`.
//! - **Clone is cheap**: [`JobId`] wraps `Arc<str>` so clones are O(1).

use ordered_float::OrderedFloat;
use serde::{Deserialize, Serialize};

use crate::error::InvalidHashError;
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

/// Default shell executable used for running commands.
///
/// Uses `/bin/bash` to support bash-specific features like process substitution
/// (`<()`), arrays, and other constructs that are not available in POSIX `/bin/sh`.
pub const DEFAULT_SHELL: &str = "/bin/bash";
use std::time::Duration;

// ---------------------------------------------------------------------------
// Newtype IDs
// ---------------------------------------------------------------------------

/// A rule name, uniquely identifying a rule within a workflow.
///
/// ```
/// use ox_core::model::RuleName;
///
/// let name = RuleName("align".into());
/// assert_eq!(name.as_str(), "align");
/// assert_eq!(format!("{name}"), "align");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuleName(pub String);

impl RuleName {
    /// Returns the rule name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RuleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for RuleName {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// A unique identifier for a concrete job instance.
///
/// ```
/// use ox_core::model::JobId;
///
/// let id = JobId("align-sample_A".into());
/// assert_eq!(format!("{id}"), "align-sample_A");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct JobId(pub Arc<str>);

impl JobId {
    /// Returns the job ID as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for JobId {
    fn from(s: &str) -> Self {
        Self(Arc::from(s))
    }
}

impl From<String> for JobId {
    fn from(s: String) -> Self {
        Self(Arc::from(s))
    }
}

/// A unique identifier for a gate (human-in-the-loop checkpoint).
///
/// ```
/// use ox_core::model::GateId;
///
/// let gate = GateId("review-checkpoint".into());
/// assert_eq!(gate.as_str(), "review-checkpoint");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GateId(pub String);

impl GateId {
    /// Returns the gate ID as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for GateId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for GateId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

/// How a rule's computation is expressed.
///
/// This enum captures the full spectrum from opaque shell commands to
/// pure function calls, enabling progressively richer optimizations.
///
/// ```
/// use ox_core::model::ExecutionBlock;
///
/// let block = ExecutionBlock::Shell { command: "echo hello".into() };
/// assert!(matches!(block, ExecutionBlock::Shell { .. }));
///
/// let call = ExecutionBlock::Call {
///     function: "my_module.transform".into(),
///     lang: "python".into(),
/// };
/// assert!(matches!(call, ExecutionBlock::Call { .. }));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutionBlock {
    /// An opaque shell command string, executed via the system shell.
    Shell {
        /// The shell command to execute.
        command: String,
    },
    /// Inline code in a specified language, executed as a script.
    Run {
        /// The inline code to execute.
        code: String,
        /// The language of the inline code (e.g., "python", "r").
        lang: String,
    },
    /// An external script file, optionally with a language hint.
    Script {
        /// Path to the script file.
        path: PathBuf,
        /// Optional language hint (inferred from extension if absent).
        lang: Option<String>,
    },
    /// A pure function call — the only mode that supports in-memory passing.
    Call {
        /// Fully qualified function name (e.g., "my_module.transform").
        function: String,
        /// The language runtime to use (e.g., "python", "r").
        lang: String,
    },
}

impl fmt::Display for ExecutionBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shell { command } => write!(f, "shell: {command}"),
            Self::Run { lang, .. } => write!(f, "run ({lang})"),
            Self::Script { path, lang } => {
                let lang_str = lang.as_deref().unwrap_or("auto");
                write!(f, "script: {} ({lang_str})", path.display())
            }
            Self::Call { function, lang } => write!(f, "call: {function} ({lang})"),
        }
    }
}

// ---------------------------------------------------------------------------
// Input / Output patterns
// ---------------------------------------------------------------------------

/// A target pattern string, possibly containing `{wildcards}`.
///
/// # Examples
///
/// ```
/// use ox_core::model::TargetPattern;
///
/// let pat = TargetPattern::from("data/{sample}.csv");
/// assert_eq!(pat.as_str(), "data/{sample}.csv");
/// assert_eq!(format!("{pat}"), "data/{sample}.csv");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TargetPattern(pub String);

impl TargetPattern {
    /// Returns the pattern as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TargetPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for TargetPattern {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<String> for TargetPattern {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl std::ops::Deref for TargetPattern {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl std::borrow::Borrow<str> for TargetPattern {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for TargetPattern {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl PartialEq<&str> for TargetPattern {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<str> for TargetPattern {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<String> for TargetPattern {
    fn eq(&self, other: &String) -> bool {
        self.0 == *other
    }
}

/// An input pattern for a rule, describing what files or resources it consumes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InputPattern {
    /// The pattern, which may contain `{wildcards}`.
    pub pattern: TargetPattern,
    /// Optional named argument (used in Call mode to map inputs to function parameters).
    pub name: Option<String>,
    /// Optional format hint (e.g., "parquet", "csv", "json", "arrow").
    pub format: Option<String>,
}

/// An output pattern for a rule, describing what it produces and how.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OutputPattern {
    /// The pattern, which may contain `{wildcards}`.
    pub pattern: TargetPattern,
    /// Optional named return (used in Call mode to map function returns to outputs).
    pub name: Option<String>,
    /// Optional serialization format (used in Call mode).
    pub format: Option<String>,
    /// Whether the output is permanent, temporary, or protected.
    pub lifecycle: OutputLifecycle,
    /// When to materialize the output to disk.
    pub materialize: MaterializePolicy,
}

/// Controls the lifecycle of an output file after a run completes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputLifecycle {
    /// The output persists indefinitely (default).
    #[default]
    Permanent,
    /// The output may be deleted after downstream consumers finish.
    Temporary,
    /// The output is kept but flagged for manual cleanup.
    Protected,
}

impl fmt::Display for OutputLifecycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Permanent => f.write_str("permanent"),
            Self::Temporary => f.write_str("temporary"),
            Self::Protected => f.write_str("protected"),
        }
    }
}

/// Controls when an output is materialized (written to disk).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterializePolicy {
    /// Always write to disk and cache (default).
    #[default]
    Always,
    /// Materialize only if a non-Call downstream consumer needs the file.
    Auto,
    /// Keep in memory only — lost if the process dies, not cached.
    Never,
    /// Materialize only if this output is a DAG leaf (final result).
    Final,
}

impl fmt::Display for MaterializePolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Always => f.write_str("always"),
            Self::Auto => f.write_str("auto"),
            Self::Never => f.write_str("never"),
            Self::Final => f.write_str("final"),
        }
    }
}

// ---------------------------------------------------------------------------
// Reproducibility and provenance (Stage 2: artifact metadata)
// ---------------------------------------------------------------------------

/// Classification of a job's output reproducibility.
///
/// This enum annotates each rule (and its concrete jobs) with the expected
/// reproducibility of its outputs.  The cache layer uses this to decide
/// whether a cached result can be safely reused:
///
/// - `Deterministic`: bit-for-bit identical outputs given the same inputs
///   and job spec — always safe to cache-reuse.
/// - `SeedDeterministic`: reproducible given a fixed random seed.  Cache
///   reuse is safe as long as the seed is part of the cache key.
/// - `Approximate`: outputs vary slightly between runs (e.g., floating
///   point non-determinism) but are "close enough" for the domain.
///   Cache reuse is safe for downstream consumers that tolerate variance.
/// - `NonReproducible`: outputs differ meaningfully each run (e.g., API
///   calls, timestamps).  Cache reuse should be avoided.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReproducibilityClass {
    /// Bit-for-bit identical outputs given the same inputs.
    #[default]
    Deterministic,
    /// Reproducible given a fixed random seed (seed must be in cache key).
    SeedDeterministic,
    /// Outputs vary slightly but are acceptable for the domain.
    Approximate,
    /// Outputs differ each run — cache reuse is unsafe.
    NonReproducible,
}

impl fmt::Display for ReproducibilityClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Deterministic => f.write_str("deterministic"),
            Self::SeedDeterministic => f.write_str("seed_deterministic"),
            Self::Approximate => f.write_str("approximate"),
            Self::NonReproducible => f.write_str("non_reproducible"),
        }
    }
}

/// Provenance record for a cached artifact.
///
/// Stored alongside cache entries (in the `ox-cache` SQLite database) to
/// enable correct invalidation without recomputation.  If the stored
/// `input_hashes` and `job_spec_hash` match the current values, the
/// cached output is valid.
///
/// # Fields
///
/// - `input_hashes`: content hashes of each input file, paired with their
///   path for diagnostics.  Ordered by path for deterministic comparison.
/// - `job_spec_hash`: a single hash covering the job's command, parameters,
///   environment, and other spec-level inputs.
/// - `reproducibility`: the [`ReproducibilityClass`] declared on the rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactProvenance {
    /// Content hashes of input files: `(hex_hash, path)`.
    pub input_hashes: Vec<(String, String)>,
    /// Hash of the job specification (command + params + env).
    pub job_spec_hash: String,
    /// Reproducibility classification from the rule.
    pub reproducibility: ReproducibilityClass,
}

// ---------------------------------------------------------------------------
// Artifact metadata (Stage 2: inline per-output identity)
// ---------------------------------------------------------------------------

/// Compact, inline metadata for a single artifact (output file).
///
/// Exactly 40 bytes on the stack: a 32-byte BLAKE3 content hash plus an 8-byte
/// size. This struct is stored inside [`MaterializationSet`] so the scheduler
/// knows each output's identity without heap allocation or database lookups.
///
/// # Layout (40 bytes, no padding)
///
/// ```text
/// ┌──────────────────────────────────┬──────────┐
/// │  content_hash: [u8; 32]          │ size: u64│
/// └──────────────────────────────────┴──────────┘
///  0                                32         40
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct ArtifactMeta {
    /// BLAKE3 content hash of the artifact (raw 32 bytes).
    pub content_hash: [u8; 32],
    /// Size of the artifact in bytes.
    pub size_bytes: u64,
}

impl ArtifactMeta {
    /// Create from raw hash bytes and size.
    pub fn new(content_hash: [u8; 32], size_bytes: u64) -> Self {
        Self {
            content_hash,
            size_bytes,
        }
    }

    /// Create from a hex-encoded hash string and size.
    ///
    /// Returns `None` if the hex string is not exactly 64 characters or contains
    /// invalid hex digits.
    pub fn from_hex(hex: &str, size_bytes: u64) -> Option<Self> {
        if hex.len() != 64 {
            return None;
        }
        let mut bytes = [0u8; 32];
        for (i, chunk) in hex.as_bytes().chunks_exact(2).enumerate() {
            let hi = hex_digit(chunk[0])?;
            let lo = hex_digit(chunk[1])?;
            bytes[i] = (hi << 4) | lo;
        }
        Some(Self {
            content_hash: bytes,
            size_bytes,
        })
    }

    /// Convert to a [`ContentHash`] (heap-allocated hex string).
    pub fn to_content_hash(&self) -> ContentHash {
        ContentHash(self.hex())
    }

    /// Return the content hash as a 64-character lowercase hex string.
    pub fn hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for byte in &self.content_hash {
            use fmt::Write;
            let _ = write!(s, "{byte:02x}");
        }
        s
    }
}

impl fmt::Display for ArtifactMeta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.hex(), self.size_bytes)
    }
}

/// Decode a single ASCII hex digit to its numeric value.
fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Materialization tracking (Stage 2: in-memory data transport)
// ---------------------------------------------------------------------------

/// A physical location where an output's data currently resides.
///
/// Outputs in the DAG are logical identifiers (see [`OutputRef`]). At runtime,
/// the same logical output may be materialized in multiple physical locations
/// simultaneously — for example, held in process memory for fast access while
/// being asynchronously flushed to disk for durability.
///
/// This enum represents a single such materialization. A per-output
/// [`MaterializationSet`] tracks all active materializations and supports
/// firing-mode selection (cheapest read) and eviction guards.
///
/// # Petri net interpretation
///
/// In the Colored Petri Net view of the scheduler, each `Materialization` is a
/// **colored token** at the output's place. The token color encodes the physical
/// location and its access cost. When a downstream job (transition) fires, the
/// scheduler selects the cheapest available color (firing mode selection).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Materialization {
    /// Data held in process memory. Fastest access (~0.1 ms).
    InMemory {
        /// Whether this materialization is pinned (immune to eviction).
        pinned: bool,
    },
    /// Data written to local or shared filesystem (~200 ms access).
    OnDisk {
        /// The filesystem path where the data resides.
        path: PathBuf,
        /// Whether the on-disk copy has been verified (checksum match).
        verified: bool,
    },
    /// Data stored in a distributed object store (Ray Plasma, S3, etc.) (~1 ms access).
    ObjectStore {
        /// Backend-specific reference (e.g., Ray ObjectRef hex, S3 URI).
        ref_id: String,
        /// Optional node locality hint for distributed scheduling.
        node: Option<String>,
    },
}

impl Materialization {
    /// Estimated read latency in microseconds, used for firing-mode selection.
    ///
    /// The scheduler picks the materialization with the lowest cost when a
    /// downstream job needs to read this output. Ordering: memory < object
    /// store < disk.
    pub fn cost_us(&self) -> u64 {
        match self {
            Self::InMemory { .. } => 100,      // ~0.1 ms
            Self::ObjectStore { .. } => 1_000, // ~1 ms
            Self::OnDisk { .. } => 200_000,    // ~200 ms
        }
    }
}

impl fmt::Display for Materialization {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InMemory { pinned } => {
                if *pinned {
                    f.write_str("memory(pinned)")
                } else {
                    f.write_str("memory")
                }
            }
            Self::OnDisk { path, verified } => {
                write!(f, "disk:{}", path.display())?;
                if *verified {
                    f.write_str("(verified)")?;
                }
                Ok(())
            }
            Self::ObjectStore { ref_id, node } => {
                write!(f, "objstore:{ref_id}")?;
                if let Some(n) = node {
                    write!(f, "@{n}")?;
                }
                Ok(())
            }
        }
    }
}

/// Tracks all active materializations for a single output, plus a reference
/// count of pending consumers.
///
/// This replaces the bare `DashMap<PathBuf, Arc<Bytes>>` proposed in the
/// original Stage 2 design with a richer structure that supports:
///
/// - **Multiple simultaneous materializations** (memory + disk + object store)
/// - **Reference counting** for eviction eligibility
/// - **Firing-mode selection** (cheapest available materialization)
/// - **Eviction guards** (last materialization survives until all consumers fire)
///
/// # Invariant
///
/// `pending_consumers` counts downstream jobs that have not yet fired. When it
/// reaches zero, the output is eligible for eviction. The eviction guard ensures
/// that at least one materialization survives while `pending_consumers > 0`.
#[derive(Debug, Clone)]
pub struct MaterializationSet {
    /// The logical output this set tracks.
    pub output_ref: OutputRef,
    /// Active materializations, ordered by insertion time.
    materializations: Vec<Materialization>,
    /// Number of downstream consumers that have not yet fired (read this output).
    pending_consumers: usize,
    /// Size in bytes of the in-memory materialization (if any).
    /// Used by the memory budget to decide eviction priority (largest-first).
    size_bytes: u64,
    /// Compact identity metadata (content hash + size) for the artifact.
    /// Set when the job completes and the output is hashed.
    artifact_meta: Option<ArtifactMeta>,
}

impl MaterializationSet {
    /// Create a new empty set for the given output with the specified consumer count.
    pub fn new(output_ref: OutputRef, pending_consumers: usize) -> Self {
        Self {
            output_ref,
            materializations: Vec::new(),
            pending_consumers,
            size_bytes: 0,
            artifact_meta: None,
        }
    }

    /// Add a materialization. Duplicates (by variant) are replaced.
    pub fn add(&mut self, mat: Materialization) {
        // Replace existing materialization of the same variant.
        self.materializations
            .retain(|m| std::mem::discriminant(m) != std::mem::discriminant(&mat));
        self.materializations.push(mat);
    }

    /// Remove a materialization by variant discriminant.
    ///
    /// Returns `false` (and does nothing) if this is the last materialization
    /// and `pending_consumers > 0` — the eviction guard prevents removal.
    pub fn try_remove(&mut self, mat: &Materialization) -> bool {
        if self.materializations.len() <= 1 && self.pending_consumers > 0 {
            return false; // eviction guard
        }
        let before = self.materializations.len();
        self.materializations
            .retain(|m| std::mem::discriminant(m) != std::mem::discriminant(mat));
        self.materializations.len() < before
    }

    /// Record that one consumer has fired (read this output).
    ///
    /// Returns the new pending count.
    pub fn consumer_fired(&mut self) -> usize {
        self.pending_consumers = self.pending_consumers.saturating_sub(1);
        self.pending_consumers
    }

    /// Select the cheapest available materialization (firing-mode selection).
    ///
    /// Returns `None` if no materializations exist (output not yet produced).
    pub fn cheapest(&self) -> Option<&Materialization> {
        self.materializations.iter().min_by_key(|m| m.cost_us())
    }

    /// Whether this output has any active materializations.
    pub fn is_available(&self) -> bool {
        !self.materializations.is_empty()
    }

    /// Whether eviction is safe (no pending consumers).
    pub fn is_evictable(&self) -> bool {
        self.pending_consumers == 0
    }

    /// Number of pending consumers.
    pub fn pending_consumers(&self) -> usize {
        self.pending_consumers
    }

    /// Iterate over all active materializations.
    pub fn iter(&self) -> impl Iterator<Item = &Materialization> {
        self.materializations.iter()
    }

    /// Number of active materializations.
    pub fn len(&self) -> usize {
        self.materializations.len()
    }

    /// Whether the set has no materializations.
    pub fn is_empty(&self) -> bool {
        self.materializations.is_empty()
    }

    /// Set the size in bytes of the in-memory data for this output.
    ///
    /// Prefer [`set_artifact_meta`] when a BLAKE3 hash is available — it
    /// sets `size_bytes` atomically with the content hash, preventing
    /// divergence. This method exists for the `register_in_memory` path
    /// where only the size is known (no hash computed yet).
    pub(crate) fn set_size_bytes(&mut self, size: u64) {
        self.size_bytes = size;
    }

    /// Get the recorded size in bytes.
    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    /// Set the artifact identity metadata (content hash + size).
    ///
    /// Also synchronizes `size_bytes` from the meta, so that
    /// `enforce_memory_budget` uses the authoritative size from the
    /// content hash computation rather than a potentially stale value
    /// from `std::fs::metadata`.
    pub fn set_artifact_meta(&mut self, meta: ArtifactMeta) {
        self.size_bytes = meta.size_bytes;
        self.artifact_meta = Some(meta);
    }

    /// Get the artifact identity metadata, if set.
    pub fn artifact_meta(&self) -> Option<&ArtifactMeta> {
        self.artifact_meta.as_ref()
    }

    /// Whether this set contains an `InMemory` materialization.
    pub fn has_in_memory(&self) -> bool {
        self.materializations
            .iter()
            .any(|m| matches!(m, Materialization::InMemory { .. }))
    }

    /// Whether this set has a disk-based fallback (OnDisk or ObjectStore).
    ///
    /// Used by the eviction policy to avoid destroying the only copy of
    /// data for outputs with no disk persistence (e.g., `Never` policy).
    pub fn has_disk_fallback(&self) -> bool {
        self.materializations.iter().any(|m| {
            matches!(
                m,
                Materialization::OnDisk { .. } | Materialization::ObjectStore { .. }
            )
        })
    }

    /// Try to evict the `InMemory` materialization.
    ///
    /// Returns `true` if an in-memory materialization was removed, `false` if
    /// none existed or the eviction guard prevented removal (last
    /// materialization with pending consumers).
    pub fn evict_in_memory(&mut self) -> bool {
        let sentinel = Materialization::InMemory { pinned: false };
        // Pinned materializations are immune to eviction.
        if self
            .materializations
            .iter()
            .any(|m| matches!(m, Materialization::InMemory { pinned: true }))
        {
            return false;
        }
        self.try_remove(&sentinel)
    }
}

impl fmt::Display for MaterializationSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}[", self.output_ref)?;
        for (i, m) in self.materializations.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{m}")?;
        }
        write!(f, "](consumers={})", self.pending_consumers)
    }
}

// ---------------------------------------------------------------------------
// Output references (resolved)
// ---------------------------------------------------------------------------

/// A resolved output reference — the concrete form of an [`OutputPattern`]
/// after wildcard resolution.
///
/// ```
/// use ox_core::model::OutputRef;
/// use std::path::PathBuf;
///
/// let file_ref = OutputRef::File(PathBuf::from("results/sample_A.csv"));
/// assert!(matches!(file_ref, OutputRef::File(_)));
///
/// let virtual_ref = OutputRef::Virtual {
///     id: "db://results.sample_A".into(),
///     check: "SELECT count(*) FROM results WHERE sample='A'".into(),
/// };
/// assert!(matches!(virtual_ref, OutputRef::Virtual { .. }));
///
/// let mem_ref = OutputRef::InMemory { type_hint: Some("DataFrame".into()) };
/// assert!(matches!(mem_ref, OutputRef::InMemory { .. }));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputRef {
    /// A concrete file path on disk.
    File(PathBuf),
    /// An external resource (database table, API endpoint, etc.) with a
    /// validation check command.
    Virtual {
        /// Identifier for the external resource.
        id: String,
        /// Command or query to verify the resource exists.
        check: String,
    },
    /// An in-process object passed between Call-mode jobs without disk I/O.
    InMemory {
        /// Optional type hint (e.g., "DataFrame", "ndarray").
        type_hint: Option<String>,
    },
}

impl fmt::Display for OutputRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(path) => write!(f, "file:{}", path.display()),
            Self::Virtual { id, .. } => write!(f, "virtual:{id}"),
            Self::InMemory { type_hint } => {
                let hint = type_hint.as_deref().unwrap_or("any");
                write!(f, "memory:{hint}")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Expand mode
// ---------------------------------------------------------------------------

/// How wildcard lists are combined when expanding a rule into concrete jobs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpandMode {
    /// Cartesian product of all wildcard lists (default).
    #[default]
    Product,
    /// Parallel zip — lists must have equal length.
    Zip,
}

impl fmt::Display for ExpandMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Product => f.write_str("product"),
            Self::Zip => f.write_str("zip"),
        }
    }
}

// ---------------------------------------------------------------------------
// Error handling
// ---------------------------------------------------------------------------

/// Backoff strategy for retrying failed jobs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backoff {
    /// Fixed delay between retries.
    Constant,
    /// Delay increases linearly with each retry.
    Linear,
    /// Delay doubles with each retry.
    #[default]
    Exponential,
}

impl fmt::Display for Backoff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Constant => f.write_str("constant"),
            Self::Linear => f.write_str("linear"),
            Self::Exponential => f.write_str("exponential"),
        }
    }
}

/// Per-rule error handling strategy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "strategy", rename_all = "snake_case")]
pub enum ErrorStrategy {
    /// Kill everything on failure (default).
    #[default]
    Terminate,
    /// Retry the job up to `count` times with the given backoff.
    Retry {
        /// Maximum number of retry attempts.
        count: u32,
        /// Backoff strategy between retries.
        backoff: Backoff,
    },
    /// Failure is not fatal — continue as if the job succeeded.
    Ignore,
    /// Let currently running jobs complete, but do not start new ones.
    Finish,
}

impl fmt::Display for ErrorStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Terminate => f.write_str("terminate"),
            Self::Retry { count, backoff } => {
                write!(f, "retry({count}, {backoff})")
            }
            Self::Ignore => f.write_str("ignore"),
            Self::Finish => f.write_str("finish"),
        }
    }
}

// ---------------------------------------------------------------------------
// Guard expressions (when clauses)
// ---------------------------------------------------------------------------

/// A conditional guard expression for rules — determines whether a rule
/// applies to a given set of wildcard values.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum GuardExpr {
    /// Wildcard value must be in the given set.
    In {
        /// The wildcard name to check.
        field: String,
        /// The allowed values.
        values: Vec<String>,
    },
    /// Wildcard value must NOT be in the given set.
    NotIn {
        /// The wildcard name to check.
        field: String,
        /// The disallowed values.
        values: Vec<String>,
    },
    /// Wildcard value must equal the given string.
    Eq {
        /// The wildcard name to check.
        field: String,
        /// The expected value.
        value: String,
    },
    /// Wildcard value must NOT equal the given string.
    NotEq {
        /// The wildcard name to check.
        field: String,
        /// The disallowed value.
        value: String,
    },
    /// Wildcard value must match the given regex pattern.
    Regex {
        /// The wildcard name to check.
        field: String,
        /// The regex pattern to match against.
        pattern: String,
    },
    /// Config key must equal the given value.
    ConfigEq {
        /// The config key to look up.
        key: String,
        /// The expected value.
        value: String,
    },
    /// Environment variable must be set (non-empty).
    EnvSet {
        /// The environment variable name.
        var: String,
    },
    /// Environment variable must equal the given value.
    EnvEq {
        /// The environment variable name.
        var: String,
        /// The expected value.
        value: String,
    },
    /// File must exist on disk.
    FileExists {
        /// The file path to check.
        path: String,
    },
    /// All sub-expressions must be true.
    And {
        /// The sub-expressions.
        conditions: Vec<GuardExpr>,
    },
    /// At least one sub-expression must be true.
    Or {
        /// The sub-expressions.
        conditions: Vec<GuardExpr>,
    },
    /// Negate a sub-expression.
    Not {
        /// The sub-expression to negate.
        condition: Box<GuardExpr>,
    },
}

impl fmt::Display for GuardExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::In { field, values } => write!(f, "{field} in {values:?}"),
            Self::NotIn { field, values } => write!(f, "{field} not in {values:?}"),
            Self::Eq { field, value } => write!(f, "{field} == {value:?}"),
            Self::NotEq { field, value } => write!(f, "{field} != {value:?}"),
            Self::Regex { field, pattern } => write!(f, "{field} =~ /{pattern}/"),
            Self::ConfigEq { key, value } => write!(f, "config[{key:?}] == {value:?}"),
            Self::EnvSet { var } => write!(f, "env({var:?}) is set"),
            Self::EnvEq { var, value } => write!(f, "env({var:?}) == {value:?}"),
            Self::FileExists { path } => write!(f, "file_exists({path:?})"),
            Self::And { conditions } => {
                let parts: Vec<String> = conditions.iter().map(|c| c.to_string()).collect();
                write!(f, "({})", parts.join(" && "))
            }
            Self::Or { conditions } => {
                let parts: Vec<String> = conditions.iter().map(|c| c.to_string()).collect();
                write!(f, "({})", parts.join(" || "))
            }
            Self::Not { condition } => write!(f, "!({condition})"),
        }
    }
}

// ---------------------------------------------------------------------------
// Resource values
// ---------------------------------------------------------------------------

/// A flexible resource specification value, supporting integers, floats, and strings.
///
/// Uses [`OrderedFloat`] for the floating-point variant so that `Eq` and `Hash`
/// are sound (raw `f64` cannot implement `Eq` because `NaN != NaN`).
///
/// # Normalization invariant
///
/// Deserialization normalizes integral floats to [`ResourceValue::Int`]:
/// `gpu = 1.0` and `gpu = 1` are semantically the same resource demand, and
/// letting them land in different variants made the job spec-hash unstable
/// across a serialize/deserialize cycle (a pipeline already cached would
/// re-run from scratch). After deserialization, `Float` only ever holds
/// non-integral values (or magnitudes beyond 2⁵³, where `f64` can no longer
/// represent every integer exactly).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(untagged)]
pub enum ResourceValue {
    /// An integer resource value (e.g., cpu = 4).
    Int(i64),
    /// A floating-point resource value (e.g., gpu = 0.5).
    Float(OrderedFloat<f64>),
    /// A string resource value (e.g., mem = "128G").
    Str(String),
}

impl<'de> Deserialize<'de> for ResourceValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ResourceValueVisitor;

        /// Largest magnitude at which `f64` still represents every integer
        /// exactly (2⁵³). Beyond it, float→int conversion would be lossy.
        const EXACT_INT_BOUND: f64 = 9_007_199_254_740_992.0;

        impl serde::de::Visitor<'_> for ResourceValueVisitor {
            type Value = ResourceValue;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an integer, a float, or a string")
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(ResourceValue::Int(v))
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                match i64::try_from(v) {
                    Ok(n) => Ok(ResourceValue::Int(n)),
                    Err(_) => Ok(ResourceValue::Float(OrderedFloat(v as f64))),
                }
            }

            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
                if v.is_finite() && v.fract() == 0.0 && v.abs() <= EXACT_INT_BOUND {
                    Ok(ResourceValue::Int(v as i64))
                } else {
                    Ok(ResourceValue::Float(OrderedFloat(v)))
                }
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(ResourceValue::Str(v.to_owned()))
            }

            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(ResourceValue::Str(v))
            }
        }

        deserializer.deserialize_any(ResourceValueVisitor)
    }
}

impl ResourceValue {
    /// Convert to `u64` for resource budget arithmetic.
    ///
    /// - `Int(n)` → `n as u64` (negative values clamp to 0).
    /// - `Float(f)` → `f.ceil() as u64` (rounds up; negative clamps to 0).
    /// - `Str(s)` → attempts to parse a byte-suffix string like `"16G"`,
    ///   `"512M"`, `"1T"`. Returns `None` if the string is unparseable.
    ///
    /// ```
    /// use ox_core::model::ResourceValue;
    /// use ordered_float::OrderedFloat;
    ///
    /// assert_eq!(ResourceValue::Int(4).as_u64(), Some(4));
    /// assert_eq!(ResourceValue::Int(-1).as_u64(), Some(0));
    /// assert_eq!(ResourceValue::Float(OrderedFloat(2.5)).as_u64(), Some(3));
    /// assert_eq!(ResourceValue::Str("16G".into()).as_u64(), Some(16_000_000_000));
    /// assert_eq!(ResourceValue::Str("512M".into()).as_u64(), Some(512_000_000));
    /// assert_eq!(ResourceValue::Str("1T".into()).as_u64(), Some(1_000_000_000_000));
    /// assert_eq!(ResourceValue::Str("1024".into()).as_u64(), Some(1024));
    /// assert_eq!(ResourceValue::Str("hello".into()).as_u64(), None);
    /// ```
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Int(v) => Some((*v).max(0) as u64),
            Self::Float(v) => {
                let f = v.into_inner();
                if f < 0.0 {
                    Some(0)
                } else {
                    Some(f.ceil() as u64)
                }
            }
            Self::Str(s) => parse_byte_suffix(s),
        }
    }
}

/// Parse a string with an optional SI byte suffix (K, M, G, T) into a `u64`.
///
/// Accepts forms like `"16G"`, `"512M"`, `"1024"` (no suffix = raw number).
/// Uses SI (base-10) multipliers to match the thesis convention.
fn parse_byte_suffix(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (num_part, multiplier) = match s.as_bytes().last()? {
        b'K' | b'k' => (&s[..s.len() - 1], 1_000u64),
        b'M' | b'm' => (&s[..s.len() - 1], 1_000_000u64),
        b'G' | b'g' => (&s[..s.len() - 1], 1_000_000_000u64),
        b'T' | b't' => (&s[..s.len() - 1], 1_000_000_000_000u64),
        _ => (s, 1u64),
    };
    let n: u64 = num_part.trim().parse().ok()?;
    Some(n.saturating_mul(multiplier))
}

impl fmt::Display for ResourceValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(v) => write!(f, "{v}"),
            Self::Float(v) => write!(f, "{v}"),
            Self::Str(v) => write!(f, "{v}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Environment specification
// ---------------------------------------------------------------------------

/// The environment in which a rule's execution block runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EnvSpec {
    /// System environment (no isolation).
    System,
    /// A uv-managed Python virtual environment.
    Uv {
        /// Optional requirements file or inline dependencies.
        requirements: Option<String>,
    },
    /// A Conda environment.
    Conda {
        /// Path to the environment YAML or environment name.
        env: String,
    },
    /// A Docker container.
    Docker {
        /// Docker image reference (e.g., "python:3.12-slim").
        image: String,
    },
    /// A Nix shell environment.
    Nix {
        /// Path to the Nix expression or flake reference.
        expr: String,
    },
    /// An Apptainer (Singularity) container.
    Apptainer {
        /// Path to the SIF image or library reference.
        image: String,
    },
}

impl fmt::Display for EnvSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::System => f.write_str("system"),
            Self::Uv { .. } => f.write_str("uv"),
            Self::Conda { env } => write!(f, "conda:{env}"),
            Self::Docker { image } => write!(f, "docker:{image}"),
            Self::Nix { expr } => write!(f, "nix:{expr}"),
            Self::Apptainer { image } => write!(f, "apptainer:{image}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Rule metadata
// ---------------------------------------------------------------------------

/// Metadata about a rule that does not affect execution.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct RuleMeta {
    /// Human-readable description of what this rule does.
    pub description: Option<String>,
}

impl fmt::Display for RuleMeta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.description {
            Some(desc) => f.write_str(desc),
            None => f.write_str("(no description)"),
        }
    }
}

// ---------------------------------------------------------------------------
// Log configuration
// ---------------------------------------------------------------------------

/// Configuration for how a rule's stdout/stderr are captured.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LogConfig {
    /// Path pattern for stdout capture (may contain `{wildcards}`).
    pub stdout: Option<String>,
    /// Path pattern for stderr capture (may contain `{wildcards}`).
    pub stderr: Option<String>,
}

// Default is derived: both fields are None.

// ---------------------------------------------------------------------------
// Rule
// ---------------------------------------------------------------------------

/// A workflow rule — the fundamental unit of computation in OxyMake.
///
/// A rule declares inputs, outputs, an execution block, and metadata.
/// The resolver expands rules into [`ConcreteJob`] instances by resolving
/// wildcards against the filesystem or configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    /// Unique name of this rule within the workflow.
    pub name: RuleName,
    /// Priority for ambiguity resolution (higher wins).
    pub priority: Option<u32>,
    /// Input patterns — may contain `{wildcards}`.
    pub inputs: Vec<InputPattern>,
    /// Output patterns — may contain `{wildcards}`.
    pub outputs: Vec<OutputPattern>,
    /// How the computation is expressed.
    pub execution: ExecutionBlock,
    /// Resource requirements (cpu, mem, gpu, custom).
    pub resources: BTreeMap<String, ResourceValue>,
    /// Optional environment specification.
    pub environment: Option<EnvSpec>,
    /// Explicit tags for filtering, grouping, and reporting.
    pub tags: BTreeMap<String, String>,
    /// Rule metadata (description, etc.).
    pub meta: RuleMeta,
    /// Per-wildcard regex constraints.
    pub wildcard_constraints: BTreeMap<String, String>,
    /// Optional conditional guard — the rule only applies when this evaluates to true.
    pub when: Option<GuardExpr>,
    /// How wildcard lists are combined (Product or Zip).
    pub expand_mode: ExpandMode,
    /// What to do when this rule's job fails.
    pub error_strategy: ErrorStrategy,
    /// Per-rule timeout (overrides global default).
    pub timeout: Option<Duration>,
    /// Per-rule executor override (e.g., "local" to force local execution on a cluster).
    pub executor: Option<String>,
    /// Stdout/stderr capture configuration.
    pub log: LogConfig,
    /// Path pattern for benchmark timing output (may contain `{wildcards}`).
    pub benchmark: Option<String>,
    /// Shorthand retry count — sets `error_strategy` to `Retry { count, .. }`.
    pub retries: Option<u32>,
    /// Named parameters (key-value pairs, accessible as `{params.X}` in shell).
    #[serde(default)]
    pub params: BTreeMap<String, String>,
    /// Parameter files whose content is tracked as cache inputs.
    ///
    /// Changes to any listed file invalidate the cache for this rule.
    /// Paths may contain `{wildcards}`.
    #[serde(default)]
    pub param_files: Vec<String>,
    /// Shell executable to use for running commands (default: `/bin/bash`).
    ///
    /// When `None`, the [`DEFAULT_SHELL`] is used.
    pub shell_executable: Option<String>,
    /// Reproducibility classification for this rule's outputs.
    #[serde(default)]
    pub reproducibility: ReproducibilityClass,
    /// 1-based line number of this rule's definition in the original source
    /// file, when the Oxymakefile was generated by `ox translate`.
    ///
    /// Populated by the translator (see `ox-translate::snakemake::parser`)
    /// and surfaced in `PlanError` messages so that failures on translated
    /// workflows can cite the original Snakefile location.
    #[serde(default)]
    pub source_line: Option<usize>,
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "rule {}", self.name)?;
        if !self.inputs.is_empty() {
            write!(f, " ({} inputs)", self.inputs.len())?;
        }
        write!(f, " -> {} outputs", self.outputs.len())
    }
}

// ---------------------------------------------------------------------------
// Resolved input / output (for ConcreteJob)
// ---------------------------------------------------------------------------

/// A fully resolved input — an [`OutputRef`] with its original name and format.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResolvedInput {
    /// The concrete reference to the input.
    pub reference: OutputRef,
    /// Optional named argument for Call mode.
    pub name: Option<String>,
    /// Optional format hint.
    pub format: Option<String>,
}

/// A fully resolved output — an [`OutputRef`] with lifecycle and materialization policy.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResolvedOutput {
    /// The concrete reference to the output.
    pub reference: OutputRef,
    /// Optional named return for Call mode.
    pub name: Option<String>,
    /// Optional format hint (e.g., "parquet", "csv", "json").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// Lifecycle policy.
    pub lifecycle: OutputLifecycle,
    /// Materialization policy.
    pub materialize: MaterializePolicy,
}

// ---------------------------------------------------------------------------
// Concrete job
// ---------------------------------------------------------------------------

/// A fully resolved job instance — a [`Rule`] with all wildcards expanded
/// and all patterns resolved to concrete references.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConcreteJob {
    /// Unique identifier for this job instance.
    pub id: JobId,
    /// The rule that generated this job.
    pub rule: RuleName,
    /// Resolved wildcard values.
    pub wildcards: BTreeMap<String, String>,
    /// Explicit tags plus any implicit tags derived from wildcards.
    pub tags: BTreeMap<String, String>,
    /// Resolved concrete inputs.
    pub inputs: Vec<ResolvedInput>,
    /// Resolved concrete outputs.
    pub outputs: Vec<ResolvedOutput>,
    /// The execution block with interpolated commands/paths.
    pub execution: ExecutionBlock,
    /// Resource requirements.
    pub resources: BTreeMap<String, ResourceValue>,
    /// Optional environment specification.
    pub environment: Option<EnvSpec>,
    /// Error handling strategy.
    pub error_strategy: ErrorStrategy,
    /// Per-job timeout.
    pub timeout: Option<Duration>,
    /// Per-job executor override.
    pub executor: Option<String>,
    /// Scheduling priority (higher runs first among ready jobs).
    pub priority: Option<u32>,
    /// Resolved benchmark output path.
    pub benchmark: Option<String>,
    /// Resolved parameters (accessible as `{params.X}` in shell).
    #[serde(default)]
    pub params: BTreeMap<String, String>,
    /// Resolved parameter file paths (wildcards interpolated).
    ///
    /// Content of these files is hashed into the cache key — changes
    /// to any listed file invalidate the cache for this job.
    #[serde(default)]
    pub param_files: Vec<PathBuf>,
    /// Resolved log configuration (wildcards interpolated).
    #[serde(default)]
    pub log: LogConfig,
    /// Shell executable for this job (default: `/bin/bash`).
    ///
    /// Inherited from the rule's `shell_executable` field. When `None`,
    /// the [`DEFAULT_SHELL`] is used.
    pub shell_executable: Option<String>,
    /// Reproducibility classification, inherited from the rule.
    #[serde(default)]
    pub reproducibility: ReproducibilityClass,
}

impl fmt::Display for ConcreteJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "job {} (rule: {})", self.id, self.rule)?;
        if !self.wildcards.is_empty() {
            let wc: Vec<_> = self
                .wildcards
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            write!(f, " [{}]", wc.join(", "))?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Graph node / edge types
// ---------------------------------------------------------------------------

/// A node in the job graph — either a job, an output, or a gate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobNode {
    /// A concrete job to be executed.
    Job(Box<ConcreteJob>),
    /// An output artifact (file, virtual resource, or in-memory object).
    Output(OutputRef),
    /// A human-in-the-loop gate that pauses execution until approved.
    Gate(GateId),
}

impl fmt::Display for JobNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Job(job) => write!(f, "{job}"),
            Self::Output(out) => write!(f, "{out}"),
            Self::Gate(gate) => write!(f, "gate:{gate}"),
        }
    }
}

/// An edge in the job graph — describes the relationship between nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobEdge {
    /// A job produces this output.
    Produces,
    /// A job consumes this input.
    Consumes,
    /// A gate blocks downstream jobs until approved.
    Blocks,
}

impl fmt::Display for JobEdge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Produces => f.write_str("produces"),
            Self::Consumes => f.write_str("consumes"),
            Self::Blocks => f.write_str("blocks"),
        }
    }
}

// ---------------------------------------------------------------------------
// Content hash
// ---------------------------------------------------------------------------

/// Validate a 64-character lowercase blake3 hex string.
///
/// Shared by [`ContentHash::from_hex`] and [`ComputationHash::from_hex`].
fn validate_hash_hex(kind: &'static str, value: &str) -> Result<(), InvalidHashError> {
    let valid = value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if valid {
        Ok(())
    } else {
        Err(InvalidHashError {
            kind,
            value: value.to_owned(),
        })
    }
}

/// A blake3 content hash, stored as a 64-character lowercase hex string.
///
/// The inner representation is private: values can only be created from a
/// [`blake3::Hash`] (always valid) or through the validating
/// [`ContentHash::from_hex`], so forged, truncated or uppercase strings are
/// rejected at the boundary instead of being compared as plain strings.
/// Deserialization applies the same validation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ContentHash(String);

impl ContentHash {
    /// Parse and validate a 64-character lowercase hex string.
    pub fn from_hex(hex: impl Into<String>) -> Result<Self, InvalidHashError> {
        let hex = hex.into();
        validate_hash_hex("content hash", &hex)?;
        Ok(Self(hex))
    }

    /// Returns the hash as a hex string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<blake3::Hash> for ContentHash {
    fn from(hash: blake3::Hash) -> Self {
        Self(hash.to_hex().to_string())
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ContentHash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::from_hex(s).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// Computation hash
// ---------------------------------------------------------------------------

/// A blake3 hash of the computation specification, stored as a hex string.
///
/// Unlike [`ContentHash`] (which identifies data by its content), a
/// `ComputationHash` identifies data by *how it was produced*: the hash of
/// the rule source, input content hashes, parameters, environment, and
/// platform. Two runs with identical specifications produce the same
/// `ComputationHash` even if the output bytes differ (e.g. non-deterministic
/// jobs).
///
/// This is the identity used by the cache key system (see `ox-cache`'s
/// `compute_cache_key`).
///
/// Like [`ContentHash`], the inner representation is private and only valid
/// 64-character lowercase hex values can be constructed (see
/// [`ComputationHash::from_hex`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ComputationHash(String);

impl ComputationHash {
    /// Parse and validate a 64-character lowercase hex string.
    pub fn from_hex(hex: impl Into<String>) -> Result<Self, InvalidHashError> {
        let hex = hex.into();
        validate_hash_hex("computation hash", &hex)?;
        Ok(Self(hex))
    }

    /// Returns the hash as a hex string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<blake3::Hash> for ComputationHash {
    fn from(hash: blake3::Hash) -> Self {
        Self(hash.to_hex().to_string())
    }
}

impl fmt::Display for ComputationHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ComputationHash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::from_hex(s).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// External reference
// ---------------------------------------------------------------------------

/// An external resource reference — identifies data by a URI or locator
/// outside OxyMake's control.
///
/// Examples: a database table (`db://warehouse.results`), an API endpoint,
/// a cloud object (`s3://bucket/key`), or a Ray ObjectRef. The `check`
/// field holds a command or query that can verify the resource exists and
/// is valid.
///
/// ```
/// use ox_core::model::ExternalRef;
///
/// let db_ref = ExternalRef {
///     uri: "db://warehouse.results".into(),
///     check: Some("SELECT 1 FROM results LIMIT 1".into()),
/// };
/// assert_eq!(db_ref.uri, "db://warehouse.results");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExternalRef {
    /// The URI or locator for the external resource.
    pub uri: String,
    /// Optional command or query to verify existence/validity.
    pub check: Option<String>,
}

impl fmt::Display for ExternalRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ext:{}", self.uri)
    }
}

// ---------------------------------------------------------------------------
// Artifact identity
// ---------------------------------------------------------------------------

/// How an artifact is identified — the "identity flavor" that determines
/// how OxyMake addresses and verifies an artifact.
///
/// Three flavors exist, each suited to different scenarios:
///
/// - **`Content`**: Identity by data content (BLAKE3 hash). Two artifacts
///   with the same bytes have the same identity regardless of how they were
///   produced. Best for deduplication and content-addressable caching.
///
/// - **`Computation`**: Identity by the computation that produced the data
///   (rule + inputs + params + env). Two runs of the same specification
///   share identity even if the output differs (non-deterministic jobs).
///   Best for cache key lookup and build avoidance.
///
/// - **`External`**: Identity by an external reference (URI, database
///   locator). The data lives outside OxyMake's control; identity is
///   whatever the external system uses. Best for virtual outputs,
///   database tables, and API endpoints.
///
/// ```
/// use ox_core::model::{ArtifactIdentity, ContentHash, ComputationHash, ExternalRef};
///
/// let by_content =
///     ArtifactIdentity::Content(ContentHash::from_hex("ab".repeat(32)).unwrap());
/// let by_computation =
///     ArtifactIdentity::Computation(ComputationHash::from_hex("cd".repeat(32)).unwrap());
/// let by_external = ArtifactIdentity::External(ExternalRef {
///     uri: "s3://bucket/key".into(),
///     check: None,
/// });
///
/// assert!(matches!(by_content, ArtifactIdentity::Content(_)));
/// assert!(matches!(by_computation, ArtifactIdentity::Computation(_)));
/// assert!(matches!(by_external, ArtifactIdentity::External(_)));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "flavor", content = "value", rename_all = "snake_case")]
pub enum ArtifactIdentity {
    /// Identity by data content — BLAKE3 hash of the bytes.
    Content(ContentHash),
    /// Identity by computation specification — cache key hash.
    Computation(ComputationHash),
    /// Identity by external reference — URI or locator.
    External(ExternalRef),
}

impl fmt::Display for ArtifactIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Content(h) => write!(f, "content:{h}"),
            Self::Computation(h) => write!(f, "computation:{h}"),
            Self::External(r) => write!(f, "{r}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

/// A snapshot of the workflow state at a point in time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Human-readable snapshot name.
    pub name: String,
    /// When the snapshot was created (milliseconds since UNIX epoch).
    pub created_at_ms: u64,
    /// Optional note describing why the snapshot was taken.
    pub note: Option<String>,
    /// Hash of the state database at snapshot time.
    pub manifest_hash: ContentHash,
    /// Hash of the Oxymakefile tree at snapshot time.
    pub workflow_hash: ContentHash,
    /// Number of jobs in the workflow at snapshot time.
    pub job_count: usize,
    /// Number of outputs in the workflow at snapshot time.
    pub output_count: usize,
}

impl fmt::Display for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "snapshot '{}' ({} jobs, {} outputs)",
            self.name, self.job_count, self.output_count
        )
    }
}

// ---------------------------------------------------------------------------
// RunReason
// ---------------------------------------------------------------------------

/// Why a job needs to execute rather than being served from cache.
///
/// Propagated through the event bus so reporters can display the reason
/// alongside job-start messages. See ADR-007 for the full design.
///
/// ```
/// use ox_core::model::RunReason;
///
/// let reason = RunReason::OutputMissing { path: "results/A.bam".into() };
/// assert_eq!(reason.to_string(), "output missing: results/A.bam");
///
/// let reason = RunReason::CacheMiss;
/// assert_eq!(reason.to_string(), "no cache entry");
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunReason {
    /// No cache entry for this input hash.
    CacheMiss,
    /// Output file(s) deleted from disk.
    OutputMissing {
        /// The path of the missing output file.
        path: String,
    },
    /// Output content changed since last cache record.
    OutputStale {
        /// The path of the stale output file.
        path: String,
    },
    /// An upstream dependency was rebuilt in this run.
    UpstreamRebuilt,
    /// `--no-cache` flag was used.
    CacheDisabled,
    /// Job has no cacheable outputs (phony targets).
    NotCacheable,
    /// `--force` flag was used.
    Forced,
}

impl RunReason {
    /// Returns `true` for reasons that are "interesting" at default verbosity:
    /// output missing, output stale, or upstream rebuilt.
    pub fn is_interesting(&self) -> bool {
        matches!(
            self,
            RunReason::OutputMissing { .. }
                | RunReason::OutputStale { .. }
                | RunReason::UpstreamRebuilt
        )
    }
}

impl fmt::Display for RunReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunReason::CacheMiss => write!(f, "no cache entry"),
            RunReason::OutputMissing { path } => write!(f, "output missing: {path}"),
            RunReason::OutputStale { path } => write!(f, "output stale: {path}"),
            RunReason::UpstreamRebuilt => write!(f, "upstream rebuilt"),
            RunReason::CacheDisabled => write!(f, "cache disabled"),
            RunReason::NotCacheable => write!(f, "not cacheable"),
            RunReason::Forced => write!(f, "forced"),
        }
    }
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// A structured event emitted by the engine during execution.
///
/// Events are the primary interface for reporters, TUIs, and programmatic
/// consumers. Every state transition produces exactly one event.
///
/// ```
/// use ox_core::model::{Event, JobId, RuleName};
/// use std::collections::BTreeMap;
///
/// let event = Event::RunStarted {
///     total_jobs: 42,
///     to_run: 10,
///     cached: 32,
/// };
/// assert!(matches!(event, Event::RunStarted { .. }));
///
/// let event = Event::JobCompleted {
///     job_id: JobId("align-A".into()),
///     duration_ms: 1234,
///     outputs: vec!["results/A.bam".into()],
/// };
/// if let Event::JobCompleted { duration_ms, .. } = &event {
///     assert_eq!(*duration_ms, 1234);
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// Emitted when a run begins, after DAG resolution.
    RunStarted {
        /// Total number of jobs in the DAG.
        total_jobs: usize,
        /// Number of jobs that will actually execute.
        to_run: usize,
        /// Number of jobs satisfied from cache.
        cached: usize,
    },
    /// Emitted when a job enters the ready queue.
    JobQueued {
        /// The job that was queued.
        job_id: JobId,
        /// The rule that generated this job.
        rule: RuleName,
        /// Tags on this job (for filtering and reporting).
        tags: BTreeMap<String, String>,
    },
    /// Emitted when a job begins execution.
    JobStarted {
        /// The job that started.
        job_id: JobId,
        /// The executor running this job (e.g., "local", "slurm").
        executor: String,
        /// Why this job needs to execute (cache miss, output missing, etc.).
        /// `None` means the reason is unknown (backward compatibility).
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<RunReason>,
    },
    /// Emitted when a job finishes successfully.
    JobCompleted {
        /// The job that completed.
        job_id: JobId,
        /// Wall-clock duration in milliseconds.
        duration_ms: u64,
        /// Paths or IDs of outputs produced.
        outputs: Vec<String>,
    },
    /// Emitted when a job fails.
    JobFailed {
        /// The job that failed.
        job_id: JobId,
        /// Human-readable error message.
        error_message: String,
        /// Process exit code, if available.
        exit_code: Option<i32>,
        /// Last N lines of stderr, if available.
        stderr_tail: Option<String>,
    },
    /// Emitted when a job is skipped (e.g., cached, guard failed).
    JobSkipped {
        /// The job that was skipped.
        job_id: JobId,
        /// Why the job was skipped.
        reason: String,
    },
    /// Emitted when a job is cancelled (e.g., fail-fast after root cause,
    /// user interrupt, or dependency failure).
    JobCancelled {
        /// The job that was cancelled.
        job_id: JobId,
        /// Why the job was cancelled.
        reason: String,
    },
    /// Emitted when a gate checkpoint is reached.
    GateReached {
        /// The gate that was reached.
        gate_id: GateId,
        /// Message to display to the user.
        message: String,
    },
    /// Emitted when a gate is approved (by a human or automation).
    GateApproved {
        /// The gate that was approved.
        gate_id: GateId,
        /// Who or what approved the gate.
        approved_by: String,
    },
    /// Emitted when all jobs have finished successfully.
    RunCompleted {
        /// Total number of jobs.
        total: usize,
        /// Number of jobs that succeeded.
        succeeded: usize,
        /// Number of jobs that failed.
        failed: usize,
        /// Number of jobs that were skipped.
        skipped: usize,
        /// Number of jobs that were cancelled.
        cancelled: usize,
        /// Total wall-clock duration in milliseconds.
        duration_ms: u64,
    },
    /// Emitted when the run fails due to an unrecoverable error.
    RunFailed {
        /// Human-readable error message.
        error_message: String,
    },
    /// Emitted when the scheduler detects a common root cause across
    /// multiple consecutive failures (fail-fast heuristic).
    ///
    /// When N consecutive job failures share the same last stderr line,
    /// the scheduler emits this event and (unless `--keep-going`) cancels
    /// remaining work. This prevents wasting time on 29 identical failures
    /// when the first 3 already reveal the root cause.
    RootCauseDetected {
        /// The shared error line across all matching failures.
        root_cause: String,
        /// How many consecutive failures matched this root cause.
        failure_count: usize,
        /// Job IDs that exhibited this root cause.
        job_ids: Vec<JobId>,
    },
    /// Emitted by executor backends for operational diagnostics.
    ///
    /// These messages replace raw `eprintln!` calls so that executor
    /// output flows through the structured event bus and is captured
    /// in event logs, state.db, and the dashboard.
    ExecutorMessage {
        /// Which executor emitted the message (e.g. "slurm", "local").
        executor: String,
        /// Human-readable diagnostic message.
        message: String,
    },
    /// Emitted for each line of stdout/stderr from a running job.
    ///
    /// Only emitted when verbosity >= 2 (`-vv`). Each line is prefixed
    /// with the job ID in the terminal reporter so the user can follow
    /// real-time output from concurrent jobs.
    JobOutput {
        /// The job that produced this output.
        job_id: JobId,
        /// The output line (without trailing newline).
        line: String,
        /// Which stream produced this line.
        stream: OutputStream,
    },
}

/// Which output stream a line came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RunStarted {
                total_jobs,
                to_run,
                cached,
            } => write!(
                f,
                "run started: {total_jobs} jobs ({to_run} to run, {cached} cached)"
            ),
            Self::JobQueued { job_id, rule, .. } => {
                write!(f, "queued {job_id} (rule: {rule})")
            }
            Self::JobStarted {
                job_id,
                executor,
                reason,
            } => {
                if let Some(r) = reason {
                    write!(f, "started {job_id} on {executor} ({r})")
                } else {
                    write!(f, "started {job_id} on {executor}")
                }
            }
            Self::JobCompleted {
                job_id,
                duration_ms,
                ..
            } => {
                write!(f, "completed {job_id} in {duration_ms}ms")
            }
            Self::JobFailed {
                job_id,
                error_message,
                ..
            } => {
                write!(f, "FAILED {job_id}: {error_message}")
            }
            Self::JobSkipped { job_id, reason } => {
                write!(f, "skipped {job_id}: {reason}")
            }
            Self::JobCancelled { job_id, reason } => {
                write!(f, "cancelled {job_id}: {reason}")
            }
            Self::GateReached { gate_id, message } => {
                write!(f, "gate {gate_id}: {message}")
            }
            Self::GateApproved {
                gate_id,
                approved_by,
            } => {
                write!(f, "gate {gate_id} approved by {approved_by}")
            }
            Self::RunCompleted {
                total,
                succeeded,
                failed,
                skipped,
                cancelled,
                duration_ms,
            } => {
                write!(
                    f,
                    "run completed: {succeeded}/{total} succeeded, {failed} failed, {skipped} skipped, {cancelled} cancelled ({duration_ms}ms)"
                )
            }
            Self::RunFailed { error_message } => {
                write!(f, "run FAILED: {error_message}")
            }
            Self::RootCauseDetected {
                root_cause,
                failure_count,
                ..
            } => {
                write!(
                    f,
                    "root cause detected across {failure_count} failures: {root_cause}"
                )
            }
            Self::ExecutorMessage { executor, message } => {
                write!(f, "[{executor}] {message}")
            }
            Self::JobOutput {
                job_id,
                line,
                stream,
            } => {
                write!(f, "[{job_id}:{stream}] {line}")
            }
        }
    }
}

impl fmt::Display for OutputStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stdout => f.write_str("out"),
            Self::Stderr => f.write_str("err"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic valid ContentHash from a test label (64 hex chars).
    fn ch(label: &str) -> ContentHash {
        let mut hex: String = label.bytes().map(|b| format!("{b:02x}")).collect();
        hex.truncate(64);
        ContentHash::from_hex(format!("{hex:0<64}")).unwrap()
    }

    /// Deterministic valid ComputationHash from a test label (64 hex chars).
    fn cmph(label: &str) -> ComputationHash {
        let mut hex: String = label.bytes().map(|b| format!("{b:02x}")).collect();
        hex.truncate(64);
        ComputationHash::from_hex(format!("{hex:0<64}")).unwrap()
    }

    // ── ContentHash / ComputationHash validation (H12) ──────────────────

    #[test]
    fn content_hash_from_hex_validates() {
        // H12: the newtypes were `pub String` — forged, truncated or
        // uppercase values were accepted and compared as plain strings,
        // and ox-lock decides cache validity on them.
        assert!(ContentHash::from_hex("a".repeat(64)).is_ok());
        assert!(ContentHash::from_hex("").is_err());
        assert!(ContentHash::from_hex("abc123").is_err()); // truncated
        assert!(ContentHash::from_hex("A".repeat(64)).is_err()); // uppercase
        assert!(ContentHash::from_hex("g".repeat(64)).is_err()); // non-hex
        assert!(ComputationHash::from_hex("0123456789abcdef".repeat(4)).is_ok());
        assert!(ComputationHash::from_hex("zz".repeat(32)).is_err());
    }

    #[test]
    fn content_hash_deserialize_rejects_forged_values() {
        // Deserialization (e.g. a hand-edited ox.lock) goes through the
        // same validation as from_hex.
        assert!(serde_json::from_str::<ContentHash>("\"not-a-hash\"").is_err());
        assert!(serde_json::from_str::<ComputationHash>("\"DEADBEEF\"").is_err());
        let ok = format!("\"{}\"", "ab".repeat(32));
        assert_eq!(
            serde_json::from_str::<ContentHash>(&ok).unwrap().as_str(),
            "ab".repeat(32)
        );
        assert!(serde_json::from_str::<ComputationHash>(&ok).is_ok());
    }

    #[test]
    fn content_hash_from_blake3() {
        let h = blake3::hash(b"hello");
        let ch = ContentHash::from(h);
        assert_eq!(ch.as_str(), h.to_hex().as_str());
        let xh = ComputationHash::from(h);
        assert_eq!(xh.as_str(), h.to_hex().as_str());
    }

    // ── ResourceValue round-trip stability (H11) ────────────────────────

    #[test]
    fn resource_value_integral_float_normalizes_to_int() {
        // H11: `gpu = 1.0` and `gpu = 1` are semantically equal but used to
        // deserialize into different variants, so the job spec-hash changed
        // across a save/load cycle. Integral floats normalize to Int.
        let v: ResourceValue = serde_json::from_str("1.0").unwrap();
        assert_eq!(v, ResourceValue::Int(1));
    }

    #[test]
    fn resource_value_fractional_float_stays_float() {
        let v: ResourceValue = serde_json::from_str("0.5").unwrap();
        assert_eq!(v, ResourceValue::Float(ordered_float::OrderedFloat(0.5)));
    }

    #[test]
    fn resource_value_roundtrip_is_stable() {
        // Whatever entered through deserialization must re-deserialize to
        // the same variant after serialization (spec-hash stability).
        for src in ["1", "1.0", "2.5", "\"16G\"", "-3", "1e300"] {
            let v1: ResourceValue = serde_json::from_str(src).unwrap();
            let s = serde_json::to_string(&v1).unwrap();
            let v2: ResourceValue = serde_json::from_str(&s).unwrap();
            assert_eq!(v1, v2, "unstable round-trip for {src} (via {s})");
        }
    }

    mod resource_value_proptests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            /// JSON round-trip of any deserialized float is variant-stable.
            ///
            /// serde_json without the `float_roundtrip` feature may drift by
            /// one ulp on extreme exponents — that is codec lossiness, not a
            /// property of ResourceValue, so such inputs are assumed away.
            #[test]
            fn float_roundtrip_stable(f in proptest::num::f64::NORMAL) {
                let s0 = serde_json::to_string(&f).unwrap();
                let back: f64 = serde_json::from_str(&s0).unwrap();
                prop_assume!(back == f);

                let v1: ResourceValue = serde_json::from_str(&s0).unwrap();
                let s = serde_json::to_string(&v1).unwrap();
                let v2: ResourceValue = serde_json::from_str(&s).unwrap();
                prop_assert_eq!(v1, v2);
            }

            /// JSON round-trip of any integer is variant-stable (always Int).
            #[test]
            fn int_roundtrip_stable(n in any::<i64>()) {
                let v1: ResourceValue =
                    serde_json::from_str(&n.to_string()).unwrap();
                prop_assert_eq!(&v1, &ResourceValue::Int(n));
                let s = serde_json::to_string(&v1).unwrap();
                let v2: ResourceValue = serde_json::from_str(&s).unwrap();
                prop_assert_eq!(v1, v2);
            }
        }
    }

    #[test]
    fn rule_name_display() {
        let name = RuleName::from("align");
        assert_eq!(name.as_str(), "align");
        assert_eq!(name.to_string(), "align");
    }

    #[test]
    fn job_id_equality() {
        let a = JobId::from("job-1");
        let b = JobId::from("job-1");
        assert_eq!(a, b);
    }

    #[test]
    fn execution_block_display() {
        let shell = ExecutionBlock::Shell {
            command: "echo hi".into(),
        };
        assert_eq!(shell.to_string(), "shell: echo hi");

        let call = ExecutionBlock::Call {
            function: "mod.func".into(),
            lang: "python".into(),
        };
        assert_eq!(call.to_string(), "call: mod.func (python)");
    }

    #[test]
    fn output_ref_display() {
        let file = OutputRef::File(PathBuf::from("out.csv"));
        assert_eq!(file.to_string(), "file:out.csv");

        let mem = OutputRef::InMemory {
            type_hint: Some("DataFrame".into()),
        };
        assert_eq!(mem.to_string(), "memory:DataFrame");
    }

    #[test]
    fn error_strategy_default_is_terminate() {
        assert_eq!(ErrorStrategy::default(), ErrorStrategy::Terminate);
    }

    #[test]
    fn error_strategy_retry_display() {
        let retry = ErrorStrategy::Retry {
            count: 3,
            backoff: Backoff::Exponential,
        };
        assert_eq!(retry.to_string(), "retry(3, exponential)");
    }

    #[test]
    fn expand_mode_default_is_product() {
        assert_eq!(ExpandMode::default(), ExpandMode::Product);
    }

    #[test]
    fn resource_value_display() {
        assert_eq!(ResourceValue::Int(4).to_string(), "4");
        assert_eq!(ResourceValue::Str("128G".into()).to_string(), "128G");
    }

    #[test]
    fn event_serialization_roundtrip() {
        let event = Event::RunStarted {
            total_jobs: 10,
            to_run: 7,
            cached: 3,
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(event, parsed);
    }

    #[test]
    fn guard_expr_display() {
        let guard = GuardExpr::In {
            field: "sample".into(),
            values: vec!["A".into(), "B".into()],
        };
        assert_eq!(guard.to_string(), r#"sample in ["A", "B"]"#);
    }

    #[test]
    fn snapshot_display() {
        let snap = Snapshot {
            name: "baseline".into(),
            created_at_ms: 0,
            note: None,
            manifest_hash: ch("abc123"),
            workflow_hash: ch("def456"),
            job_count: 42,
            output_count: 10,
        };
        assert_eq!(
            snap.to_string(),
            "snapshot 'baseline' (42 jobs, 10 outputs)"
        );
    }

    #[test]
    fn job_node_display() {
        let gate = JobNode::Gate(GateId::from("review"));
        assert_eq!(gate.to_string(), "gate:review");
    }

    // --- Display tests for every enum variant ---

    #[test]
    fn output_lifecycle_display_all_variants() {
        assert_eq!(OutputLifecycle::Permanent.to_string(), "permanent");
        assert_eq!(OutputLifecycle::Temporary.to_string(), "temporary");
        assert_eq!(OutputLifecycle::Protected.to_string(), "protected");
    }

    #[test]
    fn materialize_policy_display_all_variants() {
        assert_eq!(MaterializePolicy::Always.to_string(), "always");
        assert_eq!(MaterializePolicy::Auto.to_string(), "auto");
        assert_eq!(MaterializePolicy::Never.to_string(), "never");
        assert_eq!(MaterializePolicy::Final.to_string(), "final");
    }

    #[test]
    fn env_spec_display_all_variants() {
        assert_eq!(EnvSpec::System.to_string(), "system");
        assert_eq!(
            EnvSpec::Uv {
                requirements: Some("requirements.txt".into())
            }
            .to_string(),
            "uv"
        );
        assert_eq!(EnvSpec::Uv { requirements: None }.to_string(), "uv");
        assert_eq!(
            EnvSpec::Conda {
                env: "myenv".into()
            }
            .to_string(),
            "conda:myenv"
        );
        assert_eq!(
            EnvSpec::Docker {
                image: "python:3.12".into()
            }
            .to_string(),
            "docker:python:3.12"
        );
        assert_eq!(
            EnvSpec::Nix {
                expr: "shell.nix".into()
            }
            .to_string(),
            "nix:shell.nix"
        );
        assert_eq!(
            EnvSpec::Apptainer {
                image: "container.sif".into()
            }
            .to_string(),
            "apptainer:container.sif"
        );
    }

    #[test]
    fn backoff_display_all_variants() {
        assert_eq!(Backoff::Constant.to_string(), "constant");
        assert_eq!(Backoff::Linear.to_string(), "linear");
        assert_eq!(Backoff::Exponential.to_string(), "exponential");
    }

    #[test]
    fn error_strategy_display_all_variants() {
        assert_eq!(ErrorStrategy::Terminate.to_string(), "terminate");
        assert_eq!(ErrorStrategy::Ignore.to_string(), "ignore");
        assert_eq!(ErrorStrategy::Finish.to_string(), "finish");

        let retry_linear = ErrorStrategy::Retry {
            count: 2,
            backoff: Backoff::Linear,
        };
        assert_eq!(retry_linear.to_string(), "retry(2, linear)");

        let retry_const = ErrorStrategy::Retry {
            count: 5,
            backoff: Backoff::Constant,
        };
        assert_eq!(retry_const.to_string(), "retry(5, constant)");
    }

    #[test]
    fn rule_meta_display() {
        let with_desc = RuleMeta {
            description: Some("Align reads to reference".into()),
        };
        assert_eq!(with_desc.to_string(), "Align reads to reference");

        let without_desc = RuleMeta { description: None };
        assert_eq!(without_desc.to_string(), "(no description)");
    }

    #[test]
    fn execution_block_display_run_and_script() {
        let run = ExecutionBlock::Run {
            code: "print('hello')".into(),
            lang: "python".into(),
        };
        assert_eq!(run.to_string(), "run (python)");

        let script_with_lang = ExecutionBlock::Script {
            path: PathBuf::from("scripts/align.py"),
            lang: Some("python".into()),
        };
        assert_eq!(
            script_with_lang.to_string(),
            "script: scripts/align.py (python)"
        );

        let script_auto = ExecutionBlock::Script {
            path: PathBuf::from("scripts/run.sh"),
            lang: None,
        };
        assert_eq!(script_auto.to_string(), "script: scripts/run.sh (auto)");
    }

    #[test]
    fn output_ref_virtual_display() {
        let virt = OutputRef::Virtual {
            id: "db://results".into(),
            check: "SELECT 1".into(),
        };
        assert_eq!(virt.to_string(), "virtual:db://results");
    }

    #[test]
    fn output_ref_in_memory_none_hint() {
        let mem = OutputRef::InMemory { type_hint: None };
        assert_eq!(mem.to_string(), "memory:any");
    }

    #[test]
    fn expand_mode_display_zip() {
        assert_eq!(ExpandMode::Zip.to_string(), "zip");
        assert_eq!(ExpandMode::Product.to_string(), "product");
    }

    #[test]
    fn guard_expr_display_all_variants() {
        let not_in = GuardExpr::NotIn {
            field: "sample".into(),
            values: vec!["C".into()],
        };
        assert_eq!(not_in.to_string(), r#"sample not in ["C"]"#);

        let eq = GuardExpr::Eq {
            field: "chr".into(),
            value: "chr1".into(),
        };
        assert_eq!(eq.to_string(), r#"chr == "chr1""#);

        let not_eq = GuardExpr::NotEq {
            field: "chr".into(),
            value: "chrM".into(),
        };
        assert_eq!(not_eq.to_string(), r#"chr != "chrM""#);

        let regex = GuardExpr::Regex {
            field: "sample".into(),
            pattern: "^S\\d+$".into(),
        };
        assert_eq!(regex.to_string(), r#"sample =~ /^S\d+$/"#);
    }

    #[test]
    fn resource_value_float_display() {
        assert_eq!(ResourceValue::Float(OrderedFloat(0.5)).to_string(), "0.5");
    }

    #[test]
    fn job_edge_display_all_variants() {
        assert_eq!(JobEdge::Produces.to_string(), "produces");
        assert_eq!(JobEdge::Consumes.to_string(), "consumes");
        assert_eq!(JobEdge::Blocks.to_string(), "blocks");
    }

    #[test]
    fn content_hash_display_and_as_str() {
        let hex = "ab".repeat(32);
        let h = ContentHash::from_hex(hex.clone()).unwrap();
        assert_eq!(h.to_string(), hex);
        assert_eq!(h.as_str(), hex);
    }

    #[test]
    fn gate_id_display_and_from() {
        let gate = GateId::from("checkpoint");
        assert_eq!(gate.to_string(), "checkpoint");
        assert_eq!(gate.as_str(), "checkpoint");
    }

    #[test]
    fn job_id_display_and_as_str() {
        let id = JobId::from("align-A");
        assert_eq!(id.to_string(), "align-A");
        assert_eq!(id.as_str(), "align-A");
    }

    // --- Default tests ---

    #[test]
    fn output_lifecycle_default() {
        assert_eq!(OutputLifecycle::default(), OutputLifecycle::Permanent);
    }

    #[test]
    fn materialize_policy_default() {
        assert_eq!(MaterializePolicy::default(), MaterializePolicy::Always);
    }

    #[test]
    fn backoff_default() {
        assert_eq!(Backoff::default(), Backoff::Exponential);
    }

    #[test]
    fn log_config_default() {
        let lc = LogConfig::default();
        assert_eq!(lc.stdout, None);
        assert_eq!(lc.stderr, None);
    }

    #[test]
    fn rule_meta_default() {
        let rm = RuleMeta::default();
        assert_eq!(rm.description, None);
    }

    // --- Serde round-trip tests ---

    #[test]
    fn serde_roundtrip_input_pattern() {
        let ip = InputPattern {
            pattern: "data/{sample}.fastq".into(),
            name: Some("reads".into()),
            format: Some("fastq".into()),
        };
        let json = serde_json::to_string(&ip).unwrap();
        let parsed: InputPattern = serde_json::from_str(&json).unwrap();
        assert_eq!(ip, parsed);

        // Without optional fields
        let ip2 = InputPattern {
            pattern: "data/*.csv".into(),
            name: None,
            format: None,
        };
        let json2 = serde_json::to_string(&ip2).unwrap();
        let parsed2: InputPattern = serde_json::from_str(&json2).unwrap();
        assert_eq!(ip2, parsed2);
    }

    #[test]
    fn serde_roundtrip_output_pattern() {
        let op = OutputPattern {
            pattern: "results/{sample}.bam".into(),
            name: Some("aligned".into()),
            format: Some("bam".into()),
            lifecycle: OutputLifecycle::Temporary,
            materialize: MaterializePolicy::Auto,
        };
        let json = serde_json::to_string(&op).unwrap();
        let parsed: OutputPattern = serde_json::from_str(&json).unwrap();
        assert_eq!(op, parsed);
    }

    #[test]
    fn serde_roundtrip_env_spec_all_variants() {
        let specs = vec![
            EnvSpec::System,
            EnvSpec::Uv {
                requirements: Some("req.txt".into()),
            },
            EnvSpec::Uv { requirements: None },
            EnvSpec::Conda {
                env: "myenv".into(),
            },
            EnvSpec::Docker {
                image: "python:3.12".into(),
            },
            EnvSpec::Nix {
                expr: "shell.nix".into(),
            },
            EnvSpec::Apptainer {
                image: "img.sif".into(),
            },
        ];
        for spec in specs {
            let json = serde_json::to_string(&spec).unwrap();
            let parsed: EnvSpec = serde_json::from_str(&json).unwrap();
            assert_eq!(spec, parsed);
        }
    }

    #[test]
    fn serde_roundtrip_rule() {
        let rule = Rule {
            name: RuleName::from("align"),
            priority: Some(10),
            inputs: vec![InputPattern {
                pattern: "data/{sample}.fastq".into(),
                name: None,
                format: None,
            }],
            outputs: vec![OutputPattern {
                pattern: "results/{sample}.bam".into(),
                name: None,
                format: None,
                lifecycle: OutputLifecycle::Permanent,
                materialize: MaterializePolicy::Always,
            }],
            execution: ExecutionBlock::Shell {
                command: "bwa mem".into(),
            },
            resources: BTreeMap::from([("cpu".into(), ResourceValue::Int(4))]),
            environment: Some(EnvSpec::Conda {
                env: "bioinfo".into(),
            }),
            tags: BTreeMap::from([("pipeline".into(), "wgs".into())]),
            meta: RuleMeta {
                description: Some("Align reads".into()),
            },
            wildcard_constraints: BTreeMap::from([("sample".into(), "[A-Z]+".into())]),
            when: Some(GuardExpr::Eq {
                field: "sample".into(),
                value: "A".into(),
            }),
            expand_mode: ExpandMode::Product,
            error_strategy: ErrorStrategy::Retry {
                count: 3,
                backoff: Backoff::Exponential,
            },
            timeout: Some(Duration::from_secs(3600)),
            executor: Some("slurm".into()),
            log: LogConfig {
                stdout: Some("logs/{sample}.out".into()),
                stderr: Some("logs/{sample}.err".into()),
            },
            benchmark: Some("benchmarks/{sample}.tsv".into()),
            retries: Some(2),
            params: BTreeMap::new(),
            param_files: Vec::new(),
            shell_executable: None,
            reproducibility: ReproducibilityClass::default(),
            source_line: None,
        };
        let json = serde_json::to_string(&rule).unwrap();
        let parsed: Rule = serde_json::from_str(&json).unwrap();
        assert_eq!(rule, parsed);
    }

    #[test]
    fn serde_roundtrip_concrete_job() {
        let job = ConcreteJob {
            id: JobId::from("align-A"),
            rule: RuleName::from("align"),
            wildcards: BTreeMap::from([("sample".into(), "A".into())]),
            tags: BTreeMap::new(),
            inputs: vec![ResolvedInput {
                reference: OutputRef::Virtual {
                    id: "db://input".into(),
                    check: "SELECT 1".into(),
                },
                name: None,
                format: None,
            }],
            outputs: vec![ResolvedOutput {
                reference: OutputRef::InMemory {
                    type_hint: Some("DataFrame".into()),
                },
                name: None,
                format: None,
                lifecycle: OutputLifecycle::Permanent,
                materialize: MaterializePolicy::Always,
            }],
            execution: ExecutionBlock::Shell {
                command: "bwa mem".into(),
            },
            resources: BTreeMap::new(),
            environment: None,
            error_strategy: ErrorStrategy::Terminate,
            timeout: None,
            executor: None,
            priority: None,
            benchmark: None,
            params: BTreeMap::new(),
            param_files: Vec::new(),
            log: LogConfig::default(),
            shell_executable: None,
            reproducibility: ReproducibilityClass::default(),
        };
        let json = serde_json::to_string(&job).unwrap();
        let parsed: ConcreteJob = serde_json::from_str(&json).unwrap();
        assert_eq!(job, parsed);
    }

    // --- Display tests for composite types ---

    #[test]
    fn rule_display_with_and_without_inputs() {
        let rule_with_inputs = Rule {
            name: RuleName::from("align"),
            priority: None,
            inputs: vec![InputPattern {
                pattern: "in.txt".into(),
                name: None,
                format: None,
            }],
            outputs: vec![
                OutputPattern {
                    pattern: "a.out".into(),
                    name: None,
                    format: None,
                    lifecycle: OutputLifecycle::default(),
                    materialize: MaterializePolicy::default(),
                },
                OutputPattern {
                    pattern: "b.out".into(),
                    name: None,
                    format: None,
                    lifecycle: OutputLifecycle::default(),
                    materialize: MaterializePolicy::default(),
                },
            ],
            execution: ExecutionBlock::Shell {
                command: "echo".into(),
            },
            resources: BTreeMap::new(),
            environment: None,
            tags: BTreeMap::new(),
            meta: RuleMeta::default(),
            wildcard_constraints: BTreeMap::new(),
            when: None,
            expand_mode: ExpandMode::default(),
            error_strategy: ErrorStrategy::default(),
            timeout: None,
            executor: None,
            log: LogConfig::default(),
            benchmark: None,
            retries: None,
            params: BTreeMap::new(),
            param_files: Vec::new(),
            shell_executable: None,
            reproducibility: ReproducibilityClass::default(),
            source_line: None,
        };
        assert_eq!(
            rule_with_inputs.to_string(),
            "rule align (1 inputs) -> 2 outputs"
        );

        let rule_no_inputs = Rule {
            inputs: vec![],
            ..rule_with_inputs.clone()
        };
        assert_eq!(rule_no_inputs.to_string(), "rule align -> 2 outputs");
    }

    #[test]
    fn concrete_job_display_with_and_without_wildcards() {
        let job_with_wc = ConcreteJob {
            id: JobId::from("align-A"),
            rule: RuleName::from("align"),
            wildcards: BTreeMap::from([
                ("chr".into(), "chr1".into()),
                ("sample".into(), "A".into()),
            ]),
            tags: BTreeMap::new(),
            inputs: vec![],
            outputs: vec![],
            execution: ExecutionBlock::Shell {
                command: "echo".into(),
            },
            resources: BTreeMap::new(),
            environment: None,
            error_strategy: ErrorStrategy::default(),
            timeout: None,
            executor: None,
            priority: None,
            benchmark: None,
            params: BTreeMap::new(),
            param_files: Vec::new(),
            log: LogConfig::default(),
            shell_executable: None,
            reproducibility: ReproducibilityClass::default(),
        };
        assert_eq!(
            job_with_wc.to_string(),
            "job align-A (rule: align) [chr=chr1, sample=A]"
        );

        let job_no_wc = ConcreteJob {
            wildcards: BTreeMap::new(),
            ..job_with_wc.clone()
        };
        assert_eq!(job_no_wc.to_string(), "job align-A (rule: align)");
    }

    #[test]
    fn job_node_display_job_and_output() {
        let job = ConcreteJob {
            id: JobId::from("test-1"),
            rule: RuleName::from("test"),
            wildcards: BTreeMap::new(),
            tags: BTreeMap::new(),
            inputs: vec![],
            outputs: vec![],
            execution: ExecutionBlock::Shell {
                command: "echo".into(),
            },
            resources: BTreeMap::new(),
            environment: None,
            error_strategy: ErrorStrategy::default(),
            timeout: None,
            executor: None,
            priority: None,
            benchmark: None,
            params: BTreeMap::new(),
            param_files: Vec::new(),
            log: LogConfig::default(),
            shell_executable: None,
            reproducibility: ReproducibilityClass::default(),
        };
        let node = JobNode::Job(Box::new(job));
        assert_eq!(node.to_string(), "job test-1 (rule: test)");

        let out_node = JobNode::Output(OutputRef::File(PathBuf::from("out.txt")));
        assert_eq!(out_node.to_string(), "file:out.txt");
    }

    // --- Event Display tests for all variants ---

    #[test]
    fn event_display_all_variants() {
        let run_started = Event::RunStarted {
            total_jobs: 10,
            to_run: 7,
            cached: 3,
        };
        assert_eq!(
            run_started.to_string(),
            "run started: 10 jobs (7 to run, 3 cached)"
        );

        let queued = Event::JobQueued {
            job_id: JobId::from("j1"),
            rule: RuleName::from("r1"),
            tags: BTreeMap::new(),
        };
        assert_eq!(queued.to_string(), "queued j1 (rule: r1)");

        let started = Event::JobStarted {
            job_id: JobId::from("j1"),
            executor: "local".into(),
            reason: None,
        };
        assert_eq!(started.to_string(), "started j1 on local");

        let started_with_reason = Event::JobStarted {
            job_id: JobId::from("j1"),
            executor: "local".into(),
            reason: Some(RunReason::OutputMissing {
                path: "out.bam".into(),
            }),
        };
        assert_eq!(
            started_with_reason.to_string(),
            "started j1 on local (output missing: out.bam)"
        );

        let completed = Event::JobCompleted {
            job_id: JobId::from("j1"),
            duration_ms: 500,
            outputs: vec!["out.txt".into()],
        };
        assert_eq!(completed.to_string(), "completed j1 in 500ms");

        let failed = Event::JobFailed {
            job_id: JobId::from("j1"),
            error_message: "segfault".into(),
            exit_code: Some(139),
            stderr_tail: Some("core dumped".into()),
        };
        assert_eq!(failed.to_string(), "FAILED j1: segfault");

        let skipped = Event::JobSkipped {
            job_id: JobId::from("j1"),
            reason: "cached".into(),
        };
        assert_eq!(skipped.to_string(), "skipped j1: cached");

        let gate_reached = Event::GateReached {
            gate_id: GateId::from("g1"),
            message: "Please review".into(),
        };
        assert_eq!(gate_reached.to_string(), "gate g1: Please review");

        let gate_approved = Event::GateApproved {
            gate_id: GateId::from("g1"),
            approved_by: "admin".into(),
        };
        assert_eq!(gate_approved.to_string(), "gate g1 approved by admin");

        let cancelled = Event::JobCancelled {
            job_id: JobId::from("j1"),
            reason: "dependency failed".into(),
        };
        assert_eq!(cancelled.to_string(), "cancelled j1: dependency failed");

        let run_completed = Event::RunCompleted {
            total: 10,
            succeeded: 7,
            failed: 1,
            skipped: 1,
            cancelled: 1,
            duration_ms: 5000,
        };
        assert_eq!(
            run_completed.to_string(),
            "run completed: 7/10 succeeded, 1 failed, 1 skipped, 1 cancelled (5000ms)"
        );

        let run_failed = Event::RunFailed {
            error_message: "DAG cycle detected".into(),
        };
        assert_eq!(run_failed.to_string(), "run FAILED: DAG cycle detected");

        let root_cause = Event::RootCauseDetected {
            root_cause: "FileNotFoundError: /data/input.csv".into(),
            failure_count: 5,
            job_ids: vec![JobId::from("j1"), JobId::from("j2")],
        };
        assert_eq!(
            root_cause.to_string(),
            "root cause detected across 5 failures: FileNotFoundError: /data/input.csv"
        );
    }

    // --- Event serde round-trip for all variants ---

    #[test]
    fn serde_roundtrip_event_all_variants() {
        let events: Vec<Event> = vec![
            Event::RunStarted {
                total_jobs: 10,
                to_run: 7,
                cached: 3,
            },
            Event::JobQueued {
                job_id: JobId::from("j1"),
                rule: RuleName::from("r1"),
                tags: BTreeMap::from([("k".into(), "v".into())]),
            },
            Event::JobStarted {
                job_id: JobId::from("j1"),
                executor: "local".into(),
                reason: None,
            },
            Event::JobCompleted {
                job_id: JobId::from("j1"),
                duration_ms: 500,
                outputs: vec!["out.txt".into()],
            },
            Event::JobFailed {
                job_id: JobId::from("j1"),
                error_message: "oops".into(),
                exit_code: Some(1),
                stderr_tail: Some("err".into()),
            },
            Event::JobFailed {
                job_id: JobId::from("j2"),
                error_message: "oops".into(),
                exit_code: None,
                stderr_tail: None,
            },
            Event::JobSkipped {
                job_id: JobId::from("j1"),
                reason: "cached".into(),
            },
            Event::JobCancelled {
                job_id: JobId::from("j1"),
                reason: "dependency failed".into(),
            },
            Event::GateReached {
                gate_id: GateId::from("g1"),
                message: "review".into(),
            },
            Event::GateApproved {
                gate_id: GateId::from("g1"),
                approved_by: "admin".into(),
            },
            Event::RunCompleted {
                total: 10,
                succeeded: 8,
                failed: 1,
                skipped: 1,
                cancelled: 0,
                duration_ms: 5000,
            },
            Event::RunFailed {
                error_message: "boom".into(),
            },
            Event::RootCauseDetected {
                root_cause: "FileNotFoundError: /data/input.csv".into(),
                failure_count: 3,
                job_ids: vec![JobId::from("j1"), JobId::from("j2"), JobId::from("j3")],
            },
        ];
        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            let parsed: Event = serde_json::from_str(&json).unwrap();
            assert_eq!(event, parsed);
        }
    }

    // --- Serde round-trip for other types ---

    #[test]
    fn serde_roundtrip_execution_block_all_variants() {
        let blocks = vec![
            ExecutionBlock::Shell {
                command: "echo hi".into(),
            },
            ExecutionBlock::Run {
                code: "print(1)".into(),
                lang: "python".into(),
            },
            ExecutionBlock::Script {
                path: PathBuf::from("run.sh"),
                lang: Some("bash".into()),
            },
            ExecutionBlock::Script {
                path: PathBuf::from("run.py"),
                lang: None,
            },
            ExecutionBlock::Call {
                function: "mod.f".into(),
                lang: "python".into(),
            },
        ];
        for block in blocks {
            let json = serde_json::to_string(&block).unwrap();
            let parsed: ExecutionBlock = serde_json::from_str(&json).unwrap();
            assert_eq!(block, parsed);
        }
    }

    #[test]
    fn serde_roundtrip_output_ref_non_file_variants() {
        // Note: OutputRef::File is a tagged newtype variant containing PathBuf,
        // which serde's internally-tagged representation cannot serialize to JSON.
        // We test the other variants here.
        let refs = vec![
            OutputRef::Virtual {
                id: "db://t".into(),
                check: "SELECT 1".into(),
            },
            OutputRef::InMemory {
                type_hint: Some("DataFrame".into()),
            },
            OutputRef::InMemory { type_hint: None },
        ];
        for r in refs {
            let json = serde_json::to_string(&r).unwrap();
            let parsed: OutputRef = serde_json::from_str(&json).unwrap();
            assert_eq!(r, parsed);
        }
    }

    #[test]
    fn serde_roundtrip_guard_expr_all_variants() {
        let guards = vec![
            GuardExpr::In {
                field: "s".into(),
                values: vec!["A".into()],
            },
            GuardExpr::NotIn {
                field: "s".into(),
                values: vec!["B".into()],
            },
            GuardExpr::Eq {
                field: "s".into(),
                value: "A".into(),
            },
            GuardExpr::NotEq {
                field: "s".into(),
                value: "B".into(),
            },
            GuardExpr::Regex {
                field: "s".into(),
                pattern: "^A$".into(),
            },
        ];
        for g in guards {
            let json = serde_json::to_string(&g).unwrap();
            let parsed: GuardExpr = serde_json::from_str(&json).unwrap();
            assert_eq!(g, parsed);
        }
    }

    #[test]
    fn serde_roundtrip_resource_value_all_variants() {
        let vals = vec![
            ResourceValue::Int(4),
            ResourceValue::Float(OrderedFloat(0.5)),
            ResourceValue::Str("128G".into()),
        ];
        for v in vals {
            let json = serde_json::to_string(&v).unwrap();
            let parsed: ResourceValue = serde_json::from_str(&json).unwrap();
            assert_eq!(v, parsed);
        }
    }

    #[test]
    fn serde_roundtrip_output_lifecycle() {
        for lc in [
            OutputLifecycle::Permanent,
            OutputLifecycle::Temporary,
            OutputLifecycle::Protected,
        ] {
            let json = serde_json::to_string(&lc).unwrap();
            let parsed: OutputLifecycle = serde_json::from_str(&json).unwrap();
            assert_eq!(lc, parsed);
        }
    }

    #[test]
    fn serde_roundtrip_materialize_policy() {
        for mp in [
            MaterializePolicy::Always,
            MaterializePolicy::Auto,
            MaterializePolicy::Never,
            MaterializePolicy::Final,
        ] {
            let json = serde_json::to_string(&mp).unwrap();
            let parsed: MaterializePolicy = serde_json::from_str(&json).unwrap();
            assert_eq!(mp, parsed);
        }
    }

    #[test]
    fn serde_roundtrip_expand_mode() {
        for em in [ExpandMode::Product, ExpandMode::Zip] {
            let json = serde_json::to_string(&em).unwrap();
            let parsed: ExpandMode = serde_json::from_str(&json).unwrap();
            assert_eq!(em, parsed);
        }
    }

    #[test]
    fn serde_roundtrip_backoff() {
        for b in [Backoff::Constant, Backoff::Linear, Backoff::Exponential] {
            let json = serde_json::to_string(&b).unwrap();
            let parsed: Backoff = serde_json::from_str(&json).unwrap();
            assert_eq!(b, parsed);
        }
    }

    #[test]
    fn serde_roundtrip_error_strategy_all_variants() {
        let strategies = vec![
            ErrorStrategy::Terminate,
            ErrorStrategy::Ignore,
            ErrorStrategy::Finish,
            ErrorStrategy::Retry {
                count: 3,
                backoff: Backoff::Exponential,
            },
            ErrorStrategy::Retry {
                count: 1,
                backoff: Backoff::Constant,
            },
        ];
        for s in strategies {
            let json = serde_json::to_string(&s).unwrap();
            let parsed: ErrorStrategy = serde_json::from_str(&json).unwrap();
            assert_eq!(s, parsed);
        }
    }

    #[test]
    fn serde_roundtrip_job_edge() {
        for edge in [JobEdge::Produces, JobEdge::Consumes, JobEdge::Blocks] {
            let json = serde_json::to_string(&edge).unwrap();
            let parsed: JobEdge = serde_json::from_str(&json).unwrap();
            assert_eq!(edge, parsed);
        }
    }

    #[test]
    fn serde_roundtrip_snapshot() {
        let snap = Snapshot {
            name: "test".into(),
            created_at_ms: 1234567890,
            note: Some("a note".into()),
            manifest_hash: ch("abc"),
            workflow_hash: ch("def"),
            job_count: 5,
            output_count: 3,
        };
        let json = serde_json::to_string(&snap).unwrap();
        let parsed: Snapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(snap, parsed);

        // Without note
        let snap2 = Snapshot { note: None, ..snap };
        let json2 = serde_json::to_string(&snap2).unwrap();
        let parsed2: Snapshot = serde_json::from_str(&json2).unwrap();
        assert_eq!(snap2, parsed2);
    }

    #[test]
    fn serde_roundtrip_job_node_job_variant() {
        // Note: JobNode::Output and JobNode::Gate are newtype variants inside an
        // internally-tagged enum, which serde cannot serialize to JSON.
        // We test the Job variant (struct) which works correctly.
        let job = ConcreteJob {
            id: JobId::from("j1"),
            rule: RuleName::from("r1"),
            wildcards: BTreeMap::new(),
            tags: BTreeMap::new(),
            inputs: vec![],
            outputs: vec![],
            execution: ExecutionBlock::Shell {
                command: "echo".into(),
            },
            resources: BTreeMap::new(),
            environment: None,
            error_strategy: ErrorStrategy::default(),
            timeout: None,
            executor: None,
            priority: None,
            benchmark: None,
            params: BTreeMap::new(),
            param_files: Vec::new(),
            log: LogConfig::default(),
            shell_executable: None,
            reproducibility: ReproducibilityClass::default(),
        };
        let node = JobNode::Job(Box::new(job));
        let json = serde_json::to_string(&node).unwrap();
        let parsed: JobNode = serde_json::from_str(&json).unwrap();
        assert_eq!(node, parsed);
    }

    #[test]
    fn serde_roundtrip_log_config() {
        let lc = LogConfig {
            stdout: Some("out.log".into()),
            stderr: Some("err.log".into()),
        };
        let json = serde_json::to_string(&lc).unwrap();
        let parsed: LogConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(lc, parsed);
    }

    #[test]
    fn serde_roundtrip_rule_meta() {
        let rm = RuleMeta {
            description: Some("test".into()),
        };
        let json = serde_json::to_string(&rm).unwrap();
        let parsed: RuleMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(rm, parsed);
    }

    #[test]
    fn serde_roundtrip_content_hash() {
        let h = ch("abc123");
        let json = serde_json::to_string(&h).unwrap();
        let parsed: ContentHash = serde_json::from_str(&json).unwrap();
        assert_eq!(h, parsed);
    }

    // --- Clone / PartialEq / Hash tests ---

    #[test]
    fn resource_value_hash_all_variants() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(ResourceValue::Int(4));
        set.insert(ResourceValue::Float(OrderedFloat(0.5)));
        set.insert(ResourceValue::Str("128G".into()));
        assert_eq!(set.len(), 3);

        // Same value hashes to same bucket
        set.insert(ResourceValue::Int(4));
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn resource_value_eq() {
        assert_eq!(ResourceValue::Int(1), ResourceValue::Int(1));
        assert_eq!(
            ResourceValue::Float(OrderedFloat(1.0)),
            ResourceValue::Float(OrderedFloat(1.0))
        );
        assert_eq!(
            ResourceValue::Str("a".into()),
            ResourceValue::Str("a".into())
        );
        assert_ne!(ResourceValue::Int(1), ResourceValue::Int(2));
    }

    /// Regression test for ox-pkl: NaN must be equal to itself so that
    /// HashSet/HashMap lookups remain sound (Eq contract).
    #[test]
    fn resource_value_nan_eq_is_sound() {
        let nan = ResourceValue::Float(OrderedFloat(f64::NAN));
        // OrderedFloat defines NaN == NaN, satisfying the Eq contract.
        assert_eq!(nan, nan.clone());

        // NaN entries must be retrievable from a HashSet.
        let mut set = std::collections::HashSet::new();
        set.insert(nan.clone());
        assert!(set.contains(&nan));
    }

    #[test]
    fn job_id_ordering() {
        let a = JobId::from("aaa");
        let b = JobId::from("bbb");
        assert!(a < b);
        assert!(b > a);

        let mut ids = vec![JobId::from("c"), JobId::from("a"), JobId::from("b")];
        ids.sort();
        assert_eq!(
            ids,
            vec![JobId::from("a"), JobId::from("b"), JobId::from("c")]
        );
    }

    #[test]
    fn clone_and_eq_for_newtype_ids() {
        let rn = RuleName::from("test");
        let rn2 = rn.clone();
        assert_eq!(rn, rn2);

        let jid = JobId::from("j1");
        let jid2 = jid.clone();
        assert_eq!(jid, jid2);

        let gid = GateId::from("g1");
        let gid2 = gid.clone();
        assert_eq!(gid, gid2);
    }

    #[test]
    fn hash_for_newtype_ids() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(RuleName::from("a"));
        set.insert(RuleName::from("b"));
        set.insert(RuleName::from("a"));
        assert_eq!(set.len(), 2);

        let mut jset = HashSet::new();
        jset.insert(JobId::from("j1"));
        jset.insert(JobId::from("j1"));
        assert_eq!(jset.len(), 1);

        let mut gset = HashSet::new();
        gset.insert(GateId::from("g1"));
        gset.insert(GateId::from("g1"));
        assert_eq!(gset.len(), 1);
    }

    // --- ResolvedInput / ResolvedOutput construction ---

    #[test]
    fn resolved_input_construction_and_clone() {
        let ri = ResolvedInput {
            reference: OutputRef::File(PathBuf::from("data/A.fastq")),
            name: Some("reads".into()),
            format: Some("fastq".into()),
        };
        let ri2 = ri.clone();
        assert_eq!(ri, ri2);

        let ri3 = ResolvedInput {
            reference: OutputRef::InMemory {
                type_hint: Some("DataFrame".into()),
            },
            name: None,
            format: None,
        };
        assert_ne!(ri, ri3);
    }

    #[test]
    fn resolved_output_construction_and_clone() {
        let ro = ResolvedOutput {
            reference: OutputRef::File(PathBuf::from("results/A.bam")),
            name: Some("aligned".into()),
            format: Some("bam".into()),
            lifecycle: OutputLifecycle::Temporary,
            materialize: MaterializePolicy::Auto,
        };
        let ro2 = ro.clone();
        assert_eq!(ro, ro2);

        let ro3 = ResolvedOutput {
            reference: OutputRef::Virtual {
                id: "db://t".into(),
                check: "SELECT 1".into(),
            },
            name: None,
            format: None,
            lifecycle: OutputLifecycle::Protected,
            materialize: MaterializePolicy::Never,
        };
        assert_ne!(ro, ro3);
    }

    #[test]
    fn serde_roundtrip_resolved_input() {
        let ri = ResolvedInput {
            reference: OutputRef::Virtual {
                id: "db://input".into(),
                check: "SELECT 1".into(),
            },
            name: Some("reads".into()),
            format: Some("fastq".into()),
        };
        let json = serde_json::to_string(&ri).unwrap();
        let parsed: ResolvedInput = serde_json::from_str(&json).unwrap();
        assert_eq!(ri, parsed);
    }

    #[test]
    fn serde_roundtrip_resolved_output() {
        let ro = ResolvedOutput {
            reference: OutputRef::InMemory {
                type_hint: Some("DataFrame".into()),
            },
            name: Some("aligned".into()),
            format: Some("parquet".into()),
            lifecycle: OutputLifecycle::Temporary,
            materialize: MaterializePolicy::Final,
        };
        let json = serde_json::to_string(&ro).unwrap();
        let parsed: ResolvedOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(ro, parsed);
    }

    // --- LogConfig display (via Debug since no Display impl) ---

    #[test]
    fn log_config_with_values() {
        let lc = LogConfig {
            stdout: Some("out.log".into()),
            stderr: Some("err.log".into()),
        };
        let lc2 = lc.clone();
        assert_eq!(lc, lc2);
    }

    // --- Snapshot with note ---

    #[test]
    fn snapshot_clone_and_eq() {
        let snap = Snapshot {
            name: "s1".into(),
            created_at_ms: 100,
            note: Some("note".into()),
            manifest_hash: ch("h1"),
            workflow_hash: ch("h2"),
            job_count: 1,
            output_count: 2,
        };
        let snap2 = snap.clone();
        assert_eq!(snap, snap2);
    }

    // --- ContentHash clone and eq ---

    #[test]
    fn content_hash_clone_eq_hash() {
        use std::collections::HashSet;
        let h1 = ch("abc");
        let h2 = h1.clone();
        assert_eq!(h1, h2);

        let mut set = HashSet::new();
        set.insert(h1);
        set.insert(ch("abc"));
        assert_eq!(set.len(), 1);
    }

    // --- Execution block clone/eq/hash ---

    #[test]
    fn execution_block_clone_eq_hash() {
        use std::collections::HashSet;
        let eb = ExecutionBlock::Shell {
            command: "echo".into(),
        };
        let eb2 = eb.clone();
        assert_eq!(eb, eb2);

        let mut set = HashSet::new();
        set.insert(eb);
        set.insert(ExecutionBlock::Shell {
            command: "echo".into(),
        });
        assert_eq!(set.len(), 1);
    }

    // --- InputPattern / OutputPattern clone/eq/hash ---

    #[test]
    fn input_pattern_clone_eq_hash() {
        use std::collections::HashSet;
        let ip = InputPattern {
            pattern: "data/*.csv".into(),
            name: None,
            format: None,
        };
        let ip2 = ip.clone();
        assert_eq!(ip, ip2);

        let mut set = HashSet::new();
        set.insert(ip);
        set.insert(InputPattern {
            pattern: "data/*.csv".into(),
            name: None,
            format: None,
        });
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn output_pattern_clone_eq_hash() {
        use std::collections::HashSet;
        let op = OutputPattern {
            pattern: "out.txt".into(),
            name: None,
            format: None,
            lifecycle: OutputLifecycle::Permanent,
            materialize: MaterializePolicy::Always,
        };
        let op2 = op.clone();
        assert_eq!(op, op2);

        let mut set = HashSet::new();
        set.insert(op);
        set.insert(OutputPattern {
            pattern: "out.txt".into(),
            name: None,
            format: None,
            lifecycle: OutputLifecycle::Permanent,
            materialize: MaterializePolicy::Always,
        });
        assert_eq!(set.len(), 1);
    }

    // --- Copy trait for enums that derive Copy ---

    #[test]
    fn copy_trait_enums() {
        let ol = OutputLifecycle::Temporary;
        let ol2 = ol; // copy
        assert_eq!(ol, ol2);

        let mp = MaterializePolicy::Auto;
        let mp2 = mp;
        assert_eq!(mp, mp2);

        let em = ExpandMode::Zip;
        let em2 = em;
        assert_eq!(em, em2);

        let b = Backoff::Linear;
        let b2 = b;
        assert_eq!(b, b2);

        let je = JobEdge::Produces;
        let je2 = je;
        assert_eq!(je, je2);
    }

    // --- Debug impls (exercises derive(Debug)) ---

    #[test]
    fn debug_impls_not_empty() {
        assert!(!format!("{:?}", RuleName::from("x")).is_empty());
        assert!(!format!("{:?}", JobId::from("x")).is_empty());
        assert!(!format!("{:?}", GateId::from("x")).is_empty());
        assert!(!format!("{:?}", OutputLifecycle::Permanent).is_empty());
        assert!(!format!("{:?}", MaterializePolicy::Always).is_empty());
        assert!(!format!("{:?}", ExpandMode::Product).is_empty());
        assert!(!format!("{:?}", Backoff::Exponential).is_empty());
        assert!(!format!("{:?}", ErrorStrategy::Terminate).is_empty());
        assert!(!format!("{:?}", EnvSpec::System).is_empty());
        assert!(!format!("{:?}", RuleMeta::default()).is_empty());
        assert!(!format!("{:?}", LogConfig::default()).is_empty());
        assert!(!format!("{:?}", ch("h")).is_empty());
        assert!(!format!("{:?}", JobEdge::Produces).is_empty());
        assert!(!format!("{:?}", ResourceValue::Float(OrderedFloat(1.0))).is_empty());
    }

    // --- Serde roundtrip for RuleName, JobId, GateId ---

    #[test]
    fn serde_roundtrip_newtype_ids() {
        let rn = RuleName::from("align");
        let json = serde_json::to_string(&rn).unwrap();
        let parsed: RuleName = serde_json::from_str(&json).unwrap();
        assert_eq!(rn, parsed);

        let jid = JobId::from("j1");
        let json = serde_json::to_string(&jid).unwrap();
        let parsed: JobId = serde_json::from_str(&json).unwrap();
        assert_eq!(jid, parsed);

        let gid = GateId::from("g1");
        let json = serde_json::to_string(&gid).unwrap();
        let parsed: GateId = serde_json::from_str(&json).unwrap();
        assert_eq!(gid, parsed);
    }

    // --- ArtifactMeta tests ---

    #[test]
    fn artifact_meta_is_40_bytes() {
        assert_eq!(std::mem::size_of::<ArtifactMeta>(), 40);
    }

    #[test]
    fn artifact_meta_from_hex_roundtrip() {
        let hex = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
        let meta = ArtifactMeta::from_hex(hex, 1024).unwrap();
        assert_eq!(meta.size_bytes, 1024);
        assert_eq!(meta.hex(), hex);
    }

    #[test]
    fn artifact_meta_from_hex_rejects_bad_length() {
        assert!(ArtifactMeta::from_hex("abcd", 0).is_none());
        assert!(ArtifactMeta::from_hex("", 0).is_none());
    }

    #[test]
    fn artifact_meta_from_hex_rejects_invalid_chars() {
        // 64 chars but with 'g' which is not valid hex
        let bad = "g1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
        assert!(ArtifactMeta::from_hex(bad, 0).is_none());
    }

    #[test]
    fn artifact_meta_to_content_hash() {
        let hex = "0000000000000000000000000000000000000000000000000000000000000001";
        let meta = ArtifactMeta::from_hex(hex, 42).unwrap();
        let ch = meta.to_content_hash();
        assert_eq!(ch.as_str(), hex);
    }

    #[test]
    fn artifact_meta_display() {
        let hex = "0000000000000000000000000000000000000000000000000000000000000001";
        let meta = ArtifactMeta::from_hex(hex, 999).unwrap();
        assert_eq!(format!("{meta}"), format!("{hex}:999"));
    }

    #[test]
    fn artifact_meta_copy_clone_eq_hash() {
        let a = ArtifactMeta::new([0xab; 32], 100);
        let b = a; // Copy
        assert_eq!(a, b);

        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(a);
        set.insert(b);
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn artifact_meta_uppercase_hex_accepted() {
        let hex = "A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2";
        let meta = ArtifactMeta::from_hex(hex, 0).unwrap();
        // Output is always lowercase
        assert_eq!(meta.hex(), hex.to_ascii_lowercase());
    }

    // --- MaterializationSet tests ---

    #[test]
    fn materialization_set_basic() {
        let out = OutputRef::File(PathBuf::from("data/out.csv"));
        let mut ms = MaterializationSet::new(out, 2);

        assert!(ms.is_empty());
        assert!(!ms.is_available());
        assert_eq!(ms.pending_consumers(), 2);
        assert!(!ms.is_evictable());

        // Add disk materialization.
        ms.add(Materialization::OnDisk {
            path: PathBuf::from("data/out.csv"),
            verified: false,
        });
        assert_eq!(ms.len(), 1);
        assert!(ms.is_available());
    }

    #[test]
    fn materialization_set_cheapest_selection() {
        let out = OutputRef::File(PathBuf::from("data/out.csv"));
        let mut ms = MaterializationSet::new(out, 1);

        ms.add(Materialization::OnDisk {
            path: PathBuf::from("data/out.csv"),
            verified: true,
        });
        ms.add(Materialization::InMemory { pinned: false });

        // Memory should be cheapest.
        let cheapest = ms.cheapest().unwrap();
        assert!(matches!(cheapest, Materialization::InMemory { .. }));

        // Add object store — still memory is cheapest.
        ms.add(Materialization::ObjectStore {
            ref_id: "abc123".into(),
            node: None,
        });
        assert_eq!(ms.len(), 3);
        let cheapest = ms.cheapest().unwrap();
        assert!(matches!(cheapest, Materialization::InMemory { .. }));
    }

    #[test]
    fn materialization_set_eviction_guard() {
        let out = OutputRef::File(PathBuf::from("data/out.csv"));
        let mut ms = MaterializationSet::new(out, 1);

        let disk = Materialization::OnDisk {
            path: PathBuf::from("data/out.csv"),
            verified: false,
        };
        ms.add(disk.clone());

        // Cannot remove last materialization while consumers pending.
        assert!(!ms.try_remove(&disk));
        assert_eq!(ms.len(), 1);

        // Consumer fires — now eviction is allowed.
        ms.consumer_fired();
        assert!(ms.is_evictable());
        assert!(ms.try_remove(&disk));
        assert!(ms.is_empty());
    }

    #[test]
    fn materialization_set_replace_same_variant() {
        let out = OutputRef::File(PathBuf::from("data/out.csv"));
        let mut ms = MaterializationSet::new(out, 0);

        ms.add(Materialization::OnDisk {
            path: PathBuf::from("data/out.csv"),
            verified: false,
        });
        // Adding another OnDisk replaces the first.
        ms.add(Materialization::OnDisk {
            path: PathBuf::from("data/out.csv"),
            verified: true,
        });
        assert_eq!(ms.len(), 1);

        // The replacement should be the verified one.
        let mat = ms.cheapest().unwrap();
        match mat {
            Materialization::OnDisk { verified, .. } => assert!(verified),
            _ => panic!("expected OnDisk"),
        }
    }

    #[test]
    fn materialization_cost_ordering() {
        let mem = Materialization::InMemory { pinned: false };
        let obj = Materialization::ObjectStore {
            ref_id: "x".into(),
            node: None,
        };
        let disk = Materialization::OnDisk {
            path: PathBuf::from("f"),
            verified: false,
        };

        assert!(mem.cost_us() < obj.cost_us());
        assert!(obj.cost_us() < disk.cost_us());
    }

    #[test]
    fn materialization_display() {
        let mem = Materialization::InMemory { pinned: true };
        assert_eq!(format!("{mem}"), "memory(pinned)");

        let disk = Materialization::OnDisk {
            path: PathBuf::from("data/x.csv"),
            verified: true,
        };
        assert_eq!(format!("{disk}"), "disk:data/x.csv(verified)");

        let obj = Materialization::ObjectStore {
            ref_id: "abc".into(),
            node: Some("node-1".into()),
        };
        assert_eq!(format!("{obj}"), "objstore:abc@node-1");
    }

    #[test]
    fn materialization_set_display() {
        let out = OutputRef::File(PathBuf::from("out.csv"));
        let mut ms = MaterializationSet::new(out, 2);
        ms.add(Materialization::InMemory { pinned: false });
        ms.add(Materialization::OnDisk {
            path: PathBuf::from("out.csv"),
            verified: false,
        });
        let s = format!("{ms}");
        assert!(s.contains("memory"));
        assert!(s.contains("disk:out.csv"));
        assert!(s.contains("consumers=2"));
    }

    // ── ComputationHash tests ─────────────────────────────────────────

    #[test]
    fn computation_hash_display() {
        let hex = "cd".repeat(32);
        let h = ComputationHash::from_hex(hex.clone()).unwrap();
        assert_eq!(h.to_string(), hex);
        assert_eq!(h.as_str(), hex);
    }

    #[test]
    fn computation_hash_equality() {
        let h1 = cmph("aaa");
        let h2 = cmph("aaa");
        let h3 = cmph("bbb");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }

    #[test]
    fn computation_hash_serde_roundtrip() {
        let h = cmph("abc123");
        let json = serde_json::to_string(&h).unwrap();
        let parsed: ComputationHash = serde_json::from_str(&json).unwrap();
        assert_eq!(h, parsed);
    }

    // ── ExternalRef tests ─────────────────────────────────────────────

    #[test]
    fn external_ref_display() {
        let r = ExternalRef {
            uri: "s3://bucket/key".into(),
            check: Some("aws s3 ls".into()),
        };
        assert_eq!(r.to_string(), "ext:s3://bucket/key");
    }

    #[test]
    fn external_ref_no_check() {
        let r = ExternalRef {
            uri: "db://table".into(),
            check: None,
        };
        assert_eq!(r.to_string(), "ext:db://table");
        assert!(r.check.is_none());
    }

    #[test]
    fn external_ref_serde_roundtrip() {
        let r = ExternalRef {
            uri: "s3://bucket/key".into(),
            check: Some("check cmd".into()),
        };
        let json = serde_json::to_string(&r).unwrap();
        let parsed: ExternalRef = serde_json::from_str(&json).unwrap();
        assert_eq!(r, parsed);
    }

    // ── ArtifactIdentity tests ────────────────────────────────────────

    #[test]
    fn artifact_identity_content_display() {
        let id = ArtifactIdentity::Content(ch("abc"));
        assert_eq!(id.to_string(), format!("content:{}", ch("abc")));
    }

    #[test]
    fn artifact_identity_computation_display() {
        let id = ArtifactIdentity::Computation(cmph("def"));
        assert_eq!(id.to_string(), format!("computation:{}", cmph("def")));
    }

    #[test]
    fn artifact_identity_external_display() {
        let id = ArtifactIdentity::External(ExternalRef {
            uri: "db://t".into(),
            check: None,
        });
        assert_eq!(id.to_string(), "ext:db://t");
    }

    #[test]
    fn artifact_identity_serde_roundtrip_all_flavors() {
        let flavors = vec![
            ArtifactIdentity::Content(ch("hash1")),
            ArtifactIdentity::Computation(cmph("hash2")),
            ArtifactIdentity::External(ExternalRef {
                uri: "s3://b/k".into(),
                check: Some("ls".into()),
            }),
        ];
        for id in &flavors {
            let json = serde_json::to_string(id).unwrap();
            let parsed: ArtifactIdentity = serde_json::from_str(&json).unwrap();
            assert_eq!(*id, parsed, "roundtrip failed for {id}");
        }
    }

    #[test]
    fn artifact_identity_serde_tagged_format() {
        let id = ArtifactIdentity::Content(ch("abc"));
        let json = serde_json::to_string(&id).unwrap();
        assert!(json.contains("\"flavor\":\"content\""), "json: {json}");

        let id = ArtifactIdentity::Computation(cmph("def"));
        let json = serde_json::to_string(&id).unwrap();
        assert!(json.contains("\"flavor\":\"computation\""), "json: {json}");

        let id = ArtifactIdentity::External(ExternalRef {
            uri: "x".into(),
            check: None,
        });
        let json = serde_json::to_string(&id).unwrap();
        assert!(json.contains("\"flavor\":\"external\""), "json: {json}");
    }

    #[test]
    fn artifact_identity_hash_and_eq() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(ArtifactIdentity::Content(ch("a")));
        set.insert(ArtifactIdentity::Content(ch("a")));
        set.insert(ArtifactIdentity::Computation(cmph("a")));
        assert_eq!(set.len(), 2);
    }

    // --- Size tracking and eviction tests ---

    #[test]
    fn materialization_set_size_bytes() {
        let out = OutputRef::File(PathBuf::from("data.parquet"));
        let mut ms = MaterializationSet::new(out, 1);
        assert_eq!(ms.size_bytes(), 0);

        ms.set_size_bytes(1_048_576);
        assert_eq!(ms.size_bytes(), 1_048_576);
    }

    #[test]
    fn materialization_set_has_in_memory() {
        let out = OutputRef::File(PathBuf::from("data.csv"));
        let mut ms = MaterializationSet::new(out, 0);

        assert!(!ms.has_in_memory());

        ms.add(Materialization::OnDisk {
            path: PathBuf::from("data.csv"),
            verified: false,
        });
        assert!(!ms.has_in_memory());

        ms.add(Materialization::InMemory { pinned: false });
        assert!(ms.has_in_memory());
    }

    #[test]
    fn materialization_set_evict_in_memory() {
        let out = OutputRef::File(PathBuf::from("data.csv"));
        let mut ms = MaterializationSet::new(out, 0);

        ms.add(Materialization::OnDisk {
            path: PathBuf::from("data.csv"),
            verified: false,
        });
        ms.add(Materialization::InMemory { pinned: false });
        assert_eq!(ms.len(), 2);

        // Eviction should succeed (pending_consumers == 0, not last mat).
        assert!(ms.evict_in_memory());
        assert!(!ms.has_in_memory());
        assert_eq!(ms.len(), 1);
    }

    #[test]
    fn materialization_set_evict_pinned_refused() {
        let out = OutputRef::File(PathBuf::from("data.csv"));
        let mut ms = MaterializationSet::new(out, 0);

        ms.add(Materialization::OnDisk {
            path: PathBuf::from("data.csv"),
            verified: false,
        });
        ms.add(Materialization::InMemory { pinned: true });

        // Pinned materializations are immune to eviction.
        assert!(!ms.evict_in_memory());
        assert!(ms.has_in_memory());
    }

    #[test]
    fn materialization_set_evict_last_with_consumers_refused() {
        let out = OutputRef::File(PathBuf::from("data.csv"));
        let mut ms = MaterializationSet::new(out, 1);

        // Only in-memory materialization, pending consumers > 0.
        ms.add(Materialization::InMemory { pinned: false });

        // Eviction guard: can't remove last materialization with pending consumers.
        assert!(!ms.evict_in_memory());
        assert!(ms.has_in_memory());
    }

    #[test]
    fn materialization_set_artifact_meta() {
        let out = OutputRef::File(PathBuf::from("data.csv"));
        let mut ms = MaterializationSet::new(out, 1);

        // Initially no metadata.
        assert!(ms.artifact_meta().is_none());

        // Set metadata.
        let meta = ArtifactMeta::new([0xff; 32], 4096);
        ms.set_artifact_meta(meta);

        let got = ms.artifact_meta().unwrap();
        assert_eq!(got.content_hash, [0xff; 32]);
        assert_eq!(got.size_bytes, 4096);
    }

    // --- ReproducibilityClass tests ---

    #[test]
    fn reproducibility_class_default_is_deterministic() {
        assert_eq!(
            ReproducibilityClass::default(),
            ReproducibilityClass::Deterministic
        );
    }

    #[test]
    fn reproducibility_class_display() {
        assert_eq!(
            ReproducibilityClass::Deterministic.to_string(),
            "deterministic"
        );
        assert_eq!(
            ReproducibilityClass::SeedDeterministic.to_string(),
            "seed_deterministic"
        );
        assert_eq!(ReproducibilityClass::Approximate.to_string(), "approximate");
        assert_eq!(
            ReproducibilityClass::NonReproducible.to_string(),
            "non_reproducible"
        );
    }

    #[test]
    fn reproducibility_class_serde_round_trip() {
        for variant in [
            ReproducibilityClass::Deterministic,
            ReproducibilityClass::SeedDeterministic,
            ReproducibilityClass::Approximate,
            ReproducibilityClass::NonReproducible,
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            let parsed: ReproducibilityClass = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, variant);
        }
    }

    #[test]
    fn reproducibility_class_equality_and_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(ReproducibilityClass::Deterministic);
        set.insert(ReproducibilityClass::Approximate);
        assert!(set.contains(&ReproducibilityClass::Deterministic));
        assert!(!set.contains(&ReproducibilityClass::NonReproducible));
    }

    // --- ArtifactProvenance tests ---

    #[test]
    fn artifact_provenance_serde_round_trip() {
        let prov = ArtifactProvenance {
            input_hashes: vec![
                ("abc123".into(), "data/input.csv".into()),
                ("def456".into(), "data/ref.fa".into()),
            ],
            job_spec_hash: "spec_hash_789".into(),
            reproducibility: ReproducibilityClass::SeedDeterministic,
        };
        let json = serde_json::to_string(&prov).unwrap();
        let parsed: ArtifactProvenance = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, prov);
    }

    #[test]
    fn artifact_provenance_empty_inputs() {
        let prov = ArtifactProvenance {
            input_hashes: vec![],
            job_spec_hash: "empty_spec".into(),
            reproducibility: ReproducibilityClass::Deterministic,
        };
        let json = serde_json::to_string(&prov).unwrap();
        let parsed: ArtifactProvenance = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.input_hashes.len(), 0);
        assert_eq!(parsed.job_spec_hash, "empty_spec");
    }
}
