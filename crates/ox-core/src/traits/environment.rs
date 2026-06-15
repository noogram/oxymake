//! # Environment Provider Trait
//!
//! Defines the plugin interface for job execution environments.
//! Environments provide isolation — ensuring jobs run with the correct
//! dependencies and versions regardless of the host system.
//!
//! Built-in: `SystemEnvironment` (no isolation), `UvEnvironment`
//! Phase 2+: Conda, Docker, Nix, Apptainer

use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;

/// Specification for an environment, as declared in the Oxymakefile.
///
/// Each variant corresponds to an environment provider.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvSpec {
    /// No isolation — use host system tools.
    System,
    /// uv-managed Python environment.
    Uv {
        /// Path to pyproject.toml or requirements.txt.
        spec: PathBuf,
    },
    /// Conda environment.
    Conda {
        /// Path to environment.yaml.
        spec: PathBuf,
    },
    /// Docker/OCI container.
    Docker {
        /// Image reference (e.g., "pytorch/pytorch:2.5-cuda12.4").
        image: String,
    },
    /// Nix flake environment.
    Nix {
        /// Flake reference (e.g., "flake.nix#devShell").
        flake: String,
    },
    /// Apptainer/Singularity container (HPC-friendly).
    Apptainer {
        /// Path to .sif image.
        image: PathBuf,
    },
}

/// A prepared environment ready for job execution.
#[derive(Debug)]
pub struct PreparedEnv {
    /// Environment variables to inject into the job process.
    pub env_vars: BTreeMap<String, String>,
    /// Command prefix (e.g., `["uv", "run", "--"]` or `["docker", "run", ...]`).
    pub command_prefix: Vec<String>,
    /// Hash of the environment specification (for cache key computation).
    pub env_hash: String,
}

/// The environment provider trait — plugin interface for job isolation.
///
/// Environment providers prepare isolated execution environments from
/// a specification. They are responsible for:
/// - Installing/caching dependencies
/// - Computing a stable hash of the environment
/// - Wrapping the job's command with the appropriate prefix
pub trait EnvironmentProvider: Send + Sync {
    /// Error type specific to this provider.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Name of this provider (e.g., "uv", "conda", "docker").
    fn name(&self) -> &str;

    /// Prepare the environment from a specification.
    ///
    /// This may install dependencies, pull images, or create virtual
    /// environments. Results are cached based on the spec hash.
    fn prepare(
        &self,
        spec: &EnvSpec,
    ) -> impl Future<Output = Result<PreparedEnv, Self::Error>> + Send;

    /// Compute a hash of the environment specification.
    ///
    /// This hash becomes part of the cache key (invariant 6.4).
    /// It must capture everything that could affect the job's output:
    /// dependency versions, base image digest, etc.
    fn spec_hash(&self, spec: &EnvSpec)
    -> impl Future<Output = Result<String, Self::Error>> + Send;
}
