# `spec/tla/` — Oxymake TLA+ Specifications and Falsifiability Ledger

This directory holds two things, tightly coupled:

1. **Formal specifications** of oxymake's invariants — relative-consistency
   proofs where interior modules assume the substrate (OS, FS, executor,
   SQLite) behaves as named axioms claim, then prove internal safety
   properties on top.
2. **The falsifiability ledger** that records the evidence each spec
   earns over time. A `.tla` file is *not* a documentation artefact. It
   is a falsifiable claim about the safety of an interleaving, judged
   by what TLC produces and by the design changes its writing forces.
   Specs that do not earn evidence are deleted.

The specifications are provisioned by **M1–M5** (ADR-015 + the four
interior modules + the boundary). The ledger and sunset rule are
provisioned by **M6**.

## Layout

- The substrate frontier lives **outside** this directory at
  [`docs/architecture/boundary.md`](../../docs/architecture/boundary.md)
  — an architecture note, not a `.tla` spec. It names the 7 substrate
  axioms (OS, FS, executor, SQLite) that interior modules rely on but
  cannot prove. Demoted from `spec/tla/Boundary.tla` on 2026-05-27 per
  the §M7 + (d) boundary-scope decision; promotion criterion preserved
  (architecture note §"Promotion criterion").
- `CacheConsistency.tla` *(M2, not yet committed)* — defends OX-1, OX-6
  over cache-state separation.
- `CooperativeClaim.tla` *(M3, not yet committed)* — lease invariants
  for multi-session claim.
- `CancelPropagation.tla` *(M4, not yet committed)* — defends
  `CancelledNeverCached`, `JobFailedImpliesNoIntent`,
  `EvictPrecedesUnregister`.
- `TRACES.md`, `DESIGN-CHANGES.md`, `REVIEWS.md` — the M6 ledger
  (see *Ledgers* below).

## Frontier axioms ([`docs/architecture/boundary.md`](../../docs/architecture/boundary.md))

| # | Constant                       | Substrate property           | ADR     |
|---|--------------------------------|------------------------------|---------|
| 1 | `AtomicRename`                 | `rename(2)` atomic           | ADR-014 |
| 2 | `FsyncDurable`                 | `fsync(2)` persists          | ADR-014 |
| 3 | `NoOOMCascade`                 | OS spares mid-IPC workers    | ADR-008 |
| 4 | `ExecutorHonest`               | bridges report truthfully    | ADR-008 |
| 5 | `UserCodeMatchesRule`          | subprocess honors rule       | ADR-008 |
| 6 | `StorageDeleteAtomic`          | `unlink(2)` single-step      | ADR-014 |
| 7 | `ExecutorFailureClassification`| status enum + monotone trans | ADR-008 |

Deferred to `CrashRecovery.tla` (not yet nucleated):
`StateDbAtomicCommit`, `WAL_DurableOnFsync`.

### Usage from interior modules

Interior `.tla` files reference the substrate axioms by name in their
"Out of model" preamble, e.g.:

```tla
---- MODULE CacheConsistency ----
EXTENDS Naturals, FiniteSets, Sequences, TLC
\* Interior invariants are relative-consistency proofs, conditional
\* on the substrate axioms named at docs/architecture/boundary.md.
\* Out of model (see docs/architecture/boundary.md):
\*   AtomicRename, FsyncDurable, ExecutorHonest, ...
...
====
```

The reference to the boundary is now documentary rather than syntactic
— no `EXTENDS Boundary` is required (and none of the current modules
used it). The frontier still marks where the formal model ends and the
substrate contract begins; every interior proof is conditional on those
axioms holding in the deployed system.

## Scope of authority

| Spec | What it asserts | Source |
|---|---|---|
| `docs/architecture/boundary.md` (architecture note, ex-M5) | The 7 substrate axioms above | ADR-015, ADR-014, ADR-008, boundary-scope decision |
| `CacheConsistency.tla` (M2) | Invariants OX-1, OX-6 over cache-state separation | ADR-014, ADR-015 |
| `CooperativeClaim.tla` (M3) | Lease invariants for multi-session claim | ADR-012, ADR-015 |
| `CancelPropagation.tla` (M4) | `CancelledNeverCached`, `JobFailedImpliesNoIntent`, `EvictPrecedesUnregister` | ADR-013, ADR-015 |

## The Sunset Rule (verbatim)

> Between one release review and the next, TLC must surface
> EITHER (a) ≥1 execution trace violating a named safety invariant and
> not exercised by any pre-existing integration test, OR (b) ≥1 design
> change in oxymake whose commit message references the spec as
> motivation. A spec with neither receives a `conditional` verdict at
> the review; three consecutive `conditional` verdicts and the spec is
> deleted in the next PR citing this rule. Corroboration is always
> provisional — each review window is a fresh test.

A spec's *first commit* is the commit hash that introduced the `.tla`
file (recorded in `TRACES.md` / `DESIGN-CHANGES.md` entries that cite
it). A spec's first review is the first release review after that
commit.

