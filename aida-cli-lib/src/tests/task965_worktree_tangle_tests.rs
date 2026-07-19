use super::*;
use std::path::Path;

// Root-equality predicate: equal paths tangle, distinct ones don't. Uses
// non-existent paths so canonicalize falls back to the raw form — proving the
// predicate is pure (no real dirs / fs side effects needed). trace:TASK-965
#[test]
fn worktree_tangle_true_when_launch_cwd_equals_primary() {
    let primary = Path::new("/repo/main");
    assert!(is_worktree_tangle(primary, primary));
    assert!(is_worktree_tangle(
        Path::new("/repo/main"),
        Path::new("/repo/main")
    ));
}

#[test]
fn worktree_tangle_false_for_distinct_worktree() {
    assert!(!is_worktree_tangle(
        Path::new("/repo/.worktrees/task-1"),
        Path::new("/repo/main")
    ));
    // A worktree NESTED under the primary is still a distinct directory — not
    // the primary root itself — so it must NOT be flagged as a tangle.
    assert!(!is_worktree_tangle(
        Path::new("/repo/main/.worktrees/task-1"),
        Path::new("/repo/main")
    ));
}

// Stranded-detection predicate. The footgun: primary on a feature branch with
// ≥1 in-flight lease. trace:TASK-965
#[test]
fn stranded_true_on_feature_branch_with_leases() {
    assert!(primary_stranded_on_feature_branch(
        Some("task-965-gate"),
        Some("main"),
        2
    ));
}

#[test]
fn stranded_false_on_clean_default_branch() {
    // Same branch as default → never alarm, even with leases in flight (the
    // no-false-alarm-on-clean-primary case). Case-insensitive match too.
    assert!(!primary_stranded_on_feature_branch(
        Some("main"),
        Some("main"),
        3
    ));
    assert!(!primary_stranded_on_feature_branch(
        Some("MAIN"),
        Some("main"),
        3
    ));
    assert!(!primary_stranded_on_feature_branch(
        Some("master"),
        Some("master"),
        1
    ));
}

#[test]
fn stranded_false_when_no_leases_in_flight() {
    // On a feature branch but nothing in flight → not the footgun; stay silent.
    assert!(!primary_stranded_on_feature_branch(
        Some("feature-x"),
        Some("main"),
        0
    ));
}

#[test]
fn stranded_false_when_branch_or_default_undetectable() {
    // Detached HEAD (no branch) or an undetectable default → conservatively
    // silent, never a false alarm.
    assert!(!primary_stranded_on_feature_branch(None, Some("main"), 2));
    assert!(!primary_stranded_on_feature_branch(
        Some("feature-x"),
        None,
        2
    ));
    assert!(!primary_stranded_on_feature_branch(
        Some(""),
        Some("main"),
        2
    ));
}
