//! Computation-entry publication and verified restoration (ADR-022).
use super::run::CacheKeyComponents;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use ox_cache::{CacheStore, current_platform, hash_file};
use ox_cache_remote::RemoteCache;
use ox_core::model::{ArtifactProvenance, ContentHash, JobId, PlatformScope};
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

#[derive(Deserialize)]
struct EntryHeader {
    kind: String,
    format_version: u32,
}

/// A newer document must survive publication by an older producer.
struct NewerEntryVersion;

/// One invocation shares defect warnings between the pre-scan and scheduler.
pub(super) struct RemoteSession {
    backend: Arc<dyn RemoteCache>,
    warnings: Mutex<HashSet<&'static str>>,
    restore_exclusions: HashSet<JobId>,
    scanned_restore_parents: Mutex<HashSet<PathBuf>>,
}

impl RemoteSession {
    pub fn new(backend: Arc<dyn RemoteCache>, restore_exclusions: HashSet<JobId>) -> Self {
        Self {
            backend,
            warnings: Mutex::new(HashSet::new()),
            restore_exclusions,
            scanned_restore_parents: Mutex::new(HashSet::new()),
        }
    }

    fn warn(&self, defect: &'static str) {
        if self.warnings.lock().unwrap().insert(defect) {
            eprintln!("warning: remote cache entry {defect}; treating it as a miss");
        }
    }

