# ADR-012: Cooperative Multi-Session via SQLite Atomic Claims

## Status
Accepted

## Metadata

- **Kind:** `decision`
- **Family:** `COOP`
- **Supersedes:** `none`

## Context
Oxymake supports running multiple `ox run` processes concurrently against the
same project directory — for example, one session building mouse targets while
another builds human targets, or a user re-running while a previous run is still
completing. Each session must claim jobs without double-execution, detect crashed
peers, and reclaim orphaned work.

Traditional approaches use explicit distributed locks (file locks, advisory DB
locks, or external coordination services). These add complexity and failure modes
— a crashed process may hold a lock indefinitely.

## Decision
Cooperative multi-session coordination uses SQLite's single-writer serialization
as the sole coordination mechanism:

**Job claiming** is an atomic conditional UPDATE:
```sql
UPDATE jobs SET status='running', session_id=?, locked_by=?
WHERE id=? AND status='pending'
```
SQLite guarantees only one writer executes at a time. If two sessions race to
claim the same job, exactly one succeeds (rows_affected=1) and the other gets
rows_affected=0. No application-level locks needed.

**Session lifecycle**:
- `create_session(pid, hostname, target_filter)` registers a session with a
  unique ID (`s-{pid}-{timestamp}-{uuid}`)
- `heartbeat(session_id)` updates `heartbeat_at` periodically (~30s)
- `complete_session()` or `interrupt_session()` marks terminal state
- `find_stale_sessions(threshold_secs)` identifies sessions whose heartbeat
  exceeds the threshold
- `reclaim_stale_jobs(session_id)` resets orphaned running jobs back to pending

**Crash recovery**: A surviving session detects stale peers via heartbeat age
and reclaims their in-progress jobs. The reclaimed jobs re-enter the pending
pool and can be claimed by any active session.

## Consequences

**Easier:**
- Zero external dependencies: no lock files, no lock servers, no ZooKeeper.
  SQLite is the only coordination primitive.
- Crash recovery is automatic: stale heartbeat → reclaim. No manual cleanup
  required.
- Works correctly on NFS/network filesystems where SQLite WAL is supported
  (same guarantees as local disk).
- Session identity includes UUIDv4 suffix, preventing PID-reuse collisions.

**Harder:**
- Heartbeat interval determines crash detection latency. A 30s heartbeat means
  up to 30s + threshold before orphaned jobs are reclaimed.
- No work stealing: sessions only claim pending jobs. A long-running job on a
  slow session cannot be redistributed.
- SQLite's single-writer lock means claim contention serializes at the database
  level. At very high session counts (>10 concurrent), this could become a
  throughput bottleneck.

## Alternatives Considered

**File-based advisory locks (flock/fcntl)**: One lock file per job. Simple but
doesn't survive crashes cleanly (lock held by dead process until OS cleans up),
doesn't work reliably on all NFS implementations, and scales poorly with job
count. Rejected.

**Postgres/distributed database**: Full MVCC, row-level locking, LISTEN/NOTIFY
for real-time coordination. Far more capable but introduces an external
dependency and operational complexity disproportionate to the coordination needs.
Rejected for the daemon-free architecture (ADR-005).

**Optimistic concurrency with retry loops**: Read status, compute, write with
version check, retry on conflict. More complex than a single conditional UPDATE
and offers no advantage when SQLite already serializes writes. Rejected.
