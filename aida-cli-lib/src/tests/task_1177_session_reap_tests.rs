//! TASK-1177 — the session-reap predicate.
//!
//! Everything the reap pass decides is pure over pre-gathered facts, so the
//! whole safety matrix is exercised here without a git repo, a store, a lease
//! directory, or a real process. In particular NO test in this file removes a
//! worktree or touches a live session's tree — the execution leg is not
//! reachable from these inputs.
//
// trace:TASK-1177 | ai:claude

use super::*;

/// The happy case: a headless session that finished — spec Done/Completed,
/// branch merged into the default branch, process exited, clean worktree.
fn reapable_facts() -> ReapFacts {
    ReapFacts {
        spec_finished: true,
        process_exited: true,
        locked: false,
        worktree: AgentWorktreeFacts {
            dirty: false,
            ancestor_of_main: true,
            pr_merged: false,
            unique_unmerged_commits: 0,
        },
    }
}

#[test]
fn finished_merged_exited_session_is_reaped() {
    assert!(matches!(
        classify_session_reap(&reapable_facts()),
        ReapVerdict::Reap(_)
    ));
}

#[test]
fn squash_merged_session_is_reaped_on_the_forge_signal() {
    // The branch tip differs from origin/main (squash merge), so the ancestry
    // probe is inconclusive; the merged PR is the positive signal.
    let mut facts = reapable_facts();
    facts.worktree.ancestor_of_main = false;
    facts.worktree.pr_merged = true;
    assert!(matches!(
        classify_session_reap(&facts),
        ReapVerdict::Reap(_)
    ));
}

#[test]
fn live_process_is_never_reaped_even_when_finished_and_merged() {
    // The load-bearing boundary: AIDA may reap dead sessions, never close live
    // ones. A finished, merged, clean session whose process is STILL RUNNING is
    // left completely untouched.
    let mut facts = reapable_facts();
    facts.process_exited = false;
    match classify_session_reap(&facts) {
        ReapVerdict::Skip(reason) => assert!(
            reason.contains("still running"),
            "expected the liveness objection, got: {reason}"
        ),
        v => panic!("a live session must never be reaped, got {v:?}"),
    }
}

#[test]
fn unfinished_spec_is_not_reaped() {
    let mut facts = reapable_facts();
    facts.spec_finished = false;
    match classify_session_reap(&facts) {
        ReapVerdict::Skip(reason) => assert!(reason.contains("not finished"), "{reason}"),
        v => panic!("expected a skip, got {v:?}"),
    }
}

#[test]
fn dirty_worktree_outranks_every_other_signal() {
    let mut facts = reapable_facts();
    facts.worktree.dirty = true;
    match classify_session_reap(&facts) {
        ReapVerdict::Skip(reason) => assert!(reason.contains("uncommitted"), "{reason}"),
        v => panic!("uncommitted work must never be reaped, got {v:?}"),
    }
}

#[test]
fn locked_worktree_is_operator_protected() {
    let mut facts = reapable_facts();
    facts.locked = true;
    match classify_session_reap(&facts) {
        ReapVerdict::Skip(reason) => assert!(reason.contains("locked"), "{reason}"),
        v => panic!("expected a skip, got {v:?}"),
    }
}

#[test]
fn unmerged_branch_is_not_reaped() {
    let mut facts = reapable_facts();
    facts.worktree.ancestor_of_main = false;
    facts.worktree.pr_merged = false;
    facts.worktree.unique_unmerged_commits = 3;
    match classify_session_reap(&facts) {
        ReapVerdict::Skip(reason) => assert!(reason.contains("no merge signal"), "{reason}"),
        v => panic!("unmerged work must never be reaped, got {v:?}"),
    }
}

#[test]
fn squash_merged_branch_with_extra_unique_commits_is_kept() {
    // Work added AFTER the PR squash-merged would be lost — the shared
    // worktree-GC gate keeps it, and the reap pass inherits that verdict.
    let mut facts = reapable_facts();
    facts.worktree.ancestor_of_main = false;
    facts.worktree.pr_merged = true;
    facts.worktree.unique_unmerged_commits = 2;
    match classify_session_reap(&facts) {
        ReapVerdict::Skip(reason) => assert!(reason.contains("unique unmerged"), "{reason}"),
        v => panic!("expected a skip, got {v:?}"),
    }
}

#[test]
fn dirty_beats_liveness_in_the_reported_reason() {
    // Ordering check: the strongest objection is the one reported.
    let mut facts = reapable_facts();
    facts.worktree.dirty = true;
    facts.process_exited = false;
    facts.locked = true;
    match classify_session_reap(&facts) {
        ReapVerdict::Skip(reason) => assert!(reason.contains("uncommitted"), "{reason}"),
        v => panic!("expected a skip, got {v:?}"),
    }
}

// ---------------------------------------------------------------------------
// The process-exited derivation — three substrate signals, all state reads.
// ---------------------------------------------------------------------------

#[test]
fn exited_requires_proof_of_death() {
    // Every recorded pid gone + lease reads stale + nothing live in the tree.
    assert!(session_process_exited(LeaseState::Stale, Some(true), false));
    assert!(session_process_exited(
        LeaseState::Dormant,
        Some(true),
        false
    ));
}

#[test]
fn unknown_liveness_is_never_treated_as_exited() {
    // A lease that recorded no pid at all cannot PROVE death — absence of
    // evidence is not evidence of death, so the pass refuses.
    assert!(!session_process_exited(LeaseState::Stale, None, false));
}

#[test]
fn a_living_pid_or_a_live_lease_blocks_the_exit_verdict() {
    assert!(!session_process_exited(
        LeaseState::Stale,
        Some(false),
        false
    ));
    assert!(!session_process_exited(LeaseState::Live, Some(true), false));
}

// ---------------------------------------------------------------------------
// The protected-ref floor — the pass must never be able to delete these.
// ---------------------------------------------------------------------------

#[test]
fn protected_branches_are_never_deletable() {
    for b in ["main", "Main", "master", "aida-store", "HEAD"] {
        assert!(branch_is_protected(b, Some("origin/main")), "{b}");
    }
    // A mirror-tracking ref for the store branch is protected too.
    assert!(branch_is_protected(
        "gitlab/aida-store",
        Some("origin/main")
    ));
    // The configured default branch, whatever it is called.
    assert!(branch_is_protected("trunk", Some("origin/trunk")));
}

#[test]
fn ordinary_session_branches_are_not_protected() {
    assert!(!branch_is_protected(
        "task-1177-session-reap",
        Some("origin/main")
    ));
    assert!(!branch_is_protected(
        "worktree-agent-abc123",
        Some("origin/main")
    ));
    // An empty branch is "nothing to delete", not "protected".
    assert!(!branch_is_protected("", Some("origin/main")));
}

#[test]
fn a_live_process_inside_the_worktree_blocks_the_exit_verdict() {
    // Covers an agent that holds no lease of its own but is sitting in the
    // worktree — removing its cwd would strand it.
    assert!(!session_process_exited(LeaseState::Stale, Some(true), true));
}
