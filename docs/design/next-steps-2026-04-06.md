# Next Steps — Post Session 2026-04-06

> **Context:** This session completed Stage 2 (in-memory transport), Stage 5
> (warm workers), scheduler optimization (O(1) promote_downstream), and full
> benchmark suite (OxMake vs Dask). All code is merged to main.

---

## What Was Done This Session

- **Stage 2 complete:** `--memory-budget`, critical path gating, ArtifactMeta,
  provenance in SQLite, page-cache-aware skip, BLAKE3 deferred
- **Stage 5 prototype:** `--warm-workers fork|persistent`, fork-after-import +
  persistent dispatch modes, JAX compilation cache
- **Diamond fan-in fix:** O(1) `pending_upstream` counter → 27x speedup on
  diamond 50K, OxMake now beats Dask on all topologies
- **Benchmarks:** synthetic throughput, end-to-end numpy, pure scheduler (vs Dask)
- **Expert reviews:** 2 rounds × 4 experts, 8 critical bugs found and fixed
- **Coverage:** 84% workspace, proptest invariants, stress tests, mutation testing
- **Docs:** scheduler-performance.md, stage2 review findings, experiment reports

---

## Priority 1: Lock Consolidation in Dispatch Loop

**Issue:** 5 lock acquisitions per job in the dispatch loop (identified as R5).
At 100K+ tasks this becomes a measurable bottleneck.

**Fix:** Merge the 5 separate `state.lock().await` calls (force_rerun check,
cache skip, budget check, set Running, collect_input_data) into 1 acquisition.

**Effort:** 0.5 day. **Impact:** ~2x throughput at 100K+ scale.

**File:** `crates/ox-core/src/scheduler.rs` lines 460-540

---

## Priority 2: Warm Worker Pool (N workers, not just 1)

**Current state:** One warm worker per environment (sequential dispatch).
For embarrassingly parallel call-mode jobs, this serializes execution.

**Fix:** Pool of N workers per env. `try_acquire` pops a worker, dispatch
runs in it, `release` returns it. Pool size = min(max_jobs, cpu_count).

**Effort:** 1 day. **Impact:** Parallel warm dispatch for call-mode.

**File:** `crates/ox-exec-local/src/worker_pool.rs` — extend `workers`
from `HashMap<String, WarmWorker>` to `HashMap<String, Vec<WarmWorker>>`

---

## Priority 3: JIT Pre-Warmup with Synthetic Data

**Current state:** Design note at `docs/design/jax-jit-warmup.md`.
JAX JIT compilation adds 0.5-2s on first call per function shape.
The persistent worker keeps the JIT cache, but the first dispatch
still pays the compilation cost.

**Fix:** Add optional `[rule.X.warmup]` block in the Oxymakefile that
specifies a warmup function to call during `ensure_warm()`. This
function creates synthetic data matching expected shapes and calls
the target function once to trigger JIT.

**Effort:** 2-3 days (Oxymakefile parsing + warm worker protocol).

---

## ~~Priority 4: In-Process Execution (PyO3) — Design Phase~~

> **Rejected by design decision — violates ADR-003 without
> superseding it. Re-open only via a fresh ADR superseding ADR-003.**

~~**Current state:** Not started. The scheduler is 2-25x faster than Dask,
but end-to-end OxMake is 2-21x slower because of subprocess spawn.~~

~~**What to do next:** Design document for PyO3 integration. Key questions:~~
- ~~GIL management for parallel dispatch~~
- ~~Error isolation (PyO3 segfault kills the Rust process)~~
- ~~Codec zero-copy (pyarrow Buffer backed by Rust memory)~~
- ~~Incremental rollout: opt-in per rule via `backend = "pyo3"`~~

~~**Effort:** 1-2 weeks total. Start with a design doc + prototype.~~

---

## Priority 5: Dask as Executor Backend

**Insight from benchmarks:** Dask's strength is in-process numpy array
operations. OxMake's strength is heterogeneous workflow orchestration.

**Idea:** An `ox-exec-dask` crate that submits sub-DAGs of numpy-heavy
call-mode jobs to Dask's scheduler, getting the best of both worlds.
OxMake handles the outer DAG (caching, reproducibility, multi-executor),
Dask handles the inner compute-intensive array operations.

**Effort:** 1 week for prototype. **Impact:** Eliminates the end-to-end
gap for numpy workloads while keeping OxMake's orchestration benefits.

---

## Priority 6: Stage 4 — Per-Module Splitting

**Current state:** Design complete (internal design note, private residence).
Zero code risk, independent of other stages.

**What it does:** Splits the monolithic `features` job into 29 per-module jobs
(one per processing module). Combined with warm workers, each job runs in ~1s
instead of the current ~40s monolith.

**Effort:** 3 days (Oxymakefile restructuring + tests).
**Impact:** 2-3x speedup on feature computation stage.

---

## Benchmark Commands (for reference)

```bash
# Pure scheduler throughput (Rust, release)
cargo test -p ox-core --test scheduler_throughput --release -- --ignored --nocapture

# Dask scheduler throughput
python3 bench/dask_vs_ox/scheduler_throughput.py

# End-to-end numpy comparison
./bench/dask_vs_ox/compare.sh --quick

# Synthetic benchmark (subprocess overhead)
./bench/synthetic/run_benchmark.sh 50 1M

# Stress tests (eviction, memory accounting)
cargo test -p ox-core --test stress_scheduler -- --ignored --nocapture

# Coverage
cargo llvm-cov --workspace

# Lab pipeline
cd /path/to/your-pipeline
ox run -f pipeline/call/Oxymakefile-memory.toml -j4 --memory-budget 1G
ox run -f pipeline/call/Oxymakefile-memory.toml -j4 --warm-workers persistent
```

---

## Key Files Modified This Session

| File | What changed |
|------|-------------|
| `crates/ox-core/src/scheduler.rs` | Stage 2 plumbing, O(1) promote, memory stats, proptest |
| `crates/ox-core/src/model.rs` | MaterializationSet, ArtifactMeta, has_disk_fallback |
| `crates/ox-core/src/memory_map.rs` | Arc<[u8]> unification |
| `crates/ox-core/src/disk_writer.rs` | Flush doc fix |
| `crates/ox-exec-local/src/executor.rs` | Warm workers, page-cache skip |
| `crates/ox-exec-local/src/call_mode.rs` | Warmup script gen, dispatch payload, persistent mode |
| `crates/ox-exec-local/src/worker_pool.rs` | New: WorkerPool with fork/persistent modes |
| `crates/ox-cli/src/commands/run.rs` | --memory-budget, --warm-workers, memory stats |
| `crates/ox-plan/src/critical_path.rs` | compute() borrow-friendly method |
| `docs/design/scheduler-performance.md` | New: architecture + benchmark docs |
| `bench/dask_vs_ox/` | New: full benchmark suite |
| `bench/synthetic/` | New: transport overhead benchmark |
