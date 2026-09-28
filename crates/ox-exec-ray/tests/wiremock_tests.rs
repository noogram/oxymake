//! Integration tests using wiremock to mock the Ray Jobs API.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

use ox_core::job_graph::JobGraph;
use ox_core::model::*;
use ox_core::traits::executor::*;
use ox_exec_ray::object_store;
use ox_exec_ray::*;

/// Helper to create a minimal ConcreteJob for testing.
fn test_job(id: &str, command: &str) -> ConcreteJob {
    ConcreteJob {
        id: JobId::from(id),
        rule: RuleName::from("test-rule"),
        wildcards: BTreeMap::new(),
        tags: BTreeMap::new(),
        inputs: vec![],
        outputs: vec![],
        execution: ExecutionBlock::Shell {
            command: command.to_string(),
        },
        resources: BTreeMap::new(),
        environment: None,
        error_strategy: ErrorStrategy::default(),
        timeout: None,
        executor: None,
        priority: None,
        benchmark: None,
        params: BTreeMap::new(),
        param_files: Vec::new(),
        log: LogConfig::default(),
        shell_executable: None,
        clean_outputs: Default::default(),
        platform_scope: Default::default(),
        reproducibility: ReproducibilityClass::default(),
    }
}

fn test_ctx(tmp: &tempfile::TempDir) -> ExecContext {
    ExecContext {
        global_job_limit: 10,
        run_id: "test-run-001".to_string(),
        log_dir: tmp.path().join("logs"),
        project_dir: tmp.path().to_path_buf(),
        trusted_dirs: vec![],
        input_data: std::collections::HashMap::new(),
        memory_map: None,
    }
}

/// The DAG driver is passed to Ray as a filesystem path, so it must be
/// absolute even when callers configure the staging directory relatively.
#[tokio::test]
async fn dag_driver_entrypoint_uses_an_absolute_existing_path() {
    for use_relative_working_dir in [true, false] {
        let mock_server = MockServer::start().await;
        mount_capable_nodes(&mock_server).await;
        Mock::given(method("POST"))
            .and(path("/api/jobs/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"submission_id": "driver-path"})),
            )
            .expect(1)
            .mount(&mock_server)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let working_dir = if use_relative_working_dir {
            staging
                .path()
                .strip_prefix(std::env::current_dir().unwrap())
                .unwrap()
                .to_path_buf()
        } else {
            staging.path().to_path_buf()
        };
        let executor = RayExecutor::new(RayConfig {
            dashboard_address: mock_server.uri(),
            working_dir,
            ..RayConfig::default()
        })
        .unwrap();

        executor
            .submit_dag(
                &JobGraph::build(vec![test_job("driver-path", "true")]).unwrap(),
                &test_ctx(&tmp),
            )
            .await
            .unwrap();

        let request = mock_server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .find(|request| request.method == "POST" && request.url.path() == "/api/jobs/")
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        let driver_path = std::path::Path::new(
            payload["entrypoint"]
                .as_str()
                .unwrap()
                .strip_prefix("python3 ")
                .unwrap(),
        );
        assert!(driver_path.is_absolute(), "{payload}");
        assert!(
            driver_path.exists(),
            "{driver_path:?} must exist when submitted"
        );
    }
}

#[tokio::test]
async fn test_init_health_check() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"ray_version": "2.9.0"})),
        )
        .mount(&mock_server)
        .await;

    let config = RayConfig {
        dashboard_address: mock_server.uri(),
        working_dir: PathBuf::from("/tmp/test-ray"),
        ..RayConfig::default()
    };

    let executor = RayExecutor::new(config).unwrap();
    executor.init().await.unwrap();
    executor.health_check().await.unwrap();
}

