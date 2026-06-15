//! Terminal reporter with progress bars and colored output.
//!
//! This is the primary human-facing reporter for OxyMake. It renders
//! a rich, color-coded progress display via `indicatif` and `console`,
//! showing running jobs with spinners, throughput, and ETA.

use std::future::Future;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use ox_core::model::Event;
use ox_core::traits::reporter::{Reporter, RunSummary};
use ox_render::Theme;

use crate::format::{format_count, format_duration};

/// A running job tracked for display.
#[allow(dead_code)]
struct ActiveJob {
    job_id: String,
    started: Instant,
}

/// Terminal reporter with progress bars and colored output.
///
/// All output goes to stderr, keeping stdout free for machine-readable
/// output or piping. The reporter is safe to share across threads.
pub struct TermReporter {
    multi: MultiProgress,
    main_bar: ProgressBar,
    /// Per-job spinner bars, shown below the main progress bar.
    active_bars: Mutex<Vec<(ActiveJob, ProgressBar)>>,
    total_jobs: AtomicU64,
    completed: AtomicU64,
    failed: AtomicU64,
    /// Jobs completed from cache (excluded from throughput calculation).
    cached: AtomicU64,
    /// Names of failed jobs (for inline display in progress bar).
    failed_names: Mutex<Vec<String>>,
    /// When the run started (for throughput calculation).
    run_start: Mutex<Option<Instant>>,
    /// Verbosity level: 0=progress only, 1=job lifecycle, 2=stream output
    verbosity: u8,
    /// Directory containing job log files (for log display fallback).
    log_dir: Option<PathBuf>,
    /// Whether real-time output streaming is active (disables post-completion log dump).
    streaming: AtomicBool,
    /// Terminal width for layout.
    term_width: u16,
    /// Whether stderr is a TTY (controls progress bars and spinners).
    is_tty: bool,
    /// Semantic color theme.
    theme: Theme,
}

// Derived styles not in the Theme (combinations of semantic roles).
fn style_start(theme: &Theme) -> console::Style {
    // Yellow bold — same as running but bold for start markers.
    theme.running.clone().bold()
}
fn style_reason(theme: &Theme) -> console::Style {
    theme.muted.clone().italic()
}

impl TermReporter {
    /// Create a new terminal reporter.
    ///
    /// The reporter is inert until [`Reporter::on_event`] is called with
    /// a `RunStarted` event.
    pub fn new() -> Self {
        Self::with_verbosity(0, None)
    }

    /// Create a terminal reporter with the given verbosity level.
    ///
    /// - `verbosity >= 1`: show job start/end with duration and exit codes
    /// - `verbosity >= 2`: stream real-time stdout/stderr via [`Event::JobOutput`]
    ///
    /// `log_dir` is used as a fallback for displaying logs when streaming
    /// is not available.
    pub fn with_verbosity(verbosity: u8, log_dir: Option<PathBuf>) -> Self {
        let theme = Theme::from_env(None, &std::io::stderr());
        Self::with_verbosity_and_theme(verbosity, log_dir, theme)
    }

    /// Create a terminal reporter with explicit theme.
    pub fn with_verbosity_and_theme(verbosity: u8, log_dir: Option<PathBuf>, theme: Theme) -> Self {
        let is_tty = Self::is_tty();
        let multi = MultiProgress::new();
        let bar = multi.add(ProgressBar::hidden());
        let term_width = console::Term::stderr().size().1;
        Self {
            multi,
            main_bar: bar,
            active_bars: Mutex::new(Vec::new()),
            total_jobs: AtomicU64::new(0),
            completed: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            cached: AtomicU64::new(0),
            failed_names: Mutex::new(Vec::new()),
            run_start: Mutex::new(None),
            verbosity,
            log_dir,
            streaming: AtomicBool::new(false),
            term_width,
            is_tty,
            theme,
        }
    }

