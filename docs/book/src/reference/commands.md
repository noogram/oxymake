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

A gate is declared in the Oxymakefile and names the rules it guards:

```toml
[gate.qc_check]
after = ["align"]            # informational: the rules whose results you review
before = ["publish"]         # every job of these rules waits for the gate
message = "Check alignment QC before publishing."
```

When `ox run` reaches a job of a rule listed in `before`, it registers the
gate as *pending* in `.oxymake/state.db`, prints the message and waits,
polling the gate until a decision is recorded:

```bash
ox gate list                              # All gates: id, name, status, run
ox gate approve qc_check                  # Approve by name — the run resumes
ox gate approve qc_check --reason "ok" --approver alice
ox gate reject qc_check --reason "bad"    # Reject — the guarded jobs are cancelled
ox gate approve 3                         # By id (shown by `ox gate list`)
```

Rules of the gate ledger:

- **One record per gate and run.** A decision belongs to the `ox run` that
  asked for it; the next run that reaches the gate registers a fresh pending
  record and waits again. Runs whose guarded outputs are already up to date
  never reach the gate.
- **Names first, ids as a fallback.** `approve`/`reject` match the gate name
  against pending records; with a single run waiting there is exactly one.
  If two runs wait on the same gate, the name is ambiguous and the command
  lists the ids to use instead.
- **Two approved runs of the same job fail closed.** *Known limitation.*
  The cooperative claim protocol of `state.db` (ADR-012) is not yet the
  scheduling gate, so approving both ids makes both sessions try to run the
  job. Rather than let two executions interleave and commit a mixed output
  set, the local executor takes an exclusive lock on the job's output set:
  the session that wins it executes and commits, and the other aborts that
  job with an error (exit status 1) that names the holding session's PID
  (`job '<id>' is already being executed by another session (pid N)`). The
  committed outputs are always one execution's set, never a mix. To run the
  job exactly once, approve one id and reject the other.
- **`after` adds no dependency edge.** A gate is evaluated once a guarded
  job's own inputs are ready, so list in `after` rules that are upstream of
  the `before` rules through the DAG.
- **Local executor only.** Gates are enforced by the scheduler; `--executor
  slurm` and `--executor ray` submit the DAG without it, so a gated workflow
  is refused on those executors.
- **The ledger is the enforcement.** If `.oxymake/state.db` cannot be
  opened, a gated run fails instead of running the guarded rules unapproved.
  If a gate record cannot be written (the run reports that the gate "could
  not be registered"), the guarded jobs stay blocked and the registration is
  retried on every poll; an absent record never opens a gate.

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
