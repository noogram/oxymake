//! Codec registry — maps file extensions and format names to codecs.
//!
//! The registry is a thin wrapper around [`codec::detect_codec`] that provides
//! a more ergonomic API and collects all required Python imports for a
//! set of codecs used in a single wrapper script.

use std::collections::BTreeSet;
use std::path::Path;

use crate::codec::{self, FormatCodec};
use crate::error::CodecError;

/// Look up a codec by file path and optional format hint.
///
/// Returns an error if no codec can be determined.
pub fn lookup(path: &Path, format_hint: Option<&str>) -> Result<&'static FormatCodec, CodecError> {
    codec::detect_codec(path, format_hint).ok_or_else(|| {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();
        if ext.is_empty() {
            CodecError::NoFormatDetectable {
                path: path.to_path_buf(),
            }
        } else {
            CodecError::UnknownExtension {
                extension: ext,
                path: path.to_path_buf(),
            }
        }
    })
}

/// Collect all unique Python imports needed by a set of codecs.
///
/// Deduplicates and returns sorted imports for deterministic output.
pub fn collect_imports(codecs: &[&FormatCodec]) -> Vec<&'static str> {
    let mut imports: BTreeSet<&'static str> = BTreeSet::new();
    for codec in codecs {
        for imp in codec.python_imports {
            imports.insert(imp);
        }
    }
    imports.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn lookup_success() {
        let codec = lookup(&PathBuf::from("data.parquet"), None).unwrap();
        assert_eq!(codec.name, "parquet");
    }

    #[test]
    fn lookup_with_hint() {
        let codec = lookup(&PathBuf::from("data.bin"), Some("json")).unwrap();
        assert_eq!(codec.name, "json");
    }

    #[test]
    fn lookup_unknown_extension_error() {
        let err = lookup(&PathBuf::from("data.xyz"), None).unwrap_err();
        assert!(matches!(err, CodecError::UnknownExtension { .. }));
    }

    #[test]
    fn lookup_no_extension_error() {
        let err = lookup(&PathBuf::from("Makefile"), None).unwrap_err();
        assert!(matches!(err, CodecError::NoFormatDetectable { .. }));
    }

    #[test]
    fn collect_imports_deduplicates() {
        let codecs = vec![&codec::PARQUET, &codec::CSV]; // Both need pandas
        let imports = collect_imports(&codecs);
        assert_eq!(imports, vec!["import pandas"]);
    }

    #[test]
    fn collect_imports_multiple() {
        let codecs = vec![&codec::JSON, &codec::NPY, &codec::PARQUET];
        let imports = collect_imports(&codecs);
        assert_eq!(
            imports,
            vec![
                "import json",
                "import numpy",
                "import pandas",
                "import pathlib"
            ]
        );
    }
}
