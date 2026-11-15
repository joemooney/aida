//! Exercise the real --status dispatch with a stalled worktree lookup.
// trace:STORY-2823 | ai:codex
#![cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn status_entry_bounds_stalled_worktree_lookup() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir(root.join(".git")).unwrap();
    std::fs::create_dir(root.join(".aida")).unwrap();
    let bin = root.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let marker = root.join("looked-up");
    let git = bin.join("git");
    std::fs::write(
        &git,
        r#"#!/bin/sh
case "$*" in
  *'worktree list --porcelain -z')
    echo lookup > "$LOOKUP_MARKER"
    exec /bin/sleep 30 ;;
  *) exit 1 ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
    for json in [false, true] {
        let started = Instant::now();
        let mut command = Command::new(env!("CARGO_BIN_EXE_aida"));
        command
            .current_dir(root)
            .args(["pr", "train", "--status"])
            .env("PATH", &bin)
            .env("HOME", root)
            .env("AIDA_TELEMETRY", "0")
            .env("LOOKUP_MARKER", &marker)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for name in [
            "AIDA_STORE",
            "AIDA_PROJECT_ROOT",
            "AIDA_DRIVE_ROOT",
            "GIT_DIR",
            "GIT_WORK_TREE",
        ] {
            command.env_remove(name);
        }
        if json {
            command.arg("--json");
        }
        let mut child = command.spawn().unwrap();
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if started.elapsed() > Duration::from_secs(10) {
                child.kill().unwrap();
                let _ = child.wait();
                panic!("status entry exceeded its watchdog");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let output = child.wait_with_output().unwrap();
        let diagnostic = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            marker.exists(),
            "did not exercise worktree lookup: {diagnostic}"
        );
        assert!(!output.status.success());
        assert!(
            diagnostic.contains("train status worktree lookup failed or exceeded"),
            "{diagnostic}"
        );
        assert!(
            !diagnostic.contains("No train") && !diagnostic.contains("\"state\": \"none\""),
            "must not report a false no-train snapshot"
        );
        assert!(started.elapsed() < Duration::from_secs(8));
        std::fs::remove_file(&marker).unwrap();
    }
}
