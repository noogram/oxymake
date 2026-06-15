//! # ox-report-term — Terminal Reporter
//!
//! This crate implements the `Reporter` trait from ox-core for interactive
//! terminal use. It renders progress bars, colored status lines, and
//! rich error diagnostics using miette and indicatif.
//!
//! ## Crate responsibilities
//!
//! - Render build progress with indicatif progress bars
//! - Display job start/finish/fail events with colors and timing
//! - Format errors as rich miette diagnostics with source spans
//! - Adapt output for TTY vs piped contexts (auto-detect)
//!
//! ## What this crate NEVER does
//!
//! - Structured/machine-readable output (that's ox-report-json)
//! - Build orchestration or decision-making
//! - File I/O beyond writing to stdout/stderr

pub mod error;
pub mod format;
pub mod reporter;
