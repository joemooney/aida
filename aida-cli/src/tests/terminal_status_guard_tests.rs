use super::*;
use aida_core::RequirementStatus::*;

/// Mirrors the TASK-47 guard: refuse Closed→Open without --force,
/// while allowing Open→Closed, idempotent flips, and any path under
/// `force=true`. Pure helper so the matrix is exhaustively testable
// without touching the storage backend. trace:TASK-47 | ai:claude
fn would_block_status_change(
    old: &aida_core::RequirementStatus,
    new: &aida_core::RequirementStatus,
    force: bool,
) -> bool {
    if force {
        return false;
    }
    is_terminal_status(old) && !is_terminal_status(new)
}

/// Closed → Open without --force → blocked.
#[test]
fn completed_to_in_progress_blocks_without_force() {
    assert!(would_block_status_change(&Completed, &InProgress, false));
    assert!(would_block_status_change(&Rejected, &InProgress, false));
    assert!(would_block_status_change(&Completed, &Approved, false));
    assert!(would_block_status_change(&Rejected, &Draft, false));
}

/// Idempotent terminal flips and cross terminal flips both stay
/// closed, so no guard needed.
#[test]
fn terminal_to_terminal_passes() {
    assert!(!would_block_status_change(&Completed, &Completed, false));
    assert!(!would_block_status_change(&Rejected, &Rejected, false));
    assert!(!would_block_status_change(&Completed, &Rejected, false));
    assert!(!would_block_status_change(&Rejected, &Completed, false));
}

/// Closing a non-terminal req always allowed.
#[test]
fn open_to_terminal_passes() {
    let opens = [Draft, Approved, Planned, InProgress];
    let terminals = [Completed, Rejected];
    for old in &opens {
        for new in &terminals {
            assert!(
                !would_block_status_change(old, new, false),
                "{:?} → {:?} should be allowed",
                old,
                new
            );
        }
    }
}

/// Open-to-open transitions are unaffected by the guard.
#[test]
fn open_to_open_passes() {
    let opens = [Draft, Approved, Planned, InProgress];
    for old in &opens {
        for new in &opens {
            assert!(
                !would_block_status_change(old, new, false),
                "{:?} → {:?} should be allowed",
                old,
                new
            );
        }
    }
}

/// --force unconditionally bypasses, including Closed→Open which is
/// the whole point of the flag.
#[test]
fn force_bypasses_every_transition() {
    let all = [
        Draft, Approved, Planned, InProgress, Done, Completed, Rejected,
    ];
    for old in &all {
        for new in &all {
            assert!(
                !would_block_status_change(old, new, true),
                "--force should never block {:?} → {:?}",
                old,
                new
            );
        }
    }
}

/// STORY-86: `Done` is on the OPEN side of the guard, so transitions
/// from Done to anywhere (including back to InProgress) require no
/// --force, and transitions INTO Done from anywhere are likewise
/// unguarded. The auto-bump path (Done → Completed) lands cleanly
/// without the user ever passing --force.
// trace:STORY-86 | ai:claude
#[test]
fn done_transitions_are_unguarded() {
    let from_done = [Draft, Approved, Planned, InProgress, Completed, Rejected];
    for new in &from_done {
        assert!(
            !would_block_status_change(&Done, new, false),
            "Done → {:?} should be allowed without --force",
            new
        );
    }
    let to_done = [Draft, Approved, Planned, InProgress];
    for old in &to_done {
        assert!(
            !would_block_status_change(old, &Done, false),
            "{:?} → Done should be allowed without --force",
            old
        );
    }
    // The one transition that should still be guarded: Completed → Done
    // (re-opening a shipped spec) and Rejected → Done. Done is non-
    // terminal, so the existing Closed→Open guard catches these.
    assert!(
        would_block_status_change(&Completed, &Done, false),
        "Completed → Done is Closed→Open, guard should block without --force"
    );
    assert!(
        would_block_status_change(&Rejected, &Done, false),
        "Rejected → Done is Closed→Open, guard should block without --force"
    );
}
