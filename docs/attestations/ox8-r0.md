# OX-8 — R0 Orthogonality Attestation

> **Amendment 2026-06-10 (operator decision, premortem PM#5 — no
> temporal gates):** every calendar date in this document (the
> 2026-07-15 deadline, the 2026-12-01 promotion review, and the
> dated falsifier clauses) is void as a *gate*. R0 remains the
> evidence required for OX-8 promotion; the adjudication now happens
> at `v*` release reviews (`spec/tla/REVIEWS.md`,
> `docs/RELEASE-CHECKLIST.md`), per the ADR-015 amendment of the same
> date. The falsifier becomes date-free: inscribing OX-8 without a
> passing R0 is the discipline failing; a passing R0 left unpromoted
> across consecutive release reviews is the promotion mechanism
> failing. The body below is preserved as the historical record.

## Status

**TEMPLATE — observations PENDING.** This document is the audit-trail
artefact that gates OX-8 promotion. The four cells below must be filled
in from an actual run of `bench/orthogonality-r0/run.sh` on a host with
both `ox` and `guix` installed. Until then, R0 has NOT been attested
and OX-8 cannot be promoted to an invariant in
[`docs/adr/015-named-invariants.md`](../adr/015-named-invariants.md).

| Field | Value |
|---|---|
| Origin | Guix-capability deliberation (synthesis §I1), `popper §0` |
| Implementing task | the R0 attestation task |
| Bench script | [`bench/orthogonality-r0/run.sh`](../../bench/orthogonality-r0/run.sh) |
| Workflow `W` | [`bench/orthogonality-r0/workflow.toml`](../../bench/orthogonality-r0/workflow.toml) |
| Manifest | [`bench/orthogonality-r0/manifest.scm`](../../bench/orthogonality-r0/manifest.scm) |
| Deadline | 2026-07-15 (synthesis §7 — before FAIR-first paper rewrite) |
| Promotion review | 2026-12-01 (ADR-015 first sunset) |
| Falsifier | If R0 fails (any cell collapses) and OX-8 is nevertheless inscribed before 2026-12-01, the discipline has failed. Conversely, if R0 passes and OX-8 is NOT promoted by 2026-12-01, the promotion mechanism failed. |

---

## Claim under test

From `wiki §1` (the slogan form rejected by popper §0):

> *"OX-1 (cache key over outputs) ⊥ Guix store (content-addressable
> inputs). The two axes compose without conflict."*

Popper's split (synthesis §I1):

- *"OX-1 hashes outputs ; Guix store hashes inputs"* — definitional
  taxonomy, uncontroversial; not refutable.
- *"compose without conflict"* — **must** carry the load and **must**
  be tested by R0.

R0 attests the second half. The first half stands as taxonomy
regardless of R0's outcome.

---

## Protocol (popper §0)

Run `ox run` on workflow `W` over the 2×2 matrix:

| | **Output stable** | **Output drift** |
|---|---|---|
| **Store hash stable** | (1) both verdicts "no rerun" | (2) OX-1 "rerun", store "stable" |
| **Store hash drift**  | (3) OX-1 "no rerun", store "drift" | (4) both "rerun / drift" |

**Orthogonality holds iff** OX-1's verdict depends only on the output
column and the Guix-store verdict depends only on the row. Any cell
where both verdicts collapse together (e.g. OX-1 reruns whenever the
manifest changes, or the store hash drifts whenever the input changes)
refutes orthogonality.

### Operational encoding

The workflow `W` (see `workflow.toml`) has one rule, `emit`, which
`cat`s `input.txt` into `out/emit.txt`. Two invocations per cell:

| Axis | Knob | Stable | Drift |
|---|---|---|---|
| Output (OX-1) | `input.txt` content between inv1 and inv2 | unchanged | rewritten with different bytes |
| Store hash    | Guix manifest between inv1 and inv2       | `manifest-a.scm` both invocations | `manifest-a.scm` then `manifest-b.scm` (`coreutils` → `coreutils-minimal`) |

After the two invocations:

- **OX-1 verdict** is read from the second invocation's `ox run`
  output: `Cache: 1 of 1 job(s) up-to-date, skipping.` → *no rerun*;
  `▸ [emit] ... ✓ Completed 1/1` → *rerun*.
- **Guix store verdict** is read from
  `realpath $(guix shell -m M -- which cat)`: same path between the
  two invocations → *stable*; different paths → *drift*.

---

## Observed results

### Run metadata

