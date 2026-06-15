# OxyMake

<!-- HIBERNATION-BANNER:START — auto-managed region; empty while the project is awake. A 💤 banner is inserted here only during hibernation (see docs/HIBERNATION.md). -->
<!-- HIBERNATION-BANNER:END -->

[![CI](https://github.com/noogram/oxymake/actions/workflows/ci.yml/badge.svg)](https://github.com/noogram/oxymake/actions/workflows/ci.yml)
[![License: MIT/Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](#license)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)

**You `git checkout` an old branch, re-run your pipeline, and it rebuilds
everything — hours of work, even though not one input actually changed.**

OxyMake is a workflow engine that stops that guessing. It decides what to
re-run from the *content* of your data, not the timestamps — so the same
inputs always produce the same result, on any machine, at any time.

Three properties make that true:

- **Readable spec** — your pipeline is a plain, declarative TOML file. Any tool
  can read it; it's inspectable without running, so you never have to execute
  it to understand what it does.
- **Content-addressable** — change detection is a BLAKE3 hash of what your
  files *contain*, not when they were last touched. That is what makes phantom
  re-runs disappear — for the inputs you declare. There is no sandbox: a file
  your rule reads without declaring it is invisible to the cache key (see the
  threat-model subsection of the [paper](docs/paper/oxymake-paper.tex)).
- **Daemon-free** — every `ox run` is a self-contained process. Concurrent
  sessions coordinate through atomic SQLite claims on the shared workspace,
  with no server to install or keep alive.

And because every run emits `--json` output and a typed event stream, an agent
or a downstream tool can see exactly what ran and why.

*Best-effort single-maintainer project — see [Project Status](#project-status).*

## Coming from Snakemake?

You don't have to rewrite anything to find out if this is worth it. Point
OxyMake at the `Snakefile` you already have and watch your real DAG resolve:

```bash
ox translate Snakefile -o Oxymakefile.toml   # reads your existing Snakefile
ox run --dry-run                             # your real DAG, resolved — typically in milliseconds
```

That turns the headline 69 ms benchmark into *your* benchmark on *your*
pipeline. Same rules, same wildcards, same backward-chaining — just a faster
engine underneath. If you like what you see, keep going; if not, you changed
nothing.

## Features

- **Fast where it compounds** — 33× faster DAG *resolution* (a sub-second
  planning phase), and warm re-runs reuse the content-addressed cache instead of
  rebuilding. The full cold/warm/scale numbers — and the honest tradeoff that
  *cold* end-to-end runs are slower than Snakemake — live in the [benchmark of
  record](bench/snakemake-vs-oxymake/RESULTS.md).
- **Smart caching** — pluggable validation (`mtime+hash` default, pure `mtime`
  or full `hash` opt-in); the content-addressed `hash` mode eliminates phantom
  re-runs of declared inputs, even after a `git checkout` rewrites timestamps
- **Polyglot** — shell, Python, R, Julia per rule
- **Daemon-free** — `ox run` starts, works, exits. No server to manage.
- **Agent-friendly** — `--json` output, structured events, typed API
- **Job scheduling** — `-j N` declares the parallelism budget. *In v0.1 jobs in a ready batch still run sequentially*; true intra-batch parallelism is on the roadmap. Distributed fan-out across nodes works today via the SLURM and Ray executors.
- **Scales** — same workflow on a laptop, a SLURM cluster, or a Ray cluster (a Kubernetes executor is designed but not yet shipped)

## Quick Start

### Install

**With Cargo** (requires Rust 1.85+) — the paths that work today:

```bash
# Straight from git, no source checkout needed (canonical no-clone install)
cargo install --git https://github.com/noogram/oxymake ox-cli

# Or from a clone, e.g. to track main
git clone https://github.com/noogram/oxymake.git
cd oxymake
cargo install --path crates/ox-cli
```

Either path installs both `ox` and `oxymake` to `~/.cargo/bin/`. (The `oxymake`
name on crates.io is a reserved placeholder with no binary — install the engine
via one of the commands above, not `cargo install oxymake`.)

**From the first tagged release (v0.1.0)** — no Rust toolchain needed. These
distribution channels go live with the first published release and are **not**
available before then:

```bash
# macOS (Homebrew tap)
brew install noogram/tap/oxymake

# Linux / macOS — prebuilt binary for your platform from the release page
# https://github.com/noogram/oxymake/releases/latest
#   ox-x86_64-unknown-linux-gnu.tar.gz
#   ox-aarch64-apple-darwin.tar.gz
#   ox-x86_64-apple-darwin.tar.gz
tar xzf ox-<your-platform>.tar.gz && mv ox ~/.local/bin/

# In a Python project (fetches the prebuilt binary, no compile)
uv tool install oxymake     # or: pipx install oxymake
```

**Development mode** (build locally, run from source):

```bash
git clone https://github.com/noogram/oxymake.git
cd oxymake
cargo build                    # debug build → target/debug/ox
cargo run --bin ox -- --help   # run directly via cargo
```

**With [just](https://github.com/casey/just)** (recommended for development):

```bash
just build          # debug build
just test           # run all tests
just demo           # interactive demo of all features
just install        # install to ~/.cargo/bin/
just ox-install     # install with --force (overwrite existing)
just ox-update      # git pull + rebuild + install
just --list         # see all recipes
```

### Create a project

```bash
mkdir my-project && cd my-project
ox init
```

This creates an `Oxymakefile.toml` and a `.oxymake/` directory.

### Write a workflow

```toml
# Oxymakefile.toml
ox_version = "0.1"

[config]
samples = ["A", "B", "C"]

[rule.all]
input = ["results/{sample}.txt"]

[rule.process]
input = ["data/{sample}.csv"]
output = ["results/{sample}.txt"]
shell = "mkdir -p results && sort {input} > {output}"
```

### Create source data and run

```bash
mkdir -p data
echo -e "cherry\napple\nbanana" > data/A.csv
echo -e "zulu\nalpha\nmike" > data/B.csv
echo -e "delta\necho\nfoxtrot" > data/C.csv

ox run
# Completed: 3 succeeded, 0 failed, 0 skipped, 0 cancelled (0.0s)

cat results/A.txt
# apple
# banana
# cherry
```

### Core commands

```bash
ox lint                     # Validate the Oxymakefile
ox plan                     # Show execution plan
ox run                      # Execute the workflow
ox run --dry-run            # Show what would execute
ox run -j 4                 # Set the job budget (see note in Features: v0.1 runs ready batches sequentially)
ox run --json               # NDJSON output for agents
ox run --rule process       # Run only 'process' rules
ox run -k                   # Continue after failures
ox run --timings            # Per-phase timing breakdown
ox run -v                   # Verbose: job start/end/duration
ox test                     # Validate workflow without executing
ox dag --format dot         # DAG as Graphviz dot
ox dag --format mermaid     # DAG as Mermaid diagram
ox lock generate            # Create reproducibility lockfile
ox lock verify              # Check for drift
ox snapshot create v1       # Snapshot current state
ox snapshot diff v1 v2      # Compare snapshots
ox top                      # TUI live dashboard
ox dashboard                # Web dashboard (http://127.0.0.1:9876)
```

### Selective execution

```bash
ox run --until target.txt      # Build target and its dependencies only
ox run --omit-from clean.txt   # Exclude target and all downstream
ox run --touch                 # Mark outputs as up-to-date (like make --touch)
ox run --forcerun align        # Force re-execution of matching rules
ox run --profile fast          # Apply a named profile
```

### Cache validation

```bash
ox run                                  # Default: mtime+hash (mtime fast-path + hash fallback)
ox run --cache-validation=mtime         # Make-parity opt-in (stateless, no content check)
ox run --cache-validation=hash          # Strict: always hash (for CI / shared caches)
ox run --no-cache                       # Disable cache entirely
```

### Query and export

# `ox query` operates on job ids (as shown by `ox plan`), e.g. `annotate-chr1`,
# not bare rule names.
```bash
ox query 'deps(annotate-chr1)'           # Bazel-style dependency queries
ox query 'rdeps(align-NA12878)' --json   # Reverse dependencies (JSON output)
ox query 'allpaths(align-NA12878, annotate-chr1)'  # All paths between two jobs
ox export snakemake                       # Convert OxyMake → Snakemake (import: see "Coming from Snakemake?" above)
```

### Distributed execution

Run workflows on SLURM or Ray clusters with zero workflow changes:

```bash
# SLURM cluster
ox run --executor slurm

# Ray cluster (start Ray first: ray start --head)
ox run --executor ray
```

The Ray executor submits jobs via the Ray Jobs API, supports GPU scheduling,
and uses Ray's object store for in-memory data passing between `call`-mode rules.

Configure the Ray dashboard address in your project settings:

```toml
# .oxymake/config.toml
[executor.ray]
dashboard_address = "http://127.0.0.1:8265"
working_dir = "/shared/oxymake"
```

## Workflow Format

Rules declare transformations from inputs to outputs:

```toml
[rule.align]
input = ["data/{sample}.fastq", "refs/genome.fa"]
output = ["results/{sample}.bam"]
shell = "bwa mem -t 4 {input[1]} {input[0]} > {output}"
```

### Placeholders

| Placeholder | Meaning |
|-------------|---------|
| `{wildcard}` | Wildcard value (e.g., `{sample}`) |
| `{input}` | All inputs (space-separated) |
| `{output}` | All outputs (space-separated) |
| `{input[N]}` | N-th input (0-indexed) |
| `{output[N]}` | N-th output (0-indexed) |

### Four execution modes

```toml
# Shell command (most flexible)
shell = "sort {input} > {output}"

# Inline script
lang = "python"
run = """
import pandas as pd
df = pd.read_csv("{input}")
df.to_parquet("{output}")
"""

# External script
script = "scripts/process.py"

# Pure function (enables in-memory passing)
call = "pipeline.features:compute"
lang = "python"
```

### Conditional guards

```toml
[rule.special_analysis]
when = "sample in @special_samples"
```

### Tags and filtering

```toml
[rule.features]
tags = { stage = "features", domain = "genomics" }

# Run only tagged subgraph:
# ox run --where stage=features --where cohort=human
```

### Named profiles

Bundle common flag combinations for reuse:

```toml
[profile.fast]
jobs = 8
cache_validation = "mtime"
verbose = true

[profile.ci]
cache_validation = "hash"
keep_going = false
```

```bash
ox run --profile fast     # applies profile settings, CLI flags override
```

## Agent Integration

OxyMake is designed for AI agent-driven workflows:

```bash
# Stream structured events
ox run --json | while read -r event; do
  echo "$event" | jq .event
done

# Approve a gate programmatically
ox gate approve 1 --approver "agent:qc-bot"
```

The MCP server lives in the [`ox-mcp` crate](crates/ox-mcp): it speaks the
Model Context Protocol over stdio, exposing OxyMake's read/inspect commands
(`ox_status`, `ox_plan`, `ox_dag`, `ox_logs`, `ox_history`, `ox_lint`,
`ox_explain`, `ox_clean`) as typed tool calls with structured JSON responses.

## Output Integrity

OxyMake and user scripts share a clear responsibility split:

- **OxyMake guarantees** write integrity — all outputs use atomic writes
  (temp-then-rename), so a crash never leaves partial files. Cache validation
  is pluggable: `mtime` (default, fast), `mtime+hash` (hybrid), or `hash`
  (strict for CI). See ADR-006.
- **Scripts guarantee** domain validation — if `T` must be positive, the script
  asserts it. A script that exits 0 tells OxyMake "outputs are valid." OxyMake
  trusts this and caches the result.
- **Users**: if you have corrupt outputs from a pre-atomic-write era, delete them
  and re-run (`rm bad-file && ox run`).

See [Output Integrity](docs/design/output-integrity.md) for the full contract.

## Architecture

OxyMake resolves your workflow through three graph representations — the path
from "what you wrote" to "what actually runs":

```
Oxymakefile.toml → RuleGraph → JobGraph → ExecGraph
                   (logical)   (physical)  (runtime)
```

See [The Three Graphs](docs/book/src/concepts/three-graphs.md) for how rules
become runnable jobs. (The full formal specification lives in the
[Founding Thesis](OXYMAKE-THESIS.md), a project-internals governance document.)

This is the *workflow* pipeline, not the *code* layout. For how the two-dozen
`ox-*` crates fit together — the hexagonal, `ox-core`-centered structure — see
the [Crate Graph](docs/book/src/architecture/crate-graph.md).

## Documentation

- [Quickstart Guide](docs/book/src/getting-started/quickstart.md)
- [Concepts](docs/book/src/concepts/)
- [CLI Command Reference](docs/book/src/reference/commands.md)
- [Oxymakefile Format Reference](docs/book/src/reference/format.md)
- [Agent-Driven Workflows](docs/book/src/cookbook/agent-workflows.md)
- [Architecture Decision Records](docs/adr/)
- [FAIR Alignment](docs/FAIR-ALIGNMENT.md) — where OxyMake stands against FAIR / reproducible-workflow standards, and the v1.1 export roadmap
- [Academic Paper](docs/paper/oxymake-paper.tex) — the formal TLA+ specification and proofs (reassurance for after you've tried it, not a prerequisite)
- [The Making of OxyMake](docs/MAKING-OF.md) — how this repo was built (an agent fleet under one human maintainer)
- [Hibernation Protocol](docs/HIBERNATION.md) — **the document to read if you return after more than three months** (or if a `.hibernation` file exists at the repo root)

## Maintenance status — read before relying

OxyMake is maintained **best-effort by a single maintainer** with a
demanding day job. Concretely:

- **Issues and PRs get answered slowly** — think weeks, not days. A slow
  answer is the contract here, not a sign of abandonment.
- **Security reports are the exception**: they are read with priority via
  [private vulnerability reporting](https://github.com/noogram/oxymake/security/advisories/new)
  — see [SECURITY.md](SECURITY.md).
- **Quiet phases are explicit, never silent.** If the project goes dormant,
  that state is declared by a `.hibernation` file at the repo root and a
  banner at the top of this README — see the
  [Hibernation Protocol](docs/HIBERNATION.md). No banner and no
  `.hibernation` file means the project is awake, just slow.
- The engineering bar (tests, clippy, formal spec drift checks, gated
  releases) is enforced by CI regardless of response latency.

## Project Status

**v0.1.0-alpha** — Core pipeline and all CLI commands functional. See the
[Functional Test Report](docs/FUNCTIONAL-TEST-REPORT.md) for detailed status.

### What works
- **Core**: `ox init`, `ox lint`, `ox plan`, `ox run`, `ox status`, `ox dag`, `ox translate`, `ox export`, `ox query`, `ox test`
- **Monitoring**: `ox top` (TUI), `ox dashboard` (web), `ox history`, `ox logs`
- **Reproducibility**: `ox lock generate/verify`, `ox snapshot create/list/diff/delete`
- **Workflow control**: `ox gate`, `ox cancel`, `ox invalidate`, `ox clean`
- **Execution**: wildcards, backward-chaining, `{input}`/`{output}` interpolation, `when` clauses
- **Output**: `--json` NDJSON events, `--rule` filtering, `--keep-going` mode, `--timings`, `-v`/`-vv` verbose
- **Caching**: pluggable validation (`mtime`, `mtime+hash`, `hash`), content-addressable with persistent manifest
- **Selective execution**: `--until`, `--omit-from`, `--touch`, `--forcerun`
- **Named profiles**: `[profile.NAME]` sections with `--profile` flag
- **Environments**: `environment =` specs are delegated as command wrappers
  (`uv run`, `conda run`, `docker run`, `nix develop -c`, `apptainer exec`) —
  the named tool must be on `PATH`; OxyMake does not create, cache, or
  isolate environments itself (see Known limitations)
- **Distributed**: SLURM executor, Ray executor (`--executor slurm`, `--executor ray`)
- **Remote cache**: shared-directory backend via `ox-cache-remote`
  (atomic store, content re-verification on fetch)
- **Translation**: bidirectional Snakemake (`ox translate` / `ox export snakemake`)
- **Query**: Bazel-style dependency graph queries (`deps`, `rdeps`, `allpaths`)

### Known limitations (v0.1)
- `-j N` parallelism is sequential within each ready batch
- Kubernetes executor designed but not yet implemented
- Environment management is delegation-only: the `EnvironmentProvider`
  crates (`ox-env-system`, `ox-env-uv`) are stubs, and there is no managed
  environment creation, caching, or isolation — `environment = "uv"` without
  `uv` on `PATH` fails at job execution time
- S3 and GCS remote-cache backends are configuration stubs (`ox-cache-remote`
  ships a working shared-directory backend; object-store transport is planned)

### Demo

Run the interactive tutorial to see all features in action:

```bash
cargo build    # the demo drives the debug binary
OX=./target/debug/oxymake bash examples/demo/run-demo.sh
```

## License

Dual licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).

## Citation

If you use OxyMake in your research, please cite it (GitHub's "Cite this
repository" button reads [`CITATION.cff`](CITATION.cff)):

```bibtex
@software{oxymake2026,
  author = {Sérié, Emmanuel},
  title = {OxyMake: A Formally-Specified, Content-Addressable Workflow Engine},
  year = {2026},
  url = {https://github.com/noogram/oxymake}
}
```
