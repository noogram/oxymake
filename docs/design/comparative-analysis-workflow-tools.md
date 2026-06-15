# Comparative Analysis: Lessons from Workflow Tools for OxyMake v1.0

> **Research bead:** ox-cge6
> **Date:** 2026-03-31
> **Scope:** Snakemake, Nextflow, Bazel, Ninja, DVC, Luigi, Airflow, Prefect, Pants, Buck2, GNU Make

---

## Executive Summary

OxyMake is young. The tools surveyed here have 5-15 years of user feedback, design
iteration, and production battle-scars. This analysis extracts actionable lessons —
bugs to prevent, features to prioritize, design traps to avoid — so OxyMake can
learn from their mistakes rather than repeat them.

**Key findings:**
1. **Cache correctness is the #1 source of trust erosion** across all tools
2. **DSL choice is a one-way door** — every tool that picked a host language regrets it eventually
3. **Dry-run / reason-reporting is table-stakes** — users need to understand "why is this running?"
4. **Process management (signals, zombies, cleanup) is universally underestimated**
5. **Remote/shared cache is the killer feature** separating hobby tools from production tools

**OxyMake's current position is strong:** TOML-not-DSL (ADR-002), content-addressable
cache (ADR-001), atomic write protocol (SIGINT contract), daemon-free cooperative
execution (ADR-005), and pluggable cache validation (ADR-006) already address many
of the hardest problems these tools struggled with for years.

---

## 1. Essential Features We May Be Missing

### 1.1 Dry-Run with Reason Reporting

**What users demand:** "Why is this rule running?" and "What would run if I changed X?"

| Tool | Feature | Notes |
|------|---------|-------|
| Snakemake | `--dry-run`, `--reason`, `--summary` | Shows which rules would run and WHY (input changed, code changed, param changed) |
| Snakemake | `--forcerun`, `--until`, `--omit-from` | Surgical control over what runs |
| Bazel | `bazel query`, `bazel cquery` | Full query language for the dependency graph |
| Ninja | `-n` (dry run), `-v` (verbose commands) | Minimal but effective |
| Make | `-n` (dry run), `-W` (what-if) | `make -nW file.c` shows what would rebuild if file.c changed |

**OxyMake status:** `ox plan` and `ox explain` exist. **Gap:** Need `--reason` flag
that explains WHY each rule is triggered (which specific input changed, code hash
changed, param changed). This is the single most-requested debugging feature across
all tools surveyed.

**Priority: CRITICAL for v1.0**

### 1.2 Partial / Selective Execution

Users need to run subsets of the DAG without modifying the workflow file:

