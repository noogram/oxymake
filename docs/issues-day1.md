# Day-1 issues — ready to file after the public flip

Companion to `ops/audits/finalization-mission-2026-06-10.md` (§4). Each entry is a
GitHub issue ready to create verbatim (`gh issue create --title … --body …`).
Severity scale: S1 blocks the canonical path · S2 an early adopter will hit it ·
S3 sharp edge · S4 cosmetic. Sources: M7 virgin-quickstart, M9 doc-premortem,
chantier descopes, and the umbrella issues that honor the conscious-acceptance
clause of the 2026-06-09 audit (§8.C: "traced as public issues from day 1").

---

## 1. `ox query` unusable on a built project

**Severity:** S2 · **Source:** M7-F7
**Where:** `crates/ox-core/src/resolver.rs` (backward chaining), `crates/ox-cli/src/commands/query.rs`

On a project whose outputs already exist, the resolver backward-chains to nothing,
the job graph is empty, and every query — `deps(all)`, `rdeps(process)`, even a
concrete `deps(process-A)` — returns "no job found". The error suggests `ox plan`,
which simultaneously shows "0 jobs": circular guidance. Queries only work in the
window between deleting outputs and the next run. This does not honor the
"Bazel-style dependency queries" promise (Bazel queries the static graph regardless
of build state). Proposal: resolve queries against the full static graph (ignore
up-to-date pruning), or add `--include-cached`.

## 2. `--json` stdout purity broken on the all-cached fast path

**Severity:** S2 · **Source:** M7-F8
**Where:** `crates/ox-cli/src/commands/run.rs` (cache fast-path summary print)

`ox run --json` on a fully cached project prints the human line
`Cache: 3 of 3 job(s) up-to-date, skipping.` on **stdout**, breaking any NDJSON
consumer (`jq` dies on it). The forced-run path is pure NDJSON (verified). Route
the line to stderr or emit it as a JSON event.

## 3. `ox top` panics with a raw backtrace when stdout is not a TTY

**Severity:** S3 · **Source:** M7-F9
**Where:** `crates/ox-cli/src/commands/top.rs` (ratatui init)

`ox top` in a pipe/CI context panics ("Device not configured") instead of a
graceful "requires a terminal" message with exit 1.

## 4. `ox run --forcerun <unknown-rule>` silently no-ops

**Severity:** S3 · **Source:** M7-F10
**Where:** `crates/ox-cli/src/commands/run.rs` (forcerun filter)

A typo in `--forcerun` exits 0 having re-run nothing — unlike `--until`, which
errors on unknown targets. Validate the rule/target name and error out.

## 5. `examples/demo/run-demo.sh` does not fail fast on a bad `OX` path

**Severity:** S3 · **Source:** M7-F12
**Where:** `examples/demo/run-demo.sh`

With a wrong `OX` env var the script continues with mangled commands (e.g.
`/oxymake init` after a failed `cd`). Add `set -euo pipefail` and an upfront
`command -v "$OX"` check.

## 6. `ox translate` exits 2 on success-with-escalations

**Severity:** S4 · **Source:** M7-F13
**Where:** `crates/ox-cli/src/commands/translate.rs`

A successful translation that produced escalations exits 2, breaking `&&`-chains.
Either document this as the contract (escalations = review required) or move to
exit 0 + summary on stderr. Decide and align STATUS.md.

## 7. `ox export snakemake` emits empty `shell: ""` for input-only rules

**Severity:** S4 · **Source:** M7-F14
**Where:** `crates/ox-cli/src/commands/export.rs` / ox-translate generate path

An input-only aggregation rule (like `rule all`) exports with an empty shell
stanza — valid but ugly Snakemake. Omit the directive instead.

## 8. Book quickstart output drift: BSD `wc -c` left-pads counts

**Severity:** S4 · **Source:** M7-F15
**Where:** `docs/book` quickstart (byte-count example)

The book shows `12`, macOS prints `      12` (BSD wc padding). Pipe through
`tr -d ' '` in the example or note the platform difference.

## 9. `ox init` scaffold is not runnable out of the box

**Severity:** S2 (product decision) · **Source:** M9 descope #1
**Where:** `crates/ox-cli/src/commands/init.rs` template

`ox init && ox run` fails because the template references `data/{sample}.txt`
inputs that are not scaffolded. The error now carries a remedy (MissingSource fix,
M9), but the ideal first-run experience would scaffold a minimal `data/` example
so the very first `ox run` succeeds. Decision needed: does the template teach the
format or does it run?

