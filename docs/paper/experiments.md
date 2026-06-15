# OxyMake Paper — Planned Experiments

This document describes the experimental methodology for Section 6 of the
OxyMake paper. Each experiment specifies what to measure, how to measure it,
expected outcomes, and criteria for a positive result.

---

## Experiment 1: DAG Resolution Performance

### What to measure
Wall-clock time for DAG resolution (no execution) at 1K, 10K, and 100K jobs.

### Methodology

1. **Synthetic workflow generator**: A script that produces Oxymakefiles and
   Snakefiles with N rules, controlled fan-out (each rule depends on 1-3
   upstream rules), and M wildcards producing N total jobs.

2. **OxyMake measurement**:
   ```bash
   hyperfine --warmup 3 --min-runs 10 \
     'ox plan --json > /dev/null' \
     --export-json bench_oxymake_{N}.json
   ```

3. **Snakemake measurement**:
   ```bash
   hyperfine --warmup 3 --min-runs 10 \
     'snakemake -n --quiet > /dev/null' \
     --export-json bench_snakemake_{N}.json
   ```

4. **Scales**: N = 1,000 / 10,000 / 100,000 jobs.

5. **Hardware**: Fixed machine (Apple M4 Max, 128 GB RAM) to control for
   hardware variance. Also run on Linux x86_64 (AMD EPYC, 64 GB) for
   cross-platform comparison.

### Expected outcome
- OxyMake: <100ms at 1K, <500ms at 10K, <1s at 100K
- Snakemake: ~1s at 1K, ~30s at 10K, ~5min at 100K (based on reported user experience)
- Speedup: 50-300x

### Positive result criteria
OxyMake resolves 100K jobs in under 1 second on both platforms.

---

## Experiment 2: Cache Hit/Miss Latency

### What to measure
Time for a full no-op run (all outputs up to date) at various scales.

### Methodology

1. Run the synthetic workflow once to populate cache and outputs.
2. Measure no-op re-run time:
   ```bash
   # OxyMake: mtime fast-path (no content hashing needed)
   hyperfine --warmup 3 'ox run --dry-run --json > /dev/null'

   # Snakemake: timestamp comparison
   hyperfine --warmup 3 'snakemake -n --quiet > /dev/null'
   ```

3. Measure cache-miss scenario (touch one input file):
   ```bash
   touch data/sample_001.csv
   time ox run --dry-run --json > /dev/null
   ```

4. Measure phantom re-run scenario:
   ```bash
   git stash && git stash pop  # resets all mtimes
   ox run --dry-run --json     # should detect 0 changes (content unchanged)
   snakemake -n                # will report all jobs as needing re-run
   ```

### Expected outcome
- No-op: OxyMake <100ms, Snakemake ~1-10s (depending on scale)
- Phantom re-run: OxyMake correctly detects 0 changes; Snakemake reports
  all jobs as outdated

### Positive result criteria
- No-op run completes in <100ms for 10K jobs
- Zero phantom re-runs after git checkout (content-addressable correctness)

---

## Experiment 3: Startup Time

### What to measure
Cold-start time from invocation to first job dispatch.

### Methodology
```bash
# Measure time to first event in JSON stream
time ox run --json 2>/dev/null | head -1 > /dev/null

# More precise: use hyperfine
hyperfine --warmup 3 'ox run --dry-run --json | head -1 > /dev/null'
```

Compare against:
- `snakemake -n | head -1`
- `make -n | head -1`
- `nextflow -preview` (if available)

### Expected outcome
- OxyMake: <200ms (binary startup + TOML parse + DAG construction)
- Snakemake: ~1-3s (Python startup + imports + DAG construction)
- Make: ~50ms (baseline for native tools)

### Positive result criteria
First event emitted in under 200ms.

---

## Experiment 4: Binary Size and Resource Usage

### What to measure
- Compiled binary size (default features vs all features)
- Peak memory usage at various DAG scales
- Compilation time (full build, incremental)

### Methodology
```bash
# Binary size
cargo build --release
ls -lh target/release/ox

# Binary size with all features
cargo build --release --all-features
ls -lh target/release/ox

# Memory usage during DAG resolution
/usr/bin/time -v ox plan --json > /dev/null 2> mem_stats.txt
# (Linux: /usr/bin/time -v; macOS: use instruments or heaptrack)

# Compilation time
cargo clean && time cargo build --release
touch crates/ox-cli/src/main.rs && time cargo build --release  # incremental
```

### Expected outcome
- Binary: 5-15 MB (default), 20-40 MB (all features)
- Memory at 100K jobs: <500 MB
- Full build: <2 min; incremental: <10s

### Positive result criteria
Binary under 20 MB (default features). Memory under 500 MB at 100K jobs.

---

## Experiment 5: Bioinformatics Pipeline Port

### What to measure
Expressiveness and correctness when porting a real Snakemake workflow.

### Source workflow
Use the Snakemake variant-calling tutorial workflow (publicly available):
- 5 rules: trim, align, sort, call, filter
- ~100 samples
- Conda environments

### Methodology
1. Port the Snakefile to Oxymakefile.toml
2. Compare:
   - Line count (SLOC) of workflow definition
   - Number of distinct concepts needed (wildcards, resources, environments)
   - Features used that have no equivalent (or vice versa)
