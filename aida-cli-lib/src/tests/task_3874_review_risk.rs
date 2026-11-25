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
    if crate::test_env::run_path_isolated_test() {
        return;
    }
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
    // The same origin and object IDs are usable outside the seat clone.
    // trace:TASK-3874 | ai:codex
    let seat = remote.join("aida-seat-review");
    std::fs::rename(&review, &seat).unwrap();
    crate::forge_lookup::with_forge_lookup_deadline(Duration::from_secs(5), || {
        assert!(review_risk_base_object(&seat, &common).is_none());
        assert!(review_risk_base_object(&seat, &base).is_none());
    });
    std::fs::rename(&seat, &review).unwrap();
    let head = git(&review, &["rev-parse", "HEAD"]);
    git(
        &review,
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:test/project.git",
        ],
    );
    git(&review, &["config", "remote.origin.promisor", "true"]);
    let gh = remote.join("gh");
    crate::test_exec::write_executable(
        &gh,
        format!(
            r#"#!/bin/sh
[ "$*" = "pr view 1 --repo github.com/test/project --json headRefOid,baseRefOid" ] || exit 1
echo '{{"headRefOid":"{head}","baseRefOid":"{base}"}}'
"#
        ),
    );
    // Only the explicit, bundle-disabled fetch may open the transport.
    let ssh = remote.join("ssh");
    let log = remote.join("transports");
    let trace = remote.join("git-trace");
    crate::test_exec::write_executable(
        &ssh,
        r#"#!/bin/sh
cd "$(dirname "$0")" || exit 1
echo fetch >> transports
exec git-upload-pack .
"#,
    );
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap())),
        ("GIT_SSH_COMMAND", None),
        ("GIT_SSH", Some(ssh.to_str().unwrap())),
        ("GIT_SSH_VARIANT", Some("ssh")),
        ("GIT_TRACE2_EVENT", Some(trace.to_str().unwrap())),
    ]);
    assert_eq!(review_risk_pr_base(&review, 1, &head), Some(base.clone()));
    assert_eq!(std::fs::read_to_string(log).unwrap(), "fetch\n");
    let events = std::fs::read_to_string(trace).unwrap();
    assert!(events.contains(r#""-c","fetch.bundleURI=","-c","transfer.bundleURI=false","fetch""#));
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
