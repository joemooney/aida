use crate::*;

fn tempdir_root() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".aida")).unwrap();
    root
}

// A reviewer's claim blocks a merge; recording the verdict (with --pr)
// clears it, so the same merge gate then lets the merge through.
//
// BUG-1743: `--pr` also writes the phase-3 handshake, whose anchor is
// resolved ambiently (`review_pr_handshake_path`), NOT from the root
// passed in. Unpinned, this test wrote
// `<real repo>/.aida/review-verdicts/PR-14051.json` into the operator's
// checkout on every run — a fabricated APPROVED verdict keyed to a PR
// number, invisible because `.aida/` is gitignored. Pin all three keys to
// this test's own tempdir; the guard holds `ENV_LOCK` so no sibling can
// move them mid-test. See `assert_handshake_anchor_is_pinned`.
// trace:BUG-1743 | ai:claude
#[test]
fn recording_the_verdict_clears_the_marker_and_unblocks_merge() {
    let root = tempdir_root();
    let root_str = root.path().to_str().expect("tempdir path is utf-8");
    let _pinned = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_PROJECT_ROOT", Some(root_str)),
        ("AIDA_DRIVE_ROOT", Some(root_str)),
        ("AIDA_REVIEW_VERDICT_FILE", None),
    ]);
    let mut m = crate::gate::review_marker::Marker::for_this_process(
        14051,
        Some("abc1234"),
        Some("BUG-14051"),
        "reviewer seat",
    );
    m.pid = 0;
    m.ttl_secs = crate::gate::review_marker::CLAIM_TTL_SECS;
    crate::gate::review_marker::write(root.path(), &m).unwrap();
    assert!(matches!(
        crate::gate::review_marker::merge_gate(root.path(), 14051, || Some("abc1234".into())),
        crate::gate::review_marker::MergeGate::UnderReview(_)
    ));

    // BUG-1918: a PR approval resolves the PR's subject through the forge.
    let subject = ReviewPrSubject {
        head_ref: "bug-14051-work".into(),
        commit_subjects: vec![],
    };
    // BUG-1918: an approval must name its recorder (here, a session id).
    let reviewer = crate::gate::review_authority::IdentitySource {
        session_ids: vec!["independent-reviewer".into()],
        ..Default::default()
    };
    with_review_pr_subject(subject, || {
        crate::gate::review_authority::with_identity_source(reviewer, || {
            handle_review_record_at(
                root.path().to_path_buf(),
                "BUG-14051",
                "approved",
                Some("abc1234"),
                Some("bug-14051-work"),
                Some("looks right"),
                &[],
                &[],
                Some(14051),
            )
        })
    })
    .expect("recording an approval must succeed");

    assert!(crate::gate::review_marker::read(root.path(), 14051).is_none());
    assert_eq!(
        crate::gate::review_marker::merge_gate(root.path(), 14051, || panic!(
            "no marker, no lookup"
        )),
        crate::gate::review_marker::MergeGate::Clear
    );
}

/// Record an approval for PR-14053 from `worktree` with the launcher's
/// anchor at `anchor`, `AIDA_CONTAINED` as given.
fn record_with_anchor(
    worktree: &std::path::Path,
    anchor: &std::path::Path,
    contained: Option<&str>,
) -> Result<()> {
    let wt = worktree.to_str().unwrap();
    let _pinned = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_PROJECT_ROOT", Some(wt)),
        ("AIDA_DRIVE_ROOT", Some(wt)),
        ("AIDA_REVIEW_VERDICT_FILE", Some(anchor.to_str().unwrap())),
        ("AIDA_CONTAINED", contained),
    ]);
    let subject = ReviewPrSubject {
        head_ref: "bug-14053-work".into(),
        commit_subjects: vec![],
    };
    let reviewer = crate::gate::review_authority::IdentitySource {
        session_ids: vec!["independent-reviewer".into()],
        ..Default::default()
    };
    with_review_pr_subject(subject, || {
        crate::gate::review_authority::with_identity_source(reviewer, || {
            handle_review_record_at(
                worktree.to_path_buf(),
                "BUG-14053",
                "approved",
                Some("abc1234"),
                Some("bug-14053-work"),
                Some("looks right"),
                &[],
                &[],
                Some(14053),
            )
        })
    })
}

