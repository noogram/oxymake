# ADR-007: Enriched `ox run` Output — Inspired by Snakemake Job Reporting

## Status
Accepted (Phase 1 implemented: RunReason, verbose levels, timings)

## Metadata

- **Kind:** `decision`
- **Family:** `EXT`
- **Supersedes:** `none`

## Context

Snakemake's `ox run` equivalent prints structured per-job context (rule name,
wildcards, output files, *reason* for execution) and an upfront job-stats table.
Ninja uses compact `[N/M] description` single-line progress. Bazel adds
critical-path timing and phased output.

Current `ox run` output is functional but leaves users asking **"why did this
job run?"** — the single most useful piece of information for incremental builds.
The data to answer this already exists in the cache layer (`CacheHitStatus`)
but is discarded before reaching the reporter.

### Current output

```
  Resolving 100 jobs (50 to run, 50 cached)
  Cache: 50 of 100 job(s) up-to-date, skipping.
  ████████████████░░░░░░░░░░░░░░ 65/100 jobs [00:02:34]
  # at -v:
  [start] align-A (local)
  [1/100] ✓ align-A (1.2s)
  [2/100] ✓ align-B [cached]
  # summary:
  ✓ Completed 100/100 in 2m34s (0.6 jobs/s)
    50 succeeded, 50 skipped
```

### What's missing

1. **No run reason** — why does each job execute?
2. **No job stats table** — no per-rule breakdown before execution
3. **Cached lines are noise** — 50 `[cached]` lines add clutter, not insight

## Decision

Three phased enrichments, ordered by value:

### Phase 1: Run Reason per Job (high value)

Add a `RunReason` enum and propagate it through the event bus.

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunReason {
    /// No cache entry for this input hash.
    CacheMiss,
    /// Output file(s) deleted from disk.
    OutputMissing { path: String },
    /// Output content changed since last cache record.
    OutputStale { path: String },
    /// An upstream dependency was rebuilt in this run.
    UpstreamRebuilt,
    /// --no-cache flag.
    CacheDisabled,
    /// Job has no cacheable outputs (phony targets).
    NotCacheable,
    /// --force flag.
    Forced,
}
```

**Mapping from existing `CacheHitStatus`:**

| `CacheHitStatus` | `RunReason` | Display |
|---|---|---|
| `Miss` | `CacheMiss` | "no cache entry" |
| `OutputMissing { path }` | `OutputMissing { path }` | "output missing: {path}" |
| `Mismatch { path }` | `OutputStale { path }` | "output stale: {path}" |
| (--no-cache) | `CacheDisabled` | "cache disabled" |
| (no outputs) | `NotCacheable` | "not cacheable" |
| (--force) | `Forced` | "forced" |

**Event change:** Extend `Event::JobStarted` with `reason: Option<RunReason>`.
`Option` for backward compatibility — `None` means reason unknown.

**Data flow:**
1. `run.rs` cache pre-scan (lines 494–526) already calls `check_cached()` and
   gets `CacheHitStatus`. Map non-Hit statuses to `RunReason`, store in
   `HashMap<JobId, RunReason>`.
2. Pass map to scheduler via new `SchedulerConfig::run_reasons` field.
3. Scheduler emits reason in `JobStarted` event.
4. For `UpstreamRebuilt`: when a job completes and any downstream job was not in
   `skip_jobs`, mark it as `UpstreamRebuilt` (overrides the original reason).

**Display behavior:**

| Verbosity | Behavior |
|---|---|
| default (v=0) | Show reason only for "interesting" cases: OutputMissing, OutputStale, UpstreamRebuilt. CacheMiss is the common case — silent. |
| `-v` (v=1) | Show reason for all started jobs. |
| `-vv` (v=2) | Same as -v plus full output streaming. |

### Phase 2: Job Stats Table (medium value)

New event `Event::RunPlan` emitted after DAG resolution, before `RunStarted`:

```rust
Event::RunPlan {
    rule_counts: Vec<(RuleName, usize)>,  // sorted by count desc
    to_run: usize,
    cached: usize,
}
```

**Display (when > 1 distinct rule AND to_run > 0):**

```
  Job plan: 7 to run, 29 cached (36 total)
    align            5
    collect          1
    merge_all        1
