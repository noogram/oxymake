//! Application state for the ox-top TUI.
//!
//! [`App`] is a pure data snapshot — the UI module renders it, the main
//! loop refreshes it from the database.  This separation keeps the code
//! testable without a terminal.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use ox_state::db::StateDb;
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Panel selection
// ---------------------------------------------------------------------------

/// Which panel the user has focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Pipeline,
    RunningJobs,
    Events,
}

impl Panel {
    const ALL: [Panel; 3] = [Panel::Pipeline, Panel::RunningJobs, Panel::Events];

    fn index(self) -> usize {
        match self {
            Panel::Pipeline => 0,
            Panel::RunningJobs => 1,
            Panel::Events => 2,
        }
    }
}

// ---------------------------------------------------------------------------
// Data structs
// ---------------------------------------------------------------------------

/// Per-stage progress aggregation for the Pipeline panel.
#[derive(Debug, Clone)]
pub struct StageStats {
    pub name: String,
    pub completed: usize,
    pub total: usize,
    pub running: usize,
}

impl StageStats {
    /// Fraction complete as 0.0..=1.0.
    pub fn progress_fraction(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.completed as f64 / self.total as f64
        }
    }

    /// Human-readable status label.
    pub fn status_label(&self) -> String {
        if self.completed == self.total {
            "done".into()
        } else if self.running > 0 {
            format!("{} running", self.running)
        } else {
            "waiting".into()
        }
    }
}

/// A single running job shown in the Running Jobs panel.
#[derive(Debug, Clone)]
pub struct RunningJob {
    pub id: String,
    pub rule: String,
    pub wildcards: String,
    pub duration: Duration,
    pub resources: String,
}

/// A single event line shown in the Events panel.
#[derive(Debug, Clone)]
pub struct EventLine {
    pub timestamp: String,
    pub icon: char,
    pub message: String,
}

/// Session info for the Sessions row.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub pid: u32,
    pub hostname: String,
    pub target_filter: Option<String>,
    pub running_count: usize,
}

/// Aggregated job counts.
#[derive(Debug, Clone, Default)]
pub struct JobCounts {
    pub pending: usize,
    pub running: usize,
    pub completed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub cached: usize,
}

impl JobCounts {
    pub fn total(&self) -> usize {
        self.pending + self.running + self.completed + self.failed + self.skipped
    }

    pub fn done(&self) -> usize {
        self.completed + self.failed + self.skipped
    }

    pub fn progress_fraction(&self) -> f64 {
        let t = self.total();
        if t == 0 {
            0.0
        } else {
            self.done() as f64 / t as f64
        }
    }
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

/// All displayable state for the TUI.
///
/// Created via [`App::new`] (empty) or [`App::with_mock_data`]
/// (realistic-looking sample data for development).
#[derive(Debug)]
pub struct App {
    pub running: bool,
    pub selected_panel: Panel,
    pub pipeline_stats: Vec<StageStats>,
    pub running_jobs: Vec<RunningJob>,
    pub recent_events: Vec<EventLine>,
    pub sessions: Vec<SessionInfo>,
    pub job_counts: JobCounts,
    pub elapsed: Duration,
    pub run_id: u32,
}

impl App {
    /// Create an empty app (no data loaded yet).
    pub fn new() -> Self {
        Self {
            running: true,
            selected_panel: Panel::Pipeline,
            pipeline_stats: Vec::new(),
            running_jobs: Vec::new(),
            recent_events: Vec::new(),
            sessions: Vec::new(),
            job_counts: JobCounts::default(),
            elapsed: Duration::ZERO,
            run_id: 0,
        }
    }

