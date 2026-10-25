//! Shared output staging paths for executors.

use std::path::{Path, PathBuf};

/// Suffix reserved for engine-owned output staging files.
pub const ATOMIC_TEMP_SUFFIX: &str = ".oxytmp";

/// Append the staging suffix to the complete output path, preserving its extension.
pub fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(ATOMIC_TEMP_SUFFIX);
    PathBuf::from(name)
}