```

Replaces the current `Cache: N of M job(s) up-to-date` line. Provides
upfront scope awareness (Snakemake's strongest UX contribution).

**Implementation:** After cache scan, group non-skipped jobs by
`ConcreteJob::rule`, sort by count desc. Simple `format!` with padding — no
external table crate needed.

### Phase 3: Cached Job Display Refinement (low value)

At v=0, suppress individual `[N/M] ✓ job_id [cached]` lines. The stats table
already communicates cached count. Individual lines are noise.

At v≥1, keep individual cached lines for debugging.

### Example: After All Phases

```
$ ox run
  Job plan: 7 to run, 29 cached (36 total)
    align            5
    collect          1
    merge_all        1

  ▸ align[sample=A] — output missing: results/A.bam
  ▸ align[sample=B] — output missing: results/B.bam
  [3/36] ✓ align[sample=A] (1.2s)
  [4/36] ✓ align[sample=B] (1.1s)
  ▸ align[sample=D] — upstream rebuilt
  ...
  ✓ Completed 36/36 in 2m34s (0.2 jobs/s)
    7 succeeded, 29 cached
```

```
$ ox run -v
  Job plan: 7 to run, 29 cached (36 total)
    align            5
    collect          1
    merge_all        1

  [start] align[sample=A] (local) — output missing: results/A.bam
  [start] align[sample=B] (local) — output missing: results/B.bam
  [1/36] ✓ align[sample=A] (1.2s)
  [2/36] ✓ align[sample=B] (1.1s)
  [3/36] ✓ align[sample=C] [cached]
  ...
```

## Consequences

**Easier:**
- Users immediately understand why incremental builds re-execute jobs
- Per-rule breakdown sets expectations before long runs
- Debugging stale outputs / missing files becomes trivial
- CI logs are more informative without -v flag

**More difficult:**
- `Event::JobStarted` grows by one `Option` field (trivial serde cost)
- Scheduler must accept and propagate reason map (moderate plumbing)
- `UpstreamRebuilt` detection requires scheduler cooperation (most complex part)

**Zero-cost principle preserved:**
- `RunReason` is a small enum — no heap allocation
- Cache pre-scan already does the work; we capture rather than discard
- Stats table is computed once from data already in memory
- No new external dependencies

## Alternatives Considered

### 1. Snakemake-style multi-line blocks per job
```
rule align:
    input: data/A.fastq
    output: results/A.bam
    reason: Missing output files
```
**Rejected:** Too verbose for fast builds. Oxymake jobs are often sub-second.
A 5-line block per job would drown the terminal for 100+ job DAGs.

### 2. Timestamps per job (Snakemake style)
**Rejected:** Noise for local execution. The progress bar's elapsed time and
per-job duration in `[N/M] ✓ job (1.2s)` provide sufficient timing.

### 3. Bazel-style critical path in summary
```
INFO: Critical Path: 8.12s
```
**Deferred:** Valuable but orthogonal. Would require tracking the longest
serial dependency chain through the DAG. Good future enhancement, separate ADR.

### 4. External table-formatting crate (comfy-table, tabled)
**Rejected:** The stats table is simple enough for `format!` with calculated
padding. Adding a dependency for cosmetic formatting violates the zero-cost
principle.

## Files Affected

| File | Change |
|------|--------|
| `crates/ox-core/src/model.rs` | Add `RunReason` enum, `RunPlan` event, extend `JobStarted` |
| `crates/ox-cli/src/commands/run.rs` | Collect reasons in cache scan, emit `RunPlan` |
| `crates/ox-core/src/scheduler.rs` | Accept/propagate run reasons, detect `UpstreamRebuilt` |
| `crates/ox-report-term/src/reporter.rs` | Display reasons and stats table |
| `crates/ox-report-term/src/format.rs` | `RunReason` display formatting |
