# OxyMake Experiment Results

**Date**: 2026-04-01
**Hardware**: Apple M4 Max, 128 GB RAM, 16 cores
**OS**: macOS (Darwin 24.6.0, arm64)
**OxyMake version**: 0.1.0
**Binary**: Mach-O 64-bit executable arm64
**Timing method**: Python time.perf_counter, 5 runs each, median reported

---

## Experiment 1: DAG Resolution Performance

Synthetic workflows with N independent copy rules. Each rule copies one
input to one output. Measured with `ox run --dry-run` (full DAG resolution,
no execution).

| N (jobs) | Median (ms) | Run 1 | Run 2 | Run 3 | Run 4 | Run 5 |
|----------|-------------|-------|-------|-------|-------|-------|
| 10       | 7           | 11.1  | 6.4   | 7.7   | 5.9   | 6.6   |
| 100      | 7           | 7.7   | 7.4   | 7.4   | 7.0   | 7.1   |
| 1,000    | 13          | 13.3  | 15.5  | 11.5  | 11.9  | 13.1  |
| 5,000    | 54          | 65.6  | 42.6  | 37.6  | 63.7  | 54.0  |
| 10,000   | 102         | 130.9 | 94.3  | 102.1 | 75.4  | 112.0 |

### Observations

- Resolution scales sub-linearly across two orders of magnitude (10 to 10K jobs).
- At small scales, binary startup dominates (~7 ms); at 10K jobs wildcard expansion cost becomes visible.
- At 10K jobs: 102 ms (5x under the 500 ms target).
- All design targets met with significant headroom.

---

## Experiment 3: Startup Time

> **Superseded on 2026-08-27** by "Re-measurement at commit `1a4f724`"
> below. The table in this section was taken on the 52,514-SLOC,
> 23-crate tree of 2026-04-01 under a Python `subprocess` harness whose
> spawn overhead is included in each sample.

| Command                  | Median (ms) | Run 1 | Run 2 | Run 3 | Run 4 | Run 5 |
|--------------------------|-------------|-------|-------|-------|-------|-------|
| `ox --help`              | 6           | 5.6   | 7.5   | 7.3   | 5.9   | 6.3   |
| `ox lint` (10 rules)     | 7           | 9.6   | 6.0   | 6.7   | 8.6   | 4.8   |
| `ox lint` (1,000 rules)  | 7           | 7.4   | 7.4   | 5.6   | 7.7   | 7.4   |

### Observations

- Startup consistently 6-7 ms regardless of workflow size.
- Well under the 200 ms target (29x headroom).
- Native Rust binary eliminates Python/JVM startup overhead entirely.

---

## Experiment 4: Binary Size

> **Superseded on 2026-08-27** by "Re-measurement at commit `1a4f724`"
> below; the figures here describe the 2026-04-01 tree.

| Metric              | Value               |
|---------------------|---------------------|
| Binary size         | 14.9 MB             |
| Binary type         | Mach-O 64-bit arm64 |
| Release build time  | 71 s                |
| Target              | < 20 MB             |

---

## Implementation Metrics (current codebase)

| Metric                | Value              |
|-----------------------|--------------------|
| Total Rust SLOC       | 52,514             |
| Number of crates      | 23                 |
| Unit/integration tests| 1,275              |
| Doc tests (/// ```)   | 55                 |
| Total test functions  | 1,330              |
| Tests passing         | 1,330 (cargo test) |
| Git commits           | 336                |
| Documentation files   | 77                 |
| Development start     | 2026-03-24 22:40   |

### Lines of Code per Crate

| Crate             | Rust SLOC |
|-------------------|-----------|
| ox-core           | 12,423    |
| ox-cli            | 8,894     |
| ox-exec-ray       | 6,320     |
| ox-translate      | 4,932     |
| ox-state          | 3,221     |
| ox-exec-local     | 3,171     |
| ox-format         | 3,120     |
| ox-exec-slurm     | 2,267     |
| ox-cache          | 1,851     |
| ox-monitor-tui    | 1,265     |
| ox-mcp            | 1,097     |
| ox-report-term    | 1,076     |
| ox-dashboard      | 1,062     |
| ox-lock           | 644       |
| ox-api            | 611       |
| ox-cache-remote   | 442       |
| ox-report-json    | 421       |
| ox-codec-core     | 323       |
| ox-plan           | 289       |
| ox-metrics        | 273       |
| Other (3 stubs)   | 10        |
| **Total**         | **53,712**|

---

## Performance Summary

| Benchmark                    | Result     | Target          | Status |
|------------------------------|------------|-----------------|--------|
| DAG resolution @ 1K jobs     | 13 ms      | < 100 ms        | PASS   |
| DAG resolution @ 10K jobs    | 102 ms     | < 500 ms        | PASS   |
| Startup time                 | 6-7 ms     | < 200 ms        | PASS   |
| Binary size                  | 14.9 MB    | < 20 MB         | PASS   |
| Test suite                   | 1,330 pass | —               | PASS   |
| Feature pass rate (CLI)      | 10/10      | —               | PASS   |

---

## Re-measurement at commit `1a4f724` (2026-08-27)

**Date**: 2026-08-27
**Commit**: `1a4f724998eba2b133b24540a1ca828daf1a1477`
**Hardware**: Apple M4 Max, 128 GB RAM, 16 cores
**OS**: macOS (Darwin 25.5.0, arm64)
**Tree measured**: 64,596 Rust SLOC, 25 crates, 1,928 tests (`metrics.tex`)
**Toolchain**: stable `cargo build --release`, default features

Experiments 3 and 4 above were taken on 2026-04-01, on a 52,514-SLOC,
23-crate tree with 1,330 tests — a materially different tree from the one
this paper describes and evaluates. All three figures (startup, binary
size, release build time) were therefore re-measured at the current HEAD.

### Startup time

Method: `hyperfine -N --warmup 10 --runs 200` (shell bypassed, so no
`sh -c` spawn is included). Medians:

| Command                  | Median (ms) | Mean ± σ (ms) | Min (ms) |
|--------------------------|-------------|---------------|----------|
| `ox --help`              | 2.21        | 2.2 ± 0.1     | 1.92     |
| `ox lint` (10 rules)     | 2.37        | 2.4 ± 0.3     | 1.96     |
| `ox lint` (1,000 rules)  | 5.85        | 6.1 ± 1.8     | 5.50     |

The two `ox lint` fixtures are synthetic Oxymakefiles of 10 and 1,000
single-input/single-output copy rules, linted via `ox lint -f <path>`.

The change relative to the 6–7 ms of 2026-04-01 is not attributable to
the engine alone: the earlier harness measured `time.perf_counter`
around a Python `subprocess` call and includes its spawn cost. The two
methods are not directly comparable; the numbers above are the ones the
paper quotes.

### Binary size and build time

| Metric                                    | Value                     |
|-------------------------------------------|---------------------------|
| Binary size (release, default features)   | 17,421,504 bytes (17.4 MB / 16.6 MiB) |
| Binary type                               | Mach-O 64-bit executable arm64 |
| Full release build time (clean tree)      | 25.7 s                    |

Build time is the wall clock (`/usr/bin/time -p`) of `cargo build
--release` over the workspace immediately after `cargo clean --release`,
with the dependency sources already present in the Cargo registry cache;
it excludes network fetch. `cargo build --release --bin ox` alone, under
the same conditions, took 27.4 s. The paper quotes 26 s.

The 2026-04-01 figure of 71 s was recorded without a stated method on a
smaller tree and is not comparable.
