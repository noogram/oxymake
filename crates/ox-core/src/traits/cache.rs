//! # Cache Check Trait
//!
//! Defines the plugin interface for content-addressable caching.
//! Implementations determine whether a job's outputs are up-to-date
//! and can be skipped, and record newly completed job outputs.
//!
//! This trait is called by the scheduler at two points:
//! 1. **Before dispatch**: when a job becomes ready (all deps satisfied),
//!    the scheduler calls [`CacheCheck::is_cached`] to see if execution
//!    can be skipped. This handles intermediate jobs whose inputs were
//!    produced by upstream jobs that just completed. Before executing a job,
//!    [`CacheCheck::recorded_output_hashes`] snapshots the previous outputs so
//!    an identical rebuild need not invalidate consumers.
//! 2. **After success**: when a job completes with exit code 0, the
//!    scheduler calls [`CacheCheck::record`] to persist the result.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use crate::model::{ConcreteJob, ContentHash, RunReason};

/// Recorded output content hashes, keyed by [`crate::job_graph::output_ref_key`].
pub type OutputHashes = BTreeMap<String, ContentHash>;

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

    /// Check at dispatch, preserving the actual reason for a cache miss.
    ///
    /// `Ok(())` means a hit. Implementations can report missing or stale outputs;
    /// the default maps [`Self::is_cached`] misses to [`RunReason::CacheMiss`].
    fn check_with_reason<'a>(
        &'a self,
        job: &'a ConcreteJob,
    ) -> Pin<Box<dyn Future<Output = Result<(), RunReason>> + Send + 'a>> {
        Box::pin(async move {
            if self.is_cached(job).await {
                Ok(())
            } else {
                Err(RunReason::CacheMiss)
            }
        })
    }

    /// Snapshot previously recorded output content hashes before execution.
    ///
    /// Keys use [`crate::job_graph::output_ref_key`]. Return hashes recorded under
    /// the same cache key as the current job. The scheduler compares these hashes
    /// with produced bytes before recording the new result. Return
    /// `None` when comparison is unavailable (including mtime-only validation
    /// and non-cacheable jobs). Missing entries also force conservative
    /// downstream invalidation. The default preserves that behavior for plugins.
    fn recorded_output_hashes<'a>(
        &'a self,
        _job: &'a ConcreteJob,
    ) -> Pin<Box<dyn Future<Output = Option<OutputHashes>> + Send + 'a>> {
        Box::pin(async { None })
    }

    /// Record outputs using hashes already computed from disk after execution.
    ///
    /// Keys use [`crate::job_graph::output_ref_key`]. Missing hashes must be
    /// computed by the implementation. The default delegates to [`Self::record`].
    fn record_with_hashes<'a>(
        &'a self,
        job: &'a ConcreteJob,
        _hashes: &'a OutputHashes,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        self.record(job)
    }

    /// Record a successfully completed job's outputs in the cache.
    ///
    /// Called after a job finishes with exit code 0 and all output files
    /// exist on disk.
    fn record<'a>(&'a self, job: &'a ConcreteJob) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}
