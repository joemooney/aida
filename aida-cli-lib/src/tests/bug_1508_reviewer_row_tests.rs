//! BUG-1508: `reviewer_row_actionability` wired end-to-end through a real
//! verdict file + real git refs -- not just the pure `review_actionability`
//! classifier (covered separately in `review_verdict_tests.rs`).
//!
//! AC3: a head that moves past a recorded (blocking) verdict must read
//! `NeedsReview` again, on the SAME row, with nothing to invalidate --
//! `reviewer_row_actionability` re-derives the branch head from git on
//! every call rather than caching it.
//! trace:BUG-1508 | ai:claude

use super::*;
use aida_core::{Relationship, RelationshipType, Requirement};
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

fn commit_sha(repo: &std::path::Path, rev: &str) -> String {
    let out = git(repo, &["rev-parse", rev]);
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A `Review PR-N: ...` story `implements` a covered spec (BUG-102/BUG-776
/// shape). Recording a blocking verdict against the covered spec's reviewed
/// sha reads the row as `AwaitingRework`; a follow-up commit that moves the
/// branch past that sha reads it as `NeedsReview` again -- AC3, and the
/// permanent-indeterminate direction (AC8) is covered separately in
/// `review_verdict_tests.rs`.
#[test]
fn review_story_row_needs_review_again_once_the_branch_moves_past_the_verdict() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git_ok(root, &["init", "--initial-branch=main", "--quiet"]);
    git_ok(root, &["commit", "--allow-empty", "-m", "root", "--quiet"]);
    git_ok(root, &["checkout", "-q", "-b", "feat"]);
    git_ok(
        root,
        &["commit", "--allow-empty", "-m", "reviewed", "--quiet"],
    );
    let reviewed_sha = commit_sha(root, "feat");

    let mut spec = Requirement::new("Fix the thing".to_string(), String::new());
    spec.agreed_id = Some("BUG-9001".to_string());
    let mut story = Requirement::new("Review PR-7: Fix the thing".to_string(), String::new());
    story.agreed_id = Some("STORY-9002".to_string());
    story.relationships.push(Relationship {
        rel_type: RelationshipType::Custom("implements".to_string()),
        target_id: spec.id,
        created_at: None,
        created_by: None,
    });

    review_verdict::record_verdict(
        root,
        "BUG-9001",
        Some("request-changes"),
        Some(&reviewed_sha),
        Some("feat"),
        Some("blocking finding"),
        &[],
        "reviewer-x",
    )
    .unwrap();

    let spec_id = spec.id;
    let agreed = spec.agreed_id.clone();

    // At the reviewed sha: the reviewer already spoke; this is rework, not
    // review (AC2 -- the row is annotated, not dropped).
    let at_reviewed = reviewer_row_actionability(root, &story, &[], |uuid| {
        if uuid == spec_id {
            agreed.clone()
        } else {
            None
        }
    });
    assert_eq!(
        at_reviewed,
        review_verdict::ReviewActionability::AwaitingRework
    );

    // The implementer pushes a follow-up commit -- the head moves past what
    // was reviewed. AC3: the row must read NeedsReview again.
    git_ok(
        root,
        &["commit", "--allow-empty", "-m", "addressed", "--quiet"],
    );
    let moved = reviewer_row_actionability(root, &story, &[], |uuid| {
        if uuid == spec_id {
            agreed.clone()
        } else {
            None
        }
    });
    assert_eq!(moved, review_verdict::ReviewActionability::NeedsReview);
}

/// A directly-routed row (no review-story wrapper) with no verdict on disk
/// at all is NeedsReview -- the base case AC1 exists to fix.
#[test]
fn direct_routed_row_with_no_verdict_needs_review() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git_ok(root, &["init", "--initial-branch=main", "--quiet"]);
    git_ok(root, &["commit", "--allow-empty", "-m", "root", "--quiet"]);

    let mut spec = Requirement::new("Fix the other thing".to_string(), String::new());
    spec.agreed_id = Some("BUG-9003".to_string());

    let state = reviewer_row_actionability(root, &spec, &[], |_| None);
    assert_eq!(state, review_verdict::ReviewActionability::NeedsReview);
}
