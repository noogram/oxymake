//! Format codec descriptors for call-mode wrapper generation.
//!
//! Each [`FormatCodec`] describes how a file format maps to Python
//! deserialization/serialization code.  The local executor uses these
//! descriptors to generate a Python wrapper script that handles I/O
//! around the user's pure function.
//!
//! Phase 1 is entirely file-based: the Rust side never reads the actual
//! data — it only generates Python code that does.

use std::path::Path;

/// Describes a file format and how to read/write it in Python.
///
/// This is *not* a Rust-side serializer. It generates Python code snippets
/// that the call-mode wrapper script uses to load inputs and save outputs.
#[derive(Debug, Clone)]
pub struct FormatCodec {
    /// Canonical name (e.g., "parquet", "json", "csv").
    pub name: &'static str,

    /// File extensions this codec handles (e.g., `["parquet", "pq"]`).
    pub extensions: &'static [&'static str],

    /// Python expression that reads a file into an object.
    ///
    /// The placeholder `{path}` is replaced with the quoted file path.
    /// Example: `"pandas.read_parquet({path})"`
    pub python_read: &'static str,

    /// Python statement that writes an object to a file.
    ///
    /// Placeholders: `{obj}` = the variable name, `{path}` = quoted path.
    /// Example: `"{obj}.to_parquet({path})"`
    pub python_write: &'static str,

    /// Python import statements needed for this codec.
    ///
    /// Example: `&["import pandas"]`
    pub python_imports: &'static [&'static str],
}

/// Built-in codec: Parquet (via pandas).
pub const PARQUET: FormatCodec = FormatCodec {
    name: "parquet",
    extensions: &["parquet", "pq"],
    python_read: "pandas.read_parquet({path})",
    python_write: "{obj}.to_parquet({path})",
    python_imports: &["import pandas"],
};

/// Built-in codec: CSV (via pandas).
pub const CSV: FormatCodec = FormatCodec {
    name: "csv",
    extensions: &["csv"],
    python_read: "pandas.read_csv({path})",
    python_write: "{obj}.to_csv({path}, index=False)",
    python_imports: &["import pandas"],
};

/// Built-in codec: TSV (via pandas, tab-separated).
pub const TSV: FormatCodec = FormatCodec {
    name: "tsv",
    extensions: &["tsv"],
    python_read: "pandas.read_csv({path}, sep='\\t')",
    python_write: "{obj}.to_csv({path}, sep='\\t', index=False)",
    python_imports: &["import pandas"],
};

/// Built-in codec: JSON (via standard library).
pub const JSON: FormatCodec = FormatCodec {
    name: "json",
    extensions: &["json"],
    python_read: "json.loads(pathlib.Path({path}).read_text())",
    python_write: "pathlib.Path({path}).write_text(json.dumps({obj}, indent=2))",
    python_imports: &["import json", "import pathlib"],
};

/// Built-in codec: NumPy .npy (single array).
pub const NPY: FormatCodec = FormatCodec {
    name: "npy",
    extensions: &["npy"],
    python_read: "numpy.load({path})",
    python_write: "numpy.save({path}, {obj})",
    python_imports: &["import numpy"],
};

/// Built-in codec: NumPy .npz (multiple arrays).
pub const NPZ: FormatCodec = FormatCodec {
    name: "npz",
    extensions: &["npz"],
    python_read: "dict(numpy.load({path}))",
    python_write: "numpy.savez({path}, **{obj})",
    python_imports: &["import numpy"],
};

/// Built-in codec: Arrow IPC / Feather (via pyarrow).
pub const ARROW: FormatCodec = FormatCodec {
    name: "arrow",
    extensions: &["arrow", "feather", "ipc"],
    python_read: "pyarrow.feather.read_table({path})",
    python_write: "pyarrow.feather.write_feather({obj}, {path})",
    python_imports: &["import pyarrow.feather"],
};

/// Built-in codec: Pickle (Python-native serialization).
pub const PICKLE: FormatCodec = FormatCodec {
    name: "pickle",
    extensions: &["pkl", "pickle"],
    python_read: "pickle.loads(pathlib.Path({path}).read_bytes())",
    python_write: "pathlib.Path({path}).write_bytes(pickle.dumps({obj}))",
    python_imports: &["import pickle", "import pathlib"],
};

