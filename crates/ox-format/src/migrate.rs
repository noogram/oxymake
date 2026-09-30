//! Reviewable, lossless migration of an entire include graph to schema 2.
use crate::schema::{check, collect_sources, document, error, structural};
use ox_core::error::ParseError;
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
};

/// First release implementing the schema-2 contract. This is intentionally
/// fixed: future binary releases must not raise a migrated file's floor.
pub const SCHEMA_2_MINIMUM: &str = "0.7.0";

/// A fully checked migration. Construct with [`prepare`]; review before writing.
#[derive(Debug)]
pub struct Migration {
    originals: BTreeMap<PathBuf, String>,
    proposed: BTreeMap<PathBuf, String>,
    report: String,
}

impl Migration {
    /// Human-readable report of every changed declaration and cache implications.
    pub fn report(&self) -> &str {
        &self.report
    }

    /// Apply all checked files, staging replacements and rollback copies before
    /// changing any original. A detected concurrent edit aborts before writing.
    /// Rollback copies are retained with a diagnostic if the filesystem itself
    /// prevents recovery. This is not a crash-atomic multi-file transaction.
    pub fn write(&self) -> Result<(), ParseError> {
        self.write_with(|replacement, path| replacement.persist(path))
    }

    fn write_with(
        &self,
        mut persist: impl FnMut(
            tempfile::NamedTempFile,
            &Path,
        ) -> Result<std::fs::File, tempfile::PersistError>,
    ) -> Result<(), ParseError> {
        let mut staged = Vec::new();
        for (path, proposed) in &self.proposed {
            if proposed == &self.originals[path] {
                continue;
            }
            let io_error = |e: std::io::Error| error(path, format!("migration staging: {e}"));
            let parent = path.parent().unwrap_or(Path::new("."));
            let permissions = std::fs::metadata(path).map_err(io_error)?.permissions();
            if permissions.readonly() {
                return Err(error(path, "migration refuses a read-only original"));
            }
            let mut replacement = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
            replacement
                .write_all(proposed.as_bytes())
                .map_err(io_error)?;
            replacement
                .as_file()
                .set_permissions(permissions.clone())
                .map_err(io_error)?;
            replacement.as_file().sync_all().map_err(io_error)?;
            let mut backup = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
            backup
                .write_all(self.originals[path].as_bytes())
                .map_err(io_error)?;
            backup
                .as_file()
                .set_permissions(permissions)
                .map_err(io_error)?;
            backup.as_file().sync_all().map_err(io_error)?;
            staged.push((path, replacement, backup));
        }
        // Check every graph member, including already-schema-2 files.
        for (path, original) in &self.originals {
            if std::fs::read_to_string(path).map_err(|e| error(path, e.to_string()))? != *original {
                return Err(error(
                    path,
                    "file changed since migration review; originals left untouched",
                ));
            }
        }
        let mut installed: Vec<(&PathBuf, tempfile::NamedTempFile)> = Vec::new();
        for (path, replacement, backup) in staged {
            if let Err(failure) = persist(replacement, path) {
                let mut message = format!("migration write failed: {}", failure.error);
                for (prior, backup) in installed.into_iter().rev() {
                    if let Err(recovery) = backup.persist(prior) {
                        let retained = recovery.file.keep();
                        message.push_str(&format!(
                            "; rollback failed for {}: {}; recovery copy: {retained:?}",
                            prior.display(),
                            recovery.error
                        ));
                    }
                }
                return Err(error(path, message));
            }
            installed.push((path, backup));
        }
        Ok(())
    }
}

