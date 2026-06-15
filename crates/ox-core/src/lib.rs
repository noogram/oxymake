//! # ox-core — The Engine of OxyMake
//!
//! This crate is the kernel of oxymake's workflow orchestrator: the pure
//! mechanics that take a *declared* workflow and turn it into a *runnable*
//! schedule of jobs. It owns the data model, the three graph
//! representations, wildcard resolution, the backward-chaining DAG resolver,
//! the topological scheduler, and the trait definitions for every plugin
//! axis (executors, storage, environments, reporters, codecs).
//!
//! Founding sentence: *« Rust provides the engine. The workflow provides
//! the intent. »* This crate is the engine — it never decides *what* to
//! compute, only *how* to orchestrate computations declared elsewhere.
//!
//! ## Five-minute mental model
//!
//! Picture the smallest possible workflow. One rule, two files:
//!
//! ```toml
//! # Oxymakefile.toml
//! [rule.sort]
//! input  = ["data/{sample}.csv"]
//! output = ["results/{sample}.txt"]
//! shell  = "sort {input} > {output}"
//! ```
//!
//! When the operator runs `ox run`, the engine takes that declaration
//! through three transformations. Each transformation is its own graph
//! type, defined in this crate, with its own invariants.
//!
//! ```text
//!   Oxymakefile.toml
//!         │
//!         │  parse + validate
//!         ▼
//!     RuleGraph                  (logical — rules with wildcards)
//!         │
//!         │  resolve wildcards    ← driven by available inputs
//!         │  evaluate guards      ← `when = "..."` clauses
//!         │  optimize             ← critical-path, pruning passes
//!         ▼
//!     JobGraph                   (physical — concrete jobs, bipartite)
//!         │
//!         │  attach runtime state ← statuses, retries, metrics
//!         ▼
//!     ExecGraph                  (runtime — what the scheduler dispatches)
//! ```
//!
//! Concretely: in the example above, `{sample}` is a wildcard. The
//! `RuleGraph` knows there *exists* a rule that maps `data/X.csv` to
//! `results/X.txt` for any `X`. The resolver scans the filesystem (or
//! the cache), finds `data/A.csv`, `data/B.csv`, `data/C.csv`, and
//! produces a `JobGraph` with three concrete `ConcreteJob` instances.
//! The scheduler then walks the `JobGraph`, dispatches ready jobs to an
//! `Executor`, and as state arrives back it lives on the `ExecGraph`.
//!
//! Once you can see that pipeline in your head, the rest of the crate
//! is named after pieces of it. `dag` builds and traverses the
//! `RuleGraph`; `resolver` expands wildcards; `job_graph` materializes
//! the bipartite job/output graph; `scheduler` dispatches; `event` is
//! the unidirectional state-change protocol that everything emits.
//!
//! ## What lives where
//!
//! - [`model`] — the core types every other crate depends on: `Rule`,
//!   `ConcreteJob`, `OutputRef`, `ResourceRequest`. If you are adding a
//!   field that *every* job carries, it lives here.
//! - [`dag`] / [`resolver`] / [`job_graph`] — the three graph
//!   constructions, in order: rules-with-wildcards → resolved-jobs →
//!   bipartite job/output graph used at run-time.
//! - [`scheduler`] — the topological dispatcher. Owns the in-memory
//!   `Frontier` and emits one `event::Event` per state transition.
//! - [`event`] — the only public protocol between this crate and
//!   downstream observers. State flows one way; nothing reads back
//!   into the scheduler.
//! - [`traits`] — every plugin axis is a trait here. `Executor`,
//!   `Storage`, `EnvironmentProvider`, `Reporter`, `FormatCodec`. If
//!   you are writing a new backend, you are implementing one of these.
//! - [`wildcard`] — pattern matching used by the resolver.
//! - [`disk_writer`] / [`memory_map`] — output materialization
//!   helpers. Where the engine touches the filesystem, it goes through
//!   these.
//!
//! ## What this crate never does
//!
//! - No file I/O, network access, or UI rendering. Those live in
//!   `ox-exec-*`, `ox-cache-*`, `ox-monitor-*`, `ox-dashboard`.
//! - No hardcoded thresholds. The engine never decides *"this job is
//!   stalled"* based on elapsed time; it exposes the duration, the
//!   observer decides.
//! - No heuristics about whether a job *should* re-run. The content
//!   hash says yes or no, deterministically. That is the cache's job
//!   (see [ADR-001]).
//! - No interpretation of intent. The workflow declares; the engine
//!   executes faithfully.
//!
//! ## Three reads before you change anything in this crate
//!
//! - [ADR-001 — Content-Addressable Cache as Source of Truth][ADR-001].
//!   Explains why the cache key is `blake3(rule + sorted inputs +
//!   params + env + platform)` and why timestamps are a fast-path
//!   only. Constrains every "should I re-run?" decision.
//! - [ADR-011 — Three-Stage State Pipeline][ADR-011]. Explains the
//!   `Frontier → EventBus → Reporter/Ledger` flow. Why state lives in
//!   three stages instead of one shared structure. Constrains every
//!   change to the scheduler or the event types.
//! - [ADR-015 — Named Invariants and TLA+ Scope][ADR-015]. Lists the
//!   live invariants (`OX-1 … OX-7`, `INV-2`, `INV-3`) and which
//!   `.tla` modules defend which axiom. If you touch the cancel path,
//!   the cache, the executor bridge, or recovery, read this first.
//!
//! For the broader operator-facing entry point (returning after a
//! pause, or a fresh agent), see [`RETURNING.md`][RETURNING] at the
//! repository root. For the full design vision, see the
//! [Founding Thesis][THESIS].
//!
//! [ADR-001]: https://github.com/noogram/oxymake/blob/main/docs/adr/001-content-addressable-cache.md
//! [ADR-011]: https://github.com/noogram/oxymake/blob/main/docs/adr/011-three-layer-state-architecture.md
//! [ADR-015]: https://github.com/noogram/oxymake/blob/main/docs/adr/015-named-invariants.md
//! [RETURNING]: https://github.com/noogram/oxymake/blob/main/docs/RETURNING.md
//! [THESIS]: https://github.com/noogram/oxymake/blob/main/OXYMAKE-THESIS.md

pub mod dag;
pub mod disk_writer;
pub mod error;
pub mod event;
pub mod hashing;
pub mod job_graph;
pub mod memory_map;
pub mod model;
pub mod resolver;
pub mod scheduler;
pub mod traits;
pub mod wildcard;

// Re-export OrderedFloat so downstream crates can construct ResourceValue::Float
// without adding ordered-float as a direct dependency.
pub use ordered_float::OrderedFloat;
