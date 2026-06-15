# `REVIEWS.md` — Operator Verdicts at Release Reviews

This file holds the operator's adjudication of each spec at each
release review. Per the sunset rule (see `README.md`):

> Between one release review and the next, TLC must surface
> EITHER (a) ≥1 execution trace violating a named safety invariant and
> not exercised by any pre-existing integration test, OR (b) ≥1 design
> change in oxymake whose commit message references the spec as
> motivation. A spec with neither receives a `conditional` verdict at
> the review; three consecutive `conditional` verdicts and the spec is
> deleted in the next PR citing this rule. Corroboration is always
> provisional — each review window is a fresh test.

## Schema

```markdown
## REVIEW — YYYY-MM-DD (release vX.Y.Z)
| Spec | Verdict | Evidence | Rationale |
|---|---|---|---|
| `CacheConsistency.tla`  | keep \| conditional \| delete | TRACE-NNN / DC-NNN / none | <one phrase> |
| `CooperativeClaim.tla`  | … | … | … |
| `CancelPropagation.tla` | … | … | … |

**Overall rationale**: <one paragraph — what the panel of evidence says about the discipline as a whole>

**Next review**: at the next `v*` release
```

### Verdict values

- **`keep`** — ≥1 entry in `TRACES.md` or `DESIGN-CHANGES.md` (load-bearing
  or contributory) dated since the previous review. The spec is paying
  its keep.
- **`conditional`** — exactly zero qualifying entries, but the operator
  judges the spec is still earning (e.g. the surface has not been
  exercised yet; a follow-up TLC run is scheduled). Three consecutive
  `conditional` verdicts ⇒ mandatory delete.
- **`delete`** — zero qualifying entries and no rescue. The spec is
  removed in the next PR citing this rule (see README §How to delete).

## Review events

| Event | Review | Purpose |
|---|---|---|
| First TLC exercise | Pilot TLC run | First TLC depth-12 check on `CacheConsistency.tla` at ≥3 workers, ≥4 rules |
| Every `v*` release | Sunset review | Each spec must show ≥1 TRACES.md or ≥1 DESIGN-CHANGES.md entry since the previous review |
| Every `v*` release | Long-horizon tally | Count production bugs vs named invariants; persistent zero ⇒ ADR-meta on `spec/tla/` |

The review is a mandatory item of `docs/RELEASE-CHECKLIST.md`. (The
original calendar of engaged *dates* — 2026-06-15 through 2028-05-01 —
was voided on 2026-06-10 by operator decision, premortem PM#5: no
temporal gates. The discipline fires when the project acts, never
because time passed.)

---

## Reviews

<!--
REVIEW entries are appended below in chronological order. Operator
records the verdict at each release review.
-->

## REVIEW — 2026-05-27 (mechanism bootstrap)

| Spec | Verdict | Evidence | Rationale |
|---|---|---|---|
| (none yet) | n/a | n/a | The three TLA+ specs do not exist yet (M2–M4 not nucleated). This entry records that the mechanism is installed and the calendar engaged. The substrate boundary lives at `docs/architecture/boundary.md` (architecture note, not a `.tla` spec — demoted per the boundary-scope decision). |

**Overall rationale**: M6 installs the
falsifiability ledgers, the README sunset rule, and the review
calendar. The first real review is **2026-06-15** (pilot validation
of `CacheConsistency.tla`), conditional on M2 nucleating before
2026-06-13.

**Next review**: 2026-06-15

## REVIEW — 2026-06-10 (calendar voided — temporal gates removed)

| Spec | Verdict | Evidence | Rationale |
|---|---|---|---|
| `CacheConsistency.tla`  | conditional | none | Committed; not yet TLC-exercised. First verdict due at the first release review. |
| `CooperativeClaim.tla`  | conditional | none | Committed; not yet TLC-exercised. First verdict due at the first release review. |
| `CancelPropagation.tla` | conditional | none | Committed; not yet TLC-exercised. First verdict due at the first release review. |

**Overall rationale**: Operator decision (premortem PM#5) removed all
temporal gates from the repository. The engaged review *calendar*
(2026-06-15 … 2028-05-01) recorded in the bootstrap entry above is
void; reviews are now gated on `v*` releases via
`docs/RELEASE-CHECKLIST.md`. The sunset discipline itself — evidence
or deletion, three-conditional rule — is unchanged. The bootstrap
entry above is preserved as history, per the append-only rule.

**Next review**: at the next `v*` release
