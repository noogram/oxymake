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
ox run -j 8 --resource-budget cpu=6,mem_gb=32
ox run --rule stats             # Only run jobs from a rule (exact or /regex/)
ox run --json                   # Structured NDJSON output
ox run --note "experiment v2"   # Annotate the run
ox run --no-cache               # Ignore the cache, re-run everything
```

**Options:**
- `-j N`, `--jobs N` -- Maximum concurrent jobs (default: 1)
- `--resource-budget KEY=VALUE` -- Per-run local admission capacity; repeat
  the flag or separate pairs with commas, for example `cpu=6,mem_gb=32`.
  Uses the portable [resource aliases and units](format.md#resources), and is
  available with `--executor local` or for locally routed jobs in mixed SLURM
  runs. Pure remote runs reject it.
- `--rule RULE` -- Only run jobs from this rule (exact name or `/regex/`)
- `-k`, `--keep-going` -- Continue independent jobs after a failure
- `-n`, `--dry-run` -- Show what would run without executing
- `--json` -- Emit NDJSON events on stdout
- `--report-json PATH` -- Write the NDJSON event stream to a file
- `--note TEXT` -- Attach a note to this run
- `--no-cache` -- Ignore cached outputs and re-execute
- `--cache-remote DIR` -- Share output blobs through a directory blob store
  (forces `hash` validation; see below)
- `--executor EXEC` -- Choose executor: `local` (default), `slurm`, `ray`.
  A SLURM graph with an active rule declaring `executor = "local"` uses
  scheduler dispatch and waits for completion, including without `--follow`.
  `--jobs` bounds cluster submissions too (default: 1); `ox` must stay alive.
  Ray DAG runs reject active local overrides. Pruned or cached local jobs do
  not change dispatch. See [Executor override](format.md#executor-override).
- `--ray-allow-pending` -- Ray only; skip the default live-node feasibility
  check and allow waiting for a future node. Resource values and names are
  still validated. Using this flag with another executor is a usage error. It
  can also be enabled with `ray_allow_pending = true` in a selected profile.

`-j N` and `--resource-budget` control different limits. `-j N` limits the
number of concurrently running jobs. `--resource-budget` limits the declared
local resources those admitted jobs may hold; it does not change the job count, so a
rule with no resource declaration still uses one `-j` slot but reserves no
resource tokens. `--memory-budget` is separate again: it controls how much
completed output OxyMake keeps in memory, not whether a job may start.

A resource budget belongs to one `ox run` process. Two concurrent runs each
receive their configured capacity; OxyMake makes no host-wide capacity or
cross-process resource-arbitration promise.

Ray checks each active task's CPU (default 1), GPU, memory and custom request against
one live node's **total** capacity before submitting the DAG. Busy capable
nodes queue normally. The State API inspection has a 10-second end-to-end
deadline; malformed, unknown, partial or truncated snapshots fail closed.
An empty live-node list produces one warning and submits because a cluster may
be scaling from zero; visible nodes that cannot satisfy a request still cause
an error.
The check is a snapshot and does not monitor topology changes. See the
[Ray chapter](../concepts/ray-integration.md#resource-admission).

With `-f ../Oxymakefile.toml`, `run` and `plan` use the Oxymakefile directory
for workflow paths, `.oxymake`, cache checks and execution. Explicit
`--report-json` and `--cache-remote` paths remain relative to the invoking directory.

**Concurrent sessions.** Several `ox run` may share a workspace: each job
is claimed in `.oxymake/state.db` before it is dispatched, so a job is
executed by one session and the others wait for its result (see
[`ox gate`](#ox-gate) for the behaviour and `OX_SESSION_LEASE_SECS` for the
lease after which a dead session's jobs are taken over).

**`--cache-remote <dir>`** stores each job's output blobs in the given
directory (content-addressed, BLAKE3-verified on restore) and restores
missing outputs from it. Validation is always promoted to `hash` when the
flag is set. The directory is a *blob transport*, not a complete portable
cache: the local SQLite index under `.oxymake/cache/` that maps computation
keys to output paths and hashes does not travel with the blobs, so a fresh
checkout pointing at the same directory re-executes unless that local index
is transferred too. A remote computation-key manifest that removes this
requirement is future work. See [Caching](../concepts/cache.md).

**Local interruption.** The first Ctrl+C (or `SIGTERM`, which is what
[`ox cancel`](#ox-cancel) sends) starts a graceful shutdown: the session is
recorded `interrupted`, every in-flight job is marked `cancelled` in the
ledger, and its process group gets `SIGTERM`. The shutdown is **bounded** —
a job that has not exited after the grace period (default 10 s,
`OX_SHUTDOWN_GRACE_SECS` overrides; `0` escalates immediately) is killed with
`SIGKILL`, so a child that traps or ignores `SIGTERM` cannot hold the run
open. A second signal force-exits with `130`, cancelling this session's
still-running rows on the way out. Either way no row of the interrupted
session is left `running`, and rows owned by a *concurrent* session are never
touched (ADR-012).

A job failure without `--keep-going` is not an interruption: the run stops
dispatching new jobs and lets the ones already running finish, then exits
`1`. A Ctrl+C during that wait cancels them with the same bounded shutdown;
the exit code stays `1`, since a job did fail.

**Remote follow (`--executor ray|slurm --follow`).** Polling stops with exit
code `1` after **three consecutive rounds containing a poll error**. An entirely
successful round resets the counter, so one or two transient failures do not
abort a long run. A successful poll of one job does not hide another job's
persistent error. Ray waits 3 seconds between rounds and SLURM waits 5 seconds;
request latency is additional, so this is a retry bound, not a wall-clock deadline.
The summary names the endpoint and last error and reports last known job counts.

The first Ctrl+C or `SIGTERM` stops following promptly, including during a poll
request, and exits `130`. It reports `Follow interrupted` and last known counts.
For Ray, interruption also stops the driver and records its unfinished jobs as
cancelled after the stop succeeds. The message confirms the driver stopped or
reports the cancellation error without claiming an outcome. SLURM interruption
and endpoint loss do not cancel remote jobs; unfinished work may still be
running. Use `ox status` to inspect it and `ox cancel` to stop it.
Stopped follows record the session as `interrupted`, preserve requested
`--timings`, and do not print `Completed`. Endpoint loss and failed cancellation
leave unfinished job rows unchanged. With `--json`,
these outcomes emit `run_follow_stopped` with reason `interrupted` or
`endpoint_unreachable`, the exit code, message, and last known counts.

**Exit codes:**
- `0` -- Success (all jobs succeeded or were cached)
- `1` -- Runtime error or one or more jobs failed
- `2` -- Command-line usage error
- `130` -- Interrupted by Ctrl+C / `SIGTERM`

### `ox plan`

Show the execution plan without running anything.

Planning resolves the complete dependency graph, applies the same cache and
staleness analysis as `ox run`, and displays the jobs that may execute.
Existing generated outputs are not treated as source files, so a missing
intermediate includes its producer and downstream rebuilds; a warm cache
reports zero jobs. Job lines (and the JSON `reason` field) explain selections
such as `output missing` and `upstream rebuilt`.

With the same filesystem and cache settings, **plan reports an upper bound of
what run executes**. Planning cannot know the bytes a rebuilt job will produce,
so it conservatively selects downstream jobs with `upstream rebuilt`. At runtime,
if all rebuilt outputs match content hashes recorded under the same cache key,
consumers receive their normal cache check and may be skipped. A producer whose
cache key changed still forces its consumers, even when its bytes are identical.
Changed outputs still
invalidate consumers transitively. Missing recorded hashes, non-cacheable rules,
`--no-cache`, and `cache.validation = "mtime"` retain conservative rebuilding.

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

#### Running vs. orphaned

`ox status` does not report `jobs.status` verbatim. A job counts as
**running** only while the session that claimed it is `active` *and* its
heartbeat is younger than the claim lease (90 s by default,
`OX_SESSION_LEASE_SECS` overrides). A row left at `running` by a session
that was interrupted, completed, or stopped heartbeating is reported as
**orphaned**, with its age and the reason:

```text
Sessions: 0 active
Jobs: 14 total (1 completed, 0 running, 1 orphaned, 2 failed, 10 pending, 0 cached, 0 cancelled)
Orphaned: 1 jobs abandoned by a dead session
  substrate_tests                orphaned  12m17s  (owning session was interrupted)
  the next `ox run` re-evaluates them; `ox status` never reclaims
