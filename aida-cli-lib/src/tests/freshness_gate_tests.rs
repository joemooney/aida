// Tests for the wave-launch binary-freshness gate and the point-of-use
// staleness warning. trace:STORY-1414 trace:TASK-188 | ai:claude

use super::*;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?} failed: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, file: &str) -> String {
    std::fs::write(dir.join(file), file).unwrap();
    git(dir, &["add", "-A"]);
    git(
        dir,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            file,
        ],
    );
    git(dir, &["rev-parse", "HEAD"])
}

/// A throwaway checkout shaped like the AIDA workspace, on branch `main`, with
/// a fake dev binary at `target/release/aida`.
fn fake_workspace() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    for c in ["aida-cli-lib", "aida-core"] {
        std::fs::create_dir_all(root.join(c)).unwrap();
        std::fs::write(root.join(c).join("Cargo.toml"), "[package]\n").unwrap();
    }
    git(&root, &["init", "-q", "-b", "main"]);
    let exe = root.join("target").join("release").join("aida");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, "").unwrap();
    std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
    (tmp, exe)
}

#[test]
fn decide_proceeds_when_fresh_or_not_gated() {
    assert_eq!(decide(&Freshness::Fresh, false), GateDecision::Proceed);
    assert_eq!(decide(&Freshness::NotGated, false), GateDecision::Proceed);
    assert_eq!(decide(&Freshness::NotGated, true), GateDecision::Proceed);
}

#[test]
fn decide_refuses_stale_with_the_fix_command_and_override() {
    let stale = Freshness::Stale {
        binary_sha: "abc1234".into(),
        head_sha: "def5678901234".into(),
        branch: "main".into(),
        kind: ShaMatch::Ancestor,
    };
    match decide(&stale, false) {
        GateDecision::Refuse(msg) => {
            assert!(msg.contains("is behind main HEAD"), "{msg}");
            assert!(msg.contains("make build-fast"), "{msg}");
            assert!(msg.contains("--allow-stale-binary"), "{msg}");
            assert!(msg.contains(ALLOW_STALE_ENV), "{msg}");
        }
        other => panic!("expected refuse, got {other:?}"),
    }
    assert!(matches!(decide(&stale, true), GateDecision::Bypassed(_)));
}

#[test]
fn decide_unknown_refuses_with_guidance_never_silently() {
    let unknown = Freshness::Unknown {
        reason: "no sha".into(),
    };
    match decide(&unknown, false) {
        GateDecision::Refuse(msg) => {
            assert!(msg.contains("no sha") && msg.contains("--allow-stale-binary"));
        }
        other => panic!("expected refuse, got {other:?}"),
    }
    assert!(matches!(decide(&unknown, true), GateDecision::Bypassed(_)));
}

#[test]
fn released_binary_is_never_gated() {
    let (tmp, _dev_exe) = fake_workspace();
    commit(tmp.path(), "a");
    // An installed binary outside any workspace `target/`.
    let other = tempfile::tempdir().unwrap();
    let installed = other.path().join("bin").join("aida");
    std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
    std::fs::write(&installed, "").unwrap();
    assert_eq!(
        evaluate(tmp.path(), &installed, "0000000"),
        Freshness::NotGated
    );
}

#[test]
fn downstream_project_is_never_gated() {
    let (tmp, dev_exe) = fake_workspace();
    commit(tmp.path(), "a");
    let downstream = tempfile::tempdir().unwrap();
    assert_eq!(
        evaluate(downstream.path(), &dev_exe, "0000000"),
        Freshness::NotGated
    );
}

#[test]
fn dev_workspace_of_finds_the_checkout_owning_target() {
    let (tmp, exe) = fake_workspace();
    let found = dev_workspace_of(&exe).expect("dev build detected");
    assert_eq!(
        found.canonicalize().unwrap(),
        tmp.path().canonicalize().unwrap()
    );
}

#[test]
fn evaluate_fresh_behind_and_unknown() {
    let (tmp, exe) = fake_workspace();
    let root = tmp.path();
    let first = commit(root, "a");
    assert_eq!(evaluate(root, &exe, &first[..9]), Freshness::Fresh);

    let second = commit(root, "b");
    match evaluate(root, &exe, &first[..9]) {
        Freshness::Stale {
            head_sha,
            branch,
            kind,
            ..
        } => {
            assert_eq!(head_sha, second);
            assert_eq!(branch, "main");
            assert_eq!(kind, ShaMatch::Ancestor);
        }
        other => panic!("expected stale, got {other:?}"),
    }

    assert!(matches!(
        evaluate(root, &exe, "unknown"),
        Freshness::Unknown { .. }
    ));
}

#[test]
fn staleness_warning_only_for_behind_builds() {
    let behind = Freshness::Stale {
        binary_sha: "abc1234".into(),
        head_sha: "def5678".into(),
        branch: "main".into(),
        kind: ShaMatch::Ancestor,
    };
    let line = staleness_warning(&behind).expect("behind warns");
    assert!(line.contains("behind main HEAD") && line.contains("make build-fast"));

    let diverged = Freshness::Stale {
        binary_sha: "abc1234".into(),
        head_sha: "def5678".into(),
        branch: "main".into(),
        kind: ShaMatch::Unrelated,
    };
    assert_eq!(staleness_warning(&diverged), None);
    assert_eq!(staleness_warning(&Freshness::Fresh), None);
    assert_eq!(staleness_warning(&Freshness::NotGated), None);
}

#[test]
fn drain_lock_serializes_bypass_only_when_set() {
    let mut lock = crate::drain_lock::DrainLock {
        pid: 1,
        pid_start_time: None,
        started_at_utc: "2026-09-23T00:00:00Z".into(),
        command: "burndown run".into(),
        host: "h".into(),
        wave_id: "w".into(),
        binary_sha: "abc1234".into(),
        binary_mtime_secs: None,
        binary_path: String::new(),
        freshness_gate_bypassed: false,
        specs: vec![],
    };
    let json = serde_json::to_string(&lock).unwrap();
    assert!(!json.contains("freshness_gate_bypassed"));
    // Older lock files (no field) still parse.
    let back: crate::drain_lock::DrainLock = serde_json::from_str(&json).unwrap();
    assert!(!back.freshness_gate_bypassed);

    lock.freshness_gate_bypassed = true;
    let json = serde_json::to_string(&lock).unwrap();
    assert!(json.contains("\"freshness_gate_bypassed\":true"));
}
