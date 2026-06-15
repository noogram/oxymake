# OxyMake Glossary

Centralized definitions for key terms used across documentation, ADRs, and the
codebase. Entries are alphabetical.

---

**ADR** — Architecture Decision Record. A lightweight document capturing a
significant design decision, its context, alternatives considered, and
consequences. Stored in `docs/adr/` and numbered sequentially (e.g.,
ADR-001). See [ADR-000 (template)](adr/000-template.md).

**Call mode** — An execution mode where a job invokes a Python (or other
language) function directly via Arrow IPC, rather than running a shell command.
Enables zero-copy data passing and in-memory chaining. Contrast with *shell
mode*. See [ADR-003](adr/003-subprocess-arrow-not-pyo3.md).

**ConcreteJob** — A fully resolved job instance with all wildcards expanded,
inputs resolved, and guards evaluated. The unit that `JobGraph` operates on.
Defined in `ox-core::model::ConcreteJob`.

**Content-addressable cache** — The caching strategy where outputs are keyed by
a hash of inputs, commands, and environment rather than by timestamps. See
[ADR-001](adr/001-content-addressable-cache.md).

**Critical path** — The longest chain of dependent jobs in the DAG. Optimizing
critical-path latency is the primary lever for reducing end-to-end wall-clock
time. Identified by `CriticalPathPass` in `ox-plan`.

**ExecGraph** — The third and final graph representation. The `JobGraph`
annotated with runtime execution state (job status, metrics, retry counts).
Exists only during a run and is consumed by executor backends. Defined in
`ox-core`.

**Executor** — A backend that runs concrete jobs. OxyMake ships with three
executor crates: `ox-exec-local` (subprocess), `ox-exec-ray` (Ray cluster),
and `ox-exec-slurm` (Slurm HPC scheduler).

**ExecutorBridge** — A bidirectional adapter between OxyMake's scheduler and
remote executor backends. Translates between OxyMake's job model and the
executor's native protocol. See [ADR-008](adr/008-executor-bridge.md).

**Gate** — A named validation step that runs after specified jobs complete.
Gates enforce invariants (e.g., data quality checks) and can block downstream
execution. Defined in `ox-core::model::Gate`.

**JobGraph** — The second graph representation. The `RuleGraph` after wildcard
resolution, guard evaluation, and optimization passes. One node per concrete
job instance, one node per concrete output path. Bipartite directed graph.
Defined in `ox-core::job_graph::JobGraph`.

**MaterializePolicy** — Controls when a job output is written to disk. Four
variants:
- `Always` — always write to disk and cache (default).
- `Auto` — materialize only if a non-call-mode downstream consumer needs the
  file.
- `Never` — keep in memory only; not cached, lost if the process dies.
- `Final` — materialize only if the output is a DAG leaf (final result).

Defined in `ox-core::model::MaterializePolicy`.

**Optimization pass** — A transformation applied to the `JobGraph` before
execution. Passes include cache pruning, critical-path analysis, and
resource-aware partitioning. Composed via `ox-plan::pass::run_passes`.

**OutputLifecycle** — Controls the lifespan of an output file. Variants:
`Permanent` (kept indefinitely), `Temporary` (cleaned after run), `Protected`
(exempt from automatic cleanup). Defined in `ox-core::model::OutputLifecycle`.

**Oxymakefile** — The TOML-format workflow definition file parsed by
`ox-format`. Declares rules, their inputs/outputs, commands, and metadata.
See [ADR-002](adr/002-toml-not-dsl.md).

**ox-plan** — The optimization crate that sits between DAG construction
(`ox-core`) and execution (`ox-exec-*`). Transforms a raw `JobGraph` into an
optimized execution plan via composable passes.

**Rule** — A template for producing outputs from inputs. Rules contain wildcard
patterns that expand into concrete jobs during resolution. The building block
of an Oxymakefile. Defined in `ox-core::model::Rule`.

**RuleGraph** — The first of OxyMake's three graph representations. A bipartite
directed graph of rules and their pattern-level dependencies *before* wildcard
resolution. Compact and abstract: one node per rule, one node per unique
pattern string. Enables cycle detection and structural validation. Defined in
`ox-core::dag::RuleGraph`.

**Shell mode** — An execution mode where a job runs as a subprocess shell
command. The default execution mode. Contrast with *call mode*.

**Wildcard** — A `{name}` placeholder in rule patterns that expands to concrete
values during resolution. Wildcards enable a single rule definition to produce
multiple jobs. Defined in `ox-core::wildcard`.
