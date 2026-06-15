# Benchmarks

OxyMake ships three benchmark suites: **perf** (DAG resolution at scale),
**snakemake-compat** (output-equivalence with Snakemake), and the
top-level **word-frequency** pipeline (side-by-side timing).

> **Benchmark of record.** The numbers cited by the paper (§6) and the README
> come from a separate, head-to-head harness:
> [`bench/snakemake-vs-oxymake/RESULTS.md`](../bench/snakemake-vs-oxymake/RESULTS.md)
> (`bash bench/snakemake-vs-oxymake/run.sh`). The `perf/` suite below is a
> developer tool for DAG-resolution scaling; its `results.md` output is
> **git-ignored** and is not a public numeric truth.

## Prerequisites

| Tool | Required | Install |
|------|----------|---------|
| ox | yes (in PATH) | `cargo install --path .` |
| Python 3 | yes (perf workload generation) | system / `brew install python3` |
| just | yes | `cargo install just` |
| hyperfine | optional (statistical timing) | `cargo install hyperfine` |
| snakemake | optional (comparison runs) | `pip install snakemake` |
| graphviz | optional (DAG PNG export) | `brew install graphviz` |

## Quick start

```bash
cd benchmark

# Full perf benchmark (all sizes: 1K, 5K, 10K, 50K)
just benchmark

# Fast feedback (1K jobs only)
just benchmark-quick

# Snakemake compatibility suite
just benchmark-snakemake

# Word-frequency side-by-side (OxyMake vs Snakemake)
just benchmark-word-frequency
```

## Benchmark suites

### perf/ — DAG resolution at scale

Measures `ox plan` (DAG resolution only, no shell execution) across
synthetic workloads from 1K to 50K jobs. Optionally compares against
`snakemake --dryrun`.

```bash
# All sizes (1K, 5K, 10K, 50K)
just benchmark

# Specific sizes
just benchmark 1000 5000

# With live dashboard
bash benchmark/perf/run.sh --dashboard
```

**How it works:**

1. `generate.py` creates synthetic `Oxymakefile.toml` + `Snakefile` pairs
   at each scale (N items x 2 rules + 1 merge = ~N jobs).
2. `run.sh` times `ox plan` (and optionally `snakemake --dryrun`) across
   sizes, reports median/min/max/stddev.
3. Results are written to `perf/results.md` and a timestamped archive copy.

**Files:**

| File | Purpose |
|------|---------|
| `perf/run.sh` | Benchmark driver |
| `perf/generate.py` | Synthetic workload generator |
| `perf/results.md` | Latest results (auto-generated) |

### snakemake-compat/ — Output equivalence

Validates that OxyMake produces identical outputs to Snakemake for
real-world-style workflows. Each subdirectory contains paired
`Oxymakefile.toml` and `Snakefile` definitions.

| Suite | Description |
|-------|-------------|
| `01-tutorial-basic` | Basic Snakemake tutorial pipeline |
| `02-rnaseq-counts` | RNA-seq read counting |
| `03-csv-etl` | CSV extract-transform-load |
| `04-multi-sample-qc` | Multi-sample quality control |

```bash
just benchmark-snakemake
```

### Top-level word-frequency pipeline

Side-by-side comparison of Snakemake vs OxyMake on a small word-frequency
pipeline, including a cache-hit test.

```bash
just benchmark-word-frequency
```

## Interpreting results

Results land in `perf/results.md`. Key columns:

- **Median** — most representative single number
- **Stddev** — consistency (lower = more stable)
- **Speedup** — `snakemake_time / oxymake_time` (>1x means OxyMake is faster)

## Adding a new benchmark

1. Create a directory under `benchmark/` with an `Oxymakefile.toml`.
2. Add a `run.sh` or integrate into an existing suite.
3. Add a recipe to the `Justfile`.
4. Document it in this README.
