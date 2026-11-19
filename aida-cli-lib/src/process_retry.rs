// trace:TASK-3783 | ai:codex
pub use aida_term::process_retry::*;

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
}
