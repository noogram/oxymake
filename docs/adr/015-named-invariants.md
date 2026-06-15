# ADR-015: Named Invariants and TLA+ Scope

## Status
Accepted (revised from the D2 scope decision by a follow-up design panel;
amended to add the OX-7 draft).

## Preamble — Contingent Disposition

This ADR adopts the **contingent** position decided by pre-mortem #3 (D3,
2026-05-29): the formal-methods discipline is held as load-bearing *only
while an exogenous referee keeps confirming its value*. The disposition is
not the operator's to waive; it is bound to a CI signal:

> Per pre-mortem #3 (2026-05-29), the formal-methods discipline of this
> project is bound to the drift-tripwire CI
> (`.github/workflows/drift-tripwire.yml`) as an exogenous referee. If the
> CI shows 3+ consecutive red builds, the
> discipline auto-demotes to *exploratory, not load-bearing* — the named
> invariants survive as documentation but no longer claim formal-methods
> rigour. This contingency is binding and operator-irreversible without a
> public ADR amendment with a second-signatory or commit-trail
> justification.

## Context

Oxymake's `ox run` orchestrates jobs across components that are individually
deterministic — a worker, a scheduler, an evictor, a cancel path, a state
database — yet whose composition is concurrent. A prior framing motivated
formal specification by appealing to a contrast between non-deterministic
LLM outputs and deterministic byte-level execution. The panel of
a design panel (knuth, gödel, karpathy, popper; 4/4 unanimous) found
that framing **non-falsifiable** and **load-bearing in the wrong place**:
it conflated the cardinality of the input space with the cardinality of
the interleaving space, and it could not be refuted by any future
observation.

The correct axis is structural, not adversarial. `ox run` exhibits
**CSTAFP** — *Concurrent State Transitions Across independently-Failing
Peers, where safety is a cross-peer relation*. On at least three surfaces
the state of the system is a *relation* between the states of peers that
can each fail independently. No sequential test, even at maximal coverage,
exercises this space: the cardinality of reachable interleavings is
strictly greater than the cardinality of tested traces. Measured today —
the canonical Rust SLOC and test counts (`\metricSLOC`, `\metricTests` in
`docs/paper/metrics.tex`, regenerated from `metrics/metrics.json` per
ADR-016), against 0 `loom`/`shuttle`/`turmoil` harnesses —
the ratio of tested traces to reachable interleavings, even after quotient
by confluence classes, is ≤ 10⁻¹². A green CI for two months corroborates
an infinitesimal fraction of the state space; absence of refutation is
evidence of inadequate testing of the kernel claim, not corroboration of
it.

The empirical reference is Newcombe, Rath, Zhang, Munteanu, Brooker and
Deardeuff, *How Amazon Web Services Uses Formal Methods*, Communications
of the ACM 58(4), April 2015, pp. 66–73 (DOI 10.1145/2699417). The paper
reports seven bugs (one with a 35-step trace) found by TLC in ten AWS
systems — bugs that had been missed by testing, code review, static
analysis, stress testing and fault injection. Those systems were
orchestrated on deterministic components; the hazard class was
concurrentielle, pas adversariale. The structural similarity between
those systems and `ox run` is what justifies the technique here.

A concrete oxymake analogue of the AWS 35-step trace exists by
construction. With four actors (`worker1`, `worker2`, `evictor`,
`cli_cancel`) interleaved across 23 demonstrated steps plus 12 of
cascade, an `IoError` from a read-after-unlink can mask the intent of
cancel — the worker reports `JobFailed(IoError)` instead of
`JobCancelled`. This single trace violates three named invariants
simultaneously: `CancelledNeverCached`, `JobFailedImpliesNoIntent`, and
`EvictPrecedesUnregister`. None of those invariants is expressible inside
the smallest plausible scope (`CacheConsistency.tla` alone). This is a
constructive proof that a single-spec scope is insufficient.

## Decision

Oxymake adopts a discipline of **named invariants** carried by a small
suite of TLA+ specifications. Each spec defends one or more named
invariants for a single CSTAFP surface; the suite is bounded by an
explicit **substrate boundary** — seven named axioms about the
substrate (SQLite, filesystem, kernel, executor process) — recorded as
an architecture note rather than a `.tla` module.

### Ship-now scope — three TLA+ modules + boundary architecture note

