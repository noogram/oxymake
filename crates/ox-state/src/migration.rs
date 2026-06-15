//! Schema migration engine for `state.db`.
//!
//! OxyMake uses forward-only migrations to evolve the database schema.
//! Each migration is a function that transforms the schema from version N
//! to version N+1.  Migrations run inside a transaction so that a failure
//! never leaves the database in a half-migrated state.
//!
//! The current schema version is stored in the `schema_version` table
//! (a single-row table with one `INTEGER` column).  A freshly created
//! database starts at version 0 and is migrated to the latest version
//! on first open.
//!
//! # Adding a new migration
//!
//! 1. Write a `migrate_vN_to_vN1` function.
//! 2. Add it to the match arm in [`migrate`].
//! 3. Bump `LATEST_VERSION`.

use rusqlite::Connection;

use crate::error::StateError;

/// The latest schema version.  Bump this when adding a migration.
const LATEST_VERSION: u32 = 9;

/// Run all pending migrations, bringing the database up to
/// `LATEST_VERSION`.
///
/// Each migration runs in its own transaction.  If a migration fails
/// the database is left at the last successfully applied version.
pub fn migrate(conn: &Connection) -> Result<(), StateError> {
    // Ensure the version-tracking table exists.
    conn.execute_batch("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);")?;

    let current: u32 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    if current >= LATEST_VERSION {
        return Ok(());
    }

    for v in current..LATEST_VERSION {
        match v {
            0 => migrate_v0_to_v1(conn)?,
            1 => migrate_v1_to_v2(conn)?,
            2 => migrate_v2_to_v3(conn)?,
            3 => migrate_v3_to_v4(conn)?,
            4 => migrate_v4_to_v5(conn)?,
            5 => migrate_v5_to_v6(conn)?,
            6 => migrate_v6_to_v7(conn)?,
            7 => migrate_v7_to_v8(conn)?,
            8 => migrate_v8_to_v9(conn)?,
            _ => {
                return Err(StateError::Migration {
                    from: v,
                    to: v + 1,
                    reason: "unknown migration step".into(),
                });
            }
        }
    }

    Ok(())
}

/// Migration from version 0 (fresh database) to version 1.
///
/// Creates the four core tables: `sessions`, `jobs`, `runs`, and
/// `job_history`.  These tables support the cooperative multi-session
/// execution model described in OXYMAKE-THESIS.md section 6.13.
fn migrate_v0_to_v1(conn: &Connection) -> Result<(), StateError> {
    conn.execute_batch(
        "
        BEGIN;

        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            pid INTEGER NOT NULL,
            hostname TEXT NOT NULL,
            started_at INTEGER NOT NULL,
            target_filter TEXT,
            heartbeat_at INTEGER NOT NULL,
            status TEXT NOT NULL CHECK(status IN ('active', 'completed', 'interrupted'))
        );

        CREATE TABLE IF NOT EXISTS jobs (
            id TEXT PRIMARY KEY,
            rule_name TEXT NOT NULL,
            wildcards TEXT,
            status TEXT NOT NULL CHECK(status IN ('pending', 'running', 'completed', 'failed', 'skipped')),
            session_id TEXT REFERENCES sessions(id),
            locked_by TEXT,
            lock_expires_at INTEGER,
            cache_key TEXT,
            started_at INTEGER,
            completed_at INTEGER,
            exit_code INTEGER,
            output_hashes TEXT,
            log_path TEXT
        );

        CREATE TABLE IF NOT EXISTS runs (
            id TEXT PRIMARY KEY,
            started_at INTEGER NOT NULL,
            completed_at INTEGER,
            note TEXT,
            workflow_hash TEXT,
            job_count INTEGER,
            succeeded INTEGER DEFAULT 0,
            failed INTEGER DEFAULT 0,
            skipped INTEGER DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS job_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            run_id TEXT NOT NULL REFERENCES runs(id),
            job_id TEXT NOT NULL,
            rule_name TEXT NOT NULL,
            wildcards TEXT,
            input_hashes TEXT,
            output_hashes TEXT,
            params_hash TEXT,
            env_hash TEXT,
            executor TEXT,
            hostname TEXT,
            started_at INTEGER,
            completed_at INTEGER,
            wall_time_ms INTEGER,
            peak_mem_mb INTEGER,
            exit_code INTEGER
        );

        -- Record the new schema version.
        DELETE FROM schema_version;
        INSERT INTO schema_version (version) VALUES (1);

        COMMIT;
        ",
    )
    .map_err(|e| StateError::Migration {
        from: 0,
        to: 1,
        reason: e.to_string(),
    })?;

    Ok(())
}