    /// Returns `true` if stderr is connected to a terminal.
    pub fn is_tty() -> bool {
        std::io::stderr().is_terminal()
    }

    /// Returns a clone of the internal [`MultiProgress`] handle.
    ///
    /// This allows external code (e.g. a signal handler) to clear the
    /// progress bars without owning the reporter.  `MultiProgress` is
    /// internally `Arc`-wrapped, so the clone is cheap and shares state.
    pub fn multi(&self) -> MultiProgress {
        self.multi.clone()
    }

    /// Write a status line to stderr (non-fatal on failure).
    ///
    /// In TTY mode, output is routed through [`MultiProgress::println`] so
    /// that indicatif can suspend and redraw progress bars around the write.
    /// This prevents duplication and interleaving of log lines with the
    /// progress bar (see ox-ssnl).
    fn eprintln(&self, msg: &str) {
        if self.is_tty {
            let _ = self.multi.println(msg);
        } else {
            let mut stderr = std::io::stderr();
            let _ = writeln!(stderr, "{}", msg);
        }
    }

    /// Print the contents of a job's log file to stderr.
    fn print_job_log(&self, job_id: &ox_core::model::JobId) {
        if let Some(ref log_dir) = self.log_dir {
            let log_path = log_dir.join(format!("{}.log", job_id));
            match std::fs::read_to_string(&log_path) {
                Ok(contents) if !contents.is_empty() => {
                    self.eprintln(&format!(
                        "  {} output: {} {}",
                        self.theme.muted.apply_to("┌───"),
                        job_id,
                        self.theme.muted.apply_to("───"),
                    ));
                    for line in contents.lines() {
                        self.eprintln(&format!("  {} {}", self.theme.muted.apply_to("│"), line));
                    }
                    self.eprintln(&format!(
                        "  {} end: {} {}",
                        self.theme.muted.apply_to("└───"),
                        job_id,
                        self.theme.muted.apply_to("───"),
                    ));
                }
                _ => {}
            }
        }
    }

    /// Build the progress message suffix showing failure count and names.
    fn failure_suffix(&self) -> String {
        let fail_count = self.failed.load(Ordering::Relaxed);
        if fail_count == 0 {
            return String::new();
        }
        let names = self.failed_names.lock().unwrap();
        let names_str = if names.len() <= 3 {
            names.join(", ")
        } else {
            format!("{}, \u{2026}", names[..3].join(", "))
        };
        format!(
            " {} {}",
            self.theme
                .error
                .apply_to(format!("| {} failed:", fail_count)),
            self.theme.error.apply_to(&names_str),
        )
    }

    /// Compute throughput string (jobs/sec), excluding cached jobs.
    fn throughput_str(&self) -> String {
        let run_start = self.run_start.lock().unwrap();
        if let Some(start) = *run_start {
            let elapsed = start.elapsed().as_secs_f64();
            let executed = self.executed_count() as f64;
            if elapsed > 0.5 && executed > 0.0 {
                let rate = executed / elapsed;
                return format!(" {:.1} jobs/s", rate);
            }
        }
        String::new()
    }

    /// Compute ETA string based on executed (non-cached) job rate.
    fn eta_str(&self) -> String {
        let run_start = self.run_start.lock().unwrap();
        if let Some(start) = *run_start {
            let elapsed = start.elapsed().as_secs_f64();
            let executed = self.executed_count() as f64;
            let total = self.total_jobs.load(Ordering::Relaxed) as f64;
            let cached = self.cached.load(Ordering::Relaxed) as f64;
            let remaining = total - cached - executed;
            if elapsed > 1.0 && executed > 0.0 && remaining > 0.0 {
                let rate = executed / elapsed;
                let eta_secs = (remaining / rate) as u64;
                let eta_ms = eta_secs * 1000;
                return format!(" ETA {}", format_duration(eta_ms));
            }
        }
        String::new()
    }