| Module | LOC budget | Invariants | CSTAFP class |
|---|---|---|---|
| `spec/tla/CacheConsistency.tla` | 100–120 | OX-1, OX-6 (stationary cache safety) | CSTAFP-lite |
| `spec/tla/CooperativeClaim.tla` | 120–140 | INV-2 (multi-session reclaim / claim atomicity, ADR-012) | Full CSTAFP |
| `spec/tla/CancelPropagation.tla` | 80–120 | INV-3 (`CancelledNeverCached ∧ JobFailedImpliesNoIntent ∧ EvictPrecedesUnregister`, ADR-013) | Full CSTAFP |
| `spec/tla/Recovery.tla` (draft) | 60–100 | OX-7 (re-derivability from disk) | runtime — constructor `Frontier::resume` has landed; spec pending |

The substrate frontier is captured separately in
[`docs/architecture/boundary.md`](../architecture/boundary.md) — a
markdown architecture note (not a `.tla` spec) that names seven axioms:
the five inherited from the D4 scope decision plus `StorageDeleteAtomic`
and `ExecutorFailureClassification`. Two further axioms
(`StateDbAtomicCommit`, `WAL_DurableOnFsync`) are reserved there for
when `CrashRecovery.tla` is activated.

The demotion of the previous `spec/tla/Boundary.tla` to a markdown note
was decided in §M7 + (d) (recommendation panel 3/3
substantive, convergence C1). The 7 axioms are preserved; only their
status changes. The dissent (janis) is preserved by an attestation rule
inscribed in the architecture note: if two axioms each produce ≥1 real
bug via M3/M6 within six months (by 2026-11-27), they are promoted to
dedicated `.tla` modules (`EvictionRace.tla` / `CrashRecovery.tla`) at
the next sunset review. Promotion follows demonstrated value.

Total TLA+ budget: 468 L of `.tla` against the canonical Rust SLOC count
(`\metricSLOC` in `docs/paper/metrics.tex`) ≈ 0.79 % — under the ~1 % ratio
AWS reports. (SLOC is the canonical metrics.json
value per ADR-016; the `.tla` total is the three committed modules:
CacheConsistency 139 L + CooperativeClaim 161 L + CancelPropagation 168 L.)

### Bd-tracked scope — activate on trigger

| Module | Trigger to activate | CSTAFP class |
|---|---|---|
| `spec/tla/EvictionRace.tla` | Stage 2 / memory-pressure code lands | CSTAFP-lite |
| `spec/tla/CrashRecovery.tla` | ADR-014 implementation finalised | Full CSTAFP |

These modules are not nucleated by this ADR. They are tracked as beads
to ensure the spec follows the code rather than preceding it.

### Preamble convention for `.tla` files

Each spec file opens with:

```tla
\* This spec defends invariant <NAME> for oxymake `ox run`.
\* It models <SURFACE>, a CSTAFP <full|lite> surface.
\* Justification: ADR-015 (named invariants) + Newcombe et al. (CACM 2015).
\* Out of model (see docs/architecture/boundary.md): <list>.
```

### Reformulated load-bearing paragraph

The following replaces, as the load-bearing justification, the earlier
"LLMs vs bytes" framing:

> *Oxymake adopts named invariants and their associated TLA+ specs
> because `ox run` exhibits the **CSTAFP** property — Concurrent State
> Transitions Across independently-Failing Peers — on at least three
> surfaces: multi-session reclaim, cancel propagation, and post-crash
> recovery. On those surfaces, the system's state is a relation between
> the states of independently-failing peers, a property that no
> sequential test exercises even at maximal coverage, because the
> cardinality of the reachable interleaving space is strictly greater
> than the cardinality of the tested space. The empirical reference is
> Newcombe et al., How Amazon Web Services Uses Formal Methods, CACM
> 58(4), 2015, which reports seven bugs (one with a 35-step trace)
> found by TLC in ten AWS systems, missed by testing, code review,
> static analysis, stress testing and fault injection. Those systems
> were orchestrated on deterministic components; the hazard class is
> concurrentielle, pas adversariale.*

### OX-2 vs OX-7 — orthogonal properties

Two properties have been historically conflated in this ADR and in
the earlier `No Backchannel` discussion. They are orthogonal and must be named
separately.

**OX-2 No Backchannel** is a property of *information direction*. The
scheduler instructs peers (Ledger, executors). Peers report observed
state to the scheduler. The scheduler is never instructed by what it
observes. This is unidirectionality of control. It is defended today
by the guarded-state-transitions discipline (ADR-010) and the
content-addressable cache contract (ADR-001).