#[tokio::test]
async fn test_init_cluster_unreachable() {
    let config = RayConfig {
        dashboard_address: "http://127.0.0.1:1".to_string(), // No server running
        working_dir: PathBuf::from("/tmp/test-ray"),
        ..RayConfig::default()
    };

    let executor = RayExecutor::new(config).unwrap();
    let result = executor.init().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_capabilities() {
    let config = RayConfig {
        max_submit: Some(8),
        ..RayConfig::default()
    };
    let executor = RayExecutor::new(config).unwrap();

    let caps = executor.capabilities();
    assert!(caps.supports_gpu);
    assert!(caps.supports_memory_passing);
    assert!(!caps.supports_streaming);
    assert!(!caps.supports_shadow_dirs);
    assert!(caps.supports_job_arrays);

    assert_eq!(executor.max_concurrency(), Some(8));
}

#[tokio::test]
async fn test_submit_and_succeed() {
    let mock_server = MockServer::start().await;
    let tmp = tempfile::tempdir().unwrap();

    // Version endpoint (for init).
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"ray_version": "2.9.0"})),
        )
        .mount(&mock_server)
        .await;

    // Submit endpoint.
    Mock::given(method("POST"))
        .and(path("/api/jobs/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"submission_id": "raysubmit_test123"})),
        )
        .mount(&mock_server)
        .await;

    // Status endpoint — returns SUCCEEDED immediately.
    Mock::given(method("GET"))
        .and(path_regex(r"/api/jobs/raysubmit_test123$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": "SUCCEEDED",
            "message": null,
            "entrypoint": "echo hello",
            "start_time": 1000000,
            "end_time": 1001000,
            "metadata": {}
        })))
        .mount(&mock_server)
        .await;

    let config = RayConfig {
        dashboard_address: mock_server.uri(),
        working_dir: tmp.path().join("ray-staging"),
        poll_interval_min: Duration::from_millis(10),
        poll_interval_max: Duration::from_millis(50),
        ..RayConfig::default()
    };

    let executor = RayExecutor::new(config).unwrap();
    executor.init().await.unwrap();

    let job = test_job("job-1", "echo hello");
    let ctx = test_ctx(&tmp);

    let workspace = executor.prepare_workspace(&job, &ctx).await.unwrap();
    let result = executor.execute(&job, &workspace, &ctx).await.unwrap();

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.job_id.as_str(), "job-1");
    assert!(result.stderr_tail.is_none());

    executor
        .finalize_workspace(workspace, &result)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_submit_and_fail() {
    let mock_server = MockServer::start().await;
    let tmp = tempfile::tempdir().unwrap();

    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"ray_version": "2.9.0"})),
        )
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/jobs/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"submission_id": "raysubmit_fail1"})),
        )
        .mount(&mock_server)
        .await;

    Mock::given(method("GET"))
        .and(path_regex(r"/api/jobs/raysubmit_fail1$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": "FAILED",
            "message": "Script exited with code 1",
            "entrypoint": "exit 1",
            "start_time": 1000000,
            "end_time": 1001000,
            "metadata": {}
        })))
        .mount(&mock_server)
        .await;

    let config = RayConfig {
        dashboard_address: mock_server.uri(),
        working_dir: tmp.path().join("ray-staging"),
        poll_interval_min: Duration::from_millis(10),
        poll_interval_max: Duration::from_millis(50),
        ..RayConfig::default()
    };

    let executor = RayExecutor::new(config).unwrap();
    executor.init().await.unwrap();

    let job = test_job("job-fail", "exit 1");
    let ctx = test_ctx(&tmp);

    let workspace = executor.prepare_workspace(&job, &ctx).await.unwrap();
    let result = executor.execute(&job, &workspace, &ctx).await.unwrap();

    assert_eq!(result.exit_code, 1);
    assert!(result.stderr_tail.is_some());
    assert!(result.stderr_tail.unwrap().contains("Script exited"));
}

#[tokio::test]
async fn test_cancel_job() {
    let mock_server = MockServer::start().await;
    let tmp = tempfile::tempdir().unwrap();

    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"ray_version": "2.9.0"})),
        )
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/jobs/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"submission_id": "raysubmit_cancel1"})),
        )
        .mount(&mock_server)
        .await;

    // First poll returns RUNNING, subsequent returns STOPPED.
    Mock::given(method("GET"))
        .and(path_regex(r"/api/jobs/raysubmit_cancel1$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": "STOPPED",
            "message": "Job was stopped",
            "metadata": {}
        })))
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path_regex(r"/api/jobs/raysubmit_cancel1/stop$"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    let config = RayConfig {
        dashboard_address: mock_server.uri(),
        working_dir: tmp.path().join("ray-staging"),
        poll_interval_min: Duration::from_millis(10),
        poll_interval_max: Duration::from_millis(50),
        ..RayConfig::default()
    };

    let executor = RayExecutor::new(config).unwrap();
    executor.init().await.unwrap();

    let job = test_job("job-cancel", "sleep 3600");
    let ctx = test_ctx(&tmp);
    let workspace = executor.prepare_workspace(&job, &ctx).await.unwrap();

    // Execute returns with STOPPED status (mock returns STOPPED on first poll).
    let result = executor.execute(&job, &workspace, &ctx).await.unwrap();
    assert_eq!(result.exit_code, 137);
}

