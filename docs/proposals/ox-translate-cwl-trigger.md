# ox-translate cwl — Composite Trigger & Sunset Proposal

**Status**: Deferred (yes-later under composite trigger)
**Parent deliberation**: Guix-capability deliberation (Q-GUIX-2)
**Sunset**: event-gated — see Amendment 2026-06-10 below
**Re-evaluation**: at each `v*` release review

> **Amendment 2026-06-10 (operator decision, premortem PM#5 — no
> temporal gates):** the calendar dates in this proposal (checkpoint
> 2026-11-27, sunset 2027-05-27) are void as gates. The **composite
> trigger is unchanged** — it was always evidence-based (a named
> external adopter + a concrete workflow), which is exactly why godin's
> dissent called it load-bearing. What changes is only the clock: the
> trigger is re-evaluated at each `v*` release review
> (`docs/RELEASE-CHECKLIST.md`). If, after **two consecutive release
> reviews**, zero qualified external requests have landed via the issue
> template, godin's verdict is corroborated and the proposal is
> cancelled-with-citation (entry in `docs/non-goals.md`), exactly as
> the original sunset prescribed. Dated clauses in the body below are
> historical record, superseded by this rule.

## Summary

The proposal to ship a `ox-translate cwl` adapter — importing Common
Workflow Language (CWL) workflows into the Oxymake DAG model — is
**deferred under a composite trigger**. The work will be nucleated
only when two independent conditions both hold; otherwise the
proposal sunsets without code on 2027-05-27 with a citation entry in
`docs/non-goals.md`.

The decision is not "no" and not "yes" — it is "yes-later, on
evidence, with a hard expiry."

## The composite trigger

Both sub-conditions must be true to unlock nucleation of
`task-ox-translate-cwl-implementation`:

### Condition 1 — `ox-exec-guix` shipped

The Guix execution backend crate (the `ox-exec-guix` parent task) must have:

1. Cleared its design note (architectural review complete).
2. Published at least one release on the workspace cargo registry.

Rationale: CWL adapters depend on having a reproducible execution
substrate to translate *into*. Building a translator before the
target backend ships produces an adapter whose target keeps
shifting under it. `ox-exec-guix` is the load-bearing dependency.

### Condition 2 — ≥ 1 named external CWL request

A real external user — **not the operator, not any internal
contributor** — must open one of:

- A GitHub issue against `noogram/oxymake` requesting CWL import.
- A pull request demonstrating CWL adapter scaffolding.
- A mail/Signal/email request to the operator naming themselves
  and citing a concrete workflow they want to port.

Each form requires **a named person + a concrete CWL workflow
attached or linked**. The following do NOT qualify:

- *"It would be nice if Oxymake imported CWL"* (no concrete workflow).
- *"Some users on r/bioinformatics asked about CWL"* (no named person).
- The operator's own speculation about future demand.
- A request from another internal galaxy or contributor (internal,
  not external adoption signal).

Rationale: godin's dissent (preserved below). Building an importer
without a named adopter produces an attractive surface that nobody
maintains and that erodes trust when it bitrots. The strict
named-adopter test is the falsifier.

## The godin dissent (inscribed)

> *Tu construis le pont avant que quiconque n'ait demandé à
> traverser la rivière. Et après, tu maintiens le pont pendant
> dix ans en espérant qu'un voyageur viendra.*

godin (1/4 of the panel) argued strict "no" — even yes-later
risks anchoring future attention on a speculative artefact. The
composite trigger is the structural mitigation: the **named
adopter** clause is precisely the test godin demanded. If no
named external request lands by 2026-11-27, godin's verdict is
corroborated and the deferral compounds toward the 2027-05-27
sunset.

This dissent is **load-bearing**, not decorative. It is the
reason the trigger is composite and not just temporal.

## Trigger outcomes

