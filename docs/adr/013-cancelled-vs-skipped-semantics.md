# ADR-013: Distinct JobCancelled vs JobSkipped Semantics

## Status
Accepted

## Metadata

- **Kind:** `architecture-note`
- **Family:** `STM`
- **Supersedes:** `none`

> **Vocabulary note (2026-05-27, M1 vocabulary alignment).** The word
> **skip** historically carried two unrelated meanings: *cache hit*
> (no execution because outputs already match) and *guard exclusion* (no
> execution because a guard or flag said so). ADR-014 began the split at the
> database level by introducing the `cached` flag. This ADR completes the
> vocabulary half: the conceptual outcomes are now
>
> - **cache_hit** — cache-driven skip, normal and desired
> - **excluded** — guard- or flag-driven skip, intentional bypass
> - **cancelled** — upstream-failure or run-termination skip, not a skip at all
>
> The event variants `JobSkipped` and `JobCancelled` and the on-disk status
> values are kept as-is for serialization compatibility; the vocabulary above
> is the lens for reading them.

## Context
When a job does not execute, the reason matters for diagnostics and metrics:

- A **cache_hit** (historically: *skipped*) means the job was not executed
  because its outputs are already up-to-date. This is normal, expected, and
  desirable — it means the cache is working.
- An **excluded** outcome (historically: *skipped*) means the job was not
  executed because a guard, flag, or pre-marked status said so.
  Discriminated from cache_hit by the cache-source attribution.
- A **cancelled** outcome means the job was not executed because an upstream
  dependency failed or the user terminated the run. This indicates a problem
  (or intentional abort), not a cache win.

Prior to this decision, both cache_hit and cancelled cases emitted
`Event::JobSkipped` and were recorded with `skip_job()` in the state database.
This conflated semantically distinct outcomes, making it impossible to
distinguish "90% cache hit rate" from "90% of jobs cancelled due to early
failure" in run summaries and dashboards.

## Decision
Introduce `Event::JobCancelled { job_id, reason }` as a distinct event variant,
separate from `Event::JobSkipped { job_id, reason }`:

| Outcome (M1 vocabulary) | Event           | Ledger method        | DB status | cached flag |
|-------------------------|-----------------|----------------------|-----------|-------------|
| cache_hit               | `JobSkipped`    | `skip_job()`         | completed | 1           |
| excluded (guard/flag)   | `JobSkipped`    | `skip_job()`         | completed | 1*          |
| cancelled (upstream)    | `JobCancelled`  | `cancel_job_ids()`   | cancelled | 0           |
| cancelled (run kill)    | `JobCancelled`  | `cancel_job_ids()`   | cancelled | 0           |

\* Guard-excluded jobs currently route through the same `skip_job()` path as
cache hits ; future work (tracked in M-followup) may split them further so
that *excluded* is distinguishable from *cache_hit* without inspecting the
guard configuration.

The scheduler's `cancel_downstream()` and `cancel_remaining()` functions emit
`JobCancelled`. The cache-check and guard-check paths emit `JobSkipped`.

`RunCompleted` tracks four distinct counts: `succeeded`, `failed`, `skipped`,
and `cancelled`. Reporters display all four.

EventSink handlers map each variant to the correct Ledger method:
- `JobSkipped` → `skip_job()` (pending → completed, cached=1)
- `JobCancelled` → `cancel_job_ids()` (pending/running → cancelled)

## Consequences

**Easier:**
- Run summaries accurately report cache effectiveness vs failure impact.
- Dashboards can color-code cancelled (red/amber) differently from cache_hit
  (green/grey).
- Historical analysis can answer "how often do failures cascade?" separately
  from "what's the cache hit rate?"
- Cancellation reasons are preserved in the event payload for debugging.

**Harder:**
- Every event consumer (reporters, sinks, metrics) must handle both variants.
  Adding `JobCancelled` required updates to terminal reporter, JSON reporter,
  dashboard EventSink, CLI EventSink, and metrics collector.
- The lifecycle enum has both `Skipped` and `Cancelled` — new code must choose
  the correct one (compiler does not enforce the cache_hit / excluded /
  cancelled distinction below the `Skipped` umbrella).

## Alternatives Considered

**Single event with a `kind` field**: `JobNotExecuted { job_id, kind: Skip |
Cancel, reason }`. Keeps one event variant but pushes the distinction into a
nested enum. Rejected because match arms on `Event` are the natural dispatch
point — two variants is clearer than one variant with an inner match.

**Status flags instead of distinct status**: Keep `JobSkipped` for both but add a
`cancelled: bool` flag to the event and DB. Rejected because the database
`status` column is the primary query dimension — `WHERE status='cancelled'` is
simpler and more efficient than `WHERE status='completed' AND cancelled=1 AND
cached=0`.

**No distinction (keep conflating)**: Simpler, fewer event variants. Rejected
because the ambiguity was already causing confusion in dashboard metrics and run
summaries — the operational cost of the conflation exceeded the code cost of
the split.