#[tokio::test]
async fn test_poll_status_untracked_job() {
    let config = RayConfig {
        dashboard_address: "http://127.0.0.1:9999".to_string(),
        ..RayConfig::default()
    };
    let executor = RayExecutor::new(config).unwrap();

    // Polling an untracked job should return an error.
    let result = executor.poll_status(&JobId::from("nonexistent")).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_submit_with_resources() {
    let mock_server = MockServer::start().await;
    let tmp = tempfile::tempdir().unwrap();

    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"ray_version": "2.9.0"})),
        )
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/jobs/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"submission_id": "raysubmit_res1"})),
        )
        .mount(&mock_server)
        .await;

    Mock::given(method("GET"))
        .and(path_regex(r"/api/jobs/raysubmit_res1$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": "SUCCEEDED",
            "message": null,
            "metadata": {}
        })))
        .mount(&mock_server)
        .await;

    let config = RayConfig {
        dashboard_address: mock_server.uri(),
        working_dir: tmp.path().join("ray-staging"),
        poll_interval_min: Duration::from_millis(10),
        poll_interval_max: Duration::from_millis(50),
        ..RayConfig::default()
    };

    let executor = RayExecutor::new(config).unwrap();
    executor.init().await.unwrap();

    let mut job = test_job("job-gpu", "python train.py");
    job.resources
        .insert("cpu".to_string(), ResourceValue::Int(4));
    job.resources
        .insert("gpu".to_string(), ResourceValue::Str("0.5".to_string()));
    job.resources
        .insert("memory".to_string(), ResourceValue::Str("2G".to_string()));

    let ctx = test_ctx(&tmp);
    let workspace = executor.prepare_workspace(&job, &ctx).await.unwrap();
    let result = executor.execute(&job, &workspace, &ctx).await.unwrap();

    assert_eq!(result.exit_code, 0);
}

/// Helper to create a call-mode ConcreteJob for testing.
fn call_job(id: &str, function: &str) -> ConcreteJob {
    ConcreteJob {
        id: JobId::from(id),
        rule: RuleName::from("test-rule"),
        wildcards: BTreeMap::new(),
        tags: BTreeMap::new(),
        inputs: vec![ResolvedInput {
            reference: OutputRef::File(PathBuf::from("data/input.parquet")),
            name: Some("data".into()),
            format: None,
        }],
        outputs: vec![ResolvedOutput {
            reference: OutputRef::File(PathBuf::from("output/result.parquet")),
            name: None,
            lifecycle: OutputLifecycle::Permanent,
            materialize: MaterializePolicy::Always,
            format: None,
        }],
        execution: ExecutionBlock::Call {
            function: function.to_string(),
            lang: "python".into(),
        },
        resources: BTreeMap::new(),
        environment: None,
        error_strategy: ErrorStrategy::default(),
        timeout: None,
        executor: None,
        priority: None,
        benchmark: None,
        params: BTreeMap::new(),
        param_files: Vec::new(),
        log: LogConfig::default(),
        shell_executable: None,
        clean_outputs: Default::default(),
        platform_scope: Default::default(),
        reproducibility: ReproducibilityClass::default(),
    }
}

