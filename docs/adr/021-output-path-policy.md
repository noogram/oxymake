# ADR-021: One Output-Path Policy for Every Executor, Checked Before Dispatch

## Status
Accepted

## Metadata

- **Date:** 2026-10-30
- **Kind:** `decision`
- **Family:** `EXT`
- **Supersedes:** `none`

## Context

`ox` acts on declared output paths itself, apart from the command that
produces them. Before a job it creates parent directories and removes stale
outputs and `.oxytmp` staging files; after it, it renames staged files into
place, restores outputs from the cache (ADR-001), and on failure removes
declared outputs according to `clean_outputs`. Since the Ray output
finalization in `[Unreleased]` (#60), the Ray driver also removes and
replaces declared outputs. Which paths those actions may touch depends on the
executor:

1. **Local executor.** `LocalExecutor::prepare_workspace` checks each job's
   file outputs lexically. A relative path must normalise under the current
   directory, which is the Oxymakefile's directory. An absolute path must lie
   under an absolute `[config]` value (`ExecContext::trusted_dirs`); the
   project root itself is not consulted, so an absolute path inside the
   project is refused. A refusal reads `output path escapes project root:
   {path} resolves outside {root}`, gives no remedy, and aborts the run.
2. **Disk writer.** With `--memory-budget`, `disk_writer` (H13) confines
   writes to the project directory with a lexical and a canonical
   (symlink-resolving) check, and ignores `trusted_dirs`. An output under an
   absolute config directory, accepted by rule 1, is refused here.
3. **Ray and Slurm.** No check. The Ray DAG driver joins the working
   directory with the declared path, which keeps absolute and `../` paths.
   The Slurm job script runs `mkdir -p` on each output's parent. The same
   workflow succeeds or fails depending on `--executor`, and with #60 a
   failed Ray task can delete a declared file outside the project.

None of these rules is documented for users. The code comments state the
rationale as "the Oxymakefile is untrusted input". `SECURITY.md` says the
opposite: an Oxymakefile runs arbitrary commands by design and must be
treated as an executable script. A path check cannot protect against a
workflow that is allowed to run `rm`.

The need that exposes the gap, writing results to a directory outside the
project, is already served by an absolute `[config]` value (or `--set`)
referenced as `{config.key}` in the output path. That mechanism is
undocumented.

## Decision

**Rationale.** `ox` creates, replaces and removes files only under roots the
user declared, whatever the executor. The policy guards against declaration
mistakes (a wrong `..`, an absolute path typed by hand, a wildcard value
taken from data, see `SECURITY.md`) that would point `ox`'s own file actions
at files the user did not place under workflow management. It is not a
sandbox: the user's command can still write anywhere its process can.

**Declared roots.** The project root (the directory containing the
Oxymakefile, the working directory of `ox run`) and every absolute scalar
value of the resolved configuration, after profiles and `--set` are applied.
No new field is added.

**Rule.** Each declared file output is made absolute by joining a relative
path to the project root, then normalised lexically (`.` and `..` removed;
the path need not exist). It is accepted if and only if the result lies
under one of the declared roots. The rule does not distinguish how the path
was written: an absolute path inside the project is accepted, and a relative
path that normalises under a declared config directory is accepted.

**Where it is checked.** One function in `ox-core` evaluates the rule over
every job of the resolved graph, independently of cache state and of
`--until`/`--omit-from` selection. `ox run` calls it once the job graph is
built, before the `--dry-run` report, the cache pre-scan (which can restore
outputs from a remote cache) and the executor branch: before `ox` touches any
output path, dispatches any job, generates or submits a Ray driver, or writes
a Slurm script. `--dry-run` therefore applies the same check. The check
performs no filesystem access. Library callers that dispatch jobs themselves
call the same function.

**On refusal.** The run stops before dispatch with exit code 1 and the same
message for every executor. The message names the job, the declared path,
the normalised path, and the project root, and states the remedy: declare the
directory as an absolute `[config]` value (or pass it with `--set`) and write
the output as `{config.key}/…`.

**Runtime checks stay as defence in depth and follow the same root set.**
`LocalExecutor::prepare_workspace` applies the same rule (so it accepts
absolute paths under the project root). `disk_writer` receives the trusted
directories and accepts a target under any declared root. Its canonical
check stays: a target is refused if its canonical parent lies outside the
canonical form of every declared root. A root that is itself a symlink is
compared through its canonical form, so a path accepted before dispatch is
not refused at write time for that reason alone.

**Documentation and release notes.** The format reference documents the rule
and the remedy. The implementing change carries a `Breaking changes` entry in
`CHANGELOG.md`, because it tightens Ray and Slurm.

## Consequences

- Ray and Slurm runs whose outputs resolve outside every declared root now
  fail before submission, where they used to run. Migration: add
  `[config] outdir = "/absolute/dir"` (or `--set outdir=/absolute/dir`) and
  write the output as `{config.outdir}/file`. The `Breaking changes` entry
  states this.
- Absolute outputs inside the project, refused today by the local executor,
  are accepted on every executor. Under `--memory-budget`, outputs under a
  declared config directory are written instead of refused.
- A refusal no longer depends on the executor, the cache state, or whether
  `--memory-budget` is set, and it happens before any job starts, so no
  partial run is left to clean up.
- Every absolute config value widens the root set, including values not meant
  as output directories; a value of `/` disables confinement. This follows
  from the rationale: the policy catches mistakes, it does not isolate.
- The pre-dispatch check is lexical. A symlink inside a declared root that
  points outside all roots passes it; only `disk_writer` (with
  `--memory-budget`) detects it at write time. This residual is accepted.
- The implementation should land before, or in, the release that ships Ray
  output finalization (#60). A release that ships #60 without it must say in
  its `CHANGELOG.md` that Ray finalization acts on declared outputs outside
  the project.
- An executor added later inherits the policy without implementing it: the
  check runs before the SUBMIT direction of ADR-008, so a bridge never
  receives an unchecked output path.
- The policy is enforced by tests at the shared function, the local executor,
  `disk_writer`, and a Ray driver stub. It adds no named invariant and no
  TLA+ scope under ADR-015.
- ADR-017 does not apply: it governs which repository files are published,
  not where a run writes.

## Alternatives Considered

- **Confine nowhere** (remove the local check). Consistent with `SECURITY.md`
  and simplest, but `ox` itself would then remove and replace files anywhere a
  mistaken declaration points, including through Ray finalization on task
  failure. Rejected.
- **A check per executor.** This is the current design. It produced three
  rules and two executors with none. The Ray DAG path runs a generated Python
  driver on the cluster with no per-job Rust hook, so each executor would
  re-implement the rule in its own language. Rejected.
- **Check only at the point of each file action** (driver `os.remove`,
  `disk_writer`, `prepare_workspace`). This catches symlinks at runtime, but
  it fails mid-run after other jobs have started, needs one implementation per
  action site, and cannot give one message. Rejected as the primary check;
  the existing runtime checks remain as defence in depth.
- **A dedicated field listing output roots** (for example a workflow-level
  list of allowed directories). More explicit than treating every absolute
  config value as a root, but it adds a format surface while the config
  mechanism already serves the need. Rejected for now; it can be revisited if
  the implicit root set proves too broad.
- **Sandboxing the command** (namespaces, chroot, Landlock). It would confine
  the command as well as `ox`, but it is platform-specific and contradicts the
  trust model of `SECURITY.md`. Out of scope.
