# ADR-000: Template

## Status
Proposed | Accepted | Deprecated | Superseded by ADR-XXX

## Metadata

- **Date:** YYYY-MM-DD
- **Kind:** `decision` | `architecture-note` | `proposal`
- **Family:** `CAS` | `STM` | `COOP` | `EXT`
- **Supersedes:** `ADR-XXX` (comma-separated if multiple, `none` otherwise)

### Field conventions

**Kind** (what role does this ADR play?)
- `decision` — a real A-vs-B choice with stated alternatives that were
  rejected. The "Alternatives Considered" section is substantive.
- `architecture-note` — describes how something works ; documents structure
  rather than choosing between options. No real alternatives were considered.
- `proposal` — being designed ; status will move to `accepted` or `rejected`
  once the choice is made and validated.

**Family** (soft label — groups ADRs by structural concern ; not a rename)
- `CAS` — Content & Addressing : cache keys, content hashes, mtime/hash
  validation, cache-state separation.
- `STM` — State Machines : status transitions, state architecture, cancel vs
  skip semantics, scheduler/state-db layering.
- `COOP` — Cooperation & Coordination : daemon-free model, multi-session
  claiming, SQLite-based concurrency.
- `EXT` — Extension Points : storage backends (TOML vs DSL), execution
  backends (subprocess/Arrow, executor bridges), output adapters (reporters).

3-digit numbering is preserved. The family prefix lives in the body, not in
the filename.

## Context
What is the issue that we're seeing that is motivating this decision?

## Decision
What is the change that we're proposing and/or doing?

## Consequences
What becomes easier or more difficult to do because of this change?

## Alternatives Considered
What other options were evaluated and why were they rejected?

## Sunset

When an ADR is partially or fully superseded by another, **the same PR that
introduces the new ADR must update both sides of the link** :

1. The new ADR carries `Supersedes: ADR-XXX` in its metadata block.
2. The old ADR's `Status:` line moves to `Superseded by ADR-YYY` (or
   `Partially superseded by ADR-YYY` with a one-line note pointing to the
   specific section that was abrogated).

No partial-abrogation zombies. A reader walking the corpus must be able to
follow the link in both directions without guessing. (Godel's frontier rule
— closes the indecidable-statement gap that arises when a later ADR silently
narrows an earlier one.)
