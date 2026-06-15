//! Prometheus metric definitions for OxyMake.
//!
//! [`OxMetrics`] owns a [`prometheus::Registry`] populated with gauges,
//! counters, and histograms that mirror the monitoring roadmap's metric
//! definitions (see `docs/design/monitoring-roadmap.md` §4.1).
//!
//! Metrics can be refreshed from a [`StateDb`] snapshot or updated
//! incrementally by an event loop. The [`encode`](OxMetrics::encode)
//! method serialises the registry into Prometheus text exposition format.

use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntGauge, IntGaugeVec, Opts, Registry, TextEncoder,
};

use ox_state::db::StateDb;

// Default duration buckets (seconds): 1s, 5s, 10s, 30s, 60s, 120s, 300s, 600s
const DURATION_BUCKETS: &[f64] = &[1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0];

/// Central metrics container for an OxyMake run.
///
/// All Prometheus metrics are registered in an isolated [`Registry`] so
/// they do not collide with the global default registry.
///
/// # Example
///
/// ```
/// use ox_metrics::metrics::OxMetrics;
///
/// let m = OxMetrics::new();
/// assert_eq!(m.sessions_active.get(), 0);
/// let text = m.encode();
/// assert!(text.contains("oxymake_sessions_active 0"));
/// ```
pub struct OxMetrics {
    /// The Prometheus registry owning all metrics.
    pub registry: Registry,

    /// Job counts by status (`pending`, `running`, `completed`, `failed`,
    /// `skipped`).
    pub jobs_total: IntGaugeVec,

    /// Number of currently active sessions.
    pub sessions_active: IntGauge,

    /// Job duration histogram, labelled by rule name.
    pub job_duration_seconds: HistogramVec,

    /// Total cache hits observed.
    pub cache_hits: IntGauge,

    /// Total cache misses observed.
    pub cache_misses: IntGauge,
}

impl OxMetrics {
    /// Create a new `OxMetrics` with all metrics registered and zeroed.
    pub fn new() -> Self {
        let registry = Registry::new();

        let jobs_total = IntGaugeVec::new(
            Opts::new("oxymake_jobs_total", "Number of jobs by status"),
            &["status"],
        )
        .expect("metric creation should not fail");
        registry
            .register(Box::new(jobs_total.clone()))
            .expect("registration should not fail");

        let sessions_active = IntGauge::new(
            "oxymake_sessions_active",
            "Number of currently active sessions",
        )
        .expect("metric creation should not fail");
        registry
            .register(Box::new(sessions_active.clone()))
            .expect("registration should not fail");

        let job_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "oxymake_job_duration_seconds",
                "Job execution duration in seconds",
            )
            .buckets(DURATION_BUCKETS.to_vec()),
            &["rule"],
        )
        .expect("metric creation should not fail");
        registry
            .register(Box::new(job_duration_seconds.clone()))
            .expect("registration should not fail");

        let cache_hits = IntGauge::new("oxymake_cache_hits_total", "Total cache hits")
            .expect("metric creation should not fail");
        registry
            .register(Box::new(cache_hits.clone()))
            .expect("registration should not fail");

        let cache_misses = IntGauge::new("oxymake_cache_misses_total", "Total cache misses")
            .expect("metric creation should not fail");
        registry
            .register(Box::new(cache_misses.clone()))
            .expect("registration should not fail");

        Self {
            registry,
            jobs_total,
            sessions_active,
            job_duration_seconds,
            cache_hits,
            cache_misses,
        }
    }

    /// Refresh job-count and session-count metrics from `state.db`.
    ///
    /// This performs a read-only query against the database — it never
    /// modifies execution state (consistent with the monitoring-layer
    /// "read-only" principle).
    pub fn refresh_from_db(&self, db: &StateDb) -> Result<(), ox_state::error::StateError> {
        let counts = db.job_counts()?;
        self.jobs_total
            .with_label_values(&["pending"])
            .set(counts.pending as i64);
        self.jobs_total
            .with_label_values(&["running"])
            .set(counts.running as i64);
        self.jobs_total
            .with_label_values(&["completed"])
            .set(counts.completed as i64);
        self.jobs_total
            .with_label_values(&["failed"])
            .set(counts.failed as i64);
        self.jobs_total
            .with_label_values(&["skipped"])
            .set(counts.skipped as i64);

        let sessions = db.active_sessions()?;
        self.sessions_active.set(sessions.len() as i64);

        // Cache hits correspond to cached jobs.
        self.cache_hits.set(counts.cached as i64);
        // Cache misses are jobs that actually ran (completed minus cached, + failed).
        self.cache_misses
            .set((counts.completed - counts.cached + counts.failed) as i64);

        Ok(())
    }

    /// Encode all metrics in Prometheus text exposition format.
    ///
    /// Returns a UTF-8 string suitable for serving on a `/metrics` HTTP
    /// endpoint.
    pub fn encode(&self) -> String {
        let encoder = TextEncoder::new();
        let mut buffer = Vec::new();
        encoder
            .encode(&self.registry.gather(), &mut buffer)
            .expect("encoding to a Vec should not fail");
        String::from_utf8(buffer).expect("Prometheus text format is always valid UTF-8")
    }

    /// Record a job duration observation for a given rule.
    ///
    /// Typically called when a job transitions to `completed` or `failed`.
    pub fn observe_job_duration(&self, rule: &str, duration_secs: f64) {
        self.job_duration_seconds
            .with_label_values(&[rule])
            .observe(duration_secs);
    }
}