    /// Number of jobs that actually executed (completed - cached).
    fn executed_count(&self) -> u64 {
        let completed = self.completed.load(Ordering::Relaxed);
        let cached = self.cached.load(Ordering::Relaxed);
        completed.saturating_sub(cached)
    }

    /// Update the progress bar style based on current failure state.
    ///
    /// Green bar when all jobs pass, red when any have failed.
    fn update_bar_style(&self) {
        let fail_count = self.failed.load(Ordering::Relaxed);
        let bar_color = if fail_count > 0 { "red" } else { "green" };
        let template = format!(
            "  {{bar:30.{bar_color}/dim}} {{pos}}/{{len}} jobs{{msg}} [{{elapsed_precise}}]"
        );
        let style = ProgressStyle::with_template(&template)
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("\u{2588}\u{2588}\u{2591}");
        self.main_bar.set_style(style);
    }

    /// Add a spinner bar for a running job.
    fn add_active_job(&self, job_id: &str) {
        let spinner = self
            .multi
            .insert_after(&self.main_bar, ProgressBar::new_spinner());
        // Truncate job name to fit terminal width.
        let max_name_len = (self.term_width as usize).saturating_sub(20);
        let display_name = if job_id.len() > max_name_len {
            format!("{}...", &job_id[..max_name_len.saturating_sub(3)])
        } else {
            job_id.to_string()
        };
        let style = ProgressStyle::with_template(&format!(
            "  {{spinner:.yellow}} {}",
            self.theme.running.apply_to(&display_name)
        ))
        .unwrap_or_else(|_| ProgressStyle::default_spinner())
        .tick_strings(&[
            "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}",
            "\u{2827}", "\u{2807}", "\u{280f}",
        ]);
        spinner.set_style(style);
        spinner.enable_steady_tick(std::time::Duration::from_millis(80));

        let mut active = self.active_bars.lock().unwrap();
        active.push((
            ActiveJob {
                job_id: job_id.to_string(),
                started: Instant::now(),
            },
            spinner,
        ));
    }

    /// Remove the spinner bar for a completed/failed job.
    fn remove_active_job(&self, job_id: &str) {
        let mut active = self.active_bars.lock().unwrap();
        if let Some(pos) = active.iter().position(|(j, _)| j.job_id == job_id) {
            let (_, bar) = active.remove(pos);
            bar.finish_and_clear();
        }
    }
}

impl Default for TermReporter {
    fn default() -> Self {
        Self::new()
    }
}

impl Reporter for TermReporter {
    fn on_event<'a>(&'a self, event: &'a Event) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            match event {
                Event::RunStarted {
                    total_jobs,
                    to_run,
                    cached,
                } => {
                    let total = *total_jobs as u64;
                    self.total_jobs.store(total, Ordering::Relaxed);
                    {
                        let mut start = self.run_start.lock().unwrap();
                        *start = Some(Instant::now());
                    }

                    let header = format!(
                        "  {} {} jobs ({} to run, {} cached)",
                        self.theme.header.apply_to("Resolving"),
                        self.theme.header.apply_to(format_count(*total_jobs)),
                        self.theme.running.apply_to(format_count(*to_run)),
                        self.theme.cached.apply_to(format_count(*cached)),
                    );
                    self.eprintln(&header);

                    // Configure the progress bar to show the full DAG.
                    // Cached jobs will appear as instant completions.
                    // Only show interactive progress bar in TTY mode.
                    if self.is_tty {
                        self.main_bar.set_length(total);
                        self.main_bar.set_position(0);

                        if total > 0 {
                            self.update_bar_style();
                            self.main_bar.reset();
                            self.main_bar
                                .enable_steady_tick(std::time::Duration::from_millis(200));
                        }
                    }
                }

