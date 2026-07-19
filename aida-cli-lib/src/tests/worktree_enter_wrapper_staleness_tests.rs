//! BUG-654: `aida worktree enter` only auto-cd's when the installed shell
//! wrapper advertises the `worktree` capability via `AIDA_SHELL_WRAPPER`.
//! A pre-STORY-716 wrapper's marker lacks it (or is unset for "no wrapper"),
//! and we must warn so the silent no-op cd is explained.
use super::wrapper_marker_has_worktree_cap;

#[test]
fn current_wrapper_marker_advertises_worktree() {
    // The shape exported by SHELL_HELPERS today (STORY-716+).
    assert!(wrapper_marker_has_worktree_cap(Some(
        "role,session,dev,worktree"
    )));
    // Order / extra whitespace don't matter.
    assert!(wrapper_marker_has_worktree_cap(Some(
        "worktree, role , session"
    )));
}

#[test]
fn stale_or_missing_wrapper_does_not_advertise_worktree() {
    // Pre-STORY-716 wrapper: marker present but no `worktree` capability.
    assert!(!wrapper_marker_has_worktree_cap(Some("role,session,dev")));
    // No wrapper installed at all → marker unset.
    assert!(!wrapper_marker_has_worktree_cap(None));
    // Empty marker (set but capability-less) also can't auto-eval enter.
    assert!(!wrapper_marker_has_worktree_cap(Some("")));
    // A substring of another token must not count as the capability.
    assert!(!wrapper_marker_has_worktree_cap(Some("worktrees,role")));
}
