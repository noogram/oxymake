//! # Format Codec Trait
//!
//! Defines the plugin interface for serializing/deserializing objects
//! to/from files. Used by `call`-mode rules where OxyMake manages I/O
//! on behalf of pure functions.
//!
//! Built-in codecs (ox-codec-core): CSV, JSON, YAML
//! Phase 2+: Parquet/Arrow (ox-codec-arrow), Pickle (ox-codec-pickle)

use std::any::Any;
use std::path::Path;

/// The format codec trait — plugin interface for object serialization.
///
/// FormatCodec implementations bridge the gap between files on disk and
/// typed objects in memory. When a `call`-mode rule executes:
///
/// 1. OxyMake reads input files using the codec's `read()` method
/// 2. The resulting objects are passed to the user's function
/// 3. The function's return value is written using the codec's `write()` method
///
/// In memory mode (`materialize = "never"`), the codec is bypassed
/// entirely — objects pass directly between functions.
pub trait FormatCodec: Send + Sync {
    /// File extensions this codec handles (e.g., `["csv"]`, `["parquet", "pq"]`).
    fn extensions(&self) -> &[&str];

    /// Human-readable name of this format (e.g., "CSV", "Parquet").
    fn name(&self) -> &str;

    /// Read a file into an in-memory object.
    ///
    /// The returned `Box<dyn Any>` is passed to the user's function.
    /// The concrete type depends on the codec (e.g., CSV → `Vec<Vec<String>>`,
    /// Parquet → Arrow RecordBatch).
    fn read(&self, path: &Path) -> Result<Box<dyn Any + Send>, std::io::Error>;

    /// Write an in-memory object to a file.
    ///
    /// The `obj` is the return value from the user's function.
    fn write(&self, obj: &dyn Any, path: &Path) -> Result<(), std::io::Error>;
}
