# Panel findings — factual verification before publication (2026-08-10)

Four engineering findings were raised by a review panel and reported
*unverified*. Two of them were flagged as potentially contradicting published
claims, which would place them in the same class as the three contradictions
already recorded in `docs/paper/ERRATUM.md` section D.

Method, for each finding: establish the fact in the code, cite `file:line`,
reproduce it with the built binary where a runtime check is possible, and
conclude CONFIRMED / REFUTED / PARTIAL. Text was corrected only where a
confirmed fact contradicts a published claim; no code behaviour was changed to
match the text, and no feature was added.

Binary under test: `cargo build -p ox-cli --bin ox` at
`fd8bef5` + this branch (debug profile).

---

## 1. State-directory relocation — CONFIRMED (fourth published contradiction)

**Claim under test.** The paper discharges the `StateDbAtomicCommit` axiom with
a local-disk requirement on `.oxymake/`, and the book draws the corresponding
deployment explicitly: `state.db` on local disk, `project/` (inputs + outputs)
on the shared filesystem. The panel asserts `.oxymake` is a hard-coded relative
path with no relocation option, and that `OXYMAKE_CACHE_DIR` is documented but
never read — which would leave the announced discharge without a code path.

**Fact.**

`.oxymake` is constructed as a bare relative `PathBuf` at every site that
opens state or cache:

- `crates/ox-cli/src/commands/run.rs:1018` — `let oxymake_dir = PathBuf::from(".oxymake");`
- `crates/ox-cli/src/commands/run.rs:1101` — same, on the main run path
- `crates/ox-cli/src/commands/run.rs:1423` — `log_dir: PathBuf::from(".oxymake/logs")`
- `crates/ox-cli/src/commands/run.rs:1436` — `state_db_path = oxymake_dir.join("state.db")`
- `crates/ox-cli/src/commands/clean.rs:58`, `invalidate.rs:77`,
  `status.rs:84`, `init.rs:56` — same literal

No argument, no config key, and no environment variable feeds any of these.

`OXYMAKE_CACHE_DIR` appears exactly once in the repository —
`docs/book/src/reference/configuration.md:46`, in the environment-variable
table. It appears in no `.rs` file. The complete set of environment variables
actually read by the workspace is:

    OX_CACHE_VALIDATION, OXYMAKE_CONTAINER_CMD (written, not read — see §2),
    OXYMAKE_MEMORY_LIMIT_BYTES, OXYMAKE_OBJREF_*, OXYMAKE_WORKSPACE,
    HOME, XDG_CONFIG_HOME, SLURM_JWT, SLURM_USER, TERM, USER, CI, NO_COLOR

`OXYMAKE_JOBS`, `OXYMAKE_EXECUTOR` and `OXYMAKE_LOG` — the other three rows of
that same documented table — are equally absent from the code.

The `.oxymake/config.toml` file that the same chapter describes as the store of
project-level defaults (`[defaults]`, `[cache] dir`, `[state] dir`) is never
read either. The only configuration file loaded is the *user-global* one,
`$XDG_CONFIG_HOME/oxymake/config.toml` (`crates/ox-cli/src/commands/run.rs:742-770`),
and only two keys are pulled from it: `cache_validation` and `open_dashboard`.
`ox init` creates `Oxymakefile.toml` and an empty `.oxymake/` directory
(`crates/ox-cli/src/commands/init.rs:44-65`) — it does not write a
`config.toml`.

**Reproduction.**

    $ OXYMAKE_CACHE_DIR=$PWD/elsewhere ox run -f Oxymakefile.toml
    $ ls elsewhere        # empty
    $ find .oxymake -maxdepth 2
    .oxymake/cache  .oxymake/state.db  .oxymake/logs  .oxymake/events

The variable has no effect.

**The load-bearing consequence.** `.oxymake/` is relative to the *process
working directory*, and so are the rule's inputs and outputs — a run from a
sibling directory with `-f ../project/Oxymakefile.toml` fails on
`cat: src.txt: No such file or directory` while creating `.oxymake/` in the
sibling. The state directory therefore cannot be separated from the workspace:
placing the project on a shared filesystem necessarily places `state.db` there
too. The split the paper and the book depict — SQLite on local disk, project
tree on NFS/Lustre/GPFS — is not reachable through any code path.

The requirement is not *false*; it is satisfiable only by putting the whole
workspace on local disk. What is false is the published depiction of how it is
satisfied.

