// trace:TASK-3874 | ai:codex
use super::*;
use std::time::{Duration, Instant};

fn git(root: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .current_dir(root)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.org",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

#[test]
fn fetches_target_advanced_after_checkout() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path();
    git(remote, &["init", "-q"]);
    git(remote, &["commit", "--allow-empty", "-qm", "common"]);
    let common = git(remote, &["rev-parse", "HEAD"]);
    git(remote, &["clone", "--quiet", ".", "review"]);
    let review = remote.join("review");
    git(&review, &["commit", "--allow-empty", "-qm", "topic"]);
    git(remote, &["commit", "--allow-empty", "-qm", "advanced"]);
    let base = git(remote, &["rev-parse", "HEAD"]);
    assert!(!crate::test_git::git_command()
        .current_dir(&review)
        .args(["cat-file", "-e", &base])
        .status()
        .unwrap()
        .success());
    crate::forge_lookup::with_forge_lookup_deadline(Duration::from_secs(5), || {
        review_risk_base_object(&review, &base).unwrap();
    });
    assert_eq!(git(&review, &["merge-base", &base, "HEAD"]), common);
}

#[test]
fn stalled_version_probe_exhausts_risk_budget() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q"]);
    git(
        root,
        &["remote", "add", "origin", "https://github.com/test/project"],
    );
    let marker = root.join("resolver-started");
    crate::test_exec::write_executable(
        &root.join("gh"),
        format!(
            "#!/bin/sh\n[ \"$1\" = --version ] || exit 1\ntouch '{}'\nexec sleep 5\n",
            marker.display()
        ),
    );
    let mut env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_TEST_GH_BINARY", None)]);
    env.set_key(
        "PATH",
        std::env::join_paths([root, Path::new("/usr/bin"), Path::new("/bin")]).unwrap(),
    );
    let start = Instant::now();
    let head = "a".repeat(40);
    let base = review_risk_pr_base_with_budget(root, 1, &head, Duration::from_secs(1));
    assert!(marker.exists());
    assert!(start.elapsed() < Duration::from_secs(4));
    assert!(base.is_none());
    assert_eq!(
        crate::review_risk::assess(root, base.as_deref(), Some(&head)).tier,
        crate::review_risk::Tier::Strict
    );
}
