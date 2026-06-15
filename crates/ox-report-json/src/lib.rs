//! # ox-report-json — NDJSON Reporter
//!
//! This crate implements the `Reporter` trait from ox-core by emitting
//! one JSON object per line (NDJSON) to stdout. Designed for consumption
//! by CI systems, agents, and downstream tooling.
//!
//! ## Crate responsibilities
//!
//! - Serialize each `Event` to a single JSON line on stdout
//! - Ensure output is valid NDJSON (one complete object per line)
//! - Include timestamps, job IDs, and structured metadata in each event
//! - Provide a stable schema for machine consumers
//!
//! ## What this crate NEVER does
//!
//! - Human-friendly formatting (that's ox-report-term)
//! - Build orchestration or decision-making
//! - Aggregation or summarization of events

pub mod error;
pub mod reporter;
pub mod schema;
