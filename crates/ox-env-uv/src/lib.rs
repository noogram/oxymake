//! # ox-env-uv — uv-Managed Python Environments
//!
//! This crate implements the `EnvironmentProvider` trait from ox-core using
//! [uv](https://github.com/astral-sh/uv) to create and manage per-rule
//! Python virtual environments.
//!
//! ## Crate responsibilities
//!
//! - Create isolated Python virtualenvs via `uv venv`
//! - Install declared dependencies via `uv pip install`
//! - Cache and reuse environments when dependency specs haven't changed
//! - Inject the correct PATH/VIRTUAL_ENV into job execution environment
//!
//! ## What this crate NEVER does
//!
//! - Manage non-Python toolchains
//! - Execute build jobs (that's the executor's job)
//! - Make caching decisions about build outputs

pub mod error;
pub mod resolve;
pub mod venv;

#[cfg(test)]
mod tests {
    #[test]
    fn crate_compiles() {
        // Verify that the crate's module structure is valid.
        // Actual unit tests will be added when EnvironmentProvider is implemented.
    }
}