**OX-7 Re-derivability from disk** is a property of *state recovery*.
The running scheduler's in-memory `SchedulerState` (Frontier from
ADR-011) must be a pure function of the on-disk Ledger (`StateDb`)
plus the input DAG. Formally: for any execution that produces a
SchedulerState `S_t` at time `t`, restarting `ox run` from the same
Oxymakefile against the persisted `StateDb` at time `t` must produce
a Frontier `S_t'` such that the subsequent scheduling decisions are
identical (modulo timing fields).

A system can satisfy OX-2 (perfect unidirectional control) and still
violate OX-7 (in-memory state holds a value not derivable from disk —
process crash erases it). Conversely, a system could in principle
satisfy OX-7 (Ledger is full source-of-truth) while permitting
controlled backchannels for orchestration (violating OX-2). One name
per property; one test per name.

**Status**: OX-7 is **draft** — but the remaining gap is the *formal
spec*, not the code. The re-derivability constructor `resume` is
implemented (`crates/ox-core/src/scheduler.rs`, a method on `Frontier`,
the in-memory `SchedulerState` of ADR-011): it rebuilds `statuses`,
`pending_upstream`, and the `ready_frontier` as a pure function of the
on-disk `LedgerSnapshot` plus the input DAG. Its equivalence oracle —
state `S₁` from a live run ≡ state `S₂` reconstructed by `resume` — is
asserted by the unit tests `frontier_poison_diamond_rederives_frontier`,
`resume_from_empty_snapshot_equals_new`, and
`resume_keeps_downstream_blocked_when_upstream_failed`, and exercised
end-to-end by the `crash_and_restart_yields_same_terminal_status_set`
integration test (all passing). This **supersedes** the
earlier grep claim of "zero matches", which is stale as of
the constructor landing.

What keeps OX-7 at **draft** is the absence of a *machine-checked*
re-derivability argument: the spec `spec/tla/Recovery.tla` is not yet
written, so there is no TLC proof that the equivalence holds across the
full cardinality of crash points (the Rust tests cover specific
schedules, not the interleaving space). This matches the paper's OX-7
row, which still marks `Recovery.tla` **(pending)**. OX-7 graduates to
`Accepted` when that spec lands and model-checks. The exit criterion is
therefore the spec, not a test name — the originally-planned
`ox7_crash_and_restart_yields_same_report` test corresponds to the
shipped `crash_and_restart_yields_same_terminal_status_set` integration
test, which already passes.

A sunset is set: if `spec/tla/Recovery.tla` has not landed by
**2026-12-01** (the first six-month sunset review of this ADR, already
in §Falsifiability), OX-7 is rétrogradé à *goal* non-ratifié in the
review, per the same mechanism that governs the other `.tla` modules.

## Consequences

### ADR axis and TLA+ axis are dissociated

The ADR plafond (Shannon ~30–40) measures the **decision surface** of
the product — what choices the team has bound itself to. The TLA+ scope
(four modules ship-now, possibly six) measures the **CSTAFP surfaces**
where sequential testing cannot exercise the cardinality of
interleavings. The two quantities are independent: a project can carry a
large decision surface (many ADRs) with little or no TLA+, or a small one
with heavy TLA+. "Few ADRs implies little TLA+" is a non-sequitur — small
house, therefore no locks. Locks depend on who walks through, not on
floor area.

This dissociation is the disposition of Q4 from the design panel.
Future ADRs that mention either axis must not collapse them.

### Boundary thickens as inner scope grows

Each new internal module forces naming a new boundary axiom: adding
`EvictionRace.tla` brings `StorageDeleteAtomic`; adding
`CrashRecovery.tla` brings `StateDbAtomicCommit` and
`WAL_DurableOnFsync`. This is inverse-intuitive — widening the spec
does not shrink the frontier, it widens the frontier as well. It is
healthy: the frontier is precisely where the proof depends on SQLite,
on the filesystem, on the kernel, on the executor process. Making that
dependency *visible* is the return on investment of the substrate
boundary note (`docs/architecture/boundary.md`).

### Autonomous-agent amplification shifts the bottleneck

AWS reports ~2–3 weeks per spec, dominated by an engineer's
learn-from-scratch curve. Oxymake operates with one human and an
autonomous-agent fleet: drafting `.tla` text takes hours, and the cost moves to the
operator reading counterexamples. The bottleneck is human interpretation
of TLC traces, not syntax acquisition. Budget estimates: module 1 pays
~2 weeks of learn-curve; modules 2 + drop to ~3–5 days each.

