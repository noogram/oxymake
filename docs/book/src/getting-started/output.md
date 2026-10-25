# Understanding the Output

When you run `ox run`, OxyMake provides structured feedback about what it is
doing and why. This page explains the output formats, using the 3-rule
workflow from [Your First Workflow](./first-workflow.md) (`stats` for two
students, plus a `summary`).

## Terminal Output (Default)

By default, OxyMake prints human-readable progress and ends with a canonical
summary line (timings will vary):

```
  Resolving 3 jobs (3 to run, 0 cached)
  ▸ summary — upstream rebuilt
  ✓ Completed 3/3 in 0.6s (4.8 jobs/s)
    3 succeeded
Completed: 3 succeeded, 0 failed, 0 skipped, 0 cancelled (0.6s)
```

The last line is the **canonical summary**, always in the same shape:

```
Completed: N succeeded, N failed, N skipped, N cancelled (<elapsed>)
```

- **succeeded** -- jobs that ran and produced their outputs
- **failed** -- jobs whose command exited non-zero
- **skipped** -- jobs whose outputs were already up to date (cache hits)
- **cancelled** -- jobs that did not run because an upstream job failed

A run is successful (exit code `0`) when both `failed` and `cancelled` are `0`.

## Cached Jobs

When outputs are already up to date, OxyMake skips the work and reports the
jobs as `skipped`:

```
Cache: 3 of 3 job(s) up-to-date, skipping.
Completed: 0 succeeded, 0 failed, 3 skipped, 0 cancelled (0.0s)
```

On a partial re-run (one input changed), the cached jobs are listed and the
summary reflects the split:

```
Cache: 1 of 3 job(s) up-to-date, skipping.
  Resolving 3 jobs (2 to run, 1 cached)
  [1/3] ✓ stats-bob [cached]
  ▸ summary — upstream rebuilt
  ✓ Completed 3/3 in 0.4s (7.5 jobs/s)
    2 succeeded, 1 skipped
Completed: 2 succeeded, 0 failed, 1 skipped, 0 cancelled (0.4s)
```

## Plan Output

Use `ox plan` to see what may run without executing:

```bash
ox plan
```

```
Plan: 3 rules, 3 jobs, 2 source files
Targets: results/summary.json
  1. [stats-bob] rule=stats -> [results/bob_stats.json]
  2. [stats-alice] rule=stats -> [results/alice_stats.json]
  3. [summary] rule=summary -> [results/summary.json]
```

The header reports the totals (`N rules, N jobs, N source files`), followed by
the requested targets and the concrete jobs, each shown as
`[job-id] rule=<rule> -> [outputs]`.

The plan is an upper bound of what `ox run` executes with the same tree and
cache settings. A planned `upstream rebuilt` job may be skipped at runtime if
its producer rebuilds bytes matching hashes recorded under the same cache key
and the consumer's normal cache check passes. A producer whose cache key changed
still forces its consumers, even with identical bytes.

## JSON Output (Agent Mode)

Add `--json` to `ox run` for structured NDJSON output -- one self-contained
JSON event per line:

```bash
ox run --json
```

```json
{"event":"run_started","schema_version":1,"ts":"2026-10-08T12:00:00Z","run_id":"run-550e8400-e29b-41d4-a716-446655440000","workflow_path":"Oxymakefile.toml","total_jobs":3,"to_run":3,"cached":0}
{"event":"job_planned","schema_version":1,"ts":"2026-10-08T12:00:00Z","job_id":"stats-bob","rule":"stats","wildcards":{"student":"bob"},"resources":{},"dependencies":[]}
{"event":"job_planned","schema_version":1,"ts":"2026-10-08T12:00:00Z","job_id":"stats-alice","rule":"stats","wildcards":{"student":"alice"},"resources":{},"dependencies":[]}
{"event":"job_planned","schema_version":1,"ts":"2026-10-08T12:00:00Z","job_id":"summary","rule":"summary","wildcards":{},"resources":{},"dependencies":["stats-alice","stats-bob"]}
{"event":"job_started","schema_version":1,"ts":"2026-10-08T12:00:00Z","job_id":"stats-bob","executor":"local","reason":"cache_miss"}
{"event":"job_completed","schema_version":1,"ts":"2026-10-08T12:00:00Z","job_id":"stats-bob","duration_ms":209,"outputs":["results/bob_stats.json"]}
{"event":"job_started","schema_version":1,"ts":"2026-10-08T12:00:00Z","job_id":"stats-alice","executor":"local","reason":"cache_miss"}
{"event":"job_completed","schema_version":1,"ts":"2026-10-08T12:00:00Z","job_id":"stats-alice","duration_ms":200,"outputs":["results/alice_stats.json"]}
{"event":"job_started","schema_version":1,"ts":"2026-10-08T12:00:00Z","job_id":"summary","executor":"local","reason":"upstream_rebuilt"}
{"event":"job_completed","schema_version":1,"ts":"2026-10-08T12:00:00Z","job_id":"summary","duration_ms":194,"outputs":["results/summary.json"]}
{"event":"run_completed","schema_version":1,"ts":"2026-10-08T12:00:00Z","total":3,"succeeded":3,"failed":0,"skipped":0,"cancelled":0,"duration_ms":607,"status":"success"}
```

