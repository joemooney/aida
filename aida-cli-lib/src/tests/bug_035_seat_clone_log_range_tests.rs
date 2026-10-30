//! BUG-035: a contained lane pushes its PR branch from a seat clone, so the
//! primary checkout running `aida pr auto-queue-review` has no local branch
//! by the forge's bare head name. `git_log_messages` must still find the
//! commits (via `origin/<head>`, fetching it if needed), and a range nothing
//! resolves must come back as an error naming it, never as "no commits".
//! trace:BUG-B6B1278890774EF993D2EBC327862470-035 | ai:claude

use super::*;

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@localhost",
        ])
        .args(args)
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// A bare origin with `main`, and a primary clone of it. Returns
/// `(tmp, origin, primary, seat)`; the seat clone has not pushed anything yet.
fn fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let origin = tmp.path().join("origin.git");
    let seed = tmp.path().join("seed");
    let primary = tmp.path().join("primary");
    let seat = tmp.path().join("seat");
    git(
        tmp.path(),
        &["init", "-q", "--bare", "-b", "main", "origin.git"],
    );
    git(tmp.path(), &["clone", "-q", "origin.git", "seed"]);
    git(
        &seed,
        &["commit", "-q", "--allow-empty", "--no-verify", "-m", "base"],
    );
    git(&seed, &["push", "-q", "origin", "main"]);
    git(tmp.path(), &["clone", "-q", "origin.git", "primary"]);
    git(tmp.path(), &["clone", "-q", "origin.git", "seat"]);
    (tmp, origin, primary, seat)
}

fn push_lane_branch(seat: &std::path::Path) {
    git(seat, &["checkout", "-q", "-b", "lane-branch"]);
    git(
        seat,
        &[
            "commit",
            "-q",
            "--allow-empty",
            "--no-verify",
            "-m",
            "[AI:claude] fix(x): lane work (BUG-HOME-035)",
        ],
    );
    git(seat, &["push", "-q", "origin", "lane-branch"]);
}

#[test]
fn head_branch_only_on_origin_is_fetched_and_trailers_found() {
    let (_tmp, _origin, primary, seat) = fixture();
    push_lane_branch(&seat);

    // The primary has neither `lane-branch` nor `origin/lane-branch`.
    let messages =
        git_log_messages(&primary, "main", "lane-branch").expect("range resolves via origin");
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("(BUG-HOME-035)"), "{messages:?}");
}

#[test]
fn head_branch_present_as_remote_tracking_ref_resolves_without_local_branch() {
    let (_tmp, _origin, primary, seat) = fixture();
    push_lane_branch(&seat);
    git(&primary, &["fetch", "-q", "origin"]);

    assert_eq!(
        resolve_log_range(&primary, "main", "lane-branch"),
        ("origin/main".to_string(), "origin/lane-branch".to_string())
    );
    let messages = git_log_messages(&primary, "main", "lane-branch").expect("resolves");
    assert_eq!(messages.len(), 1, "{messages:?}");
}

#[test]
fn local_head_branch_is_used_as_is() {
    let (_tmp, _origin, primary, _seat) = fixture();
    git(&primary, &["branch", "local-only"]);
    assert_eq!(
        resolve_log_range(&primary, "main", "local-only"),
        ("main".to_string(), "local-only".to_string())
    );
}

#[test]
fn unresolvable_head_is_an_error_naming_the_range() {
    let (_tmp, _origin, primary, _seat) = fixture();
    let err = git_log_messages(&primary, "main", "nowhere-branch")
        .expect_err("a branch that exists nowhere must not read as zero commits");
    assert!(
        format!("{err:#}").contains("main..nowhere-branch"),
        "error must name the range: {err:#}"
    );
}
