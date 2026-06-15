# Returning to OxyMake

> **Audience:** the operator coming back after a pause, or a fresh agent
> picking up the rig for the first time.
> **Last reviewed:** 2026-06-10 (temporal gates removed, premortem PM#5).
> **Next review:** at the next return or `v*` release — whichever comes
> first. If a step below contradicts the repo, this file is *itself* the
> first bead. (No calendar cadence: re-reading is gated on the events
> that make it useful, not on a clock.)

**If you have been away from OxyMake for more than two weeks, read this
first.** It is the cognitive canary — the sequence of gestures that
proves the floor is still solid before you decide what to build next.

If a step below feels unfamiliar or breaks, do **not** patch the next
step. Stop, fix the floor, re-read this file.

**If a `.hibernation` file exists at the repo root — or you are
returning after a long absence — read [`HIBERNATION.md`](HIBERNATION.md)
*before* this file.** That is the document for a project that may have
slept: it tells you how to read the sleep signal and the awakening
protocol that wraps the canary below.

## What OxyMake is, in one breath

A workflow engine. File-based rules, backward-chaining DAG, content-
addressable cache. Written in Rust because the engine must be fast,
deterministic, and crash-safe; consumed via TOML because workflows are
data, not programs. The founding sentence is *"Rust provides the engine,
the workflow provides the intent"* — same shape as Gas Town's
*"Go provides transport, AI provides cognition."* The engine never
decides *what* to compute, only *how* to orchestrate computations the
workflow has already declared. Determinism is the foundation.

The persona served is **a quant maintaining a Snakemake pipeline of
5–500 rules who lost half a day this month to a phantom re-run and
wants to drive the pipeline from an agent.** Anyone broader is a bonus.

## The 30-minute re-warm sequence (six steps, in order)

These six steps are the contract. Each one earns you the right to do
the next. None is optional.

1. **`bash scripts/re-warm.sh`** — the mechanical canary. Five
   invariants: I-build, I-tests-green, I-demo-runs, I-baseline-stable,
   I-rederive. Green exit means engine compiles, tests pass, demo runs,
   the DAG resolves to a known shape, and the plan re-derives
   identically after a state wipe. Target < 2 min on a warm cargo
   cache; longer means the build graph itself is rotting.

2. **`git log --since='2 weeks ago' --oneline`** — what landed while
   you were away. Read titles, not diffs. Look for shape: which crates
   moved, which ADRs were added, which features grew.

3. **`docs/adr/STATE.md`** — the projection emitted by
   `scripts/adr-lint.py --emit-state`. Status and citation graph for
   every ADR. If `STATE.md` is older than your last ADR edit, re-run
   the linter before trusting it.

4. **The attestation table at the head of `OXYMAKE-THESIS.md`** — six
   to ten rows mapping each load-bearing principle to the test, code,
   or sunset note that attests it today. A principle with no
   attestation is aspiration, not invariant.

5. **`docs/health/` — the most recent snapshot.** Three rot metrics:
   M-dep-untested, M-clippy-allfeatures, M-test-silence. Threshold
   breaches are alarms, not chores.

6. **The five most recent entries in `docs/lore/` (chronicles).** The
   *why* that does not fit in a commit message. Nothing chronicled in
   8 weeks is itself a signal.

Total budget: **30 minutes**. If you cannot read everything in 30
minutes, the artefacts are too long. Shrink the artefacts, not the
budget.

## What is unstable right now

Seven public surfaces are pre-1.0. Their per-surface stability — what is
*stable* (breaks only with a version bump + a `CHANGELOG.md` entry) versus
*unstable* (may change shape, name, or disappear between any two releases)
— is now declared in [`STATUS.md`](../STATUS.md) (landed in the M8
public-contracts review, 2026-05-27). Read it before building against any of them:

- `ox` CLI (25 sub-commands; a stable subset is pinned, the rest exploratory)
- `Oxymakefile.toml` format (`format_version = "1"` landed, optional today)
- `.oxymake/state.db` schema (forward-only migrations; no downgrade promise)
- `--json` NDJSON events (event *names* stable; payload fields unstable)
- Plugin Rule API (extension traits, compile-time only; SemVer not yet enforced)
- Env vars (`OX_CACHE_VALIDATION`, `OX_WC_*`, `OX_JOB_ID`, …)
- `ox.lock` format (`schema_version = 1`; rollover policy TBD)

If you build against a surface `STATUS.md` marks unstable, write the
assumption down in the PR.

## What is NOT in the sequence (and why)

- **The thesis body.** Read on demand from the attestation table,
  never linearly. The thesis is the substrate the canary measures
  against, not the measurement itself.
- **The TLA+ specs (`spec/tla/*.tla`).** Read only if a CE violation
  surfaces in CI or you are designing a new module's invariants.
- **Old `docs/design/` notes predating 2026-04 that no living ADR
  cites.** Sunset by neglect — promote load-bearing content into the
  cited ADR or a chronicle, then delete the original.
- **Bead/issue boards.** They belong to the work-in-flight loop, not
  the re-warm loop. Triaging beads before the canary is green is
  substitution.

## Discipline of this file

- ≤ 800 words. If it grows past 800, prune; do not append.
- Dated. The *Last reviewed* line above is the contract.
- Re-read at each return and at each `v*` release. If it still feels
  right, bump the date and ship the one-line commit. If it does not,
  that is the first bead.
- One owner: the maintainer. Automated agents may file issues against it;
  they do not edit it without an explicit hook.

*Last sentence on purpose: the goal is to spend less than 30 minutes
here, then close this file.*