    async fn read(&self, key: &ContentHash) -> Result<Option<RemoteEntry>, NewerEntryVersion> {
        let bytes = match self.backend.fetch_entry(key).await {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Ok(None),
            Err(_) => {
                self.warn("read failed or exceeded 1 MiB");
                return Ok(None);
            }
        };
        if bytes.len() > 1024 * 1024 {
            self.warn("exceeded 1 MiB");
            return Ok(None);
        }
        let header: EntryHeader = match serde_json::from_slice(&bytes) {
            Ok(header) => header,
            Err(_) => {
                self.warn("could not be parsed");
                return Ok(None);
            }
        };
        if header.format_version > 1 {
            self.warn("has an unsupported version");
            return Err(NewerEntryVersion);
        }
        if header.kind != ENTRY_KIND {
            self.warn("has an unknown kind");
            return Ok(None);
        }
        if header.format_version != 1 {
            self.warn("has an unsupported version");
            return Ok(None);
        }
        let entry: RemoteEntry = match serde_json::from_slice(&bytes) {
            Ok(entry) => entry,
            Err(_) => {
                self.warn("could not be parsed");
                return Ok(None);
            }
        };
        if entry.cache_key != *key {
            self.warn("has a different computation key");
            return Ok(None);
        }
        Ok(Some(entry))
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
        let old = match self.read(key).await {
            Ok(old) => old,
            Err(NewerEntryVersion) => return,
        };
        if let Some(old) = old {
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
        // Selection is known before either pre-scan or scheduler lookup.
        if self.restore_exclusions.contains(&components.job_id) {
            return false;
        }
        let key = &components.cache_key;
        let refs: Vec<_> = outputs
            .iter()
            .map(|p| (p.to_string_lossy().into_owned(), p.as_path()))
            .collect::<BTreeMap<_, _>>()
            .into_values()
            .collect();
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
            let Ok(Some(entry)) = self.read(key).await else {
                return false;
            };
            let mut result = entry.result;
            let defect = if result.provenance.job_spec_hash != components.job_spec_hash {
                Some("has a different job specification")
            } else if result.provenance.input_hashes != components.input_hashes {
                Some("has different input provenance")
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
            let declared_paths: BTreeSet<_> =
                outputs.iter().map(|p| normalized_output(p, root)).collect();
            let entry_paths: BTreeSet<_> = result
                .outputs
                .iter()
                .map(|o| normalized_output(Path::new(&o.path), root))
                .collect();
            if declared_paths != entry_paths {
                self.warn("has different output paths");
                return false;
            }
            let mut paths = BTreeMap::new();
            for output in &result.outputs {
                let path = normalized_output(Path::new(&output.path), root);
                if paths
                    .insert(path, output.content_hash.as_str())
                    .is_some_and(|previous| previous != output.content_hash)
                {
                    self.warn("has conflicting output hashes");
                    return false;
                }
            }
            let hashes = outputs
                .iter()
                .map(|p| ContentHash::from_hex(paths[normalized_output(p, root).as_str()]))
                .collect::<Result<Vec<_>, _>>();
            let Ok(hashes) = hashes else {
                self.warn("has an invalid output hash");
                return false;
            };
            // Reproducibility is the reader's policy, not a producer grant.
            result.provenance.reproducibility = components.reproducibility;
            // Fetches are completed before the local row is created below.
            if !self.restore_files(outputs, &hashes, true).await {
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
        if !self.restore_files(outputs, &hashes, false).await {
            return false;
        }
        store.is_cached(key, &refs).unwrap_or(false)
    }

    async fn restore_files(
        &self,
        outputs: &[PathBuf],
        hashes: &[ContentHash],
        from_entry: bool,
    ) -> bool {
        let mut staged = Vec::new();
        for (output, hash) in outputs.iter().zip(hashes) {
            if hash_file(output).ok().as_ref() == Some(hash) {
                continue;
            }
            if let Some(parent) = output.parent() {
                // Mark before scanning so concurrent restores share the same
                // one-scan budget, including failed directory reads.
                if self
                    .scanned_restore_parents
                    .lock()
                    .unwrap()
                    .insert(parent.to_path_buf())
                {
                    cleanup_restore_staging(parent);
                }
            }
            let staging = match reserve_restore_staging(output) {
                Ok(path) => path,
                Err(e) => {
                    if output
                        .file_name()
                        .is_some_and(|n| n.as_encoded_bytes().len() > 231)
                    {
                        self.warn("output filename is too long for remote restore staging");
                    } else {
                        eprintln!(
                            "warning: remote cache staging for {} failed: {e}",
                            output.display()
                        );
                    }
                    for (path, _) in &staged {
                        let _ = tokio::fs::remove_file(path).await;
                    }
                    return false;
                }
            };
            // Include the current path before fetching: a failing backend may leave bytes.
            staged.push((staging.clone(), output));
            let fetched = match self.backend.fetch(hash, &staging).await {
                Ok(true) => true,
                failure => {
                    if from_entry {
                        self.warn("has a missing, corrupt or unreadable blob");
                    } else if let Err(e) = failure {
                        eprintln!(
                            "warning: remote cache fetch for {} failed: {e}",
                            output.display()
                        );
                    }
                    false
                }
            };
            if !fetched {
                for (path, _) in &staged {
                    let _ = tokio::fs::remove_file(path).await;
                }
                return false;
            }
        }
        for (staging, output) in &staged {
            if let Err(e) = tokio::fs::rename(staging, output).await {
                eprintln!(
                    "warning: remote cache output installation for {} failed: {e}",
                    output.display()
                );
                for (path, _) in &staged {
                    let _ = tokio::fs::remove_file(path).await;
                }
                return false;
            }
        }
        true
    }
}

// A 24-byte suffix leaves 231 bytes for the output on NAME_MAX=255
// filesystems. Exclusive creation also protects against PID reuse and another
// machine using the same PID; a collision simply takes the next counter.
fn reserve_restore_staging(output: &Path) -> std::io::Result<PathBuf> {
    let name = output.file_name().unwrap_or_default();
    if name.as_encoded_bytes().len() > 231 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "output filename is too long for remote restore staging",
        ));
    }
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    loop {
        let counter = STAGING_COUNTER
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                (n <= u32::MAX as u64).then_some(n + 1)
            })
            .map_err(|_| std::io::Error::other("remote restore staging counter exhausted"))?;
        let mut staging_name = name.to_os_string();
        staging_name.push(format!(".remote.{:08x}{counter:08x}", std::process::id()));
        let staging = output.with_file_name(staging_name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
        {
            Ok(_) => return Ok(staging),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

fn is_restore_suffix(suffix: &str) -> bool {
    if suffix.len() == 16 && suffix.bytes().all(|b| b.is_ascii_hexdigit()) {
        return true;
    }
    // Also collect crash debris from the previous UUID-based naming scheme.
    suffix
        .strip_prefix("run-")
        .and_then(|s| s.rsplit_once('.'))
        .is_some_and(|(id, counter)| {
            uuid::Uuid::parse_str(id).is_ok()
                && !counter.is_empty()
                && counter.bytes().all(|b| b.is_ascii_digit())
        })
}

fn cleanup_restore_staging(parent: &Path) {
    use std::time::{Duration, SystemTime};
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        // Search bytes so a non-UTF-8 output basename is still recognized.
        // The last marker also handles basenames that themselves contain it.
        let name = file_name.as_encoded_bytes();
        let marker = b".remote.";
        let Some(offset) = name.windows(marker.len()).rposition(|part| part == marker) else {
            continue;
        };
        if !std::str::from_utf8(&name[offset + marker.len()..]).is_ok_and(is_restore_suffix) {
            continue;
        }
        let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        // One hour is a conservative crash-debris threshold. Fresh staging
        // belongs to a potentially active restore and must be left alone.
        if metadata.is_file()
            && metadata
                .modified()
                .ok()
                .and_then(|mtime| SystemTime::now().duration_since(mtime).ok())
                .is_some_and(|age| age > Duration::from_secs(3600))
        {
            let _ = std::fs::remove_file(entry.path());
        }
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
                job_id: JobId("test".into()),
                cache_key: ContentHash::from(blake3::hash(b"key")),
                input_hashes: vec![],
                job_spec_hash: "spec".into(),
                reproducibility: ReproducibilityClass::Deterministic,
                params_hash: None,
                env_hash: None,
            };
            let session = RemoteSession::new(
                Arc::new(DirectoryCache::with_run_id(
                    &remote,
                    "run-00000000-0000-4000-8000-000000000001",
                )),
                HashSet::new(),
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
    async fn restore_accepts_an_entry_of_exactly_one_mib() {
        let f = Fixture::new();
        f.publish().await;
        let key = f.components.cache_key.as_str();
        let entry = f
            .remote
            .join("entries")
            .join(&key[..2])
            .join(format!("{key}.json"));
        let mut bytes = std::fs::read(&entry).unwrap();
        // JSON permits trailing whitespace; the transport limit includes it.
        bytes.resize(1024 * 1024, b' ');
        std::fs::write(entry, bytes).unwrap();
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
        assert_eq!(std::fs::read(f.root.join("a")).unwrap(), b"first");
        assert_eq!(std::fs::read(f.root.join("b")).unwrap(), b"second");
        assert!(store.get(&f.components.cache_key).unwrap().restored);
        assert!(f.session.warnings.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn restore_staging_supports_long_names_and_reports_name_limit() {
        let f = Fixture::new();
        f.publish().await;
        let hash = ContentHash::from_hex(&f.result.outputs[0].content_hash).unwrap();
        let output = f.root.join("x".repeat(231));
        assert!(
            f.session
                .restore_files(&[output.clone()], &[hash.clone()], true)
                .await
        );
        assert_eq!(std::fs::read(&output).unwrap(), b"first");
        let too_long = f.root.join("x".repeat(232));
        assert!(!f.session.restore_files(&[too_long], &[hash], true).await);
        let warnings = f.session.warnings.lock().unwrap();
        assert!(warnings.contains("output filename is too long for remote restore staging"));
        assert!(!warnings.contains("has a missing, corrupt or unreadable blob"));
    }

    #[tokio::test]
    async fn restore_removes_only_old_recognized_staging_in_parent() {
        use std::time::{Duration, SystemTime};
        let f = Fixture::new();
        f.publish().await;
        let old = [
            "a.remote.0000000100000001",
            "b.remote.0000000100000001",
            "nested.remote.name.remote.0000000100000001",
            "a.remote.run-00000000-0000-4000-8000-000000000001.0",
        ];
        let keep = [
            "a.remote.0000000100000002",
            "a.remote.notes",
            "a.remote.0000000100000001.extra",
        ];
        for name in old.iter().chain(&keep) {
            let file = std::fs::File::create(f.root.join(name)).unwrap();
            if *name != keep[0] {
                file.set_modified(SystemTime::now() - Duration::from_secs(7200))
                    .unwrap();
            }
        }
        let directory = f.root.join("directory.remote.0000000100000001");
        std::fs::create_dir(&directory).unwrap();
        std::fs::File::open(&directory)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(7200))
            .unwrap();
        #[cfg(unix)]
        let symlink = {
            let target = f.root.join("symlink-target");
            std::fs::write(&target, "keep").unwrap();
            std::fs::File::open(&target)
                .unwrap()
                .set_modified(SystemTime::now() - Duration::from_secs(7200))
                .unwrap();
            let link = f.root.join("link.remote.0000000100000001");
            std::os::unix::fs::symlink(target, &link).unwrap();
            link
        };
        let hash = ContentHash::from_hex(&f.result.outputs[0].content_hash).unwrap();
        assert!(
            f.session
                .restore_files(&[f.root.join("a")], &[hash], true)
                .await
        );
        for name in old {
            assert!(!f.root.join(name).exists(), "{name}");
        }
        for name in keep {
            assert!(f.root.join(name).exists(), "{name}");
        }
        assert!(directory.is_dir());
        #[cfg(unix)]
        assert!(symlink.is_symlink());
    }

    #[tokio::test]
    async fn restore_scans_parent_only_on_first_needed_fetch_in_session() {
        use std::time::{Duration, SystemTime};
        let f = Fixture::new();
        f.publish().await;
        let debris = f.root.join("a.remote.0000000100000001");
        let plant_debris = || {
            std::fs::File::create(&debris)
                .unwrap()
                .set_modified(SystemTime::now() - Duration::from_secs(7200))
                .unwrap();
        };
        let hash = ContentHash::from_hex(&f.result.outputs[0].content_hash).unwrap();
        let output = f.root.join("a");
        plant_debris();
        std::fs::write(&output, "first").unwrap();
        assert!(
            f.session
                .restore_files(
                    std::slice::from_ref(&output),
                    std::slice::from_ref(&hash),
                    true
                )
                .await
        );
        assert!(debris.exists(), "matching output must not trigger cleanup");
        std::fs::remove_file(&output).unwrap();
        assert!(
            f.session
                .restore_files(
                    std::slice::from_ref(&output),
                    std::slice::from_ref(&hash),
                    true
                )
                .await
        );
        assert!(!debris.exists(), "first needed fetch must scan the parent");
        plant_debris();
        // Many missing outputs share this already-scanned parent. None should
        // trigger another directory enumeration, even in a later restore call.
        let outputs: Vec<_> = (0..1024)
            .map(|n| f.root.join(format!("fanout-{n}")))
            .collect();
        let hashes = vec![hash; outputs.len()];
        assert!(f.session.restore_files(&outputs, &hashes, true).await);
        assert!(debris.exists(), "same session must not rescan this parent");
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

    #[tokio::test]
    async fn restore_matches_normalized_output_sets_including_repeated_names() {
        for case in ["entry_alias", "declared_duplicate", "entry_duplicate"] {
            let mut f = Fixture::new();
            if case == "entry_alias" {
                f.result.outputs[0].path = "unused/../a".into();
            }
            if case == "entry_duplicate" {
                f.result.outputs.push(CacheOutput {
                    path: "./a".into(),
                    content_hash: f.result.outputs[0].content_hash.clone(),
                });
            }
            f.publish().await;
            let mut outputs = f.outputs(&f.root);
            if case == "declared_duplicate" {
                outputs.push(f.root.join("a"));
            }
            let mut store = f.store();
            assert!(
                f.session
                    .restore(
                        &mut store,
                        &f.components,
                        &outputs,
                        PlatformScope::Exact,
                        &f.root
                    )
                    .await,
                "{case}"
            );
            assert_eq!(std::fs::read_to_string(f.root.join("a")).unwrap(), "first");
            assert_eq!(std::fs::read_to_string(f.root.join("b")).unwrap(), "second");
        }
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