/// Resolve, migrate and validate the entire graph without modifying any files.
/// Only the known generated legacy markers `0.1` and `0.3` have an automatic
/// interpretation. Other declarations require a human decision.
pub fn prepare(path: &Path) -> Result<Migration, ParseError> {
    let content = std::fs::read_to_string(path).map_err(|e| error(path, e.to_string()))?;
    let originals = collect_sources(&content, path)?;
    let mut proposed = BTreeMap::new();
    let mut report = String::from("Schema 2 migration report (preview; use --write to apply)\n");
    for (file, source) in &originals {
        let value = document(source, file)?;
        structural(&value, file, "", "root")?;
        if value.get("format_version").and_then(toml::Value::as_str) == Some("2") {
            proposed.insert(file.clone(), source.clone());
            report.push_str(&format!(
                "{}: already schema 2; unchanged\n",
                file.display()
            ));
            continue;
        }
        let marker = value.get("ox_version").and_then(toml::Value::as_str);
        if !matches!(marker, Some("0.1" | "0.3")) {
            return Err(error(
                file,
                format!(
                    "cannot justify a minimum binary requirement from ox_version {marker:?}; only known generated markers 0.1 and 0.3 migrate automatically; original files untouched"
                ),
            ));
        }
        let mut edited = source
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| error(file, e.to_string()))?;
        // Retain all formatting, comments and command bytes outside the two
        // declarations. Preserve value decorations on changed declarations too.
        for (key, text) in [
            ("format_version", "2"),
            ("ox_version", &format!(">={SCHEMA_2_MINIMUM}")),
        ] {
            let mut next = toml_edit::Value::from(text);
            if let Some(old) = edited.get(key).and_then(toml_edit::Item::as_value) {
                *next.decor_mut() = old.decor().clone();
            }
            edited[key] = toml_edit::Item::Value(next);
        }
        let next = edited.to_string();
        check(&document(&next, file)?, file)?;
        report.push_str(&format!("{}:\n  format_version: {} -> 2\n  ox_version: {} -> >={}\n  Known generated marker upgraded to the release introducing schema 2.\n", file.display(), value.get("format_version").map(toml::Value::to_string).unwrap_or_else(|| "absent (1)".into()), marker.unwrap(), SCHEMA_2_MINIMUM));
        proposed.insert(file.clone(), next);
    }
    let root = path
        .canonicalize()
        .map_err(|e| error(path, e.to_string()))?;
    // Parse the retained snapshots, including config sources, and validate the
    // merged workflow before any file can be staged or replaced.
    let before = crate::parse::parse_sources(&content, path, &originals)?;
    let after = crate::parse::parse_sources(&proposed[&root], path, &proposed)?;
    crate::validate::validate(&after).map_err(|errors| {
        error(
            path,
            errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; "),
        )
    })?;
    for (old, new) in before.rules.iter().zip(&after.rules) {
        if old.execution != new.execution || old.environment != new.environment {
            return Err(error(
                path,
                "migration changed command or software environment; originals untouched",
            ));
        }
    }
    report.push_str("Commands are preserved byte-for-byte; software environments keep their meaning. No shell assignments are extracted.\nAdopting schema 2 will move cache identity later: the first run after the eventual feature release recomputes. This migration alone does not change resource cache identity.\n");
    Ok(Migration {
        originals,
        proposed,
        report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_failure_restores_already_written_graph_members() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root.toml");
        let child = dir.path().join("child.toml");
        let root_text = "ox_version='0.1'\ninclude=['child.toml']";
        let child_text = "ox_version='0.3'";
        std::fs::write(&root, root_text).unwrap();
        std::fs::write(&child, child_text).unwrap();
        let migration = prepare(&root).unwrap();
        let mut writes = 0;
        let error = migration
            .write_with(|file, path| {
                writes += 1;
                if writes == 2 {
                    assert!(
                        std::fs::read_to_string(&child)
                            .unwrap()
                            .contains("format_version")
                    );
                    Err(tempfile::PersistError {
                        file,
                        error: std::io::Error::other("injected replacement failure"),
                    })
                } else {
                    file.persist(path)
                }
            })
            .unwrap_err();
        assert!(error.to_string().contains("injected replacement failure"));
        assert_eq!(writes, 2);
        assert_eq!(std::fs::read_to_string(&root).unwrap(), root_text);
        assert_eq!(std::fs::read_to_string(&child).unwrap(), child_text);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }
}
