use std::io::{Error, ErrorKind};
use std::time::{Duration, Instant};

/// Classify a failed `fs2` try-lock without assuming that every platform maps
/// its native contention code to `WouldBlock`.
// trace:BUG-1303 | ai:codex
pub fn is_lock_contended(error: &Error) -> bool {
    is_lock_contended_against(error, &fs2::lock_contended_error())
}

fn is_lock_contended_against(error: &Error, contended: &Error) -> bool {
    error.kind() == ErrorKind::WouldBlock
        || error
            .raw_os_error()
            .zip(contended.raw_os_error())
            .is_some_and(|(actual, expected)| actual == expected)
}

/// Acquire an exclusive lock before the caller's absolute deadline.
// trace:TASK-3884 | ai:codex
pub fn lock_exclusive_until(file: &std::fs::File, deadline: Instant) -> std::io::Result<()> {
    use fs2::FileExt;
    loop {
        if Instant::now() >= deadline {
            return Err(ErrorKind::TimedOut.into());
        }
        match file.try_lock_exclusive() {
            Err(err) if is_lock_contended(&err) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                std::thread::sleep(remaining.min(Duration::from_millis(2)));
            }
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_windows_raw_lock_violation_on_linux_ci() {
        let error = Error::from_raw_os_error(33);
        let windows_contended = Error::from_raw_os_error(33);
        assert!(is_lock_contended_against(&error, &windows_contended));
    }

    #[test]
    fn rejects_non_contention_failures() {
        assert!(!is_lock_contended_against(
            &Error::new(ErrorKind::Interrupted, "signal"),
            &Error::from_raw_os_error(33),
        ));
        assert!(!is_lock_contended_against(
            &Error::new(ErrorKind::PermissionDenied, "denied"),
            &Error::from_raw_os_error(33),
        ));
    }
}
