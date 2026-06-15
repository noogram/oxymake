# `DESIGN-CHANGES.md` — Spec-Motivated Design Changes

This file records every oxymake design change that was *motivated by
the act of writing a `spec/tla/` specification*, even when TLC produced
no counterexample. Writing the spec forces a question; the answer
sometimes changes the Rust code. That counts.

## Schema

```markdown
## DC-<NNN> — YYYY-MM-DD — <Module>.tla
- **Spec**: `spec/tla/<Module>.tla` (first commit `<short-hash>`)
- **Question forced**: <one or two sentences — what the act of formalising made impossible to ignore>
- **Pre-spec behaviour** (Rust): <how the code answered the question before the spec was written>
- **Post-spec behaviour** (Rust): <how the code answers it now>
- **Rust commit**: `<short-hash>` — <commit subject>
- **Strength of motivation**: load-bearing | contributory | clarifying
```

### Strength values

- **`load-bearing`** — the change would not have happened without the
  spec. The PR commit message must reference `spec/tla/<Module>.tla`
  for the sunset rule to credit this entry.
- **`contributory`** — the spec sharpened a change that was already in
  flight. Counts for sunset.
- **`clarifying`** — the spec named a property the code already had,
  with no code change. Does *not* count for sunset (the spec earned
  nothing from the operator's perspective).

## Counting rule

See `TRACES.md` — a spec is corroborated for a review window if
it has ≥1 `load-bearing` or `contributory` entry here, OR ≥1 trace
in `TRACES.md`, dated within the window (a window runs from one `v*`
release review to the next).

`clarifying` entries are kept for honest accounting but do not satisfy
the sunset criterion. A spec that accumulates only `clarifying`
entries should be reviewed under the *conditional* heading at the next
release review.

---

## Entries

<!--
DC entries are appended below. M6 records the schema; M2-M5 will
populate it as specs force design changes.
-->

## DC-000 — 2026-05-27 — (mechanism bootstrap)
- **Spec**: *(none — this entry is the schema placeholder)*
- **Question forced**: *(n/a)*
- **Pre-spec behaviour** (Rust): *(n/a)*
- **Post-spec behaviour** (Rust): *(n/a)*
- **Rust commit**: *(n/a)*
- **Strength of motivation**: *(n/a — bootstrap-only, DO NOT count toward sunset)*

Bootstrapped by M6 to fix the ledger schema.
The first real entry will be DC-001.

## DC-001 — 2026-06-10 — CooperativeClaim.tla
- **Spec**: `spec/tla/CooperativeClaim.tla` (belief-variable revision)
- **Question forced**: which arms does the terminal UPDATE's WHERE clause
  need for the multi-session protocol to be safe? Modelling `Terminalize`
  made the answer mechanical: `status='running'` alone admits the zombie
  write; the guard must also pin `session_id`. The constant
  `SessionFilterEnabled` reifies exactly this choice, and TLC separates
  the two worlds (green/red configs).
- **Pre-spec behaviour** (Rust): `complete_job`/`fail_job` filtered on
  `status='running'` only — any process could terminalize any running job.
- **Post-spec behaviour** (Rust): both carry `AND session_id = ?`; the
  external-executor sync paths that legitimately terminalize across
  sessions moved to explicit `reconcile_*` variants.
- **Rust commit**: `3eae6a5` — fix(state): session_id zombie guard on
  complete_job/fail_job (H16)
- **Strength of motivation**: contributory (the bug arrived via the
  pre-publication review, finding H16; formalising Terminalize in the same
  wave sharpened the fix's shape — guarded core + named reconcile
  exceptions — and produced the regression witness TRACE-001)
