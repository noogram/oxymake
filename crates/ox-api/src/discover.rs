//! Cached filesystem discovery for source files.
//!
//! [`discover_existing_files`] walks the directory tree relative to an
//! Oxymakefile and returns all regular files found (up to a depth limit).
//! Results are cached per canonical base directory, so repeated calls with
//! the same Oxymakefile path return instantly.
//!
//! The cache is invalidated when the **modification time** of the base
//! directory changes (e.g. a file was added or removed directly in it).
//! For deeper changes, callers can use [`invalidate`] to force a re-walk.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// Maximum directory depth for the recursive walk.
const MAX_DEPTH: usize = 5;

/// Directory names that are always skipped during the walk.
const SKIP_DIRS: &[&str] = &["target", "node_modules"];

struct CacheEntry {
    mtime: SystemTime,
    files: Vec<PathBuf>,
}

static CACHE: Mutex<Option<HashMap<PathBuf, CacheEntry>>> = Mutex::new(None);

/// Discover files on disk relative to the Oxymakefile's parent directory.
///
/// Returns cached results when the base directory's mtime has not changed.
pub fn discover_existing_files(oxymakefile_path: &Path) -> Vec<PathBuf> {
    let base_dir = oxymakefile_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let canonical = match base_dir.canonicalize() {
        Ok(c) => c,
        Err(_) => base_dir.to_path_buf(),
    };

    let current_mtime = dir_mtime(base_dir);

    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);

    if let Some(entry) = map.get(&canonical) {
        if Some(entry.mtime) == current_mtime {
            return entry.files.clone();
        }
    }

    // Cache miss or stale — perform the walk.
    let mut files = Vec::new();
    walk_dir(base_dir, base_dir, &mut files, MAX_DEPTH);

    if let Some(mtime) = current_mtime {
        map.insert(
            canonical,
            CacheEntry {
                mtime,
                files: files.clone(),
            },
        );
    }

    files
}

/// Force-invalidate the cache for all directories.
pub fn invalidate() {
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(map) = guard.as_mut() {
        map.clear();
    }
}

fn dir_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn walk_dir(dir: &Path, base: &Path, files: &mut Vec<PathBuf>, depth: usize) {
    if depth == 0 {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with('.') || SKIP_DIRS.contains(&&*name_str) {
                continue;
            }
            walk_dir(&path, base, files, depth - 1);
        } else if path.is_file() {
            if let Ok(rel) = path.strip_prefix(base) {
                let rel_str = rel.to_string_lossy();
                let clean = rel_str.strip_prefix("./").unwrap_or(&rel_str);
                files.push(PathBuf::from(clean));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn cached_returns_same_result() {
        let dir = tempfile::tempdir().unwrap();
        let makefile = dir.path().join("Oxymakefile.toml");
        fs::write(&makefile, "").unwrap();
        fs::write(dir.path().join("a.txt"), "").unwrap();
        fs::write(dir.path().join("b.txt"), "").unwrap();

        let first = discover_existing_files(&makefile);
        let second = discover_existing_files(&makefile);
        assert_eq!(first, second);
        assert_eq!(first.len(), 3); // Oxymakefile.toml, a.txt, b.txt
    }

    #[test]
    fn invalidate_clears_cache() {
        let dir = tempfile::tempdir().unwrap();
        let makefile = dir.path().join("Oxymakefile.toml");
        fs::write(&makefile, "").unwrap();

        let first = discover_existing_files(&makefile);
        invalidate();
        // After invalidation, a new walk should still return same results
        let second = discover_existing_files(&makefile);
        assert_eq!(first, second);
    }

    #[test]
    fn skips_hidden_and_target_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let makefile = dir.path().join("Oxymakefile.toml");
        fs::write(&makefile, "").unwrap();

        fs::create_dir(dir.path().join(".hidden")).unwrap();
        fs::write(dir.path().join(".hidden/secret.txt"), "").unwrap();
        fs::create_dir(dir.path().join("target")).unwrap();
        fs::write(dir.path().join("target/build.o"), "").unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "").unwrap();

        invalidate();
        let files = discover_existing_files(&makefile);
        let names: Vec<&str> = files.iter().map(|p| p.to_str().unwrap()).collect();
        assert!(names.contains(&"src/main.rs"));
        assert!(!names.iter().any(|n| n.contains(".hidden")));
        assert!(!names.iter().any(|n| n.contains("target")));
    }

    #[test]
    fn discovers_files_in_path_with_spaces() {
        let dir = tempfile::tempdir().unwrap();
        let spaced = dir.path().join("my project");
        fs::create_dir(&spaced).unwrap();
        let makefile = spaced.join("Oxymakefile.toml");
        fs::write(&makefile, "").unwrap();
        fs::create_dir(spaced.join("sub dir")).unwrap();
        fs::write(spaced.join("sub dir/data file.csv"), "").unwrap();
        fs::write(spaced.join("script.py"), "").unwrap();

        invalidate();
        let files = discover_existing_files(&makefile);
        let names: Vec<String> = files
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"Oxymakefile.toml".to_string()));
        assert!(names.contains(&"script.py".to_string()));
        assert!(names.contains(&"sub dir/data file.csv".to_string()));
    }

    #[test]
    fn discovers_files_with_unicode_names() {
        let dir = tempfile::tempdir().unwrap();
        let makefile = dir.path().join("Oxymakefile.toml");
        fs::write(&makefile, "").unwrap();
        fs::create_dir(dir.path().join("données")).unwrap();
        fs::write(dir.path().join("données/résultats.csv"), "").unwrap();
        fs::write(dir.path().join("日本語.txt"), "").unwrap();

        invalidate();
        let files = discover_existing_files(&makefile);
        let names: Vec<String> = files
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"données/résultats.csv".to_string()));
        assert!(names.contains(&"日本語.txt".to_string()));
    }

    #[test]
    fn discovers_files_through_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let makefile = dir.path().join("Oxymakefile.toml");
        fs::write(&makefile, "").unwrap();

        // Create a real directory with a file, then symlink to it.
        let real_dir = dir.path().join("real_data");
        fs::create_dir(&real_dir).unwrap();
        fs::write(real_dir.join("input.csv"), "data").unwrap();

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&real_dir, dir.path().join("linked_data")).unwrap();

            invalidate();
            let files = discover_existing_files(&makefile);
            let names: Vec<String> = files
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            assert!(names.contains(&"linked_data/input.csv".to_string()));
            assert!(names.contains(&"real_data/input.csv".to_string()));
        }
    }

    #[test]
    fn discovers_files_through_symlinked_file() {
        let dir = tempfile::tempdir().unwrap();
        let makefile = dir.path().join("Oxymakefile.toml");
        fs::write(&makefile, "").unwrap();

        let real_file = dir.path().join("real.csv");
        fs::write(&real_file, "data").unwrap();

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&real_file, dir.path().join("link.csv")).unwrap();

            invalidate();
            let files = discover_existing_files(&makefile);
            let names: Vec<String> = files
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            assert!(names.contains(&"real.csv".to_string()));
            assert!(names.contains(&"link.csv".to_string()));
        }
    }

    #[test]
    fn discovers_files_in_unicode_project_path() {
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("émmanuel/données");
        fs::create_dir_all(&proj).unwrap();
        let makefile = proj.join("Oxymakefile.toml");
        fs::write(&makefile, "").unwrap();
        fs::write(proj.join("output.npz"), "").unwrap();

        invalidate();
        let files = discover_existing_files(&makefile);
        let names: Vec<String> = files
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"Oxymakefile.toml".to_string()));
        assert!(names.contains(&"output.npz".to_string()));
    }
}