                Event::JobStarted {
                    job_id,
                    executor,
                    reason,
                } => {
                    if self.is_tty {
                        self.add_active_job(&job_id.to_string());
                        let suffix = self.failure_suffix();
                        let throughput = self.throughput_str();
                        let eta = self.eta_str();
                        self.main_bar.set_message(format!(
                            "{}{}{}",
                            suffix,
                            self.theme.muted.apply_to(&throughput),
                            self.theme.muted.apply_to(&eta),
                        ));
                    }

                    // Display the run reason based on verbosity:
                    // v=0: only "interesting" reasons (OutputMissing, OutputStale, UpstreamRebuilt)
                    // v>=1: all reasons
                    let show_reason = match reason {
                        Some(r) if self.verbosity >= 1 || r.is_interesting() => Some(r),
                        _ => None,
                    };

                    if self.verbosity >= 1 {
                        if let Some(r) = show_reason {
                            self.eprintln(&format!(
                                "  {} {} ({}) \u{2014} {}",
                                style_start(&self.theme).apply_to("[start]"),
                                self.theme.highlight.apply_to(job_id),
                                executor,
                                style_reason(&self.theme).apply_to(r),
                            ));
                        } else {
                            self.eprintln(&format!(
                                "  {} {} ({})",
                                style_start(&self.theme).apply_to("[start]"),
                                self.theme.highlight.apply_to(job_id),
                                executor,
                            ));
                        }
                    } else if let Some(r) = show_reason {
                        // v=0: show a compact start line only for interesting reasons
                        self.eprintln(&format!(
                            "  {} {} \u{2014} {}",
                            self.theme.command.apply_to("\u{25b8}"),
                            self.theme.highlight.apply_to(job_id),
                            style_reason(&self.theme).apply_to(r),
                        ));
                    }
                }

                Event::JobCompleted {
                    job_id,
                    duration_ms,
                    ..
                } => {
                    self.completed.fetch_add(1, Ordering::Relaxed);
                    let completed = self.completed.load(Ordering::Relaxed);

                    if self.is_tty {
                        self.remove_active_job(&job_id.to_string());
                        self.main_bar.set_position(completed);

                        let suffix = self.failure_suffix();
                        let throughput = self.throughput_str();
                        let eta = self.eta_str();
                        self.main_bar.set_message(format!(
                            "{}{}{}",
                            suffix,
                            self.theme.muted.apply_to(&throughput),
                            self.theme.muted.apply_to(&eta),
                        ));
                    }

                    if self.verbosity >= 1 {
                        let total = self.total_jobs.load(Ordering::Relaxed);
                        self.eprintln(&format!(
                            "  [{}/{}] {} {} ({})",
                            completed,
                            total,
                            self.theme.success.apply_to("\u{2713}"),
                            self.theme.highlight.apply_to(job_id),
                            self.theme.muted.apply_to(format_duration(*duration_ms)),
                        ));
                    }
                    if self.verbosity >= 2 && !self.streaming.load(Ordering::Relaxed) {
                        self.print_job_log(job_id);
                    }
                }

