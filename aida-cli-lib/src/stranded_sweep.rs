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

use crate::review_verdict::{self, RecordedVerdict, TipRelation, VerdictKind};

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

/// TASK-1423: which specs carry a review refusal that no subsequent commit
/// has answered, and for how long — the offline counterpart to
/// [`StrandedRow`]/`run_stranded_sweep`'s forge-backed report. Reads the same
/// per-spec verdict files, but the "has this moved on?" check comes from
/// LOCAL git (a branch ref already on disk) instead of asking the forge for
/// the PR's live head — see `local_branch_tip` in `lib.rs` — so the answer
/// stays available offline and fast even after the authoring session has
/// exited and no queue entry or hold exists to say so.
// trace:TASK-1423 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct StalledRow {
    pub(crate) spec_id: String,
    pub(crate) verdict_kind: VerdictKind,
    pub(crate) reviewed_sha: Option<String>,
    pub(crate) reviewed_branch: Option<String>,
    pub(crate) recorded_at: Option<String>,
    pub(crate) summary: Option<String>,
    /// Elapsed seconds since `recorded_at`, when it parses. `None` means the
    /// stall duration could not be measured — reported as "unknown" rather
    /// than guessed or omitted (PRIN-5).
    pub(crate) stall_secs: Option<i64>,
    /// Past `awaiting_you::REFUSAL_OVERDUE_SECS`. `false` when `stall_secs`
    /// is `None` — an unmeasured stall is never claimed overdue.
    pub(crate) overdue: bool,
}

/// TASK-1423: does `verdict` represent a review refusal with NO subsequent
/// run? Two independent things both have to hold:
///
///   - [`review_verdict::is_outstanding_refusal`] (BUG-1529's test: blocking,
///     not already closed by a merge, and the spec itself isn't Completed) —
///     the same "is this refusal still live" test the awaiting-you PR-review
///     surface already uses, reused rather than re-derived, and
///   - `relation` is exactly [`TipRelation::AtReviewedSha`] — the branch's
///     tip (resolved from LOCAL git; see `local_branch_tip` in `lib.rs`) IS
///     the reviewed commit. `AdvancedPast` and `Rewritten` both mean a
///     subsequent run already happened; `Unknown` means this reader cannot
///     prove one didn't (no `reviewed_sha`, no resolvable branch, …) — and an
///     unproven claim of "stalled" is not reported, the same fail-closed
///     stance [`classify_stranded`] already takes on `verdict_refusing_at_head`.
// trace:TASK-1423 | ai:claude
pub(crate) fn is_stalled(
    verdict: &RecordedVerdict,
    spec_completed: bool,
    relation: TipRelation,
) -> bool {
    review_verdict::is_outstanding_refusal(verdict, spec_completed)
        && matches!(relation, TipRelation::AtReviewedSha)
}

/// TASK-1423 acceptance: "sorted by staleness descending, because the single
/// most useful output is the oldest one." Rows whose duration is unmeasured
/// (`stall_secs: None`) sort LAST — never fabricated a position by guessing
/// they're either the oldest or the freshest. Ties (including all-`None`)
/// break on `spec_id` for a deterministic, diffable report.
// trace:TASK-1423 | ai:claude
pub(crate) fn sort_by_staleness(rows: &mut [StalledRow]) {
    rows.sort_by(|a, b| match (a.stall_secs, b.stall_secs) {
        (Some(x), Some(y)) => y.cmp(&x).then_with(|| a.spec_id.cmp(&b.spec_id)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.spec_id.cmp(&b.spec_id),
    });
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

    // trace:TASK-1423 | ai:claude
    fn stalled_row(spec_id: &str, stall_secs: Option<i64>) -> StalledRow {
        StalledRow {
            spec_id: spec_id.to_string(),
            verdict_kind: VerdictKind::RequestChanges,
            reviewed_sha: Some("aaaa".to_string()),
            reviewed_branch: Some("claude/task-1".to_string()),
            recorded_at: None,
            summary: None,
            stall_secs,
            overdue: false,
        }
    }

    #[test]
    fn a_refusal_at_the_reviewed_head_with_no_merge_or_completion_is_stalled() {
        let v = verdict(VerdictKind::RequestChanges, "aaaa");
        assert!(is_stalled(&v, false, TipRelation::AtReviewedSha));
    }

    #[test]
    fn a_refusal_closed_by_a_later_merge_is_not_stalled() {
        let mut v = verdict(VerdictKind::RequestChanges, "aaaa");
        v.closed_by_merge = Some("bbbb".to_string());
        assert!(!is_stalled(&v, false, TipRelation::AtReviewedSha));
    }

    #[test]
    fn a_refusal_on_a_completed_spec_is_not_stalled() {
        let v = verdict(VerdictKind::RequestChanges, "aaaa");
        assert!(!is_stalled(&v, true, TipRelation::AtReviewedSha));
    }

    #[test]
    fn an_approval_is_never_stalled() {
        let v = verdict(VerdictKind::Approved, "aaaa");
        assert!(!is_stalled(&v, false, TipRelation::AtReviewedSha));
    }

    #[test]
    fn a_branch_that_has_advanced_past_the_review_is_not_stalled() {
        let v = verdict(VerdictKind::RequestChanges, "aaaa");
        assert!(!is_stalled(&v, false, TipRelation::AdvancedPast));
    }

    #[test]
    fn a_rewritten_branch_is_not_stalled() {
        let v = verdict(VerdictKind::RequestChanges, "aaaa");
        assert!(!is_stalled(&v, false, TipRelation::Rewritten));
    }

    // The population an unresolvable local branch cannot distinguish from a
    // real subsequent run — PRIN-5: absent evidence is not reported as a
    // positive "stalled" claim.
    #[test]
    fn an_unresolvable_branch_is_not_reported_as_stalled() {
        let v = verdict(VerdictKind::RequestChanges, "aaaa");
        assert!(!is_stalled(&v, false, TipRelation::Unknown));
    }

    #[test]
    fn staleness_sort_puts_the_oldest_first_and_unknown_last() {
        let mut rows = vec![
            stalled_row("TASK-2", Some(60)),
            stalled_row("TASK-4", None),
            stalled_row("TASK-1", Some(3600)),
            stalled_row("TASK-3", None),
        ];
        sort_by_staleness(&mut rows);
        let ids: Vec<&str> = rows.iter().map(|r| r.spec_id.as_str()).collect();
        assert_eq!(ids, vec!["TASK-1", "TASK-2", "TASK-3", "TASK-4"]);
    }

    #[test]
    fn staleness_sort_breaks_ties_by_spec_id() {
        let mut rows = vec![
            stalled_row("TASK-9", Some(100)),
            stalled_row("TASK-5", Some(100)),
        ];
        sort_by_staleness(&mut rows);
        let ids: Vec<&str> = rows.iter().map(|r| r.spec_id.as_str()).collect();
        assert_eq!(ids, vec!["TASK-5", "TASK-9"]);
    }
}
