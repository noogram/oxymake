//! Formatting helpers for the TUI.

use std::time::Duration;

/// Format a [`Duration`] as `Xm YYs` (e.g., `3m 12s`).
///
/// Durations under one minute are shown as `0m XXs`.
///
/// ```
/// use std::time::Duration;
/// use ox_monitor_tui::format::fmt_duration;
///
/// assert_eq!(fmt_duration(Duration::from_secs(192)), "3m 12s");
/// assert_eq!(fmt_duration(Duration::from_secs(34)), "0m 34s");
/// assert_eq!(fmt_duration(Duration::from_secs(0)), "0m 00s");
/// assert_eq!(fmt_duration(Duration::from_secs(3661)), "61m 01s");
/// ```
pub fn fmt_duration(d: Duration) -> String {
    let total_secs = d.as_secs();
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    format!("{mins}m {secs:02}s")
}

/// Format a progress fraction as a percentage string.
///
/// ```
/// use ox_monitor_tui::format::fmt_percent;
///
/// assert_eq!(fmt_percent(0.083), "8.3%");
/// assert_eq!(fmt_percent(1.0), "100.0%");
/// assert_eq!(fmt_percent(0.0), "0.0%");
/// ```
pub fn fmt_percent(fraction: f64) -> String {
    format!("{:.1}%", fraction * 100.0)
}

/// Format a completed/total count with percentage.
///
/// ```
/// use ox_monitor_tui::format::fmt_progress;
///
/// assert_eq!(fmt_progress(847, 10247), "847/10247 (8.3%)");
/// assert_eq!(fmt_progress(0, 0), "0/0 (0.0%)");
/// ```
pub fn fmt_progress(done: usize, total: usize) -> String {
    let pct = if total == 0 {
        0.0
    } else {
        done as f64 / total as f64
    };
    format!("{done}/{total} ({})", fmt_percent(pct))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_formatting() {
        assert_eq!(fmt_duration(Duration::from_secs(0)), "0m 00s");
        assert_eq!(fmt_duration(Duration::from_secs(59)), "0m 59s");
        assert_eq!(fmt_duration(Duration::from_secs(60)), "1m 00s");
        assert_eq!(fmt_duration(Duration::from_secs(192)), "3m 12s");
    }

    #[test]
    fn percent_formatting() {
        assert_eq!(fmt_percent(0.0), "0.0%");
        assert_eq!(fmt_percent(0.5), "50.0%");
        assert_eq!(fmt_percent(1.0), "100.0%");
    }

    #[test]
    fn progress_formatting() {
        assert_eq!(fmt_progress(847, 10247), "847/10247 (8.3%)");
        assert_eq!(fmt_progress(3, 3), "3/3 (100.0%)");
        assert_eq!(fmt_progress(0, 0), "0/0 (0.0%)");
    }
}
