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

**Read-side derivation (the readers' half of the same rule)**: between the
moment a session dies and the moment the next `ox run` reclaims its rows,
`state.db` holds `running` rows that nobody is executing. Readers must not
present them as live work — `ox status` printing "Sessions: 0 active" next
to "1 running" was the symptom (issue #5). Nor may a reader fix them: a
viewer that reset rows would race a session whose heartbeat is merely late
under load, and `ox status` / the dashboard / `ox top` have no claim to
write. So the reconciliation lives in one read-only derivation in
`ox-state` — `ox_state::effective::effective_status` and the `job_views`
query that LEFT JOINs `jobs` to `sessions` — which every reader consumes.
The rule is the claim protocol's own predicate, applied on the read path:
a `running` row is `Running` only if `sessions.status = 'active'` **and**
`heartbeat_at > now - lease`; otherwise it is
`Orphaned { since, reason }` with `reason` one of `SessionInterrupted`,
`HeartbeatStale`, `SessionCompleted`, `SessionMissing`. Because readers and
peers evaluate the same predicate against the same lease
(`DEFAULT_LEASE_SECS`, `OX_SESSION_LEASE_SECS`), what a reader calls
orphaned is exactly what the next `ox run` will reclaim — the readers are
honest about the interval without shortening it. Reclaiming remains the
writer's, in `reset_inactive_job_rows` at run start-up.

## Consequences

**Easier:**
- No lock servers, no ZooKeeper: SQLite is the coordination primitive for
  *claiming* jobs. The local executor additionally holds per-output-path
  `flock` files under `.oxymake/locks/` as defence in depth (see
  *Dispatch-time claim* below).
- Crash recovery is automatic: stale heartbeat → reclaim. No manual cleanup
  required.
- The claim protocol works on NFS/network filesystems where SQLite WAL is
  supported (same guarantees as local disk). The output-path locks do
  **not** carry that guarantee (see below); they are no longer what the
  exactly-once property rests on.
- Session identity includes UUIDv4 suffix, preventing PID-reuse collisions.

**Harder:**
- Heartbeat interval determines crash detection latency. With the default
  lease of 90 s (heartbeat every 30 s) a peer waits up to 90 s after a crash
  before it reclaims the dead session's jobs.
- A peer's *failure* is mirrored while that peer is live. With
  `ErrorStrategy::Retry` the waiting session therefore sees the failed
  attempt, retries its claim, loses it again to the retrying owner and
  consumes the owner's next verdict — its own retry budget is spent on
  mirrored attempts, not on executions.
- A session that loses a claim waits for the owner even when it wanted to
  `--forcerun` the job: with a live owner executing the job, forcing a
  second concurrent execution would reintroduce the mixed-output race.
- No work stealing: sessions only claim pending jobs. A long-running job on a
  slow session cannot be redistributed.
- SQLite's single-writer lock means claim contention serializes at the database
  level. At very high session counts (>10 concurrent), this could become a
  throughput bottleneck.

## Dispatch-time claim (2026-09-05, issue #3)

The claim **is** the scheduling gate. Before it dispatches a job the
scheduler (`ox-core`) calls the `JobClaim` trait — the same injection
pattern as `GateCheck`: the trait lives in `ox-core`, the implementation
(`StateJobClaimer`, `ox-state/src/claim.rs`) is built by `ox run` over
`state.db`, and `ox-core` keeps no dependency on `ox-state`.

- **Won** → this session executes the job. The claim is idempotent per
  session, so a job deferred by the resource budget or `-j` is claimed
  again on its next dispatch attempt.
- **Lost** → the job is *not* launched here. It leaves the ready frontier
  and is polled every 500 ms (the gate poll interval, observing Ctrl+C /
  SIGTERM like a pending gate — a waiting run stops on the first signal),
  also while this session's own jobs are in flight. `ox run` prints
  `<job> is being executed by <session>; waiting for its result instead of
  running it here`.
- The owner's terminal state is then **consumed** as this session's own:
  `completed` promotes the downstream jobs and records the cache exactly as
  a local success would (the job shows as done in both sessions, exactly
  one execution happened); `failed` is mirrored through the job's error
  strategy; `cancelled` is mirrored and cancels downstream. A failure or
  cancellation is mirrored only while its author is **live** — an
  `active` session with a heartbeat younger than the lease. A verdict left
  by a finished, interrupted or dead session is history: the row is reset
  and the job re-run here. (`ox run` records `interrupted` on its first
  signal, before its killed jobs are terminalized, so a peer never mirrors
  an interruption as a failure.)
- **Lease.** `ox run` heartbeats its session every third of the lease
  (default 90 s, `OX_SESSION_LEASE_SECS` overrides). When the owner's
  heartbeat goes stale the waiting session reclaims its running jobs with
  `reclaim_stale_jobs_if_stale` — there is exactly one lease — and the
  next claim wins: a session killed with `kill -9` mid-job is replaced by
  its waiter after at most one lease. The reclaim is one SQLite
  transaction whose `UPDATE`s are guarded by the owner's *current*
  heartbeat (`heartbeat_at <= now - lease`), not by the heartbeat the
  waiter read earlier: an owner that heartbeats between the waiter's read
  and the reclaim keeps its rows and the waiter keeps waiting, so a live
  owner is never interrupted by a stale observation. If the killed
  session's orphaned job shell is still writing at that moment, the
  output-path locks below make the replacement fail closed rather than
  commit next to it.
- **Clock.** The lease is measured on the **wall clock** in UNIX seconds:
  `heartbeat` stores the owner's `now`, and the liveness test
  `now - heartbeat_at < lease` is evaluated with the *reader's* `now`. The
  protocol assumes one non-decreasing clock shared by every session on the
  same `state.db`. On one host that is the system clock. When sessions on
  several hosts share a `state.db` over a network filesystem, it assumes
  their clocks are synchronised (NTP) to well within the lease: a skew, or
  a backward step on one host, larger than the lease makes a live owner
  read as dead (reclaimed while running — the output locks are then the
  only defence, and they do not cross hosts) or a dead one as live (not
  reclaimed) for one lease. Nothing detects either; the lease is not a
  substitute for synchronised clocks. Lengthen `OX_SESSION_LEASE_SECS`
  where clocks are less trustworthy.
- **Suspend / resume.** A suspended `ox run` (laptop lid, `SIGSTOP`) stops
  heartbeating and is treated exactly like a crashed one: after one lease
  its running jobs are reclaimed and its session row is `interrupted`.
  That path fails closed for the resumed owner: its heartbeat only updates
  an `active` row and now does nothing; its terminal writes
  (`complete_job` / `fail_job`) carry `AND session_id = ?` and update
  zero rows once a peer has re-claimed the job, so the resumed owner
  cannot overwrite the peer's verdict — it reports its local result and
  exits, its session stays `interrupted`. On the same host the suspended
  owner's shell still holds the per-output `flock`s, so a peer that
  re-claims the job while the owner is suspended cannot execute it either:
  it fails closed with `ConcurrentExecution` until the owner's process is
  gone. If the owner resumes *before* any peer re-claims (the row is
  `pending`, unowned), its completion event re-claims the row and records
  the completion — the outputs it wrote are real, and a peer then consumes
  them as a completion. In no ordering does the resumed owner replace a
  peer's result.
- **Old rows.** `register_jobs` keeps existing statuses, so a fresh run
  must not lose its claim to history. Right after registration `ox run`
  resets, in one transaction, the `running`, `failed` and `cancelled` rows
  of its jobs whose owner is not live (a crashed or finished peer), so a
  dead session's verdicts are not mirrored. `completed` rows (yesterday's
  run, a cached row) are left in place and handled lazily by the claim:
  a cache hit never claims, and a job that does execute finds the old
  completion at claim time and resets it unless its author is live or it
  was recorded *strictly after* this session started (UNIX seconds — a
  completion in the same second as the start is history, so two runs
  within one second cannot make the second consume a completion whose
  outputs the first no longer guarantees). Resetting every completed row
  eagerly cost a warm 1001-job run about 100 ms, because the 999 cache-hit
  `skip_job` writes stopped being no-ops (#3, round-1 QA finding 4). Rows
  owned by a live peer are kept: that peer is the concurrent session this
  protocol serves. A session that stops waiting (interrupt) cancels only
  unclaimed pending rows and its own running rows, never a peer's.
- A session only ever reaches jobs in its own graph, so it never blocks on
  a peer's job whose outputs it does not need.

`spec/tla/CooperativeClaim.tla` models exactly this order — `Claim`
before `Terminalize`, `Reclaim` after staleness; the pre-#3 code recorded
the claim from the `JobStarted` event, *after* dispatch, and thereby
diverged from the spec. The change adds no transition (the waiter's poll
is a read of `status` / `done_by`; a retry after `Reclaim` is another
`Claim`), the suite was re-run unchanged and stays green.

**Defence in depth: per-output-path locks.** The local executor keeps the
locks landed for issue #2: `prepare_workspace` takes an exclusive,
non-blocking `flock` on **each output path** of the job before touching
any file and holds it until `finalize_workspace` commits; locks are taken
in sorted key order (deadlock-free), keyed on the canonicalised deepest
existing ancestor of the path (two spellings of one file contend); on the
first contended path the session aborts that job with
`ExecLocalError::ConcurrentExecution`, naming the holder and the path. The
job subprocess inherits a duplicate of every lock descriptor, so a
`SIGKILL`ed session's orphaned job keeps the locks until it exits. On a
local filesystem with a working `flock(2)` this guarantees that two
physical executions never contribute files to one committed set even if
the claim were bypassed; on NFS and other distributed filesystems `flock`
may not be visible between hosts — there the claim protocol above is the
guarantee. On targets without `flock` (non-Unix) a job with file outputs
is refused (`ExecLocalError::LockUnsupported`).

**Out of scope.** Distributed executors (`--executor slurm` / `ray`)
submit the DAG without the scheduler and keep refusing gated workflows.

## Alternatives Considered

**File-based advisory locks (flock/fcntl) as the coordination primitive**:
One lock file per job for *claiming* work. Doesn't work reliably on all NFS
implementations and scales poorly with job count, so it was rejected as the
*claim* mechanism in favour of SQLite. (The concern that a crashed holder
strands the lock does not apply to `flock`, which the kernel releases with
the last descriptor.) The per-output-path `flock` of *Dispatch-time claim*
above is not a reversal of this decision: it is a local-filesystem
fail-closed guard kept as defence in depth behind the claim, explicitly
scoped to hosts with a working `flock`.

**Postgres/distributed database**: Full MVCC, row-level locking, LISTEN/NOTIFY
for real-time coordination. Far more capable but introduces an external
dependency and operational complexity disproportionate to the coordination needs.
Rejected for the daemon-free architecture (ADR-005).

**Optimistic concurrency with retry loops**: Read status, compute, write with
version check, retry on conflict. More complex than a single conditional UPDATE
and offers no advantage when SQLite already serializes writes. Rejected.
