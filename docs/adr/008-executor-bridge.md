# ADR-008: ExecutorBridge — Bidirectional Adapter Between OxyMake and Remote Executors

## Status
Proposed

## Metadata

- **Kind:** `proposal`
- **Family:** `EXT`
- **Supersedes:** `none`

## Context
OxyMake supports multiple execution backends (local, Ray, SLURM, future
Kubernetes) through the `Executor` trait (`ox-core/src/traits/executor.rs`).
The current trait surface conflates three distinct concerns:

1. **SUBMIT** — translating OxyMake's DAG into executor-native representations
   (Ray `@ray.remote` ObjectRef chains, SLURM `sbatch --dependency=afterok`)
2. **MONITOR** — polling remote state back into OxyMake's status model
3. **CONTROL** — cancellation and cascade

Each executor reimplements these directions independently, leading to
duplicated logic (adaptive polling, meta.json writing, state reconciliation)
and making it harder to add new backends. The `ox status` command currently
cannot poll Ray DAG submissions because the monitoring direction is ad-hoc
— it reads `meta.json` but has no formal contract for what belongs there.

Additionally, after a crash or restart, `ox status` must reconnect to a
running remote execution. Today this depends on convention (meta.json +
state.db entries) rather than a defined reconnection protocol.

This is the same structural problem solved by Airflow operators, Prefect
infrastructure blocks, and Flyte plugins — but those systems use binary
plugins. OxyMake's design principle (ADR-003) favors generated scripts over
embedded runtimes. The bridge must preserve that principle.

## Decision
Formalize the implicit bridge pattern into an explicit `ExecutorBridge` trait
that separates the three directions (SUBMIT, MONITOR, CONTROL) from the
per-job execution lifecycle already defined in `Executor`.

### The ExecutorBridge Trait

```rust
/// Bidirectional adapter between OxyMake and a remote executor.
///
/// The Executor trait handles per-job lifecycle (prepare → execute → finalize).
/// ExecutorBridge handles DAG-level operations across three directions:
///   SUBMIT:  OxyMake → Remote (translate and dispatch)
///   MONITOR: Remote → OxyMake (poll and reconcile)
///   CONTROL: OxyMake → Remote (cancel and cascade)
pub trait ExecutorBridge: Send + Sync + Debug {
    type Error: std::error::Error + Send + Sync + 'static;

    // -- SUBMIT direction (OxyMake → Remote) --

    /// Translate and submit the uncached subgraph to the remote executor.
    ///
    /// Filters out cached jobs, generates executor-native representation
    /// (Ray driver script, SLURM sbatch scripts with dependencies), submits,
    /// and returns immediately. Writes run metadata to `runs/{run_id}/`.
    fn submit_dag(
        &self,
        graph: &JobGraph,
        cached_jobs: &HashSet<JobId>,
        ctx: &ExecContext,
    ) -> impl Future<Output = Result<DagSubmission, Self::Error>> + Send;

    /// Map OxyMake resources to executor-native resource specifications.
    ///
    /// Called per-job during DAG translation. Returns opaque resource spec
    /// consumed by the executor's script generator.
    fn map_resources(
        &self,
        resources: &HashMap<String, String>,
    ) -> Result<ResourceSpec, Self::Error>;

    // -- MONITOR direction (Remote → OxyMake) --

    /// Poll the overall status of a DAG submission.
    ///
    /// Returns per-job status for all active (non-cached) jobs.
    /// The caller (ox status, scheduler) uses this to update state.db.
    fn poll_dag_status(
        &self,
        submission: &DagSubmission,
    ) -> impl Future<Output = Result<DagStatus, Self::Error>> + Send;

    /// Fetch stdout/stderr logs for a specific job.
    fn fetch_logs(
        &self,
        submission: &DagSubmission,
        job_id: &JobId,
    ) -> impl Future<Output = Result<String, Self::Error>> + Send;

    /// Reconcile remote state back into state.db after completion.
    ///
    /// Called once when all jobs in a submission reach terminal state.
    /// Writes job results (exit codes, durations, peak memory) to the
    /// state database so that `ox status` and cache validation see
    /// consistent data regardless of executor.
    fn sync_results(
        &self,
        submission: &DagSubmission,
        db: &StateDb,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Reconnect to a running submission after OxyMake restart.
    ///
    /// Reads `runs/{run_id}/meta.json` and reconstructs a DagSubmission
    /// handle. Returns None if the run is no longer active on the remote.
    fn reconnect(
        &self,
        run_dir: &Path,
    ) -> impl Future<Output = Result<Option<DagSubmission>, Self::Error>> + Send;

    // -- CONTROL direction --

    /// Cancel a specific job and its downstream dependents.
    fn cancel_job(
        &self,
        submission: &DagSubmission,
        job_id: &JobId,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Cancel all jobs in a submission.
    fn cancel_all(
        &self,
        submission: &DagSubmission,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
```

### Supporting Types

```rust
/// Executor-native resource specification (opaque to the scheduler).
pub enum ResourceSpec {
    Ray(RayResources),
    Slurm(Vec<SlurmDirective>),
    // Future: Kubernetes(K8sResources),
}

/// Aggregated status of a DAG submission.
pub struct DagStatus {
    pub run_id: String,
    pub jobs: HashMap<JobId, JobStatus>,
    /// Overall: all-completed, any-failed, still-running
    pub aggregate: DagState,
}

pub enum DagState {
    Running,
    Completed,
    Failed,
    Cancelled,
}
```

