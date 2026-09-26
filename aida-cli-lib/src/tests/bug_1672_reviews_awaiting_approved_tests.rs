//! BUG-1672: `aida human` listed specs under reviews-awaiting after an
//! independent review had already approved them. The awaiting-review /
//! awaiting-merge split read only the drain's `PR-<n>.json` verdict file; a
//! keyboard `aida review` records under `<SPEC>.json`, and a fresh-reviewer
//! subagent or the orchestrator's delta check posts only a comment on the
//! spec. All three are now consulted through `spec_review_approved`.
//!
//! Fixtures: a temporary project root for the verdict files (never the live
//! `.aida/`), and in-memory `Requirement` / `Comment` values for the comment
//! trail. No git, no forge, no network.
// trace:BUG-1672 | ai:claude

use super::{latest_review_comment_verdict, review_comment_verdict, spec_review_approved};
use crate::review_verdict::VerdictKind;
use aida_core::{Comment, Requirement};
use chrono::{Duration, Utc};
use tempfile::TempDir;

const SPEC: &str = "BUG-1";
const PR: u64 = 42;

/// A comment by `author`, posted `minutes_ago` minutes before now, so the
/// trail's ordering is explicit rather than a race on `Utc::now()`.
fn comment(author: &str, content: &str, minutes_ago: i64) -> Comment {
    let mut c = Comment::new(author.to_string(), content.to_string());
    c.created_at = Utc::now() - Duration::minutes(minutes_ago);
    c.modified_at = c.created_at;
    c
}

fn spec_with(owner: &str, comments: Vec<Comment>) -> Requirement {
    let mut req = Requirement::new(
        "reviews-awaiting fixture".to_string(),
        "fixture".to_string(),
    );
    req.spec_id = Some(SPEC.to_string());
    req.owner = owner.to_string();
    req.comments = comments;
    req
}

fn write_verdict(root: &std::path::Path, key: &str, body: &str) {
    let dir = root.join(".aida").join("review-verdicts");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{key}.json")), body).unwrap();
}

// ---------------------------------------------------------------- comment parsing

#[test]
fn review_comment_words_parse_to_the_canonical_verdicts() {
    assert_eq!(
        review_comment_verdict("Review (fresh Opus reviewer): APPROVE WITH NITS on 6895a1a56a. (1) Root cause confirmed"),
        Some(VerdictKind::Approved),
        "APPROVE WITH NITS is the review skill's accepted-with-nits wording"
    );
    assert_eq!(
        review_comment_verdict(
            "Review: APPROVE (strict, independent; cae2dd13d1). Verified the re-read"
        ),
        Some(VerdictKind::Approved)
    );
    assert_eq!(
        review_comment_verdict("Review round 3 (fresh strict Opus reviewer): REQUEST_CHANGES on origin/claude/task-1515 @ 3fec216b73."),
        Some(VerdictKind::RequestChanges)
    );
    assert_eq!(
        review_comment_verdict("Review (fresh Opus reviewer): PARTIAL, leaning REQUEST CHANGES (session ended before I ran any tests)."),
        Some(VerdictKind::RequestChanges),
        "PARTIAL and a spaced REQUEST CHANGES both block"
    );
    assert_eq!(
        review_comment_verdict("Review: REJECT — the change reverts BUG-1."),
        Some(VerdictKind::Rejected)
    );
    assert_eq!(
        review_comment_verdict("Orchestrator delta check (proxy for Joe) of 440287d484..4e922d701c: APPROVED. doctor_cmd.rs only"),
        Some(VerdictKind::Approved),
        "the orchestrator's delta check is a review from a non-author seat"
    );
}

#[test]
fn qualified_or_non_review_comments_never_approve() {
    assert_eq!(
        review_comment_verdict("PROXY DECISION (orchestrator for Joe): review APPROVE WITH NITS accepted; joins batch 99."),
        None,
        "a proxy decision quoting the review is not itself a review"
    );
    assert_eq!(
        review_comment_verdict("REVIEW FINDINGS TO ADDRESS (round 2):\n- fix the thing"),
        None,
        "the rework findings block is not a verdict"
    );
    assert_eq!(
        review_comment_verdict("Review: CONTENT APPROVED — MERGE WITHHELD FOR INDEPENDENCE"),
        Some(VerdictKind::Unknown),
        "a qualified approval is not an approval"
    );
    assert!(
        !review_comment_verdict("Review: looked at the diff, no verdict yet")
            .is_some_and(|k| k.approves()),
        "a review comment with no verdict word does not approve"
    );
    assert_eq!(review_comment_verdict(""), None);
}

