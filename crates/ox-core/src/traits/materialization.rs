//! # Materialization Strategy Trait
//!
//! Defines the plugin interface for artifact materialization strategies.
//! A strategy determines how an artifact is identified (its "identity flavor")
//! and how its existence and validity are verified.
//!
//! Three identity flavors are supported:
//!
//! - **ContentHash**: Identity by data content (BLAKE3). Best for
//!   content-addressable caching and deduplication.
//! - **ComputationHash**: Identity by computation specification (cache key).
//!   Best for build avoidance and cache lookup.
//! - **ExternalRef**: Identity by external locator (URI). Best for virtual
//!   outputs, database tables, and API endpoints.
//!
//! Built-in implementations:
//! - `ContentAddressedStrategy` (ox-cache) — uses BLAKE3 content hashing
//! - `ComputationKeyStrategy` (ox-cache) — uses cache key computation
//!
//! See the dataref-abstraction-exploration design doc for the full rationale.

use std::future::Future;

use crate::model::{ArtifactIdentity, OutputRef};

/// A materialization strategy determines how an artifact is identified and
/// verified.
///
/// The strategy is a plugin axis: different outputs in the same workflow can
/// use different strategies. For example, file outputs might use
/// `ContentHash` (verify by hashing the bytes), while virtual outputs
/// (database tables) use `ExternalRef` (verify by running a check query).
///
/// # Lifecycle
///
/// 1. **Identify**: After a job produces an output, the scheduler calls
///    [`identify`](Self::identify) to compute the artifact's identity.
/// 2. **Verify**: Before a downstream job fires, the scheduler calls
///    [`verify`](Self::verify) to confirm the artifact is still valid.
///
/// # Implementor guidelines
///
/// - `identify` should be deterministic for the same data (content strategy)
///   or the same specification (computation strategy).
/// - `verify` should be fast — it's called on the scheduler's hot path.
///   Prefer metadata checks over full re-hashing when possible.
/// - Implementations must be `Send + Sync` for use in async schedulers.
pub trait MaterializationStrategy: Send + Sync {
    /// Error type specific to this strategy.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Compute the artifact identity for a produced output.
    ///
    /// Called after a job completes to determine the identity of the
    /// artifact it produced. The `output_ref` provides the logical
    /// identifier; the strategy determines the physical identity.
    fn identify(
        &self,
        output_ref: &OutputRef,
    ) -> impl Future<Output = Result<ArtifactIdentity, Self::Error>> + Send;

    /// Verify that a previously identified artifact is still valid.
    ///
    /// Returns `true` if the artifact matching `identity` is available
    /// and consistent. For content-addressed artifacts, this might check
    /// that the file exists and its hash matches. For external refs,
    /// this might run a check query.
    ///
    /// Called on the scheduler's hot path — implementations should be fast.
    fn verify(
        &self,
        output_ref: &OutputRef,
        identity: &ArtifactIdentity,
    ) -> impl Future<Output = Result<bool, Self::Error>> + Send;

    /// Return the identity flavor this strategy produces.
    ///
    /// Used by the scheduler to select the appropriate verification
    /// path without dynamic dispatch overhead.
    fn flavor(&self) -> IdentityFlavor;
}

/// Discriminant for [`ArtifactIdentity`] variants, without carrying data.
///
/// Used by [`MaterializationStrategy::flavor`] to declare which identity
/// type a strategy produces, enabling the scheduler to make routing
/// decisions without matching on the full enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdentityFlavor {
    /// Strategy produces [`ArtifactIdentity::Content`] identities.
    Content,
    /// Strategy produces [`ArtifactIdentity::Computation`] identities.
    Computation,
    /// Strategy produces [`ArtifactIdentity::External`] identities.
    External,
}

impl std::fmt::Display for IdentityFlavor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Content => f.write_str("content"),
            Self::Computation => f.write_str("computation"),
            Self::External => f.write_str("external"),
        }
    }
}
