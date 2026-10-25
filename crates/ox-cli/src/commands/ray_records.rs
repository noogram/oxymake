//! Synchronisation of a Ray DAG run's per-task records (written by the
//! driver, see [`ox_exec_ray::task_record`]) into state.db, job history and
//! the run's event log. Shared by `ox status` and `ox run --follow`.
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ox_core::event::EventBus;
use ox_core::job_graph::JobGraph;
use ox_core::model::{Event, JobId, OutputRef};
use ox_core::traits::executor::JobStatus;
use ox_exec_ray::task_record::{self, TaskRecord, TaskStatus};
use ox_state::db::StateDb;

/// How the synchronising process may move a job's row to a terminal state.
#[derive(Clone, Copy)]
pub(super) enum Owner {
    /// The `ox run --follow` session that submitted the run: claim and
    /// terminalise under that session, as the scheduler does.
    Session,
    /// Any other process (`ox status`): session-agnostic reconciliation.
    Reconcile,
}

/// Directory holding `run_id`'s task records under a runs root
/// (`.oxymake/runs`).
pub(super) fn tasks_dir(runs_root: &Path, run_id: &str) -> PathBuf {
    runs_root.join(run_id).join(task_record::TASKS_DIR)
}

/// Job status a record stands for.
pub(super) fn job_status(record: &TaskRecord) -> JobStatus {
    match record.status {
        TaskStatus::Completed => JobStatus::Completed,
        TaskStatus::Failed => JobStatus::Failed(failure_message(record)),
    }
}

fn failure_message(record: &TaskRecord) -> String {
    match (record.exit_code, &record.error) {
        (Some(code), _) => format!("exit code {code}"),
        (None, Some(error)) => error.clone(),
        (None, None) => "task failed".into(),
    }
}

/// The event a record stands for. `node` is the task's host name (the Ray
/// node ID stays in the record).
fn event_for(record: &TaskRecord, outputs: Vec<String>) -> Event {
    let job_id = JobId::from(record.job_id.as_str());
    match record.status {
        TaskStatus::Completed => Event::JobCompleted {
            job_id,
            duration_ms: record.wall_time_ms,
            outputs,
            peak_memory_bytes: record.peak_rss_bytes,
            cpu_time_ms: record.cpu_time_ms,
            node: record.hostname.clone().or_else(|| record.node_id.clone()),
        },
        TaskStatus::Failed => Event::JobFailed {
            job_id,
            error_message: failure_message(record),
            exit_code: record.exit_code,
            stderr_tail: record.error.clone(),
        },
    }
}

/// Apply `records` to `run_id`'s rows. Each record whose job is still
/// pending or running is terminalised, given the task's own start and end,
/// and written to job history. Returns one event per job this call
/// terminalised: the terminal transition is a single conditional UPDATE, so
/// across concurrent `ox status` and `ox run --follow` processes each job
/// yields exactly one event and one history row.
///
/// History follows the existing rule: usage is recorded for completed jobs
/// only; a failed task's usage stays in its record.
pub(super) fn apply(
    db: &StateDb,
    run_id: &str,
    session: &str,
    owner: Owner,
    records: &[TaskRecord],
    outputs: &dyn Fn(&str) -> Vec<String>,
) -> Vec<Event> {
    let mut events = Vec::new();
    let mut synced = Vec::new();
    let mut wall_times = HashMap::new();
    let mut peak_memory = HashMap::new();
    let mut cpu_times = HashMap::new();
    let mut nodes = HashMap::new();
    for record in records {
        let job = record.job_id.as_str();
        let exit_code = record.exit_code_or_default();
        let _ = db.claim_job(job, session);
        let terminalised = match (owner, record.status) {
            (Owner::Session, TaskStatus::Completed) => db.complete_job(job, session, exit_code, ""),
            (Owner::Session, TaskStatus::Failed) => db.fail_job(job, session, exit_code),
            (Owner::Reconcile, TaskStatus::Completed) => {
                db.reconcile_complete_job(job, exit_code, "")
            }
            (Owner::Reconcile, TaskStatus::Failed) => db.reconcile_fail_job(job, exit_code),
        };
        if !matches!(terminalised, Ok(true)) {
            continue;
        }
        let _ = db.set_job_times(job, record.started_at as u64, record.ended_at as u64);
        let event = event_for(record, outputs(job));
        if let Event::JobCompleted {
            duration_ms,
            peak_memory_bytes,
            cpu_time_ms,
            node,
            ..
        } = &event
        {
            wall_times.insert(job.to_string(), *duration_ms);
            if let Some(bytes) = peak_memory_bytes {
                peak_memory.insert(job.to_string(), *bytes);
            }
            if let Some(ms) = cpu_time_ms {
                cpu_times.insert(job.to_string(), *ms);
            }
            if let Some(node) = node {
                nodes.insert(job.to_string(), node.clone());
            }
        }
        synced.push(job.to_string());
        events.push(event);
    }
    let _ = db.record_synced_job_history(
        run_id,
        &synced,
        "ray",
        ox_state::host::hostname(),
        &wall_times,
        &peak_memory,
        &cpu_times,
        &nodes,
    );
    events
}

