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
- No lock servers, no ZooKeeper: SQLite is the coordination primitive for
  *claiming* jobs. (Until the claim is the scheduling gate, the local
  executor additionally uses per-output-path `flock` files under
  `.oxymake/locks/` — see *Known limitation* below.)
- Crash recovery is automatic: stale heartbeat → reclaim. No manual cleanup
  required.
- The claim protocol works on NFS/network filesystems where SQLite WAL is
  supported (same guarantees as local disk). The interim output-path locks
  do **not** carry that guarantee (see below).
- Session identity includes UUIDv4 suffix, preventing PID-reuse collisions.

**Harder:**
- Heartbeat interval determines crash detection latency. A 30s heartbeat means
  up to 30s + threshold before orphaned jobs are reclaimed.
- No work stealing: sessions only claim pending jobs. A long-running job on a
  slow session cannot be redistributed.
- SQLite's single-writer lock means claim contention serializes at the database
  level. At very high session counts (>10 concurrent), this could become a
  throughput bottleneck.

## Known limitation (2026-09-05)

The claim protocol is not yet consulted by the scheduler before it launches
a job: `claim_job` is recorded from the `JobStarted` event, after dispatch.
Two sessions that both reach a job (for example two `ox run` approved for
the same gate) therefore both try to execute it.

Because their outputs share the same final paths, letting both run would
interleave their writes and could commit an output set mixing files from two
physical executions — a corrupt, non-reproducible result reported as success
(issue #2, round-2 finding 1). An earlier revision tried to make the
duplicate harmless by having the loser *adopt* its peer's committed outputs;
that is unsound for a non-deterministic or multi-output job and has been
reverted.

The local executor now **fails closed** instead: `prepare_workspace` takes an
exclusive, non-blocking advisory lock (`flock`) on **each output path** of
the job before touching any file, and holds them until `finalize_workspace`
commits. Locks are taken in globally sorted order of the path's key so two
sessions can never deadlock; on the first contended path the session
releases what it took and aborts that job with
`ExecLocalError::ConcurrentExecution` (exit status 1), naming the holder and
the locked path, without disturbing the winner's outputs. Because the lock
is per path, two jobs contend as soon as their output sets *intersect*, not
only when they are equal; and the key is derived from the canonicalised
deepest existing ancestor of the path, so two spellings of one file through
a symlinked directory contend too (round-3 finding 2).

Each lock is tied to an open file description, so the kernel releases it
when the last descriptor closes — a crashed session leaves no stale lock.
The job subprocess inherits a duplicate of every lock descriptor (made in
the child only, after `fork`), so a session killed with `SIGKILL` mid-job
does **not** release the locks while its orphaned job shell can still write
the final paths: a replacement run fails closed, naming the exited session,
until that job exits (round-3 finding 1). The lock files live under
`.oxymake/locks/` and are inert once unlocked; they are never a stale
marker.

**Scope of the guarantee.** On a local filesystem with a working `flock(2)`,
two physical executions never contribute files to one committed set. On
NFS and other distributed filesystems `flock` may not be mutually visible
between hosts (or may be emulated per host), and the executor cannot detect
that; sessions on *different hosts* sharing such a directory are not
protected by these locks (the claim protocol above still is, once it gates
scheduling). On targets without `flock` (non-Unix) the executor refuses to
run a job with file outputs (`ExecLocalError::LockUnsupported`) rather than
pretend the lock is held (round-3 finding 3).

This is conservative: the losing session fails its run rather than sharing
the work. The complete fix is to make `claim_job` the scheduling gate so the
scheduler defers to the owning session before dispatch (the loser then waits
for and consumes the peer's terminal state instead of re-running); the
output-path locks then become a defence in depth rather than the guarantee.
That is the open follow-up.

## Alternatives Considered

**File-based advisory locks (flock/fcntl) as the coordination primitive**:
One lock file per job for *claiming* work. Doesn't work reliably on all NFS
implementations and scales poorly with job count, so it was rejected as the
*claim* mechanism in favour of SQLite. (The concern that a crashed holder
strands the lock does not apply to `flock`, which the kernel releases with
the last descriptor.) The per-output-path `flock` of the *Known limitation*
above is not a reversal of this decision: it is a local-filesystem
fail-closed guard against the one race the unfinished claim protocol still
allows, explicitly scoped to hosts with a working `flock`, and it is meant
to become defence in depth once the claim gates scheduling.

**Postgres/distributed database**: Full MVCC, row-level locking, LISTEN/NOTIFY
for real-time coordination. Far more capable but introduces an external
dependency and operational complexity disproportionate to the coordination needs.
Rejected for the daemon-free architecture (ADR-005).

**Optimistic concurrency with retry loops**: Read status, compute, write with
version check, retry on conflict. More complex than a single conditional UPDATE
and offers no advantage when SQLite already serializes writes. Rejected.