> **Operator decision 2026-06-10 (premortem PM#5):** all temporal gates
> were removed from this repository. The original sunset rule ran on
> six-month calendar windows with engaged review *dates*; those dates
> are void. Reviews are now **release-gated**: a spec review is a
> mandatory item of every `v*` release checklist
> (`docs/RELEASE-CHECKLIST.md`), so the discipline fires when the
> project *acts*, never because time passed. A project that never
> releases never reviews its specs — that silence is governed by
> `docs/HIBERNATION.md`, not by a clock.

## Ledgers

| File | Records | Trigger |
|---|---|---|
| [`TRACES.md`](TRACES.md) | TLC counterexamples (an execution trace violating a named invariant) and the Rust fix that resolved them | Operator runs TLC; counterexample emerges |
| [`DESIGN-CHANGES.md`](DESIGN-CHANGES.md) | Design changes motivated by the act of writing the spec (the question the spec forced; the Rust commit that answered it) | Author of a spec realises the Rust code must change |
| [`REVIEWS.md`](REVIEWS.md) | Operator verdicts at each release review: `keep` / `conditional` / `delete` per spec, with rationale | Release event (see below) |

A spec earns its keep by producing **at least one** TRACES.md *or*
DESIGN-CHANGES.md entry within each review window. The REVIEWS.md
entry is the operator's adjudication of that earning.

## Release-Gated Reviews

| Event | Review | Action if condition fails |
|---|---|---|
| **First TLC exercise** of `CacheConsistency.tla` | TLC-checked at ≥3 workers, ≥4 rules, ≥1 eviction round; trace log written. | Missing trace log ⇒ note in `REVIEWS.md` as `conditional`. |
| **Every `v*` release** | Sunset review. Each spec must show ≥1 TRACES.md entry OR ≥1 DESIGN-CHANGES.md entry since the previous review. | `conditional` verdict; three consecutive `conditional` ⇒ delete in next PR citing this rule. |
| **Every `v*` release** | Long-horizon tally. Count production bugs root-cause-matched to named invariants since the spec suite landed. | Persistent zero across reviews ⇒ ADR-meta reviewing the entirety of `spec/tla/`. |

These events are load-bearing: the discipline is **answerable to
them**. The review is a mandatory item of `docs/RELEASE-CHECKLIST.md`;
the verdicts land in `REVIEWS.md`. (The original calendar of engaged
dates was voided by the operator decision of 2026-06-10 — premortem
PM#5, no temporal gates.)

## How to run the suite

```bash
spec/tla/run-tlc.sh          # green suite — every shipped config must pass
spec/tla/run-tlc.sh --red    # falsifiability witnesses — must FAIL
spec/tla/run-tlc.sh --all    # both
```

The script pins TLC by version + sha256 (downloaded to `.tlc-cache/`,
gitignored) and archives each run's full output under `spec/tla/runs/`
(committed). Every number quoted about the suite — state counts, search
depth — must be reproducible from these artifacts; the committed
`runs/*.out` files are the reference outputs (premortem finding H19).

Two **red configurations** are committed alongside the green ones:

| Config | Models | Refutes |
|--------|--------|---------|
| `CooperativeClaimUnguarded.cfg` | terminal UPDATE without the `AND session_id=?` arm (pre-H16 code) | `DoneByClaimHolder` |
| `CacheConsistencyNondetKey.cfg` | an undeclared input leaking into the cache key | `CacheKeyDeterminism` |

A red config is the proof that its green twin checks something real:
an invariant that no configuration can violate is decoration, not
verification (premortem finding H18).

## How to add a spec

1. Write `<Name>.tla` (≤120 L target, defending named invariants from
   ADR-015 §Invariants). Reference substrate axioms by name in the
   "Out of model" preamble — see `docs/architecture/boundary.md`.
2. Commit. Record the *first-commit hash* — TRACES.md and
   DESIGN-CHANGES.md entries will cite it.
3. Configure TLC (typically a `<Name>.cfg` next to the spec).
4. The review window opens. By the next release review you must have
   ≥1 entry in TRACES.md or DESIGN-CHANGES.md citing this spec,
   otherwise the spec takes a `conditional` verdict (three consecutive
   ⇒ deletion).

## How to delete a spec

A spec is deleted by a PR whose commit message:

- Cites this README's sunset rule verbatim;
- Names the spec being removed;
- Names the release review that produced the `delete` verdict;
- Removes the `.tla` and `.cfg` files in the same commit;
- Adds the deletion-decision text to `REVIEWS.md` (the entry is kept,
  not erased — the *file* lingers as evidence of the discipline).

The substrate frontier (`docs/architecture/boundary.md`) is special:
it is an architecture note, not a `.tla` spec, and it earns its keep
through the substrate axioms it names rather than through invariant
violations. Its sunset criterion is whether ≥1 interior module still
depends on its axioms. The dedicated promotion criterion (two axioms
× ≥1 real bug via M3/M6 ⇒ promotion back to `.tla` modules, adjudicated
at release reviews) is inscribed in the note itself.

## Provenance

- ADR-015 (named invariants — CSTAFP reformulation)
- ADR-008 (executor model)
- ADR-014 (storage / object store)
- Design panel §5 D-1 (Boundary scope expansion S1, S4), §5 D-4
  (Karpathy pilot + Popper sunset)
- §M7 + (d) decision — demotion of `Boundary.tla` to
  `docs/architecture/boundary.md` (recommendation panel 3/3
  substantive, convergence C1)
- Newcombe et al., "How Amazon Web Services Uses Formal Methods",
  CACM 58(4):66–73, 2015

## Cross-references

- **M6** — the ledger mechanism. Capture
  document: `docs/design/tla-falsifiability-mechanism.md`.
- **ADR-015** — `docs/adr/015-named-invariants.md` (named invariants,
  CSTAFP, scope).
- **Chronicle** — `docs/lore/CHRONICLES.md` (entries 2026-05-27).
