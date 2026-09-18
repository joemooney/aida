//! Retry helpers for transient process spawn failures.
//!
//! On Unix, executing a file fails with `ETXTBSY` while any process holds it
//! open for writing. A concurrent fork can briefly inherit that descriptor,
//! so retrying the spawn handles the race for both real commands and parallel
//! test fixtures. trace:BUG-463 trace:BUG-468 | ai:claude

const ETXTBSY_MAX_RETRIES: usize = 5;
const ETXTBSY_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(50);

#[cfg(unix)]
fn is_etxtbsy(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(libc::ETXTBSY)
}

#[cfg(not(unix))]
fn is_etxtbsy(_error: &std::io::Error) -> bool {
    false
}

// trace:BUG-1202 | ai:codex
fn retrying_etxtbsy<T>(mut operation: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    for retry in 0..=ETXTBSY_MAX_RETRIES {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) if is_etxtbsy(&error) && retry < ETXTBSY_MAX_RETRIES => {
                std::thread::sleep(ETXTBSY_RETRY_DELAY);
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("bounded retry loop always returns")
}

pub(crate) fn command_output_retrying_etxtbsy(
    command: &mut std::process::Command,
) -> std::io::Result<std::process::Output> {
    retrying_etxtbsy(|| command.output())
}

pub(crate) fn command_status_retrying_etxtbsy(
    command: &mut std::process::Command,
) -> std::io::Result<std::process::ExitStatus> {
    retrying_etxtbsy(|| command.status())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn retry_helper_retries_etxtbsy_until_success() {
        let mut attempts = 0;
        let result = retrying_etxtbsy(|| {
            attempts += 1;
            if attempts <= 2 {
                Err(std::io::Error::from_raw_os_error(libc::ETXTBSY))
            } else {
                Ok("started")
            }
        });

        assert_eq!(result.unwrap(), "started");
        assert_eq!(attempts, 3);
    }
}
