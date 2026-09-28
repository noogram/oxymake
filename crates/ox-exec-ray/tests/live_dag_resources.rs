//! Real Ray scheduling witnesses. See the Ray chapter for prerequisites.
//! These tests are ignored by default, never replaced by a scheduler mock.
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use ox_core::job_graph::{JobGraph, make_test_job};
use ox_core::model::{ExecutionBlock, JobId, ResourceValue};
use ox_core::traits::executor::{ExecContext, Executor, JobStatus};
use ox_exec_ray::{RayConfig, RayExecutor};

fn context(root: &Path, run: &str) -> ExecContext {
    ExecContext {
        global_job_limit: 8,
        run_id: run.into(),
        log_dir: root.join("logs"),
        project_dir: root.into(),
        trusted_dirs: vec![],
        input_data: HashMap::new(),
        memory_map: None,
    }
}

async fn setup(cpu: f64) -> (tempfile::TempDir, RayExecutor) {
    let address = std::env::var("OXYMAKE_RAY_LIVE_ADDRESS").expect("set OXYMAKE_RAY_LIVE_ADDRESS");
    let shared = std::env::var("OXYMAKE_RAY_LIVE_DIR")
        .expect("set OXYMAKE_RAY_LIVE_DIR to a directory shared at the same path on every node");
    let root = tempfile::tempdir_in(shared).unwrap();
    let payload: serde_json::Value = reqwest::Client::new()
        .get(format!("{address}/api/v0/nodes?detail=1&limit=10000"))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let nodes = payload
        .pointer("/data/result/result")
        .unwrap()
        .as_array()
        .unwrap();
    let alive = nodes
        .iter()
        .filter(|n| n["state"] == "ALIVE")
        .collect::<Vec<_>>();
    assert_eq!(alive.len(), 1, "witness requires exactly one live node");
    assert_eq!(alive[0]["resources_total"]["CPU"].as_f64(), Some(cpu));
    if cpu > 1.0 {
        assert_eq!(alive[0]["resources_total"]["metal"].as_f64(), Some(1.0));
    }
    let executor = RayExecutor::new(RayConfig {
        dashboard_address: address,
        working_dir: root.path().join("runs"),
        ..Default::default()
    })
    .unwrap();
    (root, executor)
}

async fn submit(executor: &RayExecutor, root: &Path, id: &str, metal: bool) {
    let mut job = make_test_job(id, &[], &[]);
    job.execution = ExecutionBlock::Run {
        lang: "python".into(),
        code: format!(
            "import json, time, pathlib\npathlib.Path({:?}).touch()\nstart = time.time()\ntime.sleep(3)\nwith open({:?}, 'w') as f: json.dump([start, time.time()], f)",
            root.join(format!("{id}.started")).to_str().unwrap(),
            root.join(format!("{id}.json")).to_str().unwrap()
        ),
    };
    if metal {
        job.resources.insert("metal".into(), ResourceValue::Int(1));
    }
    executor
        .submit_dag(&JobGraph::build(vec![job]).unwrap(), &context(root, id))
        .await
        .unwrap();
}

async fn wait(executor: &RayExecutor, id: &str) {
    let finished = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            match executor.poll_status(&JobId::from(id)).await.unwrap() {
                JobStatus::Completed => break,
                JobStatus::Failed(reason) => panic!("{reason}"),
                JobStatus::Cancelled => panic!("unexpected cancellation"),
                _ => tokio::time::sleep(Duration::from_millis(200)).await,
            }
        }
    })
    .await;
    if finished.is_err() {
        executor.cancel(&JobId::from(id)).await.unwrap();
        panic!("driver exceeded the 90-second witness deadline");
    }
}

#[tokio::test]
#[ignore = "requires an isolated real Ray cluster with exactly one logical CPU"]
async fn single_cpu_default_task_and_two_concurrent_drivers_complete() {
    let (root, executor) = setup(1.0).await;
    submit(&executor, root.path(), "single", false).await;
    wait(&executor, "single").await;
    tokio::join!(
        submit(&executor, root.path(), "a", false),
        submit(&executor, root.path(), "b", false)
    );
    tokio::join!(wait(&executor, "a"), wait(&executor, "b"));
    assert!(root.path().join("a.json").exists() && root.path().join("b.json").exists());
}

#[tokio::test]
#[ignore = "requires an isolated real Ray cluster with two CPUs and exactly one metal token"]
async fn two_drivers_serialize_metal_with_overlapping_negative_control() {
    let (root, executor) = setup(2.0).await;
    for (left, right, metal) in [("a", "b", true), ("control_a", "control_b", false)] {
        tokio::join!(
            submit(&executor, root.path(), left, metal),
            submit(&executor, root.path(), right, metal)
        );
        tokio::join!(wait(&executor, left), wait(&executor, right));
        let interval = |id: &str| -> [f64; 2] {
            serde_json::from_slice(&std::fs::read(root.path().join(format!("{id}.json"))).unwrap())
                .unwrap()
        };
        let a = interval(left);
        let b = interval(right);
        let overlap = a[0].max(b[0]) < a[1].min(b[1]);
        assert_eq!(overlap, !metal, "metal={metal}: {a:?}, {b:?}");
    }
}

