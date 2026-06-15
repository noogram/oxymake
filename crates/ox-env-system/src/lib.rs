//! # ox-env-system — System Environment Provider
//!
//! This crate implements the `EnvironmentProvider` trait from ox-core with
//! zero isolation: jobs run with whatever PATH, env vars, and tools the
//! host system provides.
//!
//! ## Crate responsibilities
//!
//! - Implement `EnvironmentProvider` that passes through host environment
//! - Resolve tool paths using the system PATH
//! - Provide the default "just works" environment for simple workflows
//!
//! ## What this crate NEVER does
//!
//! - Create or manage virtual environments
//! - Install packages or tools
//! - Isolate jobs from each other

pub mod error;
pub mod system;

#[cfg(test)]
mod tests {
    #[test]
    fn crate_compiles() {
        // Verify that the crate's module structure is valid.
        // Actual unit tests will be added when EnvironmentProvider is implemented.
    }
}
