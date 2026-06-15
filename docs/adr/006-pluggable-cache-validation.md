# ADR-006: Pluggable Cache Validation Strategies

## Status
Accepted — **amended 2026-06-10: default changed from `mtime` to `mtime+hash`**

> **Amendment (2026-06-10, security premortem).** The built-in default validation strategy is
> now **`mtime+hash`**, not `mtime`. The stateless mtime check declares a
> hit on existence + newer-mtime alone — it never reads content, so a
> same-size corruption with a later mtime is served from a shared cache as
> a hit, contradicting SECURITY.md's content-addressable contract.
> `mtime+hash` closes that hole at near-zero cost on genuinely unchanged
> files (the hash only fires when metadata differs). `mtime` remains
> available as an explicit opt-in (`--cache-validation=mtime`) for
> Make/Snakemake-parity benchmarking on single-user local caches; it must
> never be used on a shared or multi-user cache. The three-strategy
> pluggable surface and precedence chain below are unchanged; read
> occurrences of "`mtime` (default)" in the historical text below as
> "`mtime` (opt-in)". Operator decision recorded in the task molecule.

## Metadata

- **Kind:** `decision`
- **Family:** `CAS`
- **Supersedes:** `ADR-001` (partial — changes the default validation
  strategy from content-hash to mtime ; the content-addressable cache key
  itself is unchanged. **Amended 2026-06-10:** default is `mtime+hash`)

> **Vocabulary note (2026-05-27, M1 vocabulary alignment).** The word
> *Cache* is split:
>
> - **CacheStore** (ADR-001) — the content-addressed bytes and key.
> - **CachePolicy** — the validation strategy that decides whether stored
>   outputs are still valid. This ADR defines the **CachePolicy** surface
>   (the Rust type carries the name `CacheValidation` for backward
>   source-compat ; consider it the operational form of *CachePolicy*).
>
> Reading: the CacheStore says *what is stored*, the CachePolicy says
> *under which observation it still counts as valid*.

## Context

OxyMake's cache validation always computes BLAKE3 content hashes of every output
file to determine cache hits (see ADR-001). This guarantees correctness but
causes measurable overhead: a fully-cached 35-job run takes ~15 seconds, far
above the sub-100ms target for no-op runs.

The bottleneck is I/O-bound: every `is_cached` call reads output files to hash
them, even when timestamps haven't changed. The existing `mtime_matches`
fast-path in `ox-cache/src/hash.rs` is defined but **never consulted** in the
hot path — `CacheStore::is_cached` and `CacheStore::check_cached` always call
`hash_file` unconditionally (see `lookup.rs:224` and `lookup.rs:282`).

Different workflows have fundamentally different correctness/speed tradeoffs:

| Use case | Need | Tolerance for false hits |
|----------|------|--------------------------|
| Interactive development | Sub-100ms response | Tolerant (user can `--no-cache`) |
| CI reproducibility audit | Bit-exact correctness | Zero |
| Large file pipelines (genomics, ML) | Avoid hashing multi-GB files | Moderate |
| Shared/remote cache | Deterministic keys | Zero |

A single strategy cannot serve all of these well.

## Decision

### 1. Introduce a `CacheValidation` enum

```rust
/// How output files are validated against cache entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CacheValidation {
    /// Check mtime + size only. O(1) stat calls per output.
    /// Fast but can miss same-size edits within the same second.
    /// Matches Make/Snakemake default behavior.
    #[default]
    Mtime,

    /// Check mtime + size first; if they differ, compute BLAKE3 hash.
    /// This is the sweet spot: fast on unchanged files, correct on changed ones.
    /// Only misses the pathological case where content changes but mtime+size
    /// are preserved (extremely rare outside adversarial scenarios).
    MtimeHash,

    /// Always compute BLAKE3 hash, ignoring mtime.
    /// Guarantees bit-exact correctness. Required for shared/remote caches
    /// and CI reproducibility audits.
    ContentHash,
}
```

### 2. Wire the strategy into `CacheStore`

Add a `validation: CacheValidation` field to `CacheStore`. The `is_cached` and
`check_cached` methods branch on this field:

```rust
// In CacheStore::is_cached / check_cached, for each output path:
match self.validation {
    CacheValidation::Mtime => {
        // Only stat: check mtime_secs + size against stored values.
        // If mismatch or no stored metadata → miss.
        let (mt, sz) = hash::file_meta(path)?;
        let (cached_mt, cached_sz) = stored_mtime_size;
        if mt != cached_mt || sz != cached_sz { return miss; }
    }
    CacheValidation::MtimeHash => {
        // Fast path: if mtime+size match, skip hashing.
        let (mt, sz) = hash::file_meta(path)?;
        let (cached_mt, cached_sz) = stored_mtime_size;
        if mt == cached_mt && sz == cached_sz { continue; } // hit
        // Slow path: mtime changed, verify content hash.
        let actual = hash::hash_file(path)?;
        if actual != expected_hash { return miss; }
    }
    CacheValidation::ContentHash => {
        // Always hash. Current behavior.
        let actual = hash::hash_file(path)?;
        if actual != expected_hash { return miss; }
    }
}
```

### 3. Configuration surface

**Built-in default:** `mtime` — stateless, fast, matches Make/Snakemake behavior.

**Oxymakefile.toml** (project-level):

```toml
[config]
cache_validation = "mtime+hash"   # or "mtime" (default), "hash"
```

**Environment variable:**

```bash
export OX_CACHE_VALIDATION=hash   # strict mode for CI
```

**User global config** (`~/.config/oxymake/config.toml`):

```toml
cache_validation = "mtime+hash"
```

**CLI override** (per-invocation):

