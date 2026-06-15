# OxyMake vs Snakemake — head-to-head bench

_Generated 2026-06-10._

> **This is the single benchmark of record** cited by the paper
> (§6, `docs/paper/oxymake-paper.tex`) and the README. It is the
> harness a reviewer runs (`bash bench/snakemake-vs-oxymake/run.sh`);
> its output **is** the paper's numbers. The earlier DAG-resolution
> micro-bench under `benchmark/perf/` is a developer tool only — its
> generated `results.md` is git-ignored and is **not** a public
> numeric truth. Note: the DAG-resolution phase is timed with
> `hyperfine` (wrapper-free); end-to-end uses `/usr/bin/time`, whose
> process-spawn overhead is negligible at minutes scale but would
> otherwise swamp the sub-100 ms resolution phase.

### Why Snakemake 7.32.4

Snakemake 7.32.4 is the latest release of the 7.x line — the line pinned by the bioinformatics ecosystem the paper targets (it is the version resolved by a default `pip install snakemake` in the 7.x channel and the one bundled in most active Bioconda environments). It is the version installed on the bench host, so the numbers here are what a reviewer reproduces with the documented `pip install`. An earlier exploratory run used 9.21.0; it is **not** the record and is not committed, to avoid two numeric truths. Re-running on the 9.x line is tracked as future work; the headline ratios are dominated by the Rust-vs-Python resolver gap and are not expected to move materially across Snakemake minor versions, but that is a claim to verify, not assume.

## Reproducer

```bash
bash bench/snakemake-vs-oxymake/run.sh
```

Default scales: `100 1000 10000`. Override with `SIZES="… …" RUNS=N JOBS=N`.

## Hardware

`Darwin 25.5.0 arm64 | Apple M4 Max | 16 cores | 128 GiB`

Binaries:

- ox: `cargo install --path .` (from repo root)
- snakemake: `pip install snakemake` (7.x line; this run used 7.32.4)
- python3: 3.11+

> **Cross-platform reproduction status.** All numbers here are measured on the single Apple M4 Max host above (arm64). A re-run on a Linux/x86_64 host is **pending** — it is the highest-leverage anti-substitution check (it either earns a "reproduced on Linux/x86_64 within X%" sentence or surfaces an arch-specific gap before a reviewer does). Until that run lands, no cross-architecture claim is made.

## Workload

Synthetic 4-layer DAG (rule-fan + chain):

```
  seed (shell)
    │
    ▼
  gen_{i}        ← N shell rules (Layer 1)
    │
    ▼
  process_{i}    ← N Python-via-shell rules (Layer 2)
    │
    ▼
  finalize_{i}   ← N cp / file-only rules (Layer 3)
    │
    ▼
  merge (shell)
```

Total jobs at scale N = `3·N + 2`. The 10⁴ row corresponds to N=3333 (10001 jobs).

Both `workflow.toml` (OxyMake) and `workflow.smk` (Snakemake) declare the same
DAG and the same per-job work; only the orchestrator changes between runs.

## Results

### Headline (at 10,000 jobs, -j 16)

| Metric | Snakemake | OxyMake | Δ |
|---|---:|---:|---|
| DAG resolution (cold) | 2.31 s | 69 ms | OxyMake 33.29× faster |
| DAG resolution (warm) | 2.56 s | 27 ms | OxyMake 93.39× faster |
| End-to-end wall time (cold) | 1.6 min | 2.4 min | Snakemake 1.44× faster |
| End-to-end wall time (warm cache) | 2.81 s | 372 ms | OxyMake 7.54× faster |
| End-to-end warm re-run (hash mode) | 2.81 s | 698 ms | OxyMake 4.02× faster |
| Job submission throughput | 104 jobs/s | 71 jobs/s | Snakemake 1.47× faster |
| Peak RSS (e2e cold) | 184.7 MiB | 90.7 MiB | OxyMake 2.04× smaller |
| Cache decision correctness | minimal-rebuild | minimal-rebuild | equal |

### Scaling (cold end-to-end wall time)

| Jobs (target) | Snakemake | OxyMake | Speedup |
|---:|---:|---:|---:|
| 100 | 1.10 s | 1.37 s | 0.80× |
| 1,000 | 4.31 s | 9.74 s | 0.44× |
| 10,000 | 1.6 min | 2.4 min | 0.70× |

### Scaling (cold DAG resolution)

