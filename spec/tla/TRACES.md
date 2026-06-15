# `TRACES.md` — TLC Counterexample Ledger

This file records every TLC counterexample produced against a spec in
`spec/tla/`. Each entry is append-only: even after the bug is fixed,
the trace stays as evidence the spec earned its keep within its
review window.

> **No calendar deadline (operator decision 2026-06-10, premortem
> PM#5).** An earlier discipline required a first real trace by
> 2026-09-01, enforced by a CI tripwire. That deadline is void:
> entries land here when TLC actually surfaces a counterexample, and
> the ledger's emptiness is adjudicated at each `v*` release review
> (`REVIEWS.md`, `docs/RELEASE-CHECKLIST.md`) — a spec that never
> earns evidence is deleted by the three-conditional rule, not
> reddened by a clock.

## Schema

```markdown
## TRACE-<NNN> — YYYY-MM-DD — <Module>.tla
- **Spec**: `spec/tla/<Module>.tla` (first commit `<short-hash>`)
- **Invariant violated**: `<InvariantName>` (defined at `<Module>.tla:<lineno>`)
- **TLC config**: `spec/tla/<Module>.cfg` (workers=<N>, depth=<D>, …)
- **Trace**: <inline excerpt OR `spec/tla/traces/TRACE-NNN.txt`>
- **Root cause** (Rust): <one sentence, naming the file/function>
- **Fix commit**: `<short-hash>` — <commit subject>
- **Pre-existing integration test that did NOT catch this**: <test name OR "(none — gap acknowledged)">
- **Status**: open | closed
```

### Required for sunset accounting

- **Invariant violated** must match a named invariant in ADR-015 (e.g.
  `CancelledNeverCached`, `EvictPrecedesUnregister`). A TLC violation
  of an unnamed property is a *bug in the spec*, not in the code, and
  goes to `DESIGN-CHANGES.md` instead.
- **Pre-existing integration test that did NOT catch this** must be
  filled in. The sunset rule requires the trace to surface a state
  *not exercised by any pre-existing integration test* — this field
  is how that condition is audited.

## Counting rule

A spec is **corroborated for a review window** if it has ≥1 entry
in this ledger *and* in `DESIGN-CHANGES.md` whose date falls within
the window. (Either ledger alone suffices; both make the window
strongly corroborated.) A window runs from one `v*` release review to
the next; a spec's first window starts at its first-commit author
date.

---

## Entries

<!--
TRACE entries are appended below. The mechanism is alive when this
section is no longer empty. M6 records the schema; M2-M5 will populate
it as TLC surfaces traces.
-->

## TRACE-000 — 2026-05-27 — (mechanism bootstrap)
- **Spec**: *(none — this entry is the schema placeholder)*
- **Invariant violated**: *(n/a)*
- **TLC config**: *(n/a)*
- **Trace**: *(n/a)*
- **Root cause** (Rust): *(n/a)*
- **Fix commit**: *(n/a)*
- **Pre-existing integration test that did NOT catch this**: *(n/a)*
- **Status**: closed (bootstrap-only — DO NOT count toward sunset)

Bootstrapped by M6 to fix the ledger schema.
The first real trace will be TRACE-001.

## TRACE-001 — 2026-06-10 — CooperativeClaim.tla
- **Spec**: `spec/tla/CooperativeClaim.tla` (belief-variable revision, pre-pub pass)
- **Invariant violated**: `DoneByClaimHolder` (the falsifiable refinement of
  INV-2 added by the same revision — see the honesty note at the
  NoDoubleRunning block)
- **TLC config**: `spec/tla/CooperativeClaimUnguarded.cfg`
  (`SessionFilterEnabled = FALSE`, 3 sessions, 2 jobs, TTL 2, MaxClock 4;
  violation at depth 7, ~3.5k distinct states)
- **Trace**: `spec/tla/traces/TRACE-001.txt` — s2 claims j1, goes stale,
  Reclaim resets j1, s1 re-claims it, zombie s2 terminalizes j1:
  `done_by[j1] = s2 # claim_session[j1] = s1`. Reproduce with
  `spec/tla/run-tlc.sh --red`.
- **Root cause** (Rust): `ox-state/src/db.rs` `complete_job`/`fail_job` —
  the terminal UPDATE filtered on `status='running'` but not on
  `session_id`, so a session whose claim had been reclaimed could still
  terminalize the job (and land stale output hashes) after a peer
  re-claimed it.
- **Fix commit**: `3eae6a5` — fix(state): session_id zombie guard on
  complete_job/fail_job (H16)
- **Pre-existing integration test that did NOT catch this**:
  (none — gap acknowledged; `cooperative_sessions.rs` covered claim/reclaim
  races but never a terminal write from the reclaimed session. Closed by
  `zombie_session_cannot_terminalize_reclaimed_job`, added with the fix.)
- **Status**: closed

**Chronology note (honesty).** The bug was found by code review
(pre-publication audit, finding H16), not by TLC: the spec as then
written could not express it — `NoDoubleRunning` was true by typing
(premortem finding H18). The spec was revised with per-session belief
state in the same wave, and this trace is the model's *reproduction* of
the reviewed bug, committed so the invariant's falsifiability is
machine-checkable rather than asserted. The discipline still earned its
entry: the revised spec turns the H16 class into a one-command red run.
