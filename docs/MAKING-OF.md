# The Making of OxyMake

> This is the story of *how* OxyMake was built. It is not the pitch — the pitch
> is in the [README](../README.md), and it stands on the engine's behaviour, not
> on its construction. Read this once the cache story has earned your trust.

## Built by an agent fleet

Most of the commits in this repository are authored by AI agents working under
a single human maintainer. Of roughly 1,500 commits, about two thirds carry an
agent's name in the author field. The handles are real; you can see exactly who
wrote what in the
[contributor graph](https://github.com/noogram/oxymake/graphs/contributors).

This is unusual enough to deserve an honest account rather than a marketing
line, because for a *cache engine* — a tool whose entire job is to be trusted
with deciding what not to re-run — "who wrote the correctness logic" is a fair
question, not a gimmick.

It is also a story in two chapters, because two different agent orchestrators
drove this repository at different times. The full development history was
squashed out of this public repository when it was opened up and is kept in a
private archive of the full history; what each orchestrator left behind is
reconstructed below from the lineage still visible in the code and docs.

### Chapter 1 — Gas Town (March–April 2026)

Development started on 2026-03-24 piloted by [Gas Town](https://github.com/steveyegge/gastown),
Steve Yegge's agent-orchestration rig. In Gas Town's vocabulary, worker agents
are *polecats* (named after Mad Max characters — `chrome`, `furiosa`, `nux`,
`slit`, `capable`, `dementus`, `toast`, …), a *refinery* patrols the merge
queue, a *Mayor* dispatches work, and work items are *beads* stored in Dolt.

The richest traces of that era — roughly **~100 `polecat/*` branches** (one per
dispatched work item) and **per-agent commit authorship** (sixteen polecat names
plus `mayor` and `oxymake/refinery` as git authors, spanning 2026-03-24 to
2026-04-05) — were squashed out of this public repository when it was opened up.
That full development history is preserved in a private archive of the full
history; it is not reconstructable from the single release commit you see here.

What does remain visible in this public repo is the lineage Gas Town left in the
code and docs:

- **The build/test/lint command set** — originally declared for Gas Town's
  formulas (`mol-polecat-work`, `mol-refinery-patrol`), now consolidated into the
  Definition of Done table in [CONTRIBUTING.md](../CONTRIBUTING.md). A dedicated
  "Gas Town Rig Commands" section survived in `CLAUDE.md` for a while as a relic;
  it was removed once the commands had a single home in the DoD table.
- **[docs/AUDIT-REPORT.md](AUDIT-REPORT.md)** (2026-03-25), an automated audit
  from that era.

Gas Town drove the first arc: the workspace scaffolding, the core crates, and
the bulk of the early test suite landed through its merge queue.

### Chapter 2 — cosmon (April 2026 → today)

From late April 2026 the maintainer's own orchestrator, **cosmon**, took over
and still drives the code today. Cosmon organizes work as *molecules* (tasks,
deliberations) executed by worker fleets following typed *formulas*; its
governance files live under `.cosmon/` in this repo.

The provenance convention changed with the orchestrator: cosmon workers commit
under the maintainer's git identity, and the audit trail moved into the commit
messages instead — molecule IDs like `(task-20260527-0a96)` and step markers
like `evolve(<deliberation-id>): step 2/4` identify which fleet run produced
which change, and ~35 `feat/task-*` branches mark molecule worktrees.

The finalization of the project for publication — the pre-release reviews, the
premortem deliberations that reshaped the README, the citation audit of the
academic paper — was itself conducted by cosmon fleets. The public traces are
the reports under [`ops/audits/`](../ops/audits/) (for example the
[consolidated citation audit](../ops/audits/references-audit-2026-06-07.md),
synthesized from four parallel verification molecules) and this very document,
written by a cosmon worker under molecule `task-20260610-aa22`.

### How the fleet actually worked

- **One human owns the merge queue.** No agent commit reaches `main` without
  passing the full CI gate (`cargo test`, `clippy -D warnings`, `fmt`,
  `secret-scan`, `deny`, `forbid-strings`, `topology-guard`) *and* a human
  review of the diff. The agents propose; the maintainer disposes. This held
  under both orchestrators.
- **The correctness-critical paths are the most reviewed, not the least.** The
  content-addressable cache, the hash-validation logic, and the atomic-write
  contract (ADR-006) were each landed deliberately, with tests written before
  the implementation (the project's TDD policy is a hard gate — see
  [CLAUDE.md](../CLAUDE.md)).
- **The formal specification is independent of the implementation.** The
  [TLA+ model and the academic paper](paper/oxymake-paper.tex) describe what the
  system *should* do; the test suite checks that the code *does* it. Neither was
  written to flatter the other.

### Why it is in a separate document

The engine's value proposition is "content decides what re-runs, so
`git checkout` no longer rebuilds everything." That claim is testable in five
minutes on your own DAG (`ox translate Snakefile && ox run --dry-run`) and owes
nothing to how the code was authored. Leading with the agent-fleet story would
make the *construction* the subject of the conversation instead of the
*behaviour* — and the behaviour is what you should judge. So the story lives
here, for the curious, after the try.

If you want to audit the cache-correctness logic yourself, start with
`crates/ox-cache/` and the [Output Integrity contract](design/output-integrity.md).
