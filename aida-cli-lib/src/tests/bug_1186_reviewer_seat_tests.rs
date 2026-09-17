//! BUG-1186 / ADR-40: reviewer-seat integrity on a rework re-drive.
//!
//! The drain's review phase must (1) always send the review envelope so
//! `queue work PR-N` never swaps the seat for an implementer pickup, (2) shelve
//! `reviewer-wrote` when the PR head moved under the reviewer, and (3) treat a
//! consumed review story as NOT blocking a fresh review round. The pure halves
//! are tested here; the wiring is pinned by a source-shape guard.
//! trace:BUG-1186 | ai:claude

use super::*;

#[test]
fn reviewer_wrote_message_fires_only_when_the_head_moved() {
    let pre = "35caf53ed0000000000000000000000000000000";
    assert!(reviewer_wrote_message(1882, pre, pre).is_none());
    // Casing / whitespace differences are not a move.
    assert!(reviewer_wrote_message(1882, pre, &format!("  {}  ", pre.to_uppercase())).is_none());
    // A missing SHA on either side cannot prove a move — the verdict stands.
    assert!(reviewer_wrote_message(1882, "", pre).is_none());
    assert!(reviewer_wrote_message(1882, pre, "   ").is_none());
    let msg = reviewer_wrote_message(1882, pre, "e1671b040ffffffffffffffffffffffffffffffff")
        .expect("a moved head is a breach");
    assert!(msg.contains("PR-1882"), "{msg}");
    assert!(
        msg.contains("35caf53ed") && msg.contains("e1671b040"),
        "{msg}"
    );
}

#[test]
fn review_story_round_is_open_ignores_consumed_rounds() {
    let rows = |status: &str, title: &str| {
        format!(r#"[{{"spec_id":"STORY-1180","title":"{title}","status":"{status}"}}]"#)
    };
    // The BUG-1186 shape: the round-1 review story was Done when the re-drive
    // ran, so `AlreadyExists` suppressed a fresh round and `queue work PR-1882`
    // fell through to the backing spec.
    for consumed in ["Done", "Completed", "Rejected", "Superseded", "In Progress"] {
        let open = review_story_round_is_open(&rows(consumed, "Review PR-1882: fix x"), 1882);
        assert_eq!(open, consumed == "In Progress", "status {consumed}");
    }
    for open_status in ["Draft", "Approved", "Planned", "NeedsAttention"] {
        assert!(
            review_story_round_is_open(&rows(open_status, "Review PR-1882: fix x"), 1882),
            "status {open_status} is an open round"
        );
    }
    // Title must be the review story for THIS PR (prefix match, case-insensitive).
    assert!(!review_story_round_is_open(
        &rows("Approved", "Review PR-18820: other"),
        1882
    ));
    assert!(!review_story_round_is_open(
        &rows("Approved", "Review PR-17: other"),
        1882
    ));
    assert!(review_story_round_is_open(
        &rows("approved", "review pr-1882: lower"),
        1882
    ));
    // Malformed / empty output never claims an open round.
    assert!(!review_story_round_is_open("not json", 1882));
    assert!(!review_story_round_is_open("[]", 1882));
}

#[test]
fn review_envelope_targets_pr_requires_truthy_flag_and_matching_number() {
    use crate::queue_cmd::review_envelope_targets_pr;
    assert!(review_envelope_targets_pr(Some("1"), Some("1882"), 1882));
    assert!(review_envelope_targets_pr(
        Some("yes"),
        Some(" 1882 "),
        1882
    ));
    assert!(!review_envelope_targets_pr(Some("1"), Some("17"), 1882));
    assert!(!review_envelope_targets_pr(Some("0"), Some("1882"), 1882));
    assert!(!review_envelope_targets_pr(None, Some("1882"), 1882));
    assert!(!review_envelope_targets_pr(Some("1"), None, 1882));
}

/// Source-shape guard: the review envelope must be set UNCONDITIONALLY in the
/// review phase — the BUG-1186 defect was the `if self.from_pr` gate around
/// it. The needle is split so this file cannot match its own literal.
#[test]
fn review_phase_sets_the_review_envelope_unconditionally() {
    let src = include_str!("../lib.rs");
    let gated = concat!(
        "if self.from_pr {\n            cmd.env(\"AIDA_FROM_PR_REVIEW\"",
        ", \"1\")"
    );
    assert!(
        !src.contains(gated),
        "AIDA_FROM_PR_REVIEW must not be gated on `self.from_pr` (BUG-1186)"
    );
    let unconditional = concat!("        cmd.env(\"AIDA_FROM_PR_REVIEW\"", ", \"1\")\n");
    assert!(
        src.contains(unconditional),
        "review envelope must be set for every review phase"
    );
    // And the post-review integrity guard is wired to the typed cause.
    assert!(src.contains(concat!("auto_complete::FailureKind::", "ReviewerWrote,")));
}
