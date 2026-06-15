//! API endpoint handlers for the dashboard.
//!
//! Each handler opens a read-only connection to `state.db`, queries the
//! relevant data, and returns either JSON or HTML.  The database is opened
//! per-request (SQLite connections are cheap in WAL mode) to avoid holding
//! locks across requests.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Json};
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::IntervalStream;

use ox_core::model::ContentHash;
use ox_state::db::StateDb;

use crate::server::DashboardState;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Summary of the current workflow status.
#[derive(Debug, Serialize, Deserialize)]
pub struct StatusResponse {
    /// Number of pending jobs.
    pub pending: usize,
    /// Number of running jobs.
    pub running: usize,
    /// Number of completed jobs (includes cached).
    pub completed: usize,
    /// Number of failed jobs.
    pub failed: usize,
    /// Number of skipped jobs (non-cache reasons).
    pub skipped: usize,
    /// Number of cached jobs (completed via cache hit).
    pub cached: usize,
    /// Total jobs across all statuses.
    pub total: usize,
    /// Number of active sessions.
    pub active_sessions: usize,
}

/// A node in the DAG visualization.
#[derive(Debug, Serialize, Deserialize)]
pub struct DagNode {
    /// Job identifier.
    pub id: String,
    /// Rule that produced this job.
    pub rule_name: String,
    /// Current status (pending, running, completed, failed, skipped).
    pub status: String,
    /// JSON-encoded wildcard bindings.
    #[serde(default)]
    pub wildcards: String,
    /// UNIX timestamp when the job started (if running or completed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
}

/// Edge between two DAG nodes.
#[derive(Debug, Serialize, Deserialize)]
pub struct DagEdge {
    /// Source job ID.
    pub from: String,
    /// Target job ID.
    pub to: String,
}

/// DAG structure for graph rendering.
#[derive(Debug, Serialize, Deserialize)]
pub struct DagResponse {
    /// All nodes (jobs) in the DAG.
    pub nodes: Vec<DagNode>,
    /// All edges (dependencies) between nodes.
    pub edges: Vec<DagEdge>,
}

/// Query parameters for the jobs endpoint.
#[derive(Debug, Deserialize)]
pub struct JobsQuery {
    /// Optional status filter (pending, running, completed, failed, skipped).
    pub status: Option<String>,
}

/// Information about a single job.
#[derive(Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct JobInfo {
    /// Job identifier.
    pub id: String,
    /// Rule that produced this job.
    pub rule_name: String,
    /// Current status.
    pub status: String,
    /// JSON-encoded wildcard bindings.
    pub wildcards: String,
    /// UNIX timestamp when the job started (if running or completed).
    pub started_at: Option<u64>,
    /// UNIX timestamp when the job completed (if completed or failed).
    pub completed_at: Option<u64>,
    /// Process exit code (if completed or failed).
    pub exit_code: Option<i32>,
}

/// A single run from the run history.
#[derive(Debug, Serialize, Deserialize)]
pub struct RunInfo {
    pub id: String,
    pub started_at: u64,
    pub completed_at: Option<u64>,
    pub note: Option<String>,
    pub workflow_hash: Option<ContentHash>,
    pub job_count: Option<i64>,
    pub succeeded: Option<i64>,
    pub failed: Option<i64>,
    pub skipped: Option<i64>,
}

/// A job history entry within a run.
#[derive(Debug, Serialize, Deserialize)]
pub struct RunJobInfo {
    pub run_id: String,
    pub job_id: String,
    pub rule_name: String,
    pub wildcards: Option<String>,
    pub executor: Option<String>,
    pub hostname: Option<String>,
    pub started_at: Option<u64>,
    pub completed_at: Option<u64>,
    pub wall_time_ms: Option<u64>,
    pub peak_mem_mb: Option<u64>,
    pub exit_code: Option<i32>,
}

/// Per-rule pipeline statistics with timing data.
#[derive(Debug, Serialize, Deserialize)]
pub struct RuleStats {
    pub rule_name: String,
    pub completed: usize,
    pub total: usize,
    pub running: usize,
    pub pending: usize,
    pub failed: usize,
    /// Average wall-clock duration of completed jobs for this rule (ms).
    pub avg_wall_time_ms: u64,
    /// UNIX timestamp of the earliest job start for this rule.
    pub earliest_started_at: Option<u64>,
}

/// An approval gate record.
#[derive(Debug, Serialize, Deserialize)]
pub struct GateInfo {
    pub id: i64,
    pub rule_name: String,
    pub job_id: String,
    pub status: String,
    pub created_at: u64,
    pub decided_at: Option<u64>,
    pub decided_by: Option<String>,
    pub reason: Option<String>,
}