/// Migration from version 1 to version 2.
///
/// Adds `'cancelled'` to the jobs status CHECK constraint so that
/// `ox cancel` can mark jobs as cancelled rather than resetting them
/// to pending.  SQLite does not support `ALTER TABLE … ALTER CONSTRAINT`,
/// so we recreate the table.
fn migrate_v1_to_v2(conn: &Connection) -> Result<(), StateError> {
    conn.execute_batch(
        "
        BEGIN;

        CREATE TABLE jobs_v2 (
            id TEXT PRIMARY KEY,
            rule_name TEXT NOT NULL,
            wildcards TEXT,
            status TEXT NOT NULL CHECK(status IN ('pending', 'running', 'completed', 'failed', 'skipped', 'cancelled')),
            session_id TEXT REFERENCES sessions(id),
            locked_by TEXT,
            lock_expires_at INTEGER,
            cache_key TEXT,
            started_at INTEGER,
            completed_at INTEGER,
            exit_code INTEGER,
            output_hashes TEXT,
            log_path TEXT
        );

        INSERT INTO jobs_v2 SELECT * FROM jobs;

        DROP TABLE jobs;

        ALTER TABLE jobs_v2 RENAME TO jobs;

        DELETE FROM schema_version;
        INSERT INTO schema_version (version) VALUES (2);

        COMMIT;
        ",
    )
    .map_err(|e| StateError::Migration {
        from: 1,
        to: 2,
        reason: e.to_string(),
    })?;

    Ok(())
}

/// Migration from version 2 to version 3.
///
/// Adds `snapshots` and `snapshot_jobs` tables for named workflow state
/// snapshots.  A snapshot captures the Oxymakefile hash and per-job state
/// at a point in time, enabling comparison across parameter changes.
fn migrate_v2_to_v3(conn: &Connection) -> Result<(), StateError> {
    conn.execute_batch(
        "
        BEGIN;

        CREATE TABLE IF NOT EXISTS snapshots (
            name TEXT PRIMARY KEY,
            created_at INTEGER NOT NULL,
            workflow_hash TEXT,
            description TEXT
        );

        CREATE TABLE IF NOT EXISTS snapshot_jobs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            snapshot_name TEXT NOT NULL REFERENCES snapshots(name) ON DELETE CASCADE,
            job_id TEXT NOT NULL,
            rule_name TEXT NOT NULL,
            status TEXT NOT NULL,
            cache_key TEXT,
            output_hashes TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_snapshot_jobs_name
            ON snapshot_jobs(snapshot_name);

        DELETE FROM schema_version;
        INSERT INTO schema_version (version) VALUES (3);

        COMMIT;
        ",
    )
    .map_err(|e| StateError::Migration {
        from: 2,
        to: 3,
        reason: e.to_string(),
    })?;

    Ok(())
}

