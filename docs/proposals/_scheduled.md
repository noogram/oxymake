# Deferred checkpoints — proposal triggers and re-evaluation events

This file is an **event ledger** for proposals that defer a decision
to a future trigger. Agents and operators should grep this file when
running a `v*` release review (`docs/RELEASE-CHECKLIST.md`) to surface
proposals whose trigger must be re-evaluated.

> **Operator decision 2026-06-10 (premortem PM#5):** all temporal gates
> were removed from this repository. This ledger was previously a
> *dated TTL ledger* with calendar checkpoints and hard sunset dates;
> those rows are struck below, preserved per the append-only
> discipline. Deferred proposals are now re-evaluated at **release
> reviews**, and sunsets fire on **evidence counts across consecutive
> reviews**, never on a date.

Format: one row per deferred decision.

| Event | Proposal | Trigger to re-evaluate | Action |
|-------|----------|------------------------|--------|
| Every `v*` release review | [`ox-translate-cwl-trigger.md`](ox-translate-cwl-trigger.md) | Composite trigger: named external adopter + concrete workflow (via issue template) | Trigger fired ⇒ nucleate `task-ox-translate-cwl-implementation`. Zero qualified requests after **two consecutive release reviews** ⇒ cancel-with-citation: append entry to `docs/non-goals.md`, `cs collapse <cwl-translate-idea> --reason "sunset (release-review rule)"`. Keep proposal files in-tree. |

Struck historical rows (calendar gates, voided 2026-06-10):

| Date | Proposal | Event | Action if reached without trigger firing |
|------|----------|-------|------------------------------------------|
| ~~2026-11-27~~ | [`ox-translate-cwl-trigger.md`](ox-translate-cwl-trigger.md) | Composite trigger checkpoint | Superseded by the release-review row above. |
| ~~2027-05-27~~ | [`ox-translate-cwl-trigger.md`](ox-translate-cwl-trigger.md) | Hard sunset | Superseded by the two-consecutive-reviews rule above. |

## How to add an entry

When you write a proposal that defers to a future trigger:

1. Inscribe the trigger and the sunset *rule* (evidence across
   consecutive release reviews — never a date) in the proposal itself.
2. Add one row to the event table above.
3. Link to the proposal via relative path.
4. State the action explicitly — future-you should not have to
   re-read the full proposal to know what to do.

## How to remove an entry

When a trigger fires or a sunset rule is enforced:

1. Take the documented action (nucleate the follow-up molecule, OR
   cancel-with-citation).
2. **Strike** the row (do not delete it) so the historical
   inscription remains visible.
3. Append a brief outcome line in the proposal's chronicle.

The ledger is append-only inscription, not state — it accumulates
the trace of decisions-deferred-and-resolved.
