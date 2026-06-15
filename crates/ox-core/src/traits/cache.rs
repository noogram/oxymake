//! # Cache Check Trait
//!
//! Defines the plugin interface for content-addressable caching.
//! Implementations determine whether a job's outputs are up-to-date
//! and can be skipped, and record newly completed job outputs.
//!
//! This trait is called by the scheduler at two points:
//! 1. **Before dispatch**: when a job becomes ready (all deps satisfied),
//!    the scheduler calls [`CacheCheck::is_cached`] to see if execution
//!    can be skipped.  This handles intermediate jobs whose inputs were
//!    produced by upstream jobs that just completed.
//! 2. **After success**: when a job completes with exit code 0, the
//!    scheduler calls [`CacheCheck::record`] to persist the result.

use std::future::Future;
use std::pin::Pin;

use crate::model::ConcreteJob;

/// A cache checker that the scheduler consults before and after job execution.
///
/// The trait is object-safe and `Send + Sync` so it can be wrapped in `Arc`
/// and shared across async tasks.
pub trait CacheCheck: Send + Sync {
    /// Check whether a job can be skipped because its outputs are cached
    /// and up-to-date with respect to its current inputs.
    ///
    /// Called when a job transitions from Pending → Ready, *after* all
    /// upstream dependencies have completed.  Input files are guaranteed
    /// to exist on disk at this point.
    ///
    /// Returns `true` if the job should be skipped (cache hit).
    fn is_cached<'a>(
        &'a self,
        job: &'a ConcreteJob,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>>;

    /// Record a successfully completed job's outputs in the cache.
    ///
    /// Called after a job finishes with exit code 0 and all output files
    /// exist on disk.
    fn record<'a>(&'a self, job: &'a ConcreteJob) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}
