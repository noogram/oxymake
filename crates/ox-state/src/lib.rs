//! # ox-state — the Ledger (ADR-011 Stage 3)
//!
//! This crate owns all durable state that survives between OxyMake
//! invocations: build sessions, job outcomes, content hashes, and
//! timing data. In the project vocabulary, it
//! is the **Ledger** stage of the three-stage pipeline
//!
//! ```text
//! Frontier ──► EventBus ──► EventSink ──► Ledger
//! ```
//!
//! The Rust entry point is [`StateDb`](db::StateDb) ; the alias
//! [`Ledger`] is provided so downstream code can adopt the new
//! vocabulary without a flag-day rename. Both names refer to the same
//! type.
//!
//! ## Three state concerns
//!
//! 1. **Execution state** (ephemeral, reconstructible from DAG + cache):
//!    what is running, pending, or done right now.  Enables crash recovery
//!    and cooperative multi-session execution via atomic job claiming.
//!
//! 2. **Cache metadata**: content hashes and output hashes that `ox-cache`
//!    queries to decide whether a job can be skipped.
//!
//! 3. **Audit trail** (append-only, never pruned): a history of every job
//!    execution — wall time, peak memory, exit code, input/output hashes.
//!    This transforms `.oxymake/state.db` into a lightweight research lab
//!    notebook.
//!
//! ## Crate responsibilities
//!
//! - Schema creation and forward-only migrations ([`migration`])
//! - Session lifecycle: create, heartbeat, seal, stale detection ([`session`])
//! - Job registration, atomic claiming, status transitions ([`db`])
//! - Error types for all persistence operations ([`error`])
//!
//! ## What this crate NEVER does
//!
//! - Cache invalidation logic (that belongs in `ox-cache`)
//! - File I/O beyond the SQLite database file
//! - Build orchestration or scheduling
//!
//! ## Quick start
//!
//! ```
//! # use tempfile::NamedTempFile;
//! use ox_state::db::{StateDb, JobRecord};
//!
//! let tmp = NamedTempFile::new().unwrap();
//! let db = StateDb::open(tmp.path()).unwrap();
//!
//! // Create a session.
//! let sid = db.create_session(std::process::id(), "localhost", None).unwrap();
//!
//! // Register jobs.
//! let jobs = vec![
//!     JobRecord { id: "align-A".into(), rule_name: "align".into(),
//!                 wildcards: r#"{"sample":"A"}"#.into(), cache_key: None, run_id: None },
//! ];
//! db.register_jobs(&jobs).unwrap();
//!
//! // Claim and complete a job.
//! assert!(db.claim_job("align-A", &sid).unwrap());
//! db.complete_job("align-A", &sid, 0, r#"{"out.bam":"deadbeef"}"#).unwrap();
//!
//! // Verify.
//! assert_eq!(db.job_status("align-A").unwrap().as_deref(), Some("completed"));
//! db.complete_session(&sid).unwrap();
//! ```

pub mod backend;
pub mod db;
pub mod error;
pub mod migration;
pub mod session;

/// The persisted audit store — alias for [`db::StateDb`].
///
/// Vocabulary alias introduced by the vocabulary disambiguation pass.
/// New code is encouraged to refer to the persisted
/// audit store as `Ledger` ; legacy code may keep using `StateDb` — the two
/// names refer to the same type. See ADR-011 for the three-stage pipeline.
pub type Ledger = db::StateDb;
