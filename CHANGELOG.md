# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

**Upgrading invalidates your cache.** A uv environment now contributes the
bytes of the file it declares — including a `pyproject.toml` project file and
its adjacent `uv.lock` — to the cache key, so the key format moves from v5 to
v6: the first run after this upgrade recomputes everything, once.

### Added
- **Verified cross-machine continuation with `cache_platform = "any"`,
  `ox cache-export <targets> [-o <path>]`, and `ox cache-import <manifest>`.**
  Export writes a versioned manifest of output hashes and provenance; after
  the outputs are copied, import re-hashes them before changing the cache and
  lets downstream jobs continue without the producer's raw inputs. The default
  remains `"exact"`; non-reproducible rules, mismatches, and cross-platform
  imports without the explicit opt-in are rejected. Cache entries record the
  producing platform and scope so opted-in artefacts can be audited and
  enumerated for invalidation. OxyMake cannot verify that an opted-in rule's
  outputs are truly platform-independent (#7).

### Fixed
- **`environment = { uv = "pyproject.toml" }` now invalidates outputs when a
  dependency changes.** The project file reference was dropped at parse time,
  so nothing about it entered the cache key: editing `dependencies` left
  `ox run` reporting the rule up-to-date and keeping outputs built with the
  old dependency set — the opposite of what the book promised. The reference
  is kept (`EnvSpec::Uv { project, requirements }`), and the key now hashes
  the project file and, when present, the `uv.lock` beside it. The executor
  still passes no requirements flag for a project file: uv discovers it
  (issue #8).
- **An `environment` table naming no known backend is now rejected.**
  `environment = { type = "uv", requirements = "…" }` — the natural spelling
  to try — was silently dropped: the rule ran on the host, its cache key
  recorded no environment, and `ox lint` reported the workflow as valid.
  Such a table, and any unrecognised key alongside a recognised backend, is
  now a parse error naming the accepted keys (`uv`, `conda`, `docker`, `nix`,
  `apptainer`), surfaced by `ox lint` and by every command that parses the
  workflow. If you relied on an ignored key, remove it or spell the backend
  as its own key (issue #10).
- **`environment = { uv = "requirements.txt" }` now runs.** The local
  executor wrapped the command as `uv run -r <file>`, and `uv run` has no
  `-r` flag, so every rule with a uv requirements file failed with uv's usage
  message before its command started. The flag is now
  `--with-requirements <file>`, on both the shell wrapper and the warm-worker
  argv. The SLURM job script had the same shape (`uv sync -r <file>`, also not
  a uv flag) and now emits `uv pip install -r <file>` (issue #9).

## [0.3.0] - 2026-09-09

An interruption now leaves the ledger describing a state that still exists,
every reader tells apart a job that is running from one that was abandoned,
and a rule can own an incremental cache of an external source.

**Upgrading invalidates your cache.** The per-rule output cleanup policy is
part of a job's identity, so the cache key format moves from v4 to v5: the
first run after this upgrade recomputes everything, once.

Highlights:
- **A Ctrl+C is bounded.** `SIGTERM` escalates to `SIGKILL` after a grace
  period, the force-exit writes before it exits, and a signal during the
  post-failure wait is honoured — so no run leaves a `running` row behind or
  a job process alive (issue #4).
- **`ox status`, `ox top` and the dashboard report orphaned jobs as
  orphaned**, from one read-only derivation shared by every reader: a job is
  running only while its session is alive (issue #5).
- **`clean_outputs = "always" | "on-failure" | "never"`** lets a script own
  its outputs, so an interrupted 4 GB download is not discarded on the next
  run (issue #6).

### Fixed
- **An interrupted `ox run` no longer leaves `running` rows behind** (issue #4).
  Graceful shutdown is now bounded: after `SIGTERM`, the scheduler escalates to
  `SIGKILL` on the job's process group once a grace period elapses (default 10 s,
  overridable with `OX_SHUTDOWN_GRACE_SECS`), so a child that traps or ignores
  `SIGTERM` can no longer hold the run open without bound. The force-exit path
  (second `SIGINT`/`SIGTERM`) now cancels this session's still-`running` rows
  before calling `exit(130)` instead of exiting silently. In-flight jobs of an
  interrupted run are recorded `cancelled` rather than `failed`, and the run
  exits `130`. All writes stay scoped by `session_id`, so a live peer's row is
  never terminalized (ADR-012). `ox cancel` inherits the same contract.
- **Ctrl+C is honoured while a run winds down after a failure.** Without
  `--keep-going`, a failed job stops new dispatches and the jobs already
  running are left to finish; a signal in that phase used to be ignored until
  the force-exit second signal, leaving a `SIGTERM`-deaf sibling alive and its
  row `running`. It now takes the same bounded shutdown path (#4).
- **`ox status`, `ox top` and the dashboard no longer report abandoned jobs as
  running.** A job row left at `running` by a session that was interrupted,
  completed, or that stopped heartbeating is now reported as **orphaned**,
  with its age and the reason — instead of "Sessions: 0 active" printed next
  to "1 running" on the same screen (#5). The rule is the claim protocol's
  own: a job is running only while its session is `active` with a heartbeat
  younger than the lease (90 s, `OX_SESSION_LEASE_SECS` overrides). Readers
  stay read-only — reclaiming still happens at the start of the next
  `ox run`.
- **The dashboard no longer counts an abandoned job's age as time spent
  working.** A rule's `earliest_started_at` in `GET /api/stats/rules` now
  ignores orphaned rows, so the per-rule strip — and the header's elapsed
  clock and throughput, which are derived from it — date a rule from work
  that is actually progressing. The per-rule strip also names the orphans
  instead of leaving them to look like a rule that is merely behind, and the
  timeline's rule-colour legend gained an `orphaned` key: those bars override
  the rule colour, so without it a purple bar read as just another rule.
- The dashboard serves an inline icon, so a page load no longer logs a 404 for
  `/favicon.ico`.

### Added

- **Per-rule `clean_outputs = "always" | "on-failure" | "never"`** (#6). By
  default OxyMake deletes a job's declared outputs before rerunning it, which
  makes an incremental cache of an external source impossible: editing the
  extraction script re-downloads the whole dataset. `"never"` hands the
  outputs to the script — they survive both the rerun and a failure, so a
  transient error partway through does not discard what was already
  fetched — and `"on-failure"` keeps them before the run but still cleans up
  after a failure. Three lifecycles, so the field is not a boolean. Local
  executor only; the default is unchanged, and `"never"` transfers the
  staleness guarantee to the script (see the format reference).
  **Cache keys include the policy (format v5), so upgrading invalidates every
  existing cache entry and the first run after it recomputes.**
- `OX_SHUTDOWN_GRACE_SECS` — seconds a cancelled job may take to exit before
  `ox run` kills it (default `10`; `0` escalates immediately).
- **`ox_state::effective`** — the one read-side derivation every reader
  consumes: `effective_status()`, `EffectiveStatus` (with an `Orphaned {
  since, reason }` variant), `OrphanReason`, `SessionLiveness`,
  `lease_secs_from_env()`, and the `StateDb::job_views` /
  `effective_job_counts` / `pending_job_views` queries that LEFT JOIN `jobs`
  to `sessions` (#5). Documented in
  [ADR-012](docs/adr/012-cooperative-multi-session.md).
- **`ox status --json` gains `jobs.orphaned`**, an `orphaned_jobs` array
  (`reason`, `reason_description`, `declared_status`, `session_id`,
  `elapsed_secs`) and `waiting_for_orphaned` on each pending job; pending
  lines now name an orphaned upstream as such (#5).
- **Dashboard**: `orphaned` count in `GET /api/status` and the SSE stream
  (`running` excludes it), effective `status` plus `declared_status` /
  `orphan_reason` in `GET /api/jobs`, `GET /api/dag` and `GET /api/job/:id`,
  a per-rule `orphaned` in `GET /api/stats/rules`, and a distinct badge and
  card in the UI — the ETA and jobs/sec strip no longer treat orphaned rows
  as in flight (#5).

## [0.2.0] - 2026-09-05

Gates now hold, two sessions never execute the same job twice, and outputs can
be shared through a directory remote cache. This release also carries the paper
revision (arXiv v3) and the corrections it forced on the documentation.

Highlights:
- **Gates are enforced.** `[gate.<name>]` pauses the rules it guards until
  `ox gate approve <name>`; a rejected gate cancels them. Declared but unwired
  in 0.1.0 (issue #2).
- **One execution per job across sessions.** The cooperative claim protocol
  (ADR-012) is the scheduling gate: a second `ox run` waits for its peer's result
  instead of racing it; a crashed owner's lease is reclaimed (issue #3).
- **`ox run --cache-remote <dir>`** — a directory blob store shared between
  checkouts on the same platform.
- **`state.db` schema v10** (migrated in place, serialised): gates carry a name
  and a run id; sessions heartbeat.


### Changed
- **Two `ox run` on the same job execute it once.** The cooperative claim
  protocol of `state.db` (ADR-012) is now the scheduling gate: a job is
  claimed before it is dispatched, and a session that loses the claim
  waits for the owning session's result instead of running the job — or
  failing it. Both sessions report the job done; a peer's failure or
  cancellation is mirrored while that peer is live. Sessions now heartbeat
  (every third of a 90 s lease, `OX_SESSION_LEASE_SECS` overrides); a
  session killed mid-job is taken over by its waiter once the lease has
  expired. `running`, `failed` and `cancelled` rows left in `state.db` by
  sessions that are no longer live are reset when a run starts, and an old
  completion is reset when a job that executes claims it, so a fresh run
  re-evaluates them (a completion recorded by a peer after this run
  started is consumed instead). The
  per-output-path `flock` locks of #2 stay as defence in depth; the
  `ConcurrentExecution` error is only reached when a reclaimed job's
  orphaned shell is still writing (#3).

### Fixed
- A waiting session no longer takes over a live peer's job on a stale
  observation: the reclaim of a dead session's running jobs is one
  `state.db` transaction guarded by the peer's *current* heartbeat
  (`StateDb::reclaim_stale_jobs_if_stale`), so a heartbeat landing between
  the waiter's read and its reclaim keeps the owner's rows and session
  intact. Previously the two steps were separate and both sessions could
  end up executing the job (#3, round-1 QA finding 1). `ox clean` uses the
  same guarded reclaim.
- A warm `ox run` over a large graph no longer pays for the claim
  protocol: the run-start reset of stale rows is one transaction and
  leaves completed rows alone (they are reset lazily when a job that
  executes claims them), so the 999 cache-hit writes of a 1001-job warm
  run are no-ops again. Measured against the direct parent of the change
  on the 1001-job bench fixture, a warm run with one invalidated job is
  now within noise of the parent (#3, round-1 QA finding 4).
- A run that stops waiting on a peer (Ctrl+C) no longer cancels the
  peer's running job in `state.db`; it records its own session as
  `interrupted` at the first signal and closes the session at exit (#3).
- **Gates now block.** A `[gate.<name>]` whose `before` names a rule holds
  that rule's jobs until `ox gate approve <name>`; `ox gate reject <name>`
  cancels them. Previously the gate was parsed and listed but the scheduler
  ran without a gate checker, so guarded rules executed immediately and
  `ox gate list` stayed empty (#2). `ox run` waits while a gate is pending
  (polling every 500 ms) and prints the gate message once. Gated workflows
  are refused on `--executor slurm` / `--executor ray`, which submit the DAG
  without the scheduler. Gate records are scoped to a run: each `ox run`
  that reaches a gate asks for a fresh decision. A run blocked on a gate
  now stops on the first Ctrl+C / SIGTERM (the guarded jobs are cancelled)
  instead of ignoring it until the force-exit second signal.
- `ox gate approve` / `ox gate reject` accept the gate **name** (as the
  documentation always showed) as well as the numeric id from `ox gate list`;
  the listing now shows the gate name, status, run and decider, and `--json`
  emits a JSON array.
- `state.db` schema v10: the `gates` table gains `name` and `run_id`
  (unique per `(name, run_id)`); existing rows are migrated with
  `name = rule_name`. Migration runs automatically on the next open.
- Paper: applied the round-5 outside-seat review (a citation audit of every
  `\cite` against its primary source, and a general referee read). Corrections
  include a quotation that Goble et al. do not contain, the "Goble three-layer
  model" now stated as our own reading of the four workflow forms they
  describe, what the Newcombe et al. table actually reports, petgraph's
  topological sort described as depth-first rather than Kahn's algorithm, the
  Mokhov et al. (2020) section locator, PiGx no longer cited as a CWL
  workflow, the cold-path work attributed to OxyMake alone, and end-to-end
  times described as single runs rather than medians of three. Each is
  recorded with its superseded wording in `docs/paper/ERRATUM.md` §F.
  `bench/snakemake-vs-oxymake/RESULTS.md` now states the binary invocation and
  commit actually used; no measured number changed.
- Paper title finalized as "OxyMake: A Content-Addressed Workflow Engine"
  (dropping "Convergent," and "with Model-Checked State Protocols" from the
  intermediate title, and superseding the original "Formally-Specified,
  Content-Addressable" one). The title and its echoes are aligned across
  `CITATION.cff`, `README.md`, the packaging metadata (npm, PyPI, Homebrew,
  crates.io name-reservation crate), and the rebuilt PDF and arXiv tarball.

### Added
- A `[gate.<name>]` whose `before` or `after` names a rule that the
  Oxymakefile does not define is now a validation **error**: `ox lint` fails
  and `ox run` refuses to start, naming the gate and the unknown rule. A
  misspelled `before` previously attached the gate to nothing and the rule it
  meant to guard ran unapproved (#2). The "gate enforcement is not wired"
  lint warning of the interim release is gone: gates block now.
- `docs/paper/ERRATUM.md` — an append-only record of every paper claim
  corrected after publication, with the superseded wording and the primary
  source for each correction. The paper carries a matching "Revision note
  (erratum)" section in its front matter.
- `ox run --cache-remote <dir>` stores output blobs in and restores missing
  outputs from a shared, content-verifying directory blob store. Remote-cache
  validation is always content-hash based. The backend transports blobs only:
  the local SQLite index that maps computation keys to output paths and hashes
  does not travel, so restoring in a fresh checkout additionally requires that
  local index (a remote computation-key manifest is future work).

### Fixed
- A gate whose ledger record cannot be written no longer opens. The run
  reports that the gate "could not be registered", keeps the guarded jobs
  blocked and retries the registration on every poll; previously the
  registration error was swallowed and the absent record was treated as an
  approval (#2).
- Two `ox run` starting together in a fresh directory no longer race the
  `state.db` schema migration (one of them failed with "duplicate column
  name"): the version is read and all migrations are applied under a single
  write transaction, so the second run waits and finds the schema ready. The
  WAL switch of both `state.db` and the cache manifest, which SQLite does
  not cover with the busy timeout, is retried instead of failing the open
  with "database is locked".
- Two `ox run` approved for the same gate no longer risk committing a mixed
  output set. The claim protocol is not yet the scheduling gate (known
  limitation, ADR-012), so both sessions reach the guarded job; a previous
  release had the losing session *adopt* its peer's committed outputs, which
  for a non-deterministic or multi-output rule could commit a set mixing
  files from two physical executions and report success. That adoption is
  reverted. The local executor now fails closed: it takes an exclusive
  `flock(2)` lock on **each output path** of the job (under
  `.oxymake/locks/`), so the session that wins executes and commits while a
  concurrent session aborts that job with an error naming the holder and the
  locked path (`already being executed by another session (pid N): output
  '<path>' is locked`). Jobs contend as soon as their output sets overlap,
  including through a symlinked directory. The job's subprocess inherits the
  locks, so a session killed mid-job (`kill -9`) does not free them while
  its orphaned job can still write: a replacement run fails closed, naming
  the exited session, until that job exits. **Scope:** the guarantee holds
  on local filesystems with a working `flock(2)`; on NFS and other
  distributed filesystems the lock may not be visible between hosts and `ox`
  cannot detect that. On platforms without `flock` (non-Unix) a job with file
  outputs is refused (`cannot lock the outputs of job`) instead of running
  unprotected. Library users: `ox_core::traits::executor::Workspace::state`
  borrows the private state, and `ox_exec_local::process::spawn_shell_with_callback`
  / `spawn_shell_streaming` take an `inherit_fds` parameter (#2).
- `ox gate reject` confirms with "rejected by", not "rejectd by".
- The terminal progress summary now counts cancelled jobs (gate rejected,
  interrupted) as `N cancelled` instead of folding them into `N skipped`;
  the `run_summary` line of `--report-json` gains a `cancelled` field.
- Paper and docs: corrected a fourth contradicted claim, raised by a review
  panel and verified against the code before any text moved
  (`ops/audits/panel-findings-verification-2026-08.md`, which also records the
  three other panel findings and their verdicts). The paper's local-disk
  requirement on `.oxymake/` was published as if the state directory could be
  placed apart from the project tree — the book's SLURM chapter drew exactly
  that split, `state.db` on local disk and `project/` on NFS/Lustre/GPFS. It
  has no code path: `.oxymake` is a bare relative `PathBuf` at every state,
  cache, log and event site (`crates/ox-cli/src/commands/run.rs:1018`, `:1101`,
  `:1423`, `:1436`; `clean.rs:58`; `invalidate.rs:77`; `status.rs:84`;
  `init.rs:56`), and a rule's inputs and outputs resolve against the same
  working directory, so the two cannot be separated. `OXYMAKE_CACHE_DIR`,
  `OXYMAKE_JOBS`, `OXYMAKE_EXECUTOR` and `OXYMAKE_LOG` were documented in the
  book's configuration reference but appear in no `.rs` file, as does the
  `.oxymake/config.toml` described there as the store of project defaults (only
  `$XDG_CONFIG_HOME/oxymake/config.toml` is read, for `cache_validation` and
  `open_dashboard`). The requirement stands and still discharges
  `StateDbAtomicCommit`; the paper now states it as a constraint on the whole
  workspace and names the missing relocation mechanism as a limitation.
  Recorded in `docs/paper/ERRATUM.md` section D.
- Book reference (`reference/format.md`): the environment section documented
  `[env.NAME]` blocks with `type = "uv"` and a rule-level `env = "NAME"`.
  `RawRule` carries no `deny_unknown_fields` (`crates/ox-format/src/parse.rs:239`),
  so all three were parsed and silently discarded — a rule documented as
  running under `uv` ran without it, and its cache key omitted the environment.
  The supported spelling is `environment = { uv = "requirements.txt" }`, keyed
  by backend (`parse.rs:1277`). Reference corrected, and the SLURM backend's
  user-facing warning string, which advertised the same non-existent
  `type = "apptainer"` form, corrected with it.
- Book SLURM chapter and `README.md`: the chapter claimed OxyMake "automatically
  falls back to Apptainer" on SLURM and showed `apptainer exec <image>` as the
  generated command. `crates/ox-exec-slurm/src/job_script.rs:353,359` emits an
  `OXYMAKE_CONTAINER_CMD="apptainer exec …"` assignment that nothing expands —
  the rule command is appended verbatim (`job_script.rs:104-106`), so the job
  runs uncontained while the cache key still records the container
  (`crates/ox-cache/src/key.rs:185-220`). `nix` is an explicit no-op on SLURM
  and `uv` emits `uv sync` without `uv run`; only `conda` takes effect. The
  local executor wraps all five correctly
  (`crates/ox-exec-local/src/executor.rs:376-441`). Documented as a known
  limitation with a workaround; the backend defect is not fixed here.
- Paper and docs: corrected three claims the repository contradicts, the same
  class of error the CWL review (issue #1) found — this time about OxyMake
  itself. (1) The MCP surface: the paper claimed an agent can call `run`,
  `plan`, `status` and gate approval as tools; `crates/ox-mcp/src/tools.rs`
  registers eight tools (`ox_status`, `ox_plan`, `ox_dag`, `ox_logs`,
  `ox_history`, `ox_lint`, `ox_explain`, `ox_clean`) with no `ox_run` and no
  gate tool — read-only inspection plus a destructive `ox_clean`. Corrected in
  the paper, `AGENTS.md`/`CLAUDE.md`, `ox guide` and `ox-guide(1)`, plus a
  stale `ox_run` hint in `ox-mcp` and a comment naming a nonexistent
  `ox_subscribe` tool. (2) Snakemake translation: "All four translated without
  manual intervention" contradicted `benchmark/snakemake-compat/RESULTS.md`,
  which records 3–5 manual fixes per workflow and "All 4 workflows execute to
  completion after manual fixes"; the paper now names the interventions, cites
  the benchmark directory it actually evaluated, and calls `ox translate` a
  migration aid rather than a drop-in transpiler. (3) The `tags` rule field:
  documented as an array of strings, but `crates/ox-format/src/parse.rs:255`
  declares `BTreeMap<String, String>`, making the array form a hard parse error
  — so the flagship bioinformatics cookbook could not be loaded. All three are
  recorded in `docs/paper/ERRATUM.md` (new section D).
- Book: `concepts/tags.md` rewritten. It documented `ox run --tag`,
  `ox run --exclude-tag` and `ox dag --group-by tag`; none select jobs (no such
  `run` flags exist, and `dag`'s `--group-by` is accepted but unused). Tags are
  now described as they behave: key/value labels surfaced in `ox plan --json`
  and in `job_queued` events, filterable with `ox subscribe --where KEY=VALUE`.
- Cookbooks: both workflows now lint and run to completion (22/22 and 40/40
  jobs), verified end-to-end. Fixed an ambiguous producer between
  `call_variants` and `merge_vcf`, two aggregation rules missing
  `expand = "product"`, a `chromosomes` config key that cannot resolve a
  `{chrom}` wildcard, TOML-escaped `\t`/`\n` breaking an embedded `awk`
  program, a multi-line inline table (invalid TOML), and `asorti` — a GNU awk
  extension absent from the BSD awk on macOS. Stale sample `ox plan`
  transcripts replaced with real output.
- Cache keys now name workflow inputs relative to the workflow root (the
  invocation directory), allowing identical checkouts at different absolute
  paths to reuse cache entries. Key format v4: relative paths are interpreted
  from the root, `.`/`..` components are resolved lexically, and existing
  paths are canonicalized so a path escaping the root — by absolute spelling,
  `..` prefix, or symlink — enters the key as a normalized absolute path.
  Existing caches are cleanly invalidated by the format-version bump.
- Cache documentation now describes the shipped directory remote cache rather
  than unsupported S3 and GCS URLs.
- Paper (revision v3.1): corrected external-system claims about CWL, `cwltool`,
  Nextflow, Snakemake 7, Galaxy, Cromwell, WDL, Ray, Airflow/Argo, Bazel/Buck
  and Nix/Guix against primary sources, and narrowed OxyMake's own delivery
  claims (default `mtime+hash` verification depth, opt-in rather than
  default shared cache, self-contained rather than statically-linked binary,
  Kubernetes executor as planned). The standalone arXiv abstract, which had
  gone stale against the paper, is regenerated from the corrected TeX abstract.
- ADR-018's `cwltool` cache summary now reflects the current implementation,
  including the `(size, mtime)` fallback when no checksum is available.

## [0.1.0] - 2026-06-17

### Added
- Docs: a **Crate Graph** architecture page
  (`docs/book/src/architecture/crate-graph.md`) giving first-time contributors
  a mental model of the ~24 `ox-*` crates — the hexagonal, `ox-core`-centered
  structure, a per-crate role table, and the exact inter-crate edges verified
  against `cargo tree`. Added an **Architecture** section to the book ToC
  (between Concepts and Cookbook), linked it from `README.md`, and added a
  `docs/architecture/README.md` index alongside the existing `boundary.md`.
- `ox guide` and `ox help guide` — a concise operator handbook (orientation +
  pointers to the canonical docs). Shipped as `ox-guide(1)`.
- `oxymake(1)` man page for the alias binary, so `man oxymake` resolves
  (the `oxymake` and `ox` binaries share the same CLI surface).
- Man pages generated from the clap definitions (`just man` →
  `docs/man/*.1`; install with `just man-install`). `ox(1)` and
  `ox-run(1)` document exit codes, the `--json`/NDJSON machine interfaces,
  and the full `--cache-validation` resolution chain.
- `ox --help` and `ox run --help` now document exit codes (0/1/2) and the
  machine-readable interfaces (`--json`, `--report-json`, `ox.lock`).

### Changed
- Launch hygiene (MINOR polish batch): single-sourced the canonical project
  homepage on **`oxymake.dev`** — the domain the docs site actually deploys to.
  `CITATION.cff` (`url`), the packaging publish-metadata (`crates/oxymake`
  Cargo.toml `homepage`, `packaging/pypi` `Homepage`, the npm/crate reservation
  READMEs, `RESERVE-NAMES.md`, `RELEASING.md`'s `gh repo edit --homepage`), and
  the paper's availability URL no longer point at the stale `noogram.org/oxymake`
  third home. The GitHub repository URL (`github.com/noogram/oxymake`) is
  unchanged — it is the source location, not the homepage. The released
  CHANGELOG history entry that records the org transfer is left intact.
- `just ci` now runs `cargo check --workspace` as its first gate
  (`ci: check fmt-check lint test doc demo-ci`), matching CONTRIBUTING's
  "five gates" Definition of Done (build was the one gate the recipe skipped);
  added a `just check` recipe.
- Docs: the Google Cloud SLURM cookbook is renamed **Cloud HPC with SLURM** in
  the nav and page title, and now frames Google Cloud as *one* concrete worked
  example (OxyMake targets any SLURM cluster — on-prem, academic, or cloud)
  rather than a preferred provider. The concrete `gcloud` recipe is unchanged.
- README: the empty `HIBERNATION-BANNER` region is now self-documenting
  (`auto-managed region; empty while the project is awake`) so a first-time
  visitor reading the raw source sees intent, not an orphan comment. The
  START/END markers are kept — the hibernation protocol inserts the banner
  inside that region (see `docs/HIBERNATION.md`).
- README first screen and claim-framing pass for the public launch. The
  best-effort maintenance block moved off the hero (it was the second thing a
  visitor read) down to sit immediately before `## Project Status`, leaving a
  one-line pointer near the top. The Features benchmark bullet now leads with a
  crisp, honest claim — "33× faster DAG *resolution*" (qualified as
  resolution-only; cold end-to-end is slower) with the dense cold/warm/scale
  numbers and the version string left to the linked benchmark of record. The
  "no phantom re-runs" phrasing is scoped to the content-addressed `hash` mode
  (Snakemake 7.32.4 does not phantom-re-run on mtime churn either — see
  `bench/.../RESULTS.md`). The install section now leads with the Cargo paths
  that work today (`cargo install --git … ox-cli` and `--path crates/ox-cli`)
  and clearly marks Homebrew/PyPI/prebuilt-release binaries as available only
  from the first tagged release; `cargo install oxymake` is documented as a
  name-reservation placeholder (library-only, no binary), not an install path.
- Agent onboarding: scrubbed the maintainer-private cosmon surface from the
  public agent-facing entrypoints so a stranger's agent is no longer routed to
  tooling it lacks. `AGENTS.md` (and its `CLAUDE.md` symlink) now gates the
  Cosmon section behind an explicit "(maintainer-only; external contributors
  can ignore)" marker, makes the public chain (CONTRIBUTING.md → `ox help` /
  `ox guide` → `ox serve --mcp`) self-sufficient, and documents that the only
  tracked `.cosmon/` file (`artifact-map.toml`, a curated CI input per ADR-017)
  is intentional while `.cosmon/state/` is local-only. The `ox guide` handbook
  tail and the `Guide` doc-comment no longer end on `cs help` / `cs help guide`.
- Docs: the machine-facing reference pointer now resolves to
  `docs/book/src/reference/` everywhere. `CONTRIBUTING.md`, `STATUS.md`,
  and `docs/AUDIT-REPORT.md` previously pointed contributors at a
  `docs/agent/` tree that is not part of the public repo; those five
  references and the two stale "Present" audit rows now point at the
  book's reference chapter, which is the canonical machine-facing reference.
- `ox run --cache-validation` help now spells out the strategies and the
  resolution order (flag → `OX_CACHE_VALIDATION` → Oxymakefile `[config]`
  → `~/.config/oxymake/config.toml` → default `mtime+hash`).
- Requesting a target that no rule produces and that does not exist on disk
  now fails with an actionable message ("add a rule whose `output` matches
  it, or create it as a source file") instead of the bare "no rule produces
  output matching" error.

### Removed
- `ox run --where` and `ox run --materialize`: these flags were parsed but
  never wired to any behavior (silent no-ops). Use a target or `--rule` to
  filter, and the per-output `materialize` field in the Oxymakefile.

### Fixed
- README: the license badge linked only to `LICENSE-MIT` despite reading
  "MIT/Apache-2.0"; it now points at the `## License` section (both
  `LICENSE-MIT` and `LICENSE-APACHE` ship). The paper threat-model citation
  named a wrong, non-existent section number (`§3.1`); it now uses a stable
  named reference (the threat-model subsection) since the paper ships as
  `.tex` with no numbered PDF. The "Coming from Snakemake?" headline figure
  (`64 ms`) is corrected to `69 ms` to match the benchmark of record.
- Book: the flagship `getting-started/first-workflow.md` tutorial crashed on
  the first `ox run` a newcomer issues. Its Python `run` block wrote a dict as
  `stats = {{ ... }}`; OxyMake's interpolation does not unescape `{{`/`}}`, so
  the literal braces produced a Python set-of-dict and `TypeError: unhashable
  type: 'dict'`. Uses a plain dict literal (`{ ... }`) now and runs green
  end-to-end. A note documents that only recognized `{…}` placeholders are
  substituted and `{{`/`}}` are not escapes.
- Book: replaced invented CLI transcripts (`[1/3] … done`, `3/3 jobs completed
  successfully`, `Critical path: …`, `0 jobs to run`) with real binary output
  (`Completed: N succeeded, N failed, N skipped, N cancelled`, `Plan: N rules,
  N jobs, N source files`) across `output.md`, `first-workflow.md`,
  `reference/commands.md`, `concepts/{three-graphs,idempotent-execution,
  ray-integration,slurm-integration}.md`, and the bioinformatics/climate
  cookbooks. The JSON event examples now use the real `"event"`/`"job_id"`
  schema. Reference fixes: `--executor` lists `local`/`slurm`/`ray` (no `k8s`),
  exit codes are `0`/`1`/`2`, `ox query` operates on job-ids, and the removed
  `--tag`/`--where`/`--materialize` flags are gone from examples.
- Book: `installation.md` showed `oxymake 0.1.0` and `Created Oxymakefile.toml`;
  corrected to the real `ox 0.1.0` and `Initialized OxyMake project in .`.
- ADR index trinity reconciled: disk (17 ADRs), `docs/adr/README.md`, and
  `docs/adr/STATE.md` disagreed on the ADR set. ADR-016 and ADR-017 were on
  disk but missing from the README index, and `STATE.md` stopped at 015; all
  three now agree on 001–017. ADR-011's index title was realigned to its body
  ("Three-Stage State Pipeline"; the filename keeps the historical
  `three-layer` slug per the no-rename rule, noted inline).

### Docs / CI
- ADR `STATE.md` drift is now gated in CI: a new `adr-state` job regenerates
  `scripts/adr-lint.py --emit-state` and fails the build if the committed
  `docs/adr/STATE.md` is stale, so the index projection cannot silently drift.
- `getting-started/quickstart.md` (the most accurate getting-started page) is
  now listed in `SUMMARY.md`, so mdBook renders it.
- `mdbook-mermaid` is wired into `docs/book/book.toml` (preprocessor + JS/CSS
  assets), so ```mermaid``` fences render as diagrams instead of raw code. The
  docs-deploy workflow installs `mdbook-mermaid` 0.14.x (the version that
  targets mdBook 0.4.x). Relabeled `csv`/`jsonl`/`ssh-config` fences to
  `text`/`json` to silence highlight.js "unknown language" warnings.
- Added a golden-file transcript test (`ox-cli/tests/doc_transcript_golden.rs`,
  runs under `cargo test --workspace`) that pins the documented `ox plan`,
  `ox run`, `ox run --json`, `ox --version`, and `ox init` output formats, so
  doc/binary drift fails CI instead of shipping.

## [0.1.0-alpha] - 2026-06-02

First public release. OxyMake is a formally-specified, content-addressable
workflow engine shipped as a single static `ox` binary: it keeps Snakemake's
rule model but replaces the mtime heuristic with a BLAKE3 content-addressable
cache key, adds daemon-free cooperative multi-session execution, and specifies
its cross-session safety properties in TLA+. Prebuilt binaries for Linux and
macOS are attached to this release; the `oxymake` name is reserved on crates.io
(the binary ships via GitHub Releases — see RELEASING.md).

### Changed
- Repository moved to `github.com/noogram/oxymake` and the project home to
  `noogram.org/oxymake` (org transfer). Update clone/remote URLs accordingly.
  Maintainer contact is now `emmanuel@serie-research.dev` (Independent
  researcher).

### Added
- `STATUS.md` — per-surface stability declarations for the seven public
  surfaces (CLI, `Oxymakefile.toml`, `.oxymake/state.db`, NDJSON event
  stream, plugin Rule API, environment variables, `ox.lock`). Documents
  what is stable, what is unstable, and the SemVer contract for each.
  Per the §M8 public-contracts decision (2026-05-27, Q6 robustesse contracts publics).
- `CONTRIBUTING.md` — contributor guide referencing `STATUS.md`, the
  definition of done, TDD policy, and CHANGELOG discipline.
- `docs/format/env-vars.md` — canonical reference for environment
  variables OxyMake reads and sets. Declares the stability tier of each.
- `format_version` top-level field in `Oxymakefile.toml`. Optional today
  with default `"1"`. Versions the Oxymakefile schema independently of
  the `ox` binary version. Exposed as `Workflow::format_version` and
  `ox_format::parse::DEFAULT_FORMAT_VERSION` in the `ox-format` crate.
  The `ox init` starter template now writes it.
- Project scaffolding: Cargo workspace with 14 crates
- Founding thesis document (OXYMAKE-THESIS.md)
