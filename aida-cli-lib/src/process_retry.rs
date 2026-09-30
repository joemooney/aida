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

/// The same bounded `ETXTBSY` retry as the free functions above, reachable as a
/// method so an existing builder chain is routed by renaming its terminal call
/// (`.output()` -> `.output_retrying_etxtbsy()`) instead of being wrapped in a
/// function call and re-indented.
///
/// Implemented for [`std::process::Command`] and nothing else, so applying the
/// rename to some other type's `.output()` fails to compile rather than quietly
/// changing behaviour. `spawn_retrying_etxtbsy` is the method the free functions
/// never had: without it a `.spawn()` site has no remedy at all.
// trace:BUG-1735 | ai:claude
pub(crate) trait RetryEtxtbsy {
    fn output_retrying_etxtbsy(&mut self) -> std::io::Result<std::process::Output>;
    fn status_retrying_etxtbsy(&mut self) -> std::io::Result<std::process::ExitStatus>;
    fn spawn_retrying_etxtbsy(&mut self) -> std::io::Result<std::process::Child>;
}

impl RetryEtxtbsy for std::process::Command {
    fn output_retrying_etxtbsy(&mut self) -> std::io::Result<std::process::Output> {
        retrying_etxtbsy(|| self.output())
    }

    fn status_retrying_etxtbsy(&mut self) -> std::io::Result<std::process::ExitStatus> {
        retrying_etxtbsy(|| self.status())
    }

    fn spawn_retrying_etxtbsy(&mut self) -> std::io::Result<std::process::Child> {
        retrying_etxtbsy(|| self.spawn())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real `ETXTBSY`, not a synthesised `io::Error`: Linux refuses to exec a
    /// file while any writer descriptor is open on it, so holding one open and
    /// dropping it from another thread proves the *method* waits the window out.
    /// Without the retry the very first `spawn` returns `ETXTBSY` and this fails.
    // trace:BUG-1735 | ai:claude
    #[cfg(target_os = "linux")]
    #[test]
    fn spawn_method_waits_out_a_live_writer_descriptor() {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("fixture");
        crate::test_exec::write_executable(&executable, "#!/bin/sh\nexit 7\n");

        let mut writer = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(&executable)
            .unwrap();
        writer.write_all(b"#!/bin/sh\nexit 7\n").unwrap();
        writer.sync_all().unwrap();

        // Confirm the window is genuinely open before relying on the retry:
        // a bare spawn right now must fail with ETXTBSY.
        let bare = std::process::Command::new(&executable).spawn();
        assert_eq!(
            bare.err().and_then(|e| e.raw_os_error()),
            Some(libc::ETXTBSY),
            "the writer descriptor must make a bare spawn fail, or this test proves nothing"
        );

        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(80));
            drop(writer);
        });

        let mut child = std::process::Command::new(&executable)
            .spawn_retrying_etxtbsy()
            .expect("spawn must retry past the writer descriptor");
        let status = child.wait().unwrap();
        release.join().unwrap();
        assert_eq!(status.code(), Some(7));
    }

    /// `status_retrying_etxtbsy` and `output_retrying_etxtbsy` forward to the same
    /// bounded loop; assert they are wired to the command at all (a method that
    /// ignored `self` would still compile).
    // trace:BUG-1735 | ai:claude
    #[cfg(unix)]
    #[test]
    fn output_and_status_methods_run_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("fixture");
        crate::test_exec::write_executable(&executable, "#!/bin/sh\necho hi\nexit 3\n");

        let out = std::process::Command::new(&executable)
            .output_retrying_etxtbsy()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi");
        assert_eq!(out.status.code(), Some(3));

        let status = std::process::Command::new(&executable)
            .stdout(std::process::Stdio::null())
            .status_retrying_etxtbsy()
            .unwrap();
        assert_eq!(status.code(), Some(3));
    }

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