| Field | Value |
|---|---|
| Date (UTC) | _to be filled_ |
| Host | _to be filled_ |
| `ox --version` | _to be filled_ |
| `guix --version` | _to be filled_ |
| `guix describe` (channel commits) | _to be filled_ |
| Operator | _to be filled_ |

### Cell (1) — output stable, store hash stable

| Field | Predicted | Observed |
|---|---|---|
| inv1 command | `guix shell -m manifest-a.scm -- ox run -f workflow.toml` (after `echo alpha > input.txt`) | _to be filled_ |
| inv2 command | `guix shell -m manifest-a.scm -- ox run -f workflow.toml` (input.txt unchanged) | _to be filled_ |
| OX-1 verdict | no rerun (cache hit) | _to be filled_ |
| Guix store path (inv1) | `/gnu/store/<hash>-coreutils-<version>/bin/cat` | _to be filled_ |
| Guix store path (inv2) | same as inv1 | _to be filled_ |
| Guix verdict | stable | _to be filled_ |
| Matches predicted? | — | _yes / no_ |

### Cell (2) — output drift, store hash stable

| Field | Predicted | Observed |
|---|---|---|
| inv1 command | `guix shell -m manifest-a.scm -- ox run -f workflow.toml` (after `echo alpha > input.txt`) | _to be filled_ |
| inv2 command | `guix shell -m manifest-a.scm -- ox run -f workflow.toml` (after `echo beta > input.txt`) | _to be filled_ |
| OX-1 verdict | rerun (cache miss) | _to be filled_ |
| Guix store path (inv1) | `/gnu/store/<hash>-coreutils-<version>/bin/cat` | _to be filled_ |
| Guix store path (inv2) | same as inv1 | _to be filled_ |
| Guix verdict | stable | _to be filled_ |
| Matches predicted? | — | _yes / no_ |

### Cell (3) — output stable, store hash drift

| Field | Predicted | Observed |
|---|---|---|
| inv1 command | `guix shell -m manifest-a.scm -- ox run -f workflow.toml` (after `echo alpha > input.txt`) | _to be filled_ |
| inv2 command | `guix shell -m manifest-b.scm -- ox run -f workflow.toml` (input.txt unchanged) | _to be filled_ |
| OX-1 verdict | no rerun (cache hit) | _to be filled_ |
| Guix store path (inv1) | `/gnu/store/<hash>-coreutils-<version>/bin/cat` | _to be filled_ |
| Guix store path (inv2) | `/gnu/store/<other-hash>-coreutils-minimal-<version>/bin/cat` (different hash) | _to be filled_ |
| Guix verdict | drift | _to be filled_ |
| Matches predicted? | — | _yes / no_ |

### Cell (4) — output drift, store hash drift

| Field | Predicted | Observed |
|---|---|---|
| inv1 command | `guix shell -m manifest-a.scm -- ox run -f workflow.toml` (after `echo alpha > input.txt`) | _to be filled_ |
| inv2 command | `guix shell -m manifest-b.scm -- ox run -f workflow.toml` (after `echo beta > input.txt`) | _to be filled_ |
| OX-1 verdict | rerun (cache miss) | _to be filled_ |
| Guix store path (inv1) | `/gnu/store/<hash>-coreutils-<version>/bin/cat` | _to be filled_ |
| Guix store path (inv2) | `/gnu/store/<other-hash>-coreutils-minimal-<version>/bin/cat` (different hash) | _to be filled_ |
| Guix verdict | drift | _to be filled_ |
| Matches predicted? | — | _yes / no_ |

---

## Verdict

- [ ] **R0 PASSES** — all four cells match the predicted orthogonal
      pattern. OX-1's verdict depended only on the output column;
      the Guix-store verdict depended only on the row. OX-8 is
      **eligible** for promotion to an invariant in `docs/adr/015-named-invariants.md`
      at the 2026-12-01 ADR-015 sunset review (subject to the
      `crates/ox-exec-guix` capability landing per
      `task-ox-exec-guix-spike`).

- [ ] **R0 FAILS** — at least one cell deviated from the predicted
      pattern. List collapsing cells and the observed coupling
      below. OX-8 cannot be promoted; the orthogonality claim must
      be reformulated and re-tested.

### Failure analysis (if R0 fails)

