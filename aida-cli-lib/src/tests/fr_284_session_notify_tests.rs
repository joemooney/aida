//! FR-284 NOTIFY — the "finished-but-still-live session is safe to exit" predicate.
//!
//! The NOTIFY decision is pure over the same pre-gathered `ReapFacts` the reap
//! predicate reads, so the whole matrix is exercised here without a repo, a
//! store, a lease directory, or a real process. In particular NO test in this
//! file writes a mailbox message or a sentinel — the delivery leg
//! (`notify_finished_live_sessions`) is not reachable from these pure inputs.
//!
//! The load-bearing invariant: NOTIFY fires for exactly the session that would
//! be REAPED were its process not still alive — the interactive near-miss —
//! and never for one that fails any of the reap safety gates.
//
// trace:FR-284 | ai:claude

use super::*;

/// A finished + merged + clean session whose process is STILL ALIVE — the
/// interactive session idling at its prompt after its spec merged. This is the
/// one case NOTIFY exists to speak to.
fn finished_but_live_facts() -> ReapFacts {
    ReapFacts {
        spec_finished: true,
        // The single bit that distinguishes NOTIFY from REAP.
        process_exited: false,
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
fn finished_merged_but_live_session_is_notified() {
    assert!(session_should_notify(&finished_but_live_facts()));
}

#[test]
fn squash_merged_live_session_is_notified_on_the_forge_signal() {
    // Branch tip differs from origin/main (squash merge) → the merged PR is the
    // positive signal, same as the reap path.
    let mut facts = finished_but_live_facts();
    facts.worktree.ancestor_of_main = false;
    facts.worktree.pr_merged = true;
    assert!(session_should_notify(&facts));
}

#[test]
fn an_exited_session_is_reaped_not_notified() {
    // The reap path owns the exited case — NOTIFY must NOT also fire for it, or
    // a message would be sent for a session that is about to be torn down.
    let mut facts = finished_but_live_facts();
    facts.process_exited = true;
    assert!(!session_should_notify(&facts));
    // And it IS reapable.
    assert!(matches!(
        classify_session_reap(&facts),
        ReapVerdict::Reap(_)
    ));
}

#[test]
fn unfinished_spec_is_not_notified() {
    // Nothing merged → nothing to say "safe to exit" about.
    let mut facts = finished_but_live_facts();
    facts.spec_finished = false;
    assert!(!session_should_notify(&facts));
}

#[test]
fn unmerged_live_session_is_not_notified() {
    // Finished spec but the branch is not merged — telling the session it is
    // safe to exit would be wrong; its work has not landed.
    let mut facts = finished_but_live_facts();
    facts.worktree.ancestor_of_main = false;
    facts.worktree.pr_merged = false;
    facts.worktree.unique_unmerged_commits = 3;
    assert!(!session_should_notify(&facts));
}

#[test]
fn dirty_live_session_is_not_notified() {
    // Uncommitted work present — the session is NOT done; do not nudge it out.
    let mut facts = finished_but_live_facts();
    facts.worktree.dirty = true;
    assert!(!session_should_notify(&facts));
}

#[test]
fn locked_live_session_is_not_notified() {
    // Operator-protected worktrees are left entirely alone — no reap, no nudge.
    let mut facts = finished_but_live_facts();
    facts.locked = true;
    assert!(!session_should_notify(&facts));
}

#[test]
fn squash_merged_live_session_with_extra_unique_commits_is_not_notified() {
    // Work added after the squash-merge would be lost on exit — the shared
    // worktree-GC gate keeps it, and NOTIFY inherits that verdict rather than
    // telling the session it is safe to go.
    let mut facts = finished_but_live_facts();
    facts.worktree.ancestor_of_main = false;
    facts.worktree.pr_merged = true;
    facts.worktree.unique_unmerged_commits = 2;
    assert!(!session_should_notify(&facts));
}

#[test]
fn notify_and_reap_are_mutually_exclusive_over_liveness() {
    // The two predicates partition the finished+merged+clean case on exactly one
    // bit — process liveness — so a session is never both reaped and notified.
    let mut facts = finished_but_live_facts();
    // Alive → notify, not reap.
    assert!(session_should_notify(&facts));
    assert!(matches!(
        classify_session_reap(&facts),
        ReapVerdict::Skip(_)
    ));
    // Exited → reap, not notify.
    facts.process_exited = true;
    assert!(!session_should_notify(&facts));
    assert!(matches!(
        classify_session_reap(&facts),
        ReapVerdict::Reap(_)
    ));
}

#[test]
fn session_notice_path_is_confined_to_the_notices_dir() {
    // A crafted lease id can never escape `.aida/session-notices/`.
    let root = std::path::Path::new("/tmp/proj");
    let p = session_notice_path(root, "../../etc/passwd");
    assert!(p.starts_with("/tmp/proj/.aida/session-notices"));
    assert!(!p.to_string_lossy().contains(".."));
    // An ordinary hex lease id is used verbatim.
    let ok = session_notice_path(root, "019f8c80abcd");
    assert!(ok.ends_with("019f8c80abcd"));
}