#[tokio::test]
async fn test_call_mode_prepare_generates_wrapper() {
    let mock_server = MockServer::start().await;
    let tmp = tempfile::tempdir().unwrap();

    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"ray_version": "2.9.0"})),
        )
        .mount(&mock_server)
        .await;

    let config = RayConfig {
        dashboard_address: mock_server.uri(),
        working_dir: tmp.path().join("ray-staging"),
        ..RayConfig::default()
    };

    let executor = RayExecutor::new(config).unwrap();
    executor.init().await.unwrap();

    let job = call_job("call-job-1", "pipeline.features:compute");
    let ctx = test_ctx(&tmp);

    let workspace = executor.prepare_workspace(&job, &ctx).await.unwrap();

    // Verify the wrapper script was generated.
    let wrapper_path = workspace.work_dir.join("call_wrapper.py");
    assert!(wrapper_path.exists());

    let wrapper_content = std::fs::read_to_string(&wrapper_path).unwrap();
    assert!(wrapper_content.contains("import ray"));
    assert!(wrapper_content.contains("ray.init()"));
    assert!(wrapper_content.contains("from pipeline.features import compute"));
    assert!(wrapper_content.contains("ray.put("));
    assert!(wrapper_content.contains("object_manifest.json"));
    // Always policy: should write to shared FS.
    assert!(wrapper_content.contains(".to_parquet("));
}

#[tokio::test]
async fn test_call_mode_submit_and_succeed() {
    let mock_server = MockServer::start().await;
    let tmp = tempfile::tempdir().unwrap();

    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"ray_version": "2.9.0"})),
        )
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/jobs/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"submission_id": "raysubmit_call1"})),
        )
        .mount(&mock_server)
        .await;

    Mock::given(method("GET"))
        .and(path_regex(r"/api/jobs/raysubmit_call1$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": "SUCCEEDED",
            "message": null,
            "metadata": {}
        })))
        .mount(&mock_server)
        .await;

    let config = RayConfig {
        dashboard_address: mock_server.uri(),
        working_dir: tmp.path().join("ray-staging"),
        poll_interval_min: Duration::from_millis(10),
        poll_interval_max: Duration::from_millis(50),
        ..RayConfig::default()
    };

    let executor = RayExecutor::new(config).unwrap();
    executor.init().await.unwrap();

    let job = call_job("call-job-2", "pipeline.features:compute");
    let ctx = test_ctx(&tmp);

    let workspace = executor.prepare_workspace(&job, &ctx).await.unwrap();
    let result = executor.execute(&job, &workspace, &ctx).await.unwrap();

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.job_id.as_str(), "call-job-2");
}

#[tokio::test]
async fn test_call_mode_in_memory_outputs() {
    let mock_server = MockServer::start().await;
    let tmp = tempfile::tempdir().unwrap();

    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"ray_version": "2.9.0"})),
        )
        .mount(&mock_server)
        .await;

    let config = RayConfig {
        dashboard_address: mock_server.uri(),
        working_dir: tmp.path().join("ray-staging"),
        ..RayConfig::default()
    };

    let executor = RayExecutor::new(config).unwrap();
    executor.init().await.unwrap();

    // Create a job with InMemory input and output.
    let job = ConcreteJob {
        id: JobId::from("call-mem-1"),
        rule: RuleName::from("transform"),
        wildcards: BTreeMap::new(),
        tags: BTreeMap::new(),
        inputs: vec![ResolvedInput {
            reference: OutputRef::InMemory {
                type_hint: Some("DataFrame".into()),
            },
            name: Some("data".into()),
            format: None,
        }],
        outputs: vec![ResolvedOutput {
            reference: OutputRef::InMemory {
                type_hint: Some("DataFrame".into()),
            },
            name: None,
            lifecycle: OutputLifecycle::Temporary,
            materialize: MaterializePolicy::Never,
            format: None,
        }],
        execution: ExecutionBlock::Call {
            function: "pipeline.transform:run".to_string(),
            lang: "python".into(),
        },
        resources: BTreeMap::new(),
        environment: None,
        error_strategy: ErrorStrategy::default(),
        timeout: None,
        executor: None,
        priority: None,
        benchmark: None,
        params: BTreeMap::new(),
        param_files: Vec::new(),
        log: LogConfig::default(),
        shell_executable: None,
        clean_outputs: Default::default(),
        platform_scope: Default::default(),
        reproducibility: ReproducibilityClass::default(),
    };

    let ctx = test_ctx(&tmp);
    let workspace = executor.prepare_workspace(&job, &ctx).await.unwrap();

    // Verify the wrapper handles InMemory I/O.
    let wrapper_content =
        std::fs::read_to_string(workspace.work_dir.join("call_wrapper.py")).unwrap();
    assert!(wrapper_content.contains("ray.get(ray.ObjectRef(bytes.fromhex("));
    assert!(wrapper_content.contains("OXYMAKE_OBJREF_0"));
    assert!(wrapper_content.contains("ray.put("));
    // Never policy: no disk write.
    assert!(!wrapper_content.contains(".to_parquet("));
}