## 10. Structured JSON error envelope on stderr under `--json`

**Severity:** S2 (needs ADR) · **Source:** M9 descope #2
**Where:** `crates/ox-cli/src/lib.rs` (`eprintln!("error: {e:#}")` path)

In `--json` mode fatal errors still reach stderr as free text while events are
NDJSON on stdout. An agent driving `ox` must parse two formats. Proposal: when
`--json` is active, emit a final structured error object (stderr or stdout
sentinel event). Surface-contract change → write an ADR first.

## 11. `ox status --json` with no state: exit 0 vs dedicated code

**Severity:** S4 · **Source:** M9 descope #3

Current choice: `{"state":"absent",…}` + exit 0 (absence is not an error).
Revisit if agent consumers prefer a distinct signal. Document the decision either way.

## 12. Per-subcommand "Exit codes" help section

**Severity:** S4 · **Source:** M9 descope #4

Only root `ox --help` and `ox run --help` document the 0/1/2 contract. Add a
short EXIT CODES section to the remaining subcommands' long help.

## 13. `cargo clippy --workspace --all-targets` is not clean

**Severity:** S3 (CI hardening) · **Source:** F2 chantier note
**Where:** `crates/ox-core/benches/scheduler_micro.rs`, `crates/ox-core/tests/stress_scheduler.rs` (and friends)

The DoD gate runs clippy without `--all-targets`; benches and stress/proptest
targets carry pre-existing lints (unused imports, `needless_range_loop`, …).
~30 min sweep, then harden the CI gate to `--all-targets`.

## 14. Benchmark `render_results.py`: stale column label after default flip

**Severity:** S4 · **Source:** M1 vigilance note
**Where:** `bench/snakemake-vs-oxymake/render_results.py`, RESULTS.md churn table

The churn-test column is labeled "OxyMake (mtime, default)" but the shipping
default is now `mtime+hash` (cfeb). Next bench regeneration should rename the
label to "OxyMake (mtime)" and add the default marker to the right mode.

## 15. Book documents `ox clean --cache`; the real flag is `--cache-only`

**Severity:** S4 · **Source:** bd89 residue note
**Where:** `docs/book/src/reference/commands.md`

Pre-existing doc drift spotted during the state-robustness pass; align the book
with the actual clap definition.

## 16. Re-run the benchmark suite on Linux/x86_64

**Severity:** S2 (paper commitment) · **Source:** audit PM#4, paper §6 header

All published numbers are single-platform (M4 Max, Darwin). The paper announces a
pending Linux/x86_64 re-run in the abstract and §6. Run
`bench/snakemake-vs-oxymake/` on a Linux x86_64 host, commit RESULTS.md alongside
the macOS record, and update the paper's pending notes.

## 17. Paper §5.2: cold-run causal explanation may overstate mtime fast-path work

**Severity:** S4 (paper chip) · **Source:** 055a residual note

§5.2 explains cold-run cost as "hashes every rule's source … into a BLAKE3 cache
key", which may overstate what the mtime fast-path computes on a cold run.
Re-derive the sentence from the actual code path at next paper revision.

## 18. Production callers should prefer `spawn_disk_writer_confined`

**Severity:** S3 (guardrail) · **Source:** F1a note
**Where:** `crates/ox-core/src/disk_writer.rs`

The unconfined `spawn_disk_writer` remains exposed for workspace-less callers
(scheduler tests). Any new production call site must use the confined variant
(H13 traversal defense). Consider `#[doc(hidden)]` or a lint to make the safe
default the obvious one.

---

# FAIR roadmap — v1.1 export commitments

Scoped commitments from the FAIR alignment review (`docs/FAIR-ALIGNMENT.md`),
ready to file as enhancement issues at the public flip. Forward-compatibility is
confirmed: the v1 frozen surfaces (`state.db`, NDJSON events, `ox.lock`) can emit
all of these **additively, without a breaking change** (audit: molecule
`task-20260614-a814`). Severity `enhancement` throughout — none blocks the
release; export is the public activity of the v1.1 cycle.

## R1. `ox export --ro-crate` — Workflow Run RO-Crate export

**Severity:** enhancement · **Source:** FAIR-ALIGNMENT roadmap #1
**Where:** new `crates/ox-cli/src/commands/export.rs` path; reads `crates/ox-state` (`job_history`) + `crates/ox-lock`

