# ADR State

*Projection of `docs/adr/` emitted by `scripts/adr-lint.py --emit-state`. Do not edit by hand — regenerate.*

Status is the first non-empty line under each ADR's `## Status` section, verbatim. *Cited by* counts incoming references from other ADRs (external doc citations are not listed but are tallied in the *external citations* column).

| ADR | Title | Status | Cited by | External | Primitives |
|---|---|---|---|---|---|
| ADR-001 | Content-Addressable Cache as Source of Truth | Accepted (default validation strategy changed to mtime by ADR-006, then to mtime+hash by the 2026-06-10 amendment of ADR-006) | ADR-006, ADR-015, ADR-018 | — | blake3, mtime |
| ADR-002 | TOML Workflow Format, Not a Custom DSL | Accepted | ADR-018 | — | — |
| ADR-003 | Subprocess + Arrow IPC for Language Interop, Not PyO3 | Accepted | ADR-008 | — | — |
| ADR-004 | SQLite for State Persistence, Not Dolt | Accepted | ADR-016 | — | session, sqlite, state_db |
| ADR-005 | Daemon-Free Cooperative Execution Model | Accepted | ADR-012, ADR-018 | — | session, sqlite |
| ADR-006 | Pluggable Cache Validation Strategies | Accepted — **amended 2026-06-10: default changed from `mtime` to `mtime+hash`** | ADR-001, ADR-018 | — | blake3, cache_validation, mtime |
| ADR-007 | Enriched `ox run` Output — Inspired by Snakemake Job Reporting | Accepted (Phase 1 implemented: RunReason, verbose levels, timings) | — | — | — |
| ADR-008 | ExecutorBridge — Bidirectional Adapter Between OxyMake and Remote Executors | Proposed | ADR-009, ADR-011 | — | cache_validation, executor, ray, slurm, state_db |
| ADR-009 | Contextual Output Reporter — Adapt Display to Executor Profile | Proposed | — | — | event_bus, executor, ray, slurm, state_db |
| ADR-010 | Guarded State Transitions with WHERE Preconditions | Accepted | ADR-011, ADR-015 | — | cancel, session, skip_job, sqlite, state_db |
| ADR-011 | Three-Stage State Pipeline | Accepted | ADR-015 | — | cancel, event_bus, executor, session, skip_job, sqlite, state_db |
| ADR-012 | Cooperative Multi-Session via SQLite Atomic Claims | Accepted | ADR-015 | — | session, sqlite |
| ADR-013 | Distinct JobCancelled vs JobSkipped Semantics | Accepted | ADR-015 | — | cancel, skip_job |
| ADR-014 | Cache-State Separation with Dedicated Cached Flag | Accepted | ADR-013, ADR-015 | — | skip_job, state_db |
| ADR-015 | Named Invariants and TLA+ Scope | Accepted (revised from the D2 scope decision by a follow-up design panel; | — | — | cancel, session, sqlite, state_db |
| ADR-016 | Metrics as Single Source of Truth | Accepted (drafted from the metrics deliberation §3 T2 + §5 / Task 1 sibling ADR ; | ADR-015 | — | sqlite |
| ADR-017 | Artifact Residence Topology (Topology B) | Accepted (release-readiness review). | — | — | — |
| ADR-018 | Differentiation After the CWL Review — Envelope, Not Primitive | Proposed | — | — | cache_validation, mtime |

*18 ADRs scanned. Regenerate after every ADR edit.*
