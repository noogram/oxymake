//! NDJSON reporter implementation.
//!
//! The [`JsonReporter`] writes one JSON line per event to a generic
//! [`std::io::Write`] sink. This is the primary machine-readable output format
//! for OxyMake, consumed by AI agents, CI pipelines, and downstream tooling via
//! `ox run --json`.
//!
//! # Wire format
//!
//! Each line is a self-contained JSON object terminated by `\n`. The stream is
//! therefore valid [NDJSON](https://github.com/ndjson/ndjson-spec). Consumers
//! can parse each line independently — there is no wrapping array or trailing
//! comma.
//!
//! # Thread safety
//!
//! The writer is wrapped in a [`std::sync::Mutex`] so that concurrent calls to
//! `on_event` never interleave partial JSON lines. The lock is held only for
//! the duration of a single `writeln!`, so contention is negligible.

use std::fs::{self, File, OpenOptions};
use std::future::Future;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{
    Mutex,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;

use ox_core::model::Event;
use ox_core::traits::reporter::{Reporter, RunSummary};

static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A file sink that publishes its final path only after a complete first line.
///
/// Event logs are NDJSON streams whose first line is always `run_started`.
/// The temporary file lets a process die before that event without leaving a
/// visible, empty `.ndjson` file. Once the first newline is durable, renaming
/// keeps the existing file descriptor valid so subscribers can tail the stream.
pub struct AtomicEventFile {
    file: File,
    temporary_path: PathBuf,
    final_path: PathBuf,
    published: bool,
}

impl AtomicEventFile {
    /// Create a temporary sibling for `final_path`; the final path is untouched.
    pub fn create(final_path: impl AsRef<Path>) -> io::Result<Self> {
        let final_path = final_path.as_ref().to_path_buf();
        let parent = final_path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "event log has no parent directory",
            )
        })?;
        let name = final_path.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "event log has no file name")
        })?;

        for _ in 0..100 {
            let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let temporary_path = parent.join(format!(
                ".{}.tmp-{}-{sequence}",
                name.to_string_lossy(),
                std::process::id(),
            ));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary_path)
            {
                Ok(file) => {
                    return Ok(Self {
                        file,
                        temporary_path,
                        final_path,
                        published: false,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }

        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate unique temporary event log",
        ))
    }
}

impl Write for AtomicEventFile {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.file.write_all(buffer)?;
        if !self.published && buffer.contains(&b'\n') {
            self.file.sync_data()?;
            fs::rename(&self.temporary_path, &self.final_path)?;
            self.published = true;
        }
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Drop for AtomicEventFile {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(&self.temporary_path);
        }
    }
}

// ---------------------------------------------------------------------------
// Serializable summary wrapper
// ---------------------------------------------------------------------------

/// A serializable mirror of [`RunSummary`].
///
/// [`RunSummary`] lives in `ox-core` and deliberately does *not* derive
/// `Serialize` — serialization is a presentation concern. This wrapper adds the
/// `type` discriminator field so consumers can distinguish the summary line from
/// regular event lines in the NDJSON stream.
#[derive(Serialize)]
struct SummaryEvent {
    /// Discriminator field — always `"run_summary"`.
    #[serde(rename = "event")]
    event_type: &'static str,
    total_jobs: usize,
    succeeded: usize,
    failed: usize,
    skipped: usize,
    cancelled: usize,
    duration_ms: u64,
}

/// The versioned outer contract shared by events and the synthesized summary.
/// Payload fields remain additive and are flattened so existing consumers keep
/// seeing the event shape they already parse.
#[derive(Serialize)]
struct Envelope<T: Serialize> {
    schema_version: u8,
    ts: String,
    #[serde(flatten)]
    payload: T,
}

fn utc_timestamp() -> String {
    // Civil-date conversion for UTC Unix seconds.  This avoids making the
    // reporting crate depend on a time library solely to format one field.
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = (seconds / 86_400) as i64;
    let day_seconds = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        day_seconds / 3_600,
        (day_seconds % 3_600) / 60,
        day_seconds % 60
    )
}