**Verdict: CONFIRMED.** Fourth published contradiction. Corrected in:

- `docs/paper/oxymake-paper.tex` — §Execution model note and §Limitations now
  say the requirement is on the workspace, not on a relocatable state
  directory, and name the absence of a relocation mechanism.
- `docs/paper/ERRATUM.md` §D — new entry.
- `docs/book/src/concepts/slurm-integration.md` — the diagram and the
  "Critical constraint" paragraph.
- `docs/book/src/reference/configuration.md` — the fictional `[defaults]`,
  `[cache]`, `[state]` sections and the three unimplemented environment
  variables removed; the precedence list corrected.

---

## 2. `OXYMAKE_CONTAINER_CMD` written but never read — CONFIRMED

**Fact.** `crates/ox-exec-slurm/src/job_script.rs:353` (Docker fallback) and
`:359` (explicit Apptainer) emit the line

    OXYMAKE_CONTAINER_CMD="apptainer exec <image>"

into the generated sbatch script. The script then appends the rule command
verbatim — `job_script.rs:104-106`, `script.push_str(&command)` where
`command = resolve_command(job)?` returns the raw shell string
(`job_script.rs:150-152`). The array path does the same at `:307-320`. The
variable is expanded nowhere: it is absent from every other `.rs`, from the
generated script body, and from the workspace's env-var read set enumerated in
§1.

Same file, same function: `EnvSpec::Nix` is an explicit no-op
(`job_script.rs:371`, `let _ = expr;`), and `EnvSpec::Uv` emits `uv sync -r
<req>` without running the command under `uv run` (`job_script.rs:361-367`), so
the interpreter the job actually uses is unchanged. Only `EnvSpec::Conda`
takes effect on Slurm, because `conda activate` is a shell statement that
mutates the script's own environment (`job_script.rs:337-341`).

The local executor, by contrast, does wrap correctly —
`crates/ox-exec-local/src/executor.rs:376-441` builds `conda run`,
`docker run --rm`, `uv run`, `nix develop -c` and `apptainer exec` around the
command. The defect is specific to the Slurm backend.

**The compounding risk.** The cache key hashes the environment spec regardless
of backend — `crates/ox-cache/src/key.rs:185-220`, `env_spec_content_hash`,
with an `Apptainer { image }` arm at `:217`. A Slurm job declared under
`apptainer` therefore runs *outside* the container while its result is stored
under a key that records the container. A later local run of the same rule,
which does apply the container, reads that entry as a hit. This is a
cross-backend stale-reuse path, not merely an ineffective setting.

**Does a published claim depend on it?** Yes, one:
`docs/book/src/concepts/slurm-integration.md:732-748` states that when a Docker
environment is used with the Slurm executor "OxyMake automatically falls back
to Apptainer", and shows `apptainer exec nvcr.io/...` as the generated command.
The generated line is an unused variable assignment, not a command wrapper.
`README.md:428-431` says environment specs "are delegated as command wrappers"
without qualifying the backend. The paper makes no Slurm-container claim;
`docs/paper/oxymake-paper.tex:2469` only discusses mutable image tags in the
cache key, which is unaffected.

**Verdict: CONFIRMED.** Documentation corrected in
`docs/book/src/concepts/slurm-integration.md` and `README.md` to state that
container and `uv`/`nix` environments are not applied by the Slurm backend.
The code defect is recorded here and in the CHANGELOG as a known limitation;
fixing it is a behaviour change and out of scope for this verification.

---

## 3. `RawRule` accepts unknown fields; documented `env = …` silently ignored — CONFIRMED

**Fact.** `crates/ox-format/src/parse.rs:239-295` declares

    #[derive(Debug, Deserialize)]
    struct RawRule { … }

with no `#[serde(deny_unknown_fields)]`. The attribute appears nowhere in
`crates/ox-format/src/`. Unknown rule keys are dropped without diagnostic.

The environment field is named `environment` and is a
`BTreeMap<String, String>` keyed by *backend name*
(`parse.rs:258-259`, and `parse_environment` at `parse.rs:1277-1305`, which
matches the keys `uv`, `conda`, `docker`, `nix`, `apptainer`). The real
spelling is therefore `environment = { uv = "requirements.txt" }`. There is no
`env` field on `RawRule` and no `[env.*]` table on `RawWorkflow`
(`parse.rs:178-197`).

`docs/book/src/reference/format.md:136-145` documents the opposite:

    [env.analysis]
    type = "uv"
    requirements = "requirements.txt"

    [rule.analyze]
    env = "analysis"

