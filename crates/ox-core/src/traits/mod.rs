//! Plugin trait definitions for OxyMake.
//!
//! Each trait defines a plugin axis: Executor, Storage, EnvironmentProvider,
//! Reporter, FormatCodec, and OptimizationPass.

pub mod benchmark;
pub mod cache;
pub mod environment;
pub mod executor;
pub mod format_codec;
pub mod gate;
pub mod materialization;
pub mod optimization;
pub mod remote_cache;
pub mod reporter;
pub mod storage;
