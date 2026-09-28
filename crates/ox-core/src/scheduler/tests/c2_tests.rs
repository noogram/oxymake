use super::*;

// The observer is scoped to each current-thread test runtime. Production builds
// contain neither the observer nor the acquire/release history.
thread_local! {
    static C2_BUDGET: std::cell::RefCell<Option<ResourceBudget>> = const { std::cell::RefCell::new(None) };
}

pub(in crate::scheduler) fn observe(budget: &ResourceBudget) {
    C2_BUDGET.with(|slot| *slot.borrow_mut() = Some(budget.clone()));
}

fn observed_budget() -> ResourceBudget {
    C2_BUDGET.with(|slot| slot.borrow().as_ref().unwrap().clone())
}

fn c2_history(expected_attempts: usize) {
    let budget = observed_budget();
    let inner = budget.inner.lock().unwrap();
    let mut active = 0;
    for (acquire, used) in &inner.history {
        for (key, amount) in used {
            assert!(*amount <= inner.capacity[key], "oversubscribed {key}");
        }
        if *acquire {
            active += 1;
        } else {
            active -= 1;
        }
        assert!(active >= 0, "release without an attempt");
    }
    assert_eq!(active, 0);
    assert_eq!(
        inner.history.iter().filter(|(acquire, _)| *acquire).count(),
        expected_attempts
    );
    assert_eq!(
        inner.history.len(),
        expected_attempts * 2,
        "one release per attempt"
    );
    assert!(inner.in_use.values().all(|used| *used == 0));
}

