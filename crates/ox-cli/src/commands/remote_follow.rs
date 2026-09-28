//! Progress polling shared by the Ray and SLURM DAG submission paths.
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ox_core::scheduler::SchedulerResult;
use ox_core::traits::executor::JobStatus;
use tokio::sync::Notify;

pub(super) struct FollowPolicy {
    pub interval: Duration,
    pub failed_round_limit: usize,
}

impl FollowPolicy {
    /// Stop after three consecutive rounds containing a poll error. This allows
    /// two transient failures without aborting a long run, but bounds retries
    /// when the endpoint disappears. Only an entirely successful round resets
    /// the count: a healthy job must not mask another job's persistent error.
    /// Ray waits 3 seconds between rounds; SLURM waits 5. Request latency is
    /// additional, so this is a round bound, not a wall-clock deadline.
    /// Tests inject both the interval and limit without waiting in real time.
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            failed_round_limit: 3,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum FollowStop {
    Interrupted,
    RayInterrupted {
        cancellation_error: Option<String>,
    },
    Unreachable {
        endpoint: String,
        last_error: String,
        rounds: usize,
    },
}

impl FollowStop {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Interrupted | Self::RayInterrupted { .. } => 130,
            Self::Unreachable { .. } => 1,
        }
    }

    pub fn message(&self) -> String {
        let reason = match self {
            Self::RayInterrupted { cancellation_error } => {
                return match cancellation_error {
                    None => "Follow interrupted. Ray driver stopped.".to_string(),
                    Some(error) => format!(
                        "Follow interrupted; failed to stop Ray driver: {error}. Driver state is unknown. Use 'ox status' to check progress and 'ox cancel' to retry cancellation."
                    ),
                };
            }
            Self::Interrupted => "Follow interrupted".to_string(),
            Self::Unreachable {
                endpoint,
                last_error,
                rounds,
            } => format!(
                "Follow failed: endpoint {endpoint} unavailable after {rounds} consecutive failed poll rounds; last error: {last_error}"
            ),
        };
        format!(
            "{reason}. Unfinished remote jobs may still be running; remote jobs are NOT cancelled. Use 'ox status' to check progress and 'ox cancel' to cancel them."
        )
    }
}

pub(super) struct FollowRequest<'a> {
    pub jobs: Vec<String>,
    pub total: usize,
    pub skipped: usize,
    pub endpoint: &'a str,
    pub policy: FollowPolicy,
    pub interrupted: &'a AtomicBool,
    pub shutdown: &'a Notify,
}

pub(super) async fn follow<F, Fut, E>(
    request: FollowRequest<'_>,
    mut poll: F,
    mut report: impl FnMut(String),
) -> (SchedulerResult, Option<FollowStop>)
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<JobStatus, E>>,
    E: std::fmt::Display,
{
    let start = Instant::now();
    let mut known = BTreeMap::new();
    let mut failed_rounds = 0;
    let stop = 'rounds: loop {
        if until_interrupt(
            tokio::time::sleep(request.policy.interval),
            request.interrupted,
            request.shutdown,
        )
        .await
        .is_none()
        {
            break Some(FollowStop::Interrupted);
        }
        let mut completed = 0;
        let mut failed = 0;
        let mut last_error = None;
        let mut still_running = false;
        for job in &request.jobs {
            let Some(polled) =
                until_interrupt(poll(job.clone()), request.interrupted, request.shutdown).await
            else {
                break 'rounds Some(FollowStop::Interrupted);
            };
            match polled {
                Ok(status) => {
                    match &status {
                        JobStatus::Completed => completed += 1,
                        JobStatus::Failed(msg) => {
                            report(format!("  FAILED: {job} — {msg}"));
                            failed += 1;
                        }
                        JobStatus::Cancelled => failed += 1,
                        _ => still_running = true,
                    }
                    known.insert(job.clone(), status);
                }
                Err(e) => {
                    report(format!("  Warning: failed to poll {job}: {e}"));
                    still_running = true;
                    last_error = Some(e.to_string());
                }
            }
        }
        let done = completed + failed;
        report(format!(
            "  Progress: {done}/{} ({completed} succeeded, {failed} failed)",
            request.total
        ));
        if let Some(last_error) = last_error {
            failed_rounds += 1;
            if failed_rounds >= request.policy.failed_round_limit {
                break Some(FollowStop::Unreachable {
                    endpoint: request.endpoint.into(),
                    last_error,
                    rounds: failed_rounds,
                });
            }
        } else {
            failed_rounds = 0;
        }
        if !still_running || done >= request.total {
            break None;
        }
    };
    // Preserve the last successful observation for each job when a round is
    // interrupted or polls fail. Unknown work is neither failed nor cancelled.
    let completed = known
        .values()
        .filter(|s| matches!(s, JobStatus::Completed))
        .count();
    let failed = known
        .values()
        .filter(|s| matches!(s, JobStatus::Failed(_) | JobStatus::Cancelled))
        .count();
    (
        SchedulerResult {
            total_jobs: request.total,
            succeeded: completed,
            failed,
            skipped: request.skipped,
            cancelled: 0,
            duration: start.elapsed(),
            failed_details: vec![],
            root_cause: None,
            memory_stats: None,
        },
        stop,
    )
}

