# ADR-004: SQLite for State Persistence, Not Dolt

## Status
Accepted

## Metadata

- **Kind:** `decision`
- **Family:** `COOP`
- **Supersedes:** `none`

## Context
OxyMake needs persistent state for execution tracking, caching metadata,
and audit trail. Options include SQLite, Dolt, PostgreSQL, flat files.

## Decision
Use SQLite (via rusqlite) as the single state store, with WAL mode for
concurrent readers and atomic job claiming for cooperative multi-session.

## Consequences
- Zero setup — no server, no port, no daemon (daemon-free principle)
- Atomic transactions for concurrent job completion
- Schema versioning via `PRAGMA user_version` from day 1
- Single file (`.oxymake/state.db`) — easy to backup, delete, inspect
- **Does NOT work on network filesystems** (NFS, Lustre, GPFS) — the
  scheduler must run on a local filesystem node
- For team collaboration, a remote state backend (Dolt, PostgreSQL) can
  be added as a future plugin

## Alternatives Considered
- **Dolt**: powerful for versioning and collaboration but requires a server
- **PostgreSQL**: overkill for single-user, adds deployment complexity
- **Flat files** (Snakemake's `.snakemake/`): no schema versioning, fragile to corruption
- **sled/redb** (embedded Rust DBs): less mature than SQLite, no SQL query language
