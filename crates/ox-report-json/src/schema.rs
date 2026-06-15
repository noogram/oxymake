//! JSON schema documentation for the NDJSON event stream.
//!
//! This module serves as living documentation for the wire format emitted by
//! [`JsonReporter`](crate::reporter::JsonReporter). Each event is a JSON object
//! with an `"event"` field that acts as the type discriminator (set by serde's
//! `#[serde(tag = "event", rename_all = "snake_case")]` on the `Event` enum).
//!
//! # Event catalog
//!
//! ## `run_started`
//!
//! Emitted once when a run begins, after DAG resolution.
//!
//! ```json
//! {"event":"run_started","total_jobs":103429,"to_run":847,"cached":102582}
//! ```
//!
//! | Field        | Type   | Description                            |
//! |-------------|--------|----------------------------------------|
//! | `total_jobs` | `u64`  | Total number of jobs in the DAG        |
//! | `to_run`     | `u64`  | Jobs that will actually execute         |
//! | `cached`     | `u64`  | Jobs satisfied from cache              |
//!
//! ## `job_queued`
//!
//! Emitted when a job enters the ready queue.
//!
//! ```json
//! {"event":"job_queued","job_id":"align_S001","rule":"align","tags":{"sample":"S001"}}
//! ```
//!
//! | Field    | Type              | Description                       |
//! |---------|-------------------|-----------------------------------|
//! | `job_id` | `string`          | Unique job identifier             |
//! | `rule`   | `string`          | Rule that generated this job      |
//! | `tags`   | `object<str,str>` | Key-value tags for filtering      |
//!
//! ## `job_started`
//!
//! Emitted when a job begins execution.
//!
//! ```json
//! {"event":"job_started","job_id":"align_S001","executor":"local"}
//! ```
//!
//! | Field      | Type     | Description                           |
//! |-----------|----------|---------------------------------------|
//! | `job_id`   | `string` | Unique job identifier                 |
//! | `executor` | `string` | Executor backend (e.g. "local", "slurm") |
//!
//! ## `job_completed`
//!
//! Emitted when a job finishes successfully.
//!
//! ```json
//! {"event":"job_completed","job_id":"align_S001","duration_ms":272000,"outputs":["results/S001.bam"]}
//! ```
//!
//! | Field         | Type       | Description                       |
//! |--------------|------------|-----------------------------------|
//! | `job_id`      | `string`   | Unique job identifier             |
//! | `duration_ms` | `u64`      | Wall-clock duration in ms         |
//! | `outputs`     | `[string]` | Paths or IDs of produced outputs  |
//!
//! ## `job_failed`
//!
//! Emitted when a job fails.
//!
//! ```json
//! {"event":"job_failed","job_id":"align_S002","error_message":"exit code 1","exit_code":1,"stderr_tail":"..."}
//! ```
//!
//! | Field           | Type      | Description                      |
//! |----------------|-----------|----------------------------------|
//! | `job_id`        | `string`  | Unique job identifier            |
//! | `error_message` | `string`  | Human-readable error             |
//! | `exit_code`     | `i32?`    | Process exit code, if available  |
//! | `stderr_tail`   | `string?` | Last N lines of stderr           |
//!
//! ## `job_skipped`
//!
//! Emitted when a job is skipped (cached, guard failed, downstream of failure).
//!
//! ```json
//! {"event":"job_skipped","job_id":"qc_S001","reason":"cached"}
//! ```
//!
//! | Field    | Type     | Description                        |
//! |---------|----------|------------------------------------|
//! | `job_id` | `string` | Unique job identifier              |
//! | `reason` | `string` | Why the job was skipped            |
//!
//! ## `gate_reached`
//!
//! Emitted when a human-in-the-loop gate checkpoint is reached.
//!
//! ```json
//! {"event":"gate_reached","gate_id":"review-checkpoint","message":"Check alignment QC"}
//! ```
//!
//! | Field     | Type     | Description                       |
//! |----------|----------|-----------------------------------|
//! | `gate_id` | `string` | Gate identifier                   |
//! | `message` | `string` | Message to display to the user    |
//!
//! ## `gate_approved`
//!
//! Emitted when a gate is approved.
//!
//! ```json
//! {"event":"gate_approved","gate_id":"review-checkpoint","approved_by":"alice"}
//! ```
//!
//! | Field        | Type     | Description                      |
//! |-------------|----------|----------------------------------|
//! | `gate_id`    | `string` | Gate identifier                  |
//! | `approved_by`| `string` | Who or what approved the gate    |
//!
//! ## `run_completed`
//!
//! Emitted when all jobs have finished.
//!
//! ```json
//! {"event":"run_completed","total":847,"succeeded":846,"failed":1,"skipped":0,"duration_ms":8040000}
//! ```
//!
//! | Field         | Type  | Description                        |
//! |--------------|-------|------------------------------------|
//! | `total`       | `u64` | Total number of jobs               |
//! | `succeeded`   | `u64` | Jobs that succeeded                |
//! | `failed`      | `u64` | Jobs that failed                   |
//! | `skipped`     | `u64` | Jobs that were skipped             |
//! | `duration_ms` | `u64` | Total wall-clock duration in ms    |
//!
//! ## `run_failed`
//!
//! Emitted when the run fails due to an unrecoverable error.
//!
//! ```json
//! {"event":"run_failed","error_message":"DAG cycle detected"}
//! ```
//!
//! | Field           | Type     | Description                     |
//! |----------------|----------|---------------------------------|
//! | `error_message` | `string` | Human-readable error            |
//!
//! ## `run_summary` (finish event)
//!
//! Emitted as the final line when `Reporter::finish` is called. This is
//! *not* an `Event` variant — it is synthesized by the reporter to give
//! consumers a single object summarizing the entire run.
//!
//! ```json
//! {"event":"run_summary","total_jobs":100,"succeeded":98,"failed":1,"skipped":1,"duration_ms":60000}
//! ```
//!
//! | Field         | Type  | Description                        |
//! |--------------|-------|------------------------------------|
//! | `total_jobs`  | `u64` | Total jobs in the DAG              |
//! | `succeeded`   | `u64` | Jobs that succeeded                |
//! | `failed`      | `u64` | Jobs that failed                   |
//! | `skipped`     | `u64` | Jobs that were skipped             |
//! | `duration_ms` | `u64` | Total wall-clock duration in ms    |
//!
//! # Forward compatibility
//!
//! STATUS.md §4 promises that **new payload fields may be added at any time**
//! and that **consumers must ignore unknown keys**. The FAIR forward-compat
//! audit (`fair-forward-compat.md`, 2026-06-14) leans on this to conclude that
//! emitting W3C PROV / Workflow RO-Crate in a future release (e.g. attaching
//! wall-clock `started_at`/`ended_at` or output content hashes to `job_*`
//! events) is a non-breaking, additive change. The tests below make that
//! property executable: they confirm the tag-discriminated
//! [`Event`](ox_core::model::Event) enum deserializes successfully even when
//! an unknown future field is present.

