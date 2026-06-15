# TLA+ Falsifiability Mechanism — Capture

> **Amendment 2026-06-10 (operator decision, premortem PM#5 — no
> temporal gates):** the engaged review *calendar* captured in this
> document (2026-06-15 pilot, 2026-09-01 TLC run, 2026-12-01 sunset
> review, and the feasibility table keyed to those dates) is void.
> Reviews are now release-gated — see `spec/tla/README.md`
> §Release-Gated Reviews and the ADR-015 amendment of the same date.
> This file is preserved unedited below as the historical capture of
> the mechanism's design.

> **Status:** Step 1 (Capture)
> **Parents:** design synthesis panel, ADR-015 / M1 task
> **Inscribed:** 2026-05-27
> **Author:** worker `m-canisme-de-falsifiabilit-tla-ac43`

## 1. Context

The design deliberation (panel: Knuth, Gödel, Karpathy,
Popper — 4/4 unanimous) re-anchored the TLA+ adoption decision for
oxymake on three independent legs:

1. **CSTAFP** (Popper, structural) — *Concurrent State Transitions
   Across independently-Failing Peers, where safety is a cross-peer
   relation*. Five oxymake surfaces qualify; three are Full CSTAFP, two
   are CSTAFP-lite.
2. **Cardinality** (Knuth, combinatorial) — the reachable interleaving
   space for N=8 workers × K=200 rules is ≈10⁸⁵ raw, ≈10¹² after
   confluence-class quotient. Ratio of CI-tested traces to reachable
   interleavings ≤ 10⁻¹². Two months of green CI corroborate an
   infinitesimal fraction of the state space.
3. **Construction** (Gödel) — a 23-step trace (plus 12 of cascade,
   four actors: `worker1`, `worker2`, `evictor`, `cli_cancel`)
   simultaneously violates three named invariants
   (`CancelledNeverCached`, `JobFailedImpliesNoIntent`,
   `EvictPrecedesUnregister`). Outside the original D4 scope; only
   widened scope (adding `CancelPropagation.tla`) catches it.

ADR-015 records the disposition: framing *"LLMs lie, bytes don't lie"*
is withdrawn from load-bearing position; CSTAFP + cardinality +
construction replace it; the discipline carries its own sunset clause.

## 2. The Idea (M6)

Design and install the **observational falsifiability mechanism** that
makes every `.tla` spec in `oxymake/spec/tla/` *falsifiable and
deletable on a defined horizon*. Without this mechanism, ADR-015 and
the four modules (M2 `CacheConsistency`, M3 `CooperativeClaim`, M4
`CancelPropagation`, M5 `Boundary`) decay into ceremonial decoration
— precisely the failure mode Popper §1 forbids.

The mechanism composes two clauses from the deliberation synthesis:

- **Karpathy §iii — pilot molecule (≤2 days).** A scope-bounded first
  spec (`CacheConsistency.tla`, ≤80 lines, defending invariants OX-1 +
  OX-6) executed by a polecat, TLC-checked at bounded depth, judged by
  the operator. Gate the rest of the rollout on the pilot's outcome.
- **Popper §1, §3, §4 — sunset rule (6 months).** Each spec must, within
  6 months of its first commit, surface either (a) ≥1 TLC trace
  violating a named safety invariant *not exercised by any pre-existing
  integration test*, OR (b) ≥1 oxymake commit whose message references
  the spec as design motivation. If neither: delete in the next PR
  citing this rule. Corroboration is always provisional — each
  six-month window is a fresh test.

## 3. Motivation

Three pathologies the mechanism averts.

**P1 — Ceremonial spec accumulation.** A `.tla` file is committed, never
re-opened, never refuted, never deleted. ADRs claim it "covers" a
concern. The code drifts and the spec stays. The spec stops being a
test of the code and becomes a stage prop.

**P2 — Untracked corroboration.** A spec quietly motivates design
changes (writing it forces the right questions; the answer changes the
Rust code), but no ledger records the link. Six months later there is
no evidence to reason about which specs are pulling their weight.

**P3 — Indefinite suspension.** Without dated review obligations, the
sunset rule is gestural. The deliberation already named the dates;
binding them to the ledger format is what makes the rule operate.

## 4. Initial Scope

### Three ledgers (Step 3 will create them)

| File | Purpose | Schema |
|---|---|---|
| `spec/tla/TRACES.md` | One entry per TLC counterexample | spec path, invariant violated, trace, Rust fix commit, status |
| `spec/tla/DESIGN-CHANGES.md` | One entry per design change motivated by the spec (even without a TLC trace) | spec path, question forced, Rust change commit |
| `spec/tla/REVIEWS.md` | Operator verdicts at fixed dates | per-spec verdict (keep \| conditional \| delete) + rationale |

### Sunset rule (verbatim — to be inscribed in `spec/tla/README.md`)

> Within 6 months of the first commit of `<spec>.tla`, TLC must surface
> EITHER (a) ≥1 execution trace violating a named safety invariant and
> not exercised by any pre-existing integration test, OR (b) ≥1 design
> change in oxymake whose commit message references the spec as
> motivation. If neither within 6 months, the spec is deleted in the
> next PR citing this rule. Corroboration is always provisional — each
> 6-month window is a fresh test.

### Engaged observation dates

