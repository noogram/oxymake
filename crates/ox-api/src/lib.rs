//! # ox-api — Public Rust API Facade
//!
//! This crate is the single entry point for anyone embedding OxyMake as a
//! library. It composes ox-core, ox-format, ox-state, ox-cache, and ox-plan
//! behind a clean, stable API surface.
//!
//! ## Quick start
//!
//! ```no_run
//! use ox_api::SessionBuilder;
//!
//! let session = SessionBuilder::new("Oxymakefile.toml")
//!     .targets(["output.txt"])
//!     .build()
//!     .unwrap();
//!
//! // Inspect the plan (dry run).
//! for job in ox_api::plan(&session).unwrap() {
//!     println!("{}: {:?} -> {:?}", job.id, job.inputs, job.outputs);
//! }
//! ```
//!
//! ## What this crate provides
//!
//! - [`SessionBuilder`]: configure and resolve a build with sensible defaults
//! - [`Session`]: the resolved build graph, ready for inspection
//! - [`plan`]: topologically-ordered job list for dry-run analysis
//! - [`ApiError`]: unified error type across all internal crates
//! - Re-exports of the key types callers need from inner crates
//!
//! ## What this crate NEVER does
//!
//! - Implement low-level mechanics (delegates to inner crates)
//! - Provide CLI argument parsing (that's ox-cli)
//! - Make policy decisions — all behavior is configurable

pub mod builder;
pub mod discover;
pub mod error;
pub mod run;

// ── Convenience re-exports ──────────────────────────────────────────────

pub use builder::{Session, SessionBuilder};
pub use error::ApiError;
pub use run::plan;

// ── Key types from inner crates ─────────────────────────────────────────

// Core model types that callers interact with.
pub use ox_core::model::{
    ConcreteJob, ContentHash, ErrorStrategy, Event, ExecutionBlock, JobId, OutputRef, Rule,
    RuleName,
};

// Graph types.
pub use ox_core::dag::RuleGraph;
pub use ox_core::job_graph::JobGraph;

// Resolver types.
pub use ox_core::resolver::{Config, ResolveRequest, ResolveResult};

// Event bus.
pub use ox_core::event::EventBus;

// Workflow representation.
pub use ox_format::parse::{ConfigValue, Gate, Workflow};

// Cache.
pub use ox_cache::{CacheEntry, CacheStore};

// Optimization passes.
pub use ox_plan::{CachePruningPass, OptimizationPass, PassResult, run_passes};

// File discovery.
pub use discover::discover_existing_files;
