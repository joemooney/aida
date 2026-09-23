//! TASK-1307: find specs already stranded by a refusal that predates
//! BUG-1452's prevention — refused at the PR's CURRENT head, spec still
//! Done, no merge hold, no queue entry to re-drive it. BUG-1452 stops NEW
//! instances; this module is the cleanup half for ones that already
//! happened.
//!
//! The pure classifier lives here so acceptance criterion 6 (a fixture
//! proves the sweep finds the stranded state and does NOT flag a refusal
//! against a superseded head) is testable with no git, no gh, and no store.
//! The I/O — walking `.aida/review-verdicts/`, asking the forge for a PR's
//! live head sha, reading the merge-hold marker, scanning queues — lives in
//! `run_stranded_sweep` in `lib.rs`, which has the project's existing
//! store/forge plumbing already in scope.
// trace:TASK-1307 | ai:claude

use crate::review_verdict::{RecordedVerdict, TipRelation};

/// Which of the four TASK-1307 conditions hold for one spec. All four true
/// is the stranded state the sweep reports; any single false condition
/// means some other actor (BUG-1452's prevention, a human, a prior sweep
/// `--fix`) already recovered it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct StrandedConditions {
    /// The spec's status has not moved off Done.
    pub(crate) status_not_moved: bool,
    /// No `.aida/merge-holds/PR-<n>` marker protects the PR.
    pub(crate) hold_absent: bool,
    /// No queue entry, for any user/role, targets this spec.
    pub(crate) queue_entry_absent: bool,
    /// The last recorded verdict blocks done AND was recorded against the
    /// commit that IS STILL the PR's current head — not a superseded one.
    pub(crate) verdict_refusing_at_head: bool,
}

impl StrandedConditions {
    /// The state TASK-1307 defines as "stranded": every condition holds.
    pub(crate) fn is_stranded(self) -> bool {
        self.status_not_moved
            && self.hold_absent
            && self.queue_entry_absent
            && self.verdict_refusing_at_head
    }
}

/// Pure classifier. `relation` is [`crate::review_verdict::classify_tip_relation`]'s
/// answer for where the PR's live head sits relative to `verdict`'s
/// `reviewed_sha` — the caller resolves that from real git/forge state;
/// this function only decides. Acceptance criterion 2 (don't treat a
/// refusal at a superseded head as evidence the current PR is refused)
/// falls straight out of requiring `TipRelation::AtReviewedSha` exactly:
/// `AdvancedPast`, `Rewritten`, and `Unknown` all leave
/// `verdict_refusing_at_head` false.
// trace:TASK-1307 | ai:claude
pub(crate) fn classify_stranded(
    status_is_done: bool,
    hold_present: bool,
    queue_entry_present: bool,
    verdict: Option<&RecordedVerdict>,
    relation: TipRelation,
) -> StrandedConditions {
    let verdict_refusing_at_head = verdict
        .map(|v| v.kind.blocks_done() && matches!(relation, TipRelation::AtReviewedSha))
        .unwrap_or(false);
    StrandedConditions {
        status_not_moved: status_is_done,
        hold_absent: !hold_present,
        queue_entry_absent: !queue_entry_present,
        verdict_refusing_at_head,
    }
}

/// One reported row: a spec plus the evidence trail — acceptance 1's "which
/// of the four conditions hold" per row.
#[derive(Debug, Clone)]
pub(crate) struct StrandedRow {
    pub(crate) spec_id: String,
    pub(crate) pr: u64,
    pub(crate) conditions: StrandedConditions,
    pub(crate) verdict_summary: Option<String>,
    pub(crate) reviewed_sha: Option<String>,
    pub(crate) current_head: Option<String>,
}