    /// Create an app pre-loaded with realistic mock data for development.
    pub fn with_mock_data() -> Self {
        Self {
            running: true,
            selected_panel: Panel::Pipeline,
            pipeline_stats: vec![
                StageStats {
                    name: "data".into(),
                    completed: 3,
                    total: 3,
                    running: 0,
                },
                StageStats {
                    name: "features".into(),
                    completed: 1450,
                    total: 3412,
                    running: 145,
                },
                StageStats {
                    name: "call".into(),
                    completed: 0,
                    total: 1024,
                    running: 0,
                },
                StageStats {
                    name: "annotate".into(),
                    completed: 0,
                    total: 48,
                    running: 0,
                },
            ],
            running_jobs: vec![
                RunningJob {
                    id: "features/human/chr1/NA12878".into(),
                    rule: "features".into(),
                    wildcards: "cohort=human, region=chr1, sample=NA12878".into(),
                    duration: Duration::from_secs(192),
                    resources: "cpu:4 mem:8G".into(),
                },
                RunningJob {
                    id: "features/human/chr1/NA12891".into(),
                    rule: "features".into(),
                    wildcards: "cohort=human, region=chr1, sample=NA12891".into(),
                    duration: Duration::from_secs(165),
                    resources: "cpu:4 mem:8G".into(),
                },
                RunningJob {
                    id: "features/mouse/chr1/NA12878".into(),
                    rule: "features".into(),
                    wildcards: "cohort=mouse, region=chr1, sample=NA12878".into(),
                    duration: Duration::from_secs(62),
                    resources: "cpu:4 mem:8G".into(),
                },
                RunningJob {
                    id: "features/yeast/chr1/NA12878".into(),
                    rule: "features".into(),
                    wildcards: "cohort=yeast, region=chr1, sample=NA12878".into(),
                    duration: Duration::from_secs(34),
                    resources: "cpu:4 mem:8G".into(),
                },
            ],
            recent_events: vec![
                EventLine {
                    timestamp: "14:32:15".into(),
                    icon: '\u{2713}',
                    message: "features/human/chr1/HG002 completed (4m12s)".into(),
                },
                EventLine {
                    timestamp: "14:32:14".into(),
                    icon: '\u{2192}',
                    message: "features/yeast/chr1/NA12878 started".into(),
                },
                EventLine {
                    timestamp: "14:32:10".into(),
                    icon: '\u{2717}',
                    message: "features/human/chr1/HG003 FAILED (exit 1)".into(),
                },
                EventLine {
                    timestamp: "14:32:08".into(),
                    icon: '\u{2713}',
                    message: "features/human/chr1/HG004 completed (2m45s)".into(),
                },
                EventLine {
                    timestamp: "14:32:05".into(),
                    icon: '\u{2500}',
                    message: "features/human/chr1/HG005 skipped (cached)".into(),
                },
            ],
            sessions: vec![
                SessionInfo {
                    id: "s-12345".into(),
                    pid: 12345,
                    hostname: "build-host".into(),
                    target_filter: Some("human".into()),
                    running_count: 312,
                },
                SessionInfo {
                    id: "s-12346".into(),
                    pid: 12346,
                    hostname: "build-host".into(),
                    target_filter: Some("mouse".into()),
                    running_count: 89,
                },
            ],
            job_counts: JobCounts {
                pending: 5808,
                running: 145,
                completed: 4291, // 847 executed + 3444 cached
                failed: 3,
                skipped: 0,
                cached: 3444,
            },
            elapsed: Duration::from_secs(154),
            run_id: 4,
        }
    }

    /// Move focus to the next panel (wrapping).
    pub fn next_panel(&mut self) {
        let idx = self.selected_panel.index();
        let next = (idx + 1) % Panel::ALL.len();
        self.selected_panel = Panel::ALL[next];
    }

    /// Move focus to the previous panel (wrapping).
    pub fn prev_panel(&mut self) {
        let idx = self.selected_panel.index();
        let prev = if idx == 0 {
            Panel::ALL.len() - 1
        } else {
            idx - 1
        };
        self.selected_panel = Panel::ALL[prev];
    }

    /// Refresh app state from the SQLite state database.
    ///
    /// Reads job counts, active sessions, running job details, and
    /// pipeline stats from `db`.  Called periodically by the main loop.
    pub fn refresh_from_db(&mut self, db: &StateDb) -> Result<()> {
        // Job counts
        let counts = db.job_counts()?;
        self.job_counts = JobCounts {
            pending: counts.pending,
            running: counts.running,
            completed: counts.completed,
            failed: counts.failed,
            skipped: counts.skipped,
            cached: counts.cached,
        };

        // Active sessions
        let sessions = db.active_sessions()?;
        self.sessions = sessions
            .into_iter()
            .map(|s| {
                // Count running jobs for this session by checking the DB.
                // For simplicity, we use the overall running count divided
                // across sessions or just set to 0 — the DB does not
                // directly expose per-session running counts cheaply.
                SessionInfo {
                    id: s.id,
                    pid: s.pid,
                    hostname: s.hostname,
                    target_filter: s.target_filter,
                    running_count: 0, // updated below
                }
            })
            .collect();

        // Running jobs detail
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let running_details = db.running_jobs_detail()?;
        self.running_jobs = running_details
            .iter()
            .map(|detail| {
                let duration = detail
                    .started_at
                    .map(|t| Duration::from_secs(now_secs.saturating_sub(t)))
                    .unwrap_or(Duration::ZERO);
                RunningJob {
                    id: detail.id.clone(),
                    rule: detail.rule_name.clone(),
                    wildcards: detail.wildcards.clone(),
                    duration,
                    resources: String::new(),
                }
            })
            .collect();

        // Pipeline stats (per-rule aggregation)
        let stats = db.pipeline_stats()?;
        self.pipeline_stats = stats
            .into_iter()
            .map(|s| StageStats {
                name: s.rule_name,
                completed: s.completed,
                total: s.total,
                running: s.running,
            })
            .collect();

        Ok(())
    }