_to be filled — name the collapsed cell(s), describe the observed
coupling (e.g. "Guix store hash drift caused OX-1 to flip to rerun
even with input unchanged → OxyMake's cache key is silently
dependent on PATH-resolved binary identity"), and reference the next
deliberation that must reformulate the claim._

---

## How to re-run

```bash
./bench/orthogonality-r0/run.sh
# Writes bench/orthogonality-r0/results/results.tsv
# plus per-cell logs under bench/orthogonality-r0/results/<cell>/.
# Fill the "Observed" columns above from those artefacts.
```

The script degrades gracefully on hosts without `guix` (dry-run
skeleton, exit 3); the attestation can only be completed on a Guix
host.

---

## Independence from the `ox-exec-guix` crate

R0 does NOT require the `ox-exec-guix` crate to exist. It runs by
shelling out to Guix from the host-installed OxyMake binary via
`guix shell -m manifest.scm -- ox run ...`. The crate (tracked under
`task-ox-exec-guix-spike`, deadline 2026-09-01) is for integration
ergonomics; this bench is for attestation.

---

## Verification log

### 2026-05-29 — Systems-first pivot makes OX-8 absence the steady state

Per the operator's binding systems-first pivot decision (2026-05-29;
parent pre-mortem #2), the paper pivots
from a FAIR-first venue to a systems venue (ATC / ML-Sys / EuroSys-or-OSDI
workshop; final pick deferred to M-pivot-5). Two consequences for this
attestation:

1. **`ox-exec-guix` and the `ox-translate` CWL path are deferred to a community
   open-source effort** — they are off the operator's energy budget. No
   Linux+Guix host is on the operator's roadmap, so R0 has no scheduled run.
2. **OX-8's absence from `docs/adr/015-named-invariants.md` is now the intended
   steady state**, not a temporary gap awaiting attestation. The promotion gate
   below is preserved (a community R0 run could still attest OX-8 and trigger
   promotion), but the operator no longer plans to reach it.

The 2026-05-28 demotion stands and is reinforced: OX-8 is a design
conjecture, attestation is future work, and the Guix/CWL composition story moves
to a single dedicated "Future Work" section of the paper. The 2026-12-01
promotion mechanism remains in force should a community contributor provision a
Guix host and run R0 clean.

### 2026-05-28 — Paper demotes OX-8 to design conjecture

Per the OX-8 disposition decision (parent
pre-mortem #2, Deliverable 3), the paper no longer frames
orthogonality as *"the load-bearing scientific claim"* nor *"submitted on a
falsifiable bet pending before camera-ready."* OX-8 is now stated as a **design
conjecture whose empirical attestation (R0) is future work** (abstract, §1, §6.7,
§7.1). This template and its 2026-12-01 promotion mechanism remain in force: if a
Linux+Guix host is later provisioned, the verdict greps validated against real
`ox` output, and the matrix runs clean, OX-8 can still be **promoted** from
conjecture to attested. Demotion stops the paper overclaiming before the evidence
exists; it does not destroy the upgrade path.

### 2026-05-28 — Scaffold verified on Darwin (no Guix); observations PENDING

The R0 attestation implementing task produced and verified the
bench scaffold on a Darwin host without Guix. The 4-cell observation
step is therefore **deferred to a Guix-enabled host run before the
2026-07-15 deadline**.

What was verified on this host:

| Check | Result |
|---|---|
| `cargo check --workspace` | green (no Rust changes; baseline preserved) |
| `ox lint -f bench/orthogonality-r0/workflow.toml` | "Oxymakefile is valid (1 rules)" |
| `bash -n bench/orthogonality-r0/run.sh` | syntax OK |
| `./bench/orthogonality-r0/run.sh` (dry-run, no guix) | exits 3 with the 4-row skeleton TSV as designed |

The dry-run produced the structurally correct skeleton (one row per
cell, `indeterminate / dry-run` placeholders), confirming the runner
emits the right shape. It does **not** constitute an attestation —
the OX-1 and Guix verdicts cannot be observed without Guix.

What is deferred:

- All four cells' `Observed` columns above.
- The "Run metadata" block.
- The PASS / FAIL verdict checkbox.

The operator (or a Linux CI runner with Guix) must re-run
`./bench/orthogonality-r0/run.sh` and fill the cells from
`bench/orthogonality-r0/results/{cell-*/inv*.log,store_*.txt,verdict.tsv}`
before 2026-07-15. If R0 cannot be run before that date, the
2026-12-01 OX-8 promotion review must be deferred or OX-8 abandoned —
per the falsifier above, promoting OX-8 without R0 attestation
constitutes a discipline failure.
