# ADR-009: Contextual Output Reporter — Adapt Display to Executor Profile

## Status
Proposed

## Metadata

- **Kind:** `proposal`
- **Family:** `EXT`
- **Supersedes:** `none`

## Context
OxyMake's output is currently one-size-fits-all. The `Reporter` trait
(`ox-core/src/traits/reporter.rs`) receives `Event`s and presents them,
but the presentation is the same regardless of executor context:

1. **Local execution**: progress bars, per-job spinners, ETA — appropriate
   because OxyMake orchestrates and owns the full lifecycle.
2. **Ray fire-and-forget**: the same progress bar appears, but it's misleading.
   After `submit_dag()`, OxyMake has no live connection to the jobs. The user
   needs a submission summary, dashboard URL, Ray job ID, and `ox status`
   polling instructions — not a progress bar that stalls.
3. **SLURM batch**: the user needs sbatch IDs, partition info, and squeue-style
   status — not spinners.
4. **CI / non-TTY**: already handled by TTY detection in `TermReporter`, but
   the fallback is plain text with the same content structure as TTY mode.

The root problem: reporter selection is driven by output format (JSON vs
terminal) and TTY detection, but not by **executor semantics**. The executor
determines what information is available and what presentation is useful.

This is the output complement of ADR-008 (ExecutorBridge), which formalizes
the submit/monitor/control directions. ADR-009 formalizes how those directions
surface to the user.

### Current Architecture

```
run.rs:
  EventBus → JsonReporter (always, to event log)
  EventBus → JsonReporter (if --json, to stdout)
  EventBus → TermReporter (if !--json, to stderr)
  EventBus → StateDb writer (always, for dashboard)
```

Reporter selection is hardcoded in `run.rs` (lines 1134-1184). The
`TermReporter` handles both TTY and non-TTY modes internally. There is no
hook for executor-specific output.

`ox status` (status.rs) has its own ad-hoc output logic: it reads meta.json,
detects Ray, queries the Ray Jobs API, and formats results inline. This
duplicates concerns that should live in a reporter.

## Decision
Introduce an `ExecutorReporter` trait that provides executor-aware output
hooks at three lifecycle phases: submission, polling, and completion. The
existing `Reporter` trait remains unchanged — `ExecutorReporter` is a
companion, not a replacement.

### The ExecutorReporter Trait

```rust
/// Executor-aware output hooks for the three lifecycle phases.
///
/// Unlike `Reporter` (which receives fine-grained `Event`s during local
/// orchestration), `ExecutorReporter` handles coarse-grained lifecycle
/// phases that only apply to remote executors (submit → poll → complete).
///
/// The `Reporter` trait remains the primary interface for local execution.
/// `ExecutorReporter` is used when the executor performs fire-and-forget
/// submission and OxyMake's role shifts from orchestrator to observer.
pub trait ExecutorReporter: Send + Sync {
    /// Called immediately after DAG submission succeeds.
    ///
    /// Display: submission confirmation, executor-specific IDs, dashboard
    /// URLs, job counts (active vs cached), and instructions for monitoring.
    fn on_submission(&self, info: &SubmissionInfo);

    /// Called during `ox status` or `--follow` polling.
    ///
    /// Display: per-job status table, progress percentage, executor-native
    /// status (Ray job state, SLURM job state), elapsed time.
    fn on_poll(&self, status: &PollStatus);

    /// Called when all jobs reach terminal state.
    ///
    /// Display: final summary, per-job durations, failure details,
    /// aggregate statistics.
    fn on_completion(&self, result: &CompletionInfo);
}
```

### Supporting Types

```rust
/// Information available after successful DAG submission.
pub struct SubmissionInfo {
    pub run_id: String,
    pub executor: String,          // "ray", "slurm", "k8s"
    pub total_jobs: usize,
    pub active_jobs: usize,        // submitted to executor
    pub cached_jobs: usize,        // skipped via cache
    pub connection: ConnectionInfo,
}

/// Executor-specific connection details for display.
pub enum ConnectionInfo {
    Ray {
        dashboard_url: String,
        job_id: String,
    },
    Slurm {
        partition: String,
        job_ids: Vec<String>,       // sbatch IDs
    },
    // Future: K8s { namespace, workflow_name }
}

/// Polling snapshot for display.
pub struct PollStatus {
    pub run_id: String,
    pub jobs: Vec<JobPollEntry>,
    pub aggregate: DagState,        // from ADR-008
    pub elapsed_secs: u64,
}

pub struct JobPollEntry {
    pub job_id: String,
    pub status: JobStatus,
    pub elapsed_secs: Option<u64>,
    pub executor_id: Option<String>, // Ray task ID, SLURM job ID
}

/// Final results for display.
pub struct CompletionInfo {
    pub run_id: String,
    pub succeeded: usize,
    pub failed: usize,
    pub skipped: usize,
    pub total_duration_ms: u64,
    pub failures: Vec<JobFailureDetail>,
}

pub struct JobFailureDetail {
    pub job_id: String,
    pub error: String,
    pub exit_code: Option<i32>,
}
```

### Implementations

| Implementation | Crate | When selected |
|---------------|-------|---------------|
| `RayReporter` | `ox-exec-ray` | executor = "ray" |
| `SlurmReporter` | `ox-exec-slurm` | executor = "slurm" |
| `PlainReporter` | `ox-report-term` | non-TTY fallback for any executor |
| `JsonReporter` | `ox-report-json` | `--json` flag (already exists, extend) |