#[test]
fn newest_review_comment_decides_and_owner_is_excluded() {
    // Round 3 REQUEST_CHANGES then round 4 APPROVE → approved.
    let trail = vec![
        comment(
            "bob",
            "Review round 3 (fresh strict reviewer): REQUEST_CHANGES on @ 3fec216b73.",
            30,
        ),
        comment("joe", "PROXY DECISION: rework before merge.", 20),
        comment(
            "bob",
            "Review round 4 (fresh strict reviewer): APPROVE WITH NITS on @ 3d56853e55.",
            10,
        ),
    ];
    assert_eq!(
        latest_review_comment_verdict(&trail, None),
        Some(VerdictKind::Approved)
    );
    // The same trail in reverse time order: the approval came first, then a
    // request-changes → still blocked.
    let trail = vec![
        comment("bob", "Review: APPROVE on abc123.", 30),
        comment("bob", "Review round 2: REQUEST_CHANGES on def456.", 10),
    ];
    assert_eq!(
        latest_review_comment_verdict(&trail, None),
        Some(VerdictKind::RequestChanges)
    );
    // The owner's own approval is not an independent review.
    let trail = vec![comment("alice", "Review: APPROVE, ship it.", 5)];
    assert_eq!(latest_review_comment_verdict(&trail, Some("alice")), None);
    assert_eq!(
        latest_review_comment_verdict(&trail, Some("bob")),
        Some(VerdictKind::Approved)
    );
    assert_eq!(
        latest_review_comment_verdict(&trail, Some("  ")),
        Some(VerdictKind::Approved),
        "a blank owner excludes nobody"
    );
}

// ------------------------------------------------- the three acceptance cases

/// Acceptance 1: latest review is APPROVE from a non-author reviewer, no
/// verdict file at all → NOT reviews-awaiting (it is awaiting merge).
#[test]
fn approved_by_comment_is_not_reviews_awaiting() {
    let tmp = TempDir::new().unwrap();
    let req = spec_with(
        "alice",
        vec![
            comment(
                "bob",
                "Review (fresh Opus reviewer): REQUEST_CHANGES on aaa.",
                40,
            ),
            comment(
                "bob",
                "Review: APPROVE (strict, independent; bbb). Verified.",
                10,
            ),
        ],
    );
    assert!(
        spec_review_approved(tmp.path(), &req, SPEC, PR),
        "an independent APPROVE comment with no verdict file must read as reviewed"
    );
}

/// Acceptance 2: REQUEST_CHANGES as the latest review keeps it listed, and so
/// does a trail with no review at all.
#[test]
fn request_changes_or_no_review_stays_reviews_awaiting() {
    let tmp = TempDir::new().unwrap();
    let req = spec_with(
        "alice",
        vec![
            comment("bob", "Review: APPROVE on aaa.", 40),
            comment("bob", "Review round 2: REQUEST_CHANGES on bbb.", 10),
        ],
    );
    assert!(
        !spec_review_approved(tmp.path(), &req, SPEC, PR),
        "a request-changes after the approval keeps the spec awaiting review"
    );

    let req = spec_with(
        "alice",
        vec![
            comment(
                "joe",
                "PROXY DECISION (orchestrator for Joe): approved and queued; low.",
                40,
            ),
            comment(
                "alice",
                "Implemented on claude/bug-1 @ ccc; ready for review.",
                10,
            ),
        ],
    );
    assert!(
        !spec_review_approved(tmp.path(), &req, SPEC, PR),
        "no review comment and no verdict file: still awaiting review"
    );

    // The owner's own APPROVE comment is not an independent review either.
    let req = spec_with(
        "alice",
        vec![comment("alice", "Review: APPROVE, ship it.", 5)],
    );
    assert!(!spec_review_approved(tmp.path(), &req, SPEC, PR));
}

/// Acceptance 1 via the verdict files: the spec-keyed record `aida review`
/// writes, and (regression) the drain's PR-keyed record, both approve; a
/// blocking or merge-closed spec-keyed record does not.
#[test]
fn verdict_files_under_either_key_decide_before_comments() {
    let tmp = TempDir::new().unwrap();
    let req = spec_with("alice", Vec::new());

    // Spec-keyed approval (the `aida review <SPEC>` path) → reviewed.
    write_verdict(tmp.path(), SPEC, r#"{"verdict":"Approved","summary":"ok"}"#);
    assert!(
        spec_review_approved(tmp.path(), &req, SPEC, PR),
        "the spec-keyed verdict file must count as an approval"
    );

    // Spec-keyed request-changes, no other record → still awaiting review.
    write_verdict(
        tmp.path(),
        SPEC,
        r#"{"verdict":"request-changes","findings":["x"]}"#,
    );
    assert!(!spec_review_approved(tmp.path(), &req, SPEC, PR));

    // A spec-keyed approval already closed out by a merge is not a fresh one.
    write_verdict(
        tmp.path(),
        SPEC,
        r#"{"verdict":"Approved","closed_by_merge":"deadbeef","closed_at":"2026-09-26T00:00:00Z"}"#,
    );
    assert!(!spec_review_approved(tmp.path(), &req, SPEC, PR));

    // PR-keyed approval (the drain's record) still approves on its own.
    let tmp = TempDir::new().unwrap();
    write_verdict(
        tmp.path(),
        &format!("PR-{PR}"),
        r#"{"verdict":"APPROVED","summary":"ok"}"#,
    );
    assert!(
        spec_review_approved(tmp.path(), &req, SPEC, PR),
        "the original PR-keyed signal must keep working"
    );

    // A file approval wins over a later request-changes comment: files are
    // explicit records, prose is the fallback.
    let req = spec_with(
        "alice",
        vec![comment("bob", "Review: REQUEST_CHANGES on zzz.", 1)],
    );
    assert!(spec_review_approved(tmp.path(), &req, SPEC, PR));
}
