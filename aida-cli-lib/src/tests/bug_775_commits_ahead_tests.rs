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
        &[],
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

// ── BUG-806: spec-keyed verdict fallback + drive-binary PATH ─────────────────

#[test]
fn a_fresh_spec_verdict_is_accepted_when_the_pr_file_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let vd = dir.path().join(".aida").join("review-verdicts");
    std::fs::create_dir_all(&vd).unwrap();
    let started = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    std::fs::write(
        vd.join("STORY-783.json"),
        r#"{"verdict":"approved","reviewed_sha":"abc","recorded_at":"2026-08-29T18:02:00Z"}"#,
    )
    .unwrap();
    let out = crate::spec_verdict_fallback_for_phase3(dir.path(), "STORY-783", started);
    assert!(
        matches!(
            out,
            Some(crate::auto_complete::ReviewerOutcome::Verdict(
                crate::auto_complete::Verdict::Approved
            ))
        ),
        "a verdict recorded during this session must be accepted: {out:?}"
    );
}

#[test]
fn a_verdict_recorded_before_this_session_is_stale_and_refused() {
    // The freshness gate: an approval from an EARLIER review must never
    // advance a later diff.
    let dir = tempfile::tempdir().unwrap();
    let vd = dir.path().join(".aida").join("review-verdicts");
    std::fs::create_dir_all(&vd).unwrap();
    std::fs::write(vd.join("BUG-1.json"), r#"{"verdict":"approved"}"#).unwrap();
    let started = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
    assert_eq!(
        crate::spec_verdict_fallback_for_phase3(dir.path(), "BUG-1", started),
        None
    );
}

#[test]
fn request_changes_flows_through_the_fallback_too() {
    // The fallback must carry EVERY verdict, not just approvals — otherwise a
    // request-changes review would look like "no verdict" and shelve.
    let dir = tempfile::tempdir().unwrap();
    let vd = dir.path().join(".aida").join("review-verdicts");
    std::fs::create_dir_all(&vd).unwrap();
    let started = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    std::fs::write(vd.join("BUG-2.json"), r#"{"verdict":"request-changes"}"#).unwrap();
    assert!(matches!(
        crate::spec_verdict_fallback_for_phase3(dir.path(), "BUG-2", started),
        Some(crate::auto_complete::ReviewerOutcome::Verdict(
            crate::auto_complete::Verdict::RequestChanges
        ))
    ));
}

#[test]
fn absent_or_garbage_records_fall_through() {
    let dir = tempfile::tempdir().unwrap();
    let started = std::time::SystemTime::UNIX_EPOCH;
    assert_eq!(
        crate::spec_verdict_fallback_for_phase3(dir.path(), "BUG-3", started),
        None
    );
    let vd = dir.path().join(".aida").join("review-verdicts");
    std::fs::create_dir_all(&vd).unwrap();
    std::fs::write(vd.join("BUG-3.json"), "not json").unwrap();
    assert_eq!(
        crate::spec_verdict_fallback_for_phase3(dir.path(), "BUG-3", started),
        None
    );
}

#[test]
fn drive_path_env_leads_with_the_current_exe_dir() {
    let p = crate::session::drive_path_env().expect("path assembles");
    let exe_dir = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let first = std::env::split_paths(&p).next().unwrap();
    assert_eq!(
        first, exe_dir,
        "the drive's own binary dir must win resolution"
    );
    // And the inherited PATH must still be there, or children lose git/gh.
    let inherited: Vec<_> = std::env::split_paths(&p).collect();
    assert!(
        inherited.len() > 1,
        "inherited PATH entries must be preserved"
    );
}

// ── BUG-809: sibling-checkout verdict sweep + prompt-text anchor ─────────────

/// A reviewer that checked the PR out NEXT TO the repo and recorded the
/// verdict there (env anchor stripped by the vendor sandbox) must still land:
/// the sweep finds the fresh PR-keyed file in the sibling dir and copies it
/// back to the drive root.
#[test]
fn a_fresh_sibling_checkout_verdict_is_accepted_and_copied_back() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("aida");
    std::fs::create_dir_all(&root).unwrap();
    let checkout_vd = parent
        .path()
        .join("aida-pr-1619")
        .join(".aida")
        .join("review-verdicts");
    std::fs::create_dir_all(&checkout_vd).unwrap();
    std::fs::write(
        checkout_vd.join("PR-1619.json"),
        r#"{"verdict":"APPROVED","summary":"ok","mode":"orchestrator-phase-3"}"#,
    )
    .unwrap();
    let started = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    let out = crate::sibling_verdict_sweep_for_phase3(&root, 1619, "STORY-784", started);
    assert!(
        matches!(
            out,
            Some(crate::auto_complete::ReviewerOutcome::Verdict(
                crate::auto_complete::Verdict::Approved
            ))
        ),
        "the sibling checkout's fresh verdict must be accepted: {out:?}"
    );
    assert!(
        root.join(".aida/review-verdicts/PR-1619.json").is_file(),
        "the found verdict must be copied back to the drive root for audit + calibration"
    );
}

/// The freshness gate holds for the sweep too: a sibling verdict older than
/// the reviewer session start must never advance a later diff.
#[test]
fn a_stale_sibling_verdict_is_refused() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("aida");
    std::fs::create_dir_all(&root).unwrap();
    let checkout_vd = parent
        .path()
        .join("aida-pr-9")
        .join(".aida")
        .join("review-verdicts");
    std::fs::create_dir_all(&checkout_vd).unwrap();
    std::fs::write(
        checkout_vd.join("PR-9.json"),
        r#"{"verdict":"APPROVED","summary":"old","mode":"orchestrator-phase-3"}"#,
    )
    .unwrap();
    // Session "started" in the future relative to the file's mtime.
    let started = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
    assert!(
        crate::sibling_verdict_sweep_for_phase3(&root, 9, "BUG-9", started).is_none(),
        "a verdict recorded before this session is stale for the sweep too"
    );
}