### Run Directory Contract

Every bridge writes a standard `meta.json` to `.oxymake/runs/{run_id}/`:

```json
{
  "executor": "ray|slurm|k8s",
  "version": 1,
  "submitted_at": "2025-04-01T12:00:00Z",
  "connection": {
    // executor-specific, used by reconnect()
    "ray_address": "http://127.0.0.1:8265",
    "ray_job_id": "raysubmit_abc123"
  },
  "run_id": "run-20250401-120000",
  "total_jobs": 100,
  "active_jobs": 75,
  "skipped_jobs": 25,
  "job_mapping": {
    "oxymake-job-id-1": "ray-submission-id-1"
  }
}
```

The `version` field allows forward-compatible schema evolution.
`ox status` reads `meta.json`, detects executor type, and dispatches to the
correct bridge's `poll_dag_status()`.

### Relationship to Executor Trait

`ExecutorBridge` does **not** replace `Executor`. They serve different scopes:

| Concern | Executor | ExecutorBridge |
|---------|----------|----------------|
| Scope | Single job | Entire DAG |
| Lifecycle | prepare → execute → finalize | submit → monitor → sync |
| Who calls | Scheduler (per-job loop) | CLI (fire-and-forget path) |
| Required | Yes (all backends) | Only remote backends |

The existing `submit_dag()` method on `Executor` moves to `ExecutorBridge`.
`Executor` retains per-job methods for the scheduler's synchronous loop.
Backends that support both modes (Ray) implement both traits.

### Implementation: Generated Scripts, Not Binary Plugins

Per ADR-003, bridges generate scripts rather than embedding executor SDKs:

- **Ray bridge**: generates a Python driver script with `@ray.remote` tasks
  and ObjectRef dependency chaining (already implemented in
  `ox-exec-ray/src/driver_script.rs`)
- **SLURM bridge**: generates sbatch scripts with `--dependency=afterok:$JOBID`
  for DAG edges (currently SLURM only supports individual submission; DAG
  submission is the natural next step)
- **Future K8s bridge**: generates Argo Workflow YAML or Kubernetes Job manifests

The bridge owns the translation from OxyMake's `JobGraph` to these
executor-native representations. The `cached_jobs` parameter ensures only
the uncached subgraph is translated.

### ox status Integration

With the bridge formalized, `ox status` gains a clean polling path:

```
ox status
  → find .oxymake/runs/*/meta.json
  → for each active run:
      → read meta.json → determine executor type
      → instantiate bridge (from config)
      → bridge.poll_dag_status(submission) → DagStatus
      → display per-job status with durations
```

After restart, `bridge.reconnect(run_dir)` reconstructs the submission handle
from meta.json. If the remote execution is gone (Ray cluster restarted,
SLURM job expired), `reconnect()` returns `None` and `ox status` marks
the run as lost.

### Cached Job Handling

The bridge receives `cached_jobs: &HashSet<JobId>` explicitly (moved from
executor-internal state). This makes the contract clear:

1. Scheduler computes cache hits via `CacheStore::check_cached()`
2. Passes `cached_jobs` to `bridge.submit_dag()`
3. Bridge excludes cached jobs from generated scripts
4. Bridge records `skipped_jobs` count in meta.json
5. `sync_results()` only writes results for active (non-cached) jobs

## Consequences

**What becomes easier:**
- Adding new backends: implement `ExecutorBridge` with clear method contracts
  instead of reimplementing polling, meta.json, and state sync from scratch
- `ox status` for remote runs: formal `poll_dag_status()` replaces ad-hoc
  meta.json reading with executor-specific HTTP calls
- Crash recovery: `reconnect()` provides a standard protocol instead of
  per-executor convention
- SLURM DAG submission: the bridge naturally extends SLURM from individual
  job submission to dependency-chained batches via `--dependency=afterok`
- Testing: bridge methods are independently testable (mock the HTTP client,
  verify generated scripts, test state reconciliation)

**What becomes harder or riskier:**
- Two traits to implement for remote backends (Executor + ExecutorBridge)
  instead of one. Mitigated: local executor only implements Executor.
- Migration: existing Ray `submit_dag()` moves from Executor to ExecutorBridge.
  This is a refactor of call sites in `run.rs`, not a behavior change.
- `ResourceSpec` enum grows with each backend. Acceptable: it's an internal
  type, not a user-facing API.

## Alternatives Considered

**1. Keep everything in the Executor trait (status quo)**
The current `submit_dag()` on `Executor` works but mixes job-level and
DAG-level concerns. Adding `poll_dag_status()`, `reconnect()`, and
`sync_results()` to `Executor` would bloat the trait with methods that
only apply to remote backends. Local executor would need stub implementations
for all monitoring methods.

**2. Abstract Factory / ExecutorProvider**
A factory that produces both Executor and Bridge instances. Adds indirection
without clear benefit — the two traits are independently useful and don't
need coordinated construction.

**3. Airflow-style binary operator plugins**
Each backend as a dynamically-loaded plugin with a standard interface.
Rejected per ADR-003: OxyMake prefers generated scripts over binary
plugins for debuggability and portability. The bridge generates scripts;
it doesn't embed executor SDKs.

**4. Separate SUBMIT / MONITOR / CONTROL traits**
Three fine-grained traits instead of one ExecutorBridge. Overly granular:
all three directions share the same connection state (Ray client, SLURM
config) and the same meta.json. Splitting them forces either trait
inheritance or shared state wrappers.