- `--until <rule>` — run everything up to (but not past) a rule
- `--omit-from <rule>` — run everything except downstream of a rule
- `--forcerun <rule>` — force re-execution of specific rules regardless of cache
- `--touch` — mark outputs as up-to-date without executing (Make's `-t`)

**OxyMake status:** `ox run <targets>` exists. `ox invalidate` exists.
**Gap:** `--until`, `--omit-from`, `--touch` equivalents.

**Priority: HIGH for v1.0**

### 1.3 Job Statistics and Progress Reporting

| Tool | Feature |
|------|---------|
| Snakemake | Job stats table (total/done/running per rule), ETA, resource usage |
| Bazel | Build Event Protocol (structured JSON stream of all build events) |
| Ninja | Minimal progress bar `[42/100]`, `-v` for full commands |
| Airflow | Gantt chart, task duration history, SLA monitoring |

**OxyMake status:** `ox top` (TUI), `ox dashboard` (web), `ox status` exist.
**Gap:** Build Event Protocol equivalent — a structured NDJSON event stream that
external tools can consume. This enables CI integration, custom dashboards, and
agent-friendly monitoring without coupling to specific UIs.

**Priority: HIGH for v1.0**

### 1.4 Remote / Shared Cache

Every production build/workflow system eventually needs shared caching:

| Tool | Approach |
|------|----------|
| Bazel | Remote cache (gRPC), remote execution, content-addressable store |
| Pants | Remote cache (Bazel-compatible protocol) |
| Buck2 | RE API (remote execution), content-addressable cache |
| DVC | Git-integrated remote storage (S3, GCS, Azure, SSH) |
| Snakemake | `--cache` with shared filesystem, cloud storage backends |

**OxyMake status:** `ox-storage-local` crate exists. No remote storage yet.
**Gap:** Remote cache backend (S3, GCS, or Bazel-compatible gRPC protocol).
Content-addressable cache (ADR-001) makes this architecturally feasible —
same hash = same artifact, shareable across machines.

**Priority: HIGH for v1.0 (at minimum, design the interface)**

### 1.5 Configuration / Parameterization

| Tool | Approach |
|------|----------|
| Snakemake | `--config key=value`, `configfile:`, profiles |
| Nextflow | `-params-file`, `-c config`, `-profile`, scoped config |
| Bazel | `.bazelrc` files, `--config` named configs |
| DVC | `params.yaml`, parameter dependencies tracked by hash |

**OxyMake status:** Config overrides exist (`--set`). Profiles?
**Gap:** Named profiles (e.g., `ox run --profile ci` vs `--profile dev`) and
parameter files that are tracked as cache key inputs.

**Priority: MEDIUM for v1.0**

### 1.6 Containerized / Isolated Execution

| Tool | Approach |
|------|----------|
| Snakemake | `singularity:`, `conda:` per-rule |
| Nextflow | `container` directive per-process, Docker/Singularity/Podman |
| Bazel | Sandboxed execution (filesystem sandbox) |

**OxyMake status:** `ox-env-uv` for Python environments. No container support.
**Gap:** Per-rule container specification. Important for reproducibility on
shared clusters and CI.

**Priority: MEDIUM for v1.0**

---

## 2. Critical Bugs They Encountered (That We Must Prevent)

### 2.1 Cache Correctness

| Tool | Bug | Root Cause | OxyMake Mitigation |
|------|-----|------------|-------------------|
| Snakemake | Phantom re-runs after `git checkout` | mtime-based change detection | ADR-001: content hash is source of truth, mtime is fast-path only |
| Snakemake | Clock skew on cached directory outputs (#3097) | mtime of symlinked cache doesn't match input mtime | Content hash comparison, not mtime ordering |
| Snakemake | Unnecessary upstream execution for cached rules (#1766) | Cache lookup happens after DAG resolution | Design: check cache BEFORE scheduling upstream |
| Bazel | Unexpected cache misses, non-hermetic actions | Implicit host dependencies leak into cache keys | ADR-001: explicit cache key = `blake3(rule + inputs + params + env + platform)` |
| DVC | Production pipelines re-run everything | Hash hardcoded to local env, not portable | Content-addressable cache with portable keys |
| Nextflow | `-resume` fails when workdir moved/cleaned | Cache tied to absolute work directory paths | Use content hashes, not paths, as cache keys |

**OxyMake's ADR-001 (content-addressable cache) already prevents the majority of
these bugs.** The remaining risk is ensuring the cache key includes ALL relevant
dimensions from day 1 — adding a new dimension later invalidates the entire cache.

**Recommendation:** Audit current cache key to ensure it includes:
- Rule source hash (code that runs)
- Input content hashes (sorted, deterministic)
- Parameter hash (config values)
- Environment specification hash (Python version, container image, etc.)
- Platform identifier (OS, arch)
- Tool version (OxyMake version itself, for format changes)

### 2.2 Process Management

| Tool | Bug | Root Cause | OxyMake Mitigation |
|------|-----|------------|-------------------|
| Snakemake | Zombie processes accumulate (#2354) | Checkpoint + large DAG overwhelms process reaping | Tokio-based async executor with proper child reaping |
| Snakemake | Fork-bombing on PBS clusters (#3804) | Thread count explosion in v9 migration | Explicit thread/process budget, not unbounded spawning |
| Snakemake | SIGTTIN hangs | Children inherit terminal, try to read stdin | Already fixed (ox-mn6a): redirect child stdin to /dev/null |
| Make | Zombie processes with `-j` | Incomplete SIGCHLD handling | Rust's `tokio::process::Command` handles this correctly |
| Airflow | Scheduler process leaks | Long-running daemon accumulates resources | ADR-005: daemon-free, no long-running coordinator |

**OxyMake's daemon-free model (ADR-005) eliminates an entire class of process
management bugs** that plague Airflow, Luigi, and other daemon-based systems.

### 2.3 Concurrency and Race Conditions

| Tool | Bug | Root Cause |
|------|-----|------------|
| Make | Parallel builds race on shared intermediate files | No coordination between `-j` workers |
| Bazel | Non-deterministic outputs from parallel actions | Build rules with hidden shared state |
| Snakemake | Race conditions writing to shared temp directories | Multiple rules using same temp path |

**OxyMake mitigation:** Atomic write protocol (stage -> rename) prevents partial
output visibility. Cooperative SQLite locking (ADR-005) prevents duplicate execution.
**Remaining risk:** User rules that write to shared paths outside OxyMake's control.

### 2.4 Path Handling

| Problem | Tools Affected | OxyMake Status |
|---------|---------------|----------------|
| Spaces in paths | Make (catastrophic), Snakemake (partial) | Needs testing |
| Symlink resolution | Snakemake (#3097), Bazel | Needs policy |
| Relative vs absolute paths | Nearly all tools | TOML paths are relative to Oxymakefile |
| Windows path separators | Make, Snakemake | Not yet relevant (Unix-first) |

**Recommendation:** Add integration tests for spaces in paths and symlinks before v1.0.

---

## 3. Design Decisions They Regret

### 3.1 DSL Choice (The One-Way Door)

| Tool | DSL | Regret |
|------|-----|--------|
| Snakemake | Python-based | No LSP, hard to validate statically, import-time side effects, Python version coupling |
| Nextflow | Groovy-based | Alienates non-JVM users, Groovy's decline, had to create DSL2 and now "strict syntax" to constrain it |
| Bazel | Starlark | Better than Python/Groovy (no side effects) but still a language to learn, high barrier for small projects |
| Make | Custom (tab-sensitive) | "One of the worst design botches in the history of Unix" — Eric S. Raymond |
| Airflow | Python DAGs | DAG parsing overhead (60s+ in production), import-time execution, hard to scale |

**OxyMake's TOML choice (ADR-002) is validated by every tool's experience.** TOML is:
- Parseable in microseconds (no import-time execution)
- Statically analyzable (schema validation, LSP support)
- Language-agnostic (no Python/Groovy/Starlark dependency)
- Not Turing-complete (by design — computation happens in rules, not orchestration)

**The minimal expression language is the risk surface.** Keep it minimal. Every feature
added to the expression language is a step toward becoming a DSL. The expression
language should NEVER get: loops, conditionals beyond ternary, I/O beyond `glob()`,
or user-defined functions.

### 3.2 Complexity Ceiling

| Tool | Problem |
|------|---------|
| Bazel | "Rolling out Bazel at our company took ~1 person-decade of engineering time." 11.2% of adopters abandoned it (median 638 days before giving up). |
| Pants | Steep learning curve for plugin development |
| Buck2 | Migration from Buck1 broke ecosystem, Starlark complexity |
| Airflow | Simple DAG -> production DAG requires deep operational knowledge |

**Lesson for OxyMake:** The "5-minute to first workflow" experience is critical.
If a user can't get a basic pipeline running in under 5 minutes, they'll use Make
or a shell script instead. Bazel's failure with small projects is a cautionary tale.

**Recommendation:** Maintain a "zero-config" mode where `ox init` + a simple
Oxymakefile gets a working pipeline with no additional setup. Advanced features
(remote cache, containers, environments) should be opt-in additions, not prerequisites.

### 3.3 Timestamp-Based Change Detection

Every tool that started with mtime-based change detection eventually regretted it:

- **Snakemake:** Added `--rerun-triggers` to control what triggers re-runs, added code
  change detection, but mtime remains the default for outputs
- **Make:** No content awareness at all — any `touch` triggers rebuild
- **Ninja:** Also timestamp-based, but as an IR it delegates this problem to generators

**OxyMake already solved this (ADR-001).** Content hash as source of truth with
mtime as fast-path optimization is the correct architecture.

### 3.4 Monolithic Architecture

| Tool | Problem | Migration |
|------|---------|-----------|
| Airflow | Scheduler + webserver + worker + DB = complex deployment | Airflow 2.0 split scheduler, 3.0 further modularized |
| Prefect | v1 -> v2 was a complete rewrite (broke all existing workflows) | "Orion" rewrite alienated early adopters |
| Luigi | Central scheduler is SPOF | No real fix, project declining |

**OxyMake's crate-based architecture is correct.** 23 focused crates with clear
boundaries. The daemon-free model (ADR-005) avoids the deployment complexity trap.

---

## 4. UX Patterns That Work

### 4.1 Snakemake: Reason Reporting and Surgical Control

```
$ snakemake --dry-run --reason
rule align:
    reason: Updated input files: data/reads.fastq
rule sort:
    reason: Input files updated by another job
```

Users **love** knowing exactly why something is running. This builds trust in the
system and reduces "just re-run everything" behavior.

**OxyMake equivalent needed:** `ox plan --reason` or `ox explain --verbose`

### 4.2 Bazel: Query Language

```
$ bazel query 'deps(//my:target)'        # All transitive dependencies
$ bazel query 'rdeps(//..., //lib:foo)'   # Everything that depends on foo
$ bazel query 'somepath(//a, //b)'        # Path between two targets
```

A query language turns the build graph into an explorable database. This is
invaluable for large projects and CI integration.

**OxyMake equivalent:** `ox dag` exists but may need richer query capabilities.
Consider: `ox query 'deps(target)'`, `ox query 'why(target)'` for v1.0.

### 4.3 Ninja: Minimal Output by Default

Ninja shows `[42/100] Building foo.o` — one line per action, overwritten in-place.
`-v` shows full commands. This is the gold standard for build output UX.

**OxyMake should adopt this pattern** for `ox run` default output: progress bar
with rule names, expandable to full commands with `-v`.

### 4.4 DVC: Git-Integrated Pipeline

DVC stores pipeline definitions and metrics in git-tracked files (`dvc.yaml`,
`dvc.lock`). Running `dvc repro` reproduces the pipeline; `dvc metrics diff`
shows how metrics changed between git commits.

**Lesson for OxyMake:** The Oxymakefile is already in git. Consider: `ox diff`
that shows which rules changed between git commits, and `ox metrics` for
tracking user-defined metrics across runs.

### 4.5 Nextflow: Resume by Default

Nextflow's `-resume` flag (which most users always enable) skips all tasks
whose inputs haven't changed. The "work directory" caches everything.

**OxyMake already does this by default** via the content-addressable cache.
No special flag needed — this is a UX win.

### 4.6 Airflow: Gantt Chart and Task Duration Tracking

Airflow's Gantt chart view shows task execution over time, making it easy to
identify bottlenecks and parallelism opportunities.

**OxyMake equivalent:** The TUI (`ox top`) could include a timeline view.
The history (`ox history`) should track per-rule duration for trend analysis.

---

## 5. Prioritized Feature Recommendations for v1.0

Based on the analysis above, ranked by impact and effort:

### Tier 1: Must-Have (blocks production adoption)

| # | Feature | Effort | Rationale |
|---|---------|--------|-----------|
| 1 | **Reason reporting** (`--reason` flag) | Medium | #1 requested debugging feature across all tools. "Why is this running?" must be answerable. |
| 2 | **Selective execution** (`--until`, `--omit-from`, `--touch`) | Medium | Surgical DAG control is table-stakes for iterative development. |
| 3 | **Structured event stream** (Build Event Protocol) | Medium | Enables CI integration, agent consumption, custom dashboards without coupling to specific UIs. |
| 4 | **Remote cache interface** (trait + S3/GCS backend) | High | Separates hobby tool from production tool. Design the trait now even if only local backend ships in v1.0. |
| 5 | **Spaces-in-paths and symlink tests** | Low | Every tool has been bitten by this. Prevent before v1.0. |

### Tier 2: Should-Have (significant UX improvement)

| # | Feature | Effort | Rationale |
|---|---------|--------|-----------|
| 6 | **Named profiles** (`--profile ci`, `--profile dev`) | Low | Reduces config friction for multi-environment workflows. |
| 7 | **Graph query** (`ox query 'deps(X)'`, `ox query 'rdeps(X)'`) | Medium | Turns the DAG into an explorable database. Invaluable for large projects. |
| 8 | **Minimal progress output** (Ninja-style `[42/100]`) | Low | Better default output than verbose logging. |
| 9 | **Per-rule duration tracking** in history | Low | Enables bottleneck identification and regression detection. |
| 10 | **Parameter files** (tracked as cache inputs) | Low | DVC's `params.yaml` pattern — config values that invalidate cache when changed. |

### Tier 3: Nice-to-Have (competitive differentiator)

| # | Feature | Effort | Rationale |
|---|---------|--------|-----------|
| 11 | **Per-rule container specification** | High | Important for cluster/CI reproducibility but can be deferred. |
| 12 | **Metrics tracking** (`ox metrics diff`) | Medium | DVC-inspired, useful for ML/data science workflows. |
| 13 | **DAG diff between git commits** (`ox diff`) | Medium | Show which rules changed — useful for code review. |
| 14 | **Watch mode** (`ox watch`) | Medium | File-watcher triggered re-execution for interactive development. |
| 15 | **Workflow linting** (beyond `ox lint`) | Low | Detect common mistakes: circular deps, missing inputs, unused rules. |

---

## 6. Anti-Patterns to Avoid

Based on failures observed across tools:

1. **Don't add a Turing-complete expression language.** Snakemake's Python DSL,
   Nextflow's Groovy, Airflow's Python DAGs — all became complexity traps.
   TOML + minimal expressions is the right call. Guard this boundary aggressively.

2. **Don't require a daemon.** Luigi's central scheduler and Airflow's scheduler
   are SPOFs and operational burdens. ADR-005 is correct.

3. **Don't use timestamps as source of truth.** Every tool regrets this.
   ADR-001 is correct.

4. **Don't make the default config complex.** Bazel's 11.2% abandonment rate
   is driven by complexity. "5 minutes to first workflow" must be sacred.

5. **Don't break backwards compatibility without a migration path.**
   Nextflow's DSL1->DSL2, Prefect v1->v2, and Buck1->Buck2 all alienated users.
   When breaking changes are needed, provide `ox translate` (already exists!)
   or versioned Oxymakefile formats.

6. **Don't treat process management as an afterthought.** Zombie processes,
   signal handling, stdin inheritance — these are the bugs that make users
   lose trust. OxyMake's SIGINT contract and stdin redirect (ox-mn6a) show
   the right level of care here.

7. **Don't ignore Windows.** Make never supported it properly. Even if OxyMake
   is Unix-first, avoid hard-coding assumptions (path separators, signals)
   that make Windows support impossible later.

---

## 7. What OxyMake Already Gets Right

Credit where due — OxyMake's existing architecture already addresses many of
the hardest problems these tools struggled with:

| Problem Domain | Industry Pain | OxyMake Solution |
|---------------|--------------|-----------------|
| Change detection | mtime phantom re-runs | ADR-001: content hash + mtime fast-path |
| DSL complexity | Python/Groovy/Starlark barriers | ADR-002: TOML, not a DSL |
| Language interop | Tight coupling to Python runtime | ADR-003: subprocess + Arrow IPC |
| Daemon overhead | SPOF schedulers, deployment complexity | ADR-005: daemon-free cooperative |
| Cache flexibility | One-size-fits-all validation | ADR-006: pluggable cache validation |
| Signal handling | Zombies, partial outputs, SIGTTIN | SIGINT contract + ox-mn6a fix |
| Concurrent execution | Race conditions, lock contention | Atomic write protocol + SQLite coordination |

**The foundation is solid. The gaps are in UX (reason reporting, query, profiles)
and production infrastructure (remote cache, containers), not architecture.**

---

## Appendix: Tool-by-Tool Deep Dives

### A. Snakemake

**Strengths:** Python familiarity, rich ecosystem (wrappers, conda integration),
excellent dry-run/reason reporting, `--forcerun`/`--until`/`--omit-from`.

**Weaknesses:** Python DSL limits tooling, mtime-based detection causes phantom
re-runs, DAG resolution slow for large workflows (#2354: "very slow on large
workflow"), zombie process accumulation, fork-bombing on cluster managers (#3804).

**Usage trend:** Declining (27% -> 17% in bioinformatics surveys 2021-2024),
being replaced by Nextflow in many organizations.

**Key lessons for OxyMake:**
- Reason reporting is the #1 most-loved feature — implement it
- `--forcerun`/`--until`/`--omit-from` are essential for iterative development
- Cache correctness issues with directory outputs and symlinks are subtle
- Large DAG performance (>10K rules) needs explicit attention

### B. Nextflow

**Strengths:** Elegant resume functionality, channel-based dataflow, strong cloud
support (AWS Batch, Google Life Sciences), large ecosystem (nf-core).

**Weaknesses:** Groovy DSL alienates users, DSL1->DSL2 migration was painful,
Groovy language declining in relevance, resume tied to absolute work directory paths.

**Key lessons for OxyMake:**
- Resume/cache should work regardless of directory location (content hash, not paths)
- Channel-based dataflow is powerful but complex — OxyMake's file-based approach is simpler
- Strict syntax evolution shows the cost of starting with too-permissive DSL
- Strong cloud/container integration drives adoption in bioinformatics

### C. Bazel

**Strengths:** Remote cache + remote execution, query language, hermeticity model,
build event protocol, massive scale capability.

**Weaknesses:** Extreme complexity (1 person-decade to deploy), 11.2% abandonment
rate, Starlark learning curve, poor experience for small projects, non-hermetic
builds common in practice despite design intent.

**Key lessons for OxyMake:**
- Query language is transformative for large projects — implement basic version
- Build Event Protocol enables rich tooling ecosystem — design structured events
- Remote cache is the killer production feature — design the interface early
- Complexity is the enemy of adoption — keep the simple case simple

### D. Ninja

**Strengths:** Extreme speed, minimal output UX, clean IR model (build.ninja),
widely used as backend (CMake, Meson, GN, Chromium).

**Weaknesses:** Not meant for direct use, no features beyond basic builds,
no sub-second timestamp resolution, `.DELETE_ON_ERROR` equivalent never added.

**Key lessons for OxyMake:**
- Minimal progress output `[42/100]` is the gold standard — adopt it
- IR model (separation of frontend/backend) is architecturally sound
- Speed matters — users notice 100ms vs 10s for no-op builds
- `.DELETE_ON_ERROR` (clean up failed outputs) — OxyMake already does this via atomic writes

### E. DVC

**Strengths:** Git integration, experiment tracking, metrics diffing, parameter
file tracking, remote storage for large artifacts.

**Weaknesses:** Production deployment issues (hashes hardcoded to local env),
pipeline flexibility limited to disk I/O, poor handling of many small files,
`dvc pull` struggles with large datasets.

**Key lessons for OxyMake:**
- `params.yaml` pattern (tracked parameter files) is elegant — implement it
- Metrics tracking across runs is valuable for data science workflows
- Git integration for pipeline state is natural — Oxymakefile already in git
- Large file handling needs explicit design (hash-based dedup, chunk storage)

### F. Luigi

**Strengths:** Simple Python API, clear task dependency model, early mover.

**Weaknesses:** Central scheduler is SPOF, limited scalability, minimal monitoring,
project declining in usage, no dynamic task generation.

**Key lessons for OxyMake:**
- Central scheduler = fragile architecture (confirmed by ADR-005 decision)
- Simple API matters — don't over-engineer the workflow definition format
- Project decline shows importance of active community and ecosystem

### G. Airflow

**Strengths:** Largest community, rich UI (Gantt chart, tree view, task logs),
extensive operator ecosystem, mature scheduling.

**Weaknesses:** DAG parsing overhead (60s+ in production), scheduler complexity,
heavy deployment (DB + webserver + scheduler + worker), Python DAG import-time
execution causes bugs, Airflow 3.0 still addressing fundamental architecture issues.

**Key lessons for OxyMake:**
- DAG parsing must be fast (microseconds, not seconds) — TOML parsing achieves this
- Gantt chart / timeline view is beloved — consider for `ox top` or `ox dashboard`
- Task duration history enables regression detection — track in `ox history`
- Don't require a database server for basic operation — SQLite is correct

### H. Prefect

**Strengths:** Developer-friendly API, good observability, flexible deployment,
strong Python ecosystem integration.

**Weaknesses:** v1->v2 complete rewrite broke all existing workflows, alienated
early adopters, frequent API changes, heavier than needed for simple use cases.

**Key lessons for OxyMake:**
- Breaking changes without migration paths destroy trust
- `ox translate` already exists — maintain it as the migration tool
- Versioned Oxymakefile format with explicit version field

### I. Pants

**Strengths:** User-friendly for Python/JVM monorepos, good dependency inference,
Bazel-compatible remote cache, active development.

**Weaknesses:** Steep plugin development curve, primarily Python ecosystem,
bootstrap overhead, limited language support outside Python/JVM/Go.

**Key lessons for OxyMake:**
- Dependency inference (auto-detecting inputs) is powerful UX
- Plugin system design matters — keep it simple or don't have one
- Remote cache compatibility with Bazel protocol is strategic

### J. Buck2

**Strengths:** Fast incremental builds, Starlark-based rules, remote execution,
modern Rust implementation.

**Weaknesses:** Meta-centric (hard to use outside Meta's ecosystem), limited
community, Starlark complexity, Buck1->Buck2 migration broke ecosystem, sparse
documentation.

**Key lessons for OxyMake:**
- Rust implementation validates OxyMake's language choice for performance
- Open-sourcing without community investment leads to low adoption
- Documentation quality directly impacts adoption — invest early

### K. GNU Make

**Strengths:** Universal availability, simple mental model (targets, prerequisites,
recipes), 48 years of proven reliability, zero dependencies.

**Weaknesses:** Tab sensitivity ("worst design botch in Unix history"), weak
parallelism (`-j` races on shared intermediates), no content-based detection,
no Windows support, no wildcard/glob in prerequisites, recursive Make considered harmful.

**Key lessons for OxyMake:**
- Simplicity and availability drive adoption — Make persists because it's everywhere
- Tab sensitivity is a permanent warning about syntax decisions
- "Recursive Make Considered Harmful" — flat namespace is better
- `make -n` (dry run) and `make -W` (what-if) are still best-in-class UX patterns
