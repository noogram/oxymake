# Stage 2 Review Round 2 — Consolidated Expert Findings & Test Strategy

> **Date:** 2026-04-06
> **Reviewers:** tolnay (API+tests), architect (HPC+experimentation), knuth (correctness+formal), forgemaster (engineering+usability)
> **State:** Post all previous fixes (Arc unification, ghost InMemory, key mismatch, disk writer hang, Never-policy data loss)

---

## PART A: Remaining Code Issues

### Already Fixed During This Review

| # | Severity | Finding | Reviewer | Status |
|---|----------|---------|----------|--------|
| R1 | **HIGH** | Never-policy gets spurious OnDisk → stale read after eviction | tolnay | **Fixed** (d17b7e9) |
| R2 | **HIGH** | enforce_memory_budget evicts Never outputs with no disk fallback | tolnay | **Fixed** (d17b7e9) |

### Remaining (to fix in next sprint)

| # | Severity | Finding | Reviewer | Action |
|---|----------|---------|----------|--------|
| R3 | **MEDIUM** | `let _ = writer.write(req).await` silently swallows channel-close errors | tolnay, forgemaster | Log warning; distinguish channel-close from per-file I/O |
| R4 | **MEDIUM** | DiskWriter stats never surfaced — user can't verify feature is active | forgemaster | Print memory watermark + eviction count at end of run |
| R5 | **MEDIUM** | Lock convoy: 5 lock acquisitions per job in dispatch loop | architect | Consolidate to 1 acquisition per job |
| R6 | **MEDIUM** | memory_used_bytes understates true RSS (disk writer holds Arc clones) | architect | Document 60-70% budget guideline; track pending_disk_bytes |
| R7 | **LOW** | Blocking std::fs::read fallback under async lock | architect, forgemaster | Add debug_assert!(memory_map.is_some()) |
| R8 | **LOW** | DiskWriterHandle::flush documented but doesn't exist | tolnay | Update doc or add flush() method |
| R9 | **LOW** | Critical path uses unit weights, not duration estimates | knuth | Design limitation; document clearly |
| R10 | **LOW** | Dead code with allow(dead_code) — enqueue_disk_write etc. | tolnay | Move to #[cfg(test)] or remove |

---

## PART B: Automated Test & Benchmark Strategy

### B1. Property-Based Tests (proptest) — Correctness

| Test | Invariant | Generator | Priority |
|------|-----------|-----------|----------|
| **budget_accounting** | `memory_used_bytes == sum(data.len() in memory_store)` | Random DAG (5-50 jobs), random sizes, simulate register→consume→evict | P0 |
| **eviction_safety** | No output with pending_consumers > 0 is evicted | Same DAG, assert post-eviction snapshot | P0 |
| **eviction_disk_fallback** | Evicted outputs always have OnDisk | Same DAG with mixed Never/Auto policies | P0 |
| **hash_integrity** | `blake3(memory_store[key]) == artifact_meta.hash` | Random byte vectors through register_materializations | P1 |
| **critical_path_optimal** | `cp.len() == brute_force_max_path_len` | Random DAGs ≤20 nodes, DFS enumeration | P1 |
| **scheduler_liveness** | All jobs reach terminal state | Random DAGs, mock executor, parallelism=1 | P1 |

### B2. Integration Tests — Full Data Flow

| Test | What it validates | Implementation |
|------|-------------------|----------------|
| **full_pipeline_memory_path** | 3-job chain: data flows memory→memory→disk | Tempdir + real Oxymakefile + LocalExecutor |
| **never_policy_no_disk** | Never output exists only in memory, downstream reads it | Tempdir + assert file does NOT exist on disk |
| **diamond_memory_reuse** | A→{B,C}→D: A's output survives until both B,C fire | Tempdir + assert eviction happens after D |
| **cache_correctness_roundtrip** | Run 1 executes, run 2 cache-hits with provenance | Two sequential ox runs, assert 0 executed on run 2 |
| **ctrl_c_cleanup** | No .oxytmp files after SIGINT | Spawn ox with sleep job, SIGINT, check temps |