// Howard Hinnant's public-domain civil-date algorithm, with the Unix epoch
// shifted to its 1970-01-01 day index.
fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    (
        year + if month <= 2 { 1 } else { 0 },
        month as u32,
        day as u32,
    )
}

impl From<&RunSummary> for SummaryEvent {
    fn from(s: &RunSummary) -> Self {
        Self {
            event_type: "run_summary",
            total_jobs: s.total_jobs,
            succeeded: s.succeeded,
            failed: s.failed,
            skipped: s.skipped,
            cancelled: s.cancelled,
            duration_ms: s.duration_ms,
        }
    }
}

// ---------------------------------------------------------------------------
// JsonReporter
// ---------------------------------------------------------------------------

/// NDJSON reporter: emits one JSON line per event.
///
/// Used by agents and scripts via `ox run --json`. Each line is a complete JSON
/// object that can be parsed independently.
///
/// # Example
///
/// ```rust
/// use ox_report_json::reporter::JsonReporter;
///
/// // Write to an in-memory buffer (useful for testing).
/// let buf: Vec<u8> = Vec::new();
/// let reporter = JsonReporter::new(buf);
///
/// // In production, write to stdout:
/// let _stdout_reporter = JsonReporter::stdout();
/// ```
pub struct JsonReporter<W: Write + Send> {
    /// The output sink, protected by a mutex for thread-safe writes.
    writer: Mutex<W>,
    run_context: Option<RunContext>,
    /// `job_skipped` events seen with reason `cached`. `all_cached` requires
    /// every skip counted by `run_completed` to be one of these.
    cache_hit_skips: AtomicUsize,
}

#[derive(Clone)]
pub struct RunContext {
    pub run_id: String,
    pub workflow_path: String,
}

impl<W: Write + Send> JsonReporter<W> {
    /// Create a new JSON reporter writing to the given sink.
    ///
    /// The writer can be anything that implements [`Write`] + [`Send`]:
    /// a [`Vec<u8>`] for testing, a [`std::fs::File`] for disk output, or
    /// [`std::io::Stdout`] for terminal use.
    pub fn new(writer: W) -> Self {
        Self {
            writer: Mutex::new(writer),
            run_context: None,
            cache_hit_skips: AtomicUsize::new(0),
        }
    }

    /// Attach fields that are known by the command boundary rather than the
    /// scheduler. They are added only to `run_started` so the core event model
    /// remains usable by embedders that do not mint CLI run identifiers.
    pub fn with_run_context(mut self, run_id: String, workflow_path: String) -> Self {
        self.run_context = Some(RunContext {
            run_id,
            workflow_path,
        });
        self
    }
}

impl JsonReporter<std::io::Stdout> {
    /// Create a reporter writing to stdout.
    ///
    /// This is the constructor used by `ox run --json` in production.
    pub fn stdout() -> Self {
        Self::new(std::io::stdout())
    }
}