/// Built-in codec: YAML (via PyYAML).
pub const YAML: FormatCodec = FormatCodec {
    name: "yaml",
    extensions: &["yaml", "yml"],
    python_read: "yaml.safe_load(pathlib.Path({path}).read_text())",
    python_write: "pathlib.Path({path}).write_text(yaml.dump({obj}))",
    python_imports: &["import yaml", "import pathlib"],
};

/// All built-in codecs, in priority order (first match wins for shared extensions).
pub const BUILTIN_CODECS: &[&FormatCodec] = &[
    &PARQUET, &CSV, &TSV, &JSON, &NPY, &NPZ, &ARROW, &PICKLE, &YAML,
];

/// Detect the format codec for a file path, using an optional explicit hint.
///
/// Resolution order:
/// 1. If `format_hint` is `Some`, match by codec name or extension.
/// 2. Otherwise, match by the file's extension.
/// 3. If no match, return `None`.
pub fn detect_codec(path: &Path, format_hint: Option<&str>) -> Option<&'static FormatCodec> {
    if let Some(hint) = format_hint {
        // Match by codec name first, then by extension.
        let hint_lower = hint.to_lowercase();
        for codec in BUILTIN_CODECS {
            if codec.name == hint_lower {
                return Some(codec);
            }
        }
        for codec in BUILTIN_CODECS {
            if codec.extensions.contains(&hint_lower.as_str()) {
                return Some(codec);
            }
        }
    }

    // Fall back to file extension detection.
    let ext = path.extension()?.to_str()?.to_lowercase();
    BUILTIN_CODECS
        .iter()
        .find(|codec| codec.extensions.contains(&ext.as_str()))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn detect_parquet_by_extension() {
        let codec = detect_codec(&PathBuf::from("data/prices.parquet"), None).unwrap();
        assert_eq!(codec.name, "parquet");
    }

    #[test]
    fn detect_parquet_by_pq_extension() {
        let codec = detect_codec(&PathBuf::from("data/prices.pq"), None).unwrap();
        assert_eq!(codec.name, "parquet");
    }

    #[test]
    fn detect_csv_by_extension() {
        let codec = detect_codec(&PathBuf::from("results/output.csv"), None).unwrap();
        assert_eq!(codec.name, "csv");
    }

    #[test]
    fn detect_json_by_extension() {
        let codec = detect_codec(&PathBuf::from("config.json"), None).unwrap();
        assert_eq!(codec.name, "json");
    }

    #[test]
    fn detect_by_format_hint_overrides_extension() {
        // File has .csv extension but hint says json.
        let codec = detect_codec(&PathBuf::from("data.csv"), Some("json")).unwrap();
        assert_eq!(codec.name, "json");
    }

    #[test]
    fn detect_by_format_hint_name() {
        let codec = detect_codec(&PathBuf::from("data.bin"), Some("parquet")).unwrap();
        assert_eq!(codec.name, "parquet");
    }

    #[test]
    fn detect_npy() {
        let codec = detect_codec(&PathBuf::from("weights.npy"), None).unwrap();
        assert_eq!(codec.name, "npy");
    }

    #[test]
    fn detect_arrow_feather() {
        let codec = detect_codec(&PathBuf::from("table.feather"), None).unwrap();
        assert_eq!(codec.name, "arrow");
    }

    #[test]
    fn detect_pickle() {
        let codec = detect_codec(&PathBuf::from("model.pkl"), None).unwrap();
        assert_eq!(codec.name, "pickle");
    }

    #[test]
    fn detect_yaml() {
        let codec = detect_codec(&PathBuf::from("config.yaml"), None).unwrap();
        assert_eq!(codec.name, "yaml");

        let codec = detect_codec(&PathBuf::from("config.yml"), None).unwrap();
        assert_eq!(codec.name, "yaml");
    }

    #[test]
    fn detect_unknown_returns_none() {
        assert!(detect_codec(&PathBuf::from("data.xyz"), None).is_none());
    }

    #[test]
    fn detect_no_extension_no_hint_returns_none() {
        assert!(detect_codec(&PathBuf::from("Makefile"), None).is_none());
    }

    #[test]
    fn python_read_has_path_placeholder() {
        for codec in BUILTIN_CODECS {
            assert!(
                codec.python_read.contains("{path}"),
                "codec {} python_read missing {{path}} placeholder",
                codec.name
            );
        }
    }

    #[test]
    fn python_write_has_placeholders() {
        for codec in BUILTIN_CODECS {
            assert!(
                codec.python_write.contains("{obj}") && codec.python_write.contains("{path}"),
                "codec {} python_write missing placeholders",
                codec.name
            );
        }
    }
}
