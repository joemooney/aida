//! Writer diagnostics and cleanup used by intake journal recovery.
// trace:TASK-3096 | ai:codex
use super::*;

#[test]
fn writer_refusal_captures_stderr_and_exit_status() {
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", "printf refused >&2; exit 7"]);
    let (status, stdout, stderr) =
        bounded_with_stderr(cmd, &[], 1024, Instant::now() + Duration::from_secs(2)).unwrap();
    assert_eq!(status.code(), Some(7));
    assert!(stdout.is_empty());
    assert_eq!(stderr, b"refused");
}

#[test]
fn writer_timeout_preserves_diagnostics_and_reaps_the_group() {
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", "printf waiting >&2; exec sleep 30"]);
    let error = bounded_with_stderr(cmd, &[], 1024, Instant::now() + Duration::from_millis(200))
        .unwrap_err();
    assert!(error.to_string().contains("deadline exceeded"));
    assert!(error.to_string().contains("waiting"));
}

#[test]
fn writer_diagnostic_overflow_and_spawn_failure_return_promptly() {
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", "printf too-long >&2; exec sleep 30"]);
    let error =
        bounded_with_stderr(cmd, &[], 3, Instant::now() + Duration::from_millis(200)).unwrap_err();
    assert!(error.to_string().contains("limit"));
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("contains\0nul");
    assert!(bounded_with_stderr(cmd, &[], 1024, Instant::now() + Duration::from_secs(2)).is_err());
}
