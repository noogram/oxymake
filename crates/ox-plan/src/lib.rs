//! # ox-plan — Optimization Passes on JobGraph
//!
//! This crate transforms a raw `JobGraph` into an optimized execution plan.
//! It sits between DAG construction (ox-core) and execution (ox-exec-*),
//! applying graph-level optimizations that reduce total build time.
//!
//! ## Architecture
//!
//! ```text
//!   ox-core::resolver          ox-plan                    ox-exec
//!   ┌────────────────┐    ┌──────────────────┐    ┌──────────────────┐
//!   │ Vec<ConcreteJob>│──▶│ JobGraph::build() │──▶│ ExecGraph::new() │
//!   └────────────────┘    │                  │    └──────────────────┘
//!                         │  run_passes(     │
//!                         │    cache_prune,  │
//!                         │    crit_path,    │
//!                         │    partition,    │
//!                         │  )              │
//!                         └──────────────────┘
//! ```
//!
//! ## Crate responsibilities
//!
//! - **Subgraph pruning**: remove jobs whose outputs are already up-to-date
//! - **Critical-path analysis**: identify and prioritize the longest chain
//! - **Resource-aware partitioning**: respect memory/CPU constraints
//! - **Pass pipeline**: compose multiple optimization passes in order
//!
//! ## What this crate NEVER does
//!
//! - Execute jobs or interact with the filesystem
//! - Parse workflow files (that's ox-format)
//! - Make caching decisions (that's ox-cache)
//!
//! ## Key types
//!
//! - [`pass::OptimizationPass`] — the trait all passes implement
//! - [`pass::PassResult`] — metadata about a pass execution
//! - [`pass::run_passes`] — run a pipeline of passes
//! - [`prune::CachePruningPass`] — marks cached jobs as skipped

pub mod critical_path;
pub mod error;
pub mod partition;
pub mod pass;
pub mod prune;

// Re-export the most commonly used types for convenience.
pub use critical_path::{CriticalPathPass, compute_critical_path};
pub use error::PlanError;
pub use pass::{OptimizationPass, PassResult, run_passes};
pub use prune::CachePruningPass;