### B3. CLI Usability Tests (assert_cmd)

| Test | Assertion | Priority |
|------|-----------|----------|
| `--memory-budget invalid` → clear error | stderr contains "unknown size suffix" | must-have |
| `--memory-budget 0` = no flag | identical output to no flag | must-have |
| `--memory-budget 1G` prints stats | stderr contains memory watermark | must-have (drives R4) |
| eviction logged when budget tight | stderr contains eviction notice | nice-to-have |

### B4. Benchmark Suite (criterion)

| Benchmark | What it measures | Expected |
|-----------|------------------|----------|
| **materialization_overhead** | MaterializationSet lifecycle per output | < 200ns |
| **blake3_throughput** | Hashing cost at 1KB→64MB | ~5 GB/s (negligible) |
| **eviction_scan** | O(N) scan for N=100→100K outputs | Baseline for heap optimization |
| **e2e_memory_vs_disk** | 10-job chain, 1MB outputs, budget=0 vs 100MB | Measure actual speedup |

### B5. Stress Tests (#[ignore], nightly CI)

| Test | Parameters | What it catches |
|------|------------|-----------------|
| **high_fanout_diamond** | 1→1000→1, 50KB outputs, 10MB budget | Eviction churn, lock contention |
| **10k_jobs_tight_budget** | 10K linear, 50KB each, 100MB budget | Accounting drift, O(N²) eviction |
| **never_accumulation** | 1000 Never jobs, no budget | Unbounded memory growth (by design) |
| **concurrent_disk_writer** | 10K writes, 1MB each, buffer=64 | Backpressure, flush correctness |

### B6. Synthetic Pipeline Generator

`ox-bench-gen` tool with parameters:
```
--jobs N --fanout F --depth D --output-size S
--shape {linear|diamond|wide|tree}
--mix shell:0.7,call:0.3
--policy auto:0.6,never:0.2,always:0.2
```

### B7. A/B Comparison Framework

Protocol: same DAG, same machine, 10 runs, discard 2 warmup, Mann-Whitney U test.
```bash
ox run -j8 --memory-budget 0    --metrics disk.ndjson
ox run -j8 --memory-budget 512M --metrics memory.ndjson
```
Alert: >10% wall-time regression, any output hash divergence.

### B8. Instrumentation Metrics (--metrics flag)

| Metric | Source | Purpose |
|--------|--------|---------|
| memory_watermark_bytes | enforce_memory_budget | Budget validation |
| eviction_count/bytes | enforce_memory_budget | Pressure measurement |
| register_mat_lock_us | handle_completion timer | Lock contention |
| disk_writer_queue_depth | channel len | Backpressure |
| input_data_hit_rate | collect_input_data | Stage 2 effectiveness |
| peak_rss_bytes | getrusage | True memory validation |

---

## PART C: Implementation Priority

### Sprint 1: Correctness (1-2 days)
- [ ] R3: Log disk-write channel errors (not silent `let _ =`)
- [ ] R7: debug_assert for blocking fs::read path
- [ ] P0 proptest: budget_accounting, eviction_safety, eviction_disk_fallback

### Sprint 2: Observability (1-2 days)
- [ ] R4: Print memory watermark + eviction stats at end of run
- [ ] R8: Fix DiskWriterHandle::flush doc
- [ ] CLI usability tests (assert_cmd)

### Sprint 3: Performance (2-3 days)
- [ ] R5: Lock consolidation in dispatch loop
- [ ] criterion benchmarks (materialization_overhead, blake3, eviction_scan, e2e)
- [ ] Stress tests (high_fanout, 10k_jobs)

### Sprint 4: Experimentation (2-3 days)
- [ ] ox-bench-gen synthetic pipeline generator
- [ ] --metrics instrumentation flag
- [ ] A/B comparison framework
- [ ] Regression detection in CI