    /// Apply a single NDJSON event to the app state.
    ///
    /// Events are expected to be JSON objects with a `type` field.
    /// Recognized event types:
    /// - `job_started`: adds to running jobs and events
    /// - `job_completed`: removes from running, updates counts, adds event
    /// - `job_failed`: removes from running, updates counts, adds event
    /// - `job_skipped`: updates counts, adds event
    pub fn apply_ndjson_event(&mut self, line: &str) -> Result<()> {
        let event: NdjsonEvent = serde_json::from_str(line)?;
        let timestamp = chrono_timestamp();

        match event.event_type.as_str() {
            "job_started" => {
                if let Some(id) = &event.job_id {
                    self.running_jobs.push(RunningJob {
                        id: id.clone(),
                        rule: event.rule.clone().unwrap_or_default(),
                        wildcards: event.wildcards.clone().unwrap_or_default(),
                        duration: Duration::ZERO,
                        resources: String::new(),
                    });
                    self.job_counts.running += 1;
                    if self.job_counts.pending > 0 {
                        self.job_counts.pending -= 1;
                    }
                    self.push_event(EventLine {
                        timestamp: timestamp.clone(),
                        icon: '\u{2192}',
                        message: format!("{} started", id),
                    });
                }
            }
            "job_completed" => {
                if let Some(id) = &event.job_id {
                    self.running_jobs.retain(|j| j.id != *id);
                    if self.job_counts.running > 0 {
                        self.job_counts.running -= 1;
                    }
                    self.job_counts.completed += 1;
                    let dur = event
                        .duration_ms
                        .map(|ms| crate::format::fmt_duration(Duration::from_millis(ms)))
                        .unwrap_or_default();
                    self.push_event(EventLine {
                        timestamp: timestamp.clone(),
                        icon: '\u{2713}',
                        message: format!("{} completed ({})", id, dur),
                    });
                }
            }
            "job_failed" => {
                if let Some(id) = &event.job_id {
                    self.running_jobs.retain(|j| j.id != *id);
                    if self.job_counts.running > 0 {
                        self.job_counts.running -= 1;
                    }
                    self.job_counts.failed += 1;
                    let exit = event.exit_code.unwrap_or(1);
                    self.push_event(EventLine {
                        timestamp: timestamp.clone(),
                        icon: '\u{2717}',
                        message: format!("{} FAILED (exit {})", id, exit),
                    });
                }
            }
            "job_skipped" => {
                if let Some(id) = &event.job_id {
                    if self.job_counts.pending > 0 {
                        self.job_counts.pending -= 1;
                    }
                    self.job_counts.completed += 1;
                    self.job_counts.cached += 1;
                    self.push_event(EventLine {
                        timestamp: timestamp.clone(),
                        icon: '\u{2500}',
                        message: format!("{} completed (cached)", id),
                    });
                }
            }
            _ => {} // Ignore unknown event types
        }

        Ok(())
    }

    /// Push an event, keeping only the most recent 100.
    fn push_event(&mut self, event: EventLine) {
        self.recent_events.insert(0, event);
        self.recent_events.truncate(100);
    }
}

// ---------------------------------------------------------------------------
// NDJSON event
// ---------------------------------------------------------------------------

/// An event parsed from NDJSON input (piped from `ox run --json`).
#[derive(Debug, Deserialize)]
struct NdjsonEvent {
    /// Event type: job_started, job_completed, job_failed, job_skipped.
    #[serde(rename = "type")]
    event_type: String,
    /// Job identifier.
    job_id: Option<String>,
    /// Rule name.
    rule: Option<String>,
    /// Wildcard bindings.
    wildcards: Option<String>,
    /// Duration in milliseconds (for completed events).
    duration_ms: Option<u64>,
    /// Exit code (for failed events).
    exit_code: Option<i32>,
}

/// Return a simple HH:MM:SS timestamp string.
fn chrono_timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let secs = now % 60;
    let mins = (now / 60) % 60;
    let hours = (now / 3600) % 24;
    format!("{:02}:{:02}:{:02}", hours, mins, secs)
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_does_not_panic() {
        let app = App::new();
        assert!(app.running);
        assert!(app.pipeline_stats.is_empty());
    }