```bash
ox run                              # uses default (mtime)
ox run --cache-validation=mtime+hash  # hybrid mode
ox run --cache-validation=hash      # strict mode for CI
ox run --no-cache                   # disable cache entirely (existing)
```

**Precedence** (highest wins):
1. `--cache-validation=<strategy>` CLI flag
2. `OX_CACHE_VALIDATION` environment variable
3. `[config] cache_validation` in Oxymakefile.toml
4. User global config: `~/.config/oxymake/config.toml`
5. Built-in default: `mtime`

### 4. Change the default to `mtime`

The built-in default is `Mtime` — pure filesystem metadata, stateless (no
`.oxymake/` dependency). This matches what Make/Snakemake users expect and
delivers sub-100ms no-op runs.

Reasons for `mtime` as default:
- **Familiarity**: Make/Snakemake users expect mtime-based invalidation
- **Speed**: ~10ms for 35 stat calls vs 3.7s+ for hashing
- **Stateless**: No dependency on `.oxymake/` directory
- **Sufficient**: Correct for the vast majority of workflows

Users who need stronger guarantees can opt in:
- `--cache-validation=mtime+hash` for the hybrid (fast + hash fallback)
- `--cache-validation=hash` for bit-exact correctness (CI, shared cache)
- `OX_CACHE_VALIDATION=hash` in CI environments

### 5. Do NOT implement custom/callback validation (yet)

The bead description mentions a user-defined callback strategy (e.g., checking
npz array shapes). This is deferred because:

- No concrete use case has been requested via an actual workflow
- The trait-based `CacheCheck` interface already allows plugging in arbitrary
  logic at the scheduler level
- A callback API adds serialization, error handling, and trust boundaries
  that aren't justified without demand

If needed later, the `CacheValidation` enum can be extended with a
`Custom(Box<dyn Fn(&Path) -> bool>)` variant without breaking existing code.

## Implementation Plan

### Phase 1: Activate the mtime fast-path (high impact, low risk)

**Files changed:** `crates/ox-cache/src/lookup.rs`

1. Add `CacheValidation` enum to `ox-cache` (new file: `strategy.rs`)
2. Add `validation: CacheValidation` field to `CacheStore`
3. Modify `CacheStore::is_cached` and `check_cached` to branch on strategy
4. Default to `MtimeHash` — this alone should reduce the 35-job no-op from
   ~15s to <500ms (just stat calls + rare hash verification)
5. Update `CacheStore::open` to accept a `CacheValidation` parameter

### Phase 2: Configuration plumbing

**Files changed:** `crates/ox-format/src/parse.rs`, `crates/ox-cli/src/commands/run.rs`

1. Add `cache_validation` to the `[config]` section in `ox-format`
2. Add `--cache-validation` CLI flag to `RunArgs`
3. Thread the resolved strategy through to `CacheStore::open`
4. Update `SchedulerCache::new` to pass the strategy

### Phase 3: Documentation and migration

1. Update `docs/book/src/concepts/cache.md` to describe the three strategies
2. Update ADR-001 status to "Superseded by ADR-006" (for the default change)
3. Add a note to the changelog about the default change

## Performance Expectations

| Strategy | 35-job fully-cached (estimated) | Correctness |
|----------|--------------------------------|-------------|
| `mtime` (default) | ~10ms (35 stat calls) | Vulnerable to same-size-same-second edits |
| `mtime+hash` | ~50-200ms (35 stats, ~0 hashes on steady state) | Correct for all practical scenarios |
| `hash` | ~15s (35 full file reads + hash) | Bit-exact |
| `--no-cache` | N/A (all jobs re-execute) | N/A |

The mtime+hash strategy achieves the sub-100ms target on steady-state runs
(where mtime hasn't changed) while maintaining content-hash correctness when
files are modified.

## Alternatives Considered

### blake3 replacing sha256
Not applicable — the codebase already uses BLAKE3 exclusively (no sha256). The
bead description was based on outdated assumptions. No change needed.

### `mtime+hash` as default
Initially chosen as the default (balancing correctness and speed), but changed
to `mtime` after user feedback. The `mtime+hash` hybrid adds hash I/O overhead
that isn't needed for the common case. Users who need hash verification can
opt in via config or env var.

### Per-rule validation strategy
Deferred. Allowing `cache_validation` per-rule (e.g., mtime for cheap rules,
hash for expensive ones) adds complexity without clear demand. The global
setting with CLI override covers the known use cases. Per-rule can be added
later by extending the rule schema.

### Remote cache interaction
The `mtime` and `mtime+hash` strategies are **local-only** — remote caches
always use `ContentHash` because mtime is not meaningful across machines. This
is enforced automatically: when `--cache-remote` is specified, validation is
promoted to `ContentHash` regardless of the configured strategy. This rule
should be documented clearly.

## Open Questions

1. **Should `mtime+hash` update stored mtime after a hash-verified hit?**
   If a file's mtime changed but content didn't (e.g., after `git checkout`),
   should we update the stored mtime so the next check uses the fast path?
   Recommendation: yes — add an `update_mtime` call after hash verification to
   prevent repeated hashing of the same unchanged file.

2. **Nanosecond mtime precision?** The current `file_meta` uses second-granularity
   (`as_secs()`). Modern filesystems support nanosecond mtime. Using nanos would
   reduce the (already tiny) false-positive window for `mtime+hash`. This is a
   minor follow-up that doesn't affect the strategy design.

3. **Interaction with materialization `auto`/`never` policies?** Outputs with
   `materialize = "never"` are never written to disk, so cache validation is
   N/A. Outputs with `materialize = "auto"` may or may not exist — the strategy
   should gracefully handle missing outputs (treat as cache miss, not error).
