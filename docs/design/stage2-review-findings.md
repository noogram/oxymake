# Stage 2 Code Review — Consolidated Findings

> **Date:** 2026-04-06
> **Reviewers:** tolnay (API), architect (systems), knuth (correctness), forgemaster (engineering)
> **Scope:** All Stage 2 in-memory critical path code

---

## Summary

Four expert reviews identified 25+ findings across API surface, architecture,
algorithmic correctness, and engineering quality. All critical bugs were fixed.

## Critical Bugs Fixed

| # | Finding | Reviewer | Fix |
|---|---------|----------|-----|
| 1 | Key mismatch: spawned task uses `p.display()`, scheduler uses `output_ref_key()` — data silently lost on non-UTF-8 paths | architect | Use `output_ref_key()` in spawned task |
| 2 | Ghost InMemory: Never-policy without data registers phantom materialization, causing silent missing-input failures | knuth | Don't register InMemory without actual data |
| 3 | OutputMemoryMap leak: off-path outputs stored by spawned task never cleaned up | architect | Cleanup in register_materializations |
| 4 | `Arc<Vec<u8>>` vs `Arc<[u8]>` type mismatch: gratuitous copy on every drain | tolnay, forgemaster | Unified to `Arc<[u8]>` everywhere |
| 5 | `ReproducibilityClass` unknown default: `Deterministic` (most permissive) instead of `NonReproducible` | knuth | Changed to `NonReproducible` |

## Warnings Addressed

| # | Finding | Reviewer | Resolution |
|---|---------|----------|------------|
| 6 | `set_size_bytes` public — can diverge from ArtifactMeta | tolnay | Made `pub(crate)` |
| 7 | `memory_used_bytes` soft limit, not hard ceiling | knuth | Documented as intentional |
| 8 | `Never` + `budget=0` accumulates unbounded memory | knuth | Documented as by-design |
| 9 | Blocking `std::fs::read` under async scheduler lock | forgemaster | Documented as legacy-only fallback |
| 10 | Test `memory_map_populated_on_job_success` asserts nothing useful | forgemaster | Improved with honest documentation |

## Deferred (Future Work)

| # | Finding | Reviewer | Rationale |
|---|---------|----------|-----------|
| 11 | `OutputKey` newtype for String keys | tolnay, architect | High churn, low immediate risk |
| 12 | `SchedulerRunOptions` struct (10 positional args) | tolnay | Semver concern, defer to next major |
| 13 | Unify `input_data` + `memory_map` in ExecContext | tolnay | Architectural change, defer to Stage 2.5 |
| 14 | O(n) eviction scan — use heap for large DAGs | forgemaster | Acceptable at current scale |
| 15 | `DiskWriterStats` fields pub — should have read-only accessors | tolnay | Nit, defer |
| 16 | Double-write per input (prepare_workspace + execute) | forgemaster | Minimal overhead, not worth complexity |
| 17 | `DiskWriteError` variant instead of stringly-typed errors | tolnay | Defer to error refactor |

## Mutation Testing Results

| Crate | Caught | Missed | Unviable | Timeout | Kill Rate |
|-------|--------|--------|----------|---------|-----------|
| ox-plan (critical_path.rs) | 5 | 2→1 | 4 | 1 | 83% |
| ox-core (model.rs + memory_map.rs + disk_writer.rs) | 134 | 35 | 17 | 1 | 79% |

## Commits

1. `ac99392` — feat: complete Stage 2 plumbing
2. `860a2bb` — feat: finalize executor in-memory integration
3. `9f770c2` — fix: review-driven improvements (zero-copy, mutation kills)
4. `6bbe4b6` — fix: review consolidation — Arc<[u8]> unification
5. `e010a19` — fix: address full expert review — key mismatch, ghost, leak
