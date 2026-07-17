use super::reconcile_verdict;
use crate::auto_complete::PhaseReconcile;

#[test]
fn merged_pr_and_completed_spec_is_shipped() {
    match reconcile_verdict(Some(94), true, "BUG-233") {
        PhaseReconcile::ShippedOutOfBand { reason } => {
            assert!(reason.contains("PR-94"), "{reason}");
            assert!(reason.contains("BUG-233"), "{reason}");
        }
        other => panic!("expected ShippedOutOfBand, got {other:?}"),
    }
}

#[test]
fn verified_merged_pr_alone_is_shipped() {
    // Instance A: the human merged out-of-band; the status auto-bump may
    // lag, so a merged PR that credits the dispatched spec is proof
    // enough.
    assert!(matches!(
        reconcile_verdict(Some(94), false, "BUG-233"),
        PhaseReconcile::ShippedOutOfBand { .. }
    ));
}

#[test]
fn completed_spec_with_no_pr_is_shipped() {
    // Instance B: resolved-by-supersession — no PR was ever needed.
    assert!(matches!(
        reconcile_verdict(None, true, "BUG-230"),
        PhaseReconcile::ShippedOutOfBand { .. }
    ));
}

#[test]
fn no_pr_and_open_spec_is_a_genuine_failure() {
    // The regression guard: reality confirms nothing shipped.
    assert_eq!(
        reconcile_verdict(None, false, "BUG-241"),
        PhaseReconcile::GenuineFailure
    );
}