#[tokio::test]
async fn test_object_manifest_read() {
    let tmp = tempfile::tempdir().unwrap();

    // Write a test manifest.
    let manifest = object_store::ObjectManifest {
        refs: {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "output_0".into(),
                object_store::RayObjectRef {
                    object_ref_hex: "deadbeef".into(),
                    type_hint: Some("DataFrame".into()),
                },
            );
            m
        },
    };
    let manifest_json = serde_json::to_string(&manifest).unwrap();
    std::fs::write(
        tmp.path().join(object_store::MANIFEST_FILENAME),
        &manifest_json,
    )
    .unwrap();

    let read = object_store::read_manifest(tmp.path()).await.unwrap();
    assert_eq!(read.refs.len(), 1);
    assert_eq!(read.refs["output_0"].object_ref_hex, "deadbeef");
}

/// Test that `submit_dag` generates a native Ray driver script encoding
/// the full DAG with ObjectRef dependency chaining, and submits it as
/// a single Ray job (fire-and-forget). This replaced the old sequential
/// per-job submission (ox-cmb3).
#[tokio::test]
async fn test_submit_dag_respects_dependencies() {
    use ox_core::job_graph::{JobGraph, make_test_job};

    let mock_server = MockServer::start().await;
    mount_capable_nodes(&mock_server).await;
    let tmp = tempfile::tempdir().unwrap();

    // Version endpoint.
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"ray_version": "2.9.0"})),
        )
        .mount(&mock_server)
        .await;

    // Submit endpoint: one driver job is submitted.
    Mock::given(method("POST"))
        .and(path("/api/jobs/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"submission_id": "raysubmit_driver_001"})),
        )
        .mount(&mock_server)
        .await;

    let config = RayConfig {
        dashboard_address: mock_server.uri(),
        working_dir: tmp.path().join("ray-staging"),
        poll_interval_min: Duration::from_millis(1),
        poll_interval_max: Duration::from_millis(10),
        ..RayConfig::default()
    };

    let executor = RayExecutor::new(config).unwrap();
    executor.init().await.unwrap();

    // Build a DAG: data -> signals -> collect
    let jobs = vec![
        make_test_job("data", &[], &["data.out"]),
        make_test_job("signals", &["data.out"], &["signals.out"]),
        make_test_job("collect", &["signals.out"], &["collect.out"]),
    ];
    let graph = JobGraph::build(jobs).unwrap();

    let ctx = test_ctx(&tmp);
    let result = executor.submit_dag(&graph, &ctx).await.unwrap();

    // One driver job submitted for 3 tasks.
    assert_eq!(result.total_jobs, 3);
    assert_eq!(result.submitted, 1);

    // All job IDs map to the single driver submission ID.
    assert!(result.job_submissions.contains_key("data"));
    assert!(result.job_submissions.contains_key("signals"));
    assert!(result.job_submissions.contains_key("collect"));
    assert_eq!(result.job_submissions["data"], "raysubmit_driver_001");

    // Verify the driver script was generated with DAG structure.
    let driver_path = tmp
        .path()
        .join("ray-staging")
        .join(&ctx.run_id)
        .join("oxymake_dag_driver.py");
    assert!(driver_path.exists(), "driver script should be written");

    let driver_content = std::fs::read_to_string(&driver_path).unwrap();
    assert!(driver_content.contains("ray.init()"));
    assert!(driver_content.contains("@ray.remote"));
    assert!(driver_content.contains("run_shell"));
    assert!(driver_content.contains("ray.get("));
    // The driver should reference all 3 job IDs.
    assert!(driver_content.contains("data"));
    assert!(driver_content.contains("signals"));
    assert!(driver_content.contains("collect"));
}