Emit a Workflow Run RO-Crate — workflow definition + declared inputs/outputs +
provenance bundle — from the durable record. This is the standard self-describing
crate that the FAIR-computational-workflows community consumes, and the
highest-leverage FAIR artefact. Reads existing surfaces only: `state.db.job_history`
(input/output hashes, wall-clock times, executor, host, session) and `ox.lock`
(workflow identity, per-rule inputs/outputs, env pinning, platform). No frozen
surface changes.

## R2. W3C PROV serialisation on the `--json` sink

**Severity:** enhancement · **Source:** FAIR-ALIGNMENT roadmap #2
**Where:** `crates/ox-report-json` (sink) + `crates/ox-state` (`job_history` as PROV source)

Map the run record to the W3C PROV data model: entities (artefacts ↔ content
hash), activities (job executions ↔ wall-clock start/end), agents (session /
executor / host), and the `used` / `wasGeneratedBy` / `wasAssociatedWith` edges.
The durable source is `state.db.job_history` (the live event stream gains absolute
timestamps + output hashes additively if it must become a standalone PROV source —
events carry no `run_id` today). Natural graft on the existing `--json` sink.

## R3. CWL import/export interop

**Severity:** enhancement · **Source:** FAIR-ALIGNMENT roadmap #3
**Where:** `crates/ox-translate` (alongside the existing Snakemake import path)

OxyMake reads Snakemake in today; add Common Workflow Language (CWL)
import/export for interoperability symmetry. Heavier than R1/R2 (CWL's type and
requirement model is richer than the Snakefile surface we translate), so scoped
after the RO-Crate + PROV work within the v1.1 cycle.

---

# Umbrella issues — the audit's accepted MEDIUM/LOW families

The 2026-06-09 audit (§7) consciously accepted ~41 MEDIUM + 22 LOW findings with
the clause "traced as public issues from day 1". One umbrella per family; the
detailed per-finding evidence lives in the five review molecules' reports.

## U1. Parser: silent fallbacks instead of rejection (~10 MEDIUM)

Invalid `timeout` → no timeout; `retry=-1` → 4G retries; typo'd
`error_strategy`/`backoff`/`expand` → silent default (`expand="Zip"` → cartesian
product instead of zip); unknown `[environment]` key → no env; `input` table
without `path` → empty pattern; quadratic dedup on 500k-row CSV. The parser knows
how to reject cleanly (lifecycle/materialize do) — apply the same pattern everywhere.

## U2. Swallowed errors (~12 MEDIUM)

`invalidate()` reports success on failed DELETE; EventSink session-create failure
→ all state writes silently fail (FK); migration read error treated as fresh DB;
unreadable env file → `spec_hash=None` without warning; NDJSON drops events on
broken pipe without a sentinel.

## U3. Secondary concurrency/state defects (~8 MEDIUM)

`register_jobs` reassigns foreign run_ids; `cancel_jobs` over-reports; sync
sessions leak on every `ox status`; `finalize_job_history` not idempotent;
in-memory eviction without persistence ack; concurrent migration not serialized.

## U4. Executor MEDIUMs (~7)

Warm pool serialized behind a single Mutex (no call-mode parallelism); stdin
write timeout leaves a corrupted worker pooled; `job.timeout` ignored by
SLURM/Ray polling loops; unquoted `cd {project_dir}`; Python injection via Ray
driver script path; no `env_clear` (host shell secrets inherited by jobs).
The last two deserve a security-labeled pass.

## U5. Public-surface MEDIUMs (~9)

No-op format validations (`check_output_wildcards`); `--set` without `=` ignored;
`format!`-built JSON in `test`/`gate` (incomplete escaping); MCP log-path
traversal via forged job_id; `ox_plan` MCP hardcodes `cached: 0`; dashboard XSS
latent in `status` field; ox-api discovery depth-5 hardcode + stale mtime cache.

## U6. Translator MEDIUMs (~6) + LOW sweep (22)

Non-quote-aware splits (`sort -k1,2`); Snakemake export re-quoting without
escaping `"""`; `message:` dropped though announced mapped; global
`wildcard_constraints` dropped as Info; named `sample`/`zip` ports skipped;
nested YAML lost. Plus the 22 LOW cosmetics from the audit annex (db.rs header
drift, `--port` panic, metrics usize underflow, …).
