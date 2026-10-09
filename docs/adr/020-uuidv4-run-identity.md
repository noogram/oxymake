# ADR-020: UUIDv4 Run Identity and Exact Selection

## Status
Accepted

## Metadata

- **Date:** 2026-10-09
- **Kind:** `decision`
- **Family:** `COOP`
- **Supersedes:** `none`

## Context

PID-derived run IDs can repeat across processes and hosts. They also made an
explicit status lookup unsafe: a failed selected-driver lookup could broaden
into an unrelated cluster-wide Ray listing. Existing databases and artifacts
already contain opaque legacy strings that must retain their identity.

## Decision

Each executing `ox run` invocation mints exactly one
`run-<lowercase-hyphenated-UUIDv4>` value and propagates it unchanged through
database references, events, paths, staging, and executor metadata. A run-row
collision fails registration before jobs or history attach to the ID.

Historical IDs and artifacts are unchanged: there is no migration, alias,
case folding, prefix inference, or remote rewrite. `status --run` and
`history --run-id` first resolve an exact, case-sensitive local key. Proper
prefixes always fail. A known run with no history succeeds. A missing run and
an unavailable bound remote run are distinct errors, and explicit selection
never broadens to unrelated remote jobs. `cancel` gains no run selector.

Run IDs are opaque and carry no chronological guarantee. Recorded timestamps
remain the ordering source. UUIDv4 supplies practical collision resistance,
not a claim that collision is mathematically impossible. Event schema version
1 and the string meaning of `run_started.run_id` remain unchanged.

## Consequences

Independent invocations no longer depend on a process-local PID namespace, and
all carriers can be compared byte-for-byte. Users must copy full IDs. Existing
short or non-UUID IDs continue to work only as exact legacy keys. Filesystem
sorting by ID is not a run chronology.

## Alternatives Considered

- UUIDv7 and ULID add sortable encodings without a demonstrated requirement;
  timestamps already order recorded runs.
- Host, time, and PID combinations require host-identity, clock, privacy, and
  reuse policies while still needing another uniqueness mechanism.
- Prefix and case-normalized lookup make identity ambiguous and can enlarge a
  remote query beyond the selected local run.
