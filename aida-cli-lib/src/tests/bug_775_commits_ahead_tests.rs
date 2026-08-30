//! BUG-775: the branch-vs-default-branch check must work in a repo with NO
//! `origin` remote (the local-merge workflow, where the incident happened),
//! and `queue done` must REFUSE — not warn and continue — when it cannot
//! produce an answer.
//!
//! These run real `git` against throwaway repos, because the whole defect was
//! in what git was (and wasn't) asked. trace:BUG-775 | ai:claude

use super::*;
use std::process::Command;
use tempfile::TempDir;

fn git(repo: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("git runs")
}

fn git_ok(repo: &std::path::Path, args: &[&str]) {
    let out = git(repo, args);
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A repo with NO remote at all, default branch `<default>`, plus a feature
/// branch `feat` carrying `ahead` commits.
fn local_only_repo(default: &str, ahead: u32) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let p = tmp.path();
    git_ok(
        p,
        &["init", &format!("--initial-branch={default}"), "--quiet"],
    );
    git_ok(p, &["commit", "--allow-empty", "-m", "root", "--quiet"]);
    git_ok(p, &["checkout", "-q", "-b", "feat"]);
    for i in 0..ahead {
        git_ok(
            p,
            &["commit", "--allow-empty", "-m", &format!("c{i}"), "--quiet"],
        );
    }
    tmp
}

/// (a) No origin remote, default branch `master` — the exact shape of the
/// repo where the gate silently no-opped. It must now resolve a real count
/// against the LOCAL default branch.
#[test]
fn commits_ahead_resolves_against_local_master_with_no_origin() {
    let tmp = local_only_repo("master", 3);
    assert_eq!(
        branch_commits_ahead_default(tmp.path(), "feat"),
        workflow_hints::CommitsAhead::Ahead(3)
    );
    // The Option-shaped wrapper the hint paths use agrees.
    assert_eq!(branch_commits_ahead_main(tmp.path(), "feat"), Some(3));
}

/// Same, with a local `main` and no remote.
#[test]
fn commits_ahead_resolves_against_local_main_with_no_origin() {
    let tmp = local_only_repo("main", 1);
    assert_eq!(
        branch_commits_ahead_default(tmp.path(), "feat"),
        workflow_hints::CommitsAhead::Ahead(1)
    );
}

/// Being ON the default branch is its own outcome, whatever it is called —
/// not an "unresolvable" one that would refuse every `queue done` run there.
#[test]
fn being_on_the_default_branch_is_a_distinct_outcome() {
    for default in ["main", "master"] {
        let tmp = local_only_repo(default, 0);
        git_ok(tmp.path(), &["checkout", "-q", default]);
        assert_eq!(
            branch_commits_ahead_default(tmp.path(), default),
            workflow_hints::CommitsAhead::OnDefaultBranch,
            "default branch {default}"
        );
        assert_eq!(
            branch_commits_ahead_default(tmp.path(), "HEAD"),
            workflow_hints::CommitsAhead::OnDefaultBranch
        );
    }
}

/// No default branch anywhere (fresh repo on a differently-named branch, no
/// remote) → Unknown, carrying a reason. This is the input that must produce
/// a refusal downstream.
#[test]
fn no_resolvable_default_branch_is_unknown_with_a_reason() {
    let tmp = TempDir::new().unwrap();
    let p = tmp.path();
    git_ok(p, &["init", "--initial-branch=trunkish", "--quiet"]);
    git_ok(p, &["commit", "--allow-empty", "-m", "root", "--quiet"]);
    // A local `init.defaultBranch` would reintroduce a resolvable default;
    // unsetting is best-effort (it exits non-zero when unset already).
    let _ = git(
        p,
        &["config", "--local", "--unset-all", "init.defaultBranch"],
    );
    match branch_commits_ahead_default(p, "trunkish") {
        workflow_hints::CommitsAhead::Unknown(reason) => {
            assert!(
                reason.contains("default branch"),
                "reason should name what failed: {reason}"
            );
        }
        // A machine whose global `init.defaultBranch` is `trunkish` would
        // resolve it as the default branch — also a correct answer, and
        // still not the silent-skip the bug was about.
        workflow_hints::CommitsAhead::OnDefaultBranch => {}
        other => panic!("expected Unknown, got {other:?}"),
    }
}

