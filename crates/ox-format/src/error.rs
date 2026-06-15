//! Format-specific error helpers that wrap [`ox_core::error::ParseError`].
//!
//! This module re-exports `ParseError` and provides convenience constructors
//! used by the parser and validator.

pub use ox_core::error::ParseError;