    #[test]
    fn mock_data_does_not_panic() {
        let app = App::with_mock_data();
        assert!(app.running);
        assert!(!app.pipeline_stats.is_empty());
        assert!(!app.running_jobs.is_empty());
        assert!(!app.recent_events.is_empty());
    }

    #[test]
    fn panel_navigation_wraps() {
        let mut app = App::new();
        assert_eq!(app.selected_panel, Panel::Pipeline);

        app.next_panel();
        assert_eq!(app.selected_panel, Panel::RunningJobs);
        app.next_panel();
        assert_eq!(app.selected_panel, Panel::Events);
        app.next_panel();
        assert_eq!(app.selected_panel, Panel::Pipeline);

        app.prev_panel();
        assert_eq!(app.selected_panel, Panel::Events);
    }

    #[test]
    fn stage_stats_progress() {
        let s = StageStats {
            name: "data".into(),
            completed: 3,
            total: 3,
            running: 0,
        };
        assert!((s.progress_fraction() - 1.0).abs() < f64::EPSILON);
        assert_eq!(s.status_label(), "done");

        let s2 = StageStats {
            name: "features".into(),
            completed: 50,
            total: 100,
            running: 10,
        };
        assert!((s2.progress_fraction() - 0.5).abs() < f64::EPSILON);
        assert_eq!(s2.status_label(), "10 running");

        let s3 = StageStats {
            name: "call".into(),
            completed: 0,
            total: 100,
            running: 0,
        };
        assert_eq!(s3.status_label(), "waiting");
    }

    #[test]
    fn job_counts_aggregation() {
        let c = JobCounts {
            pending: 10,
            running: 2,
            completed: 8, // 5 executed + 3 cached
            failed: 1,
            skipped: 0,
            cached: 3,
        };
        assert_eq!(c.total(), 21);
        assert_eq!(c.done(), 9);
        let expected = 9.0 / 21.0;
        assert!((c.progress_fraction() - expected).abs() < 1e-10);
    }

