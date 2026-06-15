#![allow(clippy::manual_strip, clippy::ptr_arg)]
//! # ox-format — Oxymakefile Parser
//!
//! This crate is responsible for reading `Oxymakefile.toml` and producing
//! a `Vec<Rule>` that the rest of the pipeline can consume.
//!
//! ## Crate responsibilities
//!
//! - TOML deserialization of Oxymakefile syntax into intermediate structs
//! - Validation of rule definitions (duplicate targets, missing fields, etc.)
//! - Conversion from the on-disk TOML schema to `ox_core::model::Rule`
//! - Helpful error messages with span information for malformed input
//!
//! ## What this crate NEVER does
//!
//! - Graph construction or resolution (that's ox-core / ox-plan)
//! - Execution or caching decisions
//! - Anything beyond parsing and validating the declared workflow

pub mod error;
pub mod parse;
pub mod targets;
pub mod validate;