                Event::JobFailed {
                    job_id,
                    error_message,
                    exit_code,
                    stderr_tail,
                } => {
                    self.failed.fetch_add(1, Ordering::Relaxed);
                    self.completed.fetch_add(1, Ordering::Relaxed);
                    let completed = self.completed.load(Ordering::Relaxed);
                    let total = self.total_jobs.load(Ordering::Relaxed);

                    // Track the failed job name.
                    {
                        let mut names = self.failed_names.lock().unwrap();
                        names.push(job_id.to_string());
                    }

                    if self.is_tty {
                        self.remove_active_job(&job_id.to_string());
                        self.main_bar.set_position(completed);
                        // Switch bar color to red on first failure.
                        self.update_bar_style();

                        let suffix = self.failure_suffix();
                        let throughput = self.throughput_str();
                        let eta = self.eta_str();
                        self.main_bar.set_message(format!(
                            "{}{}{}",
                            suffix,
                            self.theme.muted.apply_to(&throughput),
                            self.theme.muted.apply_to(&eta),
                        ));
                    }

                    let exit_str = match exit_code {
                        Some(code) => format!(
                            "{}",
                            self.theme.error.apply_to(format!("FAILED (exit {})", code))
                        ),
                        None => format!("{}", self.theme.error.apply_to("FAILED")),
                    };
                    self.eprintln(&format!(
                        "  [{}/{}] {} {} {}",
                        completed,
                        total,
                        self.theme.error.apply_to("\u{2717}"),
                        self.theme.highlight.apply_to(job_id),
                        exit_str,
                    ));

                    // Print the error block.
                    self.eprintln(&format!(
                        "\n  {}: job {} failed: {}",
                        self.theme.error.apply_to("error"),
                        self.theme.highlight.apply_to(job_id),
                        error_message,
                    ));
                    if self.verbosity >= 2 && !self.streaming.load(Ordering::Relaxed) {
                        self.print_job_log(job_id);
                    } else if let Some(stderr) = stderr_tail {
                        for line in stderr.lines() {
                            self.eprintln(&format!(
                                "    {}: {}",
                                self.theme.warning.apply_to("stderr"),
                                line,
                            ));
                        }
                    }
                    // (no trailing blank line — the next event provides its own separator)
                }

                Event::JobSkipped { job_id, reason } => {
                    if reason == "cached" {
                        // Show cached jobs as instant completions in the
                        // progress bar so the user sees the full DAG flow.
                        self.cached.fetch_add(1, Ordering::Relaxed);
                        self.completed.fetch_add(1, Ordering::Relaxed);
                        let completed = self.completed.load(Ordering::Relaxed);

                        if self.is_tty {
                            self.main_bar.set_position(completed);
                        }

                        let total = self.total_jobs.load(Ordering::Relaxed);
                        self.eprintln(&format!(
                            "  [{}/{}] {} {} {}",
                            completed,
                            total,
                            self.theme.cached.apply_to("\u{2713}"),
                            self.theme.highlight.apply_to(job_id),
                            self.theme.cached.apply_to("[cached]"),
                        ));
                    }
                }

                Event::JobCancelled { job_id, reason } => {
                    self.completed.fetch_add(1, Ordering::Relaxed);
                    let completed = self.completed.load(Ordering::Relaxed);

                    if self.is_tty {
                        self.remove_active_job(&job_id.to_string());
                        self.main_bar.set_position(completed);
                    }

                    let total = self.total_jobs.load(Ordering::Relaxed);
                    self.eprintln(&format!(
                        "  [{}/{}] {} {} -- {}",
                        completed,
                        total,
                        self.theme.warning.apply_to("\u{2298}"),
                        self.theme.highlight.apply_to(job_id),
                        self.theme.warning.apply_to(reason),
                    ));
                }

                Event::GateReached { gate_id, message } => {
                    self.eprintln(&format!(
                        "\n  {} {}: {}",
                        style_start(&self.theme).apply_to("GATE"),
                        self.theme.header.apply_to(gate_id),
                        message,
                    ));
                }

                Event::RootCauseDetected {
                    root_cause,
                    failure_count,
                    ..
                } => {
                    self.eprintln(&format!(
                        "\n  {} Detected common root cause across {} failures:",
                        self.theme.error.apply_to("\u{26a0}"),
                        self.theme.header.apply_to(failure_count),
                    ));
                    self.eprintln(&format!("    {}", root_cause));
                }

                Event::ExecutorMessage { message, .. } => {
                    if self.verbosity >= 2 {
                        self.eprintln(&format!(
                            "  {} {}",
                            self.theme.command.apply_to("\u{25b8}"),
                            self.theme.command.apply_to(message),
                        ));
                    }
                }