/// Test that `submit_dag` succeeds even for a DAG where jobs will fail
/// at runtime. With the native driver approach, submit_dag is fire-and-forget:
/// it submits the driver and returns immediately. Failure detection happens
/// at runtime inside the Ray driver, not during submission.
#[tokio::test]
async fn test_submit_dag_fire_and_forget() {
    use ox_core::job_graph::{JobGraph, make_test_job};

    let mock_server = MockServer::start().await;
    mount_capable_nodes(&mock_server).await;
    let tmp = tempfile::tempdir().unwrap();

    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"ray_version": "2.9.0"})),
        )
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/jobs/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"submission_id": "raysubmit_driver_ff"})),
        )
        .mount(&mock_server)
        .await;

    let config = RayConfig {
        dashboard_address: mock_server.uri(),
        working_dir: tmp.path().join("ray-staging"),
        poll_interval_min: Duration::from_millis(1),
        poll_interval_max: Duration::from_millis(10),
        ..RayConfig::default()
    };

    let executor = RayExecutor::new(config).unwrap();
    executor.init().await.unwrap();

    let jobs = vec![
        make_test_job("data", &[], &["data.out"]),
        make_test_job("signals", &["data.out"], &["signals.out"]),
    ];
    let graph = JobGraph::build(jobs).unwrap();

    let ctx = test_ctx(&tmp);

    // With native driver, submit_dag always succeeds (fire-and-forget).
    // The driver handles runtime failures internally.
    let result = executor.submit_dag(&graph, &ctx).await.unwrap();

    assert_eq!(result.total_jobs, 2);
    assert_eq!(result.submitted, 1); // One driver job
    assert!(result.job_submissions.contains_key("data"));
    assert!(result.job_submissions.contains_key("signals"));
}

/// H24: after `submit_dag`, tracking was keyed by run_id only, so
/// `cancel(job_id)` and `poll_status(job_id)` silently no-oped — the Ray
/// driver and all its tasks kept running. Every uncached job must be
/// indexed to the driver submission so that cancelling any job in the
/// DAG cascades to `ray job stop` on the driver.
#[tokio::test]
async fn test_submit_dag_cancel_by_job_id_stops_driver() {
    use ox_core::job_graph::{JobGraph, make_test_job};

    let mock_server = MockServer::start().await;
    mount_capable_nodes(&mock_server).await;
    let tmp = tempfile::tempdir().unwrap();

    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"ray_version": "2.9.0"})),
        )
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/jobs/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"submission_id": "raysubmit_driver_c1"})),
        )
        .mount(&mock_server)
        .await;

    // Status endpoint: the driver is running.
    Mock::given(method("GET"))
        .and(path_regex(r"/api/jobs/raysubmit_driver_c1$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": "RUNNING",
            "message": "",
            "metadata": {}
        })))
        .mount(&mock_server)
        .await;

    // Stop endpoint: cancelling a DAG job by its OxyMake ID must hit this.
    Mock::given(method("POST"))
        .and(path_regex(r"/api/jobs/raysubmit_driver_c1/stop$"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&mock_server)
        .await;

    let config = RayConfig {
        dashboard_address: mock_server.uri(),
        working_dir: tmp.path().join("ray-staging"),
        poll_interval_min: Duration::from_millis(1),
        poll_interval_max: Duration::from_millis(10),
        ..RayConfig::default()
    };

    let executor = RayExecutor::new(config).unwrap();
    executor.init().await.unwrap();

    let jobs = vec![
        make_test_job("data", &[], &["data.out"]),
        make_test_job("signals", &["data.out"], &["signals.out"]),
    ];
    let graph = JobGraph::build(jobs).unwrap();
    let ctx = test_ctx(&tmp);
    executor.submit_dag(&graph, &ctx).await.unwrap();

    // Poll a DAG job by its OxyMake job ID — must reflect the driver status.
    let status = executor
        .poll_status(&JobId::from("signals"))
        .await
        .expect("DAG job must be tracked by job_id after submit_dag");
    assert_eq!(status, JobStatus::Running);

    // Cancel by OxyMake job ID — must cascade to `ray job stop` on the driver.
    executor.cancel(&JobId::from("signals")).await.unwrap();

    // The .expect(1) on the stop mock is verified when mock_server drops.
    mock_server.verify().await;
}

// C1: a successful submission ack is not evidence that a task can run.
fn node_payload(nodes: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"result": true, "data": {"result": {
        "total": nodes.as_array().unwrap().len(), "num_after_truncation": nodes.as_array().unwrap().len(),
        "num_filtered": nodes.as_array().unwrap().len(), "result": nodes,
        "partial_failure_warning": null
    }}})
}