| State at checkpoint | Action |
|---------------------|--------|
| Both conditions met by **2026-11-27** | Nucleate `task-ox-translate-cwl-implementation` immediately. |
| Either condition unmet by **2026-11-27** | Hold; re-check at sunset. |
| Either condition still unmet by **2027-05-27** | Cancel-with-citation: write `docs/non-goals.md` entry citing this proposal + the Guix-capability deliberation. Close molecule with sunset note. |

## Panel summary (Guix-capability deliberation)

A 4-persona deliberation evaluated Q-GUIX-2 ("Should we ship
`ox-translate cwl`?"). Aggregate verdict: 3/4 yes-later + 1/4
strict no.

- **karpathy** (yes-later): CWL is the de-facto bioinformatics
  workflow lingua-franca; an importer materially widens Oxymake's
  reachable user base, but only after `ox-exec-guix` provides a
  reproducible execution target.
- **torvalds** (yes-later): "Show me the workflow." Build the
  adapter when someone hands us a concrete CWL file they want to
  run. Speculative spec-only adapters rot.
- **feynman** (yes-later): The translation problem is bounded and
  well-specified by the CWL schema — it is not research, it is
  engineering. Defer is fine; build is straightforward when
  triggered.
- **godin** (no): As inscribed above. The composite trigger is the
  structural acknowledgement that this dissent is *correct in
  spirit* even if outvoted on direction.

The aggregate verdict (b) yes-later was selected with the godin
dissent integrated into the trigger formulation, not buried in a
minority report.

## Falsifier

If by **2026-11-27** the trigger has fired (both conditions met)
AND `task-ox-translate-cwl-implementation` has NOT been nucleated
within 7 days, the trigger discipline failed and this proposal
becomes a chronicle entry on attention-debt rather than a working
deferral.

Conversely, **0 external CWL requests by 2026-11-27** corroborates
godin's "no" verdict and tightens the lean toward sunset rather
than away from it.

## Tracker mechanism — choice and rationale

Two options were evaluated for surfacing trigger-relevant external
requests:

| Option | Surface | Naming guarantee | Friction for external user | Visibility |
|--------|---------|------------------|---------------------------|------------|
| (a) `secretariat`/beads + keyword watcher | private galaxy | none (operator transcribes) | high — requires operator relay | internal |
| (b) `.github/ISSUE_TEMPLATE/cwl-import-request.md` | public repo | automatic via GitHub @user | low — single click on issues tab | public |

**Choice: (b) issue template** (file:
`.github/ISSUE_TEMPLATE/cwl-import-request.md`).

Rationale:

- The named-adopter clause is the load-bearing part of godin's
  dissent. Option (b) makes naming **structural** (GitHub assigns
  the author identity), not transcription-dependent.
- Option (a) introduces a relay step (operator hears about a
  request, transcribes to a bead, watcher fires) — each link in
  that chain is an opportunity to anchor on speculation rather
  than evidence.
- Option (b) is also the lowest-friction path for the external
  user. If even *that* surface produces zero qualified requests
  by 2026-11-27, that result corroborates godin's verdict
  cleanly. The cleaner the falsifier, the better the trigger.
- The template's required fields (real name, concrete workflow,
  CWL version, why Oxymake) operationalize the strict
  named-adopter test. Incomplete templates are visible as
  incomplete — no judgment call is needed.

The maintainer note at the bottom of the template wires the trigger
mechanically: when a complete template arrives AND `ox-exec-guix`
has shipped, nucleate `task-ox-translate-cwl-implementation` and
link the issue. Otherwise label `trigger-pending:ox-exec-guix`.

## References

- Deliberation: Guix-capability deliberation synthesis.
- Parent task (Guix backend): the `ox-exec-guix` task.
- This idea: the CWL-translate idea.
- Tracker: [`.github/ISSUE_TEMPLATE/cwl-import-request.md`](../../.github/ISSUE_TEMPLATE/cwl-import-request.md).
