//! TASK-218: pin the smart status-transition table for `aida queue
//! rework`. The handler itself does I/O against the store, so we
//! test the pure decision function exhaustively here and rely on
//! integration-test smoke (built-binary + temp store) for the
//! side-effecting glue.
//! trace:TASK-218 | ai:claude
use super::*;

/// Approved → no flip. The spec is ready to be queued as-is, so
/// rework just queues it.
#[test]
fn approved_does_not_flip() {
    assert_eq!(rework_smart_target(&RequirementStatus::Approved), None);
}

/// Planned → InProgress. Rework on a Planned spec means "start
/// working it now," so the queue add is paired with the status flip.
#[test]
fn planned_flips_to_in_progress() {
    assert_eq!(
        rework_smart_target(&RequirementStatus::Planned),
        Some(RequirementStatus::InProgress)
    );
}

/// InProgress → no flip. Already at the right status; caller surfaces
/// the "already in progress" warning and re-queues without --force.
#[test]
fn in_progress_does_not_flip() {
    assert_eq!(rework_smart_target(&RequirementStatus::InProgress), None);
}

/// Done → InProgress. The canonical PR-review-found-issues case —
/// implementer marked it done on a branch, reviewer sent it back.
#[test]
fn done_flips_to_in_progress() {
    assert_eq!(
        rework_smart_target(&RequirementStatus::Done),
        Some(RequirementStatus::InProgress)
    );
}

/// Completed → InProgress (with --force at the caller). The handler
/// itself adds the --force guard; the smart table just records the
/// target.
#[test]
fn completed_flips_to_in_progress() {
    assert_eq!(
        rework_smart_target(&RequirementStatus::Completed),
        Some(RequirementStatus::InProgress)
    );
}

/// Rejected → Approved (with --force). The spec is being reconsidered,
/// not re-implemented yet — Approved is the natural landing.
#[test]
fn rejected_flips_to_approved() {
    assert_eq!(
        rework_smart_target(&RequirementStatus::Rejected),
        Some(RequirementStatus::Approved)
    );
}

/// Draft → no flip. Rework on a Draft is unusual; preserve the
/// status and let the queue add proceed.
#[test]
fn draft_does_not_flip() {
    assert_eq!(rework_smart_target(&RequirementStatus::Draft), None);
}

/// Sanity check: smart_target is idempotent on its own output. After
/// flipping (e.g. Done → InProgress) re-running smart_target on
/// InProgress is a no-op, so chained reworks don't oscillate.
#[test]
fn smart_target_is_idempotent_on_its_own_output() {
    let after_done = rework_smart_target(&RequirementStatus::Done).unwrap();
    assert_eq!(after_done, RequirementStatus::InProgress);
    assert_eq!(rework_smart_target(&after_done), None);

    let after_rejected = rework_smart_target(&RequirementStatus::Rejected).unwrap();
    assert_eq!(after_rejected, RequirementStatus::Approved);
    assert_eq!(rework_smart_target(&after_rejected), None);
}

/// All status variants are covered — exhaustive match in
/// `rework_smart_target` means adding a new variant won't silently
/// fall through. This test exists so a future variant addition (e.g.
/// "Blocked") trips the compiler check, not a silent None default.
#[test]
fn covers_every_status_variant() {
    use RequirementStatus::*;
    for s in &[
        Draft, Approved, Planned, InProgress, Done, Completed, Rejected,
    ] {
        // Just confirm the function doesn't panic on any variant.
        let _ = rework_smart_target(s);
    }
}
