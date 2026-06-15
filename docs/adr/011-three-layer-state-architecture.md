# ADR-011: Three-Stage State Pipeline

## Status
Accepted

## Metadata

- **Kind:** `architecture-note`
- **Family:** `STM`
- **Supersedes:** `none`

> **Vocabulary note (2026-05-27, M1 vocabulary alignment).** This ADR
> previously described a "three-layer state architecture." The word *Layer*
> conflated pipeline ordering with containment hierarchy. The pipeline meaning
> is now named **Stage**; the word **Stratum** is reserved for future
> transactional/resident layering work. The Bridge that crosses from the
> event bus into SQLite is renamed **EventSink** (or **Projection** when the
> event-sourcing reading is intended), so that `ExecutorBridge` (ADR-008) and
> the db-side bridge are no longer two unrelated objects sharing one word.
> The in-memory scheduler state previously called `SchedulerState` is the
> **Frontier**; the persisted audit store previously typed `StateDb` is the
> **Ledger**; the in-memory enum previously called `ScheduleStatus` is the
> **JobLifecycle**.

## Context
Oxymake must track job execution state for three distinct purposes:

1. **Scheduling decisions** — which jobs are ready, which are running, what to
   dispatch next. This must be fast (sub-microsecond lookups) and reflect the
   topological structure of the DAG.
2. **User-facing reporting** — terminal progress bars, dashboard views, metrics.
   These consume state changes as a stream of events.
3. **Crash recovery and audit** — persistent record of what happened, surviving
   process termination. Used for cooperative sessions, post-mortem analysis, and
   history queries.

A single shared data structure cannot serve all three well: scheduling needs
O(1) frontier access, reporting needs temporal event ordering, and persistence
needs durable writes.

## Decision
State flows through three stages connected by a unidirectional event bus:

```
Frontier (in-memory scheduler state)
       │
       ▼ emits Event variants
   EventBus (tokio::broadcast, capacity=1024)
       │
       ├──▶ Reporter(s)    — terminal, dashboard, JSON, metrics
       └──▶ EventSink      — Event → SQLite transitions
                │
                ▼
           Ledger (SQLite WAL — persisted audit store)
```

**Stage 1: Frontier** — In-memory `HashMap<JobId, JobLifecycle>` plus a
`ready_frontier` set. Owns the scheduling loop. Source of truth for what to
dispatch next. (Rust symbol: `Frontier`. M1-followup completed the Rust-level
rename from the legacy `SchedulerState` / `ScheduleStatus` identifiers to
`Frontier` / `JobLifecycle`.)

**Stage 2: EventBus** — `tokio::broadcast` channel. The scheduler emits typed
`Event` variants (JobStarted, JobCompleted, JobFailed, JobSkipped, JobCancelled,
RunCompleted, etc.). Subscribers are independent — a slow reporter cannot block
the scheduler.

**Stage 3: Ledger** — SQLite database with WAL mode. An **EventSink** task
subscribes to the bus and translates events into guarded state transitions
(ADR-010). Source of truth for persistent state, history, and cooperative
session coordination. (Rust symbol: `StateDb` — same source-compat note.)

Information flows strictly downward through the three stages. There is no
backchannel from Ledger to Frontier — the scheduler never reads from SQLite
during execution.

## Naming map (M1 vocabulary pass)

| Old word in this ADR        | New word              | Note |
|-----------------------------|-----------------------|------|
| Layer (pipeline meaning)    | **Stage**             | Three sequential stages on the downward path |
| Layer (containment meaning) | **Stratum** (reserved)| For future transactional/resident layering — not used yet |
| `SchedulerState`            | **Frontier**          | In-memory state; the "ready frontier" is its defining structure |
| `StateDb`                   | **Ledger**            | Append-only audit store |
| `ScheduleStatus`            | **JobLifecycle**      | Enum of per-job lifecycle states |
| db-side `Bridge`            | **EventSink**         | Subscribes to bus, writes to Ledger |
| Bridge (ADR-008 sense)      | `ExecutorBridge`      | Keeps the original name — the two are no longer homonyms |

## Consequences

**Easier:**
- Adding a new reporter or sink is trivial: subscribe to the bus, handle events.
- Frontier performance is decoupled from I/O — SQLite write latency doesn't
  affect dispatch throughput.
- Crash recovery is clean: the Ledger reflects the last successfully persisted
  state; in-memory Frontier state is reconstructed from the DAG on restart.
- Testing each stage independently: the Frontier can be tested without SQLite;
  reporters can be tested with synthetic events.

**Harder:**
- The Ledger may lag behind the Frontier by the event queue depth. Queries
  during execution see slightly stale data (acceptable for dashboards, not for
  scheduling).
- The EventSink must handle `RecvError::Lagged` gracefully when the bus
  overflows (current behavior: log and continue, accepting the gap).
- Debugging state inconsistencies requires correlating three stages. The
  finalization step (reading terminal states from the Ledger after event-bus
  flush) is the reconciliation point.

## Alternatives Considered

**Single shared state (Frontier writes directly to SQLite)**: Simpler
architecture but couples scheduler throughput to disk I/O. A 1ms SQLite write
per job transition becomes a bottleneck at thousands of jobs. Rejected for
performance reasons.

**Event sourcing (events are the primary store, state derived)**: Events stored
in an append-only log, state materialized on read. Attractive for audit trails
but adds complexity for simple queries like "how many jobs are running?" and
makes cooperative session coordination harder (need to replay log). Rejected as
over-engineering for current needs, though the event bus design leaves the door
open. If we ever adopt this reading, the EventSink is best renamed
**Projection**.

**Two stages (drop the event bus, the Ledger subscribes directly)**: Removes
the broadcast channel but forces the scheduler to manage subscriber lifecycles.
Rejected because the bus provides natural fan-out and backpressure isolation.
