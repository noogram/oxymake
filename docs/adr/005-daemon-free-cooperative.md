# ADR-005: Daemon-Free Cooperative Execution Model

## Status
Accepted

## Metadata

- **Kind:** `decision`
- **Family:** `COOP`
- **Supersedes:** `none`

## Context
Workflow tools typically use either exclusive locks (one process at a time)
or a central daemon/server for coordination. Both have drawbacks:
exclusive locks prevent concurrent work on disjoint subgraphs; daemons
add deployment complexity.

## Decision
OxyMake uses a daemon-free cooperative model where multiple `ox run`
processes coordinate via atomic SQLite operations:

- Each `ox run` creates a session in SQLite
- Jobs are claimed atomically (`UPDATE ... WHERE status = 'pending'`)
- Running jobs from other sessions are attached to (wait, don't re-launch)
- Stale sessions detected via heartbeat timeout with two-phase reclaim

`ox run` semantics: "ensure these outputs exist" (convergent/idempotent),
not "launch these jobs" (imperative).

## Consequences
- No daemon to deploy, manage, or restart
- Multiple `ox run` processes work on disjoint subgraphs without conflict
- Idempotent by construction: same command twice = no extra work
- Requires SQLite on local filesystem (not NFS)
- Heartbeat-based stale detection has a 2-minute window where a slow
  session could be incorrectly declared dead (mitigated by two-phase reclaim)

## Alternatives Considered
- **Exclusive lock**: simpler but prevents concurrent work on independent subgraphs
- **Central daemon**: more capable but adds deployment complexity, violates daemon-free principle
- **Redis/etcd coordination**: robust but adds infrastructure dependency
