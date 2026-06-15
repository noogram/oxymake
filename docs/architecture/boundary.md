# Boundary — Substrate Axioms (Architecture Note)

> **This is NOT a TLA+ spec. It is the frontier of what oxymake does not
> attempt to prove formally. The 7 axioms below are statements about
> substrate behaviour we accept on authority of the OS, the filesystem,
> and SQLite.**

- **Last reviewed:** 2026-05-27
- **Source ADR:** [ADR-015 — Named Invariants and TLA+ Scope](../adr/015-named-invariants.md)
- **Status:** Demoted from `spec/tla/Boundary.tla` per the §M7 + (d)
  boundary-scope decision. Demotion is reversible: any axiom that produces ≥1 real
  bug via M3/M6 may be promoted back to a dedicated `.tla` module
  (e.g. `EvictionRace.tla`, `CrashRecovery.tla`) at the next release
  review (the 6-month attestation window was voided 2026-06-10 —
  operator decision, premortem PM#5, no temporal gates).

## Why a markdown note and not a `.tla` file

The TLA+ form (`Boundary.tla`) carried seven `ASSUME C \in BOOLEAN`
declarations and no transitions. TLC does not check `ASSUME`s about
unspecified constants — it only enforces them as premises of the
interior modules that import the module via `EXTENDS`. Since none of
the interior modules in oxymake's ship-now scope (`CacheConsistency`,
`CooperativeClaim`, `CancelPropagation`) currently `EXTENDS Boundary`,
the file produced zero TLC obligations and earned zero ledger evidence.
The recommendation panel (3/3 substantive,
convergence C1) concluded that a markdown note preserves the
metamathematical content (named axioms, ADR attribution, scope of
authority) while removing the false signal that TLC is checking
something it is not.

The 7 axioms are not weakened by this move. They remain the explicit
frontier of the formal model — the contracts the substrate (OS,
filesystem, SQLite, executor process) must honour for the interior
proofs of `CacheConsistency`, `CooperativeClaim`, and
`CancelPropagation` to be relative-consistency proofs at all.

## The 7 substrate axioms

Each axiom is a property of a substrate component that interior modules
assume but cannot prove. The numbering and naming match the constants
that lived in `Boundary.tla`.

| # | Axiom                            | Substrate property                                                  | Justifying ADR |
|---|----------------------------------|---------------------------------------------------------------------|----------------|
| 1 | `AtomicRename`                   | `rename(2)` is atomic on the target filesystem                      | ADR-014        |
| 2 | `FsyncDurable`                   | `fsync(2)` effectively persists writes through power loss           | ADR-014        |
| 3 | `NoOOMCascade`                   | The OS does not kill a worker mid-IPC                               | ADR-008        |
| 4 | `ExecutorHonest`                 | Executor bridges return truthful status to the orchestrator         | ADR-008        |
| 5 | `UserCodeMatchesRule`            | The subprocess implements the rule's declared contract              | ADR-008        |
| 6 | `StorageDeleteAtomic`            | `unlink(2)` is single-step (no torn delete observable mid-call)     | ADR-014        |
| 7 | `ExecutorFailureClassification`  | Status ∈ {Done, Failed, Cancelled, Lost}; per-executor map monotone | ADR-008        |

### Deferred (reserved for future modules)

Two further axioms were scoped in `Boundary.tla` for activation when
`CrashRecovery.tla` lands:

- `StateDbAtomicCommit` — SQLite COMMIT is atomic across the state-db
  schema (ADR-014).
- `WAL_DurableOnFsync` — the WAL is durable once `fsync(2)` returns
  (ADR-014).

They remain reserved here and will be promoted (along with their owning
interior module) if and when the M3/M6 attestation window surfaces
evidence that justifies a dedicated `.tla` module.

## Promotion criterion (dissent preservation)

The janis dissent — that demoting Boundary
risks losing the discipline of named, grep-able axioms — is preserved
by the following rule, inscribed in M0 L3 (thesis attestation table):

> If two or more of the seven axioms above have each produced ≥1 real
> bug via the interior modules `CooperativeClaim` (M3) or
> `CancelPropagation` (M6), then at the next `v*` release review the
> implicated axioms are promoted back into dedicated `.tla` modules
> (`EvictionRace.tla` and/or `CrashRecovery.tla` as appropriate).
> **Promotion follows demonstrated value, not anticipated value.**

*(The original rule was windowed — "within six months of this demotion,
i.e. by 2026-11-27". The calendar gate was voided on 2026-06-10 by
operator decision — premortem PM#5, no temporal gates; the evidence
threshold is unchanged and is adjudicated at release reviews.)*

If a release review finds fewer than two axioms having produced
demonstrable bugs, this markdown note is sufficient; the axioms
remain named here and the interior modules continue to cite them by
name.

## Usage from interior modules

Interior `.tla` files refer to these axioms by name, in their
"Out of model" preamble, e.g.:

```tla
\* This spec defends invariant <NAME> for oxymake `ox run`.
\* Out of model (see docs/architecture/boundary.md):
\*   AtomicRename, FsyncDurable, NoOOMCascade,
\*   ExecutorHonest, UserCodeMatchesRule,
\*   StorageDeleteAtomic, ExecutorFailureClassification.
```

The reference is now documentary, not syntactic: no `EXTENDS Boundary`
is required (and indeed none of the current modules used it).

## Provenance

- ADR-015 — Named Invariants and TLA+ Scope (the ship-now scope no
  longer includes `Boundary.tla`; it includes this note instead).
- ADR-008 — Executor model (justifies axioms 3, 4, 5, 7).
- ADR-014 — Cache-state separation / storage (justifies axioms 1, 2, 6,
  and the deferred axioms).
- Recommendation panel — 3/3 substantive,
  convergence C1; §M7 + (d) authorises the demotion and inscribes the
  promotion criterion above.
- Newcombe, Rath, Zhang, Munteanu, Brooker, Deardeuff. *How Amazon Web
  Services Uses Formal Methods*. Communications of the ACM 58(4),
  April 2015, pp. 66–73. DOI 10.1145/2699417 — the empirical reference
  that justifies oxymake's TLA+ discipline as a whole, including the
  distinction between interior modules (TLC-checked) and the substrate
  frontier (axiomatised in prose).
