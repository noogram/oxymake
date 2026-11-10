//! Computation-entry publication and verified restoration (ADR-022).
use super::run::CacheKeyComponents;
use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use ox_cache::{CacheStore, current_platform, hash_file};
use ox_cache_remote::RemoteCache;
use ox_core::model::{ArtifactProvenance, ContentHash, PlatformScope};
use serde::{Deserialize, Serialize};

pub(super) const ENTRY_KIND: &str = "oxymake.remote-cache-entry";
static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Wire fields shared by remote entries and explicit adoption entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct CacheResult {
    pub outputs: Vec<CacheOutput>,
    pub provenance: ArtifactProvenance,
    pub origin_platform: String,
    pub platform_scope: PlatformScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct CacheOutput {
    pub path: String,
    pub content_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct RemoteEntry {
    kind: String,
    format_version: u32,
    producer_version: String,
    key_format: String,
    cache_key: ContentHash,
    #[serde(flatten)]
    result: CacheResult,
}

/// One invocation shares defect warnings between the pre-scan and scheduler.
pub(super) struct RemoteSession {
    backend: Arc<dyn RemoteCache>,
    run_id: String,
    warnings: Mutex<HashSet<&'static str>>,
}

impl RemoteSession {
    pub fn new(backend: Arc<dyn RemoteCache>, run_id: String) -> Self {
        Self {
            backend,
            run_id,
            warnings: Mutex::new(HashSet::new()),
        }
    }

    fn warn(&self, defect: &'static str) {
        if self.warnings.lock().unwrap().insert(defect) {
            eprintln!("warning: remote cache entry {defect}; treating it as a miss");
        }
    }

    async fn read(&self, key: &ContentHash) -> Option<RemoteEntry> {
        let bytes = match self.backend.fetch_entry(key).await {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return None,
            Err(_) => {
                self.warn("read failed or exceeded 1 MiB");
                return None;
            }
        };
        if bytes.len() > 1024 * 1024 {
            self.warn("exceeded 1 MiB");
            return None;
        }
        let entry: RemoteEntry = match serde_json::from_slice(&bytes) {
            Ok(entry) => entry,
            Err(_) => {
                self.warn("could not be parsed");
                return None;
            }
        };
        let defect = if entry.kind != ENTRY_KIND {
            Some("has an unknown kind")
        } else if entry.format_version != 1 {
            Some("has an unsupported version")
        } else if entry.cache_key != *key {
            Some("has a different computation key")
        } else {
            None
        };
        if let Some(defect) = defect {
            self.warn(defect);
            return None;
        }
        Some(entry)
    }

    pub async fn publish(&self, key: &ContentHash, result: CacheResult, outputs: &[PathBuf]) {
        for (output, record) in outputs.iter().zip(&result.outputs) {
            let Ok(hash) = ContentHash::from_hex(&record.content_hash) else {
                return;
            };
            if let Err(e) = self.backend.store(&hash, output).await {
                eprintln!(
                    "warning: remote cache store for {} failed: {e}",
                    output.display()
                );
                return;
            }
        }
        // A forced execution replaces old hashes; identical entries need no write.
        if let Some(old) = self.read(key).await {
            let hashes = |r: &CacheResult| {
                r.outputs
                    .iter()
                    .map(|o| (o.path.clone(), o.content_hash.clone()))
                    .collect::<BTreeMap<_, _>>()
            };
            if old.result.outputs.len() == result.outputs.len()
                && hashes(&old.result).len() == old.result.outputs.len()
                && (hashes(&old.result) == hashes(&result)
                    && old.result.provenance == result.provenance
                    && old.result.origin_platform == result.origin_platform
                    && old.result.platform_scope == result.platform_scope)
            {
                let mut complete = true;
                for output in &old.result.outputs {
                    let Ok(hash) = ContentHash::from_hex(&output.content_hash) else {
                        complete = false;
                        break;
                    };
                    if !self.backend.exists(&hash).await.unwrap_or(false) {
                        complete = false;
                        break;
                    }
                }
                if complete {
                    return;
                }
            }
        }
        let entry = RemoteEntry {
            kind: ENTRY_KIND.into(),
            format_version: 1,
            producer_version: env!("CARGO_PKG_VERSION").into(),
            key_format: ox_cache::CACHE_KEY_FORMAT_VERSION.into(),
            cache_key: key.clone(),
            result,
        };
        let bytes = match serde_json::to_vec(&entry) {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!("warning: remote cache entry serialization failed: {e}");
                return;
            }
        };
        if let Err(e) = self.backend.store_entry(key, &bytes).await {
            eprintln!("warning: remote cache entry publication failed: {e}");
        }
    }

    /// The entry supplies hashes only; every destination comes from `outputs`.
    pub async fn restore(
        &self,
        store: &mut CacheStore,
        components: &CacheKeyComponents,
        outputs: &[PathBuf],
        scope: PlatformScope,
        root: &Path,
    ) -> bool {
        let key = &components.cache_key;
        let refs: Vec<_> = outputs.iter().map(PathBuf::as_path).collect();
        if store.is_cached(key, &refs).unwrap_or(false) {
            return true;
        }
        let local = store.get(key);
        let hashes: Vec<ContentHash> = if let Some(entry) = &local {
            let Some(hashes) = outputs
                .iter()
                .map(|p| {
                    entry
                        .output_hashes
                        .get(p.to_string_lossy().as_ref())
                        .cloned()
                })
                .collect()
            else {
                return false;
            };
            hashes
        } else {
            let Some(entry) = self.read(key).await else {
                return false;
            };
            let result = entry.result;
            let defect = if result.provenance.job_spec_hash != components.job_spec_hash {
                Some("has a different job specification")
            } else if result.platform_scope != scope {
                Some("has a different platform scope")
            } else if scope == PlatformScope::Exact && result.origin_platform != current_platform()
            {
                Some("has a different origin platform")
            } else {
                None
            };
            if let Some(defect) = defect {
                self.warn(defect);
                return false;
            }
            let paths: BTreeMap<_, _> = result
                .outputs
                .iter()
                .map(|o| (o.path.as_str(), o.content_hash.as_str()))
                .collect();
            if paths.len() != result.outputs.len()
                || paths.len() != outputs.len()
                || outputs
                    .iter()
                    .any(|p| !paths.contains_key(normalized_output(p, root).as_str()))
            {
                self.warn("has different or duplicate output paths");
                return false;
            }
            let hashes = outputs
                .iter()
                .map(|p| ContentHash::from_hex(paths[normalized_output(p, root).as_str()]))
                .collect::<Result<Vec<_>, _>>();
            let Ok(hashes) = hashes else {
                self.warn("has an invalid output hash");
                return false;
            };
            // Fetches are completed before the local row is created below.
            if !self.restore_files(outputs, &hashes).await {
                return false;
            }
            if let Err(e) = store.record_restored(
                key.clone(),
                &refs,
                &result.provenance,
                &result.origin_platform,
                result.platform_scope,
            ) {
                eprintln!("warning: remote cache local recording failed: {e}");
                return false;
            }
            return store.is_cached(key, &refs).unwrap_or(false);
        };
        if !self.restore_files(outputs, &hashes).await {
            return false;
        }
        store.is_cached(key, &refs).unwrap_or(false)
    }

    async fn restore_files(&self, outputs: &[PathBuf], hashes: &[ContentHash]) -> bool {
        let mut staged = Vec::new();
        for (output, hash) in outputs.iter().zip(hashes) {
            if hash_file(output).ok().as_ref() == Some(hash) {
                continue;
            }
            let staging = output.with_file_name(format!(
                "{}.remote.{}.{}",
                output.file_name().unwrap_or_default().to_string_lossy(),
                self.run_id,
                STAGING_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            // Include the current path before fetching: a failing backend may leave bytes.
            staged.push((staging.clone(), output));
            if !matches!(self.backend.fetch(hash, &staging).await, Ok(true)) {
                self.warn("has a missing, corrupt or unreadable blob");
                for (path, _) in &staged {
                    let _ = tokio::fs::remove_file(path).await;
                }
                return false;
            }
        }
        for (staging, output) in &staged {
            if let Err(e) = tokio::fs::rename(staging, output).await {
                eprintln!("warning: remote cache output installation failed: {e}");
                for (path, _) in &staged {
                    let _ = tokio::fs::remove_file(path).await;
                }
                return false;
            }
        }
        true
    }
}

/// Lexical ADR-021 normalization; never resolve symlinks on either side.
pub(super) fn normalized_output(path: &Path, root: &Path) -> String {
    fn normalize(path: &Path) -> PathBuf {
        let mut out = PathBuf::new();
        for part in path.components() {
            match part {
                Component::CurDir => (),
                Component::ParentDir => {
                    out.pop();
                }
                other => out.push(other.as_os_str()),
            }
        }
        out
    }
    let root = normalize(root);
    let path = normalize(&root.join(path));
    path.strip_prefix(&root)
        .unwrap_or(&path)
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ox_cache_remote::DirectoryCache;
    use ox_core::model::ReproducibilityClass;

    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        writer: PathBuf,
        remote: PathBuf,
        session: RemoteSession,
        components: CacheKeyComponents,
        result: CacheResult,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("reader");
            let writer = dir.path().join("writer");
            let remote = dir.path().join("remote");
            std::fs::create_dir_all(&root).unwrap();
            std::fs::create_dir_all(&writer).unwrap();
            let outputs = [("a", "first"), ("b", "second")]
                .into_iter()
                .map(|(path, bytes)| {
                    std::fs::write(writer.join(path), bytes).unwrap();
                    CacheOutput {
                        path: path.into(),
                        content_hash: blake3::hash(bytes.as_bytes()).to_hex().to_string(),
                    }
                })
                .collect();
            let result = CacheResult {
                outputs,
                provenance: ArtifactProvenance {
                    input_hashes: vec![],
                    job_spec_hash: "spec".into(),
                    reproducibility: ReproducibilityClass::Deterministic,
                },
                origin_platform: current_platform(),
                platform_scope: PlatformScope::Exact,
            };
            let components = CacheKeyComponents {
                cache_key: ContentHash::from(blake3::hash(b"key")),
                input_hashes: vec![],
                job_spec_hash: "spec".into(),
                params_hash: None,
                env_hash: None,
            };
            let session = RemoteSession::new(
                Arc::new(DirectoryCache::with_run_id(
                    &remote,
                    "run-00000000-0000-4000-8000-000000000001",
                )),
                "run-00000000-0000-4000-8000-000000000001".into(),
            );
            Self {
                _dir: dir,
                root,
                writer,
                remote,
                session,
                components,
                result,
            }
        }
        fn outputs(&self, root: &Path) -> Vec<PathBuf> {
            vec![root.join("a"), root.join("b")]
        }
        fn blob(&self, index: usize) -> PathBuf {
            let hash = &self.result.outputs[index].content_hash;
            self.remote.join(&hash[..2]).join(hash)
        }
        async fn publish(&self) {
            self.session
                .publish(
                    &self.components.cache_key,
                    self.result.clone(),
                    &self.outputs(&self.writer),
                )
                .await;
        }
        fn store(&self) -> CacheStore {
            CacheStore::open_with(
                &self.root.join("state"),
                ox_cache::CacheValidation::ContentHash,
            )
            .unwrap()
        }
    }

    #[tokio::test]
    async fn failed_fetch_preserves_every_declared_output_and_removes_staging() {
        for corrupt in [false, true] {
            let f = Fixture::new();
            f.publish().await;
            if corrupt {
                std::fs::write(f.blob(1), "corrupt").unwrap();
            } else {
                std::fs::remove_file(f.blob(1)).unwrap();
            }
            for path in f.outputs(&f.root) {
                std::fs::write(path, "keep me").unwrap();
            }
            let mut store = f.store();
            assert!(
                !f.session
                    .restore(
                        &mut store,
                        &f.components,
                        &f.outputs(&f.root),
                        PlatformScope::Exact,
                        &f.root
                    )
                    .await
            );
            for path in f.outputs(&f.root) {
                assert_eq!(std::fs::read_to_string(path).unwrap(), "keep me");
            }
            assert!(store.get(&f.components.cache_key).is_none());
            assert!(!std::fs::read_dir(&f.root).unwrap().any(|p| {
                p.unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".remote.")
            }));
        }
    }

    #[tokio::test]
    async fn matching_outputs_need_no_blobs_and_preserve_foreign_origin_under_any() {
        let mut f = Fixture::new();
        f.result.platform_scope = PlatformScope::Any;
        f.result.origin_platform = "other/arch".into();
        f.publish().await;
        for (src, dest) in f.outputs(&f.writer).iter().zip(f.outputs(&f.root)) {
            std::fs::copy(src, dest).unwrap();
        }
        for i in 0..2 {
            std::fs::remove_file(f.blob(i)).unwrap();
        }
        let mut store = f.store();
        assert!(
            f.session
                .restore(
                    &mut store,
                    &f.components,
                    &f.outputs(&f.root),
                    PlatformScope::Any,
                    &f.root
                )
                .await
        );
        let row = store.get(&f.components.cache_key).unwrap();
        assert!(row.restored);
        assert!(!row.adopted);
        assert_eq!(row.platform.as_deref(), Some("other/arch"));
        assert_eq!(row.provenance, Some(f.result.provenance));
    }

    #[tokio::test]
    async fn blob_store_failure_never_publishes_an_entry() {
        let f = Fixture::new();
        std::fs::create_dir_all(f.blob(1)).unwrap();
        f.publish().await;
        assert!(f.blob(0).is_file());
        assert!(!f.remote.join("entries").exists());
    }

    #[tokio::test]
    async fn identical_publication_is_unchanged_and_forced_hashes_replace_entry() {
        let mut f = Fixture::new();
        f.publish().await;
        let path = f
            .remote
            .join("entries")
            .join(&f.components.cache_key.as_str()[..2])
            .join(format!("{}.json", f.components.cache_key.as_str()));
        // Add an allowed future field; retaining it proves no replacement took place.
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["future_field"] = true.into();
        let bytes = serde_json::to_vec(&value).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        f.publish().await;
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::write(f.writer.join("a"), "new result").unwrap();
        f.result.outputs[0].content_hash = blake3::hash(b"new result").to_hex().to_string();
        f.publish().await;
        let stored: RemoteEntry = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(stored.result, f.result);
        let mut store = f.store();
        assert!(
            f.session
                .restore(
                    &mut store,
                    &f.components,
                    &f.outputs(&f.root),
                    PlatformScope::Exact,
                    &f.root
                )
                .await
        );
        assert_eq!(
            std::fs::read_to_string(f.root.join("a")).unwrap(),
            "new result"
        );
    }

    #[test]
    fn output_names_are_lexical_and_relative_only_inside_root() {
        assert_eq!(
            normalized_output(Path::new("build/../a"), Path::new("/project")),
            "a"
        );
        assert_eq!(
            normalized_output(Path::new("/project/./a"), Path::new("/project")),
            "a"
        );
        assert_eq!(
            normalized_output(Path::new("../outside/a"), Path::new("/project")),
            "/outside/a"
        );
    }
}