#[cfg(test)]
mod forward_compat_tests {
    use ox_core::model::Event;

    /// A `job_completed` line carrying *future* PROV fields (`started_at`,
    /// `ended_at` wall-clock, and `output_hashes`) must still deserialize
    /// against today's [`Event`](ox_core::model::Event) enum — the unknown keys are ignored, not
    /// rejected. This is the load-bearing guarantee behind the audit's
    /// "breaking-change-needed: NO" verdict. If anyone adds
    /// `#[serde(deny_unknown_fields)]` to `Event`, this test goes red.
    #[test]
    fn event_tolerates_unknown_future_fields() {
        let future = r#"{
            "event": "job_completed",
            "job_id": "align_S001",
            "duration_ms": 272000,
            "outputs": ["results/S001.bam"],
            "started_at": "2026-06-14T10:00:00Z",
            "ended_at": "2026-06-14T10:04:32Z",
            "output_hashes": {"results/S001.bam": "abc123"}
        }"#;
        let ev: Event =
            serde_json::from_str(future).expect("unknown future fields must be tolerated");
        assert!(matches!(ev, Event::JobCompleted { .. }));
    }

    /// An *unknown event name* would be matched-and-ignored by a disciplined
    /// consumer (STATUS.md §4 rule 1). We assert the contract from the
    /// producer side: today's enum simply fails to deserialize a future
    /// variant, which is why consumers must branch on the raw `"event"`
    /// string first — documented here so the discipline is not lost.
    #[test]
    fn unknown_event_name_does_not_masquerade() {
        let future = r#"{"event": "artifact_published", "uri": "ro-crate/..."}"#;
        // A future variant is not a current one: strict typed parse fails,
        // confirming consumers must dispatch on the string discriminator.
        assert!(serde_json::from_str::<Event>(future).is_err());
        // But it is still valid JSON a tolerant consumer can skip.
        let raw: serde_json::Value = serde_json::from_str(future).unwrap();
        assert_eq!(raw["event"], "artifact_published");
    }
}
