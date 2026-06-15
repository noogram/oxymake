//! # ox-lock — Reproducibility Lockfile
//!
//! This crate generates and verifies `ox.lock`, a TOML lockfile that captures
//! the exact state needed to reproduce a workflow run: Oxymakefile hash,
//! resolved rule versions, environment specs, input file hashes, and platform
//! info.
//!
//! ## Crate responsibilities
//!
//! - Define the lockfile schema (TOML-serializable data model)
//! - Write the lockfile after a successful `ox run`
//! - Read and verify an existing lockfile against current state
//! - Report drift between locked and current state
//!
//! ## What this crate NEVER does
//!
//! - Execute builds or schedule jobs
//! - Modify the cache or state database

pub mod error;
pub mod model;
pub mod verify;
pub mod writer;

pub use error::LockError;
pub use model::Lockfile;
pub use verify::{DriftReport, verify_lockfile};
pub use writer::write_lockfile;
