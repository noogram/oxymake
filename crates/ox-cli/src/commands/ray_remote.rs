//! Ray driver lifecycle shared by status, logs and cancellation.
use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context, Result};
use ox_exec_ray::ray_client::{RayClient, RayJobStatus};
use ox_state::db::{RemoteJobSubmission, StateDb};

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?)
}

fn client(address: Option<&str>) -> Result<RayClient> {
    Ok(RayClient::new(
        address.unwrap_or("http://127.0.0.1:8265").into(),
        ox_exec_ray::ray_client_http(Duration::from_secs(10))?,
    ))
}

fn same_driver(a: &RemoteJobSubmission, b: &RemoteJobSubmission) -> bool {
    a.executor == b.executor && a.address == b.address && a.submission_id == b.submission_id
}

pub(super) fn stop_selected(db: &StateDb, mut selected: Vec<String>) -> Result<Vec<String>> {
    let jobs = db.remote_job_submissions()?;
    let mut stopped = HashSet::new();
    let rt = runtime()?;
    for job in &jobs {
        if job.executor != "ray"
            || !selected.contains(&job.job_id)
            || !matches!(job.status.as_str(), "pending" | "running")
        {
            continue;
        }
        if stopped.insert((job.address.clone(), job.submission_id.clone())) {
            rt.block_on(client(job.address.as_deref())?.stop_job(&job.submission_id))
                .with_context(|| {
                    format!(
                        "could not stop Ray driver {}; local cancellation not recorded",
                        job.submission_id
                    )
                })?;
            for sibling in jobs.iter().filter(|other| same_driver(job, other)) {
                if !selected.contains(&sibling.job_id) {
                    selected.push(sibling.job_id.clone());
                }
            }
        }
    }
    Ok(selected)
}

pub(super) fn sync_driver_failures(db: &StateDb, run: &str, address: Option<&str>) -> Result<()> {
    let jobs = db.remote_job_submissions()?;
    let mut seen = HashSet::new();
    let rt = runtime()?;
    for job in jobs
        .iter()
        .filter(|j| j.executor == "ray" && j.run_id == run)
    {
        if !seen.insert(job.submission_id.clone()) {
            continue;
        }
        let details = rt.block_on(
            client(address.or(job.address.as_deref()))?.get_job_details(&job.submission_id),
        )?;
        if matches!(details.status, RayJobStatus::Failed | RayJobStatus::Stopped) {
            let affected = jobs
                .iter()
                .filter(|other| {
                    same_driver(job, other)
                        && matches!(other.status.as_str(), "pending" | "running")
                })
                .collect::<Vec<_>>();
            if details.status == RayJobStatus::Stopped {
                db.cancel_job_ids(
                    &affected
                        .iter()
                        .map(|j| j.job_id.clone())
                        .collect::<Vec<_>>(),
                )?;
            } else if !affected.is_empty() {
                let session = db.create_session(std::process::id(), "ray-sync", None)?;
                for job in affected {
                    db.claim_job(&job.job_id, &session)?;
                    db.reconcile_fail_job(&job.job_id, 1)?;
                }
                db.complete_session(&session)?;
            }
            let counts = db.job_counts_for_run(run)?;
            db.end_run(
                run,
                counts.completed.saturating_sub(counts.cached),
                counts.failed,
                counts.cached,
            )?;
        }
    }
    Ok(())
}

pub(super) fn job_logs(db: &StateDb, id: &str) -> Result<Option<String>> {
    let Some(job) = db
        .remote_job_submissions()?
        .into_iter()
        .find(|j| j.executor == "ray" && j.job_id == id)
    else {
        return Ok(None);
    };
    let rt = runtime()?;
    let client = client(job.address.as_deref())?;
    let logs = rt.block_on(client.get_job_logs(&job.submission_id))?;
    Ok(Some(logs))
}