/// Register before checking the sticky flag so notify_waiters cannot be lost
/// between the flag check and select. Both sleeping and in-flight I/O stop.
async fn until_interrupt<T>(
    future: impl Future<Output = T>,
    interrupted: &AtomicBool,
    shutdown: &Notify,
) -> Option<T> {
    let notified = shutdown.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();
    if interrupted.load(Ordering::SeqCst) {
        return None;
    }
    tokio::select! {
        biased;
        _ = notified => None,
        value = future => Some(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn remote_follow_stops_at_injected_failure_bound() {
        for endpoint in ["http://ray.invalid:8265", "http://slurm.invalid:6820"] {
            let interrupted = AtomicBool::new(false);
            let shutdown = Notify::new();
            let mut calls = 0;
            let (result, stop) = follow(
                FollowRequest {
                    jobs: vec!["job".into()],
                    total: 1,
                    skipped: 0,
                    endpoint,
                    policy: FollowPolicy {
                        interval: Duration::ZERO,
                        failed_round_limit: 2,
                    },
                    interrupted: &interrupted,
                    shutdown: &shutdown,
                },
                |_| {
                    calls += 1;
                    assert!(
                        calls <= 3,
                        "polling exceeded injected bound after one healthy round"
                    );
                    std::future::ready(if calls == 1 {
                        Ok(JobStatus::Running)
                    } else {
                        Err("connection refused")
                    })
                },
                |_| {},
            )
            .await;
            assert_eq!(calls, 3);
            assert_eq!(result.succeeded, 0);
            let stop = stop.expect("endpoint loss must not be an empty success");
            assert_eq!(stop.exit_code(), 1);
            assert!(stop.message().contains(endpoint));
            assert!(stop.message().contains("connection refused"));
        }
    }

    #[tokio::test]
    async fn remote_follow_http_outage_recovery_and_happy_path() {
        use ox_exec_ray::ray_client::{RayClient, RayJobStatus};
        use ox_exec_slurm::slurm_rest::SlurmRestClient;
        use serde_json::json;
        use std::sync::{Arc, atomic::AtomicUsize};
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        for ray in [true, false] {
            // 0 = running, 1 = unavailable, 2 = complete. The recovery sequence
            // has four failures overall but never three consecutive failures.
            for sequence in [vec![0, 1, 1, 1], vec![0, 1, 1, 0, 1, 1, 2], vec![0, 2]] {
                let server = MockServer::start().await;
                let calls = Arc::new(AtomicUsize::new(0));
                let expected_calls = sequence.len();
                let outage = sequence.last() == Some(&1);
                let happy = sequence == [0, 2];
                let seen = calls.clone();
                Mock::given(method("GET")).respond_with(move |_: &wiremock::Request| {
                    let index = seen.fetch_add(1, Ordering::SeqCst);
                    assert!(index < sequence.len(), "poll exceeded bound");
                    if sequence[index] == 1 { return ResponseTemplate::new(503).set_body_string("dashboard gone"); }
                    let complete = sequence[index] == 2;
                    ResponseTemplate::new(200).set_body_json(if ray {
                        json!({"status": if complete { "SUCCEEDED" } else { "RUNNING" }})
                    } else {
                        json!({"jobs":[{"job_id":42,"job_state":[if complete { "COMPLETED" } else { "RUNNING" }]}]})
                    })
                }).expect(expected_calls as u64).mount(&server).await;
                let ray_client = RayClient::new(
                    server.uri(),
                    ox_exec_ray::ray_client_http(Duration::from_secs(1)).unwrap(),
                )
                .unwrap();
                let slurm_client = SlurmRestClient::new(server.uri(), "test".into(), None);
                let interrupted = AtomicBool::new(false);
                let shutdown = Notify::new();
                let mut lines = vec![];
                let (result, stop) = follow(
                    FollowRequest {
                        jobs: vec!["job".into()],
                        total: 1,
                        skipped: 0,
                        endpoint: &server.uri(),
                        policy: FollowPolicy {
                            interval: Duration::ZERO,
                            failed_round_limit: 3,
                        },
                        interrupted: &interrupted,
                        shutdown: &shutdown,
                    },
                    |_| async {
                        if ray {
                            ray_client
                                .get_job_details("driver")
                                .await
                                .map(|s| {
                                    if s.status == RayJobStatus::Succeeded {
                                        JobStatus::Completed
                                    } else {
                                        JobStatus::Running
                                    }
                                })
                                .map_err(|e| e.to_string())
                        } else {
                            slurm_client
                                .get_job(42)
                                .await
                                .map(|s| {
                                    if s.unwrap().job_state == ["COMPLETED"] {
                                        JobStatus::Completed
                                    } else {
                                        JobStatus::Running
                                    }
                                })
                                .map_err(|e| e.to_string())
                        }
                    },
                    |line| lines.push(line),
                )
                .await;
                assert_eq!(calls.load(Ordering::SeqCst), expected_calls);
                if outage {
                    let stop = stop.unwrap();
                    assert_eq!(stop.exit_code(), 1);
                    assert!(stop.message().contains(&server.uri()));
                    assert!(stop.message().contains("503"), "{}", stop.message());
                    assert_eq!(result.succeeded, 0);
                } else {
                    assert!(stop.is_none());
                    assert_eq!((result.succeeded, result.failed), (1, 0));
                }
                if happy {
                    assert_eq!(
                        lines,
                        [
                            "  Progress: 0/1 (0 succeeded, 0 failed)",
                            "  Progress: 1/1 (1 succeeded, 0 failed)"
                        ]
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn remote_follow_interrupt_during_poll_preserves_known_counts() {
        let interrupted = AtomicBool::new(false);
        let shutdown = Notify::new();
        let mut calls = 0;
        let (result, stop) = tokio::time::timeout(
            Duration::from_secs(1),
            follow(
                FollowRequest {
                    jobs: vec!["done".into(), "running".into()],
                    total: 2,
                    skipped: 0,
                    endpoint: "remote",
                    policy: FollowPolicy::new(Duration::ZERO),
                    interrupted: &interrupted,
                    shutdown: &shutdown,
                },
                |_| {
                    calls += 1;
                    let calls = calls;
                    let interrupted = &interrupted;
                    let shutdown = &shutdown;
                    async move {
                        if calls == 1 {
                            return Ok::<_, String>(JobStatus::Completed);
                        }
                        interrupted.store(true, Ordering::SeqCst);
                        shutdown.notify_waiters();
                        std::future::pending().await
                    }
                },
                |_| {},
            ),
        )
        .await
        .expect("interrupt must cancel the in-flight poll future");
        assert_eq!(calls, 2);
        assert_eq!(
            (result.succeeded, result.failed, result.cancelled),
            (1, 0, 0)
        );
        assert_eq!(stop, Some(FollowStop::Interrupted));
        assert_eq!(stop.unwrap().exit_code(), 130);
    }

    #[tokio::test]
    async fn remote_follow_interrupt_before_or_during_interval() {
        for already_interrupted in [true, false] {
            let interrupted = AtomicBool::new(already_interrupted);
            let shutdown = Notify::new();
            let run = follow(
                FollowRequest {
                    jobs: vec!["job".into()],
                    total: 1,
                    skipped: 0,
                    endpoint: "remote",
                    policy: FollowPolicy::new(Duration::from_secs(3600)),
                    interrupted: &interrupted,
                    shutdown: &shutdown,
                },
                |_| std::future::ready(Err::<JobStatus, _>("must not poll")),
                |_| {},
            );
            let signal = async {
                tokio::task::yield_now().await;
                interrupted.store(true, Ordering::SeqCst);
                shutdown.notify_waiters();
            };
            let ((result, stop), ()) =
                tokio::time::timeout(Duration::from_secs(1), async { tokio::join!(run, signal) })
                    .await
                    .expect("interrupt must wake the poll delay");
            assert_eq!(stop, Some(FollowStop::Interrupted));
            assert_eq!(
                (result.succeeded, result.failed, result.cancelled),
                (0, 0, 0)
            );
        }
    }

    #[tokio::test]
    async fn remote_follow_successful_job_does_not_hide_failed_rounds() {
        let interrupted = AtomicBool::new(false);
        let shutdown = Notify::new();
        let mut calls = 0;
        let (result, stop) = follow(
            FollowRequest {
                jobs: vec!["done".into(), "lost".into()],
                total: 2,
                skipped: 0,
                endpoint: "remote",
                policy: FollowPolicy {
                    interval: Duration::ZERO,
                    failed_round_limit: 2,
                },
                interrupted: &interrupted,
                shutdown: &shutdown,
            },
            |job| {
                calls += 1;
                assert!(calls <= 4);
                std::future::ready(if job == "done" {
                    Ok(JobStatus::Completed)
                } else {
                    Err("gone")
                })
            },
            |_| {},
        )
        .await;
        assert_eq!(calls, 4);
        assert_eq!((result.succeeded, result.failed), (1, 0));
        assert_eq!(stop.unwrap().exit_code(), 1);
    }

    #[tokio::test]
    async fn remote_follow_dashboard_disappears_after_running_response() {
        use ox_exec_ray::ray_client::RayClient;
        use ox_exec_slurm::slurm_rest::SlurmRestClient;
        use std::io::{Read, Write};
        for ray in [true, false] {
            // Serve one running response, then close the listener. Subsequent
            // polls get a real transport failure, with no retry sleeps.
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut buf = [0u8; 4096];
                assert!(socket.read(&mut buf).unwrap() > 0);
                let body = if ray {
                    r#"{"status":"RUNNING"}"#
                } else {
                    r#"{"jobs":[{"job_id":42,"job_state":["RUNNING"]}]}"#
                };
                write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            });
            let ray_client = RayClient::new(
                endpoint.clone(),
                ox_exec_ray::ray_client_http(Duration::from_secs(1)).unwrap(),
            )
            .unwrap();
            let slurm_client = SlurmRestClient::new(endpoint.clone(), "test".into(), None);
            let interrupted = AtomicBool::new(false);
            let shutdown = Notify::new();
            let calls = std::cell::Cell::new(0);
            let last_error = std::cell::RefCell::new(String::new());
            let (result, stop) = follow(
                FollowRequest {
                    jobs: vec!["job".into()],
                    total: 1,
                    skipped: 0,
                    endpoint: &endpoint,
                    policy: FollowPolicy {
                        interval: Duration::ZERO,
                        failed_round_limit: 2,
                    },
                    interrupted: &interrupted,
                    shutdown: &shutdown,
                },
                |_| async {
                    calls.set(calls.get() + 1);
                    assert!(
                        calls.get() <= 3,
                        "endpoint loss must respect injected bound"
                    );
                    let result = if ray {
                        ray_client
                            .get_job_details("driver")
                            .await
                            .map(|_| JobStatus::Running)
                            .map_err(|e| e.to_string())
                    } else {
                        slurm_client
                            .get_job(42)
                            .await
                            .map(|_| JobStatus::Running)
                            .map_err(|e| e.to_string())
                    };
                    if let Err(e) = &result {
                        *last_error.borrow_mut() = e.clone();
                    }
                    result
                },
                |_| {},
            )
            .await;
            server.join().unwrap();
            assert_eq!(calls.get(), 3);
            assert_eq!(result.succeeded, 0);
            assert!(!last_error.borrow().is_empty());
            assert_eq!(
                stop,
                Some(FollowStop::Unreachable {
                    endpoint,
                    last_error: last_error.into_inner(),
                    rounds: 2,
                })
            );
        }
    }
}
