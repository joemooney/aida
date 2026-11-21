// Binary-driving e2e suite; Linux-only like its siblings (BUG-688).
#![cfg(target_os = "linux")]
//! BUG-2707: the real `aida review submit-draft`, as a standalone reviewer
//! runs it. A contradictory approval exits non-zero with the remedy and
//! accepts nothing; the corrected draft is accepted; a refused later attempt
//! is retained beside the earlier accepted draft. The command needs no
//! project, identity or working directory: only the launch directory.
// trace:BUG-2707 | ai:claude

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

/// Write `body` as the pending draft and run the submission on `dir`.
fn submit(root: &Path, dir: &Path, body: &str) -> Output {
    std::fs::write(dir.join("pending.json"), body).unwrap();
    let home = root.join("home");
    let cwd = root.join("elsewhere");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    Command::new(env!("CARGO_BIN_EXE_aida"))
        .args(["review", "submit-draft"])
        .arg(dir)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", &home)
        .env("AIDA_HOME", &home)
        .env("AIDA_TELEMETRY", "0")
        .output()
        .expect("run aida")
}

#[test]
fn bug_2707_submit_draft_refuses_then_accepts_and_retains_a_refused_later_attempt() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = launch_dir(tmp.path());
    let accepted = dir.join("draft.json");
    let pending = dir.join("pending.json");

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
    assert!(!accepted.exists(), "a refusal accepts nothing");
    assert_eq!(std::fs::read_to_string(&pending).unwrap(), contradictory);

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
    assert_eq!(std::fs::read_to_string(&accepted).unwrap(), fixed);
    assert!(!pending.exists(), "the accepted draft is consumed");

    // A later refused attempt leaves the accepted draft as it was and stays
    // in place, where the launcher's collection refuses until it is resolved.
    let out = submit(tmp.path(), &dir, &contradictory);
    assert!(!out.status.success());
    assert_eq!(std::fs::read_to_string(&accepted).unwrap(), fixed);
    assert_eq!(std::fs::read_to_string(&pending).unwrap(), contradictory);
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["draft.json", "launch.json", "pending.json"]);
}
