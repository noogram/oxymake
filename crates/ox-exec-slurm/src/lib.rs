//! # SLURM Executor
//!
//! Submits OxyMake jobs to SLURM clusters. Supports two modes:
//!
//! - **CLI mode** (default): shells out to `sbatch`/`sacct`/`squeue`.
//! - **REST mode**: talks to `slurmrestd` via HTTP/JSON (v0.0.40+).
//!
//! Select REST mode by setting `SlurmConfig::api_url` or passing
//! `--slurm-api http://host:6820` on the command line.
//!
//! ## Architecture
//!
//! - **Job submission**: generates a bash script with `#SBATCH` directives,
//!   submits via `sbatch --parsable` (CLI) or `POST /slurm/v0.0.40/job/submit`
//!   (REST), captures the SLURM job ID.
//! - **Status polling**: adaptive backoff using `sacct`/REST job endpoint
//!   (primary) with `squeue` fallback (CLI only).
//! - **Cancellation**: `scancel` (CLI) or `DELETE /slurm/v0.0.40/job/{id}` (REST).
//!
//! ## State.db constraint (ADR-004)
//!
//! The scheduling process must run on a node with local disk — SQLite WAL
//! does not work on NFS/Lustre/GPFS. Compute nodes access workflow data
//! via shared filesystems but never touch `state.db`.

pub mod error;
pub mod executor;
pub mod job_array;
pub mod job_script;
pub mod resource_mapper;
pub mod slurm_cli;
pub mod slurm_rest;
pub mod status_parser;

pub use error::SlurmError;
pub use executor::SlurmConfig;
pub use executor::SlurmExecutor;
pub use job_array::{JobArrayConfig, JobArraySpec};
pub use slurm_rest::SlurmRestClient;
