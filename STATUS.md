# OxyMake — Surface Status

> **Honest minimum.** This file declares, per public surface, what is **stable**
> (we will break it only with a major-version bump and a deprecation window)
> and what is **unstable** (it may change shape, name, or disappear between any
> two pre-1.0 releases).
>
> A reader who knows what is unstable trusts the project **more**, not less.

**Project release stage:** pre-1.0 (`0.x`). Until `1.0`, the
[Cargo / SemVer convention](https://doc.rust-lang.org/cargo/reference/semver.html)
treats every `0.X.Y → 0.(X+1).0` bump as potentially breaking. We use that
window to consolidate the unstable surfaces below.

**Last reviewed:** 2026-05-27 (per the §M8 public-contracts decision).
Re-read every 8 weeks or at every minor-version bump, whichever comes first.

---

## The seven public surfaces

| # | Surface | Status | Stability tier |
|---|---------|--------|----------------|
| 1 | `ox` CLI subcommands and flags | partial | mixed — see below |
| 2 | `Oxymakefile.toml` schema | partial | mixed — see below |
| 3 | `.oxymake/state.db` (SQLite schema) | unstable | internal — accessed only via `ox` |
| 4 | `--json` / NDJSON event stream | partial | event **names** stable; payload fields unstable |
| 5 | Plugin Rule API (extension traits) | unstable | source-level only; SemVer not yet enforced |
| 6 | Environment variables (`OX_*`) | unstable | only `OX_CACHE_VALIDATION` documented today |
| 7 | `ox.lock` reproducibility lockfile | unstable | `schema_version = 1`, no rollover policy yet |

Each surface is detailed below. **Anything not explicitly listed under
"Stable today" should be considered unstable**, even if it appears in
`--help`, in the book, or in a code snippet.

---

## 1. `ox` CLI subcommands and flags

The `ox` binary exposes 25 subcommands today (see `crates/ox-cli/src/lib.rs`).

### Stable today

- **`ox run`** — core invocation. The following flags are **stable**:
  - `<target>` positional argument
  - `--jobs / -j`
  - `--report-json <path>`
  - `--executor <name>` (the *flag*; backends themselves are tiered separately)
  - `--profile <name>`
  - `--keep-going / -k`
  - `--cache-validation <strategy>` (values: `mtime`, `mtime+hash`, `hash`;
    default `mtime+hash`, content-verifying — ADR-006 amendment)
  - `--no-cache`

  One `ox run` flag is shipped but **unstable**: `--cache-remote <dir>`
  stores output blobs in and restores missing outputs from a shared
  directory, forcing content-hash validation. It is a blob transport: the
  local SQLite index mapping computation keys to outputs does not travel,
  so a fresh checkout cannot restore from the shared directory alone. Its
  semantics will evolve when a remote computation-key manifest lands.
- **`ox init`** — `ox init [dir] [--force]`.
- **`ox plan`**, **`ox status`**, **`ox lint`**, **`ox explain`**,
  **`ox query`**, **`ox logs`**, **`ox history`** — *names* and *exit codes*
  are stable. The shape of their human-readable output is **unstable**
  (use `--report-json` / structured outputs where available).
- **`ox lock`** — name and exit codes stable. Lockfile schema is unstable
  (see §7).
- **`ox cancel`**, **`ox invalidate`**, **`ox clean`** — names and the
  fact that they mutate state stable; specific flags unstable.
- Global flags: `--color {auto,always,never}` is stable. `--version` is
  stable (Cargo guarantee).

### Unstable

- **All other subcommands**: `dag`, `snapshot`, `gate`, `serve`,
  `subscribe`, `top`, `dashboard`, `test`, `check-consistency`,
  `translate`, `export`, `logo`. These exist for exploration and may
  be renamed, restructured, or removed before `1.0`.
- The textual output format of every subcommand (column order,
  emoji, summaries) is **unstable** by default. Use `--json` /
  `--report-json` when machine-parsing.
- Exit codes: `0` (success), `1` (runtime error or job failure), and `2`
  (command-line usage error, the clap convention) are stable. Any other
  exit code is unstable.

### Semver contract

Removing or renaming any *stable* subcommand or flag requires a major
version bump (post-1.0) or a minor bump (pre-1.0) accompanied by an
entry in `CHANGELOG.md`. Adding new subcommands or new optional flags
is always safe.

---

## 2. `Oxymakefile.toml` schema

The Oxymakefile is the file users edit. It is the **most load-bearing
public surface** and the one we are most conservative with.

### Stable today

The following documented fields will not change shape without a minor
bump and a `CHANGELOG.md` entry:

- Top-level: `ox_version`, `format_version`, `include`.
- `[config]` — list, scalar, and `{ source, key, columns }` file-source
  forms documented in `docs/book/src/reference/format.md` (Config Section).
- `[rule.<name>]` — fields documented in `docs/book/src/reference/format.md` (Rule Definitions):
  `input`, `output`, `shell`, `run`, `script`, `call`, `lang`, `tags`,
  `params`, `param_files`, `resources`, `environment`, `when`, `expand`,
  `error_strategy`, `timeout`, `executor`, `priority`, `description`,
  `benchmark`, `retries`, `wildcard_constraints`, `log`,
  `shell_executable`, `reproducibility`.
- `[gate.<name>]` — `after`, `before`, `message`.
- `[profile.<name>]` — flag-overrides documented in the book.
- `[environment]` (global) — `uv`, `conda`, `docker`, `nix`, `apptainer`.
- `[executor.slurm]` — fields documented in `docs/book/src/concepts/slurm-integration.md`.

### Unstable

- Any TOML field not in the list above is **experimental** and may be
  removed without a deprecation window.
- The exact set of `error_strategy.backoff` values beyond `constant`,
  `linear`, `exponential` is unstable.
- `reproducibility` class values (`deterministic`, `seed_deterministic`,
  `approximate`, `non_reproducible`) — the names are stable; the
  *semantics* attached to each class are still being calibrated.

### `format_version`

The top-level `format_version = "1"` field, added in the
2026-05-27 public-contracts review, is **optional today** and **required after
`format_version` reaches `"2"`**. Migration policy:

- Files without `format_version` are read as `format_version = "1"`.
- A future `format_version = "2"` will only be introduced alongside a
  migration tool (`ox lint --migrate` or `ox format --upgrade`,
  TBD).
- We will never silently re-interpret an existing field; breaking
  changes flip the `format_version` instead.

### Semver contract

Removing a *stable* field or changing its meaning requires a minor
version bump (pre-1.0) plus a `CHANGELOG.md` entry under **Breaking
changes**. Renaming is treated as removal + addition.

---

## 3. `.oxymake/state.db` (SQLite schema)

The state database is an **internal artifact** of the `ox` process. It
holds run history, job status, and cache validation metadata. Users
should not query it directly.

- Schema version: `LATEST_VERSION = 9` (see
  `crates/ox-state/src/migration.rs`).
- Migrations: forward-only, transactional, run on open.
- **Public contract:** *opening a state DB written by an older `ox`
  binary either succeeds with an in-place migration, or fails cleanly
  with a clear error*. We do not promise downgrade.

### Stable today

- The fact that `.oxymake/state.db` exists at that path under the
  workflow root.
- The forward-migration guarantee above.

### Unstable

- Table names, column names, indexes, triggers — all internal. Tools
  that parse `state.db` outside of `ox` will break and we will not
  treat that as a regression.
- The current schema version (`9`) will keep climbing.

---

## 4. `--json` / NDJSON event stream

`ox run --report-json <path>` and the `JsonReporter` write one JSON
object per line (NDJSON). The schema lives in
`crates/ox-report-json/src/schema.rs`.

### Stable today (event names — the discriminator `"event"` field)

- `run_started`
- `job_queued`
- `job_started`
- `job_completed`
- `job_failed`
- `job_skipped`
- `gate_reached`
- `gate_approved`
- `run_completed`
- `run_failed`
- `run_summary` (terminal event)

These eleven event names will not be removed or renamed without a minor
bump and a `CHANGELOG.md` entry.

### Stable today (required payload fields)

- Every event has an `"event"` field (string).
- `job_*` events always carry `"job_id"` (string).
- `run_completed` and `run_summary` always carry `total`, `succeeded`,
  `failed`, `skipped`, `duration_ms`.

### Unstable

- All other payload fields are unstable in name and shape. New fields
  may be added at any time; consumers must ignore unknown keys.
- The order of events within a run beyond the obvious causal order
  (`run_started` first, `run_summary` last) is unstable.

### Forward compatibility

Consumers should:

1. Match on `"event"` and ignore unknown event names.
2. Ignore unknown fields.
3. Treat the absence of an optional field as "not applicable", not as
   an error.

A future `"schema_version"` field at the top of each event is under
consideration but not yet shipped — track this in `CHANGELOG.md`.

**FAIR forward-compat (audited 2026-06-14).** Emitting W3C PROV / Workflow
RO-Crate in a future release (v1.1) is **additive, non-breaking** against this
frozen surface: the PROV ingredients the stream lacks (absolute wall-clock
`started_at`/`ended_at`, output content hashes, `run_id`) are already persisted
in `state.db.job_history`, and new event fields may be added at any time (no
`deny_unknown_fields`). The "additive = non-breaking" property is now guarded by
`forward_compat_tests` in `ox-report-json` and `ox-lock`. No pre-freeze contract
change was required — see the audit `fair-forward-compat.md`.

---

## 5. Plugin Rule API (extension traits)

The extension surface lives in `crates/ox-core/src/traits/`. Today it
exposes the following public traits:

- `CacheCheck`, `Storage`, `FormatCodec`, `Reporter`, `Executor`,
  `EnvironmentProvider`, `GateCheck`, `RemoteCache`,
  `BenchmarkSink`, `MaterializationStrategy`, `StateBackend`,
  `OptimizationPass`.

### Status

**All of these are unstable.** OxyMake does not yet load external
plugins at runtime — extension happens at compile time, by depending
on `ox-core` and providing an implementation. We will revisit SemVer
discipline on these traits once at least one out-of-tree implementation
exists and has been used in anger.

### What we will not do silently

- We will not change a trait's method signature without a
  `CHANGELOG.md` entry.
- We will not remove a documented trait without a deprecation note in
  the previous release.

### What we may do without ceremony

- Add new methods to a trait (with default implementations).
- Add new traits.
- Reorganize the module path of a trait.

---

## 6. Environment variables (`OX_*`)

OxyMake reads a small, documented set of environment variables. The
canonical list lives in `docs/format/env-vars.md`.

### Stable today

- `OX_CACHE_VALIDATION` — cache validation strategy (values: `mtime`,
  `mtime+hash`, `hash`). Overridden by `--cache-validation`; overrides
  the Oxymakefile `[config]` and `~/.config/oxymake/config.toml`.
- `OX_WC_<wildcard>` — set by executors inside a job's environment to
  expose the resolved wildcard value to scripts.
- `OX_JOB_ID` — set by executors inside a job's environment.

### Honoured external conventions (stable)

- `NO_COLOR` — disables ANSI colour output.
- `TERM=dumb` — disables ANSI colour output.
- `CI` — used for colour and verbosity heuristics.

### Unstable

- Anything else prefixed with `OX_` not in this list. If you read or
  set it, expect it to change.

### Semver contract

Renaming or removing a stable `OX_*` variable requires a minor bump and
a `CHANGELOG.md` entry under **Breaking changes**.

---

## 7. `ox.lock` reproducibility lockfile

`ox lock` writes a `ox.lock` file capturing rule versions, environment
specs, input hashes, and platform. The format is defined in
`crates/ox-lock/src/model.rs`.

### Stable today

- The file is TOML.
- Top-level `schema_version` (currently `1`).
- The fact that `ox lock --verify` succeeds against a lockfile written
  by the same `ox` minor version.

### Unstable

- Field names and the table layout. Treat `ox.lock` as opaque — read
  and write it only via `ox lock`.
- The rollover policy when `schema_version` increments (we will not
  blindly migrate user-checked-in lockfiles; the exact UX is TBD).

### Roadmap

The lockfile rollover policy is the open question that gates `ox.lock`
moving from *unstable* to *stable*. A draft proposal:

1. Bumping `schema_version` requires a minor version bump.
2. The old `ox` binary refuses to read a newer-version lockfile with a
   clear error pointing to upgrade.
3. The new `ox` binary reads old lockfiles for at least one minor
   version after the bump, then drops support.

This is **not** yet enforced. Track this in `CHANGELOG.md`.

---

## Cross-cutting principles

- **Documentation is the spec.** If a field, flag, or event is not
  documented in the book or in the relevant `docs/book/src/reference/`
  page, it is unstable by default — regardless of what `--help`
  prints.
- **`CHANGELOG.md` is the audit trail.** Every change to a *stable*
  surface requires a `CHANGELOG.md` entry under
  `[Unreleased] / Breaking changes` (for removal or shape change) or
  `[Unreleased] / Added` (for new optional surface).
- **Pre-1.0 honesty.** We will not pretend to honour SemVer harder
  than we actually do. If we ship a breaking change to an unstable
  surface, we say so in the changelog; we do not retroactively call
  it stable.
- **Self-applied falsifier.** The project polices its own discipline
  with an *exogenous* referee:
  [`.github/workflows/drift-tripwire.yml`](.github/workflows/drift-tripwire.yml)
  turns the GitHub build **red** if the TLA+/Rust line ratio drops
  below the ADR-015 floor of `0.44 %`. It addresses the *self-refereed
  discipline* hidden assumption named in pre-mortem #3: a promise a
  project polices for itself is waivable in silence. This tripwire is
  deliberately **non-waivable** — there is no skip env var and no
  `continue-on-error`; silencing it requires editing the workflow in a
  visible, attributable commit. It measures **content drift only**
  (spec vs. code, at each push): per the operator decision of
  2026-06-10 (premortem PM#5), no calendar-based check — staleness
  timer or deadline — may redden the badge. Spec liveness is reviewed
  at release checkpoints (`spec/tla/README.md`), not by a clock.

---

## Adding a new public surface

When introducing a new public surface (a new subcommand, a new TOML
field, a new event, a new env var):

1. Pick a tier: **stable** or **unstable**. Default to **unstable**.
2. Document it in the corresponding reference page.
3. Add it to this file under the relevant section.
4. Add an entry to `CHANGELOG.md` under `[Unreleased] / Added`.

See `CONTRIBUTING.md` for the full checklist.
