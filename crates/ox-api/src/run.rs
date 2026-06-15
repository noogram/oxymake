//! Build execution — orchestrates parsing, planning, caching, and execution.
//!
//! This module provides [`plan`] for inspecting the build graph without
//! executing, useful for dry-run analysis and tooling integration.

use crate::builder::Session;
use crate::error::ApiError;
use ox_core::model::ConcreteJob;

/// Return the jobs in the session's graph in topological (dependency) order.
///
/// This is the dry-run / plan view: it shows what *would* execute without
/// actually running anything.
pub fn plan(session: &Session) -> Result<Vec<&ConcreteJob>, ApiError> {
    let order = session.job_graph.topological_order()?;
    Ok(order
        .iter()
        .filter_map(|id| session.job_graph.get_job(id))
        .collect())
}