Pending: 10 jobs waiting
  repro_spheres                  waiting for: substrate_tests (orphaned)
```

`ox status` is a **reader**: it never rewrites a row it reports as
orphaned. Reclaiming happens at the start of the next `ox run`. The same
derivation backs `ox top` and the web dashboard — see
[ADR-012](https://github.com/noogram/oxymake/blob/main/docs/adr/012-cooperative-multi-session.md).

`--json` carries the distinction too: `jobs.orphaned` alongside
`jobs.running`, an `orphaned_jobs` array (each entry with `reason`,
`reason_description`, `declared_status`, `session_id`, `elapsed_secs`),
and `waiting_for_orphaned` on each pending job.

### `ox logs`

View job logs.

```bash
ox logs stats-alice         # Logs for a specific job
ox logs --failed            # Logs for all failed jobs
```

### `ox history`

List past runs.

Per-job detail includes the executor selected for each executed job, including
local overrides in SLURM runs. The text table has an `EXECUTOR` column; JSON
uses `executor`.

```bash
ox history                     # Recent runs
ox history --json              # Structured history
ox history --run-id run-1234   # Per-job detail for one run
```

With `--run-id … --json`, each job is emitted as one JSON object per line.
Besides timing and exit code, the object carries the provenance of the
cache decision — `input_hashes`, `output_hashes`, `params_hash`,
`env_hash`, `reproducibility_class` and `artifact_provenance` — so a cache
report can be reconstructed from recorded state instead of being caught
live. A job the cache layer never keyed (`--no-cache`, or
`cache_validation = "mtime"`) reports `null` for these. `peak_mem_mb` is
the executor's per-job peak-memory observation, stored as whole MiB rounded
up (the text column is `MEM MiB`; the JSON key remains `peak_mem_mb`). Unlike
history's whole-MiB round-up, benchmark TSV `max_rss` uses two decimal places:
1500 KiB appears as 2 in history and 1.46 in the TSV; a positive sub-MiB value
rounds up to 1 in history.

`ox run --executor slurm --follow` records the maximum `sacct` MaxRSS across
that job's steps (`.batch`, `.0`, etc.), retaining allocation status. Zero,
missing accounting, and squeue fallback leave memory `null`. SLURM REST
currently reports no peak-memory observation. Submission without `--follow`
does not collect this accounting.

Local cold launches on Linux and macOS record the child's `wait4` RSS in
history, rounded up to MiB. This includes already-reaped descendants but
is not a simultaneous process-tree total; background or daemonised work can
escape it. Fork-mode warm workers also record the child each dispatch forks
and reaps, so the same retained template can publish distinct peaks for
successive jobs. Persistent-mode warm workers remain `null`: one process serves
every dispatch, so a per-dispatch RSS peak does not exist. CPU time is measured
for cold and warm-fork children too, but appears only in benchmark TSV, not
history.

The figure belongs to the attempt that finished the job; earlier failed
attempts are not accumulated. Failures and cancellations leave memory `null`.
The scheduler's `ErrorStrategy::Ignore` is an exception: it treats a failed
attempt as completed and its `job_completed` event can carry that attempt's
memory. Mock commands verify persistence; a live SLURM cluster has not verified
this path.

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
- **Two approved runs of the same job execute it once.** Before it
  dispatches a job, `ox run` claims it in `state.db` (the cooperative claim
  protocol of ADR-012). Approving both ids therefore makes one session
  execute the job and the other wait: it prints `<job> is being executed
  by <session>; waiting for its result instead of running it here`, then
  takes over the owner's result — a completion counts as done in both
  sessions (both exit 0, exactly one execution happened), a failure or
  cancellation by a live owner is mirrored. Sessions heartbeat every third
  of a lease (default 90 s, `OX_SESSION_LEASE_SECS` overrides); if the
  owner is killed mid-job, the waiting session reclaims the job once the
  lease has expired and executes it itself. A waiting run stops on the
  first Ctrl+C / SIGTERM without touching the peer's job. **Defence in
  depth:** the local executor also takes an exclusive `flock(2)` on each
  output path under `.oxymake/locks/` for the duration of execution and
  commit, and a job whose paths are locked by another live process fails
  closed (`job '<id>' is already being executed by another session (pid
  N): output '<path>' is locked`) — for instance a reclaimed job whose
  killed owner's orphaned shell is still writing. That lock holds on local
  filesystems with a working `flock(2)`; the claim protocol holds wherever
  SQLite does, hosts included.
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

`ox cancel` marks the jobs cancelled in `.oxymake/state.db` and sends
`SIGTERM` to the owning `ox run`, which handles it on the same graceful path
as Ctrl+C — including the bounded `SIGTERM` → `SIGKILL` escalation described
under [`ox run`](#ox-run).

### `ox top`

Live TUI dashboard for monitoring execution.

```bash
ox top                      # Interactive dashboard
```

Shows real-time job status, resource utilization, and DAG progress.

It uses the same running-vs-orphaned derivation as
[`ox status`](#ox-status): the Running Jobs panel lists only jobs a live
session is executing, and the panel title carries an `N orphaned` count
when rows were abandoned. `ox top` never reclaims them.

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