    #[test]
    fn job_counts_zero_total() {
        let c = JobCounts::default();
        assert_eq!(c.total(), 0);
        assert!((c.progress_fraction() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn refresh_from_db_with_real_data() {
        use ox_state::db::{JobRecord, StateDb};
        use tempfile::NamedTempFile;

        let tmp = NamedTempFile::new().unwrap();
        let db = StateDb::open(tmp.path()).unwrap();

        // Create a session.
        let sid = db.create_session(42, "test-host", Some("human")).unwrap();

        // Register jobs across two rules.
        let jobs: Vec<JobRecord> = vec![
            JobRecord {
                id: "build/a".into(),
                rule_name: "build".into(),
                wildcards: r#"{"x":"a"}"#.into(),
                cache_key: None,
                run_id: None,
            },
            JobRecord {
                id: "build/b".into(),
                rule_name: "build".into(),
                wildcards: r#"{"x":"b"}"#.into(),
                cache_key: None,
                run_id: None,
            },
            JobRecord {
                id: "test/a".into(),
                rule_name: "test".into(),
                wildcards: r#"{"x":"a"}"#.into(),
                cache_key: None,
                run_id: None,
            },
        ];
        db.register_jobs(&jobs).unwrap();

        // Transition some jobs.
        db.claim_job("build/a", &sid).unwrap();
        db.complete_job("build/a", &sid, 0, "{}").unwrap();
        db.claim_job("build/b", &sid).unwrap(); // running
        db.skip_job("test/a").unwrap();

        // Refresh and verify.
        let mut app = App::new();
        app.refresh_from_db(&db).unwrap();

        assert_eq!(app.job_counts.completed, 2); // 1 executed + 1 cached
        assert_eq!(app.job_counts.running, 1);
        assert_eq!(app.job_counts.cached, 1);
        assert_eq!(app.job_counts.skipped, 0);
        assert_eq!(app.job_counts.pending, 0);
        assert_eq!(app.running_jobs.len(), 1);
        assert_eq!(app.running_jobs[0].id, "build/b");
        assert_eq!(app.sessions.len(), 1);
        assert_eq!(app.sessions[0].pid, 42);
        assert!(!app.pipeline_stats.is_empty());

        // Verify pipeline stats contain both rules.
        let rule_names: Vec<&str> = app.pipeline_stats.iter().map(|s| s.name.as_str()).collect();
        assert!(rule_names.contains(&"build"));
        assert!(rule_names.contains(&"test"));
    }

    #[test]
    fn ndjson_job_started() {
        let mut app = App::new();
        app.job_counts.pending = 5;

        let line = r#"{"type":"job_started","job_id":"build/a","rule":"build","wildcards":"{}"}"#;
        app.apply_ndjson_event(line).unwrap();

        assert_eq!(app.running_jobs.len(), 1);
        assert_eq!(app.running_jobs[0].id, "build/a");
        assert_eq!(app.job_counts.running, 1);
        assert_eq!(app.job_counts.pending, 4);
        assert_eq!(app.recent_events.len(), 1);
        assert!(app.recent_events[0].message.contains("started"));
    }

    #[test]
    fn ndjson_job_completed() {
        let mut app = App::new();
        app.job_counts.running = 1;
        app.running_jobs.push(RunningJob {
            id: "build/a".into(),
            rule: "build".into(),
            wildcards: "{}".into(),
            duration: Duration::from_secs(10),
            resources: String::new(),
        });

        let line = r#"{"type":"job_completed","job_id":"build/a","duration_ms":10000}"#;
        app.apply_ndjson_event(line).unwrap();

        assert!(app.running_jobs.is_empty());
        assert_eq!(app.job_counts.running, 0);
        assert_eq!(app.job_counts.completed, 1);
        assert!(app.recent_events[0].message.contains("completed"));
    }

    #[test]
    fn ndjson_job_failed() {
        let mut app = App::new();
        app.job_counts.running = 1;
        app.running_jobs.push(RunningJob {
            id: "build/a".into(),
            rule: "build".into(),
            wildcards: "{}".into(),
            duration: Duration::ZERO,
            resources: String::new(),
        });

        let line = r#"{"type":"job_failed","job_id":"build/a","exit_code":2}"#;
        app.apply_ndjson_event(line).unwrap();

        assert!(app.running_jobs.is_empty());
        assert_eq!(app.job_counts.failed, 1);
        assert!(app.recent_events[0].message.contains("FAILED"));
        assert!(app.recent_events[0].message.contains("exit 2"));
    }

    #[test]
    fn ndjson_job_skipped() {
        let mut app = App::new();
        app.job_counts.pending = 3;

        let line = r#"{"type":"job_skipped","job_id":"test/a"}"#;
        app.apply_ndjson_event(line).unwrap();

        assert_eq!(app.job_counts.completed, 1);
        assert_eq!(app.job_counts.cached, 1);
        assert_eq!(app.job_counts.pending, 2);
        assert!(app.recent_events[0].message.contains("cached"));
    }

    #[test]
    fn ndjson_unknown_type_is_ignored() {
        let mut app = App::new();
        let line = r#"{"type":"unknown","job_id":"x"}"#;
        app.apply_ndjson_event(line).unwrap();
        assert!(app.recent_events.is_empty());
    }

    #[test]
    fn ndjson_invalid_json_returns_error() {
        let mut app = App::new();
        assert!(app.apply_ndjson_event("not json").is_err());
    }

    #[test]
    fn ndjson_sequence_of_events() {
        let mut app = App::new();
        app.job_counts.pending = 3;

        // Start two jobs
        app.apply_ndjson_event(r#"{"type":"job_started","job_id":"j1","rule":"r1"}"#)
            .unwrap();
        app.apply_ndjson_event(r#"{"type":"job_started","job_id":"j2","rule":"r2"}"#)
            .unwrap();
        assert_eq!(app.running_jobs.len(), 2);
        assert_eq!(app.job_counts.running, 2);
        assert_eq!(app.job_counts.pending, 1);

        // Complete one
        app.apply_ndjson_event(r#"{"type":"job_completed","job_id":"j1","duration_ms":5000}"#)
            .unwrap();
        assert_eq!(app.running_jobs.len(), 1);
        assert_eq!(app.job_counts.running, 1);
        assert_eq!(app.job_counts.completed, 1);

        // Fail the other
        app.apply_ndjson_event(r#"{"type":"job_failed","job_id":"j2","exit_code":1}"#)
            .unwrap();
        assert!(app.running_jobs.is_empty());
        assert_eq!(app.job_counts.running, 0);
        assert_eq!(app.job_counts.failed, 1);

        // Skip one (cached — counts as completed)
        app.apply_ndjson_event(r#"{"type":"job_skipped","job_id":"j3"}"#)
            .unwrap();
        assert_eq!(app.job_counts.completed, 2); // j1 + j3 (cached)
        assert_eq!(app.job_counts.cached, 1);
        assert_eq!(app.job_counts.pending, 0);

        // Events list should have 5 entries (most recent first).
        assert_eq!(app.recent_events.len(), 5);
    }
}
