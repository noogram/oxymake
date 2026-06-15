//! Gate check trait for the scheduler.
//!
//! The [`GateCheck`] trait allows the scheduler to query whether a gate
//! has been approved. The default implementation (`NoGates`) always
//! returns `Approved`, meaning gates are not enforced unless a concrete
//! implementation is provided (e.g., backed by `ox-state`'s `gates` table).

use std::future::Future;
use std::pin::Pin;

use crate::model::GateId;

/// Status of a gate as seen by the scheduler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateStatus {
    /// The gate is approved — downstream jobs may proceed.
    Approved,
    /// The gate is pending — downstream jobs must wait.
    Pending,
    /// The gate has been rejected — downstream jobs should be cancelled.
    Rejected,
    /// The gate was not found — treat as if it doesn't exist (proceed).
    NotFound,
}

/// A gate checker that the scheduler consults before dispatching jobs
/// blocked by gate nodes.
///
/// Object-safe and `Send + Sync` for use with `Arc` across async tasks.
pub trait GateCheck: Send + Sync {
    /// Query the current status of a gate.
    fn check_gate<'a>(
        &'a self,
        gate_id: &'a GateId,
    ) -> Pin<Box<dyn Future<Output = GateStatus> + Send + 'a>>;

    /// Register a gate as pending (called when the scheduler first encounters it).
    fn register_gate<'a>(
        &'a self,
        gate_id: &'a GateId,
        run_id: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

/// No-op implementation: all gates are treated as approved.
///
/// Used when no gate checking is configured.
pub struct NoGates;

impl GateCheck for NoGates {
    fn check_gate<'a>(
        &'a self,
        _gate_id: &'a GateId,
    ) -> Pin<Box<dyn Future<Output = GateStatus> + Send + 'a>> {
        Box::pin(async { GateStatus::Approved })
    }

    fn register_gate<'a>(
        &'a self,
        _gate_id: &'a GateId,
        _run_id: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
}