/// Migration from version 3 to version 4.
///
/// Adds the `gates` table for manual approval gates in workflows.
/// A gate pauses the scheduler until explicitly approved or rejected.
fn migrate_v3_to_v4(conn: &Connection) -> Result<(), StateError> {
    conn.execute_batch(
        "
        BEGIN;

        CREATE TABLE IF NOT EXISTS gates (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            rule_name TEXT NOT NULL,
            job_id TEXT NOT NULL,
            status TEXT NOT NULL CHECK(status IN ('pending', 'approved', 'rejected')) DEFAULT 'pending',
            created_at INTEGER NOT NULL,
            decided_at INTEGER,
            decided_by TEXT,
            reason TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_gates_status ON gates(status);
        CREATE INDEX IF NOT EXISTS idx_gates_job ON gates(job_id);

        DELETE FROM schema_version;
        INSERT INTO schema_version (version) VALUES (4);

        COMMIT;
        ",
    )
    .map_err(|e| StateError::Migration {
        from: 3,
        to: 4,
        reason: e.to_string(),
    })?;

    Ok(())
}

/// Migration from version 4 to version 5.
///
/// Adds the `job_edges` table for persisting DAG edges between jobs.
/// This enables the dashboard to render the dependency graph without
/// needing access to the in-memory `JobGraph`.
fn migrate_v4_to_v5(conn: &Connection) -> Result<(), StateError> {
    conn.execute_batch(
        "
        BEGIN;

        CREATE TABLE IF NOT EXISTS job_edges (
            from_job TEXT NOT NULL,
            to_job TEXT NOT NULL,
            edge_type TEXT NOT NULL DEFAULT 'dependency',
            PRIMARY KEY (from_job, to_job)
        );

        CREATE INDEX IF NOT EXISTS idx_job_edges_from ON job_edges(from_job);
        CREATE INDEX IF NOT EXISTS idx_job_edges_to ON job_edges(to_job);

        DELETE FROM schema_version;
        INSERT INTO schema_version (version) VALUES (5);

        COMMIT;
        ",
    )
    .map_err(|e| StateError::Migration {
        from: 4,
        to: 5,
        reason: e.to_string(),
    })?;

    Ok(())
}

/// Migration from version 5 to version 6.
///
/// Adds DAG-level submission tracking for remote executors (Ray, SLURM).
/// - `dag_submissions` table: tracks each DAG submission with run_id,
///   executor type, and dashboard address.
/// - `executor_submission_id` column on `jobs`: maps OxyMake job IDs to
///   executor-specific submission IDs (e.g., Ray submission IDs).
fn migrate_v5_to_v6(conn: &Connection) -> Result<(), StateError> {
    conn.execute_batch(
        "
        BEGIN;

        CREATE TABLE IF NOT EXISTS dag_submissions (
            run_id TEXT PRIMARY KEY,
            executor TEXT NOT NULL,
            dashboard_address TEXT,
            total_jobs INTEGER NOT NULL,
            submitted_at INTEGER NOT NULL,
            completed_at INTEGER
        );

        -- Add executor_submission_id column to jobs table for mapping
        -- OxyMake job IDs to remote executor submission IDs.
        ALTER TABLE jobs ADD COLUMN executor_submission_id TEXT;

        DELETE FROM schema_version;
        INSERT INTO schema_version (version) VALUES (6);

        COMMIT;
        ",
    )
    .map_err(|e| StateError::Migration {
        from: 5,
        to: 6,
        reason: e.to_string(),
    })?;

    Ok(())
}

/// Migration from version 6 to version 7.
///
/// Adds `run_id` column to the `jobs` table so that each job can be
/// associated with the `ox run` invocation that registered it.  This
/// enables `ox status` to scope its display to a single run instead
/// of dumping the accumulated state from all past runs.
fn migrate_v6_to_v7(conn: &Connection) -> Result<(), StateError> {
    conn.execute_batch(
        "
        BEGIN;

        ALTER TABLE jobs ADD COLUMN run_id TEXT REFERENCES runs(id);

        CREATE INDEX IF NOT EXISTS idx_jobs_run_id ON jobs(run_id);

        DELETE FROM schema_version;
        INSERT INTO schema_version (version) VALUES (7);

        COMMIT;
        ",
    )
    .map_err(|e| StateError::Migration {
        from: 6,
        to: 7,
        reason: e.to_string(),
    })?;

    Ok(())
}

/// Migration from version 7 to version 8.
///
/// Adds reproducibility and provenance columns to the `job_history` table
/// for Stage 2 artifact metadata tracking.
fn migrate_v7_to_v8(conn: &Connection) -> Result<(), StateError> {
    conn.execute_batch(
        "
        BEGIN;

        ALTER TABLE job_history ADD COLUMN reproducibility_class TEXT;
        ALTER TABLE job_history ADD COLUMN artifact_provenance_json TEXT;

        DELETE FROM schema_version;
        INSERT INTO schema_version (version) VALUES (8);

        COMMIT;
        ",
    )
    .map_err(|e| StateError::Migration {
        from: 7,
        to: 8,
        reason: e.to_string(),
    })?;

    Ok(())
}

/// Migration from version 8 to version 9.
///
/// Adds a `cached` column to `jobs` so that cached jobs are stored with
/// `status = 'completed', cached = 1` instead of `status = 'skipped'`.
/// Existing "skipped" rows are migrated to `completed` + `cached = 1`.
fn migrate_v8_to_v9(conn: &Connection) -> Result<(), StateError> {
    conn.execute_batch(
        "
        BEGIN;

        ALTER TABLE jobs ADD COLUMN cached INTEGER NOT NULL DEFAULT 0;

        UPDATE jobs SET status = 'completed', cached = 1 WHERE status = 'skipped';

        DELETE FROM schema_version;
        INSERT INTO schema_version (version) VALUES (9);

        COMMIT;
        ",
    )
    .map_err(|e| StateError::Migration {
        from: 8,
        to: 9,
        reason: e.to_string(),
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn fresh_database_migrates_to_v1() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 9);
    }

    #[test]
    fn migration_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        // Running again should be a no-op.
        migrate(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 9);
    }

    #[test]
    fn tables_exist_after_migration() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        for table in &[
            "sessions",
            "jobs",
            "runs",
            "job_history",
            "snapshots",
            "snapshot_jobs",
            "job_edges",
        ] {
            let exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(exists, "table {table} should exist after migration");
        }
    }

    #[test]
    fn v8_adds_reproducibility_columns_to_job_history() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        // Verify the new columns exist by inserting a row with them.
        conn.execute(
            "INSERT INTO runs (id, started_at) VALUES ('test-run', 1000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO job_history
                (run_id, job_id, rule_name, reproducibility_class, artifact_provenance_json)
             VALUES ('test-run', 'j1', 'build', 'deterministic', '{}')",
            [],
        )
        .unwrap();

        let (repro, prov): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT reproducibility_class, artifact_provenance_json FROM job_history WHERE job_id = 'j1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert_eq!(repro.as_deref(), Some("deterministic"));
        assert_eq!(prov.as_deref(), Some("{}"));
    }
}