### Falsifiability — the discipline must be falsifiable

This ADR is itself a claim. To remain falsifiable rather than
self-confirming, the discipline carries a sunset mechanism, tracked by
M6:

- **2026-06-15** — pilot molecule
  `task-tla-pilot-cache-consistency` must land a ≤80 L spec, TLC
  depth 12, and a one-page note within two days; operator validation
  under one hour. Failure reduces this ADR's ship-now scope to
  ceremonial D4.
- **2026-12-01** — first six-month sunset review. Each spec must show
  ≥1 entry in `spec/tla/TRACES.md` (a TLC-produced trace violating a
  named invariant) OR ≥1 entry in `spec/tla/DESIGN-CHANGES.md` (a
  design change motivated by the spec). Otherwise the spec is deleted
  with a sunset citation.
- **2027-06-01, 2027-12-01** — successive six-month reviews. Three
  consecutive `conditional` outcomes mandate deletion.
- **2028-05-01** — long-horizon review. Count production bugs whose
  root cause matches a named invariant. If zero, a meta-ADR reviews
  the entire `spec/tla/` directory.

The mechanism details, including the schemas of `spec/tla/TRACES.md`
and `spec/tla/REVIEWS.md`, are scoped to M6.

### Easier

- Concurrency hazards land in writing before they land in incidents:
  a TLC-produced trace is a precise, replayable artefact.
- The frontier ([`docs/architecture/boundary.md`](../architecture/boundary.md))
  makes substrate assumptions explicit and grep-able; an axiom change
  is a load-bearing review event.
- New ADRs touching a CSTAFP surface can reference the relevant spec
  rather than re-justifying the safety property in prose.

### Harder

- Maintaining four `.tla` files plus a boundary is a non-zero
  ongoing cost (~1 d/spec/year of drift).
- The discipline requires operator attention to counterexamples; a
  TLC trace that no one reads is worse than no spec at all (sunset
  catches this).
- The boundary grows whenever the inner scope grows, which means
  adding a module is never a one-file change.

## Alternatives Considered

**Keep the D4 three-module scope (CacheConsistency + CooperativeClaim +
Boundary as a `.tla` module).** Rejected on two grounds: (1) the
constructively-built 23-step oxymake trace violates three named
invariants simultaneously, none of which lives inside that scope, so
`CancelPropagation.tla` is required; (2) the `Boundary.tla` form
carried no TLC obligations because no interior module `EXTENDS` it —
its content is preserved without loss as an architecture note (see
`docs/architecture/boundary.md`). `CancelPropagation.tla` is the most
pure instance of CSTAFP in the codebase and is integrally modellable
in ≤120 L.

