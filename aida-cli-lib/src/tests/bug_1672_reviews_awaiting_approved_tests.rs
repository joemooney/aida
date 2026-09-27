//! BUG-1672: `aida human` listed specs under reviews-awaiting after an
//! independent review had already approved them. The awaiting-review /
//! awaiting-merge split read only the drain's `PR-<n>.json` verdict file; a
//! keyboard `aida review` records under `<SPEC>.json`, and a fresh-reviewer
//! subagent posts only a comment on the spec. All three are now consulted
//! through `spec_review_approved`, and the newest review wins.
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

fn write_verdict(root: &std::path::Path, key: &str, body: &str) -> std::path::PathBuf {
    let dir = root.join(".aida").join("review-verdicts");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{key}.json"));
    std::fs::write(&path, body).unwrap();
    path
}

/// Pin a verdict file's modification time `minutes_ago` minutes back, so the
/// file-versus-comment ordering is explicit.
fn age_file(path: &std::path::Path, minutes_ago: i64) {
    let at = std::time::SystemTime::now()
        - std::time::Duration::from_secs(u64::try_from(minutes_ago * 60).unwrap());
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(at).unwrap();
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
        review_comment_verdict("Review round 4 (fresh strict Opus reviewer): APPROVE WITH NITS on origin/claude/task-1515 @ 3d56853e55."),
        Some(VerdictKind::Approved)
    );
    assert_eq!(
        review_comment_verdict("VERDICT: APPROVED\nsummary follows"),
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
        review_comment_verdict("Review: REJECT — the change reverts it."),
        Some(VerdictKind::Rejected)
    );
    assert_eq!(
        review_comment_verdict("VERDICT: REQUEST_CHANGES"),
        Some(VerdictKind::RequestChanges)
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
        review_comment_verdict("Orchestrator delta check (proxy for Joe) of 440287d484..4e922d701c: APPROVED. doctor_cmd.rs only"),
        None,
        "only comments that start as a review or a VERDICT line count"
    );
    assert_eq!(
        review_comment_verdict("Reviewer notes: APPROVE looks likely once CI is green"),
        None,
        "a head merely starting with the letters of review is not a review"
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
fn newest_review_comment_decides() {
    // Round 3 REQUEST_CHANGES then round 4 APPROVE, listed out of order.
    let trail = vec![
        comment(
            "joe",
            "Review round 4 (fresh strict reviewer): APPROVE WITH NITS on @ 3d56853e55.",
            10,
        ),
        comment(
            "joe",
            "Review round 3 (fresh strict reviewer): REQUEST_CHANGES on @ 3fec216b73.",
            30,
        ),
        comment("joe", "PROXY DECISION: rework before merge.", 5),
    ];
    assert_eq!(
        latest_review_comment_verdict(&trail).map(|(k, _)| k),
        Some(VerdictKind::Approved),
        "the newest REVIEW comment decides; a later non-review comment does not"
    );
    // The approval came first, then a request-changes: still blocked.
    let trail = vec![
        comment("joe", "Review: APPROVE on abc123.", 30),
        comment("joe", "Review round 2: REQUEST_CHANGES on def456.", 10),
    ];
    assert_eq!(
        latest_review_comment_verdict(&trail).map(|(k, _)| k),
        Some(VerdictKind::RequestChanges)
    );
    assert_eq!(
        latest_review_comment_verdict(&[comment("joe", "Implemented; ready.", 1)]),
        None
    );
}

// ------------------------------------------------- the three acceptance cases

/// Acceptance 1: the latest review is an APPROVE and there is no verdict file
/// at all: NOT reviews-awaiting (it is awaiting merge). The author is the
/// operator's name, as on the real trails, and still counts.
#[test]
fn approved_by_comment_is_not_reviews_awaiting() {
    let tmp = TempDir::new().unwrap();
    let req = spec_with(
        "",
        vec![
            comment(
                "joe",
                "Review (fresh Opus reviewer): REQUEST_CHANGES on aaa.",
                40,
            ),
            comment(
                "joe",
                "Review: APPROVE (strict, independent; bbb). Verified.",
                10,
            ),
        ],
    );
    assert!(
        spec_review_approved(tmp.path(), &req, SPEC, PR),
        "an APPROVE review comment with no verdict file must read as reviewed"
    );
}

/// Acceptance 2: REQUEST_CHANGES as the latest review keeps it listed, and so
/// does a trail with no review at all.
#[test]
fn request_changes_or_no_review_stays_reviews_awaiting() {
    let tmp = TempDir::new().unwrap();
    let req = spec_with(
        "",
        vec![
            comment("joe", "Review: APPROVE on aaa.", 40),
            comment("joe", "Review round 2: REQUEST_CHANGES on bbb.", 10),
        ],
    );
    assert!(
        !spec_review_approved(tmp.path(), &req, SPEC, PR),
        "a request-changes after the approval keeps the spec awaiting review"
    );

    let req = spec_with(
        "",
        vec![
            comment(
                "joe",
                "PROXY DECISION (orchestrator for Joe): approved and queued; low.",
                40,
            ),
            comment(
                "joe",
                "Implemented on claude/bug-1 @ ccc; ready for review.",
                10,
            ),
        ],
    );
    assert!(
        !spec_review_approved(tmp.path(), &req, SPEC, PR),
        "no review comment and no verdict file: still awaiting review"
    );
}

/// Acceptance 1 via the verdict files: the spec-keyed record `aida review`
/// writes, and (regression) the drain's PR-keyed record, both approve; a
/// blocking or merge-closed spec-keyed record does not.
#[test]
fn verdict_files_under_either_key_approve() {
    let tmp = TempDir::new().unwrap();
    let req = spec_with("", Vec::new());

    write_verdict(tmp.path(), SPEC, r#"{"verdict":"Approved","summary":"ok"}"#);
    assert!(
        spec_review_approved(tmp.path(), &req, SPEC, PR),
        "the spec-keyed verdict file must count as an approval"
    );

    write_verdict(
        tmp.path(),
        SPEC,
        r#"{"verdict":"request-changes","findings":["x"]}"#,
    );
    assert!(!spec_review_approved(tmp.path(), &req, SPEC, PR));

    write_verdict(
        tmp.path(),
        SPEC,
        r#"{"verdict":"Approved","closed_by_merge":"deadbeef","closed_at":"2026-09-26T00:00:00Z"}"#,
    );
    assert!(
        !spec_review_approved(tmp.path(), &req, SPEC, PR),
        "an approval a merge already closed out is not a fresh one"
    );

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
}

/// The newest review wins across files and comments: a request-changes
/// comment posted after a file approval keeps the spec listed; a file
/// approval written after an older request-changes comment clears it.
#[test]
fn newest_review_wins_between_files_and_comments() {
    let blocking = spec_with(
        "",
        vec![comment("joe", "Review: REQUEST_CHANGES on zzz.", 10)],
    );

    for key in [SPEC.to_string(), format!("PR-{PR}")] {
        let tmp = TempDir::new().unwrap();
        let path = write_verdict(tmp.path(), &key, r#"{"verdict":"Approved"}"#);

        age_file(&path, 30);
        assert!(
            !spec_review_approved(tmp.path(), &blocking, SPEC, PR),
            "{key}: a request-changes comment newer than the approving file keeps it listed"
        );

        age_file(&path, 1);
        assert!(
            spec_review_approved(tmp.path(), &blocking, SPEC, PR),
            "{key}: an approving file newer than the request-changes comment clears it"
        );
    }
}

// trace:BUG-1672 | ai:codex
#[test]
fn blocking_files_override_older_approving_comments() {
    for key in [SPEC.to_string(), format!("PR-{PR}")] {
        for verdict in ["request-changes", "rejected", "unknown"] {
            let tmp = TempDir::new().unwrap();
            let req = spec_with("", vec![comment("joe", "Review: APPROVE", 20)]);
            let path = write_verdict(tmp.path(), &key, &format!(r#"{{"verdict":"{verdict}"}}"#));
            age_file(&path, 10);
            assert!(
                !spec_review_approved(tmp.path(), &req, SPEC, PR),
                "{key}: newer {verdict}"
            );
            age_file(&path, 30);
            assert!(
                spec_review_approved(tmp.path(), &req, SPEC, PR),
                "{key}: newer comment approval"
            );
        }
    }
}

// trace:BUG-1672 | ai:codex
#[test]
fn newest_review_wins_across_spec_and_pr_files() {
    for (approval_key, blocker_key) in [(SPEC, "PR-42"), ("PR-42", SPEC)] {
        for verdict in ["request-changes", "rejected", "unknown"] {
            let tmp = TempDir::new().unwrap();
            let req = spec_with("", Vec::new());
            let approval = write_verdict(tmp.path(), approval_key, r#"{"verdict":"approved"}"#);
            let blocker = write_verdict(
                tmp.path(),
                blocker_key,
                &format!(r#"{{"verdict":"{verdict}"}}"#),
            );
            age_file(&approval, 30);
            age_file(&blocker, 10);
            assert!(
                !spec_review_approved(tmp.path(), &req, SPEC, PR),
                "{blocker_key}: newer {verdict}"
            );
            age_file(&approval, 1);
            assert!(
                spec_review_approved(tmp.path(), &req, SPEC, PR),
                "{approval_key}: newer approval"
            );
        }
    }
}

// trace:BUG-1672 | ai:codex
#[test]
fn equal_review_times_preserve_blocker_in_any_source_order() {
    let at = Utc::now() - Duration::minutes(10);
    let mut approval = comment("joe", "Review: APPROVE", 10);
    let mut blocker = comment("joe", "Review: REQUEST_CHANGES", 10);
    approval.created_at = at;
    blocker.created_at = at;
    for comments in [
        vec![approval.clone(), blocker.clone()],
        vec![blocker, approval],
    ] {
        let tmp = TempDir::new().unwrap();
        assert!(!spec_review_approved(
            tmp.path(),
            &spec_with("", comments),
            SPEC,
            PR
        ));
    }
    for (approval_key, blocker_key) in [(SPEC, "PR-42"), ("PR-42", SPEC)] {
        let tmp = TempDir::new().unwrap();
        for (key, verdict) in [(approval_key, "approved"), (blocker_key, "request-changes")] {
            let path = write_verdict(tmp.path(), key, &format!(r#"{{"verdict":"{verdict}"}}"#));
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(at.into())
                .unwrap();
        }
        assert!(!spec_review_approved(
            tmp.path(),
            &spec_with("", Vec::new()),
            SPEC,
            PR
        ));
    }
}

// trace:BUG-1672 | ai:codex
#[test]
fn missing_times_cannot_clear_a_blocker() {
    use super::review_candidates_approved;
    let at = Utc::now();
    for candidates in [
        vec![(true, Some(at)), (false, None)],
        vec![(true, None), (false, Some(at))],
        vec![(true, None), (false, None)],
    ] {
        assert!(!review_candidates_approved(&candidates));
        let reversed: Vec<_> = candidates.into_iter().rev().collect();
        assert!(!review_candidates_approved(&reversed));
    }
    assert!(review_candidates_approved(&[(true, None)]));
    assert!(!review_candidates_approved(&[]));
}

// trace:BUG-1672 | ai:codex
#[test]
fn all_spec_alias_files_participate_in_ordering() {
    let tmp = TempDir::new().unwrap();
    let mut req = spec_with("", Vec::new());
    req.agreed_id = Some("BUG-2".to_string());
    let approval = write_verdict(tmp.path(), SPEC, r#"{"verdict":"approved"}"#);
    let blocker = write_verdict(tmp.path(), "BUG-2", r#"{"verdict":"request-changes"}"#);
    age_file(&approval, 30);
    age_file(&blocker, 10);
    assert!(!spec_review_approved(tmp.path(), &req, SPEC, PR));
    age_file(&approval, 1);
    assert!(spec_review_approved(tmp.path(), &req, SPEC, PR));
}
