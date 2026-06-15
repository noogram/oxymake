//! # ox-exec-local — Local Process Executor
//!
//! This crate implements the `Executor` trait from ox-core by spawning
//! child processes on the local machine via `tokio::process::Command`.
//!
//! ## Crate responsibilities
//!
//! - Spawn shell commands as async child processes
//! - Capture stdout/stderr and exit status
//! - Enforce per-job timeouts and resource limits
//! - Stream execution events back to the scheduler
//!
//! ## What this crate NEVER does
//!
//! - Decide *which* jobs to run (that's the scheduler)
//! - Remote execution or container isolation
//! - Cache lookups or skip decisions

pub mod call_mode;
pub mod error;
pub mod executor;
pub mod process;
pub mod worker_pool;
