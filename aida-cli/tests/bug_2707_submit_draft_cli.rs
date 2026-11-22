// Binary-driving e2e suite; Linux-only like its siblings (BUG-688).
#![cfg(target_os = "linux")]
//! BUG-2707: the real `aida review submit-draft`, as a standalone reviewer
//! runs it. A contradictory approval exits non-zero with the remedy and
//! leaves the draft intact on both refusal and acceptance.
// trace:BUG-2707 | ai:claude

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

/// A launch directory as the launcher prepares it, under the default policy.
fn launch_dir(root: &Path) -> PathBuf {
    let dir = root.join("review tree/.aida/review-verdicts/draft-fixture");
    std::fs::create_dir_all(&dir).unwrap();
    let record = serde_json::json!({"version": 1, "pr": 59, "head": HEAD,
        "policy": {"blocking": "unset"}});
    std::fs::write(dir.join("launch.json"), record.to_string()).unwrap();
    dir
}

fn draft(verdict: &str, findings: &[&str]) -> String {
    serde_json::json!({"version": 1, "pr": 59, "reviewed_sha": HEAD, "verdict": verdict,
        "summary": "complete rationale", "findings": findings})
    .to_string()
}

/// Verify validation preserves the reviewer-written file on either outcome.
fn submit(root: &Path, dir: &Path, body: &str) -> Output {
    let path = dir.join("draft.json");
    std::fs::write(&path, body).unwrap();
    let before = path.metadata().unwrap();
    let home = root.join("home");
    let cwd = root.join("elsewhere");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_aida"))
        .args(["review", "submit-draft"])
        .arg(dir)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &home)
        .env("AIDA_HOME", &home)
        .env("AIDA_TELEMETRY", "0")
        .output()
        .expect("run aida");
    let after = path.metadata().unwrap();
    assert_eq!(before.ino(), after.ino());
    assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    output
}

#[test]
fn bug_2707_submit_draft_refuses_then_accepts_and_retains_a_refused_later_attempt() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = launch_dir(tmp.path());
    let path = dir.join("draft.json");

    let contradictory = draft(
        "approved",
        &["P3: rename the helper", "style: wrap the line"],
    );
    let out = submit(tmp.path(), &dir, &contradictory);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("2 unresolved findings"), "{text}");
    assert!(text.contains("set \\\"verdict\\\" to"), "{text}");
    assert!(text.contains("request-changes"), "{text}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), contradictory);

    let fixed = draft(
        "request-changes",
        &["P3: rename the helper", "style: wrap the line"],
    );
    let out = submit(tmp.path(), &dir, &fixed);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{text}");
    assert!(text.contains("reviewer draft accepted"), "{text}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), fixed);
    let out = submit(tmp.path(), &dir, &contradictory);
    assert!(!out.status.success());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), contradictory);
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["draft.json", "launch.json"]);
}