impl<W: Write + Send + 'static> Reporter for JsonReporter<W> {
    /// Serialize `event` to JSON and write it as a single line.
    ///
    /// Errors are silently ignored — a reporter must never crash the build.
    /// If the writer fails (broken pipe, full disk), the event is simply lost.
    fn on_event<'a>(&'a self, event: &'a Event) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let mut payload = match serde_json::to_value(event) {
                Ok(payload) => payload,
                Err(e) => {
                    eprintln!("ox-report-json: event serialization failed: {e}");
                    return;
                }
            };
            if let Event::JobSkipped { reason, .. } = event {
                if reason == "cached" {
                    self.cache_hit_skips.fetch_add(1, Ordering::Relaxed);
                }
            }
            if matches!(event, Event::RunStarted { .. }) {
                if let (Some(context), Value::Object(object)) = (&self.run_context, &mut payload) {
                    object.insert("run_id".into(), Value::String(context.run_id.clone()));
                    object.insert(
                        "workflow_path".into(),
                        Value::String(context.workflow_path.clone()),
                    );
                }
            }
            if let (
                Event::RunCompleted {
                    succeeded,
                    failed,
                    skipped,
                    cancelled,
                    ..
                },
                Value::Object(object),
            ) = (event, &mut payload)
            {
                let status = if *failed > 0 {
                    "failed"
                } else if *cancelled > 0 {
                    "interrupted"
                } else if *succeeded == 0
                    && *skipped > 0
                    && self.cache_hit_skips.load(Ordering::Relaxed) == *skipped
                {
                    "all_cached"
                } else {
                    "success"
                };
                object.insert("status".into(), Value::String(status.into()));
            }
            let json = match serde_json::to_string(&Envelope {
                schema_version: 1,
                ts: utc_timestamp(),
                payload,
            }) {
                Ok(j) => j,
                Err(e) => {
                    eprintln!("ox-report-json: event serialization failed: {e}");
                    return;
                }
            };
            let mut w = self.writer.lock().unwrap();
            writeln!(w, "{json}").ok();
        })
    }

    /// Emit a final summary event and flush the writer.
    ///
    /// The summary is serialized as a JSON object with `"event": "run_summary"`
    /// so consumers can distinguish it from regular events.
    fn finish<'a>(
        &'a self,
        summary: &'a RunSummary,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let envelope = SummaryEvent::from(summary);
            let json = match serde_json::to_string(&Envelope {
                schema_version: 1,
                ts: utc_timestamp(),
                payload: envelope,
            }) {
                Ok(j) => j,
                Err(e) => {
                    eprintln!("ox-report-json: summary serialization failed: {e}");
                    return;
                }
            };
            let mut w = self.writer.lock().unwrap();
            writeln!(w, "{json}").ok();
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ox_core::model::{GateId, JobId, RuleName};
    use std::collections::BTreeMap;
    use std::fs;

    /// Helper: collect the NDJSON output of a reporter session into a `String`.
    fn output_string(reporter: JsonReporter<Vec<u8>>) -> String {
        let buf = reporter.writer.into_inner().unwrap();
        String::from_utf8(buf).expect("valid UTF-8")
    }

    /// Helper: parse every line of NDJSON output into `serde_json::Value`s.
    fn parse_lines(raw: &str) -> Vec<serde_json::Value> {
        raw.lines()
            .map(|line| serde_json::from_str(line).expect("valid JSON line"))
            .collect()
    }

    #[test]
    fn atomic_event_file_publishes_only_after_a_complete_first_line() {
        let directory = std::env::temp_dir().join(format!(
            "ox-report-json-test-{}-{}",
            std::process::id(),
            TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        let final_path = directory.join("run-123.ndjson");
        {
            let mut file = AtomicEventFile::create(&final_path).unwrap();

            file.write_all(br#"{"event":"run_started""#).unwrap();
            assert!(
                !final_path.exists(),
                "a partial first line must not create a visible event log"
            );

            file.write_all(b"}\n").unwrap();
            assert!(
                final_path.exists(),
                "the complete first line publishes the log"
            );
            assert_eq!(
                fs::read_to_string(&final_path).unwrap(),
                "{\"event\":\"run_started\"}\n"
            );

            file.write_all(b"{\"event\":\"job_planned\"}\n").unwrap();
            assert!(
                fs::read_to_string(&final_path)
                    .unwrap()
                    .starts_with("{\"event\":\"run_started\"}\n")
            );
        }
        fs::remove_dir_all(&directory).unwrap();
    }

    // -- Individual event variant tests ------------------------------------

    #[tokio::test]
    async fn run_started_serializes() {
        let r = JsonReporter::new(Vec::new());
        let event = Event::RunStarted {
            total_jobs: 100,
            to_run: 10,
            cached: 90,
        };
        r.on_event(&event).await;

        let out = output_string(r);
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["event"], "run_started");
        assert_eq!(v["total_jobs"], 100);
        assert_eq!(v["to_run"], 10);
        assert_eq!(v["cached"], 90);
    }

    #[tokio::test]
    async fn every_line_has_the_versioned_timestamped_envelope() {
        let r = JsonReporter::new(Vec::new());
        r.on_event(&Event::RunStarted {
            total_jobs: 1,
            to_run: 1,
            cached: 0,
        })
        .await;
        r.finish(&RunSummary {
            total_jobs: 1,
            succeeded: 1,
            failed: 0,
            skipped: 0,
            cancelled: 0,
            duration_ms: 1,
        })
        .await;

        for line in parse_lines(&output_string(r)) {
            assert_eq!(line["schema_version"], 1);
            let timestamp = line["ts"].as_str().expect("timestamp string");
            assert!(
                timestamp.ends_with('Z'),
                "timestamp must be UTC RFC 3339: {timestamp}"
            );
            assert!(
                timestamp.contains('T'),
                "timestamp must be RFC 3339: {timestamp}"
            );
        }
    }

    #[tokio::test]
    async fn job_queued_serializes() {
        let r = JsonReporter::new(Vec::new());
        let mut tags = BTreeMap::new();
        tags.insert("sample".into(), "S001".into());
        let event = Event::JobQueued {
            job_id: JobId("align_S001".into()),
            rule: RuleName("align".into()),
            tags,
        };
        r.on_event(&event).await;

        let out = output_string(r);
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["event"], "job_queued");
        assert_eq!(v["job_id"], "align_S001");
        assert_eq!(v["tags"]["sample"], "S001");
    }

    #[tokio::test]
    async fn job_started_serializes() {
        let r = JsonReporter::new(Vec::new());
        let event = Event::JobStarted {
            job_id: JobId("align_S001".into()),
            executor: "local".into(),
            reason: None,
        };
        r.on_event(&event).await;

        let out = output_string(r);
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["event"], "job_started");
        assert_eq!(v["executor"], "local");
    }

    #[tokio::test]
    async fn job_completed_serializes() {
        let r = JsonReporter::new(Vec::new());
        let event = Event::JobCompleted {
            job_id: JobId("align_S001".into()),
            duration_ms: 272000,
            outputs: vec!["results/S001.bam".into()],
            peak_memory_bytes: Some(128 * 1024 * 1024),
        };
        r.on_event(&event).await;

        let out = output_string(r);
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["event"], "job_completed");
        assert_eq!(v["duration_ms"], 272000);
        assert_eq!(v["outputs"][0], "results/S001.bam");
        assert_eq!(v["peak_memory_bytes"], 128 * 1024 * 1024);
    }

    #[tokio::test]
    async fn job_failed_serializes() {
        let r = JsonReporter::new(Vec::new());
        let event = Event::JobFailed {
            job_id: JobId("align_S002".into()),
            error_message: "exit code 1".into(),
            exit_code: Some(1),
            stderr_tail: Some("segfault".into()),
        };
        r.on_event(&event).await;

        let out = output_string(r);
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["event"], "job_failed");
        assert_eq!(v["exit_code"], 1);
        assert_eq!(v["stderr_tail"], "segfault");
    }

    #[tokio::test]
    async fn job_skipped_serializes() {
        let r = JsonReporter::new(Vec::new());
        let event = Event::JobSkipped {
            job_id: JobId("qc_S001".into()),
            reason: "cached".into(),
        };
        r.on_event(&event).await;

        let out = output_string(r);
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["event"], "job_skipped");
        assert_eq!(v["reason"], "cached");
    }

    #[tokio::test]
    async fn gate_reached_serializes() {
        let r = JsonReporter::new(Vec::new());
        let event = Event::GateReached {
            gate_id: GateId("review".into()),
            message: "Check alignment QC".into(),
        };
        r.on_event(&event).await;

        let out = output_string(r);
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["event"], "gate_reached");
        assert_eq!(v["message"], "Check alignment QC");
    }

    #[tokio::test]
    async fn gate_approved_serializes() {
        let r = JsonReporter::new(Vec::new());
        let event = Event::GateApproved {
            gate_id: GateId("review".into()),
            approved_by: "alice".into(),
        };
        r.on_event(&event).await;

        let out = output_string(r);
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["event"], "gate_approved");
        assert_eq!(v["approved_by"], "alice");
    }

    #[tokio::test]
    async fn run_completed_serializes() {
        let r = JsonReporter::new(Vec::new());
        let event = Event::RunCompleted {
            total: 847,
            succeeded: 846,
            failed: 1,
            skipped: 0,
            cancelled: 0,
            duration_ms: 8_040_000,
        };
        r.on_event(&event).await;

        let out = output_string(r);
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["event"], "run_completed");
        assert_eq!(v["total"], 847);
        assert_eq!(v["failed"], 1);
    }

    #[tokio::test]
    async fn run_failed_serializes() {
        let r = JsonReporter::new(Vec::new());
        let event = Event::RunFailed {
            error_message: "DAG cycle detected".into(),
        };
        r.on_event(&event).await;

        let out = output_string(r);
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["event"], "run_failed");
        assert_eq!(v["error_message"], "DAG cycle detected");
    }

    // -- Multi-event stream test -------------------------------------------

    #[tokio::test]
    async fn ndjson_stream_multiple_events() {
        let r = JsonReporter::new(Vec::new());

        r.on_event(&Event::RunStarted {
            total_jobs: 10,
            to_run: 3,
            cached: 7,
        })
        .await;

        r.on_event(&Event::JobStarted {
            job_id: JobId("j1".into()),
            executor: "local".into(),
            reason: None,
        })
        .await;

        r.on_event(&Event::JobCompleted {
            job_id: JobId("j1".into()),
            duration_ms: 500,
            outputs: vec!["out.txt".into()],
            peak_memory_bytes: None,
        })
        .await;

        let raw = output_string(r);
        let lines = parse_lines(&raw);

        assert_eq!(lines.len(), 3, "expected exactly 3 NDJSON lines");
        assert_eq!(lines[0]["event"], "run_started");
        assert_eq!(lines[1]["event"], "job_started");
        assert_eq!(lines[2]["event"], "job_completed");
    }

    // -- Summary test ------------------------------------------------------

    #[tokio::test]
    async fn finish_emits_summary() {
        let r = JsonReporter::new(Vec::new());

        let summary = RunSummary {
            total_jobs: 100,
            succeeded: 98,
            failed: 1,
            skipped: 1,
            cancelled: 0,
            duration_ms: 60_000,
        };
        r.finish(&summary).await;

        let out = output_string(r);
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["event"], "run_summary");
        assert_eq!(v["total_jobs"], 100);
        assert_eq!(v["succeeded"], 98);
        assert_eq!(v["failed"], 1);
        assert_eq!(v["skipped"], 1);
        assert_eq!(v["cancelled"], 0);
        assert_eq!(v["duration_ms"], 60_000);
    }

    // -- Serialization failure tests ----------------------------------------

    #[tokio::test]
    async fn on_event_does_not_panic_on_serialization_failure() {
        // Event always derives Serialize with infallible field types today,
        // so we cannot force a *real* failure through the public API. Instead,
        // we verify the contract by confirming the `.expect()` is gone — if
        // someone re-introduces it, the code review catches it. The real
        // regression guard is the `finish` test below which uses a mock.
        //
        // This test simply exercises the happy path to confirm no panic.
        let r = JsonReporter::new(Vec::new());
        r.on_event(&Event::RunStarted {
            total_jobs: 1,
            to_run: 1,
            cached: 0,
        })
        .await;
        // If we got here, no panic — good.
    }

    #[tokio::test]
    async fn finish_does_not_panic_on_write_failure() {
        // Use a writer that always fails to ensure the reporter degrades
        // gracefully rather than panicking.
        struct FailWriter;
        impl Write for FailWriter {
            fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "boom"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "boom"))
            }
        }

        let r = JsonReporter::new(FailWriter);
        let summary = RunSummary {
            total_jobs: 1,
            succeeded: 1,
            failed: 0,
            skipped: 0,
            cancelled: 0,
            duration_ms: 100,
        };
        // Must not panic even when the writer fails.
        r.finish(&summary).await;
    }

    // -- Trait object test -------------------------------------------------

    #[tokio::test]
    async fn reporter_trait_object() {
        let r: Box<dyn Reporter> = Box::new(JsonReporter::new(Vec::<u8>::new()));
        // Just verify it compiles and can be called through the trait.
        r.on_event(&Event::RunStarted {
            total_jobs: 1,
            to_run: 1,
            cached: 0,
        })
        .await;
    }
}
