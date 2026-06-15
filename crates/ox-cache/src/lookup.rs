//! Cache store — persistence and lookup for cached job results.
//!
//! The [`CacheStore`] stores cache entries in a SQLite database, mapping
//! cache keys to [`CacheEntry`] records. Each record tracks the content
//! hashes and mtime/size metadata of a job's output files, enabling fast
//! hit/miss determination on subsequent runs.
//!
//! Previously this was backed by a JSON manifest that was loaded entirely
//! into memory. The SQLite backend avoids unbounded memory growth for
//! long-lived projects with many cached targets.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use ox_core::model::ContentHash;

use crate::error::CacheError;
use crate::hash;
use crate::strategy::CacheValidation;

/// Cached record for a completed job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    /// The cache key that produced this entry.
    pub cache_key: ContentHash,
    /// Map from output file path (canonical string) to its content hash.
    pub output_hashes: BTreeMap<String, ContentHash>,
    /// Map from output file path to (mtime_secs, size) for the fast path.
    pub output_mtimes: BTreeMap<String, (u64, u64)>,
    /// Unix timestamp (seconds) when this job completed.
    pub completed_at: u64,
    /// Provenance metadata for cache correctness (Stage 2).
    pub provenance: Option<ox_core::model::ArtifactProvenance>,
}

/// Result of checking whether a job's cached outputs are still valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheHitStatus {
    /// All outputs match their cached hashes — skip execution.
    Hit,
    /// No cache entry exists for this cache key (or entry is missing an output).
    Miss,
    /// A cached output file no longer exists on disk.
    OutputMissing {
        /// The path of the missing output file.
        path: String,
    },
    /// An output file's content hash differs from the stored hash.
    /// The stale cache entry has been removed — re-execution will self-heal.
    Mismatch {
        /// The path of the output whose hash didn't match.
        path: String,
    },
}

impl CacheHitStatus {
    /// Returns `true` if this is a cache hit.
    pub fn is_hit(&self) -> bool {
        matches!(self, Self::Hit)
    }
}

/// The cache store backed by a SQLite database.
///
/// The database lives at `<oxymake_dir>/cache/cache.db`. On first open,
/// any existing `manifest.json` is migrated automatically.
pub struct CacheStore {
    conn: Connection,
    /// Kept for legacy migration detection.
    cache_dir: PathBuf,
    /// How output files are validated against cache entries.
    validation: CacheValidation,
    /// In-memory session cache for input file hashes (avoids repeated SQLite lookups).
    input_memo: HashMap<PathBuf, ContentHash>,
}

impl CacheStore {
    /// Open or create the cache store.
    ///
    /// If a legacy `manifest.json` exists, it is migrated into SQLite and
    /// then renamed to `manifest.json.migrated`. The parent directories
    /// are created as needed.
    /// Open or create the cache store with the default validation strategy.
    pub fn open(oxymake_dir: &Path) -> Result<Self, CacheError> {
        Self::open_with(oxymake_dir, CacheValidation::default())
    }

    /// Open or create the cache store with a specific validation strategy.
    pub fn open_with(oxymake_dir: &Path, validation: CacheValidation) -> Result<Self, CacheError> {
        let cache_dir = oxymake_dir.join("cache");
        std::fs::create_dir_all(&cache_dir)?;
        let db_path = cache_dir.join("cache.db");

        let conn = Connection::open(&db_path)
            .map_err(|e| CacheError::Manifest(format!("sqlite open: {e}")))?;

        // WAL mode for concurrent reads + single-writer performance.
        conn.pragma_update(None, "journal_mode", "wal")
            .map_err(|e| CacheError::Manifest(format!("sqlite pragma: {e}")))?;

        // NOTE: The `mtime_secs` column now stores nanoseconds (not seconds)
        // for sub-second precision. The column name is kept for backward
        // compatibility with existing databases. Old entries with second-level
        // values will fail the mtime fast-path check and fall through to a
        // full content hash verification, then get re-recorded with nanos.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cache_entries (
                cache_key   TEXT PRIMARY KEY,
                completed_at INTEGER NOT NULL,
                reproducibility_class TEXT,
                input_hashes_json TEXT,
                job_spec_hash TEXT
            );
            CREATE TABLE IF NOT EXISTS output_records (
                cache_key    TEXT NOT NULL,
                path         TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                mtime_secs   INTEGER NOT NULL,
                size         INTEGER NOT NULL,
                PRIMARY KEY (cache_key, path),
                FOREIGN KEY (cache_key) REFERENCES cache_entries(cache_key) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS input_file_hashes (
                path         TEXT PRIMARY KEY,
                content_hash TEXT NOT NULL,
                mtime_nanos  INTEGER NOT NULL,
                size         INTEGER NOT NULL
            );",
        )
        .map_err(|e| CacheError::Manifest(format!("sqlite schema: {e}")))?;

