//! # ox-codec-core — Format Codecs for Call Mode
//!
//! This crate provides format codec descriptors and a registry for
//! auto-detecting file formats by extension. Codecs generate Python
//! code snippets used by the call-mode wrapper script to deserialize
//! inputs and serialize outputs around the user's pure function.
//!
//! ## Supported formats (Phase 1)
//!
//! | Format  | Extensions          | Python library |
//! |---------|--------------------|--------------------|
//! | Parquet | `.parquet`, `.pq`  | pandas             |
//! | CSV     | `.csv`             | pandas             |
//! | TSV     | `.tsv`             | pandas             |
//! | JSON    | `.json`            | json (stdlib)      |
//! | NumPy   | `.npy`             | numpy              |
//! | NumPy   | `.npz`             | numpy              |
//! | Arrow   | `.arrow`, `.feather`, `.ipc` | pyarrow  |
//! | Pickle  | `.pkl`, `.pickle`  | pickle (stdlib)    |
//! | YAML    | `.yaml`, `.yml`    | pyyaml             |
//!
//! ## Crate responsibilities
//!
//! - Define format codec descriptors with Python read/write snippets
//! - Auto-detect format from file extension or explicit hint
//! - Collect Python imports for wrapper script generation
//!
//! ## What this crate NEVER does
//!
//! - Read or write data files from Rust (that's the Python wrapper's job)
//! - Cache invalidation (that's ox-cache)
//! - Domain-specific validation of artifact contents

pub mod codec;
pub mod error;
pub mod json;
pub mod registry;