/// Detailed information for a single job.
#[derive(Debug, Serialize, Deserialize)]
pub struct JobDetail {
    pub id: String,
    pub rule_name: String,
    pub wildcards: String,
    pub status: String,
    pub started_at: Option<u64>,
    pub log_path: Option<String>,
    pub stderr_tail: Option<String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /` — Serve the dashboard HTML shell.
pub async fn index_handler() -> impl IntoResponse {
    Html(include_str!("../templates/index.html"))
}

/// `GET /api/status` — Current workflow status summary.
///
/// Opens state.db, reads job counts and active session count,
/// returns a [`StatusResponse`].
pub async fn api_status(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<StatusResponse>, StatusError> {
    let db = StateDb::open(&state.db_path).map_err(|_| StatusError::DbOpen)?;
    let counts = db.job_counts().map_err(|_| StatusError::Query)?;
    let sessions = db.active_sessions().map_err(|_| StatusError::Query)?;

    Ok(Json(StatusResponse {
        pending: counts.pending,
        running: counts.running,
        completed: counts.completed,
        failed: counts.failed,
        skipped: counts.skipped,
        cached: counts.cached,
        total: counts.pending + counts.running + counts.completed + counts.failed + counts.skipped,
        active_sessions: sessions.len(),
    }))
}

/// `GET /api/dag` — DAG structure as JSON.
///
/// Returns nodes (jobs with status) and edges (dependencies).
/// In this scaffold, edges are empty — the full DAG reconstruction
/// from state.db comes in a later phase when `ox-core` graph data
/// is persisted.
pub async fn api_dag(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<DagResponse>, StatusError> {
    let db = StateDb::open(&state.db_path).map_err(|_| StatusError::DbOpen)?;

    // Build nodes from all jobs in the database with full details.
    let all = db.all_jobs_detail().map_err(|_| StatusError::Query)?;
    let nodes: Vec<DagNode> = all
        .into_iter()
        .map(|j| DagNode {
            id: j.id,
            rule_name: j.rule_name,
            wildcards: j.wildcards,
            status: j.status,
            started_at: j.started_at,
        })
        .collect();

    // Query persisted job-to-job edges from state.db.
    let edges = db
        .job_edges()
        .map_err(|_| StatusError::Query)?
        .into_iter()
        .map(|(from, to)| DagEdge { from, to })
        .collect();

    Ok(Json(DagResponse { nodes, edges }))
}

/// `GET /api/jobs` — List jobs with optional status filter.
///
/// Query parameters:
/// - `status` (optional): filter by job status
pub async fn api_jobs(
    Query(params): Query<JobsQuery>,
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<Vec<JobInfo>>, StatusError> {
    let db = StateDb::open(&state.db_path).map_err(|_| StatusError::DbOpen)?;

    let all_detail = db.all_jobs_detail().map_err(|_| StatusError::Query)?;

    let jobs: Vec<JobInfo> = all_detail
        .into_iter()
        .filter(|j| params.status.as_ref().is_none_or(|f| f == &j.status))
        .map(|j| JobInfo {
            id: j.id,
            rule_name: j.rule_name,
            status: j.status,
            wildcards: j.wildcards,
            started_at: j.started_at,
            completed_at: j.completed_at,
            exit_code: j.exit_code,
        })
        .collect();

    Ok(Json(jobs))
}

/// `GET /api/events` — SSE stream of live state changes.
///
/// Polls state.db every second and emits status updates as SSE events.
/// In a later phase, this will subscribe to the EventBus for sub-millisecond
/// latency when running in-process.
pub async fn api_events_sse(
    State(state): State<Arc<DashboardState>>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let interval = tokio::time::interval(Duration::from_secs(1));
    let stream = IntervalStream::new(interval).map(move |_| {
        let event = match StateDb::open(&state.db_path) {
            Ok(db) => match db.job_counts() {
                Ok(counts) => {
                    let status = StatusResponse {
                        pending: counts.pending,
                        running: counts.running,
                        completed: counts.completed,
                        failed: counts.failed,
                        skipped: counts.skipped,
                        cached: counts.cached,
                        total: counts.pending
                            + counts.running
                            + counts.completed
                            + counts.failed
                            + counts.skipped,
                        active_sessions: 0, // Skip session query in SSE for performance
                    };
                    Event::default()
                        .event("status")
                        .data(serde_json::to_string(&status).unwrap_or_default())
                }
                Err(_) => Event::default()
                    .event("error")
                    .data("Failed to query job counts"),
            },
            Err(_) => Event::default()
                .event("error")
                .data("Failed to open state.db"),
        };
        Ok(event)
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// `GET /api/runs` — Run history with timing.
pub async fn api_runs(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<Vec<RunInfo>>, StatusError> {
    let db = StateDb::open(&state.db_path).map_err(|_| StatusError::DbOpen)?;
    let runs = db.list_runs().map_err(|_| StatusError::Query)?;

    Ok(Json(
        runs.into_iter()
            .map(|r| RunInfo {
                id: r.id,
                started_at: r.started_at,
                completed_at: r.completed_at,
                note: r.note,
                workflow_hash: r.workflow_hash,
                job_count: r.job_count,
                succeeded: r.succeeded,
                failed: r.failed,
                skipped: r.skipped,
            })
            .collect(),
    ))
}

/// `GET /api/runs/:id/jobs` — Job history for a specific run.
pub async fn api_run_jobs(
    Path(run_id): Path<String>,
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<Vec<RunJobInfo>>, StatusError> {
    let db = StateDb::open(&state.db_path).map_err(|_| StatusError::DbOpen)?;
    let entries = db
        .job_history_for_run(&run_id)
        .map_err(|_| StatusError::Query)?;

    Ok(Json(
        entries
            .into_iter()
            .map(|e| RunJobInfo {
                run_id: e.run_id,
                job_id: e.job_id,
                rule_name: e.rule_name,
                wildcards: e.wildcards,
                executor: e.executor,
                hostname: e.hostname,
                started_at: e.started_at,
                completed_at: e.completed_at,
                wall_time_ms: e.wall_time_ms,
                peak_mem_mb: e.peak_mem_mb,
                exit_code: e.exit_code,
            })
            .collect(),
    ))
}

/// `GET /api/stats/rules` — Per-rule pipeline statistics with timing.
pub async fn api_stats_rules(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<Vec<RuleStats>>, StatusError> {
    let db = StateDb::open(&state.db_path).map_err(|_| StatusError::DbOpen)?;
    let stats = db
        .pipeline_stats_with_timing()
        .map_err(|_| StatusError::Query)?;

    Ok(Json(
        stats
            .into_iter()
            .map(|s| RuleStats {
                rule_name: s.rule_name,
                completed: s.completed,
                total: s.total,
                running: s.running,
                pending: s.pending,
                failed: s.failed,
                avg_wall_time_ms: s.avg_wall_time_ms,
                earliest_started_at: s.earliest_started_at,
            })
            .collect(),
    ))
}

/// `GET /api/gates` — Approval gates with status.
pub async fn api_gates(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<Vec<GateInfo>>, StatusError> {
    let db = StateDb::open(&state.db_path).map_err(|_| StatusError::DbOpen)?;
    let gates = db.list_gates().map_err(|_| StatusError::Query)?;

    Ok(Json(
        gates
            .into_iter()
            .map(|g| GateInfo {
                id: g.id,
                rule_name: g.rule_name,
                job_id: g.job_id,
                status: g.status,
                created_at: g.created_at,
                decided_at: g.decided_at,
                decided_by: g.decided_by,
                reason: g.reason,
            })
            .collect(),
    ))
}

/// `GET /api/job/:id` — Single job detail (status + log info).
pub async fn api_job_detail(
    Path(job_id): Path<String>,
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<JobDetail>, StatusError> {
    let db = StateDb::open(&state.db_path).map_err(|_| StatusError::DbOpen)?;

    // Get full job row: id, rule_name, wildcards, status, started_at
    let all = db.all_jobs_detail().map_err(|_| StatusError::Query)?;
    let job = all
        .into_iter()
        .find(|j| j.id == job_id)
        .ok_or(StatusError::NotFound)?;

    let log_path = db
        .job_log_info(&job_id)
        .map_err(|_| StatusError::Query)?
        .and_then(|info| info.log_path);

    // Try to read last few lines of log file as stderr_tail
    let stderr_tail = log_path.as_ref().and_then(|p| {
        std::fs::read_to_string(p).ok().map(|content| {
            let lines: Vec<&str> = content.lines().collect();
            let start = if lines.len() > 10 {
                lines.len() - 10
            } else {
                0
            };
            lines[start..].join("\n")
        })
    });

    Ok(Json(JobDetail {
        id: job_id,
        rule_name: job.rule_name,
        wildcards: job.wildcards,
        status: job.status,
        started_at: job.started_at,
        log_path,
        stderr_tail,
    }))
}

// ---------------------------------------------------------------------------
// Error handling
// ---------------------------------------------------------------------------

/// Errors that can occur in dashboard API handlers.
#[derive(Debug)]
pub enum StatusError {
    /// Failed to open the state database.
    DbOpen,
    /// Failed to execute a query.
    Query,
    /// Requested resource was not found.
    NotFound,
}

impl IntoResponse for StatusError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            StatusError::DbOpen => (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Cannot open state.db — is OxyMake initialized?",
            ),
            StatusError::Query => (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "Database query failed",
            ),
            StatusError::NotFound => (axum::http::StatusCode::NOT_FOUND, "Resource not found"),
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic valid ContentHash from a test label (64 hex chars).
    fn ch(label: &str) -> ContentHash {
        let mut hex: String = label.bytes().map(|b| format!("{b:02x}")).collect();
        hex.truncate(64);
        ContentHash::from_hex(format!("{hex:0<64}")).unwrap()
    }
    use crate::server::create_router;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tempfile::NamedTempFile;
    use tower::ServiceExt;

    // -- Response type serialization unit tests --

    #[test]
    fn status_response_roundtrips() {
        let resp = StatusResponse {
            pending: 1,
            running: 2,
            completed: 3,
            failed: 4,
            skipped: 0,
            cached: 5,
            total: 10,
            active_sessions: 0,
        };
        let json_str = serde_json::to_string(&resp).unwrap();
        let parsed: StatusResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed.total, 10);
    }

    #[test]
    fn dag_node_omits_none_started_at() {
        let node = DagNode {
            id: "j2".into(),
            rule_name: "qc".into(),
            status: "pending".into(),
            wildcards: "{}".into(),
            started_at: None,
        };
        let json_str = serde_json::to_string(&node).unwrap();
        assert!(!json_str.contains("started_at"));
    }

    #[test]
    fn dag_edge_roundtrips() {
        let edge = DagEdge {
            from: "a".into(),
            to: "b".into(),
        };
        let json_str = serde_json::to_string(&edge).unwrap();
        let parsed: DagEdge = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed.from, "a");
        assert_eq!(parsed.to, "b");
    }

    #[test]
    fn job_info_roundtrips() {
        let job = JobInfo {
            id: "j42".into(),
            rule_name: "align".into(),
            status: "failed".into(),
            wildcards: "{}".into(),
            started_at: Some(100),
            completed_at: Some(200),
            exit_code: Some(1),
        };
        let json_str = serde_json::to_string(&job).unwrap();
        let parsed: JobInfo = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed.exit_code, Some(1));
    }

    #[test]
    fn jobs_query_deserializes_with_status() {
        let q: JobsQuery = serde_json::from_str(r#"{"status":"failed"}"#).unwrap();
        assert_eq!(q.status, Some("failed".into()));
    }

    #[test]
    fn jobs_query_deserializes_without_status() {
        let q: JobsQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.status.is_none());
    }

    #[test]
    fn status_error_db_open_is_503() {
        let resp = StatusError::DbOpen.into_response();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn status_error_query_is_500() {
        let resp = StatusError::Query.into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn status_error_not_found_is_404() {
        let resp = StatusError::NotFound.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // -- Integration tests (with seeded state.db) --

    /// Create a temporary state.db and return the router + temp file handle.
    fn test_app() -> (NamedTempFile, Router) {
        let tmp = NamedTempFile::new().unwrap();
        let db = StateDb::open(tmp.path()).unwrap();

        // Seed some test data.
        let sid = db.create_session(1, "test-host", None).unwrap();
        let jobs = vec![
            ox_state::db::JobRecord {
                id: "build-A".into(),
                rule_name: "build".into(),
                wildcards: r#"{"target":"A"}"#.into(),
                cache_key: None,
                run_id: None,
            },
            ox_state::db::JobRecord {
                id: "build-B".into(),
                rule_name: "build".into(),
                wildcards: r#"{"target":"B"}"#.into(),
                cache_key: None,
                run_id: None,
            },
            ox_state::db::JobRecord {
                id: "test-A".into(),
                rule_name: "test".into(),
                wildcards: r#"{"target":"A"}"#.into(),
                cache_key: None,
                run_id: None,
            },
        ];
        db.register_jobs(&jobs).unwrap();
        db.claim_job("build-A", &sid).unwrap();
        db.complete_job("build-A", &sid, 0, "{}").unwrap();
        db.claim_job("build-B", &sid).unwrap();
        // build-B left running
        // test-A left pending

        // Seed a run with job history.
        db.begin_run("run-1", Some(&ch("abc123")), 3, Some("test run"))
            .unwrap();
        db.record_job_history(&ox_state::db::JobHistoryEntry {
            run_id: "run-1".into(),
            job_id: "build-A".into(),
            rule_name: "build".into(),
            wildcards: Some(r#"{"target":"A"}"#.into()),
            input_hashes: None,
            output_hashes: None,
            params_hash: None,
            env_hash: None,
            executor: Some("local".into()),
            hostname: Some("test-host".into()),
            started_at: Some(1000),
            completed_at: Some(2000),
            wall_time_ms: Some(1000),
            peak_mem_mb: Some(128),
            exit_code: Some(0),
            reproducibility_class: None,
            artifact_provenance_json: None,
        })
        .unwrap();
        db.end_run("run-1", 1, 0, 0).unwrap();

        // Seed a gate.
        db.create_gate("deploy", "build-A").unwrap();

        drop(db);

        let state = Arc::new(DashboardState {
            db_path: tmp.path().to_path_buf(),
        });
        let router = create_router(state);
        (tmp, router)
    }

    #[tokio::test]
    async fn index_returns_html() {
        let (_tmp, app) = test_app();
        let req = Request::builder().uri("/").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("OxyMake Dashboard"));
        assert!(html.contains("htmx"));
    }

    #[tokio::test]
    async fn api_status_returns_valid_json() {
        let (_tmp, app) = test_app();
        let req = Request::builder()
            .uri("/api/status")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let status: StatusResponse = serde_json::from_slice(&body).unwrap();

        assert_eq!(status.completed, 1); // build-A
        assert_eq!(status.running, 1); // build-B
        assert_eq!(status.pending, 1); // test-A
        assert_eq!(status.total, 3);
        assert_eq!(status.active_sessions, 1);
    }

    #[tokio::test]
    async fn api_dag_returns_nodes() {
        let (_tmp, app) = test_app();
        let req = Request::builder()
            .uri("/api/dag")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let dag: DagResponse = serde_json::from_slice(&body).unwrap();

        assert_eq!(dag.nodes.len(), 3);
        assert!(dag.edges.is_empty()); // No edges registered in this test

        // Verify nodes have rule_name populated
        let build_nodes: Vec<_> = dag
            .nodes
            .iter()
            .filter(|n| n.rule_name == "build")
            .collect();
        assert_eq!(build_nodes.len(), 2);
        let test_nodes: Vec<_> = dag.nodes.iter().filter(|n| n.rule_name == "test").collect();
        assert_eq!(test_nodes.len(), 1);
    }

    #[tokio::test]
    async fn api_dag_returns_edges() {
        let tmp = NamedTempFile::new().unwrap();
        let db = StateDb::open(tmp.path()).unwrap();

        let sid = db.create_session(1, "test-host", None).unwrap();
        let jobs = vec![
            ox_state::db::JobRecord {
                id: "build-A".into(),
                rule_name: "build".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
            ox_state::db::JobRecord {
                id: "test-A".into(),
                rule_name: "test".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            },
        ];
        db.register_jobs(&jobs).unwrap();
        db.register_edges(&[("build-A".into(), "test-A".into())])
            .unwrap();
        db.claim_job("build-A", &sid).unwrap();

        drop(db);

        let state = Arc::new(DashboardState {
            db_path: tmp.path().to_path_buf(),
        });
        let app = create_router(state);
        let req = Request::builder()
            .uri("/api/dag")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let dag: DagResponse = serde_json::from_slice(&body).unwrap();

        assert_eq!(dag.nodes.len(), 2);
        assert_eq!(dag.edges.len(), 1);
        assert_eq!(dag.edges[0].from, "build-A");
        assert_eq!(dag.edges[0].to, "test-A");
    }

    #[tokio::test]
    async fn api_jobs_with_status_filter() {
        let (_tmp, app) = test_app();
        let req = Request::builder()
            .uri("/api/jobs?status=running")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let jobs: Vec<JobInfo> = serde_json::from_slice(&body).unwrap();

        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, "build-B");
        assert_eq!(jobs[0].rule_name, "build");
        assert_eq!(jobs[0].status, "running");
    }

    #[tokio::test]
    async fn api_jobs_all() {
        let (_tmp, app) = test_app();
        let req = Request::builder()
            .uri("/api/jobs")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let jobs: Vec<JobInfo> = serde_json::from_slice(&body).unwrap();

        assert_eq!(jobs.len(), 3);
    }

    #[tokio::test]
    async fn api_runs_returns_run_history() {
        let (_tmp, app) = test_app();
        let req = Request::builder()
            .uri("/api/runs")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let runs: Vec<RunInfo> = serde_json::from_slice(&body).unwrap();

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].id, "run-1");
        assert_eq!(runs[0].succeeded, Some(1));
    }

    #[tokio::test]
    async fn api_run_jobs_returns_history() {
        let (_tmp, app) = test_app();
        let req = Request::builder()
            .uri("/api/runs/run-1/jobs")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let jobs: Vec<RunJobInfo> = serde_json::from_slice(&body).unwrap();

        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].job_id, "build-A");
        assert_eq!(jobs[0].wall_time_ms, Some(1000));
        assert_eq!(jobs[0].peak_mem_mb, Some(128));
    }

    #[tokio::test]
    async fn api_run_jobs_empty_for_unknown_run() {
        let (_tmp, app) = test_app();
        let req = Request::builder()
            .uri("/api/runs/nonexistent/jobs")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let jobs: Vec<RunJobInfo> = serde_json::from_slice(&body).unwrap();
        assert!(jobs.is_empty());
    }

    #[tokio::test]
    async fn api_stats_rules_returns_pipeline_stats_with_timing() {
        let (_tmp, app) = test_app();
        let req = Request::builder()
            .uri("/api/stats/rules")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let stats: Vec<RuleStats> = serde_json::from_slice(&body).unwrap();

        // We have "build" (2 jobs: 1 completed, 1 running) and "test" (1 job: pending)
        assert_eq!(stats.len(), 2);

        let build = stats.iter().find(|s| s.rule_name == "build").unwrap();
        assert_eq!(build.total, 2);
        assert_eq!(build.completed, 1); // build-A completed
        assert_eq!(build.running, 1); // build-B running
        // build-A completed_at - started_at may be 0 in fast tests, but field should exist
        // avg_wall_time_ms is u64 so just check it deserializes correctly
        assert!(build.earliest_started_at.is_some());

        let test_rule = stats.iter().find(|s| s.rule_name == "test").unwrap();
        assert_eq!(test_rule.total, 1);
        assert_eq!(test_rule.pending, 1);
        assert_eq!(test_rule.avg_wall_time_ms, 0); // no completed jobs
    }

    #[tokio::test]
    async fn api_gates_returns_gate_list() {
        let (_tmp, app) = test_app();
        let req = Request::builder()
            .uri("/api/gates")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let gates: Vec<GateInfo> = serde_json::from_slice(&body).unwrap();

        assert_eq!(gates.len(), 1);
        assert_eq!(gates[0].rule_name, "deploy");
        assert_eq!(gates[0].job_id, "build-A");
        assert_eq!(gates[0].status, "pending");
    }

    #[tokio::test]
    async fn api_job_detail_returns_job() {
        let (_tmp, app) = test_app();
        let req = Request::builder()
            .uri("/api/job/build-A")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let detail: JobDetail = serde_json::from_slice(&body).unwrap();

        assert_eq!(detail.id, "build-A");
        assert_eq!(detail.rule_name, "build");
        assert_eq!(detail.status, "completed");
        assert_eq!(detail.wildcards, r#"{"target":"A"}"#);
    }

    #[tokio::test]
    async fn api_job_detail_not_found() {
        let (_tmp, app) = test_app();
        let req = Request::builder()
            .uri("/api/job/nonexistent")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // -- Additional response type roundtrip tests --

    #[test]
    fn run_info_roundtrips() {
        let run = RunInfo {
            id: "run-42".into(),
            started_at: 1000,
            completed_at: Some(2000),
            note: Some("test run".into()),
            workflow_hash: Some(ch("abc")),
            job_count: Some(10),
            succeeded: Some(8),
            failed: Some(1),
            skipped: Some(1),
        };
        let json_str = serde_json::to_string(&run).unwrap();
        let parsed: RunInfo = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed.id, "run-42");
        assert_eq!(parsed.succeeded, Some(8));
        assert_eq!(parsed.failed, Some(1));
    }

    #[test]
    fn run_info_optional_fields_absent() {
        let run = RunInfo {
            id: "run-1".into(),
            started_at: 100,
            completed_at: None,
            note: None,
            workflow_hash: None,
            job_count: None,
            succeeded: None,
            failed: None,
            skipped: None,
        };
        let json_str = serde_json::to_string(&run).unwrap();
        let parsed: RunInfo = serde_json::from_str(&json_str).unwrap();
        assert!(parsed.completed_at.is_none());
        assert!(parsed.note.is_none());
    }

    #[test]
    fn run_job_info_roundtrips() {
        let entry = RunJobInfo {
            run_id: "run-1".into(),
            job_id: "build-A".into(),
            rule_name: "build".into(),
            wildcards: Some(r#"{"target":"A"}"#.into()),
            executor: Some("local".into()),
            hostname: Some("host-1".into()),
            started_at: Some(1000),
            completed_at: Some(2000),
            wall_time_ms: Some(1000),
            peak_mem_mb: Some(256),
            exit_code: Some(0),
        };
        let json_str = serde_json::to_string(&entry).unwrap();
        let parsed: RunJobInfo = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed.wall_time_ms, Some(1000));
        assert_eq!(parsed.peak_mem_mb, Some(256));
        assert_eq!(parsed.executor, Some("local".into()));
    }

    #[test]
    fn rule_stats_roundtrips() {
        let stats = RuleStats {
            rule_name: "align".into(),
            completed: 90,
            total: 100,
            running: 5,
            pending: 4,
            failed: 1,
            avg_wall_time_ms: 5000,
            earliest_started_at: Some(1000),
        };
        let json_str = serde_json::to_string(&stats).unwrap();
        let parsed: RuleStats = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed.rule_name, "align");
        assert_eq!(parsed.total, 100);
        assert_eq!(parsed.avg_wall_time_ms, 5000);
    }

    #[test]
    fn gate_info_roundtrips() {
        let gate = GateInfo {
            id: 1,
            rule_name: "deploy".into(),
            job_id: "build-A".into(),
            status: "approved".into(),
            created_at: 1000,
            decided_at: Some(2000),
            decided_by: Some("alice".into()),
            reason: Some("looks good".into()),
        };
        let json_str = serde_json::to_string(&gate).unwrap();
        let parsed: GateInfo = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed.decided_by, Some("alice".into()));
        assert_eq!(parsed.reason, Some("looks good".into()));
    }

    #[test]
    fn gate_info_pending_has_no_decision() {
        let gate = GateInfo {
            id: 2,
            rule_name: "review".into(),
            job_id: "test-B".into(),
            status: "pending".into(),
            created_at: 500,
            decided_at: None,
            decided_by: None,
            reason: None,
        };
        let json_str = serde_json::to_string(&gate).unwrap();
        let parsed: GateInfo = serde_json::from_str(&json_str).unwrap();
        assert!(parsed.decided_at.is_none());
        assert!(parsed.decided_by.is_none());
    }

    #[test]
    fn job_detail_roundtrips() {
        let detail = JobDetail {
            id: "build-A".into(),
            rule_name: "build".into(),
            wildcards: r#"{"target":"A"}"#.into(),
            status: "completed".into(),
            started_at: Some(1000),
            log_path: Some("/tmp/logs/build-A.log".into()),
            stderr_tail: Some("warning: unused variable".into()),
        };
        let json_str = serde_json::to_string(&detail).unwrap();
        let parsed: JobDetail = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed.id, "build-A");
        assert_eq!(parsed.log_path, Some("/tmp/logs/build-A.log".into()));
    }

    #[test]
    fn dag_response_roundtrips() {
        let dag = DagResponse {
            nodes: vec![
                DagNode {
                    id: "a".into(),
                    rule_name: "build".into(),
                    status: "completed".into(),
                    wildcards: "{}".into(),
                    started_at: Some(100),
                },
                DagNode {
                    id: "b".into(),
                    rule_name: "test".into(),
                    status: "pending".into(),
                    wildcards: "{}".into(),
                    started_at: None,
                },
            ],
            edges: vec![DagEdge {
                from: "a".into(),
                to: "b".into(),
            }],
        };
        let json_str = serde_json::to_string(&dag).unwrap();
        let parsed: DagResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed.nodes.len(), 2);
        assert_eq!(parsed.edges.len(), 1);
        assert_eq!(parsed.edges[0].from, "a");
    }

    #[tokio::test]
    async fn api_status_missing_db_returns_503() {
        let state = Arc::new(DashboardState {
            db_path: "/nonexistent/state.db".into(),
        });
        let app = create_router(state);
        let req = Request::builder()
            .uri("/api/status")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// Regression test for ox-g9ei: verify that event-driven state updates
    /// make the dashboard reflect live activity during a run, not just after.
    ///
    /// Simulates the pattern: register jobs as pending → emit JobStarted →
    /// dashboard sees "running" immediately (without waiting for scheduler
    /// completion).
    #[tokio::test]
    async fn live_state_updates_via_events_ox_g9ei() {
        use ox_core::event::EventBus;
        use ox_core::model::{Event, JobId};

        let tmp = NamedTempFile::new().unwrap();
        let db_path = tmp.path().to_path_buf();

        // Set up state.db with pending jobs.
        {
            let db = StateDb::open(&db_path).unwrap();
            let sid = db.create_session(1, "test-host", None).unwrap();
            let jobs = vec![
                ox_state::db::JobRecord {
                    id: "compile-X".into(),
                    rule_name: "compile".into(),
                    wildcards: "{}".into(),
                    cache_key: None,
                    run_id: None,
                },
                ox_state::db::JobRecord {
                    id: "compile-Y".into(),
                    rule_name: "compile".into(),
                    wildcards: "{}".into(),
                    cache_key: None,
                    run_id: None,
                },
            ];
            db.register_jobs(&jobs).unwrap();

            // Simulate the event-driven subscriber (mirrors run.rs logic).
            let bus = EventBus::new();
            let mut rx = bus.subscribe();
            let writer_db_path = db_path.clone();
            let writer_sid = sid.clone();
            let handle = tokio::spawn(async move {
                let wdb = StateDb::open(&writer_db_path).unwrap();
                while let Ok(event) = rx.recv().await {
                    match event {
                        Event::JobStarted { ref job_id, .. } => {
                            let _ = wdb.claim_job(job_id.as_str(), &writer_sid);
                        }
                        Event::JobCompleted { ref job_id, .. } => {
                            // Claim first in case JobStarted was never received.
                            let _ = wdb.claim_job(job_id.as_str(), &writer_sid);
                            let _ = wdb.complete_job(job_id.as_str(), &writer_sid, 0, "");
                        }
                        Event::JobFailed {
                            ref job_id,
                            exit_code,
                            ..
                        } => {
                            let _ = wdb.claim_job(job_id.as_str(), &writer_sid);
                            let _ =
                                wdb.fail_job(job_id.as_str(), &writer_sid, exit_code.unwrap_or(1));
                        }
                        Event::JobSkipped { ref job_id, .. } => {
                            let _ = wdb.skip_job(job_id.as_str());
                        }
                        Event::JobCancelled { ref job_id, .. } => {
                            let _ = wdb.cancel_job_ids(&[job_id.to_string()]);
                        }
                        _ => {}
                    }
                }
            });

            // Before any events: dashboard should see all pending.
            let state = Arc::new(DashboardState {
                db_path: db_path.clone(),
            });
            let app = create_router(state.clone());
            let req = Request::builder()
                .uri("/api/status")
                .body(Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let status: StatusResponse = serde_json::from_slice(&body).unwrap();
            assert_eq!(status.pending, 2, "both jobs should start as pending");
            assert_eq!(status.running, 0);

            // Emit JobStarted for compile-X.
            bus.emit(Event::JobStarted {
                job_id: JobId::from("compile-X"),
                executor: "local".into(),
                reason: None,
            });
            // Give the subscriber task a moment to process.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            // Dashboard should now show 1 running, 1 pending.
            let app = create_router(state.clone());
            let req = Request::builder()
                .uri("/api/status")
                .body(Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let status: StatusResponse = serde_json::from_slice(&body).unwrap();
            assert_eq!(status.running, 1, "compile-X should be running");
            assert_eq!(status.pending, 1, "compile-Y should still be pending");

            // Emit JobCompleted for compile-X.
            bus.emit(Event::JobCompleted {
                job_id: JobId::from("compile-X"),
                duration_ms: 100,
                outputs: vec![],
            });
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            // Dashboard should show 1 completed, 1 pending.
            let app = create_router(state.clone());
            let req = Request::builder()
                .uri("/api/status")
                .body(Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let status: StatusResponse = serde_json::from_slice(&body).unwrap();
            assert_eq!(status.completed, 1, "compile-X should be completed");
            assert_eq!(status.pending, 1, "compile-Y should still be pending");
            assert_eq!(status.running, 0);

            // Emit JobFailed for compile-Y.
            bus.emit(Event::JobFailed {
                job_id: JobId::from("compile-Y"),
                error_message: "segfault".into(),
                exit_code: Some(139),
                stderr_tail: None,
            });
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            // Dashboard should show 1 completed, 1 failed.
            let app = create_router(state.clone());
            let req = Request::builder()
                .uri("/api/status")
                .body(Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let status: StatusResponse = serde_json::from_slice(&body).unwrap();
            assert_eq!(status.completed, 1);
            assert_eq!(status.failed, 1);
            assert_eq!(status.pending, 0);
            assert_eq!(status.running, 0);

            // Clean up the subscriber task.
            handle.abort();
        }
    }
}