| Date | Observation | Source |
|---|---|---|
| 2026-06-15 | Pilot polecat livré : `CacheConsistency.tla` ≤80 L, TLC depth 12, ≤2 j. Pass: operator validates in <1 h. Fail: >4 h interpretation OR pilot >2 j ⇒ D-1 falls back to ceremonial. | Karpathy §iii |
| 2026-09-01 | `CacheConsistency.tla` TLC-checked at ≥3 workers, ≥4 rules, ≥1 eviction round. Trace log written. | Popper §1 |
| 2026-12-01 | Sunset review #1. Each spec must show ≥1 TRACES.md OR ≥1 DESIGN-CHANGES.md entry, else delete PR cited under sunset rule. | Popper §3 |
| 2027-06-01 | Sunset review #2. Three `conditional` consecutive ⇒ mandatory delete. | Popper §4 |
| 2027-12-01 | Sunset review #3. | Popper §4 |
| 2028-05-01 | Long-horizon : count production bugs root-cause-matched to named invariants. If 0 ⇒ ADR-meta reviewing the entirety of `spec/tla/`. | Popper §3 |

### Pilot molecule (Karpathy §iii) — to be nucleated as child of M2

> `task-tla-pilot-cache-consistency` — polecat draft
> `spec/tla/CacheConsistency.tla` (≤80 L, defending OX-1 + OX-6) plus
> an `Init` state of 2 workers, 1 cache, 2 rules. TLC bounded check at
> depth 12. Deliverable: spec file, TLC config, run log, and a 1-page
> note answering:
> (a) did TLC surface any state the current oxymake tests cannot reach?
> (b) how many human-minutes did the operator spend on counterexample
>     interpretation?
> (c) was the agent-write → human-judge loop net positive vs the
>     operator writing alone?

**Pass criteria** — polecat delivers syntactically valid spec + TLC run
in ≤2 days; operator validates in <1 hour.

**Fail criteria** — pilot takes >2 days OR operator spends >4 hours on
interpretation ⇒ "fleet amplifies" falsified, D-1 falls back to D4
ceremonial scope.

## 5. Out of scope (this molecule)

- Writing the actual `.tla` files (M2–M5).
- Drafting ADR-015 (M1 — sibling molecule).
- Nucleating the pilot polecat itself (deferred to M2 child).

## 6. Cross-references

- Synthesis §5 D-4 — design panel synthesis
- Karpathy persona note — falsification molecule
- Popper persona note — sunset rule
- ADR-015 — `docs/adr/015-named-invariants.md` (M1)
- CHRONICLES 2026-05-27 — *La ligne 75 retirée du raisonnement
  load-bearing*

---

## 7. Feasibility (Step 2, 2026-05-27)

### 7.1 Directory structure

`spec/` does not yet exist in the worktree (verified `ls spec/` ⇒ ENOENT).
The mechanism prescribes a fresh subtree `spec/tla/` containing:

```
spec/
└── tla/
    ├── README.md           ← verbatim sunset rule
    ├── TRACES.md           ← counterexample ledger
    ├── DESIGN-CHANGES.md   ← spec-motivated commit ledger
    ├── REVIEWS.md          ← dated operator verdicts
    └── (future) *.tla      ← created by M2–M5
```

No existing path collides. `Cargo.toml`, `workspace.toml`, and the
`crates/` layout are untouched. The folder may sit at the workspace
root next to `docs/`, `crates/`, `tests/`. **Verdict: structurally
feasible.**

### 7.2 Calendar compatibility

Today: **2026-05-27**. All engaged dates are in the future:

| Date | Δ from today | Compatibility |
|---|---|---|
| 2026-06-15 (pilot) | +19 d | Achievable if M2 nucleates a `task-tla-pilot-cache-consistency` polecat by 2026-06-13 (≤2 d budget). |
| 2026-09-01 (TLC) | +97 d | Compatible with M2 implementation cadence. |
| 2026-12-01 (review #1) | +188 d | ≥ 6 months from M2 first commit if M2 lands before 2026-06-01. |
| 2027-06-01 (review #2) | +1 y 5 d | Standard semester cadence. |
| 2027-12-01 (review #3) | +1 y 6 m | Standard semester cadence. |
| 2028-05-01 (long-horizon) | +2 y | Long-horizon ADR-meta. |

No conflict with existing project milestones. **Verdict: calendar
compatible**, conditional on M2 starting before 2026-06-01.

### 7.3 Blocking dependencies

- **M1 (ADR-015)** — Merged at `cc49386`. ADR
  explicitly references M6 ledgers (`spec/tla/TRACES.md`,
  `spec/tla/REVIEWS.md`). ✅ unblocked.
- **M2–M5 (the `.tla` modules)** — Out of scope for M6; the ledgers
  *document* M2–M5 specs once they land, but the ledger files
  themselves can be created empty-with-schema before any spec exists.
  ✅ not a blocker.
- **Pilot molecule** — Will be nucleated as a child of M2; M6 does not
  spawn it. ✅ not a blocker.

**Verdict: no blocking dependency.**

### 7.4 Risks identified (Step 3 must address)

| Risk | Mitigation in Step 3 |
|---|---|
| Empty ledgers risk being forgotten | Placeholder entry referencing this molecule in each ledger + dated reminder in REVIEWS.md |
| Operator may not see the 2026-12-01 review date | Inscribe the calendar in `spec/tla/README.md` *and* in a chronicle entry; surface it via `cs schedule` or `~/.config/cosmon/patrols.toml` if the operator opts in |
| Sunset rule may be cited but never applied | Each REVIEW entry must produce a verdict (keep / conditional / delete), not a deferral |
| Specs land without first-commit dates being tracked | Convention: a spec's "first commit" is the commit that introduces the `.tla` file. TRACES/DESIGN-CHANGES entries must cite the first-commit hash so the 6-month window is unambiguous |

### 7.5 Step 2 verdict

**Feasible.** Step 3 (actionable plan) will create the three ledger
files with their schemas + placeholder entries, the `spec/tla/README.md`
holding the verbatim sunset rule + the engaged review calendar, and
inscribe a chronicle entry.