3. Run both on the same data and compare:
   - Identical output files (content hash comparison)
   - Total execution time
   - Behavior after `git checkout` (phantom re-run test)

### Expected outcome
- Oxymakefile is 20-40% shorter (no Python boilerplate)
- Identical output files (byte-for-byte if deterministic tools)
- OxyMake avoids phantom re-runs; Snakemake does not

### Positive result criteria
- Feature parity for the ported workflow
- No phantom re-runs after git operations
- Output files are identical

---

## Experiment 6: Quantitative Finance Organic Growth

### What to measure
OxyMake's support for iteratively growing workflows.

### Methodology

Simulate a 6-week research cycle:

1. **Week 1**: 5 rules, 50 features, single cohort.
   Run, snapshot as "baseline-v1".

2. **Week 2**: Add 12 call rules. Only calls run (features cached).
   Measure incremental build time.

3. **Week 3**: Add merge + annotate rules.
   Run `ox snapshot diff baseline-v1` and verify correct diff.

4. **Week 4**: Expand to 3 cohorts (human, mouse, yeast).
   Measure scale-up: only new cohorts compute.

5. **Week 5**: Add 200 alternative features.
   Measure: existing features are 100% cache-hit.

6. **Week 6**: Modify feature computation for one feature.
   Measure: only that feature and its downstream re-compute.

At each step measure:
- Number of jobs executed vs cached
- Wall-clock time
- `ox snapshot diff` correctness
- `ox dag --group-by stage` output readability

### Expected outcome
- At each step, only genuinely new/modified jobs execute
- Snapshot diff correctly reports additions, modifications, unchanged
- Hierarchical DAG is comprehensible at 10K+ job scale

### Positive result criteria
- Zero unnecessary re-computation across all 6 iterations
- Snapshot diff matches ground truth at every step

---

## Experiment 7: Agent-Driven Workflow

### What to measure
Whether an AI agent can drive a pipeline end-to-end via `--json`.

### Methodology

1. Create a pipeline with a gate (human-in-the-loop checkpoint before
   deployment).

2. Write a Claude Code skill that:
   - Invokes `ox run --json` and reads the event stream
   - When `gate.reached` is received, evaluates metrics from the output
   - Approves or rejects the gate based on metric thresholds
   - On `job.failed`, reads the error, adjusts parameters, and retries

3. Run the agent 10 times on the same pipeline with varying initial
   conditions (some leading to gate approval, some to rejection, some
   to recoverable failures).

4. Compare with a baseline where the agent parses terminal output instead
   of NDJSON.

### Expected outcome
- NDJSON mode: 90%+ success rate for end-to-end completion
- Terminal parsing mode: significantly lower success rate
- Agent correctly approves/rejects gates based on metrics

### Positive result criteria
- Agent completes the pipeline without human intervention in 9/10 runs
  using NDJSON mode
- NDJSON mode success rate is measurably higher than terminal parsing mode

---

## Experiment 8: FAIR Compliance Assessment

### What to measure
OxyMake's compliance with FAIR workflow indicators from
Wilkinson et al. (2025).

### Methodology

Use the indicator framework from the paper. For each indicator, assess:
- Whether OxyMake supports it natively
- Whether it requires user discipline (e.g., adding metadata)
- Whether it is not supported

Indicators to assess:

**Findable**
- F1: Globally unique identifier for workflows (lockfile hash)
- F2: Rich metadata (TOML is self-documenting)
- F3: Searchable workflow registry (future work)

**Accessible**
- A1: Standard protocol for retrieval (git, HTTP)
- A2: Open format (TOML, no vendor lock-in)

**Interoperable**
- I1: Standard serialization (Arrow IPC, Parquet)
- I2: Workflow description language (TOML vs CWL)
- I3: Cross-platform execution (scaling ladder)

**Reusable**
- R1: Clear provenance (audit trail)
- R2: Reproducibility (lockfile, content-addressed cache)
- R3: Community standards (license, open source)

### Expected outcome
OxyMake natively supports 70-80% of FAIR indicators, with remaining
indicators achievable through user discipline or future features.

### Positive result criteria
Higher FAIR compliance score than Snakemake on the same indicator set.

---

## Data Collection: Implementation Metrics

These metrics should be collected from the implementation and included
in Section 5 of the paper.

### Commands to run

```bash
# Lines of code per crate
tokei crates/ --sort code -t rust

# Test count
cargo test --workspace 2>&1 | grep 'test result'

# Coverage
cargo llvm-cov --workspace --summary-only

# Commit count and development time
git log --oneline | wc -l
git log --format='%ai' | head -1   # first commit
git log --format='%ai' | tail -1   # last commit

# Compilation time
cargo clean && cargo build --release --timings

# Binary size
ls -lh target/release/ox

# Workspace dependency count
cargo tree --workspace --depth 1 | wc -l
```

### Metrics table template

| Metric | Value |
|--------|-------|
| Total Rust SLOC | TBD |
| Number of crates | 14 |
| Test count | TBD |
| Test coverage | TBD |
| Commit count | TBD |
| Development period | TBD |
| Full build time | TBD |
| Incremental build time | TBD |
| Binary size (default) | TBD |
| Binary size (all features) | TBD |
| Memory at 100K jobs | TBD |
| DAG resolution at 100K | TBD |
