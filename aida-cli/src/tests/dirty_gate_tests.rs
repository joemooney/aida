use super::{dirty_gate_outcome, DirtyGateOutcome};

#[test]
fn clean_tree_proceeds() {
    assert_eq!(
        dirty_gate_outcome(false, false, false),
        DirtyGateOutcome::Proceed
    );
    assert_eq!(
        dirty_gate_outcome(false, false, true),
        DirtyGateOutcome::Proceed
    );
}

#[test]
fn force_discards_regardless() {
    assert_eq!(
        dirty_gate_outcome(true, true, false),
        DirtyGateOutcome::Proceed
    );
    assert_eq!(
        dirty_gate_outcome(true, true, true),
        DirtyGateOutcome::Proceed
    );
}

#[test]
fn dirty_pool_return_salvages_not_refuses() {
    // BUG-652: the regression — a dirty pooled tree being returned must
    // salvage and continue, never refuse (which would break reuse).
    assert_eq!(
        dirty_gate_outcome(true, false, true),
        DirtyGateOutcome::Salvage
    );
}

#[test]
fn dirty_non_pool_or_non_return_still_refuses() {
    assert_eq!(
        dirty_gate_outcome(true, false, false),
        DirtyGateOutcome::Refuse
    );
}