Each event carries an `event` discriminant (`run_started`, `job_planned`,
`job_started`, `job_completed`, `run_completed`), a `schema_version` (`1`) and
a UTC RFC 3339 `ts` timestamp. Every planned job gets its `job_planned` line
right after `run_started`, before any job starts or is skipped. The format is
designed for AI agents and scripts: each line parses independently. Use
`--report-json <path>` to write the same stream to a file. See
[Agent-Driven Workflows](../cookbook/agent-workflows.md) for details.

Every executing run, fully cached runs included, also writes this stream to
`.oxymake/events/<run-id>.ndjson`. The file is created before the run is
recorded or any job starts: if it cannot be created (for example, the
directory is not writable), `ox run` exits 1 without recording the run.

Each executing invocation mints one opaque UUIDv4-based run ID. The same full
value names its default event file and appears in state, run artifacts, and
executor metadata. Legacy IDs remain valid exact keys. Run IDs do not sort by
creation time; use recorded timestamps.

## Status and History JSON

`ox status --json` includes top-level integer `"schema_version": 1` in
every emitted object: the current or selected run (`--run ID`), grouped
summaries (`--group-by rule` or `stage`), absent state, and any separate Ray
driver or cluster summary. Ray status can emit several consecutive JSON
objects; consumers should parse a stream of JSON values.

`ox history --json` includes the same field in each NDJSON run record and
each job record selected by `--run-id ID`. No state, no runs, or a selected
run with no job history produces an empty stream.

Status and history each have their own schema version, independent of each
other and of the run-event journal. They start at 1; keeping them independent
lets a snapshot or history format change without forcing event-log consumers
to migrate. A non-additive change (removing or renaming a field, changing its
type or meaning, or changing the record structure) increments the affected
version. Additive fields do not increment it; readers should ignore unknown
fields. These payloads remain unstable before 1.0. Error diagnostics on
stderr are separate from these success-record schemas.

## DAG Visualization

Use `ox dag` to render the dependency graph. The default format is Graphviz
DOT:

```bash
ox dag
```

```
digraph oxymake {
  rankdir=LR;
  "results/summary.json" -> "all";
  "stats" -> "results/{student}_stats.json";
  "data/{student}.csv" -> "stats";
  "summary" -> "results/summary.json";
  "results/{student}_stats.json" -> "summary";
}
```

Other formats:

```bash
ox dag --format mermaid       # Mermaid graph syntax
ox dag --format dot           # Graphviz DOT (same as default)
ox dag --group-by rule        # Collapse nodes by field
ox dag --json                 # Structured JSON
```

To trace a single target's dependency chain instead, use `ox explain`:

```bash
ox explain results/summary.json
```

```
Dependency chain for: results/summary.json

► 1. [summary] rule=summary
     inputs:  [results/alice_stats.json, results/bob_stats.json]
     outputs: [results/summary.json]
  2. [stats-alice] rule=stats
     inputs:  [data/alice.csv]
     outputs: [results/alice_stats.json]
  3. [stats-bob] rule=stats
     inputs:  [data/bob.csv]
     outputs: [results/bob_stats.json]
```

## Error Output

When a job fails, OxyMake reports the failure, cancels the dependent jobs, and
ends with a non-zero exit code:

```
  Resolving 1 jobs (1 to run, 0 cached)
  [1/1] ✗ broken FAILED (exit 1)

  error: job broken failed: exit code 1
    stderr: --- stderr ---
    stderr: boom
  ✗ Completed 1/1 in <0.1s
    1 failed
  Failed: broken
Completed: 0 succeeded, 1 failed, 0 skipped, 0 cancelled (0.0s)
  Failed jobs (showing 1 of 1):
    broken: boom
  Run 'ox logs --failed' for full details.
```

`ox logs --failed` prints the full captured output of each failed job. In
`--json` mode, the failure is reported as a `job_completed` event with a
non-success status, so automated tooling can recover programmatically.

## Verbosity Levels

Control output detail with `-v`:

```bash
ox run           # Normal output
ox run -v        # Verbose: job start/end, durations, and exit codes
ox run -vv       # Debug: also show each job's stdout/stderr
```

## Next Steps

- [CLI Commands](../reference/commands.md) -- full command reference
- [Execution Modes](../concepts/execution-modes.md) -- how rules are executed