#[tokio::test]
async fn dag_rejects_impossible_requests_before_submission() {
    use ox_core::job_graph::{JobGraph, make_test_job};
    for (cpu, nodes) in [
        (
            None,
            serde_json::json!([{"node_ip":"127.0.0.1","state":"ALIVE","resources_total":{"CPU":8}}]),
        ),
        (
            None,
            serde_json::json!([{"node_ip":"127.0.0.1","state":"ALIVE","resources_total":{"CPU":8,"metal":0}}]),
        ),
        (
            Some(9),
            serde_json::json!([{"node_ip":"127.0.0.1","state":"ALIVE","resources_total":{"CPU":8,"metal":1}}]),
        ),
        (
            Some(2),
            serde_json::json!([{"node_ip":"a","state":"ALIVE","resources_total":{"CPU":8}}, {"node_ip":"b","state":"ALIVE","resources_total":{"CPU":1,"metal":1}}]),
        ),
        (
            None,
            serde_json::json!([{"node_ip":"127.0.0.1","state":"ALIVE","resources_total":{"metal":1}}]),
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v0/nodes"))
            .respond_with(ResponseTemplate::new(200).set_body_json(node_payload(nodes)))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/jobs/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"submission_id":"unexpected"})),
            )
            .expect(0)
            .mount(&server)
            .await;
        let tmp = tempfile::tempdir().unwrap();
        let executor = RayExecutor::new(RayConfig {
            dashboard_address: server.uri(),
            working_dir: tmp.path().into(),
            ..Default::default()
        })
        .unwrap();
        let mut job = make_test_job("a", &[], &["a.out"]);
        job.resources.insert("metal".into(), ResourceValue::Int(1));
        if let Some(cpu) = cpu {
            job.resources.insert("cpu".into(), ResourceValue::Int(cpu));
        }
        let err = executor
            .submit_dag(&JobGraph::build(vec![job]).unwrap(), &test_ctx(&tmp))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("rule")
                && err.contains("metal=1")
                && err.contains("nodes:")
                && err.contains("--ray-allow-pending"),
            "{err}"
        );
    }
}

#[tokio::test]
async fn node_inspection_http_failure_names_pending_escape_hatch() {
    use ox_core::job_graph::{JobGraph, make_test_job};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v0/nodes"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let tmp = tempfile::tempdir().unwrap();
    let executor = RayExecutor::new(RayConfig {
        dashboard_address: server.uri(),
        working_dir: tmp.path().into(),
        ..Default::default()
    })
    .unwrap();
    let error = executor
        .submit_dag(
            &JobGraph::build(vec![make_test_job("a", &[], &["a.out"])]).unwrap(),
            &test_ctx(&tmp),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("404") && error.contains("--ray-allow-pending"),
        "{error}"
    );
}

async fn mount_capable_nodes(server: &MockServer) {
    Mock::given(method("GET")).and(path("/api/v0/nodes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(node_payload(serde_json::json!([
            {"node_ip":"127.0.0.1","state":"ALIVE","resources_total":{"CPU":8,"metal":1},"resources_available":{"CPU":0,"metal":0}}
        ])))).mount(server).await;
}

