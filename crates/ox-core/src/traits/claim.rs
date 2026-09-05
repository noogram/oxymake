//! Job claim trait for the scheduler (cooperative multi-session, ADR-012).
//!
//! Two `ox run` sessions on the same workspace can both decide that a job
//! needs to run. The [`JobClaim`] trait is how the scheduler asks, *before*
//! dispatching a job, whether another session has already taken it. The
//! production implementation lives in `ox-state` and is backed by the atomic
//! `UPDATE … WHERE status = 'pending'` of the `jobs` table; `ox-core` only
//! defines the contract, exactly like [`GateCheck`](super::gate::GateCheck).
//!
//! # Protocol as the scheduler drives it
//!
//! 1. When a job becomes ready, the scheduler calls [`JobClaim::claim`].
//!    [`ClaimOutcome::Won`] means this session owns the job and launches it.
//!    [`ClaimOutcome::Lost`] means a peer session owns it: the job is **not**
//!    launched here.
//! 2. A lost job is polled with [`JobClaim::peer_state`] until the owner
//!    records a terminal state, which the waiting session then *consumes* as
//!    its own outcome — `Completed` promotes the downstream jobs as if the
//!    job had run here, `Failed`/`Cancelled` are mirrored.
//! 3. [`PeerJobState::Unclaimed`] means the row is up for grabs again (the
//!    owner's lease expired and its jobs were reclaimed, or the owner never
//!    actually started): the scheduler re-enters step 1.
//!
//! The default [`NoClaims`] always wins, which is the single-session
//! behaviour.

use std::future::Future;
use std::pin::Pin;

use crate::model::JobId;

/// Result of trying to claim a job for this session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// This session owns the job and must execute it. Claiming a job this
    /// session already owns is also `Won` (the claim is idempotent per
    /// session), so a job deferred after a successful claim can be claimed
    /// again on the next dispatch attempt.
    Won,
    /// Another session owns the job (or has already terminalized it). The
    /// scheduler must not launch it and should wait for the owner's
    /// terminal state through [`JobClaim::peer_state`].
    Lost {
        /// Identifier of the owning session, when the implementation knows it.
        owner: Option<String>,
    },
}

/// What the shared state records about a job owned by a peer session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerJobState {
    /// The owner is alive (its lease has not expired) and the job has not
    /// reached a terminal state yet: keep waiting.
    Running,
    /// The owner recorded a successful completion. The outputs are committed.
    Completed,
    /// The owner recorded a failure with this exit code.
    Failed {
        /// Exit code recorded by the owner (non-zero).
        exit_code: i32,
    },
    /// The owner cancelled the job.
    Cancelled,
    /// Nobody owns the job any more (the owner's lease expired and its jobs
    /// were reclaimed, or the row was reset): the scheduler may claim it.
    Unclaimed,
}

/// Claim protocol the scheduler consults before dispatching a job.
///
/// Object-safe and `Send + Sync` for use with `Arc` across async tasks.
pub trait JobClaim: Send + Sync {
    /// Atomically claim `job_id` for this session.
    fn claim<'a>(
        &'a self,
        job_id: &'a JobId,
    ) -> Pin<Box<dyn Future<Output = ClaimOutcome> + Send + 'a>>;

    /// Observe the state of a job this session lost the claim for.
    ///
    /// Implementations are expected to enforce the owner's lease here: when
    /// the owner's heartbeat is stale its jobs are reclaimed and the result
    /// is [`PeerJobState::Unclaimed`].
    fn peer_state<'a>(
        &'a self,
        job_id: &'a JobId,
    ) -> Pin<Box<dyn Future<Output = PeerJobState> + Send + 'a>>;
}

/// No-op implementation: every claim is won.
///
/// Used when no shared state is configured (single-session execution).
pub struct NoClaims;

impl JobClaim for NoClaims {
    fn claim<'a>(
        &'a self,
        _job_id: &'a JobId,
    ) -> Pin<Box<dyn Future<Output = ClaimOutcome> + Send + 'a>> {
        Box::pin(async { ClaimOutcome::Won })
    }

    fn peer_state<'a>(
        &'a self,
        _job_id: &'a JobId,
    ) -> Pin<Box<dyn Future<Output = PeerJobState> + Send + 'a>> {
        Box::pin(async { PeerJobState::Unclaimed })
    }
}