#[tokio::test]
#[ignore = "requires an isolated real Ray cluster with two CPUs and exactly one metal token"]
async fn busy_node_queues_and_cancelled_task_never_starts_after_release() {
    let (root, executor) = setup(2.0).await;
    for cancel in [false, true] {
        let started = root.path().join(format!("holder-{cancel}.started"));
        let release = root.path().join(format!("holder-{cancel}.release"));
        let holder_id = format!("holder-{cancel}");
        let mut holder = make_test_job(&holder_id, &[], &[]);
        holder
            .resources
            .insert("metal".into(), ResourceValue::Int(1));
        holder.execution = ExecutionBlock::Run {
            lang: "python".into(),
            code: format!(
                "import pathlib, time\npathlib.Path({:?}).touch()\ndeadline = time.monotonic() + 45\nwhile not pathlib.Path({:?}).exists() and time.monotonic() < deadline: time.sleep(.1)",
                started.to_str().unwrap(),
                release.to_str().unwrap()
            ),
        };
        executor
            .submit_dag(
                &JobGraph::build(vec![holder]).unwrap(),
                &context(root.path(), &holder_id),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(30), async {
            while !started.exists() {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("holder did not start");
        let queued = format!("queued-{cancel}");
        // Feasibility must accept the busy node based on its total metal=1.
        submit(&executor, root.path(), &queued, true).await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(!root.path().join(format!("{queued}.started")).exists());
        if cancel {
            executor
                .cancel(&JobId::from(queued.as_str()))
                .await
                .unwrap();
        }
        std::fs::write(release, "release").unwrap();
        wait(&executor, &holder_id).await;
        if cancel {
            tokio::time::sleep(Duration::from_secs(6)).await;
            assert!(matches!(
                executor
                    .poll_status(&JobId::from(queued.as_str()))
                    .await
                    .unwrap(),
                JobStatus::Cancelled
            ));
            assert!(!root.path().join(format!("{queued}.started")).exists());
        } else {
            wait(&executor, &queued).await;
            assert!(root.path().join(format!("{queued}.started")).exists());
            assert!(root.path().join(format!("{queued}.json")).exists());
        }
    }
}

#[tokio::test]
#[ignore = "requires an isolated real Ray cluster with one node and at least two CPUs"]
async fn two_tasks_serialize_when_their_memory_sum_exceeds_one_node() {
    let address = std::env::var("OXYMAKE_RAY_LIVE_ADDRESS").expect("set OXYMAKE_RAY_LIVE_ADDRESS");
    let shared = std::env::var("OXYMAKE_RAY_LIVE_DIR")
        .expect("set OXYMAKE_RAY_LIVE_DIR to a directory shared at the same path on every node");
    let root = tempfile::tempdir_in(shared).unwrap();
    let payload: serde_json::Value = reqwest::Client::new()
        .get(format!("{address}/api/v0/nodes?detail=1&limit=10000"))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let alive = payload
        .pointer("/data/result/result")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["state"] == "ALIVE")
        .collect::<Vec<_>>();
    assert_eq!(alive.len(), 1, "witness requires exactly one live node");
    assert!(alive[0]["resources_total"]["CPU"].as_f64().unwrap() >= 2.0);
    let total_memory = alive[0]["resources_total"]["memory"]
        .as_f64()
        .expect("Ray node must advertise memory") as u64;
    let each_memory = total_memory / 2 + 1;
    assert!(each_memory <= total_memory && each_memory.saturating_mul(2) > total_memory);

    let executor = RayExecutor::new(RayConfig {
        dashboard_address: address,
        working_dir: root.path().join("runs"),
        ..Default::default()
    })
    .unwrap();
    let mut jobs = Vec::new();
    for id in ["memory-a", "memory-b"] {
        let mut job = make_test_job(id, &[], &[]);
        job.resources
            .insert("memory".into(), ResourceValue::Int(each_memory as i64));
        job.execution = ExecutionBlock::Run {
            lang: "python".into(),
            code: format!(
                "import json, time\nstart = time.time()\ntime.sleep(3)\nwith open({:?}, 'w') as f: json.dump([start, time.time()], f)",
                root.path().join(format!("{id}.json")).to_str().unwrap()
            ),
        };
        jobs.push(job);
    }
    executor
        .submit_dag(
            &JobGraph::build(jobs).unwrap(),
            &context(root.path(), "memory-serialization"),
        )
        .await
        .unwrap();
    wait(&executor, "memory-a").await;
    let interval = |id: &str| -> [f64; 2] {
        serde_json::from_slice(&std::fs::read(root.path().join(format!("{id}.json"))).unwrap())
            .unwrap()
    };
    let a = interval("memory-a");
    let b = interval("memory-b");
    assert!(a[1] <= b[0] || b[1] <= a[0], "{a:?} overlaps {b:?}");
}

#[tokio::test]
#[ignore = "requires a real token-mode Ray cluster and cluster-local token configuration"]
async fn token_mode_dashboard_and_driver_authenticate() {
    let address = std::env::var("OXYMAKE_RAY_LIVE_ADDRESS")
        .expect("set OXYMAKE_RAY_LIVE_ADDRESS to a token-mode Ray dashboard");
    let shared = std::env::var("OXYMAKE_RAY_LIVE_DIR")
        .expect("set OXYMAKE_RAY_LIVE_DIR to a directory shared at the same path on every node");
    assert_eq!(
        std::env::var("RAY_AUTH_MODE").as_deref(),
        Ok("token"),
        "set RAY_AUTH_MODE=token and configure a client-side token source"
    );
    let root = tempfile::tempdir_in(shared).unwrap();
    let executor = RayExecutor::new(RayConfig {
        allow_pending: true,
        dashboard_address: address,
        working_dir: root.path().join("runs"),
        ..Default::default()
    })
    .unwrap();
    submit(&executor, root.path(), "token-mode", false).await;
    wait(&executor, "token-mode").await;
    assert!(root.path().join("token-mode.json").exists());
}