/// (b) `queue done`'s gate REFUSES when commits-ahead is unresolvable —
/// where it used to print a warning and carry on.
#[test]
fn queue_done_gate_refuses_when_commits_ahead_is_unresolvable() {
    let tmp = TempDir::new().unwrap();
    let result = workflow_hints::queue_done_precheck_diagnose(
        "TASK-5",
        Ok(tmp.path().to_path_buf()),
        |_| Some("task-5-2".to_string()),
        |_, _| workflow_hints::CommitsAhead::Unknown("origin/main unresolved".to_string()),
        |_, _| panic!("PR lookup must not run once the gate has already refused"),
    );
    match result {
        workflow_hints::QueueDoneGateDiagnose::Refuse(lines) => {
            let joined = lines.join("\n");
            assert!(joined.contains("refused"), "{joined}");
            assert!(joined.contains("task-5-2"), "names the branch: {joined}");
            assert!(
                joined.contains("origin/main unresolved"),
                "names the reason: {joined}"
            );
            assert!(
                joined.contains("--skip-pr-check"),
                "offers the documented override: {joined}"
            );
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// The default-branch no-op must stay a silent proceed — a refusal there
/// would block every legitimate `queue done` run from the default branch.
#[test]
fn queue_done_gate_proceeds_silently_on_the_default_branch() {
    let tmp = TempDir::new().unwrap();
    let result = workflow_hints::queue_done_precheck_diagnose(
        "TASK-5",
        Ok(tmp.path().to_path_buf()),
        |_| Some("master".to_string()),
        |_, _| workflow_hints::CommitsAhead::OnDefaultBranch,
        |_, _| panic!("PR lookup must not run on the default branch"),
    );
    assert_eq!(result, workflow_hints::QueueDoneGateDiagnose::Proceed);
}

// ---- the review-verdict tip probe, against real git --------------------

/// (c) The tip is still the reviewed commit → the gate's input says so.
/// (d) One more commit on top → it says the branch advanced.
#[test]
fn verdict_tip_relation_tracks_the_branch_against_the_reviewed_sha() {
    let tmp = local_only_repo("master", 1);
    let p = tmp.path();
    let reviewed = String::from_utf8_lossy(&git(p, &["rev-parse", "HEAD"]).stdout)
        .trim()
        .to_string();

    // Nothing new since the review.
    assert_eq!(
        verdict_tip_relation(p, Some("feat"), Some(&reviewed)),
        review_verdict::TipRelation::AtReviewedSha
    );
    // A short sha (what a reviewer actually writes) resolves the same way.
    assert_eq!(
        verdict_tip_relation(p, Some("feat"), Some(&reviewed[..7])),
        review_verdict::TipRelation::AtReviewedSha
    );

    // The rework lands a real commit.
    git_ok(p, &["commit", "--allow-empty", "-m", "fix", "--quiet"]);
    assert_eq!(
        verdict_tip_relation(p, Some("feat"), Some(&reviewed)),
        review_verdict::TipRelation::AdvancedPast
    );

    // A sha from another clone is not in this history at all.
    assert_eq!(
        verdict_tip_relation(
            p,
            Some("feat"),
            Some("0123456789abcdef0123456789abcdef01234567")
        ),
        review_verdict::TipRelation::Unknown
    );
    // No recorded sha → unknowable.
    assert_eq!(
        verdict_tip_relation(p, Some("feat"), None),
        review_verdict::TipRelation::Unknown
    );
}

/// An amended branch: the reviewed commit is gone from the history, so the
/// relation is `Rewritten` (allowed, but loudly).
#[test]
fn verdict_tip_relation_reports_rewritten_history() {
    let tmp = local_only_repo("master", 1);
    let p = tmp.path();
    let reviewed = String::from_utf8_lossy(&git(p, &["rev-parse", "HEAD"]).stdout)
        .trim()
        .to_string();
    git_ok(
        p,
        &[
            "commit",
            "--amend",
            "--allow-empty",
            "-m",
            "reworked",
            "--quiet",
        ],
    );
    assert_eq!(
        verdict_tip_relation(p, Some("feat"), Some(&reviewed)),
        review_verdict::TipRelation::Rewritten
    );
}

/// End-to-end over the two pure decisions plus real git: a recorded
/// RequestChanges refuses while the branch is unchanged, and stops refusing
/// once a commit lands.
#[test]
fn recorded_request_changes_blocks_until_a_commit_lands() {
    let tmp = local_only_repo("master", 1);
    let p = tmp.path();
    let reviewed = String::from_utf8_lossy(&git(p, &["rev-parse", "HEAD"]).stdout)
        .trim()
        .to_string();
    review_verdict::record_verdict(
        p,
        "TASK-5",
        Some("RequestChanges"),
        Some(&reviewed),
        Some("feat"),
        Some("three blocking defects"),
        "test",
    )
    .unwrap();

    let v = review_verdict::read_recorded_verdict(p, "TASK-5").unwrap();
    let before = review_verdict::queue_done_verdict_gate(
        "TASK-5",
        Some(&v),
        verdict_tip_relation(p, Some("feat"), v.reviewed_sha.as_deref()),
    );
    assert!(
        matches!(before, review_verdict::VerdictGate::Refuse(_)),
        "unchanged branch must refuse, got {before:?}"
    );

    git_ok(
        p,
        &["commit", "--allow-empty", "-m", "address review", "--quiet"],
    );
    let after = review_verdict::queue_done_verdict_gate(
        "TASK-5",
        Some(&v),
        verdict_tip_relation(p, Some("feat"), v.reviewed_sha.as_deref()),
    );
    assert_eq!(after, review_verdict::VerdictGate::Proceed);
}

// ── BUG-802: the drive-root anchor + PR handshake ────────────────────────────

/// The env anchor must beat find_project_root: a reviewer inside a PR checkout
/// (which has its own .aida/ and git toplevel) would otherwise write the
/// handshake into the checkout, invisible to the orchestrator. Env mutation →
/// serialized by the same guard the other env-sensitive tests use.
// trace:BUG-802 | ai:claude
#[test]
fn drive_root_env_anchor_beats_project_root_discovery() {
    let _guard = crate::test_env::env_lock();
    let drive = tempfile::tempdir().unwrap();
    std::env::set_var("AIDA_DRIVE_ROOT", drive.path());
    let resolved = crate::drive_root_or_project_root().unwrap();
    assert_eq!(resolved, drive.path());
    std::env::remove_var("AIDA_DRIVE_ROOT");
}

#[test]
fn a_dangling_drive_root_falls_back_rather_than_writing_into_the_void() {
    let _guard = crate::test_env::env_lock();
    std::env::set_var("AIDA_DRIVE_ROOT", "/nonexistent/definitely/not/here");
    // Must not error out or return the bogus path — fall back to discovery.
    let resolved = crate::drive_root_or_project_root();
    if let Ok(p) = resolved {
        assert_ne!(p, std::path::Path::new("/nonexistent/definitely/not/here"));
    }
    std::env::remove_var("AIDA_DRIVE_ROOT");
}

/// What `record --pr` writes must be exactly what phase 4 parses.
// trace:BUG-802 | ai:claude
#[test]
fn the_handshake_the_verb_writes_is_the_handshake_phase4_parses() {
    use crate::review_verdict::VerdictKind;
    for raw in ["approved", "request-changes", "rejected"] {
        let kind = VerdictKind::parse(raw);
        let label = kind.label();
        assert!(
            crate::auto_complete::Verdict::parse(label).is_some(),
            "orchestrator must accept the label the verb records: {label}"
        );
    }
}