/// Append `events` to `run_id`'s default event log, when it exists. The log
/// is opened in append mode, as the run's own reporter writes it.
pub(super) fn append_to_event_log(events_dir: &Path, run_id: &str, events: &[Event]) {
    use ox_core::traits::reporter::Reporter;
    if events.is_empty() {
        return;
    }
    let path = events_dir.join(format!("{run_id}.ndjson"));
    let Ok(file) = std::fs::OpenOptions::new().append(true).open(path) else {
        return;
    };
    let Ok(rt) = tokio::runtime::Builder::new_current_thread().build() else {
        return;
    };
    let reporter = ox_report_json::reporter::JsonReporter::new(file);
    rt.block_on(async {
        for event in events {
            reporter.on_event(event).await;
        }
    });
}

/// Per-job polling for `ox run --follow`: a job whose record exists is
/// finished, whatever the driver's state; its record is applied once and its
/// event emitted on the run's bus (event log, reporters, history).
pub(super) struct Follower<'a> {
    tasks_dir: PathBuf,
    run_id: &'a str,
    db: Option<&'a StateDb>,
    session: Option<&'a str>,
    graph: &'a JobGraph,
    bus: &'a EventBus,
    finished: RefCell<HashMap<String, JobStatus>>,
}

impl<'a> Follower<'a> {
    pub(super) fn new(
        tasks_dir: PathBuf,
        run_id: &'a str,
        db: Option<&'a StateDb>,
        session: Option<&'a str>,
        graph: &'a JobGraph,
        bus: &'a EventBus,
    ) -> Self {
        Self {
            tasks_dir,
            run_id,
            db,
            session,
            graph,
            bus,
            finished: RefCell::new(HashMap::new()),
        }
    }

    /// The job's status from its record, or `None` while it has none.
    pub(super) fn poll(&self, job: &str) -> Option<JobStatus> {
        if let Some(status) = self.finished.borrow().get(job) {
            return Some(status.clone());
        }
        let record = task_record::read(&self.tasks_dir, job)?;
        let outputs = |id: &str| self.outputs(id);
        let events = match (self.db, self.session) {
            (Some(db), Some(session)) => apply(
                db,
                self.run_id,
                session,
                Owner::Session,
                std::slice::from_ref(&record),
                &outputs,
            ),
            _ => vec![event_for(&record, outputs(job))],
        };
        for event in events {
            self.bus.emit(event);
        }
        let status = job_status(&record);
        self.finished
            .borrow_mut()
            .insert(job.to_string(), status.clone());
        Some(status)
    }

    /// Declared outputs, as the scheduler reports them in `job_completed`.
    fn outputs(&self, job: &str) -> Vec<String> {
        let Some(job) = self.graph.get_job(&JobId::from(job)) else {
            return Vec::new();
        };
        job.outputs
            .iter()
            .map(|output| match &output.reference {
                OutputRef::File(path) => path.display().to_string(),
                OutputRef::Virtual { id, .. } => id.clone(),
                OutputRef::InMemory { type_hint } => {
                    type_hint.clone().unwrap_or_else(|| "<memory>".into())
                }
            })
            .collect()
    }
}