#[tokio::test]
async fn busy_capacity_and_pending_override_submit_one_zero_cpu_driver() {
    use ox_core::job_graph::{JobGraph, make_test_job};
    for allow_pending in [false, true] {
        let server = MockServer::start().await;
        if !allow_pending {
            Mock::given(method("GET")).and(path("/api/v0/nodes"))
                .respond_with(ResponseTemplate::new(200).set_body_json(node_payload(serde_json::json!([
                    {"node_ip":"127.0.0.1","state":"ALIVE","resources_total":{"CPU":8,"memory":1048576,"metal":1},"resources_available":{"CPU":0,"memory":0,"metal":0}}
                ])))).mount(&server).await;
        }
        Mock::given(method("POST"))
            .and(path("/api/jobs/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"submission_id":"driver"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/jobs/driver"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"PENDING"})),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/jobs/driver/stop"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let tmp = tempfile::tempdir().unwrap();
        let executor = RayExecutor::new(RayConfig {
            allow_pending,
            dashboard_address: server.uri(),
            working_dir: tmp.path().into(),
            ..Default::default()
        })
        .unwrap();
        let mut job = make_test_job("a", &[], &["a.out"]);
        job.resources.insert("metal".into(), ResourceValue::Int(1));
        job.resources.insert("mem_mb".into(), ResourceValue::Int(1));
        executor
            .submit_dag(&JobGraph::build(vec![job]).unwrap(), &test_ctx(&tmp))
            .await
            .unwrap();
        assert!(matches!(
            executor.poll_status(&JobId::from("a")).await.unwrap(),
            JobStatus::Queued
        ));
        executor.cancel(&JobId::from("a")).await.unwrap();
        let requests = server.received_requests().await.unwrap();
        let submitted = requests
            .iter()
            .find(|r| r.method == "POST" && r.url.path() == "/api/jobs/")
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&submitted.body).unwrap();
        assert_eq!(body["entrypoint_num_cpus"], 0.0);
        assert!(body.get("entrypoint_resources").is_none());
        assert!(body.get("entrypoint_memory").is_none());
        assert!(body.get("runtime_env").is_none());
        if allow_pending {
            assert!(!requests.iter().any(|r| r.url.path() == "/api/v0/nodes"));
        }
        let source =
            std::fs::read_to_string(tmp.path().join("test-run-001/oxymake_dag_driver.py")).unwrap();
        assert!(source.contains("resources=json.loads"));
        assert!(source.contains("memory=1048576"));
    }
}

#[tokio::test]
async fn dag_rejects_memory_larger_than_every_live_node_before_submission() {
    use ox_core::job_graph::{JobGraph, make_test_job};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v0/nodes"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(node_payload(serde_json::json!([
                {"node_ip":"a","state":"ALIVE","resources_total":{"CPU":8,"memory":1024}},
                {"node_ip":"b","state":"ALIVE","resources_total":{"CPU":8,"memory":2048}}
            ]))),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/jobs/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"submission_id":"unexpected"})),
        )
        .expect(0)
        .mount(&server)
        .await;
    let tmp = tempfile::tempdir().unwrap();
    let executor = RayExecutor::new(RayConfig {
        dashboard_address: server.uri(),
        working_dir: tmp.path().into(),
        ..Default::default()
    })
    .unwrap();
    let mut job = make_test_job("a", &[], &["a.out"]);
    job.resources
        .insert("memory".into(), ResourceValue::Int(2049));
    let error = executor
        .submit_dag(&JobGraph::build(vec![job]).unwrap(), &test_ctx(&tmp))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("memory=2049")
            && error.contains("no single live node")
            && error.contains("--ray-allow-pending"),
        "{error}"
    );
}

#[tokio::test]
async fn pending_override_still_rejects_invalid_values_and_names() {
    use ox_core::job_graph::{JobGraph, make_test_job};
    let server = MockServer::start().await;
    let tmp = tempfile::tempdir().unwrap();
    let executor = RayExecutor::new(RayConfig {
        allow_pending: true,
        dashboard_address: server.uri(),
        working_dir: tmp.path().into(),
        ..Default::default()
    })
    .unwrap();
    for (key, value) in [("metal", -1), ("custom:cpu", 1), ("custom: metal", 1)] {
        let mut job = make_test_job("a", &[], &["a.out"]);
        job.resources.insert(key.into(), ResourceValue::Int(value));
        assert!(
            executor
                .submit_dag(&JobGraph::build(vec![job]).unwrap(), &test_ctx(&tmp))
                .await
                .is_err()
        );
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn time_min_is_an_infeasible_custom_request_on_cpu_nodes() {
    use ox_core::job_graph::{JobGraph, make_test_job};
    let server = MockServer::start().await;
    mount_capable_nodes(&server).await;
    let tmp = tempfile::tempdir().unwrap();
    let executor = RayExecutor::new(RayConfig {
        dashboard_address: server.uri(),
        working_dir: tmp.path().into(),
        ..Default::default()
    })
    .unwrap();
    let mut job = make_test_job("a", &[], &["a.out"]);
    job.resources
        .insert("time_min".into(), ResourceValue::Int(60));
    let error = executor
        .submit_dag(&JobGraph::build(vec![job]).unwrap(), &test_ctx(&tmp))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("time_min=60") && error.contains("no single live node"),
        "{error}"
    );
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method == "GET")
    );
}