// BUG-2201 / TASK-2040: a contained reviewer whose launcher anchor sits
// in a read-only directory (the primary checkout, outside its write
// boundary) writes the handshake into its own worktree for the launcher
// to collect, and leaves the anchor alone. Uncontained, the same refusal
// is still an error. trace:BUG-2201 | ai:claude
#[cfg(unix)]
#[test]
fn contained_record_falls_back_to_worktree_when_anchor_is_read_only() {
    use std::os::unix::fs::PermissionsExt;
    let base = tempfile::tempdir().unwrap();
    let worktree = base.path().join("relay-pr-14053");
    std::fs::create_dir_all(worktree.join(".aida")).unwrap();
    let anchor_dir = base.path().join("relay/.aida/review-verdicts");
    std::fs::create_dir_all(&anchor_dir).unwrap();
    std::fs::set_permissions(&anchor_dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let restore = |dir: &std::path::Path| {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap()
    };
    if std::fs::write(anchor_dir.join("probe"), b"").is_ok() {
        // Running as root: a mode bit cannot make the directory read-only.
        restore(&anchor_dir);
        return;
    }
    let anchor = anchor_dir.join("PR-14053.json");
    let fallback = worktree.join(".aida/review-verdicts/PR-14053.json");

    let uncontained = record_with_anchor(&worktree, &anchor, None);
    assert!(format!(
        "{:#}",
        uncontained.expect_err("uncontained refusal is an error")
    )
    .contains("handshake was NOT written"),);
    assert!(!fallback.exists(), "uncontained never falls back");

    record_with_anchor(&worktree, &anchor, Some("1"))
        .expect("contained record falls back instead of failing");
    restore(&anchor_dir);
    assert!(!anchor.exists(), "the read-only anchor is untouched");
    let body: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&fallback).unwrap()).unwrap();
    assert_eq!(body["mode"], "orchestrator-phase-3", "{body}");
    assert_eq!(body["reviewed_sha"], "abc1234", "{body}");
}

#[test]
fn verdict_without_pr_clears_markers_naming_the_spec_only() {
    let root = tempdir_root();
    for (pr, spec) in [(1, "TASK-1"), (2, "TASK-2")] {
        let m =
            crate::gate::review_marker::Marker::for_this_process(pr, Some("abc"), Some(spec), "r");
        crate::gate::review_marker::write(root.path(), &m).unwrap();
    }
    let cleared = clear_review_markers_for_verdict(root.path(), "task-1", None);
    assert_eq!(cleared, vec![1]);
    assert!(crate::gate::review_marker::read(root.path(), 2).is_some());
}

// TASK-1459: `aida review <SPEC>` prefers the forge's live PR head over
// the local `origin/<branch>` tracking ref, falling back to that ref
// when the forge can't be reached. A repo with no recognized forge
// remote resolves to `ForgeKind::None` (`PureGitForge`, which errors
// immediately with no subprocess — see `forge.rs`), so this exercises
// the fallback deterministically with no `gh`/network dependency.
#[test]
fn marker_head_best_effort_falls_back_to_local_ref_without_a_forge() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    let git = |args: &[&str]| {
        assert!(crate::test_git::git_command()
            .arg("-C")
            .arg(repo)
            .args(args)
            .status()
            .expect("git")
            .success());
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "test@example.test"]);
    git(&["config", "user.name", "Test"]);
    std::fs::write(repo.join("f"), "x").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "init"]);
    let head = resolve_commit_sha(repo, "HEAD").expect("HEAD resolves");
    // A local remote-tracking ref, as if fetched, but no `origin` remote
    // URL is configured — `resolve_open_change_forge_kind` degrades to
    // `ForgeKind::None`.
    git(&["update-ref", "refs/remotes/origin/some-branch", "HEAD"]);

    let got = review_marker_head_best_effort(repo, 999, "some-branch");
    assert_eq!(got.as_deref(), Some(head.as_str()));
}

#[test]
fn marker_head_best_effort_is_none_with_no_forge_and_no_local_ref() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    assert!(crate::test_git::git_command()
        .arg("-C")
        .arg(repo)
        .args(["init", "-q"])
        .status()
        .expect("git init")
        .success());
    assert_eq!(
        review_marker_head_best_effort(repo, 999, "no-such-branch"),
        None
    );
}

#[test]
fn awaiting_annotation_is_head_scoped_and_read_only() {
    let root = tempdir_root();
    let m = crate::gate::review_marker::Marker::for_this_process(
        5,
        Some("aaa111"),
        None,
        "reviewer seat",
    );
    crate::gate::review_marker::write(root.path(), &m).unwrap();
    assert!(crate::gate::review_marker::live_description(root.path(), 5, Some("aaa111")).is_some());
    assert!(crate::gate::review_marker::live_description(root.path(), 5, Some("bbb222")).is_none());
    assert!(crate::gate::review_marker::live_description(root.path(), 6, Some("aaa111")).is_none());
    assert!(crate::gate::review_marker::read(root.path(), 5).is_some());
}