**Keep the prior load-bearing framing ("LLMs lie, bytes don't").**
Rejected on Popper grounds: the kernel claim ("no bug in concurrent
surfaces that a ~100 L spec would have surfaced") is testable, but the
hedged form ("almost nothing") is not falsifiable. Replacing the
framing with CSTAFP makes the discipline answerable to evidence.

**Tie TLA+ spec count to ADR count.** Rejected as a category error.
ADRs measure product decisions (the user-visible *what we chose*);
TLA+ specs measure concurrent interleaving safety (the
*invariant-preserved-across-peers*). The Karpathy image is decisive:
small house, therefore no locks — locks depend on who walks through,
not on floor area.

**Ship all six modules at once (including `EvictionRace` and
`CrashRecovery`).** Rejected as premature: the surface code for Stage 2
(memory pressure / eviction-under-load) is not yet in flight, and the
ADR-014 implementation is not yet finalised. A spec that anticipates
unwritten code drifts faster than one that follows it.

**Defer all TLA+ work behind a pilot.** Rejected as composition error:
the pilot (M-style ≤2 d falsification by Karpathy) and the sunset
mechanism (Popper's 6-month criterion) test different things — the
pilot tests the *writing pipeline* (does the agent fleet amplify?), the
sunset tests the *value of the spec* (does TLC pay?). Both belong; the
pilot is a precondition on activation rather than a substitute for the
discipline.

## References

- Newcombe, Rath, Zhang, Munteanu, Brooker, Deardeuff. *How Amazon
  Web Services Uses Formal Methods*. Communications of the ACM 58(4),
  April 2015, pp. 66–73. DOI 10.1145/2699417.
- Design panel synthesis (verdict 4/4, §5 D-1 / D-2 / D-3 / D-5 are the
  source texts reformulated above).
- Superseded design panel that introduced the D4 scope;
  the present ADR revises its load-bearing reasoning.
- ADR-010 — guarded state transitions (the WHERE-precondition layer
  on which `CooperativeClaim.tla` and `CancelPropagation.tla` model
  their atomic steps).
- ADR-012 — cooperative multi-session (the surface modelled by
  `CooperativeClaim.tla`).
- ADR-013 — cancelled vs skipped semantics (the surface modelled by
  `CancelPropagation.tla`).
- ADR-014 — cache-state separation (the surface to be modelled by
  `CrashRecovery.tla` once activated).
- M6 — sunset mechanism (`TRACES.md`, `REVIEWS.md`, dated reviews).
- Chronicle entry in `docs/lore/CHRONICLES.md` (2026-05-27) — records
  that the "LLMs lie / bytes don't lie" framing was withdrawn from
  load-bearing position.
- §M7 + (d) recommendation panel — 3/3 substantive,
  convergence C1; authorises the demotion of `spec/tla/Boundary.tla` to
  `docs/architecture/boundary.md` and inscribes the promotion criterion
  (two axioms × ≥1 real bug via M3/M6 within six months ⇒ promoted to
  dedicated `.tla` modules at next sunset review).
- Complementary review round (panel godin/karpathy/
  forgemaster/kahneman/janis); the source of the OX-7 draft and the
  recognition that OX-2 (No Backchannel) and OX-7 (re-derivability) are
  orthogonal (the spelled-out distinction is kitchen orders vs kitchen
  burning down; the round also produced the Rust test skeleton that
  operationalises OX-7).
- Internal audit that surfaced the substitution OX-2 ↔
  stateless and motivated this amendment.
- M4 — `Frontier::resume` constructor (`crates/ox-core/src/scheduler.rs`)
  has landed and is tested; the OX-7 draft exit criterion is therefore now
  the `spec/tla/Recovery.tla` spec, not this constructor.

## Amendment — 2026-06-10: temporal gates removed (operator decision, premortem PM#5)

The operator ruled that **no time-based gating may ship in this
repository**: no deadlines, no sunset *dates*, no staleness timers, no
date-conditional CI behavior. This amendment applies that ruling to
the calendar inscribed in this ADR; the body above is preserved as the
historical record and is **superseded on every calendar point** as
follows:

- The engaged review dates in §Falsifiability (2026-06-15,
  2026-12-01, 2027-06-01, 2027-12-01, 2028-05-01) are **void**.
  Reviews are now **release-gated**: a spec review is a mandatory item
  of every `v*` release checklist (`docs/RELEASE-CHECKLIST.md`), with
  verdicts recorded in `spec/tla/REVIEWS.md`. The sunset *discipline*
  — evidence or deletion, three-consecutive-`conditional` rule — is
  unchanged; only its trigger moved from the calendar to the release
  event.
- The `Recovery.tla` sunset ("if not landed by 2026-12-01") becomes:
  OX-7 is adjudicated at each release review; it remains a *goal*
  (non-ratified) until `spec/tla/Recovery.tla` lands and model-checks.
- The boundary-note promotion criterion ("two axioms × ≥1 real bug
  within six months, by 2026-11-27") becomes: promotion is adjudicated
  at release reviews on the same evidence, with no expiry date.
- The CI tripwire (`.github/workflows/drift-tripwire.yml`) retains
  only the **content** check (TLA+/Rust ratio floor 0.44 %); its
  60-day staleness timer and 2026-09-01 TRACES.md deadline were
  removed in the same decision.
- The contingent-disposition clause in the Preamble ("3+ consecutive
  red builds **within any 6-month window**") loses its window: 3+
  consecutive red Drift Tripwire builds auto-demote the discipline,
  with no calendar qualifier. Consecutiveness is an event property;
  the window was the only temporal residue. This edit to the binding
  quote (here and in `OXYMAKE-THESIS.md`) is made under the clause's
  own escape hatch: a public ADR amendment with commit-trail
  justification — this section, this commit.

Rationale: a calendar gate on a solo-maintained project fires on the
maintainer's *absence*, not on the artefact's *state* — it punishes
sleep, which `docs/HIBERNATION.md` recognises as a legitimate state.
Release-gating preserves the falsifiability (the discipline still
cannot be waived silently: skipping the review item is visible in the
release checklist) while ensuring every red signal traces to a content
change someone made, never to time passing.
