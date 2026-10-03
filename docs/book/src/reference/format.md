# Oxymakefile Format

OxyMake workflows are defined in `Oxymakefile.toml`, a declarative TOML file.
This page is the complete format reference.

## Top-Level Fields

```toml
format_version = "2"
ox_version = ">=0.7.0"       # Minimum capable binary, not a reproducibility pin.
```

Absent `format_version` selects legacy schema 1. Only `"1"` and `"2"` are
supported. Schema 1 leaves `ox_version` informational and warns for each file
that no binary requirement is enforced; its resource identity is unchanged.

Schema 2 requires exactly `>=MAJOR.MINOR.PATCH` with canonical decimal version
components, for example `>=0.7.0`. Missing requirements, shorthand (`0.1`), other
comparators, ranges, prerelease/build suffixes, and an unsatisfied minimum are
errors. Diagnostics name the file, requirement and running binary version.
Every included file is checked before any external config source is read or a
workflow is planned or run. Included files declare their own schema and minimum;
a legacy include still warns and has no enforced binary requirement.

Schema 2 rejects unknown structural keys, including nested rule, profile, gate,
executor, log, input/output descriptor and guard fields. Dynamic maps (`config`,
`tags`, `params`, resource names, wildcard constraints, profile overrides and
named input/output paths) keep their user-defined names. Named resource classes,
variables tables and resource exports are not implemented.

Use [`ox migrate --to-format 2`](commands/migrate.md) to review an upgrade.

## Config Section

The `[config]` section defines workflow-level variables used for wildcard
expansion:

```toml
[config]
samples = ["A", "B", "C"]
chromosomes = ["chr1", "chr2", "chr3"]
models = ["linear", "ridge", "lasso"]
```

Config values are arrays of strings. They drive wildcard expansion in rules.

## Rule Definitions

Each rule is a `[rule.<name>]` table:

```toml
[rule.process]
input = ["data/{sample}.csv"]
output = ["results/{sample}.txt"]
shell = "python process.py {input} {output}"
```

