//! Formatting helpers for terminal output.
//!
//! Pure functions that convert raw values into human-friendly strings.
//! These are the testable core of the terminal reporter.

use std::collections::BTreeMap;

/// Format a duration in milliseconds into a compact human-readable string.
///
/// - Under 1 second: "0.3s"
/// - Under 1 minute: "12s"
/// - Under 1 hour: "4m32s"
/// - 1 hour and above: "2h14m"
///
/// # Examples
///
/// ```
/// use ox_report_term::format::format_duration;
/// assert_eq!(format_duration(300), "0.3s");
/// assert_eq!(format_duration(12000), "12s");
/// assert_eq!(format_duration(272000), "4m32s");
/// assert_eq!(format_duration(8040000), "2h14m");
/// ```
pub fn format_duration(ms: u64) -> String {
    let total_secs = ms / 1000;
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let secs = total_secs % 60;

    if total_secs == 0 {
        // Sub-second: show one decimal place
        let tenths = (ms + 50) / 100; // round to nearest tenth
        if tenths == 0 {
            // Very small durations: show as <0.1s
            "<0.1s".to_string()
        } else if tenths >= 10 {
            // Rounded up to 1.0s
            "1s".to_string()
        } else {
            format!("0.{}s", tenths)
        }
    } else if total_secs < 60 {
        format!("{}s", total_secs)
    } else if hours == 0 {
        format!("{}m{:02}s", minutes, secs)
    } else {
        format!("{}h{:02}m", hours, minutes)
    }
}

/// Format a job name from a rule name and optional wildcards.
///
/// - No wildcards: just the rule name, e.g. "align"
/// - With wildcards: "align (sample=S001)"
/// - Multiple wildcards: "align (sample=S001, lane=L01)" (sorted by key)
///
/// # Examples
///
/// ```
/// use std::collections::BTreeMap;
/// use ox_report_term::format::format_job_name;
///
/// assert_eq!(format_job_name("align", &BTreeMap::new()), "align");
///
/// let mut wc = BTreeMap::new();
/// wc.insert("sample".to_string(), "S001".to_string());
/// assert_eq!(format_job_name("align", &wc), "align (sample=S001)");
/// ```
pub fn format_job_name(rule: &str, wildcards: &BTreeMap<String, String>) -> String {
    if wildcards.is_empty() {
        rule.to_string()
    } else {
        let pairs: Vec<String> = wildcards
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();
        format!("{} ({})", rule, pairs.join(", "))
    }
}

/// Format a count with thousands separators (commas).
///
/// # Examples
///
/// ```
/// use ox_report_term::format::format_count;
/// assert_eq!(format_count(0), "0");
/// assert_eq!(format_count(999), "999");
/// assert_eq!(format_count(1000), "1,000");
/// assert_eq!(format_count(103429), "103,429");
/// assert_eq!(format_count(1000000), "1,000,000");
/// ```
pub fn format_count(n: usize) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let len = bytes.len();
    if len <= 3 {
        return s;
    }

    let mut result = String::with_capacity(len + (len - 1) / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            result.push(',');
        }
        result.push(b as char);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- format_duration ----

    #[test]
    fn duration_zero() {
        assert_eq!(format_duration(0), "<0.1s");
    }

    #[test]
    fn duration_sub_second() {
        assert_eq!(format_duration(300), "0.3s");
        assert_eq!(format_duration(50), "0.1s"); // rounds up
        assert_eq!(format_duration(100), "0.1s"); // rounds to nearest tenth
        assert_eq!(format_duration(999), "1s"); // rounds to 1s
    }

    #[test]
    fn duration_seconds() {
        assert_eq!(format_duration(1000), "1s");
        assert_eq!(format_duration(12000), "12s");
        assert_eq!(format_duration(59000), "59s");
    }

    #[test]
    fn duration_minutes() {
        assert_eq!(format_duration(60000), "1m00s");
        assert_eq!(format_duration(272000), "4m32s");
        assert_eq!(format_duration(258000), "4m18s");
    }

    #[test]
    fn duration_hours() {
        assert_eq!(format_duration(3600000), "1h00m");
        assert_eq!(format_duration(8040000), "2h14m");
    }

    // ---- format_job_name ----

    #[test]
    fn job_name_no_wildcards() {
        assert_eq!(format_job_name("align", &BTreeMap::new()), "align");
    }

    #[test]
    fn job_name_one_wildcard() {
        let mut wc = BTreeMap::new();
        wc.insert("sample".to_string(), "S001".to_string());
        assert_eq!(format_job_name("align", &wc), "align (sample=S001)");
    }

    #[test]
    fn job_name_multiple_wildcards() {
        let mut wc = BTreeMap::new();
        wc.insert("sample".to_string(), "S001".to_string());
        wc.insert("lane".to_string(), "L01".to_string());
        // BTreeMap sorts by key: lane < sample
        assert_eq!(
            format_job_name("align", &wc),
            "align (lane=L01, sample=S001)"
        );
    }

    // ---- format_count ----

    #[test]
    fn count_zero() {
        assert_eq!(format_count(0), "0");
    }

    #[test]
    fn count_small() {
        assert_eq!(format_count(42), "42");
        assert_eq!(format_count(999), "999");
    }

    #[test]
    fn count_thousands() {
        assert_eq!(format_count(1000), "1,000");
        assert_eq!(format_count(1234), "1,234");
    }

    #[test]
    fn count_large() {
        assert_eq!(format_count(103429), "103,429");
        assert_eq!(format_count(1000000), "1,000,000");
        assert_eq!(format_count(1234567890), "1,234,567,890");
    }
}
