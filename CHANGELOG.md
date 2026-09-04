# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- **Gates now block.** A `[gate.<name>]` whose `before` names a rule holds
  that rule's jobs until `ox gate approve <name>`; `ox gate reject <name>`
  cancels them. Previously the gate was parsed and listed but the scheduler
  ran without a gate checker, so guarded rules executed immediately and
  `ox gate list` stayed empty (#2). `ox run` waits while a gate is pending
  (polling every 500 ms) and prints the gate message once. Gated workflows
  are refused on `--executor slurm` / `--executor ray`, which submit the DAG
  without the scheduler. Gate records are scoped to a run: each `ox run`
  that reaches a gate asks for a fresh decision.
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
