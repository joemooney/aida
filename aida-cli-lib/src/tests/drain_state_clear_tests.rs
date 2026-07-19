use super::should_clear_drain_state;

#[test]
fn failed_resume_keeps_drain_state() {
    // BUG-438: a resume that failed at a phase must NOT consume the
    // checkpoint — it stays re-resumable after the operator fixes the blocker.
    assert!(!should_clear_drain_state(true, true, 3));
}

#[test]
fn successful_resume_clears() {
    assert!(should_clear_drain_state(true, true, 0));
}

#[test]
fn non_resume_drain_unchanged() {
    // Pre-BUG-438 behaviour preserved for a normal drain: clear on both
    // success and failure (the file is only left behind by a true crash,
    // not a clean run that happened to fail a phase).
    assert!(should_clear_drain_state(true, false, 0));
    assert!(should_clear_drain_state(true, false, 4));
}

#[test]
fn non_owner_never_clears() {
    assert!(!should_clear_drain_state(false, true, 0));
    assert!(!should_clear_drain_state(false, false, 0));
}