/// A `.aida/review-verdicts/<STEM>.json` file is spec-keyed
/// (`review_verdict::verdict_path` writes `<SPEC-ID>.json`) unless `<STEM>`
/// is the orchestrator's PR-anchored handshake shape (`PR-<digits>`,
/// written by `record_verdict_at_path` for the phase-3 handshake). The
/// sweep only cares about spec-keyed records — a PR-anchored file carries
/// no spec id to look up in the store.
// trace:TASK-1307 | ai:claude
pub(crate) fn is_spec_keyed_verdict_filename(stem: &str) -> bool {
    let upper = stem.to_ascii_uppercase();
    let Some(digits) = upper.strip_prefix("PR-") else {
        return true;
    };
    // An all-digit suffix is the PR-anchored handshake shape — exclude it.
    // Anything else ("PR-REVIEW-NOTES") is a spec id that merely starts
    // with "PR-" and stays included.
    digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review_verdict::VerdictKind;

    fn verdict(kind: VerdictKind, sha: &str) -> RecordedVerdict {
        RecordedVerdict {
            kind,
            raw: kind.label().to_string(),
            reviewed_sha: Some(sha.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn all_four_conditions_true_is_stranded() {
        let v = verdict(VerdictKind::RequestChanges, "aaaa");
        let c = classify_stranded(true, false, false, Some(&v), TipRelation::AtReviewedSha);
        assert!(c.is_stranded());
        assert!(c.status_not_moved);
        assert!(c.hold_absent);
        assert!(c.queue_entry_absent);
        assert!(c.verdict_refusing_at_head);
    }

    // Acceptance criterion 2 / 6: a refusal against a superseded head must
    // NOT be flagged — the branch moved past it and rework may already be
    // in progress.
    #[test]
    fn a_refusal_against_a_superseded_head_is_not_flagged() {
        let v = verdict(VerdictKind::RequestChanges, "aaaa");
        let c = classify_stranded(true, false, false, Some(&v), TipRelation::AdvancedPast);
        assert!(!c.is_stranded());
        assert!(!c.verdict_refusing_at_head);
    }

    #[test]
    fn a_rewritten_head_is_not_flagged() {
        let v = verdict(VerdictKind::RequestChanges, "aaaa");
        let c = classify_stranded(true, false, false, Some(&v), TipRelation::Rewritten);
        assert!(!c.is_stranded());
    }

    #[test]
    fn an_unknown_relation_is_not_flagged() {
        let v = verdict(VerdictKind::RequestChanges, "aaaa");
        let c = classify_stranded(true, false, false, Some(&v), TipRelation::Unknown);
        assert!(!c.is_stranded());
    }

    #[test]
    fn a_hold_already_present_clears_the_state() {
        let v = verdict(VerdictKind::RequestChanges, "aaaa");
        let c = classify_stranded(true, true, false, Some(&v), TipRelation::AtReviewedSha);
        assert!(!c.is_stranded());
        assert!(!c.hold_absent);
    }

    #[test]
    fn a_queue_entry_already_present_clears_the_state() {
        let v = verdict(VerdictKind::RequestChanges, "aaaa");
        let c = classify_stranded(true, false, true, Some(&v), TipRelation::AtReviewedSha);
        assert!(!c.is_stranded());
        assert!(!c.queue_entry_absent);
    }

    #[test]
    fn status_already_moved_off_done_clears_the_state() {
        let v = verdict(VerdictKind::RequestChanges, "aaaa");
        let c = classify_stranded(false, false, false, Some(&v), TipRelation::AtReviewedSha);
        assert!(!c.is_stranded());
        assert!(!c.status_not_moved);
    }

    #[test]
    fn an_approving_verdict_never_flags() {
        let v = verdict(VerdictKind::Approved, "aaaa");
        let c = classify_stranded(true, false, false, Some(&v), TipRelation::AtReviewedSha);
        assert!(!c.is_stranded());
    }

    #[test]
    fn no_verdict_never_flags() {
        let c = classify_stranded(true, false, false, None, TipRelation::Unknown);
        assert!(!c.is_stranded());
    }

    #[test]
    fn spec_keyed_filenames_are_recognized() {
        assert!(is_spec_keyed_verdict_filename("TASK-1298"));
        assert!(is_spec_keyed_verdict_filename("BUG-1310"));
        assert!(is_spec_keyed_verdict_filename("STORY-1350"));
        // Not a PR-anchored handshake just because it starts with "PR-" —
        // only an all-digit suffix is the handshake shape.
        assert!(is_spec_keyed_verdict_filename("PR-REVIEW-NOTES"));
    }

    #[test]
    fn pr_anchored_handshake_filenames_are_excluded() {
        assert!(!is_spec_keyed_verdict_filename("PR-2014"));
        assert!(!is_spec_keyed_verdict_filename("pr-2014"));
    }
}