        // Enable foreign key enforcement (required per-connection in SQLite).
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| CacheError::Manifest(format!("sqlite pragma: {e}")))?;

        let mut store = Self {
            conn,
            cache_dir,
            validation,
            input_memo: HashMap::new(),
        };
        store.migrate_json_if_needed()?;
        Ok(store)
    }

    /// Migrate a legacy `manifest.json` into SQLite if it exists.
    fn migrate_json_if_needed(&mut self) -> Result<(), CacheError> {
        let manifest_path = self.cache_dir.join("manifest.json");
        if !manifest_path.exists() {
            return Ok(());
        }

        let data = std::fs::read_to_string(&manifest_path)?;
        if data.trim().is_empty() {
            // Empty manifest — just remove it.
            let migrated = self.cache_dir.join("manifest.json.migrated");
            std::fs::rename(&manifest_path, &migrated)?;
            return Ok(());
        }

        let entries: BTreeMap<String, CacheEntry> =
            serde_json::from_str(&data).map_err(|e| CacheError::Manifest(e.to_string()))?;

        let tx = self
            .conn
            .transaction()
            .map_err(|e| CacheError::Manifest(format!("sqlite tx: {e}")))?;

        for entry in entries.values() {
            tx.execute(
                "INSERT OR REPLACE INTO cache_entries (cache_key, completed_at) VALUES (?1, ?2)",
                params![entry.cache_key.as_str(), entry.completed_at as i64],
            )
            .map_err(|e| CacheError::Manifest(format!("sqlite insert: {e}")))?;

            for (path, content_hash) in &entry.output_hashes {
                let (mt, sz) = entry.output_mtimes.get(path).copied().unwrap_or((0, 0));
                tx.execute(
                    "INSERT OR REPLACE INTO output_records
                     (cache_key, path, content_hash, mtime_secs, size)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        entry.cache_key.as_str(),
                        path,
                        content_hash.as_str(),
                        mt as i64,
                        sz as i64,
                    ],
                )
                .map_err(|e| CacheError::Manifest(format!("sqlite insert: {e}")))?;
            }
        }

        tx.commit()
            .map_err(|e| CacheError::Manifest(format!("sqlite commit: {e}")))?;

        // Rename instead of delete so the user can recover if needed.
        let migrated = self.cache_dir.join("manifest.json.migrated");
        std::fs::rename(&manifest_path, &migrated)?;
        Ok(())
    }

    /// Check if a job's outputs are up-to-date with respect to the cache.
    ///
    /// Returns `true` if:
    /// 1. A cache entry exists for this cache key.
    /// 2. All output files in `outputs` still exist on disk.
    /// 3. All output files match their stored metadata or content hashes.
    ///
    /// Uses an mtime+size fast path: when a file's modification time and size
    /// match the values recorded at cache time, the stored content hash is
    /// trusted without re-reading the file. Only when metadata differs does
    /// the method fall back to a full BLAKE3 content hash verification.
    #[tracing::instrument(
        name = "cache.lookup",
        skip_all,
        fields(cache_key = %cache_key.as_str(), output_count = outputs.len()),
    )]
    pub fn is_cached(
        &self,
        cache_key: &ContentHash,
        outputs: &[&Path],
    ) -> Result<bool, CacheError> {
        // Check the entry exists.
        let exists: bool = self
            .conn
            .query_row(
                "SELECT 1 FROM cache_entries WHERE cache_key = ?1",
                params![cache_key.as_str()],
                |_| Ok(true),
            )
            .unwrap_or(false);

        if !exists {
            tracing::debug!(
                target: "ox.cache",
                counter = "cache.lookup.miss",
                "miss: no entry"
            );
            return Ok(false);
        }

        for path in outputs {
            let key = path.to_string_lossy().to_string();

            let row: Option<(String, i64, i64)> = self
                .conn
                .query_row(
                    "SELECT content_hash, mtime_secs, size FROM output_records
                     WHERE cache_key = ?1 AND path = ?2",
                    params![cache_key.as_str(), &key],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .ok();

            let (expected_hash, cached_mtime, cached_size) = match row {
                Some((h, mt, sz)) => (h, mt as u64, sz as u64),
                None => return Ok(false),
            };

            if !path.exists() {
                return Ok(false);
            }

            match self.validation {
                CacheValidation::Mtime => {
                    // Only check mtime + size — skip hashing entirely.
                    if !hash::mtime_matches(path, cached_mtime, cached_size) {
                        return Ok(false);
                    }
                }
                CacheValidation::MtimeHash => {
                    // Fast path: if mtime and size match, trust stored hash.
                    if hash::mtime_matches(path, cached_mtime, cached_size) {
                        continue;
                    }
                    // Slow path: metadata changed — verify content hash.
                    let actual_hash = hash::hash_file(path)?;
                    if actual_hash.as_str() != expected_hash {
                        return Ok(false);
                    }
                }
                CacheValidation::ContentHash => {
                    // Always hash, ignore mtime.
                    let actual_hash = hash::hash_file(path)?;
                    if actual_hash.as_str() != expected_hash {
                        return Ok(false);
                    }
                }
            }
        }

        tracing::debug!(
            target: "ox.cache",
            counter = "cache.lookup.hit",
            "hit"
        );
        Ok(true)
    }

    /// Check cache status with self-healing: if an output hash mismatch is
    /// detected, the stale cache entry is removed so the job will re-execute.
    ///
    /// Returns a [`CacheHitStatus`] that distinguishes between a clean miss,
    /// a missing output file, and a content hash mismatch.
    ///
    /// Uses an mtime+size fast path: when a file's modification time and size
    /// match the values recorded at cache time, the stored content hash is
    /// trusted without re-reading the file. Only when metadata differs does
    /// the method fall back to a full BLAKE3 content hash verification (and
    /// self-heal on mismatch).
    pub fn check_cached(
        &mut self,
        cache_key: &ContentHash,
        outputs: &[&Path],
    ) -> Result<CacheHitStatus, CacheError> {
        let exists: bool = self
            .conn
            .query_row(
                "SELECT 1 FROM cache_entries WHERE cache_key = ?1",
                params![cache_key.as_str()],
                |_| Ok(true),
            )
            .unwrap_or(false);

        if !exists {
            return Ok(CacheHitStatus::Miss);
        }

        for path in outputs {
            let key = path.to_string_lossy().to_string();

            let row: Option<(String, i64, i64)> = self
                .conn
                .query_row(
                    "SELECT content_hash, mtime_secs, size FROM output_records
                     WHERE cache_key = ?1 AND path = ?2",
                    params![cache_key.as_str(), &key],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .ok();

            let (expected_hash, cached_mtime, cached_size) = match row {
                Some((h, mt, sz)) => (h, mt as u64, sz as u64),
                None => return Ok(CacheHitStatus::Miss),
            };

            if !path.exists() {
                self.remove_entry(cache_key);
                return Ok(CacheHitStatus::OutputMissing { path: key });
            }

            match self.validation {
                CacheValidation::Mtime => {
                    // Only check mtime + size — skip hashing entirely.
                    if !hash::mtime_matches(path, cached_mtime, cached_size) {
                        self.remove_entry(cache_key);
                        return Ok(CacheHitStatus::Mismatch { path: key });
                    }
                }
                CacheValidation::MtimeHash => {
                    // Fast path: if mtime and size match, trust stored hash.
                    if hash::mtime_matches(path, cached_mtime, cached_size) {
                        continue;
                    }
                    // Slow path: metadata changed — verify content hash.
                    let actual_hash = hash::hash_file(path)?;
                    if actual_hash.as_str() != expected_hash {
                        self.remove_entry(cache_key);
                        return Ok(CacheHitStatus::Mismatch { path: key });
                    }
                }
                CacheValidation::ContentHash => {
                    // Always hash, ignore mtime.
                    let actual_hash = hash::hash_file(path)?;
                    if actual_hash.as_str() != expected_hash {
                        self.remove_entry(cache_key);
                        return Ok(CacheHitStatus::Mismatch { path: key });
                    }
                }
            }
        }

        Ok(CacheHitStatus::Hit)
    }

    /// Stateless mtime check — the Make/Snakemake approach.
    ///
    /// Returns [`CacheHitStatus::Hit`] when ALL outputs exist on disk and
    /// every output's mtime is strictly newer than the newest input mtime.
    /// No database lookup is performed — this is pure filesystem metadata.
    ///
    /// This is the correct behavior for [`CacheValidation::Mtime`] mode:
    /// zero dependency on cache.db or any other persistent state.
    ///
    /// # Security
    ///
    /// This check never reads file content. An output that was corrupted or
    /// replaced — even with same-size content — is declared a hit as long as
    /// its mtime is newer than the inputs'. That is why `Mtime` is **not**
    /// the default validation strategy and must never be used on a shared
    /// or multi-user cache; use `MtimeHash` (default) or `ContentHash`
    /// there. See SECURITY.md ("Cache integrity").
    pub fn check_mtime_stateless(
        inputs: &[&Path],
        outputs: &[&Path],
    ) -> Result<CacheHitStatus, CacheError> {
        // Find the newest input mtime.
        let mut max_input_mtime: u64 = 0;
        for path in inputs {
            if !path.exists() {
                // Input doesn't exist — can't determine freshness.
                return Ok(CacheHitStatus::Miss);
            }
            let (mt, _sz) = hash::file_meta(path)?;
            if mt > max_input_mtime {
                max_input_mtime = mt;
            }
        }

        // Check each output: must exist and be newer than all inputs.
        for path in outputs {
            let key = path.to_string_lossy().to_string();
            if !path.exists() {
                return Ok(CacheHitStatus::OutputMissing { path: key });
            }
            let (output_mt, _sz) = hash::file_meta(path)?;
            if output_mt <= max_input_mtime {
                return Ok(CacheHitStatus::Mismatch { path: key });
            }
        }

        Ok(CacheHitStatus::Hit)
    }

    /// Record a completed job's outputs in the cache.
    ///
    /// Hashes each output file and stores its content hash and mtime/size
    /// metadata under the given cache key.
    #[tracing::instrument(
        name = "cache.record",
        skip_all,
        fields(cache_key = %cache_key.as_str(), output_count = outputs.len()),
    )]
    pub fn record(
        &mut self,
        cache_key: ContentHash,
        outputs: &[&Path],
        provenance: Option<&ox_core::model::ArtifactProvenance>,
    ) -> Result<(), CacheError> {
        tracing::debug!(
            target: "ox.cache",
            counter = "cache.record",
            "record"
        );
        let completed_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let tx = self
            .conn
            .transaction()
            .map_err(|e| CacheError::Manifest(format!("sqlite tx: {e}")))?;

        // Upsert the entry (replace if key already exists), including provenance columns.
        let (repro_class, input_hashes_json, job_spec_hash) = match provenance {
            Some(prov) => (
                Some(prov.reproducibility.to_string()),
                Some(serde_json::to_string(&prov.input_hashes).unwrap_or_default()),
                Some(prov.job_spec_hash.clone()),
            ),
            None => (None, None, None),
        };

        tx.execute(
            "INSERT OR REPLACE INTO cache_entries
                (cache_key, completed_at, reproducibility_class, input_hashes_json, job_spec_hash)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                cache_key.as_str(),
                completed_at as i64,
                repro_class,
                input_hashes_json,
                job_spec_hash,
            ],
        )
        .map_err(|e| CacheError::Manifest(format!("sqlite insert: {e}")))?;

        // Remove old output records for this key (in case outputs changed).
        tx.execute(
            "DELETE FROM output_records WHERE cache_key = ?1",
            params![cache_key.as_str()],
        )
        .map_err(|e| CacheError::Manifest(format!("sqlite delete: {e}")))?;

        for path in outputs {
            let key = path.to_string_lossy().to_string();
            let h = hash::hash_file(path)?;
            let (mt, sz) = hash::file_meta(path)?;
            tx.execute(
                "INSERT INTO output_records (cache_key, path, content_hash, mtime_secs, size)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![cache_key.as_str(), &key, h.as_str(), mt as i64, sz as i64],
            )
            .map_err(|e| CacheError::Manifest(format!("sqlite insert: {e}")))?;
        }

        tx.commit()
            .map_err(|e| CacheError::Manifest(format!("sqlite commit: {e}")))?;
        Ok(())
    }

    /// Returns the validation strategy used by this cache store.
    pub fn validation(&self) -> CacheValidation {
        self.validation
    }

    /// Hash an input file with mtime-based fast path.
    ///
    /// Returns the BLAKE3 content hash of `path`, but avoids re-reading the
    /// file when the stored mtime+size match the current metadata. This turns
    /// repeated cache-key computations for large, unchanged input files into
    /// a single `stat()` call instead of a full file read.
    ///
    /// Results are memoized in memory for the duration of this session and
    /// persisted in SQLite for fast lookup across runs.
    pub fn hash_input_cached(&mut self, path: &Path) -> Result<ContentHash, CacheError> {
        // 1. Session memo — no I/O at all.
        if let Some(h) = self.input_memo.get(path) {
            return Ok(h.clone());
        }

        // 2. Current file metadata (single stat call).
        let (current_mtime, current_size) = hash::file_meta(path)?;

        // 3. Check SQLite for a stored hash with matching mtime+size.
        let key = path.to_string_lossy().to_string();
        let stored: Option<(String, i64, i64)> = self
            .conn
            .query_row(
                "SELECT content_hash, mtime_nanos, size FROM input_file_hashes WHERE path = ?1",
                params![&key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .ok();

        if let Some((hash_str, stored_mtime, stored_size)) = stored {
            if stored_mtime as u64 == current_mtime && stored_size as u64 == current_size {
                // A corrupted/forged stored hash falls through to re-hashing
                // instead of being trusted as-is.
                if let Ok(h) = ContentHash::from_hex(hash_str as String) {
                    self.input_memo.insert(path.to_path_buf(), h.clone());
                    return Ok(h);
                }
            }
        }

        // 4. Slow path: full BLAKE3 hash.
        let h = hash::hash_file(path)?;

        // 5. Persist to SQLite (best-effort) and session memo.
        let _ = self.conn.execute(
            "INSERT OR REPLACE INTO input_file_hashes (path, content_hash, mtime_nanos, size)
             VALUES (?1, ?2, ?3, ?4)",
            params![&key, h.as_str(), current_mtime as i64, current_size as i64],
        );
        self.input_memo.insert(path.to_path_buf(), h.clone());
        Ok(h)
    }

    /// No-op for backward compatibility.
    ///
    /// With the SQLite backend, all mutations are committed immediately.
    /// Callers that previously called `save()` after mutations can continue
    /// to do so without changes.
    pub fn save(&self) -> Result<(), CacheError> {
        Ok(())
    }

    /// Invalidate all cache entries that reference any of the given output
    /// paths. Returns the number of entries removed.
    pub fn invalidate(&mut self, outputs: &[&Path]) -> usize {
        let keys_to_check: Vec<String> = outputs
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();

        let mut removed = 0usize;
        for path in &keys_to_check {
            // Find all cache keys that have this output path.
            let cache_keys: Vec<String> = self
                .conn
                .prepare("SELECT DISTINCT cache_key FROM output_records WHERE path = ?1")
                .and_then(|mut stmt| {
                    stmt.query_map(params![path], |row| row.get(0))?
                        .collect::<Result<Vec<String>, _>>()
                })
                .unwrap_or_default();

            for ck in &cache_keys {
                let deleted = self
                    .conn
                    .execute(
                        "DELETE FROM cache_entries WHERE cache_key = ?1",
                        params![ck],
                    )
                    .unwrap_or(0);
                if deleted > 0 {
                    // Cascade doesn't fire on some builds; clean up explicitly.
                    let _ = self.conn.execute(
                        "DELETE FROM output_records WHERE cache_key = ?1",
                        params![ck],
                    );
                    removed += deleted;
                }
            }
        }
        removed
    }

    /// Return all distinct output paths stored in the cache.
    pub fn all_output_paths(&self) -> Vec<PathBuf> {
        self.conn
            .prepare("SELECT DISTINCT path FROM output_records")
            .and_then(|mut stmt| {
                stmt.query_map([], |row| row.get::<_, String>(0))?
                    .collect::<Result<Vec<String>, _>>()
            })
            .unwrap_or_default()
            .into_iter()
            .map(PathBuf::from)
            .collect()
    }

    /// Remove all entries from the cache.
    pub fn clear(&mut self) {
        let _ = self
            .conn
            .execute_batch("DELETE FROM output_records; DELETE FROM cache_entries;");
    }

    /// Number of cached entries.
    pub fn len(&self) -> usize {
        self.conn
            .query_row("SELECT COUNT(*) FROM cache_entries", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap_or(0) as usize
    }

    /// Whether the cache store is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get a cache entry by key.
    pub fn get(&self, cache_key: &ContentHash) -> Option<CacheEntry> {
        let row_data: (i64, Option<String>, Option<String>, Option<String>) = self
            .conn
            .query_row(
                "SELECT completed_at, reproducibility_class, input_hashes_json, job_spec_hash
                 FROM cache_entries WHERE cache_key = ?1",
                params![cache_key.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .ok()?;

        let (completed_at, repro_class, input_hashes_json, job_spec_hash) = row_data;

        let provenance = match (repro_class, job_spec_hash) {
            (Some(rc), Some(jsh)) => {
                let reproducibility = match rc.as_str() {
                    "deterministic" => ox_core::model::ReproducibilityClass::Deterministic,
                    "seed_deterministic" => ox_core::model::ReproducibilityClass::SeedDeterministic,
                    "approximate" => ox_core::model::ReproducibilityClass::Approximate,
                    "non_reproducible" => ox_core::model::ReproducibilityClass::NonReproducible,
                    // Unknown variants default to NonReproducible (safest:
                    // prevents stale cache reuse if a new variant is added).
                    _ => ox_core::model::ReproducibilityClass::NonReproducible,
                };
                let input_hashes: Vec<(String, String)> = input_hashes_json
                    .and_then(|j| serde_json::from_str(&j).ok())
                    .unwrap_or_default();
                Some(ox_core::model::ArtifactProvenance {
                    input_hashes,
                    job_spec_hash: jsh,
                    reproducibility,
                })
            }
            _ => None,
        };

        let records: Vec<(String, String, i64, i64)> = self
            .conn
            .prepare(
                "SELECT path, content_hash, mtime_secs, size FROM output_records
                 WHERE cache_key = ?1",
            )
            .and_then(|mut stmt| {
                stmt.query_map(params![cache_key.as_str()], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?
                .collect::<Result<Vec<_>, _>>()
            })
            .unwrap_or_default();

        let mut output_hashes = BTreeMap::new();
        let mut output_mtimes = BTreeMap::new();
        for (path, hash, mt, sz) in records {
            // A corrupted stored hash invalidates the whole entry (treated
            // as a cache miss) rather than being compared as a plain string.
            let hash = ContentHash::from_hex(hash as String).ok()?;
            output_hashes.insert(path.clone(), hash);
            output_mtimes.insert(path, (mt as u64, sz as u64));
        }

        Some(CacheEntry {
            cache_key: cache_key.clone(),
            output_hashes,
            output_mtimes,
            completed_at: completed_at as u64,
            provenance,
        })
    }

    /// Count cache entries whose output files no longer exist on disk.
    pub fn count_orphans(&self) -> usize {
        let all_keys: Vec<String> = self
            .conn
            .prepare("SELECT cache_key FROM cache_entries")
            .and_then(|mut stmt| {
                stmt.query_map([], |row| row.get(0))?
                    .collect::<Result<Vec<String>, _>>()
            })
            .unwrap_or_default();

        let mut orphan_count = 0;
        for ck in &all_keys {
            let paths: Vec<String> = self
                .conn
                .prepare("SELECT path FROM output_records WHERE cache_key = ?1")
                .and_then(|mut stmt| {
                    stmt.query_map(params![ck], |row| row.get(0))?
                        .collect::<Result<Vec<String>, _>>()
                })
                .unwrap_or_default();

            if paths.iter().any(|p| !Path::new(p).exists()) {
                orphan_count += 1;
            }
        }
        orphan_count
    }

    /// Remove cache entries whose output files no longer exist on disk.
    /// Returns the number of entries removed.
    pub fn remove_orphans(&mut self) -> usize {
        let all_keys: Vec<String> = self
            .conn
            .prepare("SELECT cache_key FROM cache_entries")
            .and_then(|mut stmt| {
                stmt.query_map([], |row| row.get(0))?
                    .collect::<Result<Vec<String>, _>>()
            })
            .unwrap_or_default();

        let mut removed = 0;
        for ck in &all_keys {
            let paths: Vec<String> = self
                .conn
                .prepare("SELECT path FROM output_records WHERE cache_key = ?1")
                .and_then(|mut stmt| {
                    stmt.query_map(params![ck], |row| row.get(0))?
                        .collect::<Result<Vec<String>, _>>()
                })
                .unwrap_or_default();

            if paths.iter().any(|p| !Path::new(p).exists()) {
                let _ = self.conn.execute(
                    "DELETE FROM cache_entries WHERE cache_key = ?1",
                    params![ck],
                );
                let _ = self.conn.execute(
                    "DELETE FROM output_records WHERE cache_key = ?1",
                    params![ck],
                );
                removed += 1;
            }
        }
        removed
    }

    /// Remove a single cache entry and its output records.
    fn remove_entry(&mut self, cache_key: &ContentHash) {
        let _ = self.conn.execute(
            "DELETE FROM cache_entries WHERE cache_key = ?1",
            params![cache_key.as_str()],
        );
        let _ = self.conn.execute(
            "DELETE FROM output_records WHERE cache_key = ?1",
            params![cache_key.as_str()],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic valid ContentHash from a test label (64 hex chars).
    fn ch(label: &str) -> ContentHash {
        let mut hex: String = label.bytes().map(|b| format!("{b:02x}")).collect();
        hex.truncate(64);
        ContentHash::from_hex(format!("{hex:0<64}")).unwrap()
    }

    fn make_store(dir: &Path) -> CacheStore {
        CacheStore::open(dir).unwrap()
    }

    #[test]
    fn open_creates_directory() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let _store = CacheStore::open(&oxdir).unwrap();
        assert!(oxdir.join("cache").exists());
    }

    #[test]
    fn record_and_is_cached() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store(&oxdir);

        // Create an output file.
        let out = dir.path().join("out.txt");
        std::fs::write(&out, b"result data").unwrap();

        let key = ch("test_key_abc");
        store.record(key.clone(), &[out.as_path()], None).unwrap();

        assert!(store.is_cached(&key, &[out.as_path()]).unwrap());
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn is_cached_false_when_file_changed() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store(&oxdir);

        let out = dir.path().join("out.txt");
        std::fs::write(&out, b"original").unwrap();

        let key = ch("key1");
        store.record(key.clone(), &[out.as_path()], None).unwrap();

        // Modify the file (different content AND likely different mtime).
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&out, b"modified content").unwrap();

        assert!(!store.is_cached(&key, &[out.as_path()]).unwrap());
    }

    #[test]
    fn is_cached_false_when_file_corrupted_same_size() {
        // Regression test: corruption that preserves file size (and may happen
        // within the same filesystem-mtime second) must still be detected.
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store(&oxdir);

        let out = dir.path().join("out.txt");
        std::fs::write(&out, b"AAAA").unwrap();

        let key = ch("same_size_key");
        store.record(key.clone(), &[out.as_path()], None).unwrap();

        // Corrupt with same-size content, no sleep — same mtime second.
        std::fs::write(&out, b"BBBB").unwrap();

        assert!(
            !store.is_cached(&key, &[out.as_path()]).unwrap(),
            "same-size corruption must be detected even without mtime change",
        );
    }

    #[test]
    fn check_cached_detects_same_size_corruption() {
        // Regression test: check_cached must return Mismatch (not Hit) when
        // output content changes but file size stays the same.
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store(&oxdir);

        let out = dir.path().join("out.txt");
        std::fs::write(&out, b"AAAA").unwrap();

        let key = ch("same_size_check_key");
        store.record(key.clone(), &[out.as_path()], None).unwrap();

        // Corrupt with same-size content, no sleep.
        std::fs::write(&out, b"BBBB").unwrap();

        let status = store.check_cached(&key, &[out.as_path()]).unwrap();
        assert!(
            matches!(status, CacheHitStatus::Mismatch { .. }),
            "expected Mismatch for same-size corruption, got {status:?}",
        );
        assert_eq!(store.len(), 0, "stale entry should be self-healed away");
    }

    #[test]
    fn is_cached_false_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store(&oxdir);

        let out = dir.path().join("out.txt");
        std::fs::write(&out, b"data").unwrap();

        let key = ch("key2");
        store.record(key.clone(), &[out.as_path()], None).unwrap();

        std::fs::remove_file(&out).unwrap();
        assert!(!store.is_cached(&key, &[out.as_path()]).unwrap());
    }

    #[test]
    fn is_cached_false_when_no_entry() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let store = make_store(&oxdir);

        let out = dir.path().join("out.txt");
        std::fs::write(&out, b"data").unwrap();

        let key = ch("nonexistent");
        assert!(!store.is_cached(&key, &[out.as_path()]).unwrap());
    }

    #[test]
    fn save_is_noop_with_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");

        let out = dir.path().join("out.txt");
        std::fs::write(&out, b"sqlite test").unwrap();

        let mut store = make_store(&oxdir);
        store
            .record(ch("sqlite_key"), &[out.as_path()], None)
            .unwrap();
        store.save().unwrap();

        // The database file must exist.
        let db_path = oxdir.join("cache").join("cache.db");
        assert!(db_path.exists());
    }

    #[test]
    fn save_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");

        let out = dir.path().join("out.txt");
        std::fs::write(&out, b"persist me").unwrap();

        let key = ch("persist_key");

        {
            let mut store = make_store(&oxdir);
            store.record(key.clone(), &[out.as_path()], None).unwrap();
            store.save().unwrap();
        }

        // Reload from disk.
        let store2 = make_store(&oxdir);
        assert_eq!(store2.len(), 1);
        assert!(store2.is_cached(&key, &[out.as_path()]).unwrap());
    }

    #[test]
    fn invalidate_removes_matching_entries() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store(&oxdir);

        let out1 = dir.path().join("a.txt");
        let out2 = dir.path().join("b.txt");
        std::fs::write(&out1, b"aaa").unwrap();
        std::fs::write(&out2, b"bbb").unwrap();

        store.record(ch("k1"), &[out1.as_path()], None).unwrap();
        store.record(ch("k2"), &[out2.as_path()], None).unwrap();
        assert_eq!(store.len(), 2);

        let removed = store.invalidate(&[out1.as_path()]);
        assert_eq!(removed, 1);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn all_output_paths_returns_distinct_paths() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store(&oxdir);

        let out1 = dir.path().join("a.txt");
        let out2 = dir.path().join("b.txt");
        std::fs::write(&out1, b"aaa").unwrap();
        std::fs::write(&out2, b"bbb").unwrap();

        store.record(ch("k1"), &[out1.as_path()], None).unwrap();
        store.record(ch("k2"), &[out2.as_path()], None).unwrap();

        let paths = store.all_output_paths();
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&out1));
        assert!(paths.contains(&out2));
    }

    // -----------------------------------------------------------------------
    // check_cached — self-healing cache tests
    // -----------------------------------------------------------------------

    #[test]
    fn check_cached_hit() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store(&oxdir);

        let out = dir.path().join("out.txt");
        std::fs::write(&out, b"good data").unwrap();

        let key = ch("hit_key");
        store.record(key.clone(), &[out.as_path()], None).unwrap();

        let status = store.check_cached(&key, &[out.as_path()]).unwrap();
        assert_eq!(status, CacheHitStatus::Hit);
        assert!(status.is_hit());
        assert_eq!(store.len(), 1); // Entry preserved
    }

    #[test]
    fn check_cached_hit_uses_mtime_fast_path() {
        // Verify that an unmodified file is detected as a hit via the
        // mtime+size fast path (no re-hashing needed). This is the primary
        // performance optimization for fully-cached DAGs.
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store(&oxdir);

        let out = dir.path().join("large_output.bin");
        // Write a file, record it, then check — the file hasn't changed
        // so the mtime fast path should produce a Hit.
        std::fs::write(&out, b"cached output data").unwrap();

        let key = ch("mtime_fast_key");
        store.record(key.clone(), &[out.as_path()], None).unwrap();

        // Repeated checks should all be hits via the fast path.
        for _ in 0..3 {
            let status = store.check_cached(&key, &[out.as_path()]).unwrap();
            assert_eq!(status, CacheHitStatus::Hit);
        }
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn is_cached_uses_mtime_fast_path() {
        // Same as above but for the non-mut is_cached method.
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store(&oxdir);

        let out = dir.path().join("output.bin");
        std::fs::write(&out, b"cached data").unwrap();

        let key = ch("mtime_is_cached_key");
        store.record(key.clone(), &[out.as_path()], None).unwrap();

        for _ in 0..3 {
            assert!(store.is_cached(&key, &[out.as_path()]).unwrap());
        }
    }

    #[test]
    fn check_cached_miss_no_entry() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store(&oxdir);

        let out = dir.path().join("out.txt");
        std::fs::write(&out, b"data").unwrap();

        let key = ch("missing_key");
        let status = store.check_cached(&key, &[out.as_path()]).unwrap();
        assert_eq!(status, CacheHitStatus::Miss);
        assert!(!status.is_hit());
    }

    #[test]
    fn check_cached_mismatch_invalidates_entry() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store(&oxdir);

        let out = dir.path().join("out.txt");
        std::fs::write(&out, b"original content").unwrap();

        let key = ch("mismatch_key");
        store.record(key.clone(), &[out.as_path()], None).unwrap();
        assert_eq!(store.len(), 1);

        // Modify the file — triggers hash mismatch on slow path.
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&out, b"corrupted content").unwrap();

        let status = store.check_cached(&key, &[out.as_path()]).unwrap();
        assert!(
            matches!(status, CacheHitStatus::Mismatch { .. }),
            "expected Mismatch, got {status:?}",
        );
        assert!(!status.is_hit());
        // Self-healing: stale entry should be removed.
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn check_cached_output_missing_invalidates_entry() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store(&oxdir);

        let out = dir.path().join("out.txt");
        std::fs::write(&out, b"data").unwrap();

        let key = ch("missing_output_key");
        store.record(key.clone(), &[out.as_path()], None).unwrap();
        assert_eq!(store.len(), 1);

        std::fs::remove_file(&out).unwrap();

        let status = store.check_cached(&key, &[out.as_path()]).unwrap();
        assert!(
            matches!(status, CacheHitStatus::OutputMissing { .. }),
            "expected OutputMissing, got {status:?}",
        );
        // Self-healing: stale entry should be removed.
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn check_cached_self_heals_then_re_records() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store(&oxdir);

        let out = dir.path().join("out.txt");
        std::fs::write(&out, b"original").unwrap();

        let key = ch("heal_key");
        store.record(key.clone(), &[out.as_path()], None).unwrap();

        // Corrupt the output.
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&out, b"corrupted").unwrap();

        // Mismatch detected, entry removed.
        let status = store.check_cached(&key, &[out.as_path()]).unwrap();
        assert!(matches!(status, CacheHitStatus::Mismatch { .. }));
        assert_eq!(store.len(), 0);

        // Simulate re-execution: write correct output and re-record.
        std::fs::write(&out, b"fixed output").unwrap();
        store.record(key.clone(), &[out.as_path()], None).unwrap();

        // Now cache should report a hit.
        let status = store.check_cached(&key, &[out.as_path()]).unwrap();
        assert_eq!(status, CacheHitStatus::Hit);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn clear_removes_all() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store(&oxdir);

        let out = dir.path().join("c.txt");
        std::fs::write(&out, b"ccc").unwrap();
        store.record(ch("k3"), &[out.as_path()], None).unwrap();
        assert_eq!(store.len(), 1);

        store.clear();
        assert_eq!(store.len(), 0);
        assert!(store.is_empty());
    }

    #[test]
    fn get_returns_entry() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store(&oxdir);

        let out = dir.path().join("out.txt");
        std::fs::write(&out, b"get test").unwrap();

        let key = ch("get_key");
        store.record(key.clone(), &[out.as_path()], None).unwrap();

        let entry = store.get(&key).expect("entry should exist");
        assert_eq!(entry.cache_key, key);
        assert_eq!(entry.output_hashes.len(), 1);
    }

    #[test]
    fn migrate_json_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let cache_dir = oxdir.join("cache");
        std::fs::create_dir_all(&cache_dir).unwrap();

        // Create a legacy manifest.
        let out = dir.path().join("legacy.txt");
        std::fs::write(&out, b"legacy data").unwrap();

        let mut entries = BTreeMap::new();
        let h = hash::hash_file(&out).unwrap();
        let (mt, sz) = hash::file_meta(&out).unwrap();
        let mut output_hashes = BTreeMap::new();
        let mut output_mtimes = BTreeMap::new();
        let key_str = out.to_string_lossy().to_string();
        output_hashes.insert(key_str.clone(), h);
        output_mtimes.insert(key_str, (mt, sz));
        entries.insert(
            "legacy_key".to_string(),
            CacheEntry {
                cache_key: ch("legacy_key"),
                output_hashes,
                output_mtimes,
                completed_at: 12345,
                provenance: None,
            },
        );

        let manifest_path = cache_dir.join("manifest.json");
        let json = serde_json::to_string_pretty(&entries).unwrap();
        std::fs::write(&manifest_path, &json).unwrap();

        // Open should migrate the JSON to SQLite.
        let store = CacheStore::open(&oxdir).unwrap();
        assert_eq!(store.len(), 1);

        let key = ch("legacy_key");
        assert!(store.is_cached(&key, &[out.as_path()]).unwrap());

        // manifest.json should be renamed.
        assert!(!manifest_path.exists());
        assert!(cache_dir.join("manifest.json.migrated").exists());
    }

    // -----------------------------------------------------------------------
    // CacheValidation strategy tests
    // -----------------------------------------------------------------------

    fn make_store_with(dir: &Path, validation: CacheValidation) -> CacheStore {
        CacheStore::open_with(dir, validation).unwrap()
    }

    #[test]
    fn mtime_strategy_hit_when_mtime_matches() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store_with(&oxdir, CacheValidation::Mtime);

        let out = dir.path().join("out.txt");
        std::fs::write(&out, b"data").unwrap();

        let key = ch("mtime_key");
        store.record(key.clone(), &[out.as_path()], None).unwrap();

        // File unchanged — mtime matches → hit.
        assert!(store.is_cached(&key, &[out.as_path()]).unwrap());
        assert_eq!(
            store.check_cached(&key, &[out.as_path()]).unwrap(),
            CacheHitStatus::Hit
        );
    }

    #[test]
    fn mtime_strategy_miss_when_mtime_differs() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store_with(&oxdir, CacheValidation::Mtime);

        let out = dir.path().join("out.txt");
        std::fs::write(&out, b"original").unwrap();

        let key = ch("mtime_key2");
        store.record(key.clone(), &[out.as_path()], None).unwrap();

        // Modify the file so mtime changes.
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&out, b"modified content").unwrap();

        // Mtime differs → miss (even though we don't hash).
        assert!(!store.is_cached(&key, &[out.as_path()]).unwrap());
    }

    #[test]
    fn mtime_strategy_same_size_corruption_caught_only_via_mtime() {
        // DOCUMENTED INSECURE OPT-IN: `Mtime` never reads content. This
        // corruption is caught here *only because the overwrite changed the
        // mtime* — a corruption that preserves mtime + size (or, in the
        // stateless run-loop path, any corruption with a later mtime; see
        // `stateless_mtime_serves_corrupted_output_as_hit`) goes undetected.
        // That hole is why `Mtime` is no longer the default (ADR-006
        // amendment); the default is `MtimeHash`.
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store_with(&oxdir, CacheValidation::Mtime);

        let out = dir.path().join("out.txt");
        std::fs::write(&out, b"AAAA").unwrap();

        let key = ch("mtime_corrupt");
        store.record(key.clone(), &[out.as_path()], None).unwrap();

        // Overwrite with same-size content (mtime will differ because of sleep).
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&out, b"BBBB").unwrap();

        // Mtime changed → miss (mtime strategy catches mtime changes).
        assert!(!store.is_cached(&key, &[out.as_path()]).unwrap());
    }

    #[test]
    fn stateless_mtime_serves_corrupted_output_as_hit() {
        // DOCUMENTED INSECURE OPT-IN: the stateless mtime check (used by the
        // run loop in `Mtime` mode) declares a hit on existence + newer
        // mtime alone. A corrupted/replaced output with a later mtime is
        // served as a hit — this is the poisoning vector that motivated
        // flipping the default to `MtimeHash`. If this test ever fails,
        // the stateless check gained content verification and SECURITY.md
        // must be updated accordingly.
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input.txt");
        let output = dir.path().join("output.txt");
        std::fs::write(&input, b"source").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&output, b"legitimate build output").unwrap();

        // Sanity: fresh output → hit.
        assert_eq!(
            CacheStore::check_mtime_stateless(&[input.as_path()], &[output.as_path()]).unwrap(),
            CacheHitStatus::Hit
        );

        // Corrupt the output: same size, different content, later mtime.
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&output, b"poisoned build output!!").unwrap();

        // Still a hit — content is never inspected.
        assert_eq!(
            CacheStore::check_mtime_stateless(&[input.as_path()], &[output.as_path()]).unwrap(),
            CacheHitStatus::Hit
        );
    }

    #[test]
    fn content_hash_strategy_always_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store_with(&oxdir, CacheValidation::ContentHash);

        let out = dir.path().join("out.txt");
        std::fs::write(&out, b"data").unwrap();

        let key = ch("hash_key");
        store.record(key.clone(), &[out.as_path()], None).unwrap();

        // File unchanged — content hash matches → hit.
        assert!(store.is_cached(&key, &[out.as_path()]).unwrap());
        assert_eq!(
            store.check_cached(&key, &[out.as_path()]).unwrap(),
            CacheHitStatus::Hit
        );
    }

    #[test]
    fn content_hash_strategy_detects_same_size_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store_with(&oxdir, CacheValidation::ContentHash);

        let out = dir.path().join("out.txt");
        std::fs::write(&out, b"AAAA").unwrap();

        let key = ch("hash_corrupt");
        store.record(key.clone(), &[out.as_path()], None).unwrap();

        // Overwrite with same-size, different content.
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&out, b"BBBB").unwrap();

        // ContentHash always hashes — catches same-size corruption.
        assert!(!store.is_cached(&key, &[out.as_path()]).unwrap());
        // Re-record to restore entry for check_cached test.
        std::fs::write(&out, b"AAAA").unwrap();
        store.record(key.clone(), &[out.as_path()], None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&out, b"BBBB").unwrap();
        assert_eq!(
            store.check_cached(&key, &[out.as_path()]).unwrap(),
            CacheHitStatus::Mismatch {
                path: out.to_string_lossy().to_string()
            }
        );
    }

    #[test]
    fn mtime_hash_strategy_fast_path_and_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store_with(&oxdir, CacheValidation::MtimeHash);

        let out = dir.path().join("out.txt");
        std::fs::write(&out, b"data").unwrap();

        let key = ch("mh_key");
        store.record(key.clone(), &[out.as_path()], None).unwrap();

        // Fast path: mtime matches → hit without hashing.
        assert!(store.is_cached(&key, &[out.as_path()]).unwrap());

        // Modify with different-size content.
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&out, b"different content").unwrap();

        // Slow path: mtime differs, hash differs → miss.
        assert!(!store.is_cached(&key, &[out.as_path()]).unwrap());
    }

    #[test]
    fn open_with_preserves_strategy() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");

        let store = make_store_with(&oxdir, CacheValidation::ContentHash);
        assert_eq!(store.validation(), CacheValidation::ContentHash);

        let store = make_store_with(&oxdir, CacheValidation::Mtime);
        assert_eq!(store.validation(), CacheValidation::Mtime);

        let store = make_store_with(&oxdir, CacheValidation::MtimeHash);
        assert_eq!(store.validation(), CacheValidation::MtimeHash);
    }

    // -- Path handling tests (ox-58w3) ----------------------------------------

    #[test]
    fn record_and_lookup_with_spaces_in_path() {
        let dir = tempfile::tempdir().unwrap();
        let spaced = dir.path().join("my project");
        std::fs::create_dir(&spaced).unwrap();
        let oxdir = spaced.join(".oxymake");
        let mut store = CacheStore::open(&oxdir).unwrap();

        let out = spaced.join("sub dir/result file.csv");
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        std::fs::write(&out, b"data with spaces").unwrap();

        let key = ch("spaces_key");
        store.record(key.clone(), &[out.as_path()], None).unwrap();
        assert!(store.is_cached(&key, &[out.as_path()]).unwrap());
    }

    #[test]
    fn record_and_lookup_with_unicode_path() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("données");
        std::fs::create_dir(&subdir).unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = CacheStore::open(&oxdir).unwrap();

        let out = subdir.join("résultats.csv");
        std::fs::write(&out, b"unicode data").unwrap();

        let key = ch("unicode_key");
        store.record(key.clone(), &[out.as_path()], None).unwrap();
        assert!(store.is_cached(&key, &[out.as_path()]).unwrap());
    }

    #[test]
    fn record_and_lookup_with_cjk_path() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("日本語");
        std::fs::create_dir(&subdir).unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = CacheStore::open(&oxdir).unwrap();

        let out = subdir.join("出力データ.csv");
        std::fs::write(&out, b"cjk data").unwrap();

        let key = ch("cjk_key");
        store.record(key.clone(), &[out.as_path()], None).unwrap();
        assert!(store.is_cached(&key, &[out.as_path()]).unwrap());
    }

    #[test]
    fn cache_through_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = CacheStore::open(&oxdir).unwrap();

        let real_file = dir.path().join("real_output.csv");
        std::fs::write(&real_file, b"real data").unwrap();

        #[cfg(unix)]
        {
            let link = dir.path().join("linked_output.csv");
            std::os::unix::fs::symlink(&real_file, &link).unwrap();

            // Cache the symlink path.
            let key = ch("symlink_key");
            store.record(key.clone(), &[link.as_path()], None).unwrap();
            assert!(store.is_cached(&key, &[link.as_path()]).unwrap());
        }
    }

    // -- Input file hash caching tests ----------------------------------------

    #[test]
    fn hash_input_cached_returns_correct_hash() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store(&oxdir);

        let input = dir.path().join("input.dat");
        std::fs::write(&input, b"hello world").unwrap();

        // First call: computes BLAKE3 and stores in SQLite.
        let h1 = store.hash_input_cached(&input).unwrap();
        // Second call: should return the same hash from session memo.
        let h2 = store.hash_input_cached(&input).unwrap();
        assert_eq!(h1, h2);

        // Verify it matches a direct BLAKE3 hash.
        let direct = hash::hash_file(&input).unwrap();
        assert_eq!(h1, direct);
    }

    #[test]
    fn hash_input_cached_uses_mtime_fast_path_across_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");

        let input = dir.path().join("big_input.dat");
        std::fs::write(&input, b"large file contents").unwrap();

        // First session: compute and store.
        let h1 = {
            let mut store = make_store(&oxdir);
            store.hash_input_cached(&input).unwrap()
        };

        // Second session (new CacheStore, empty session memo): should use
        // SQLite mtime fast-path without re-hashing.
        let h2 = {
            let mut store = make_store(&oxdir);
            store.hash_input_cached(&input).unwrap()
        };

        assert_eq!(h1, h2);
    }

    // -----------------------------------------------------------------------
    // Stateless mtime tests (Make/Snakemake style)
    // -----------------------------------------------------------------------

    #[test]
    fn stateless_mtime_hit_when_outputs_newer_than_inputs() {
        let dir = tempfile::tempdir().unwrap();

        // Create input files first.
        let inp1 = dir.path().join("input1.txt");
        let inp2 = dir.path().join("input2.txt");
        std::fs::write(&inp1, b"input data 1").unwrap();
        std::fs::write(&inp2, b"input data 2").unwrap();

        // Wait, then create output files (so outputs are newer).
        std::thread::sleep(std::time::Duration::from_millis(50));
        let out1 = dir.path().join("output1.txt");
        let out2 = dir.path().join("output2.txt");
        std::fs::write(&out1, b"output data 1").unwrap();
        std::fs::write(&out2, b"output data 2").unwrap();

        let status = CacheStore::check_mtime_stateless(
            &[inp1.as_path(), inp2.as_path()],
            &[out1.as_path(), out2.as_path()],
        )
        .unwrap();
        assert_eq!(status, CacheHitStatus::Hit);
    }

    #[test]
    fn stateless_mtime_miss_when_output_older_than_input() {
        let dir = tempfile::tempdir().unwrap();

        // Create output files first.
        let out = dir.path().join("output.txt");
        std::fs::write(&out, b"old output").unwrap();

        // Wait, then create input files (so inputs are newer).
        std::thread::sleep(std::time::Duration::from_millis(50));
        let inp = dir.path().join("input.txt");
        std::fs::write(&inp, b"new input").unwrap();

        let status = CacheStore::check_mtime_stateless(&[inp.as_path()], &[out.as_path()]).unwrap();
        assert!(
            matches!(status, CacheHitStatus::Mismatch { .. }),
            "expected Mismatch when output is older than input, got {status:?}",
        );
    }

    #[test]
    fn stateless_mtime_miss_when_output_missing() {
        let dir = tempfile::tempdir().unwrap();

        let inp = dir.path().join("input.txt");
        std::fs::write(&inp, b"input").unwrap();

        let out = dir.path().join("nonexistent_output.txt");

        let status = CacheStore::check_mtime_stateless(&[inp.as_path()], &[out.as_path()]).unwrap();
        assert!(
            matches!(status, CacheHitStatus::OutputMissing { .. }),
            "expected OutputMissing, got {status:?}",
        );
    }

    #[test]
    fn stateless_mtime_miss_when_input_missing() {
        let dir = tempfile::tempdir().unwrap();

        let inp = dir.path().join("nonexistent_input.txt");
        let out = dir.path().join("output.txt");
        std::fs::write(&out, b"output").unwrap();

        let status = CacheStore::check_mtime_stateless(&[inp.as_path()], &[out.as_path()]).unwrap();
        assert_eq!(status, CacheHitStatus::Miss);
    }

    #[test]
    fn stateless_mtime_hit_with_no_inputs() {
        // A job with no inputs (source rule) — outputs just need to exist.
        let dir = tempfile::tempdir().unwrap();

        let out = dir.path().join("output.txt");
        std::fs::write(&out, b"output").unwrap();

        let status = CacheStore::check_mtime_stateless(&[], &[out.as_path()]).unwrap();
        assert_eq!(status, CacheHitStatus::Hit);
    }

    #[test]
    fn hash_input_cached_rehashes_on_mtime_change() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");

        let input = dir.path().join("changing.dat");
        std::fs::write(&input, b"version 1").unwrap();

        let h1 = {
            let mut store = make_store(&oxdir);
            store.hash_input_cached(&input).unwrap()
        };

        // Modify file (sleep to ensure mtime changes on all filesystems).
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&input, b"version 2").unwrap();

        let h2 = {
            let mut store = make_store(&oxdir);
            store.hash_input_cached(&input).unwrap()
        };

        assert_ne!(h1, h2, "hash must change when file content changes");
    }

    #[test]
    fn record_and_get_with_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store(&oxdir);

        let out = dir.path().join("out.txt");
        std::fs::write(&out, b"provenance test").unwrap();

        let prov = ox_core::model::ArtifactProvenance {
            input_hashes: vec![
                ("hash_a".into(), "data/input.csv".into()),
                ("hash_b".into(), "data/ref.fa".into()),
            ],
            job_spec_hash: "spec_abc".into(),
            reproducibility: ox_core::model::ReproducibilityClass::SeedDeterministic,
        };

        let key = ch("prov_key");
        store
            .record(key.clone(), &[out.as_path()], Some(&prov))
            .unwrap();

        let entry = store.get(&key).expect("entry should exist");
        let got_prov = entry.provenance.expect("provenance should be stored");
        assert_eq!(got_prov.job_spec_hash, "spec_abc");
        assert_eq!(
            got_prov.reproducibility,
            ox_core::model::ReproducibilityClass::SeedDeterministic
        );
        assert_eq!(got_prov.input_hashes.len(), 2);
        assert_eq!(got_prov.input_hashes[0].0, "hash_a");
    }

    #[test]
    fn record_without_provenance_get_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let oxdir = dir.path().join(".oxymake");
        let mut store = make_store(&oxdir);

        let out = dir.path().join("out.txt");
        std::fs::write(&out, b"no provenance").unwrap();

        let key = ch("no_prov_key");
        store.record(key.clone(), &[out.as_path()], None).unwrap();

        let entry = store.get(&key).expect("entry should exist");
        assert!(
            entry.provenance.is_none(),
            "provenance should be None when not provided"
        );
    }

    // -----------------------------------------------------------------------
    // Property tests: shared-cache poisoning detection
    // -----------------------------------------------------------------------

    mod poisoning_proptests {
        use super::*;
        use proptest::prelude::*;

        /// Overwrite `path` with `content` and push its mtime strictly into
        /// the future, simulating an attacker (or corruption) on a shared
        /// cache: same size, different bytes, later timestamp — no sleeps.
        fn corrupt_with_later_mtime(path: &Path, content: &[u8]) {
            std::fs::write(path, content).unwrap();
            let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
            std::fs::OpenOptions::new()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(later)
                .unwrap();
        }

        /// Original content and a same-size corruption guaranteed to differ
        /// in at least one byte.
        fn content_and_corruption() -> impl Strategy<Value = (Vec<u8>, Vec<u8>)> {
            (
                proptest::collection::vec(any::<u8>(), 1..256),
                any::<u8>(),
                any::<usize>(),
            )
                .prop_map(|(original, xor, idx)| {
                    let mut corrupted = original.clone();
                    let i = idx % original.len();
                    // XOR with a non-zero byte guarantees a content change.
                    corrupted[i] ^= xor | 1;
                    (original, corrupted)
                })
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(100))]

            /// On a simulated shared cache, a same-size corruption with a
            /// later mtime MUST be detected (= cache miss) under `MtimeHash`.
            /// This is the property that the default validation strategy
            /// closes the stateless-mtime poisoning hole.
            #[test]
            fn mtime_hash_detects_same_size_later_mtime_corruption(
                (original, corrupted) in content_and_corruption(),
            ) {
                let dir = tempfile::tempdir().unwrap();
                let oxdir = dir.path().join(".oxymake");
                let mut store = make_store_with(&oxdir, CacheValidation::MtimeHash);

                let out = dir.path().join("artifact.bin");
                std::fs::write(&out, &original).unwrap();
                let key = ContentHash::from(blake3::hash(b"shared_cache_key"));
                store.record(key.clone(), &[out.as_path()], None).unwrap();

                corrupt_with_later_mtime(&out, &corrupted);

                prop_assert!(
                    !store.is_cached(&key, &[out.as_path()]).unwrap(),
                    "MtimeHash must detect same-size corruption with later mtime"
                );
            }

            /// Same property under `ContentHash` (always hashes).
            #[test]
            fn content_hash_detects_same_size_later_mtime_corruption(
                (original, corrupted) in content_and_corruption(),
            ) {
                let dir = tempfile::tempdir().unwrap();
                let oxdir = dir.path().join(".oxymake");
                let mut store = make_store_with(&oxdir, CacheValidation::ContentHash);

                let out = dir.path().join("artifact.bin");
                std::fs::write(&out, &original).unwrap();
                let key = ContentHash::from(blake3::hash(b"shared_cache_key"));
                store.record(key.clone(), &[out.as_path()], None).unwrap();

                corrupt_with_later_mtime(&out, &corrupted);

                prop_assert!(
                    !store.is_cached(&key, &[out.as_path()]).unwrap(),
                    "ContentHash must detect same-size corruption with later mtime"
                );
            }
        }
    }
}