### Rule Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `input` | Array of strings | No | Input file patterns with `{wildcards}` |
| `output` | Array of strings or descriptors | Yes | Output file patterns with `{wildcards}`; descriptors configure individual outputs |
| `shell` | String | One of shell/run/script/call | Opaque shell command |
| `run` | String | One of shell/run/script/call | Inline script (with `lang`) |
| `script` | String | One of shell/run/script/call | Path to script file |
| `call` | String | One of shell/run/script/call | Python function reference |
| `lang` | String | With `run`/`script` | Language: `python`, `r`, `julia` |
| `tags` | Table of string → string | No | Key/value labels for grouping and event filtering, e.g. `tags = { stage = "align", speed = "slow" }`. An array of strings is **not** accepted. |
| `resources` | Table | No | Resource requirements; see [Resources](#resources) below for what each executor actually does with them |
| `resource_class` | String | No | Schema 2 only: name of a root-level resource class whose values this rule inherits before applying its `resources` overrides |
| `environment` | Table | No | The software environment to run the command in (`uv`, conda, Nix, Apptainer/Singularity); see the environment backend section. Not to be confused with environment *variables* — there is no field for those today, and under schema 2 a rule-level `env` is rejected with that explanation. |
| `when` | String | No | Conditional guard expression |
| `params` | Table | No | Rule-specific parameters |
| `clean_outputs` | String | No | `always` (default), `on-failure`, `never`; see Output cleanup below |
| `cache_platform` | String | No | `exact` (default), `any`; see Cross-platform cache reuse below |

There is no `env` field. An Oxymakefile has no way to declare environment
variables for a job's command — not the `environment` table above (that
selects a software backend, not variables) and not a dedicated field. A
variable a command needs today must come from the parent shell's
environment, from `{resources.NAME}`/`{threads}` interpolation into the
command text (see [Resource interpolation in commands](#resource-interpolation-in-commands)),
or from what the executor itself sets (see
[`docs/format/env-vars.md`](https://github.com/noogram/oxymake/blob/main/docs/format/env-vars.md)).

`materialize` belongs to an output descriptor, not directly to the rule. Its
values are `always`, `auto`, `never`, and `final`:

<!-- schema2-materialize-example-start -->
```toml
format_version = "2"
ox_version = ">=0.7.0"

[rule.a]
output = [{ path = "a", materialize = "always" }]
shell = "touch a"
```
<!-- schema2-materialize-example-end -->

### Cross-platform cache reuse

`cache_platform` controls whether the platform (`OS/architecture`) participates
in a rule's cache key. The default, `"exact"`, restricts reuse to the same
platform. Set it to `"any"` only when the rule's outputs are suitable for reuse
across platforms:

```toml
[rule.merge_counts]
input = ["data/*.parquet"]
output = ["build/counts.parquet"]
shell = "duckdb -c '...'"
cache_platform = "any"
```

The engine cannot verify this claim. A rule that emits platform-specific
artifacts, such as machine code, must use `"exact"`. The parser rejects
`cache_platform = "any"` together with
`reproducibility = "non_reproducible"`; `"approximate"` and
`"seed_deterministic"` are allowed.

### Executor override

A rule may declare `executor = "local"` to run on the host running `ox`.
It is a no-op in a local run. In a SLURM run, selecting an uncached job with this override uses
the local scheduler for the selected graph: local rules execute on this host,
other rules use SLURM, and `ox run` waits for completion even without
`--follow`. Dependencies, cache checks, retries, logs, cancellation and
the `--jobs` limit still go through the scheduler. This limit also bounds
cluster submissions and defaults to 1; `ox` must stay alive. Both hosts must already
see the input and output files through a shared filesystem.

Ray's whole-DAG driver cannot execute a task on the submitting host.
An active job declaring `executor = "local"` is therefore rejected
before Ray submission, including with `--follow`. Use a separate local run
for those targets. Cached jobs and jobs excluded by target selection,
`--until`, or `--omit-from` neither block Ray nor switch SLURM dispatch.

Only the exact value `"local"` is accepted. Other values are parse errors
naming the rule; choose the run's backend with `ox run --executor`.
Per-rule site placement, several remote backends in one run, and artifact
transfer inside a run are outside this field's scope. Job-start events and
history record the executor selected for each executed job.

### Output cleanup

`clean_outputs` is an optional per-rule string (currently local-executor only):

| Value | Before execution | After failure |
|-------|------------------|---------------|
| `"always"` (default) | Delete existing declared outputs | Delete partial outputs |
| `"on-failure"` | Keep existing declared outputs | Delete partial and existing outputs |
| `"never"` | Keep existing declared outputs | Keep all declared outputs |

Three values express three distinct lifecycles; a boolean cannot represent
all of them. The default preserves the existing guarantee that stale outputs
from a failed run cannot masquerade as valid results. OxyMake always clears
its own `.oxytmp` staging files before execution. Output verification and
hashing after execution are unchanged. A failed job is still recorded as a
failure and never writes a successful cache entry. Changing the policy
invalidates the job's cache key.

**Warning:** `"never"` hands the staleness guarantee to the script. The script
must validate existing files, replace stale data, and exit successfully only
when every declared output is complete. Preserved files alone do not prove
that a failed run succeeded. `"on-failure"` also requires the script to validate
any existing outputs it reuses on a successful run.

Slurm and Ray carry this field but keep their existing output behavior; this
policy controls automatic cleanup by the local executor. Explicit `ox clean`
and output lifecycle policies such as `temp` are separate mechanisms.

#### Incremental cache of an external source

For a dataset of 509 parquet files (about 4 GB over S3), declare the actual
files as outputs and let an idempotent extraction script reuse complete files:

```toml
[rule.fetch_dataset]
input = ["scripts/extract.py", "dataset-manifest.json"]
output = ["cache/2025-01.parquet", "cache/2025-02.parquet"] # List all 509 files.
clean_outputs = "never"
shell = "python scripts/extract.py dataset-manifest.json"
```

The script checks each completed file against the manifest, downloads missing
or stale files to its own temporary paths, then renames each completed file
into place. Editing the script invalidates the rule, but complete downloads
remain available to reuse. A transient network failure at file 400 preserves
the earlier downloads for the next attempt. Avoid OxyMake's reserved
`.oxytmp` suffix for the script's temporary files. Unlike a stamp-only rule,
OxyMake tracks and hashes the real dataset outputs.

### Execution Modes

Four modes form a spectrum from flexibility to optimizability:

**shell** -- Opaque shell command. Maximum flexibility, no optimization.
```toml
[rule.align]
shell = "bwa mem ref.fa {input} > {output}"
```

**run** -- Inline script with language specification.
```toml
[rule.stats]
lang = "python"
run = """
import pandas as pd
df = pd.read_csv("{input}")
df.describe().to_csv("{output}")
"""
```

**script** -- External script file.
```toml
[rule.analyze]
lang = "python"
script = "scripts/analyze.py"
```

**call** -- Pure function reference. Supports in-memory Arrow IPC passing.
```toml
[rule.features]
input = [{ path = "data/{sample}.parquet", format = "parquet" }]
output = [{ path = "features/{sample}.parquet", format = "parquet", materialize = "auto" }]
call = "pipeline.features:compute_features"
```

### Wildcards

Wildcards in `{braces}` are resolved from `[config]` arrays or from a requested
target path that matches a rule output pattern. Existing files are recognized
as source inputs; their names do not themselves enumerate wildcard values.

```toml
[config]
samples = ["A", "B"]

[rule.process]
input = ["data/{sample}.csv"]     # {sample} expanded from config.samples
output = ["results/{sample}.txt"]
```

### Resources

```toml
[rule.heavy_job]
output = ["results/big.txt"]
shell = "compute_heavy"
resources = { cpus = 4, mem_gb = 16, gpu = 1 }
```

Schema 2 can collect repeated declarations into a root-level named class:

```toml
format_version = "2"
ox_version = ">=0.7.0"

[resource_classes.standard]
cpu = 2
mem = "4G"

[rule.first]
output = ["first.txt"]
shell = "compute --threads {threads} > {output}"
resource_class = "standard"

[rule.larger]
output = ["larger.txt"]
shell = "compute --threads {threads} > {output}"
resource_class = "standard"
resources = { cpus = 6 }
```

A class contains resource values only. It cannot inherit another class or
declare variables, execution, conditions, or environment settings. Definitions
may follow their references or live in included files; all files in the include
graph share one class namespace. Duplicate class definitions and unknown class
references are errors. A rule's inline `resources` replace individual canonical
dimensions from its class, so `cpus` may override class `cpu`, but aliases may
not be duplicated within either table.

Classes resolve before command interpolation. Their effective values therefore
work with `{threads}` and `{resources.NAME}` exactly like inline values. Cache
identity also follows those effective values: class names, definition order,
and definition file paths do not enter the key. As before, a resource affects a
key only when interpolation places its value in the execution text.

#### What a declared resource actually does, per route

A declared resource means something different on every execution route.
Read this table before relying on `resources` for anything beyond
scheduling hints. Definitions: **Requested** — the declaration is parsed
and attached to the job. **Admitted** — something checks the declaration
before letting the job start, and can refuse or delay it. **Enforced** —
something stops the running job from exceeding the declaration.
**Measured** — the actual usage is observed and reported back, independent
of what was declared.

| Route | Requested | Admitted | Enforced | Measured |
|---|---|---|---|---|
| Local, no `--resource-budget` | Parsed and carried on the job exactly as written — the canonical form (`cpu`/`cpus` and the memory aliases) is computed only once a budget is active | **No** — nothing gates start; `-j` concurrency is the only limit | **No** | CPU/RSS via `wait4` if `benchmark` is set (see [Benchmark output](#benchmark-output)); not tied to the declaration |
| Local, `--resource-budget` | Yes | **Yes**, for canonical resources present in the budget — OxyMake will not start more concurrent work than the budget allows (a budgeted `cpu` also constrains `cpus`; an unbudgeted key, e.g. `mem` with no `mem` capacity configured, is not interpreted at all) | **No** — a job declaring `cpu = 1` may spawn any number of threads, and nothing sets a `ulimit` or kills it for exceeding declared memory | Same as above |
| Ray, native DAG | Yes | **Yes** — becomes a real Ray task request (`num_cpus`, `num_gpus`, `memory`, custom resources via `.options()`) checked against live node capacity before submission (`crates/ox-exec-ray/src/driver_script.rs:239-255`, `crates/ox-exec-ray/src/feasibility.rs:58`, called from `crates/ox-exec-ray/src/executor.rs:701`) | Partial — Ray's own scheduler will not co-place tasks whose declared totals exceed a node's advertised capacity, but a task using more than it reserved is not stopped (logical scheduling, not containment) | Ray dashboard node/task stats, not surfaced by OxyMake |
| Ray, Jobs API and arrays | Yes | **Partial** — CPU/GPU/custom resources are requested at the *entrypoint* (driver) level, shared across the jobs the entrypoint runs, not admitted per job; memory is **not** reserved at all — it only becomes an `OXYMAKE_MEMORY_LIMIT_BYTES` environment hint (`crates/ox-exec-ray/src/runtime_env.rs:91-98`) that nothing in OxyMake reads back | **No** | Not measured by OxyMake |
| SLURM, CLI/REST, single job | Yes — mapped to `#SBATCH` directives (`crates/ox-exec-slurm/src/resource_mapper.rs`) | **Depends on cluster configuration** — SLURM itself can enforce (cgroups) or merely record, depending on how the cluster is configured; OxyMake does not control or detect this | Depends on cluster configuration, same caveat | Whatever `sacct`/`seff` reports on that cluster; OxyMake does not read it back |
| SLURM, CLI/REST, array | Yes — same directive mapping, shared across array tasks | Same as single job | Same as single job | Same as single job |
| SLURM, CLI/REST, DAG (`--dependency` chain) | Yes — each step is its own single-job script with its own directives | Same as single job, per step | Same as single job, per step | Same as single job, per step |
| Mixed SLURM+local (`executor = "local"` override) | Yes | Locally routed jobs go through the local admission row above (opt-in `--resource-budget`); remote jobs keep their SLURM request and its cluster-dependent admission | Per the relevant row above | Per the relevant row above |

The portable resource vocabulary shared by Ray and local scheduler
admission is:

| Resource | Accepted keys | Value |
|----------|---------------|-------|
| CPU | `cpu`, `cpus` | Token count, exact to `0.0001` |
| GPU | `gpu`, `gpus` | Token count, exact to `0.0001`; Ray requires whole counts above 1, while local admission accepts fractions above 1 |
| Memory | `mem`, `memory` | Bytes, or a string using `K`/`KB`/`KiB` through `T`/`TB`/`TiB` |
| Memory | `mem_mb` | MiB (2^20 bytes), including fractional MiB |
| Memory | `mem_gb` | GiB (2^30 bytes), including fractional GiB |
| Custom token | any other key, or `custom:<name>` | Case-sensitive token count, exact to `0.0001` |

All memory suffixes above use binary scaling, so `"1GB"` and `"1GiB"`
both mean 1,073,741,824 bytes. The explicit `custom:` prefix always names a
custom token: `custom:mem_mb` is distinct from the memory key `mem_mb`.

Aliases may not be combined for the same resource (`cpu` with `cpus`, for
example), and a bare custom name may not be combined with its prefixed form
(`metal` with `custom:metal`). Invalid units, negative or non-finite values,
fractional byte results, overflow, and token counts finer than `0.0001` are
errors. These checks normalize a declaration without rewriting the original
`resources` table. Executor-specific keys, including SLURM directives such as
`time`, retain the vocabulary documented by that executor. `time = 60`
is a SLURM time limit; on Ray it would request a custom token named `time`
and the default feasibility check rejects it unless a node advertises 60 units.
Do not copy SLURM-only keys into a Ray workflow.

Ray rejects custom names with leading or trailing whitespace, and names
colliding (case-insensitively) with `cpu`, `cpus`, `gpu`, `gpus`, `memory`,
`object_store_memory`, or the reserved prefixes `node:` and `accelerator_type:`.
This also applies to explicit `custom:` names. Other names, including quotes,
backslashes and non-ASCII characters, are forwarded as data.

Native Ray DAG tasks reserve CPU, GPU, memory and custom resources. Memory is
forwarded as `memory=<bytes>` and included in the live-node feasibility check.
This is logical scheduling admission, not a hard RSS limit. The per-job Jobs
API and array paths do not reserve memory; there, a declared memory value is
instead forwarded to the job as the `OXYMAKE_MEMORY_LIMIT_BYTES` environment
variable (`crates/ox-exec-ray/src/runtime_env.rs:91-98`). That variable is a
hint for user code to read, not a limit OxyMake or Ray enforces — nothing in
OxyMake sets an rlimit or cgroup from it, and nothing kills a job for
exceeding it.

Local scheduler admission is opt-in with `ox run --resource-budget KEY=VALUE`.
The flag is repeatable and comma-separated, for example
`--resource-budget cpu=6,mem_gb=32 --resource-budget metal=1`; it uses exactly
the aliases, binary memory units and duplicate checks above. Capacity counts
are whole tokens; job demands may be fractional. Memory capacity is normalized
to whole bytes, so `mem_gb=1`, `mem_mb=1024`, and `memory=1GiB` are identical.
Two spellings of one canonical resource across flags are an error.

Only budgeted canonical resources are interpreted and enforced. For example,
a `cpu = 6` budget also constrains a `cpus = 4` demand. A job with no demand
consumes no resource tokens and still uses one `-j` / `--jobs` slot. Zero
demand fits zero capacity; a positive demand against zero capacity, or any
demand exceeding total capacity, is a configuration error naming the job and
resource before execution of the selected DAG starts, even when every job is
cached. Rules excluded by target selection, `--until` or `--omit-from` do not
participate in validation. In mixed SLURM runs, only locally routed jobs
consume this budget; remote jobs retain their SLURM resource requests. Pure
remote runs reject `--resource-budget`.

`-j N` caps concurrent jobs, while `--resource-budget` caps resources held by
those jobs. This is distinct from `--memory-budget`, which caps in-memory
materialized outputs and never admits or rejects a job. The resource budget is
per `ox run`: two concurrent runs each get their full configured budget.
OxyMake does not detect host capacity or coordinate budgets across processes.

#### Resource interpolation in commands

A declared resource can reach the command text itself, through two
placeholders resolved while the job graph is built
(`crates/ox-core/src/resolver.rs:1350-1357`):

- `{threads}` substitutes the canonical CPU resource value (the Snakemake
  convention), declared as either `cpu` or `cpus`.
- `{resources.NAME}` substitutes any declared resource under its own key,
  e.g. `{resources.mem_gb}` for `resources = { mem_gb = 16 }`.

This mechanism covers **inline commands only** — `shell` and `run` blocks,
where the interpolated text is the command OxyMake executes
(`crates/ox-core/src/resolver.rs:1136-1204`). For `script`, only the path
string is interpolated, not the file's contents; for `call`, only the
function-reference string is interpolated, not the function body. A
`{threads}`/`{resources.NAME}` reference inside a script file or a called
function is not substituted — it reaches the script/function unexpanded.
Neither placeholder is available in a shared wrapper outside the command
(there is no such wrapper today; see the `env` field note above).

The interpolated execution block — the actual command text after
substitution — is what enters the cache key as `rule_source`
(`crates/ox-cache/src/key.rs`, `crates/ox-cli/src/commands/run.rs`). The
`resources` table itself is **not** a separate cache key input. The
practical consequence: a rule that writes `resources = { cpu = 4 }` and a
`shell` command that never mentions `{threads}` or `{resources.cpu}` can
have its `cpu` value changed without invalidating the cache, because the
command text — the only place resources reach the key — did not change. A
rule that interpolates `{threads}` or `{resources.NAME}` into its command
does get cache invalidation when that value changes, as a side effect of
the command text changing, not because `resources` is tracked directly.

#### A shared multi-user machine without root

This is the pattern for the case that motivated the guarantee table above:
several people running many rules on one Linux box, no root, no cgroups.

OxyMake bounds how many jobs *start*, not what each one *takes*. On a
shared machine, get all three of these, not just the first:

1. **Declare resources on every rule** — `resources = { cpu = N, mem_gb = M }`
   — even though local admission ignores them by default. This is what
   makes the next two steps possible and keeps the numbers visible in one
   place.
2. **Run with a matching `--resource-budget`** — e.g.
   `ox run --resource-budget cpu=<cores you're allowed>,mem_gb=<memory you're
   allowed>`. This caps how many jobs your own `ox run` starts concurrently.
   It does not cap what a single started job does once running, and it does
   not know about — or coordinate with — anyone else's `ox run`, your own
   other concurrent runs, or any process outside OxyMake.
3. **Cap library thread pools yourself**, inside the command, using
   `{threads}`/`{resources.NAME}` interpolation (above) so the number is
   declared once: e.g.
   `shell = "OMP_NUM_THREADS={threads} POLARS_MAX_THREADS={threads} {command}"`.
   OxyMake has no mechanism that does this for you.

A per-run `--resource-budget` coordinates nothing across runs or users. Two
people each running `ox run --resource-budget cpu=8` on the same 8-core
machine will each believe they have the whole machine; OxyMake will start
up to 8 CPU-tokens' worth of work *per run*, not per machine.

#### Memory limits: reservation, not enforcement

Nothing in OxyMake today enforces a declared memory value against a
running process — see the guarantee table above. If a future version adds
local memory enforcement, the mechanism matters: `ulimit -v` (`RLIMIT_AS`)
caps a process's *address space*, not its resident set (RSS). Address
space is routinely far larger than resident memory for a modern runtime —
a process can reserve gigabytes of virtual address space it never
touches — so an address-space limit rejects far more processes than a
resident-memory limit would, or none at all if set loosely enough to be
harmless. It is not a drop-in for "limit this job's memory." One team has
reported, unconfirmed and still under investigation on their side, that
wrapping a job in a `ulimit -v` cap appeared to break an S3 client's TLS
setup on its first call; whether the cap, a related environment change, or
something else in their environment caused it has not been established.
It is recorded here only as a caution, not as a finding about `ulimit -v`
in general: an enforcement feature should name its actual mechanism (RSS
via cgroup, address space via rlimit, or none) rather than promise
"memory".

### Benchmark output

Set `benchmark` on a rule to write a two-line TSV after a successful job:

```toml
[rule.measured]
output = ["results/output.txt"]
shell = "produce results/output.txt"
benchmark = "benchmarks/output.tsv"
```

The columns are `s`, `h:m:s`, `max_rss`, and `cpu_time`, in that order.
`max_rss` is expressed in MiB with two decimal places when the executor
supplies a per-job measurement, and `cpu_time` is expressed in seconds.
History's `peak_mem_mb` instead rounds up to whole MiB: 1500 KiB renders as
1.46 in benchmark TSV and 2 in history. The round-up keeps a positive sub-MiB
history observation distinguishable from zero. An unmeasured value is
written as `-`; a measured zero stays zero.

On Linux and macOS, local cold launches (buffered and streaming output) use
`wait4` on the job's own child process. RSS is that child's high-water mark,
including already-reaped descendants; it is not a simultaneous process-tree
memory total. Background or daemonised work can escape the observation.
The collector converts Linux KiB and macOS bytes to bytes before formatting.
CPU time is the same child's user plus system time, in seconds; no CPU/wall
ratio is inferred. It is recorded in benchmark TSV, not in `job_history`.
Failed and signalled children retain any measurements returned by `wait4` in
their executor `JobResult`; scheduler publication rules still apply. A timed-out
child is measured in the lower-level `ProcessResult`, but the executor returns
a timeout error and drops that result, so no measurement reaches `JobResult`.

With `--warm-workers fork`, the retained Python template owns and reaps one
child per dispatch using `os.wait4`. That child's RSS and CPU time use the same
conversion and output paths as a cold child, so successive dispatches through
one template do not inherit one another's dispatch-added peaks. The RSS figure
is nevertheless the child's high-water mark and starts with the template's
resident footprint inherited at fork. Only memory above that warm baseline was
added by the dispatch; the reported total is not the memory the dispatch would
need by itself. A NumPy- or PyTorch-heavy template can therefore dominate the
reported peak for a small dispatch. CPU time is local to the child. The
optional reply fields are backward compatible: an older template omits them
and a newer binary records them as absent; an older binary ignores fields added
by a newer template. Malformed or missing usage is absent, never zero.

`--warm-workers persistent` remains unmeasured. One process executes every
dispatch in-process, so its RSS high-water mark contains prior dispatches and
its CPU cannot be separated without a quiescence contract. OxyMake does not
publish a delta or sample as a per-dispatch figure. Unsupported platforms also
leave RSS and CPU absent.

### Conditional Guards

```toml
[rule.expensive]
output = ["results/{seed}.txt"]
shell = "compute {seed}"
when = "seed in @selected_seeds"
```

Guards are evaluated at DAG resolution time. Jobs whose guard is false are
never created.

## Include Directives

Split large workflows across files:

```toml
include = ["rules/alignment.toml", "rules/qc.toml"]
```

## Environment Specification

A rule declares its environment with an `environment` inline table whose
*key* is the backend and whose value is that backend's argument:

```toml
[rule.analyze]
output = ["results/summary.txt"]
shell = "python analyze.py"
environment = { uv = "requirements.txt" }
```

Supported keys: `uv`, `conda`, `docker`, `apptainer`, `nix`. Omitting
`environment` runs the command on the host as-is.

The table is validated: an `environment` table that names none of those keys
— `environment = { type = "uv", requirements = "…" }`, for instance — or that
carries an extra key beside a recognised backend is a **parse error** naming
the accepted keys. It is never silently dropped.

For `uv`, a value ending in `.toml` is treated as a project file: uv
discovers it on its own, so it is not passed on the command line, but its
bytes (and those of an adjacent `uv.lock`) enter the cache key. Any other
value is a requirements file, passed as `uv run --with-requirements <file>`.

A top-level `environment` table sets the default for every rule that does not
declare its own:

```toml
environment = { uv = "pyproject.toml" }
```

There is no named-environment mechanism: `[env.NAME]` blocks and a rule-level
`env = "NAME"` reference are not part of the format, and — because rule tables
do not reject unknown keys — they are silently ignored rather than reported as
an error. (Keys *inside* an `environment` table are rejected, as above.) See [Environments](../concepts/environments.md) for what each backend
does.

## Next Steps

- [CLI Commands](./commands.md) -- how to run workflows
- [Expression Language](./expressions.md) -- guard and expression syntax
- [Configuration](./configuration.md) -- project-level settings

## Existing wildcard outputs

Output-pattern matches take precedence over disk existence when their inputs
resolve. An existing hand-maintained file may remain a source when an input of
its own producer is missing. Fixed inputs of wildcard producers (such as shared
configuration) and missing sources deeper in the graph remain errors. Known
cached outputs require resolvable inputs or verified adoption. Use
`wildcard_constraints` to exclude hand-maintained names from rule ownership.
See [Existing files and rule ownership](../concepts/rules-and-wildcards.md#existing-files-and-rule-ownership).

Rust callers can use `ox_core::resolver::resolve_with_source_fallback` to supply
an `&dyn Fn(&std::path::Path) -> bool` policy for that fallback. Returning `false`
keeps a known generated output from becoming a source after missing-input
resolution. `resolve` allows the fallback by default. Explicit
`ResolveRequest::existing_files` are always leaves; other errors are propagated.

`resolve_with_source_fallback_at` also accepts a filesystem base directory, keeping
relative path checks independent of process cwd. CLI, MCP and `SessionBuilder`
use `ox_api::resolution::discover_source_files` and `ox_api::resolution::resolve`
to apply the same output filtering and cache-provenance policy relative to the
Oxymakefile directory (`workflow_directory`). An absent cache is an empty store
and permits unrecorded manual sources; a cache that cannot be opened denies
fallback because provenance cannot be checked.
