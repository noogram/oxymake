# CLI Commands

OxyMake provides the `ox` command-line tool. Every command supports `--json`
for structured NDJSON output.

## Core Commands

### `ox init`

Initialize a new OxyMake project in the current directory.

```bash
ox init
```

Creates a starter `Oxymakefile.toml` and `.oxymake/` directory.

### `ox run`

Execute the workflow, ensuring requested outputs exist.

```bash
ox run                          # Build default targets
ox run results/report.html      # Build a specific target
ox run -j 8                     # Parallel execution (8 jobs)
ox run --rule stats             # Only run jobs from a rule (exact or /regex/)
ox run --json                   # Structured NDJSON output
ox run --note "experiment v2"   # Annotate the run
ox run --no-cache               # Ignore the cache, re-run everything
```

**Options:**
- `-j N`, `--jobs N` -- Maximum concurrent jobs (default: 1)
- `--rule RULE` -- Only run jobs from this rule (exact name or `/regex/`)
- `-k`, `--keep-going` -- Continue independent jobs after a failure
- `-n`, `--dry-run` -- Show what would run without executing
- `--json` -- Emit NDJSON events on stdout
- `--report-json PATH` -- Write the NDJSON event stream to a file
- `--note TEXT` -- Attach a note to this run
- `--no-cache` -- Ignore cached outputs and re-execute
- `--cache-remote DIR` -- Share output blobs through a directory blob store
  (forces `hash` validation; see below)
- `--executor EXEC` -- Choose executor: `local` (default), `slurm`, `ray`

**`--cache-remote <dir>`** stores each job's output blobs in the given
directory (content-addressed, BLAKE3-verified on restore) and restores
missing outputs from it. Validation is always promoted to `hash` when the
flag is set. The directory is a *blob transport*, not a complete portable
cache: the local SQLite index under `.oxymake/cache/` that maps computation
keys to output paths and hashes does not travel with the blobs, so a fresh
checkout pointing at the same directory re-executes unless that local index
is transferred too. A remote computation-key manifest that removes this
requirement is future work. See [Caching](../concepts/cache.md).

**Exit codes:**
- `0` -- Success (all jobs succeeded or were cached)
- `1` -- Runtime error or one or more jobs failed
- `2` -- Command-line usage error

### `ox plan`

Show the execution plan without running anything.

```bash
ox plan                     # Show what would run (optimized)
ox plan --json              # Structured plan output
ox plan --no-optimize       # Show the raw plan (skip optimization passes)
ox plan --level rules       # Show the RuleGraph instead of the JobGraph
```

### `ox lint`

Validate the Oxymakefile without executing.

```bash
ox lint                     # Check for errors
ox lint --json              # Structured diagnostics
```

Checks for: syntax errors, missing inputs, cycles, ambiguous rules,
undefined wildcards.

## Inspection Commands

### `ox dag`

Visualize the dependency graph.

```bash
ox dag                      # Graphviz DOT output (default)
ox dag --format mermaid     # Mermaid graph syntax
ox dag --group-by rule      # Collapse nodes by field
ox dag --json               # Structured JSON
```

### `ox status`

Show current execution status.

```bash
ox status                   # Summary of current state
ox status --json            # Structured status
```

### `ox logs`

View job logs.

```bash
ox logs stats-alice         # Logs for a specific job
ox logs --failed            # Logs for all failed jobs
```

### `ox history`

List past runs.

```bash
ox history                  # Recent runs
ox history --json           # Structured history
```

## Management Commands

### `ox gate`

Manage gates (human-in-the-loop checkpoints).

```bash
ox gate list                              # Show pending gates
ox gate approve qc_check                  # Approve a gate
ox gate approve qc_check --reason "ok"    # Approve with reason
```

### `ox snapshot`

Manage workflow snapshots for comparison.

```bash
ox snapshot save baseline-v1        # Save current state
ox snapshot diff baseline-v1        # Compare with snapshot
ox snapshot list                    # List snapshots
```

### `ox invalidate`

Invalidate cached outputs to force re-execution.

```bash
ox invalidate stats                 # Invalidate a rule
ox invalidate results/alice.txt     # Invalidate a specific output
```

### `ox clean`

Remove outputs and cache.

```bash
ox clean                    # Remove all outputs
ox clean --cache            # Also remove cache
ox clean --state            # Delete a corrupt state.db (it is a regenerable cache)
```

### `ox cancel`

Cancel running jobs.

```bash
ox cancel                   # Cancel all running jobs
ox cancel stats-alice       # Cancel a specific job
```

### `ox top`

Live TUI dashboard for monitoring execution.

```bash
ox top                      # Interactive dashboard
```

Shows real-time job status, resource utilization, and DAG progress.

## Global Options

Every command accepts:

| Flag | Description |
|------|-------------|
| `--color <MODE>` | Color output mode (`auto`, `always`, `never`) |
| `-V`, `--version` | Print version |
| `-h`, `--help` | Print help |

Most subcommands additionally accept `--json` (structured NDJSON output) and
`-v`/`-vv` (increase verbosity).

## Next Steps

- [Oxymakefile Format](./format.md) -- workflow definition reference
- [Configuration](./configuration.md) -- project settings
