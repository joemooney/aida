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
    std::fs::write(review.join("topic"), "topic").unwrap();
    git(&review, &["add", "topic"]);
    git(&review, &["commit", "-qm", "topic"]);
    let blob = git(&review, &["rev-parse", "HEAD:topic"]);
    let object = review.join(format!(".git/objects/{}/{}", &blob[..2], &blob[2..]));
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
    let bundle = remote.join("fixture.bundle");
    let uri = bundle.to_str().unwrap();
    git(remote, &["bundle", "create", uri, "HEAD"]);
    git(&review, &["config", "fetch.bundleURI", uri]);
    let epoch = std::time::SystemTime::UNIX_EPOCH;
    std::fs::File::open(&bundle)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_accessed(epoch))
        .unwrap();
    let observed = remote.join("observed");
    std::fs::write(&observed, &common).unwrap();
    let gh = remote.join("gh");
    crate::test_exec::write_executable(
        &gh,
        format!(
            r#"#!/bin/sh
[ "$*" = "pr view 1 --repo github.com/test/project --json headRefOid,baseRefOid" ] || exit 1
printf '{{"headRefOid":"{head}","baseRefOid":"%s"}}\n' "$(cat "$(dirname "$0")/observed")"
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
    std::fs::rename(&review, &seat).unwrap();
    assert!(review_risk_pr_base(&seat, 1, &head).is_none());
    std::fs::write(&observed, &base).unwrap();
    assert!(review_risk_pr_base(&seat, 1, &head).is_none());
    std::fs::rename(&seat, &review).unwrap();
    std::fs::write(&observed, &common).unwrap();
    std::fs::rename(&object, remote.join("missing-blob")).unwrap();
    for (key, value) in [
        ("remote.other.promisor", "yes"),
        ("extensions.partialClone", "origin"),
    ] {
        git(&review, &["config", key, value]);
        let result = review_risk_pr_base(&review, 1, &head);
        assert!(result.is_none());
        let risk = crate::review_risk::assess(&review, result.as_deref(), Some(&head));
        assert_eq!(risk.tier, crate::review_risk::Tier::Strict);
        git(&review, &["config", "--unset", key]);
    }
    assert!(!log.exists());
    let refused = std::fs::read_to_string(&trace).unwrap();
    for command in ["cat-file", "merge-base", "diff", "fetch", "remote-https"] {
        assert!(!refused.contains(&format!(r#""{command}""#)));
    }
    std::fs::rename(remote.join("missing-blob"), &object).unwrap();
    std::fs::write(&observed, &base).unwrap();
    assert_eq!(review_risk_pr_base(&review, 1, &head), Some(base.clone()));
    assert_eq!(std::fs::read_to_string(log).unwrap(), "fetch\n");
    let events = std::fs::read_to_string(trace).unwrap();
    assert!(events.contains(r#""-c","fetch.bundleURI=","-c","transfer.bundleURI=false","fetch""#));
    assert_eq!(bundle.metadata().unwrap().accessed().unwrap(), epoch);
    assert!(!events.contains("git-remote-https"));
    git(&review, &["fetch", "--no-tags", "origin", &base]);
    assert!(bundle.metadata().unwrap().accessed().unwrap() > epoch);
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