/// A spec-keyed `review record` file in the sibling checkout works as well —
/// the sweep understands both shapes.
#[test]
fn a_sibling_spec_keyed_record_is_accepted() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("aida");
    std::fs::create_dir_all(&root).unwrap();
    let checkout_vd = parent
        .path()
        .join("review-co")
        .join(".aida")
        .join("review-verdicts");
    std::fs::create_dir_all(&checkout_vd).unwrap();
    std::fs::write(
        checkout_vd.join("STORY-1.json"),
        r#"{"verdict":"request-changes","reviewed_sha":"abc","recorded_at":"2026-08-29T18:02:00Z"}"#,
    )
    .unwrap();
    let started = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    let out = crate::sibling_verdict_sweep_for_phase3(&root, 42, "STORY-1", started);
    assert!(
        matches!(
            out,
            Some(crate::auto_complete::ReviewerOutcome::Verdict(
                crate::auto_complete::Verdict::RequestChanges
            ))
        ),
        "a fresh spec-keyed sibling record must be accepted: {out:?}"
    );
}

/// The drive root itself is never treated as a "sibling" (already consulted),
/// and unrelated garbage in sibling dirs falls through to None.
#[test]
fn the_sweep_skips_the_drive_root_and_garbage() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("aida");
    let root_vd = root.join(".aida").join("review-verdicts");
    std::fs::create_dir_all(&root_vd).unwrap();
    // A fresh file at the ROOT would have been read by the primary path;
    // the sweep must not double-report it as a sibling find.
    std::fs::write(root_vd.join("PR-5.json"), r#"{"verdict":"APPROVED"}"#).unwrap();
    let sibling_vd = parent
        .path()
        .join("junk")
        .join(".aida")
        .join("review-verdicts");
    std::fs::create_dir_all(&sibling_vd).unwrap();
    std::fs::write(sibling_vd.join("PR-5.json"), "not json").unwrap();
    let started = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    assert!(
        crate::sibling_verdict_sweep_for_phase3(&root, 5, "BUG-5", started).is_none(),
        "root is not a sibling; garbage siblings must not produce a verdict"
    );
}

/// The prompt-text anchor: with the verdict-file env set, the reviewer prompt
/// gains an absolute-path anchor naming both the file and the directory to
/// run `aida review record` from; without it, nothing is appended.
#[test]
fn reviewer_prompt_anchor_names_the_absolute_verdict_path() {
    let _guard = crate::test_env::env_lock();
    std::env::set_var(
        "AIDA_REVIEW_VERDICT_FILE",
        "/home/u/proj/.aida/review-verdicts/PR-7.json",
    );
    let suffix = crate::queue_cmd::reviewer_verdict_anchor_suffix().expect("suffix renders");
    assert!(
        suffix.contains("/home/u/proj/.aida/review-verdicts/PR-7.json"),
        "anchor must carry the exact absolute verdict path: {suffix}"
    );
    assert!(
        suffix.contains("from `/home/u/proj`"),
        "anchor must name the drive root to run `review record` from: {suffix}"
    );
    std::env::remove_var("AIDA_REVIEW_VERDICT_FILE");
    assert!(
        crate::queue_cmd::reviewer_verdict_anchor_suffix().is_none(),
        "no env, no anchor"
    );
}
