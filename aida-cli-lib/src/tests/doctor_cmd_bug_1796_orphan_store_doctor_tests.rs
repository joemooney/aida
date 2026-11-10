// trace:TASK-3780 | ai:codex
use super::*;
use std::process::Command;

fn git(path: &std::path::Path, args: &[&str]) {
    let output = crate::test_git::git_command()
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn doctor_reports_orphan_store_ahead_commit_count() {
    // trace:BUG-1796.ac18732d | ai:codex
    let temp = tempfile::tempdir().unwrap();
    let remote = temp.path().join("remote.git");
    let seed = temp.path().join("seed");
    let project = temp.path().join("project");
    let store = project.join(".aida-store");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&remote).unwrap();
    std::fs::create_dir_all(&seed).unwrap();
    git(
        &remote,
        &["init", "--bare", "--initial-branch=aida-store", "--quiet"],
    );
    git(&seed, &["init", "--initial-branch=aida-store", "--quiet"]);
    git(&seed, &["config", "user.email", "test@example.com"]);
    git(&seed, &["config", "user.name", "Test"]);
    std::fs::write(seed.join("base"), "base\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-m", "base", "--quiet"]);
    git(
        &seed,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&seed, &["push", "-u", "origin", "aida-store", "--quiet"]);
    git(
        &project,
        &[
            "clone",
            remote.to_str().unwrap(),
            store.to_str().unwrap(),
            "--quiet",
        ],
    );
    git(&store, &["config", "user.email", "test@example.com"]);
    git(&store, &["config", "user.name", "Test"]);
    for n in 1..=3 {
        std::fs::write(store.join(format!("local-{n}")), "local\n").unwrap();
        git(&store, &["add", "."]);
        git(&store, &["commit", "-m", &format!("local {n}"), "--quiet"]);
    }

    let finding = scan_remote_drift(&project)
        .into_iter()
        .find(|finding| finding.id == "orphan-store-ahead-of-origin")
        .expect("doctor reports the local store commits missing from origin");
    assert!(
        finding.summary.contains("3 commits ahead"),
        "{}",
        finding.summary
    );
    assert!(
        finding.summary.contains("3 unpushed"),
        "{}",
        finding.summary
    );
}
