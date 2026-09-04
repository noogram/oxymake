//! [`GateCheck`] backed by the `gates` table — the production gate checker.
//!
//! `ox-core` defines the [`GateCheck`] trait but ships only the permissive
//! [`NoGates`](ox_core::traits::gate::NoGates).  [`StateGateChecker`] is the
//! implementation `ox run` hands to the scheduler so that a `[gate.<name>]`
//! declared in the Oxymakefile actually blocks the rules it guards until
//! `ox gate approve <name>` (or `reject`) records a decision.
//!
//! # Record rule
//!
//! One record per `(gate name, run_id)`:
//!
//! - [`register_gate`](GateCheck::register_gate) inserts a `pending` record
//!   for the checker's run unless one already exists — idempotent, so the
//!   scheduler may call it on every poll.
//! - [`check_gate`](GateCheck::check_gate) reads that record and maps its
//!   status to [`GateStatus`].  A gate that was never registered for this
//!   run is `NotFound` (the scheduler treats it as open).
//!
//! An approval therefore belongs to the run that asked for it: the next
//! `ox run` that reaches the same gate registers a fresh pending record and
//! waits again.  This is the behaviour a QC checkpoint wants (new upstream
//! results, new sign-off) and it keeps `ox gate approve <name>` unambiguous —
//! with a single run waiting there is exactly one pending record per name.
//!
//! # Failure policy
//!
//! A database error while checking a gate yields `Pending`, never
//! `Approved`: a gate exists to stop work, so the checker fails closed and
//! logs the error through `tracing`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use ox_core::model::GateId;
use ox_core::traits::gate::{GateCheck, GateStatus};

use crate::db::StateDb;

/// Gate checker that reads and writes the `gates` table of `state.db`.
///
/// Holds its own [`StateDb`] connection behind a mutex (a `rusqlite`
/// connection is `Send` but not `Sync`); every operation is a single short
/// SQLite statement executed without awaiting, so the lock is never held
/// across a suspension point.
pub struct StateGateChecker {
    db: Mutex<StateDb>,
    run_id: Option<String>,
}

impl StateGateChecker {
    /// Create a checker scoped to `run_id` (the `ox run` invocation whose
    /// gates it registers and checks).
    pub fn new(db: StateDb, run_id: Option<String>) -> Self {
        Self {
            db: Mutex::new(db),
            run_id,
        }
    }

    fn run_id(&self) -> Option<&str> {
        self.run_id.as_deref()
    }

    fn check_sync(&self, gate_id: &GateId) -> GateStatus {
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(poisoned) => poisoned.into_inner(),
        };
        match db.gate_status(gate_id.as_str(), self.run_id()) {
            Ok(Some(status)) => match status.as_str() {
                "approved" => GateStatus::Approved,
                "rejected" => GateStatus::Rejected,
                _ => GateStatus::Pending,
            },
            Ok(None) => GateStatus::NotFound,
            Err(e) => {
                tracing::error!(
                    target: "ox.state.gate",
                    gate = %gate_id,
                    error = %e,
                    "gate status lookup failed; treating the gate as pending"
                );
                GateStatus::Pending
            }
        }
    }

    fn register_sync(&self, gate_id: &GateId, run_id: Option<&str>) {
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(poisoned) => poisoned.into_inner(),
        };
        // The checker is scoped to one run; the scheduler passes the same
        // run id, but the checker's own id is authoritative so that
        // `check_gate` (which receives no run id) reads the record that
        // `register_gate` wrote.
        let run_id = self.run_id().or(run_id);
        if let Err(e) = db.register_gate(gate_id.as_str(), run_id) {
            tracing::error!(
                target: "ox.state.gate",
                gate = %gate_id,
                error = %e,
                "gate registration failed"
            );
        }
    }
}

impl GateCheck for StateGateChecker {
    fn check_gate<'a>(
        &'a self,
        gate_id: &'a GateId,
    ) -> Pin<Box<dyn Future<Output = GateStatus> + Send + 'a>> {
        Box::pin(async move { self.check_sync(gate_id) })
    }

    fn register_gate<'a>(
        &'a self,
        gate_id: &'a GateId,
        run_id: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move { self.register_sync(gate_id, run_id) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn checker(run_id: &str) -> (NamedTempFile, StateGateChecker, StateDb) {
        let tmp = NamedTempFile::new().unwrap();
        let db = StateDb::open(tmp.path()).unwrap();
        let other = StateDb::open(tmp.path()).unwrap();
        (
            tmp,
            StateGateChecker::new(db, Some(run_id.to_string())),
            other,
        )
    }

    #[tokio::test]
    async fn unregistered_gate_is_not_found() {
        let (_tmp, checker, _db) = checker("run-1");
        let gate = GateId::from("qc");
        assert_eq!(checker.check_gate(&gate).await, GateStatus::NotFound);
    }

    #[tokio::test]
    async fn registered_gate_is_pending_until_decided() {
        let (_tmp, checker, db) = checker("run-1");
        let gate = GateId::from("qc");

        checker.register_gate(&gate, Some("run-1")).await;
        assert_eq!(checker.check_gate(&gate).await, GateStatus::Pending);
        // Idempotent: a second registration adds no record.
        checker.register_gate(&gate, Some("run-1")).await;
        assert_eq!(db.list_gates().unwrap().len(), 1);

        // `ox gate approve qc` from another connection.
        let id = db.resolve_pending_gate("qc").unwrap();
        db.approve_gate(id, "alice", "ok").unwrap();
        assert_eq!(checker.check_gate(&gate).await, GateStatus::Approved);
    }

    #[tokio::test]
    async fn rejected_gate_reports_rejected() {
        let (_tmp, checker, db) = checker("run-1");
        let gate = GateId::from("qc");
        checker.register_gate(&gate, Some("run-1")).await;
        let id = db.resolve_pending_gate("qc").unwrap();
        db.reject_gate(id, "alice", "bad metrics").unwrap();
        assert_eq!(checker.check_gate(&gate).await, GateStatus::Rejected);
    }

    #[tokio::test]
    async fn approval_is_scoped_to_the_run() {
        let (_tmp, checker, db) = checker("run-2");
        let gate = GateId::from("qc");
        // An approval recorded for a previous run does not open the gate
        // for this one.
        let old = db.register_gate("qc", Some("run-1")).unwrap().unwrap();
        db.approve_gate(old, "alice", "").unwrap();

        assert_eq!(checker.check_gate(&gate).await, GateStatus::NotFound);
        checker.register_gate(&gate, Some("run-2")).await;
        assert_eq!(checker.check_gate(&gate).await, GateStatus::Pending);
    }
}