#[derive(Debug)]
struct C2Executor {
    stages: tokio::sync::mpsc::UnboundedSender<(String, &'static str)>,
    proceed: Semaphore,
    failure: &'static str,
    attempts: AtomicUsize,
    cancelled: Notify,
}

impl C2Executor {
    fn new(
        failure: &'static str,
    ) -> (
        Arc<Self>,
        tokio::sync::mpsc::UnboundedReceiver<(String, &'static str)>,
    ) {
        let (stages, rx) = tokio::sync::mpsc::unbounded_channel();
        (
            Arc::new(Self {
                stages,
                proceed: Semaphore::new(0),
                failure,
                attempts: AtomicUsize::new(0),
                cancelled: Notify::new(),
            }),
            rx,
        )
    }

    async fn stage(&self, job: &str, stage: &'static str) -> Result<(), MockError> {
        self.stages.send((job.into(), stage)).unwrap();
        self.proceed.acquire().await.unwrap().forget();
        if self.failure == stage {
            return Err(MockError(stage.into()));
        }
        if self.failure == "panic" && stage == "execute" {
            panic!("C2 injected unwind");
        }
        Ok(())
    }
}

impl Executor for C2Executor {
    type Error = MockError;
    async fn init(&self) -> Result<(), MockError> {
        Ok(())
    }
    async fn health_check(&self) -> Result<(), MockError> {
        Ok(())
    }
    async fn cleanup(&self) -> Result<(), MockError> {
        Ok(())
    }
    fn capabilities(&self) -> ExecutorCapabilities {
        ExecutorCapabilities::default()
    }
    fn max_concurrency(&self) -> Option<usize> {
        None
    }
    async fn prepare_workspace(
        &self,
        job: &ConcreteJob,
        _: &ExecContext,
    ) -> Result<Workspace, MockError> {
        self.stage(job.id.as_str(), "prepare").await?;
        Ok(Workspace::new(PathBuf::from(job.id.as_str())))
    }
    async fn execute(
        &self,
        job: &ConcreteJob,
        _: &Workspace,
        _: &ExecContext,
    ) -> Result<JobResult, MockError> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        self.stage(job.id.as_str(), "execute").await?;
        Ok(JobResult {
            job_id: job.id.clone(),
            exit_code: i32::from(
                self.failure == "retry" && attempt == 0 || self.failure == "always_retry",
            ),
            duration: Duration::ZERO,
            peak_memory_bytes: None,
            cpu_time: None,
            log_path: None,
            stderr_tail: None,
        })
    }
    async fn finalize_workspace(
        &self,
        workspace: Workspace,
        _: &JobResult,
    ) -> Result<(), MockError> {
        self.stage(workspace.work_dir.to_str().unwrap(), "finalize")
            .await
    }
    async fn cancel(&self, _: &JobId) -> Result<(), MockError> {
        self.cancelled.notify_one();
        Ok(())
    }
    async fn poll_status(&self, _: &JobId) -> Result<JobStatus, MockError> {
        Ok(JobStatus::Running)
    }
    async fn submit_dag(&self, _: &JobGraph, _: &ExecContext) -> Result<DagSubmission, MockError> {
        Err(MockError("unsupported".into()))
    }
}

fn c2_job(id: &str, resource: &str, amount: ResourceValue) -> ConcreteJob {
    let mut job = make_job(id, id, vec![], vec![]);
    job.resources.insert(resource.into(), amount);
    job
}

fn c2_run(
    jobs: Vec<ConcreteJob>,
    executor: Arc<C2Executor>,
    capacity: (&str, u64),
    shutdown: Option<Arc<Notify>>,
) -> tokio::task::JoinHandle<Result<SchedulerResult, OxError>> {
    let graph = JobGraph::build(jobs).unwrap();
    let config = SchedulerConfig {
        max_jobs: 3,
        resource_budget: BTreeMap::from([(capacity.0.into(), capacity.1)]),
        ..Default::default()
    };
    tokio::spawn(async move {
        tokio::time::timeout(
            Duration::from_secs(300),
            run_scheduler_with_cache(
                &graph,
                executor,
                &config,
                &EventBus::new(),
                &default_ctx(),
                None,
                None,
                None,
                shutdown,
                None,
            ),
        )
        .await
        .expect("scheduler stalled")
    })
}

async fn c2_stage(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<(String, &'static str)>,
) -> (String, &'static str) {
    tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("missing stage")
        .unwrap()
}

#[tokio::test(start_paused = true)]
async fn c2_cpu_serial_gpu_overlap_and_finite_mix_progress() {
    for (resource, capacity, amounts, initial) in [
        ("cpu", 6, vec![4.0, 4.0], 1),
        ("gpu", 1, vec![0.5, 0.5, 0.5], 2),
        ("cpu", 6, vec![6.0, 1.0, 2.0, 1.0, 3.0, 1.0], 1),
    ] {
        let count = amounts.len();
        let jobs = amounts
            .into_iter()
            .enumerate()
            .map(|(i, amount)| {
                c2_job(
                    &format!("job{i}"),
                    resource,
                    ResourceValue::Float(amount.into()),
                )
            })
            .collect();
        let (executor, mut rx) = C2Executor::new("");
        let run = c2_run(jobs, executor.clone(), (resource, capacity), None);
        for _ in 0..initial {
            assert_eq!(c2_stage(&mut rx).await.1, "prepare");
        }
        if count <= 3 {
            assert!(
                rx.try_recv().is_err(),
                "extra job started while capacity was exhausted"
            );
            let budget = observed_budget();
            assert_eq!(budget.inner.lock().unwrap().history.len(), initial);
        }
        executor.proceed.add_permits(initial);
        for _ in initial..3 * count {
            c2_stage(&mut rx).await;
            executor.proceed.add_permits(1);
        }
        assert_eq!(run.await.unwrap().unwrap().succeeded, count);
        c2_history(count);
    }
}

#[tokio::test(start_paused = true)]
async fn c2_attempt_errors_and_unwind_release_once() {
    for (failure, stages) in [
        ("prepare", 1),
        ("execute", 2),
        ("finalize", 3),
        ("panic", 2),
    ] {
        let (executor, mut rx) = C2Executor::new(failure);
        let run = c2_run(
            vec![c2_job("job", "cpu", ResourceValue::Int(1))],
            executor.clone(),
            ("cpu", 1),
            None,
        );
        for _ in 0..stages {
            c2_stage(&mut rx).await;
            let budget = observed_budget();
            let inner = budget.inner.lock().unwrap();
            assert_eq!(inner.in_use[&CanonicalResource::Cpu], 10_000);
            assert_eq!(
                inner.history.len(),
                1,
                "permit must cover every attempt phase"
            );
            drop(inner);
            executor.proceed.add_permits(1);
        }
        let result = run.await.unwrap();
        if failure == "finalize" {
            assert_eq!(result.unwrap().failed, 1);
        } else {
            assert!(result.is_err());
        }
        c2_history(1);
    }
}

#[tokio::test(start_paused = true)]
async fn c2_cancel_preparing_running_and_budget_waiter() {
    for cancel_stage in ["prepare", "execute", "finalize"] {
        let (executor, mut rx) = C2Executor::new("");
        let shutdown = Arc::new(Notify::new());
        let run = c2_run(
            vec![
                c2_job("a", "cpu", ResourceValue::Int(1)),
                c2_job("b", "cpu", ResourceValue::Int(1)),
            ],
            executor.clone(),
            ("cpu", 1),
            Some(shutdown.clone()),
        );
        loop {
            let (_, stage) = c2_stage(&mut rx).await;
            if stage == cancel_stage {
                break;
            }
            executor.proceed.add_permits(1);
        }
        shutdown.notify_one();
        executor.cancelled.notified().await;
        assert!(!run.is_finished(), "cancelled execution is still alive");
        let budget = observed_budget();
        assert_eq!(
            budget.inner.lock().unwrap().in_use[&CanonicalResource::Cpu],
            10_000
        );
        assert_eq!(
            budget.inner.lock().unwrap().history.len(),
            1,
            "Cancelled must not release a live task's permit"
        );
        executor.proceed.add_permits(3);
        let result = run.await.unwrap().unwrap();
        assert_eq!(result.cancelled, 2);
        assert_eq!(
            executor.attempts.load(Ordering::SeqCst),
            1,
            "budget waiter must never start"
        );
        c2_history(1);
    }
}

#[test]
fn c2_exact_memory_custom_zero_and_uninterpreted_values() {
    let budget = ResourceBudget::new(BTreeMap::from([
        ("mem_mb".into(), 1),
        ("gpu".into(), 0),
        ("metal".into(), 1),
    ]));
    let raw = BTreeMap::from([
        ("memory".into(), ResourceValue::Str("1048576B".into())),
        ("gpus".into(), ResourceValue::Int(0)),
        ("custom:metal".into(), ResourceValue::Str("0.0001".into())),
        (
            "cpu".into(),
            ResourceValue::Str("deliberately invalid".into()),
        ),
    ]);
    assert!(budget.fits(&raw));
    assert!(budget.fits(&BTreeMap::new()));
    let guard = budget.acquire(&raw);
    assert_eq!(
        budget.inner.lock().unwrap().in_use[&CanonicalResource::Memory],
        1_048_576
    );
    assert_eq!(
        budget.inner.lock().unwrap().in_use[&CanonicalResource::Custom("metal".into())],
        1
    );
    assert!(!budget.fits(&BTreeMap::from([("mem".into(), ResourceValue::Int(1))])));
    assert!(!budget.fits(&BTreeMap::from([(
        "gpu".into(),
        ResourceValue::Str("0.0001".into())
    )])));
    drop(guard);
    c2_history(1);
    assert!(
        ResourceBudget::try_new(BTreeMap::from([("cpu".into(), 1), ("cpus".into(), 1)])).is_err()
    );
    assert!(ResourceBudget::try_new(BTreeMap::from([("cpu".into(), u64::MAX)])).is_err());
    let max_memory = ResourceBudget::new(BTreeMap::from([("mem".into(), u64::MAX)]));
    let full = BTreeMap::from([("memory".into(), ResourceValue::Str(u64::MAX.to_string()))]);
    let guard = max_memory.acquire(&full);
    assert!(!max_memory.fits(&BTreeMap::from([("mem".into(), ResourceValue::Int(1))])));
    drop(guard);
}

#[tokio::test(start_paused = true)]
async fn c2_retry_releases_and_reacquires_and_long_backoff_cancels() {
    for shutdown_after in [None, Some(7)] {
        let (executor, mut stages) = C2Executor::new(if shutdown_after.is_some() {
            "always_retry"
        } else {
            "retry"
        });
        let mut job = c2_job("retry", "cpu", ResourceValue::Int(1));
        job.error_strategy = ErrorStrategy::Retry {
            count: 20,
            backoff: Backoff::Exponential,
        };
        let graph = JobGraph::build(vec![job]).unwrap();
        let config = SchedulerConfig {
            resource_budget: BTreeMap::from([("cpu".into(), 1)]),
            ..Default::default()
        };
        let bus = EventBus::new();
        let mut events = bus.subscribe();
        let shutdown = Arc::new(Notify::new());
        let signal = shutdown.clone();
        let exec = executor.clone();
        let run = tokio::spawn(async move {
            run_scheduler_with_cache(
                &graph,
                exec,
                &config,
                &bus,
                &default_ctx(),
                None,
                None,
                None,
                Some(signal),
                None,
            )
            .await
        });
        let attempts = shutdown_after.unwrap_or(2);
        for attempt in 1..=attempts {
            for _ in 0..3 {
                tokio::time::timeout(Duration::from_secs(70), stages.recv())
                    .await
                    .unwrap()
                    .unwrap();
                executor.proceed.add_permits(1);
            }
            if attempt == 1 || shutdown_after.is_some() {
                loop {
                    if matches!(events.recv().await.unwrap(), Event::JobFailed { .. }) {
                        break;
                    }
                }
                c2_history(attempt);
                let budget = observed_budget();
                assert!(
                    budget
                        .inner
                        .lock()
                        .unwrap()
                        .history
                        .iter()
                        .enumerate()
                        .all(|(i, (acquire, _))| *acquire == (i % 2 == 0)),
                    "each retry must release before reacquiring"
                );
            }
        }
        if shutdown_after.is_some() {
            // Attempt 7 is now in a 64-second backoff. Shutdown must return
            // without advancing through that deadline or starting attempt 8.
            shutdown.notify_one();
            let result = tokio::time::timeout(Duration::from_millis(100), run)
                .await
                .expect("shutdown blocked by retry backoff")
                .unwrap()
                .unwrap();
            assert_eq!(result.cancelled, 1);
        } else {
            assert_eq!(run.await.unwrap().unwrap().succeeded, 1);
        }
        c2_history(attempts);
    }
}

#[tokio::test(start_paused = true)]
async fn c2_lost_claim_never_acquires_or_prepares() {
    let (executor, mut stages) = C2Executor::new("");
    let graph = JobGraph::build(vec![c2_job("peer", "cpu", ResourceValue::Int(1))]).unwrap();
    let config = SchedulerConfig {
        resource_budget: BTreeMap::from([("cpu".into(), 1)]),
        ..Default::default()
    };
    let claimer = Arc::new(ScriptedClaimer::new().script(
        "peer",
        vec![lost()],
        vec![PeerJobState::Completed],
    ));
    let result = run_scheduler_with_claims(
        &graph,
        executor,
        &config,
        &EventBus::new(),
        &default_ctx(),
        None,
        None,
        None,
        None,
        None,
        Some(claimer.clone()),
    )
    .await
    .unwrap();
    assert_eq!(result.succeeded, 1);
    assert_eq!(claimer.claims_for("peer"), 1);
    assert!(
        stages.try_recv().is_err(),
        "output locks live inside prepare, which must never be called after a lost claim"
    );
    c2_history(0);
}

#[tokio::test(start_paused = true)]
async fn c2_zero_absent_and_unselected_demands() {
    let zero = c2_job("zero", "cpu", ResourceValue::Int(0));
    let absent = make_job("absent", "absent", vec![], vec![]);
    let mut graph = JobGraph::build(vec![
        zero,
        absent,
        c2_job("unselected", "cpu", ResourceValue::Int(7)),
    ])
    .unwrap();
    // Selection has removed this job from the execution graph.
    graph.mark_skipped(&JobId::from("unselected"));
    let config = SchedulerConfig {
        max_jobs: 1,
        resource_budget: BTreeMap::from([("cpu".into(), 0)]),
        ..Default::default()
    };
    let result = run_scheduler(
        &graph,
        Arc::new(MockExecutor::new()),
        &config,
        &EventBus::new(),
        &default_ctx(),
    )
    .await
    .unwrap();
    assert_eq!(result.succeeded, 2);
    c2_history(2);
    let budget = observed_budget();
    assert_eq!(
        budget
            .inner
            .lock()
            .unwrap()
            .history
            .iter()
            .map(|(acquire, _)| *acquire)
            .collect::<Vec<_>>(),
        vec![true, false, true, false],
        "absent demand still takes one -j slot"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn c2_permit_covers_blocked_output_hashing() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("hash-input");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let mut job = make_job("hash", "hash", vec![], vec![fifo.to_str().unwrap()]);
    job.resources.insert("cpu".into(), ResourceValue::Int(1));
    let graph = JobGraph::build(vec![job]).unwrap();
    let (opened, reader_started) = tokio::sync::oneshot::channel();
    let (close, wait_to_close) = std::sync::mpsc::channel::<()>();
    let writer = std::thread::spawn(move || {
        let file = std::fs::OpenOptions::new().write(true).open(fifo).unwrap();
        opened.send(()).unwrap();
        let _ = wait_to_close.recv();
        drop(file);
    });
    let run = tokio::spawn(async move {
        let config = SchedulerConfig {
            resource_budget: BTreeMap::from([("cpu".into(), 1)]),
            ..Default::default()
        };
        run_scheduler_with_cache(
            &graph,
            Arc::new(MockExecutor::new()),
            &config,
            &EventBus::new(),
            &default_ctx(),
            Some(Arc::new(HashRecordingCache::default())),
            None,
            None,
            None,
            None,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), reader_started)
        .await
        .unwrap()
        .unwrap();
    let budget = observed_budget();
    assert_eq!(
        budget.inner.lock().unwrap().history.len(),
        1,
        "hashing still owns the attempt permit"
    );
    assert_eq!(
        budget.inner.lock().unwrap().in_use[&CanonicalResource::Cpu],
        10_000
    );
    assert!(!run.is_finished());
    close.send(()).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), run)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .succeeded,
        1
    );
    writer.join().unwrap();
    c2_history(1);
}

#[test]
fn c2_fractional_alias_admission() {
    let budget = ResourceBudget::new(BTreeMap::from([("gpu".into(), 1)]));
    let half = BTreeMap::from([("gpus".into(), ResourceValue::Float(0.5.into()))]);
    let first = budget.acquire(&half);
    let second = budget.acquire(&half);
    assert!(!budget.fits(&half), "two halves exhaust one GPU");
    drop(first);
    assert!(budget.fits(&half));
    drop(second);
}

#[tokio::test(start_paused = true)]
async fn c2_impossible_rejected_before_claim_or_start() {
    for (capacity, demand) in [(6, 7), (0, 1)] {
        let mut job = make_job("impossible", "r", vec![], vec!["preserved.txt"]);
        job.resources
            .insert("cpu".into(), ResourceValue::Int(demand));
        let graph = JobGraph::build(vec![job]).unwrap();
        let config = SchedulerConfig {
            resource_budget: BTreeMap::from([("cpu".into(), capacity)]),
            ..Default::default()
        };
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let claimer = Arc::new(ScriptedClaimer::new());
        let executor = Arc::new(MockExecutor::new());
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            run_with_claimer(
                &graph,
                executor.clone(),
                &config,
                &bus,
                claimer.clone(),
                None,
            ),
        )
        .await;
        assert!(
            result.is_ok(),
            "impossible demand must fail instead of waiting"
        );
        let error = result.unwrap().unwrap_err().to_string();
        assert!(
            error.contains("impossible") && error.contains("cpu"),
            "{error}"
        );
        assert_eq!(claimer.claims_for("impossible"), 0);
        assert_eq!(executor.call_count.load(Ordering::SeqCst), 0);
        assert!(drain(&mut rx).is_empty());
    }
}

#[tokio::test]
async fn c2_empty_budget_leaves_raw_declarations_uninterpreted() {
    let mut job = c2_job("raw", "cpu", ResourceValue::Int(-1));
    job.resources
        .insert("cpus".into(), ResourceValue::Str("not a number".into()));
    job.resources
        .insert("memory".into(), ResourceValue::Str("not a unit".into()));
    let graph = JobGraph::build(vec![job]).unwrap();
    let executor = Arc::new(MockExecutor::new());
    let result = run_scheduler(
        &graph,
        executor.clone(),
        &SchedulerConfig::default(),
        &EventBus::new(),
        &default_ctx(),
    )
    .await
    .unwrap();
    assert_eq!(result.succeeded, 1);
    assert_eq!(executor.call_count.load(Ordering::SeqCst), 1);
}
