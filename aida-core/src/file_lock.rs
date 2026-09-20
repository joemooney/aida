use std::io::{Error, ErrorKind};

/// Classification for errors returned by `fs2`'s non-blocking lock methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryLockError {
    /// Another holder currently owns the requested lock.
    Contended,
    /// The system call was interrupted and can be attempted again.
    Interrupted,
    /// A genuine lock failure that should be surfaced to the caller.
    Other,
}

/// Classify a failed `fs2` try-lock without assuming that every platform maps
/// its native contention code to `WouldBlock`.
// trace:BUG-1303 | ai:codex
pub fn classify_try_lock_error(error: &Error) -> TryLockError {
    classify_try_lock_error_against(error, &fs2::lock_contended_error())
}

fn classify_try_lock_error_against(error: &Error, contended: &Error) -> TryLockError {
    if error.kind() == ErrorKind::WouldBlock
        || error
            .raw_os_error()
            .zip(contended.raw_os_error())
            .is_some_and(|(actual, expected)| actual == expected)
    {
        TryLockError::Contended
    } else if error.kind() == ErrorKind::Interrupted {
        TryLockError::Interrupted
    } else {
        TryLockError::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_windows_raw_lock_violation_on_linux_ci() {
        let error = Error::from_raw_os_error(33);
        let windows_contended = Error::from_raw_os_error(33);
        assert_eq!(
            classify_try_lock_error_against(&error, &windows_contended),
            TryLockError::Contended
        );
    }

    #[test]
    fn distinguishes_interrupted_and_genuine_failures() {
        assert_eq!(
            classify_try_lock_error_against(
                &Error::new(ErrorKind::Interrupted, "signal"),
                &Error::from_raw_os_error(33),
            ),
            TryLockError::Interrupted
        );
        assert_eq!(
            classify_try_lock_error_against(
                &Error::new(ErrorKind::PermissionDenied, "denied"),
                &Error::from_raw_os_error(33),
            ),
            TryLockError::Other
        );
    }
}