Each reporter implementation lives in the executor's crate (not in
ox-report-term), because it depends on executor-specific connection
details and formatting conventions.

### RayReporter Example Output

**on_submission:**
```
  Submitted 75 jobs to Ray (25 cached)
  Dashboard: http://127.0.0.1:8265
  Driver job: raysubmit_abc123
  Run: run-20250401-120000

  Monitor: ox status
  Follow:  ox status --follow
```

**on_poll:**
```
  Run run-20250401-120000 (Ray)
  Progress: 45/75 (60.0%)  12 running  18 pending  0 failed
  Elapsed: 3m42s

  Running:
    build[x86]       2m10s  ray-001
    build[arm64]     1m55s  ray-002
    test[unit]       0m30s  ray-003
```

**on_completion:**
```
  Run complete: 73 succeeded, 2 failed, 25 cached
  Duration: 8m15s (4.2 jobs/s)

  Failed:
    test[integration] — exit 1: assertion failed at line 42
    lint[clippy]      — exit 1: 3 warnings treated as errors
```

### Selection Logic

Reporter selection happens in two places:

1. **`ox run`**: After executor selection (from `--executor` flag or
   Oxymakefile.toml), instantiate the matching `ExecutorReporter`. For
   local execution, use the existing `Reporter` trait path (no change).
   For remote executors, wire `ExecutorReporter` instead of `TermReporter`.

2. **`ox status`**: Read `meta.json` → determine executor type →
   instantiate the matching `ExecutorReporter` → call `on_poll()`.
   This replaces the ad-hoc Ray status logic currently inline in
   `status.rs`.

```rust
fn select_reporter(executor: &str, json: bool, is_tty: bool) -> Box<dyn ExecutorReporter> {
    if json {
        return Box::new(JsonExecutorReporter::new());
    }
    match executor {
        "ray" => Box::new(RayReporter::new(is_tty)),
        "slurm" => Box::new(SlurmReporter::new(is_tty)),
        _ => Box::new(PlainReporter::new()),
    }
}
```

### Relationship to Existing Reporter Trait

`ExecutorReporter` does **not** replace `Reporter`. They serve different modes:

| Concern | Reporter | ExecutorReporter |
|---------|----------|------------------|
| When used | Local orchestration | Remote fire-and-forget |
| Granularity | Per-event (JobStarted, etc.) | Per-phase (submit, poll, complete) |
| Data source | EventBus broadcast | DagSubmission + polling |
| Owned by | ox-report-term, ox-report-json | Executor crates |
| TTY handling | Internal (is_tty flag) | Per-implementation |

For local execution, nothing changes. The `TermReporter` continues to
receive events via EventBus and display progress bars.

For remote execution (fire-and-forget path), `ExecutorReporter` takes
over after `submit_dag()` returns. The EventBus still runs (events are
logged to the NDJSON file), but the user-facing output comes from
`ExecutorReporter.on_submission()`.

### Migration Path

1. Define `ExecutorReporter` trait and supporting types in `ox-core`
2. Implement `RayReporter` in `ox-exec-ray` (extract from `status.rs`)
3. Wire reporter selection in `run.rs` based on executor type
4. Refactor `ox status` to use `ExecutorReporter::on_poll()` instead
   of inline Ray logic
5. Implement `SlurmReporter` in `ox-exec-slurm`
6. Add `PlainReporter` for non-TTY / generic fallback

Steps 1-3 can land together. Steps 4-6 are independent follow-ups.

## Consequences

**What becomes easier:**
- Adding executor-specific output: implement `ExecutorReporter` in the
  executor crate instead of modifying the central `status.rs`
- `ox status` for remote runs: uniform `on_poll()` interface instead of
  ad-hoc per-executor logic
- Testing output: `ExecutorReporter` methods take typed structs, not
  raw events, making assertion-based testing straightforward
- Non-TTY / CI mode: each executor reporter handles its own fallback
  rather than the central `TermReporter` trying to be everything

**What becomes harder or riskier:**
- Two reporter traits (`Reporter` + `ExecutorReporter`) increase
  conceptual surface. Mitigated: they serve clearly distinct modes
  (local vs remote) and are not interchangeable.
- Executor crates gain presentation responsibility. This is intentional:
  executor-specific output requires executor-specific knowledge.
- The `--follow` flag for remote executors must wire polling into
  `ExecutorReporter::on_poll()` rather than the EventBus. This is a
  new code path that needs careful timeout and error handling.

## Alternatives Considered

**1. Extend the existing Reporter trait with executor context**
Add executor metadata to `Event` variants so `TermReporter` can branch
on executor type. This keeps one trait but pushes executor-specific
logic into a reporter that shouldn't know about Ray or SLURM. Violates
separation of concerns.

**2. Template-based output customization**
Let users define output templates (Tera, Handlebars) that executors
populate. Maximum flexibility but high complexity for a problem that
has a small, known set of executor types. Templates are hard to test
and debug. Consider for a future extension if user customization
demand materializes.

**3. Merge ExecutorReporter into ExecutorBridge**
Add `on_submission()`, `on_poll()`, `on_completion()` methods directly
to `ExecutorBridge` (ADR-008). This mixes infrastructure concerns
(submit, monitor, cancel) with presentation concerns (formatting,
TTY detection, colors). Separate traits allow the bridge to be tested
without output, and the reporter to be tested without a real executor.

**4. Status quo — keep inline output in status.rs**
Works today for Ray. Will not scale when SLURM DAG submission lands
and K8s is added. Each executor adds more ad-hoc branches to `status.rs`.