                Event::JobOutput {
                    job_id,
                    line,
                    stream,
                } if self.verbosity >= 2 => {
                    self.streaming.store(true, Ordering::Relaxed);
                    if *stream == ox_core::model::OutputStream::Stderr {
                        self.eprintln(&format!(
                            "  [{}:{}] {}",
                            self.theme.highlight.apply_to(job_id),
                            self.theme.warning.apply_to("err"),
                            self.theme.warning.apply_to(line),
                        ));
                    } else {
                        self.eprintln(&format!(
                            "  [{}:out] {}",
                            self.theme.highlight.apply_to(job_id),
                            line,
                        ));
                    }
                }

                _ => {}
            }
        })
    }

    fn finish<'a>(
        &'a self,
        summary: &'a RunSummary,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            // Clear all active spinners.
            if self.is_tty {
                {
                    let mut active = self.active_bars.lock().unwrap();
                    for (_, bar) in active.drain(..) {
                        bar.finish_and_clear();
                    }
                }
                self.main_bar.finish_and_clear();
            }

            let mut parts = Vec::new();
            if summary.succeeded > 0 {
                parts.push(format!(
                    "{}",
                    self.theme
                        .success
                        .apply_to(format!("{} succeeded", summary.succeeded)),
                ));
            }
            if summary.failed > 0 {
                parts.push(format!(
                    "{}",
                    self.theme
                        .error
                        .apply_to(format!("{} failed", summary.failed)),
                ));
            }
            if summary.skipped > 0 {
                parts.push(format!(
                    "{}",
                    self.theme
                        .muted
                        .apply_to(format!("{} skipped", summary.skipped)),
                ));
            }

            // Compute throughput for the summary.
            let elapsed_secs = summary.duration_ms as f64 / 1000.0;
            let total_done = (summary.succeeded + summary.failed + summary.skipped) as f64;
            let throughput = if elapsed_secs > 0.0 {
                format!(" ({:.1} jobs/s)", total_done / elapsed_secs)
            } else {
                String::new()
            };

            let status_icon = if summary.failed == 0 {
                self.theme.success.apply_to("\u{2713}").to_string()
            } else {
                self.theme.error.apply_to("\u{2717}").to_string()
            };

            self.eprintln(&format!(
                "  {} Completed {}/{} in {}{}",
                status_icon,
                summary.succeeded + summary.failed + summary.skipped,
                summary.total_jobs,
                format_duration(summary.duration_ms),
                self.theme.muted.apply_to(&throughput),
            ));
            self.eprintln(&format!("    {}", parts.join(", ")));

            // Show failed job names in the summary if any.
            let names = self.failed_names.lock().unwrap();
            if !names.is_empty() {
                self.eprintln(&format!(
                    "  {} {}",
                    self.theme.error.apply_to("Failed:"),
                    names.join(", "),
                ));
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_does_not_panic() {
        let _reporter = TermReporter::new();
    }

    #[test]
    fn default_does_not_panic() {
        let _reporter = TermReporter::default();
    }

    #[tokio::test]
    async fn run_started_sets_total_to_full_dag() {
        let reporter = TermReporter::new();
        let event = Event::RunStarted {
            total_jobs: 100,
            to_run: 50,
            cached: 50,
        };
        reporter.on_event(&event).await;
        // Progress bar should reflect the full DAG, not just to_run.
        assert_eq!(reporter.total_jobs.load(Ordering::Relaxed), 100);
    }

    #[tokio::test]
    async fn job_completed_increments() {
        let reporter = TermReporter::new();
        reporter.total_jobs.store(10, Ordering::Relaxed);

        let event = Event::JobCompleted {
            job_id: "j1".into(),
            duration_ms: 1234,
            outputs: vec![],
        };
        reporter.on_event(&event).await;
        assert_eq!(reporter.completed.load(Ordering::Relaxed), 1);
        assert_eq!(reporter.failed.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn job_failed_increments_both() {
        let reporter = TermReporter::new();
        reporter.total_jobs.store(10, Ordering::Relaxed);

        let event = Event::JobFailed {
            job_id: "j1".into(),
            error_message: "oom".to_string(),
            exit_code: Some(1),
            stderr_tail: None,
        };
        reporter.on_event(&event).await;
        assert_eq!(reporter.completed.load(Ordering::Relaxed), 1);
        assert_eq!(reporter.failed.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn job_failed_tracks_names() {
        let reporter = TermReporter::new();
        reporter.total_jobs.store(10, Ordering::Relaxed);

        let event = Event::JobFailed {
            job_id: "compile-foo".into(),
            error_message: "oom".to_string(),
            exit_code: Some(1),
            stderr_tail: None,
        };
        reporter.on_event(&event).await;

        let names = reporter.failed_names.lock().unwrap();
        assert_eq!(names.len(), 1);
        assert_eq!(names[0], "compile-foo");
    }

    #[test]
    fn failure_suffix_empty_when_no_failures() {
        let reporter = TermReporter::new();
        assert_eq!(reporter.failure_suffix(), "");
    }

    #[test]
    fn failure_suffix_shows_names() {
        let reporter = TermReporter::new();
        reporter.failed.store(2, Ordering::Relaxed);
        {
            let mut names = reporter.failed_names.lock().unwrap();
            names.push("job-a".into());
            names.push("job-b".into());
        }
        let suffix = reporter.failure_suffix();
        assert!(suffix.contains("2 failed"));
        assert!(suffix.contains("job-a"));
        assert!(suffix.contains("job-b"));
    }

    #[test]
    fn failure_suffix_truncates_at_three() {
        let reporter = TermReporter::new();
        reporter.failed.store(5, Ordering::Relaxed);
        {
            let mut names = reporter.failed_names.lock().unwrap();
            for i in 0..5 {
                names.push(format!("job-{}", i));
            }
        }
        let suffix = reporter.failure_suffix();
        assert!(suffix.contains("5 failed"));
        assert!(suffix.contains("\u{2026}"));
    }

    #[test]
    fn is_tty_returns_bool() {
        let _ = TermReporter::is_tty();
    }

    /// Regression test for ox-2sek: `multi()` must return a clone that
    /// can clear bars externally (used by the SIGINT handler).
    #[test]
    fn multi_clone_allows_external_clear() {
        let reporter = TermReporter::new();
        let multi = reporter.multi();
        // Clearing an empty MultiProgress must not panic.
        multi.clear().unwrap();
    }

    #[tokio::test]
    async fn active_jobs_tracked() {
        let reporter = TermReporter::new();
        reporter.add_active_job("build-x");
        reporter.add_active_job("test-y");
        {
            let active = reporter.active_bars.lock().unwrap();
            assert_eq!(active.len(), 2);
        }
        reporter.remove_active_job("build-x");
        {
            let active = reporter.active_bars.lock().unwrap();
            assert_eq!(active.len(), 1);
            assert_eq!(active[0].0.job_id, "test-y");
        }
    }

    #[tokio::test]
    async fn run_started_records_start_time() {
        let reporter = TermReporter::new();
        let event = Event::RunStarted {
            total_jobs: 10,
            to_run: 5,
            cached: 5,
        };
        reporter.on_event(&event).await;
        let start = reporter.run_start.lock().unwrap();
        assert!(start.is_some());
    }

    #[tokio::test]
    async fn cached_job_increments_progress() {
        let reporter = TermReporter::new();
        reporter.total_jobs.store(10, Ordering::Relaxed);

        let event = Event::JobSkipped {
            job_id: "data".into(),
            reason: "cached".into(),
        };
        reporter.on_event(&event).await;
        assert_eq!(reporter.completed.load(Ordering::Relaxed), 1);
        assert_eq!(reporter.cached.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn non_cached_skip_does_not_increment_progress() {
        let reporter = TermReporter::new();
        reporter.total_jobs.store(10, Ordering::Relaxed);

        let event = Event::JobSkipped {
            job_id: "build".into(),
            reason: "downstream of failure".into(),
        };
        reporter.on_event(&event).await;
        assert_eq!(reporter.completed.load(Ordering::Relaxed), 0);
        assert_eq!(reporter.cached.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn executed_count_excludes_cached() {
        let reporter = TermReporter::new();
        reporter.completed.store(5, Ordering::Relaxed);
        reporter.cached.store(2, Ordering::Relaxed);
        assert_eq!(reporter.executed_count(), 3);
    }

    #[tokio::test]
    async fn full_dag_progress_flow() {
        let reporter = TermReporter::new();

        // 10 total jobs, 3 cached, 7 to run.
        let start = Event::RunStarted {
            total_jobs: 10,
            to_run: 7,
            cached: 3,
        };
        reporter.on_event(&start).await;
        assert_eq!(reporter.total_jobs.load(Ordering::Relaxed), 10);

        // 3 cached jobs complete instantly.
        for name in &["data-a", "data-b", "data-c"] {
            let ev = Event::JobSkipped {
                job_id: (*name).into(),
                reason: "cached".into(),
            };
            reporter.on_event(&ev).await;
        }
        assert_eq!(reporter.completed.load(Ordering::Relaxed), 3);
        assert_eq!(reporter.cached.load(Ordering::Relaxed), 3);

        // 2 real jobs complete.
        for name in &["build-x", "build-y"] {
            let ev = Event::JobCompleted {
                job_id: (*name).into(),
                duration_ms: 500,
                outputs: vec![],
            };
            reporter.on_event(&ev).await;
        }
        assert_eq!(reporter.completed.load(Ordering::Relaxed), 5);
        assert_eq!(reporter.cached.load(Ordering::Relaxed), 3);
        assert_eq!(reporter.executed_count(), 2);
    }

    /// Regression test for ox-q0hl: TermReporter must process events even
    /// when is_tty is false (non-TTY mode: piped, cron, CI).
    #[tokio::test]
    async fn non_tty_events_still_processed() {
        let reporter = TermReporter::new();
        // Force non-TTY mode (tests run with stderr redirected, so is_tty
        // is already false, but be explicit).
        assert!(!reporter.is_tty, "test must run with non-TTY stderr");

        let start = Event::RunStarted {
            total_jobs: 3,
            to_run: 2,
            cached: 1,
        };
        reporter.on_event(&start).await;
        assert_eq!(reporter.total_jobs.load(Ordering::Relaxed), 3);

        // Cached job.
        let skip = Event::JobSkipped {
            job_id: "data".into(),
            reason: "cached".into(),
        };
        reporter.on_event(&skip).await;
        assert_eq!(reporter.cached.load(Ordering::Relaxed), 1);
        assert_eq!(reporter.completed.load(Ordering::Relaxed), 1);

        // Real job completes.
        let done = Event::JobCompleted {
            job_id: "build".into(),
            duration_ms: 500,
            outputs: vec![],
        };
        reporter.on_event(&done).await;
        assert_eq!(reporter.completed.load(Ordering::Relaxed), 2);

        // Failed job.
        let fail = Event::JobFailed {
            job_id: "test".into(),
            error_message: "assertion failed".into(),
            exit_code: Some(1),
            stderr_tail: Some("assert_eq failed".into()),
        };
        reporter.on_event(&fail).await;
        assert_eq!(reporter.completed.load(Ordering::Relaxed), 3);
        assert_eq!(reporter.failed.load(Ordering::Relaxed), 1);

        // finish() must not panic in non-TTY mode.
        let summary = RunSummary {
            total_jobs: 3,
            succeeded: 1,
            failed: 1,
            skipped: 1,
            duration_ms: 600,
        };
        reporter.finish(&summary).await;
    }
}