impl Default for OxMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ox_state::db::{JobRecord, StateDb};
    use tempfile::NamedTempFile;

    fn temp_db() -> (NamedTempFile, StateDb) {
        let tmp = NamedTempFile::new().unwrap();
        let db = StateDb::open(tmp.path()).unwrap();
        (tmp, db)
    }

    #[test]
    fn initial_values_are_zero() {
        let m = OxMetrics::new();
        assert_eq!(m.jobs_total.with_label_values(&["pending"]).get(), 0);
        assert_eq!(m.jobs_total.with_label_values(&["running"]).get(), 0);
        assert_eq!(m.jobs_total.with_label_values(&["completed"]).get(), 0);
        assert_eq!(m.jobs_total.with_label_values(&["failed"]).get(), 0);
        assert_eq!(m.jobs_total.with_label_values(&["skipped"]).get(), 0);
        assert_eq!(m.sessions_active.get(), 0);
        assert_eq!(m.cache_hits.get(), 0);
        assert_eq!(m.cache_misses.get(), 0);
    }

    #[test]
    fn refresh_from_db_updates_counts() {
        let (_tmp, db) = temp_db();
        let sid = db.create_session(1, "localhost", None).unwrap();

        let jobs: Vec<JobRecord> = (0..5)
            .map(|i| JobRecord {
                id: format!("j{i}"),
                rule_name: "build".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            })
            .collect();
        db.register_jobs(&jobs).unwrap();

        // Transition some jobs.
        db.claim_job("j0", &sid).unwrap();
        db.complete_job("j0", &sid, 0, "{}").unwrap();
        db.claim_job("j1", &sid).unwrap();
        db.fail_job("j1", &sid, 1).unwrap();
        db.skip_job("j2").unwrap();
        db.claim_job("j3", &sid).unwrap();
        // j3 running, j4 pending

        let m = OxMetrics::new();
        m.refresh_from_db(&db).unwrap();

        assert_eq!(m.jobs_total.with_label_values(&["pending"]).get(), 1);
        assert_eq!(m.jobs_total.with_label_values(&["running"]).get(), 1);
        assert_eq!(m.jobs_total.with_label_values(&["completed"]).get(), 2); // j0 + j2 (cached)
        assert_eq!(m.jobs_total.with_label_values(&["failed"]).get(), 1);
        assert_eq!(m.jobs_total.with_label_values(&["skipped"]).get(), 0);
        assert_eq!(m.sessions_active.get(), 1);
        assert_eq!(m.cache_hits.get(), 1);
        assert_eq!(m.cache_misses.get(), 2); // (completed - cached) + failed = (2-1) + 1
    }

    #[test]
    fn encode_produces_valid_prometheus_text() {
        let m = OxMetrics::new();
        m.jobs_total.with_label_values(&["completed"]).set(42);
        m.sessions_active.set(2);

        let text = m.encode();

        // Verify it contains expected metric lines.
        assert!(text.contains("oxymake_jobs_total"));
        assert!(text.contains(r#"status="completed""#));
        assert!(text.contains("42"));
        assert!(text.contains("oxymake_sessions_active 2"));
        assert!(text.contains("oxymake_cache_hits_total"));
        assert!(text.contains("oxymake_cache_misses_total"));

        // Every line should be valid (no panics during encoding).
        // HELP and TYPE lines start with #, data lines are metric values.
        for line in text.lines() {
            assert!(
                line.starts_with('#') || line.starts_with("oxymake_") || line.is_empty(),
                "unexpected line format: {line}"
            );
        }
    }

    #[test]
    fn metric_labels_are_distinct() {
        let m = OxMetrics::new();
        m.jobs_total.with_label_values(&["pending"]).set(10);
        m.jobs_total.with_label_values(&["running"]).set(20);

        assert_eq!(m.jobs_total.with_label_values(&["pending"]).get(), 10);
        assert_eq!(m.jobs_total.with_label_values(&["running"]).get(), 20);
    }

    #[test]
    fn observe_job_duration() {
        let m = OxMetrics::new();
        m.observe_job_duration("features", 12.5);
        m.observe_job_duration("features", 3.0);
        m.observe_job_duration("data", 1.0);

        let text = m.encode();
        assert!(text.contains("oxymake_job_duration_seconds"));
        assert!(text.contains(r#"rule="features""#));
        assert!(text.contains(r#"rule="data""#));
    }
}