Every element of that snippet is inert: `[env.*]` is an unknown top-level
table, `env =` is an unknown rule key, and `type = "uv"` is not the shape
`parse_environment` matches.

**Reproduction.** With the documented spelling above, `ox plan` reports
`1 rules, 1 jobs` and `ox run` executes the shell command with no `uv` in
sight. With the real spelling and a deliberately bad requirements path:

    environment = { uv = "requirements-DOES-NOT-EXIST.txt" }
    $ ox run -f Oxymakefile.toml
    stderr: error: unexpected argument '-r' found
    stderr: Usage: uv run [OPTIONS] [COMMAND]

`uv run` is reached only by the real spelling. A rule documented as running
under `uv` runs without it, and — since `env_spec_content_hash` sees `None` —
its cache key omits the environment entirely.

`docs/book/src/concepts/environments.md` and
`docs/book/src/getting-started/installation.md:49-52` already use the correct
form; `reference/format.md` is the sole outlier. A user-facing warning string
inside the Slurm backend
(`crates/ox-exec-slurm/src/job_script.rs:349`) also advertised
`environment = { type = "apptainer", … }`.

**Verdict: CONFIRMED.** `docs/book/src/reference/format.md` corrected to the
supported spelling; the misleading warning string corrected to the same. The
missing `deny_unknown_fields` is recorded as a known limitation — adding it is
a breaking parser change and out of scope here.

---

## 4. glibc 2.39 floor on the published Linux artefact — PARTIAL

**What is established locally.** `.github/workflows/release.yml:58-98` defines
the release build matrix. It has exactly three entries:

    x86_64-unknown-linux-gnu   on ubuntu-latest
    aarch64-apple-darwin       on macos-*
    x86_64-apple-darwin        on macos-*

There is **no** `*-unknown-linux-musl` target, no `cross`/`zig` toolchain, and
no container build. The runner label is the floating `ubuntu-latest`, not a
pinned image, and the build is a plain
`cargo build --release --bin ox --target ${{ matrix.target }}` (`:82`).

Two facts follow with certainty from the workflow alone: the published Linux
binary is dynamically linked against the glibc of whatever `ubuntu-latest`
resolved to at build time, and there is no statically linked alternative in the
release. Because the label floats, the glibc floor is not pinned by the
repository and can rise on a future release without any change to the
workflow.

**What is not established here.** The specific `GLIBC_2.39` symbol requirement
reported by the panel comes from the ELF `verneed` table of the published
v0.1.0 artefact. That artefact was not downloaded, so the exact floor is not
confirmed by this audit. It is *consistent* with `ubuntu-latest` mapping to
Ubuntu 24.04 (glibc 2.39), and if the floor is 2.39 it does exclude RHEL/Rocky
8 and 9, SLES 15, and Ubuntu 22.04 — but that chain rests on an inference about
the runner image, not on a local measurement.

**Does a published claim depend on it?** No. The paper's uses of "portable"
(`docs/paper/oxymake-paper.tex:459, 477, 2109, 2406`) all describe *other*
systems or the workflow-specification layer, never the distribution of the
`ox` binary. `README.md`, `RELEASING.md` and
`docs/book/src/getting-started/installation.md` make no statement about
supported glibc versions or Linux distributions. Nothing in the published text
is contradicted.

**Verdict: PARTIAL — matrix gap confirmed, symbol floor unverified.** Does not
block publication of the paper. No text change. Recorded as a release-
engineering gap: add a `x86_64-unknown-linux-musl` target and pin the Ubuntu
runner if a documented glibc floor is wanted.

---

## Summary

| # | Finding | Verdict | Contradicts published text? |
|---|---------|---------|-----------------------------|
| 1 | `.oxymake` hard-coded; `OXYMAKE_CACHE_DIR` unimplemented | CONFIRMED | Yes — paper §limitations, §named invariants, book Slurm chapter, book configuration chapter |
| 2 | `OXYMAKE_CONTAINER_CMD` written, never read | CONFIRMED | Yes — book Slurm chapter, README (paper unaffected) |
| 3 | `RawRule` accepts unknown fields; `env = …` inert | CONFIRMED | Yes — book format reference |
| 4 | glibc floor / no musl target | PARTIAL | No |

Three of the four are real. One (#1) is a fourth entry for `ERRATUM.md`
section D. In every case the text was corrected to say what the code does; no
code behaviour was changed and no feature was added.
