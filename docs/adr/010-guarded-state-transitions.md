# ADR-010: Guarded State Transitions with WHERE Preconditions

## Status
Accepted

## Metadata

- **Kind:** `decision`
- **Family:** `STM`
- **Supersedes:** `none`

## Context
Oxymake's state database records job lifecycle transitions (pending → running →
completed/failed/cancelled). Multiple callers — the event bridge, cooperative
sessions, and cancellation paths — can attempt the same transition concurrently
or out of order. Without enforcement, a stale event could overwrite a terminal
state (e.g., marking a completed job as running again), corrupting the audit
trail and metrics.

Prior to this decision, transitions were unconditional UPDATEs that always
succeeded regardless of the job's current state.

## Decision
Every state-mutating method in `StateDb` enforces a WHERE precondition on the
current status and returns a `bool` indicating whether the transition fired:

| Method | Guard (WHERE) | Transition |
|--------|---------------|------------|
| `claim_job` | `status='pending'` | pending → running |
| `complete_job` | `status='running'` | running → completed |
| `fail_job` | `status='running'` | running → failed |
| `skip_job` | `status='pending'` | pending → completed (cached=1) |
| `cancel_job_ids` | `status IN ('running','pending')` | → cancelled |

The caller inspects the return value to decide whether to proceed (e.g., emit
downstream events) or silently no-op. No panics, no errors — just a boolean
indicating whether the precondition held.

## Consequences

**Easier:**
- Out-of-order event processing is safe; late-arriving events are harmlessly
  dropped by the guard.
- Cooperative multi-session claiming is race-free without application-level locks
  — SQLite's single-writer serialization plus the WHERE clause provides atomicity.
- Idempotent retries: re-applying a transition on an already-transitioned job
  returns `false` but never corrupts state.

**Harder:**
- Callers must handle the `false` case (most ignore it, which is correct for
  event bridges but must be conscious for claim contention).
- The transition graph is implicit in scattered SQL strings rather than encoded
  as a state machine type — adding a new status requires auditing all methods.

## Alternatives Considered

**Application-level Mutex around transitions**: Would serialize all transitions
through a single lock, eliminating the need for WHERE guards. Rejected because
it forces single-threaded access to the database and doesn't survive process
crashes (lock not released).

**Explicit state machine type with compile-time transition checks**: A Rust enum
encoding valid transitions (e.g., `Pending::claim() -> Running`). Rejected as
over-engineering for the current scale — the number of transitions is small and
the SQLite guard provides runtime safety. Could be reconsidered if the state
graph grows significantly.

**Optimistic locking with version column**: Add a `version` column and
`WHERE version = ?` on every update. Rejected because status-based guards are
more semantically meaningful (the precondition IS the status, not an opaque
counter) and sufficient for the current transition graph.
