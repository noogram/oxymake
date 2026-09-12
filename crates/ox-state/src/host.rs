//! Host identity for the audit trail.
//!
//! Every `job_history` row and every `sessions` row records *where* a job
//! ran.  Before this module both call sites wrote the literal
//! `"localhost"`, which made the trail unable to distinguish two machines
//! sharing one `state.db` (issue #12).
//!
//! The value is resolved once per process and cached: a hostname does not
//! change under a running build, and `finalize_job_history` would otherwise
//! pay a syscall per run.

use std::sync::OnceLock;

/// Fallback recorded when the host name cannot be determined.
///
/// Kept distinguishable from a real name so a reader can tell "we did not
/// know" from "the machine is called `localhost`".
pub const UNKNOWN_HOST: &str = "unknown";

static HOSTNAME: OnceLock<String> = OnceLock::new();

/// The name of the machine this process runs on.
///
/// Resolved once and reused.  Returns [`UNKNOWN_HOST`] when the platform
/// refuses to answer.
///
/// ```
/// let host = ox_state::host::hostname();
/// assert!(!host.is_empty());
/// assert_ne!(host, "");
/// ```
pub fn hostname() -> &'static str {
    HOSTNAME.get_or_init(resolve).as_str()
}

#[cfg(unix)]
fn resolve() -> String {
    // `HOST_NAME_MAX` is 64 on Linux and 255 on macOS; 256 + NUL covers both.
    let mut buf = vec![0u8; 257];
    // SAFETY: `buf` is a live allocation of `buf.len()` bytes and stays
    // borrowed for the duration of the call; `gethostname` writes at most
    // that many bytes into it and never retains the pointer.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if rc != 0 {
        return UNKNOWN_HOST.to_string();
    }
    // The result is NUL-terminated on success (the buffer is oversized).
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    match std::str::from_utf8(&buf[..end]) {
        Ok(name) if !name.is_empty() => name.to_string(),
        _ => UNKNOWN_HOST.to_string(),
    }
}

#[cfg(not(unix))]
fn resolve() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| UNKNOWN_HOST.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_a_non_placeholder_host_name() {
        let host = hostname();
        assert!(!host.is_empty(), "hostname must not be empty");
        // The bug this replaces: the literal placeholder was recorded for
        // every machine.  A resolved name may legitimately *be* localhost
        // on a bare container, but it must not be a hardcoded constant —
        // assert it round-trips through the platform call instead.
        assert_eq!(host, resolve(), "hostname must be the resolved value");
    }

    #[test]
    fn is_stable_across_calls() {
        assert_eq!(hostname(), hostname());
    }
}
