# Architecture Decision Records

This directory holds Oxymake's Architecture Decision Records (ADRs). Each
file documents a structural choice — what was decided, why, and what was
considered instead.

## How to write a new ADR

1. Copy `000-template.md` to the next available number :
   `cp 000-template.md NNN-short-slug.md` (3-digit zero-padded).
2. Fill the `## Metadata` block (Kind / Family / Supersedes). See the
   template for field definitions ; the conventions are summarised below.
3. Write Context, Decision, Consequences, Alternatives Considered. If the
   ADR is an `architecture-note` (descriptive rather than choice-based),
   the Alternatives Considered section can be a single line stating why no
   real alternatives were on the table.
4. If the new ADR supersedes (fully or partially) an existing one, follow
   the **Sunset rule** : add `Supersedes: ADR-XXX` here AND update the old
   ADR's `## Status` to `Superseded by ADR-NNN` (or `Partially superseded
   by ADR-NNN`) in the same PR.

## Field conventions

### Kind — what role does this ADR play?

| Kind                 | Use when                                                                                       |
| -------------------- | ---------------------------------------------------------------------------------------------- |
| `decision`           | a real A-vs-B choice with stated alternatives that were rejected                               |
| `architecture-note`  | describes how something works ; documents structure rather than choosing between options       |
| `proposal`           | being designed ; status will move to `accepted` or `rejected` once validated                   |

### Family — soft topic label (not a filename rename)

| Family | Concern                          | Example ADRs              |
| ------ | -------------------------------- | ------------------------- |
| `CAS`  | Content & Addressing             | ADR-001, ADR-006, ADR-014 |
| `STM`  | State Machines                   | ADR-010, ADR-011, ADR-013 |
| `COOP` | Cooperation & Coordination       | ADR-004, ADR-005, ADR-012 |
| `EXT`  | Extension Points                 | ADR-002, ADR-003, ADR-007, ADR-008, ADR-009 |

The family is a **soft label in the body**, not a rename of the file.
Numbering remains a flat 3-digit sequence. The family helps a reader walk
the corpus by concern without changing the URL of any ADR.

## Current index

| #   | Title                                            | Kind                | Family | Status                                 |
| --- | ------------------------------------------------ | ------------------- | ------ | -------------------------------------- |
| 001 | Content-Addressable Cache as Source of Truth     | decision            | CAS    | Partially superseded by ADR-006        |
| 002 | TOML Workflow Format, Not a Custom DSL           | decision            | EXT    | Accepted                               |
| 003 | Subprocess + Arrow IPC, Not PyO3                 | decision            | EXT    | Accepted                               |
| 004 | SQLite for State Persistence, Not Dolt           | decision            | COOP   | Accepted                               |
| 005 | Daemon-Free Cooperative Execution Model          | decision            | COOP   | Accepted                               |
| 006 | Pluggable Cache Validation Strategies            | decision            | CAS    | Accepted (supersedes ADR-001 default)  |
| 007 | Enriched `ox run` Output                         | decision            | EXT    | Accepted (Phase 1 implemented)         |
| 008 | ExecutorBridge — Bidirectional Adapter           | proposal            | EXT    | Proposed                               |
| 009 | Contextual Output Reporter                       | proposal            | EXT    | Proposed                               |
| 010 | Guarded State Transitions                        | decision            | STM    | Accepted                               |
| 011 | Three-Stage State Pipeline †                     | architecture-note   | STM    | Accepted                               |
| 012 | Cooperative Multi-Session via SQLite Claims      | decision            | COOP   | Accepted                               |
| 013 | Distinct JobCancelled vs JobSkipped Semantics    | architecture-note   | STM    | Accepted                               |
| 014 | Cache-State Separation with `cached` Flag        | decision            | CAS    | Accepted                               |
| 015 | Named Invariants and TLA+ Scope                  | architecture-note   | STM    | Accepted                               |
| 016 | Metrics as Single Source of Truth                | decision            | EXT    | Accepted                               |
| 017 | Artifact Residence Topology (Topology B)         | decision            | EXT    | Accepted                               |

† ADR-011's filename retains the historical `three-layer` slug (renaming
breaks the URL, per the flat 3-digit no-rename rule above); its title was
realigned to "Three-Stage State Pipeline" in the 2026-05-27 vocabulary pass.

## The Sunset rule

When an ADR is partially or fully superseded by another :

1. The new ADR carries `Supersedes: ADR-XXX` in its `## Metadata` block.
2. The old ADR's `## Status` line moves to `Superseded by ADR-YYY` (full)
   or `Partially superseded by ADR-YYY` with a one-line note pointing at
   the section that was abrogated (partial).

Both updates land in the same PR. No partial-abrogation zombies — a reader
walking the corpus must be able to follow the link in both directions
without guessing.

## ADR meta-linter

`scripts/adr-lint.py` runs an **advisory radar** over this directory :

- **cross-references** — pairs of ADRs that cite the same primitive (e.g.
  both touch `StateDb`) without acknowledging each other ; hints at a
  missing `Supersedes` link or a missing prose cross-link ;
- **lexical opposition** — one ADR says `MUST NOT` X, another says `MAY` X ;
  surfaces wording that may contradict ;
- **orphans** — ADRs not cited by any other ADR or doc.

The linter is intentionally a **radar, not a proof**. Per Godel,
semantic-contradiction detection is undecidable in general ; the script
flags suspicious patterns and leaves the call to a human reviewer. It
exits 0 by default so it can run as a CI hint without blocking merges ;
pass `--strict` for local discipline.

```bash
# Walk docs/adr/ and print advisory warnings :
scripts/adr-lint.py

# Machine-readable output (for CI step summaries) :
scripts/adr-lint.py --json

# Treat any warning as failure (local discipline only — CI stays advisory) :
scripts/adr-lint.py --strict
```

CI wires the linter in `.github/workflows/ci.yml` under the `adr-lint`
job, which never fails the build — its purpose is to surface signal on
PR pages, not gate merges.

### STATE.md — the machine projection

`scripts/adr-lint.py --emit-state docs/adr/STATE.md` regenerates
[`STATE.md`](STATE.md), a deterministic table of every ADR (number, title,
status, citers, primitives). It is **machine-generated — do not hand-edit** ;
rerun the command after any ADR edit. Unlike the advisory linter, the
`adr-state` CI job **does** gate the build : it regenerates STATE.md and fails
if the committed copy drifted, so disk / this index / STATE.md cannot fall out
of sync.

## Why not heavier ADR machinery?

Oxymake intentionally avoids the 140-class ADR discipline that cosmon
needs to govern its LLM adversary. Shannon's read of the corpus : the
natural ceiling here is ~30–40 ADRs, not 140. The `Kind` + `Family`
fields scale the existing 17 ADRs to that ceiling without bureaucracy.
The Sunset rule + linter close Godel's frontier gap (partial-abrogation
zombies, lexical contradictions) without forcing every ADR through a
formal model-check.