| Jobs (target) | Snakemake | OxyMake | Speedup |
|---:|---:|---:|---:|
| 100 | 418 ms | 4 ms | 101.88× |
| 1,000 | 512 ms | 10 ms | 50.68× |
| 10,000 | 2.31 s | 69 ms | 33.29× |

### Cache-decision correctness

Protocol: rebuild cleanly, then overwrite one Layer-1 input. Expected re-run scope: `process_i_000000` + `finalize_i_000000` + `merge` = **3 jobs**.

| Jobs | System | Jobs re-run | Expected | Status |
|---:|---|---:|---:|---|
| 100 | snakemake | 3 | 3 | ✓ minimal |
| 100 | ox | 3 | 3 | ✓ minimal |
| 1,000 | snakemake | 3 | 3 | ✓ minimal |
| 1,000 | ox | 3 | 3 | ✓ minimal |
| 10,000 | snakemake | 3 | 3 | ✓ minimal |
| 10,000 | ox | 3 | 3 | ✓ minimal |

### Content-addressing under git-checkout (mtime churn)

Protocol: build cleanly, then bump the mtime of the shared tracked input (`bench_lib.py`, the `lib` input of every `process` job) **without changing a byte** — exactly what `git checkout`, a tree copy, or a backup-restore does. A purely timestamp-based decision must re-run every job that reads the file; a content-addressed decision must re-run **zero**. Re-run radius if anything fires: `process` + `finalize` + `merge` = `2·N + 1` jobs.

| Jobs | Snakemake 7.32.4 | OxyMake (mtime, default) | OxyMake (`--cache-validation hash`) |
|---:|---:|---:|---:|
| 100 | 0 | 67 | 0 |
| 1,000 | 0 | 667 | 0 |
| 10,000 | 0 | 6667 | 0 |

**What the measurement shows — read this before citing it.**

- **Snakemake 7.32.4 does _not_ phantom-re-run on mtime churn.** Its default rerun-triggers record per-output provenance (code, params, input set, software-env) rather than comparing live input-vs-output mtimes; a `touch` — or a far-future timestamp — leaves it at zero re-runs. The "a git checkout re-runs the whole campaign" failure mode is **not** exhibited by this version. Treat any prose that asserts it is (including the paper introduction) as unverified against the benchmarked version.
- **OxyMake's mtime fast-path (the default) _is_ fooled** by the churn — it re-runs the full `2·N + 1` radius, because the cheap path trusts the timestamp it is named for.
- **`--cache-validation hash` restores correctness** — zero re-runs, because the BLAKE3 key hashes content, not time. This buys **parity with Snakemake's robustness plus cross-machine / cross-cache portability** (where mtimes are meaningless), and it protects OxyMake's own mtime-default users. It does **not** demonstrate superiority over Snakemake on this scenario for the benchmarked version.

![Scaling](scaling.pdf)

`scaling.pdf` plots cold end-to-end wall time and cold DAG resolution time, on a log-log scale.

## Falsifier (from pre-mortem 2026-05-27 item #2)

- Bench closed (reproducible) **and** OxyMake competitive or better at 10⁴ jobs ⇒ Path β systems-first wave 2 viable.
- Bench closed **and** OxyMake worse than Snakemake at 10⁴ jobs ⇒ Path β collapses on the systems-first half; FAIR-first stands alone. The bench result is itself a falsifier of the assumed perf advantage.
- Bench **not** closed by 2026-09-30 ⇒ pre-mortem failure mode #1 reasserted; systems-first paper cancelled.

## Notes

- Per-job work is intentionally tiny (echo / cp / one Python line). This is *deliberate*: at 10⁴ jobs the orchestration overhead is the variable under test, not user compute.
- The `-j` parallelism is the same for both systems.
- DAG resolution is measured with `ox plan` and `snakemake --dryrun`.
- Peak RSS is the maximum-resident-set-size of the parent orchestrator process (children are sampled by macOS/Linux `time` separately and not aggregated here).
- Cache-decision correctness is checked by **rewriting the content** of one Layer-1 input and confirming both systems schedule only the downstream branch.
- The git-checkout / mtime-churn section bumps an input's mtime **without** changing content; it is the test that distinguishes timestamp-based from content-addressed decisions. Read its findings note before citing content-addressing as an advantage over Snakemake.
- 100K-job scaling is **out of scope** for this wave (an honest scope downgrade recorded in the §D1 design synthesis).

