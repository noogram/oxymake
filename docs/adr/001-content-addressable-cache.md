# ADR-001: Content-Addressable Cache as Source of Truth

## Status
Accepted (default validation strategy changed to mtime by ADR-006, then to mtime+hash by the 2026-06-10 amendment of ADR-006)

> **Vocabulary note (2026-05-27, M1 vocabulary alignment).** The word
> *Cache* historically named two distinct objects: the bytes
> (content-addressed store) and the strategy (validation policy). They are
> now disambiguated as
>
> - **CacheStore** — the content-addressed bytes and the rule that maps
>   inputs to a cache key. This ADR governs the **CacheStore**.
> - **CachePolicy** — the validation strategy (mtime / mtime+hash / hash)
>   that decides whether stored outputs are still valid. See ADR-006.
>
> Existing Rust symbols (`CacheStore`, `CacheValidation`) already carry
> these meanings ; this note pins the vocabulary so cross-ADR references
> stay precise.

## Context
Snakemake uses file modification timestamps (mtime) as the primary mechanism
to decide if a job needs re-running. This causes phantom re-runs when:
- `git checkout` resets mtime to current time
- `cp` without `-p` resets mtime
- NFS/distributed filesystems have clock skew
- CI/CD creates fresh clones

## Decision
OxyMake uses content hashing (blake3) as the **source of truth** for change
detection. The cache key is:

```
blake3(rule_source_hash + sorted(input_content_hashes) + params_hash + env_spec_hash + platform)
```

Timestamps serve as a **fast-path optimization only**: if mtime + size are
unchanged since the last recorded run, skip the hash computation. But when
mtime differs, we hash to check if content actually changed.

## Consequences
- No phantom re-runs from git operations, file copies, or clock skew
- Cache is shareable across machines (same content = same hash)
- Slightly more I/O on first run (must hash all inputs)
- Cache key must include ALL dimensions from day 1 (adding one later invalidates entire cache)

## Alternatives Considered
- **Timestamps only** (Snakemake): simpler but causes phantom re-runs
- **Checksums only** (no mtime fast-path): correct but slower for large unchanged files
- **Git-based tracking**: too coupled to version control, not all workflows use git
