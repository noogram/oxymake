# Deferred follow-up — `task-ox-translate-cwl-implementation`

**Status**: NOT yet nucleated. Gated by composite trigger
(see [`ox-translate-cwl-trigger.md`](ox-translate-cwl-trigger.md)).
**Parent idea**: the CWL-translate idea.
**Parent deliberation**: the Guix-capability deliberation.

This document is the **one-step-away nucleation kit**. When both
trigger conditions are satisfied (≤ 2026-11-27), the operator runs
the command at §"Nucleation command" and the implementation
molecule is born with full context already wired.

## When to nucleate

Trigger fires when BOTH:

1. `ox-exec-guix` crate published its first release AND design
   note cleared.
2. A complete CWL import request issue lands on `noogram/oxymake`
   from a named external user with a concrete workflow attached.

Either unmet at 2026-11-27 ⇒ hold; either unmet at 2027-05-27 ⇒
cancel-with-citation (see parent proposal §"Trigger outcomes").

## Nucleation command

```bash
cs nucleate task-work --kind task \
  --var topic="ox-translate cwl — CWL import adapter for Oxymake" \
  --var parent_idea="<cwl-translate-idea>" \
  --var parent_delib="<guix-capability-delib>" \
  --var trigger_issue="<github-issue-url-from-named-adopter>" \
  --var trigger_adopter="<adopter-real-name>" \
  --var depends_on="<ox-exec-guix-task> (ox-exec-guix)" \
  --blocked-by <ox-exec-guix-task> \
  --tag "adapter:cwl" --tag "external-adoption"
```

Update `--var trigger_issue` and `--var trigger_adopter` from the
GitHub issue at firing time. Do not nucleate without these values
— the named-adopter trace is part of the molecule provenance.

## Task scope (when nucleated)

The implementation molecule should ship a `crates/ox-translate-cwl`
adapter with:

### Phase A — schema + parse (≈ 2 weeks)

- Parse CWL v1.2 (latest stable) workflow + tool YAML using the
  upstream `cwl-utils` Python reference as ground truth (port via
  `serde_yaml` + hand-written types, NOT a YAML-to-AST blackbox).
- Reject v1.0 / v1.1 with a clear error message pointing to a
  CWL migration tool. Supporting three versions of the spec on
  day-1 is scope creep godin would (correctly) attack.
- Unit tests against the official CWL conformance suite — at
  least the `required` tier (currently ~50 tests).

### Phase B — translate to Oxymake DAG (≈ 3 weeks)

- Map `CommandLineTool` → Oxymake step with `ox-exec-guix` recipe.
- Map `Workflow` → Oxymake DAG with input/output wiring.
- Handle CWL `scatter` via Oxymake's existing fan-out primitive
  (DO NOT invent a new primitive — if `scatter` cannot be
  expressed via existing fan-out, surface back to core, do not
  paper over).
- Skip on day-1: `ExpressionTool` (JavaScript eval — security
  boundary), `SubworkflowFeatureRequirement` (defer until a
  concrete adopter needs it).

### Phase C — round-trip + adopter validation (≈ 1 week)

- Run the trigger-adopter's actual workflow end-to-end on
  Oxymake + `ox-exec-guix`.
- Compare outputs against `cwltool` reference run on the same
  inputs (byte-identical or documented divergence).
- Adopter sign-off is the completion criterion — not green tests.

### Out of scope (explicitly)

- CWL **export** from Oxymake DAGs (asymmetric on purpose —
  import is the adoption surface, export is a different bet).
- `ExpressionTool` JS eval (security perimeter).
- v1.0 / v1.1 support (migration tool exists upstream).
- Subworkflows (defer to second adopter signal).

## Deliverables

- `crates/ox-translate-cwl/` crate with the three-phase scope.
- `docs/adapters/cwl.md` user-facing documentation citing the
  conformance subset.
- `examples/cwl-import/<adopter-workflow>/` worked example built
  from the trigger adopter's workflow.
- Adopter sign-off recorded in the molecule's `done` artifact.

## What the molecule MUST cite

The implementation molecule must include in its briefing:

- This document's path.
- The trigger issue URL (from the named external adopter).
- The composite trigger proposal
  ([`ox-translate-cwl-trigger.md`](ox-translate-cwl-trigger.md)).
- The 4-persona panel synthesis (Guix-capability deliberation).
- godin's dissent, *with the named-adopter clause now
  satisfied*. The dissent earned the trigger; honoring it means
  shipping only to that adopter's concrete need, not building
  the speculative super-set.

## Sunset path (if trigger never fires)

If trigger never fires by 2027-05-27:

1. Append entry to `docs/non-goals.md` (create if missing):
   ```markdown
   ## ox-translate cwl (2026-05-28 → 2027-05-27 sunset)
   Deferred under composite trigger from the Guix-capability deliberation.
   Trigger did not fire. godin's "no" verdict corroborated.
   References:
   - docs/proposals/ox-translate-cwl-trigger.md
   - docs/proposals/ox-translate-cwl-followup.md (this file)
   ```
2. `cs collapse <cwl-translate-idea> --reason "sunset 2027-05-27, trigger never fired"`.
3. Leave both proposal files in-tree as chronicle artifacts. Do
   NOT delete — they are the inscription of a decision-not-taken,
   which is itself load-bearing for future "should we build X?"
   deliberations.
