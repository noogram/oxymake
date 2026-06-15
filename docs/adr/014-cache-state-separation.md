# ADR-014: Cache-State Separation with Dedicated Cached Flag

## Status
Accepted

## Metadata

- **Kind:** `decision`
- **Family:** `CAS`
- **Supersedes:** `none`

## Context
When a job's outputs are already cached, oxymake skips execution and records the
job as completed. For metrics and history, we need to distinguish "completed by
execution" from "completed by cache hit" — both produce valid outputs, but only
executed jobs consumed compute time.

The initial schema stored cache hits as `status='skipped'`. This created a
semantic collision: "skipped" meant both "cache hit" and "excluded by guard
condition," and cached jobs didn't contribute to completion counts, making
progress bars inaccurate (a 1000-job DAG with 900 cache hits showed 100 total
instead of 1000).

## Decision
Cache hits are stored as `status='completed'` with a separate `cached` boolean
column (INTEGER 0/1) in the `jobs` table:

```sql
-- Cache hit: job outputs are valid, counts toward completion
UPDATE jobs SET status='completed', cached=1
WHERE id=? AND status='pending'

-- Execution completion: job ran and succeeded
UPDATE jobs SET status='completed', cached=0, exit_code=?, output_hashes=?
WHERE id=? AND status='running'
```

`JobCounts` reports `completed` (ran and succeeded), `cached` (cache hits), and
`total_done` (completed + cached + failed + cancelled) as separate fields.

The `skip_job()` method in StateDb sets both `status='completed'` and `cached=1`
in a single atomic transition from `pending`.

## Consequences

**Easier:**
- Progress bars are accurate: cached jobs count toward the denominator.
  A 1000-job DAG with 900 cache hits shows "900/1000 done" immediately.
- Metrics distinguish compute cost: `completed` = actual work done,
  `cached` = work avoided. Cache hit rate = `cached / (completed + cached)`.
- Queries for "all jobs with valid outputs" are just `WHERE status='completed'`
  regardless of how they got there.
- Historical trend analysis can track cache effectiveness over time.

**Harder:**
- Two columns encode one concept: callers must check both `status` and `cached`
  to fully characterize a job's outcome.
- The `cached` flag is only meaningful when `status='completed'`. A cancelled job
  with `cached=0` is not "ran and failed to cache" — it never ran. The flag's
  semantics are context-dependent.
- Schema migration (v8 → v9) required updating all existing `status='skipped'`
  rows to `status='completed', cached=1`.

## Alternatives Considered

**Separate `cached` status value**: Add `cached` as a distinct status
(`pending | running | completed | failed | cached | cancelled`). Rejected
because it bifurcates "has valid outputs" into two statuses, complicating every
query that needs to know "is this job done?" (`WHERE status IN ('completed',
'cached')`).

**Cache metadata in a separate table**: A `cache_hits` table recording which
jobs were cache hits. Keeps the jobs table clean but requires a JOIN for any
query combining execution and cache data. Rejected as unnecessary complexity
for a single boolean distinction.

**Encode in `exit_code`**: Use a sentinel exit code (e.g., -1) for cache hits.
Rejected because `exit_code` has established POSIX semantics and sentinel values
are fragile — any code checking `exit_code == 0` for success would need to also
check for the sentinel.
