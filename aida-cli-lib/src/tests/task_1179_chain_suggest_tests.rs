//! TASK-1179 — the CHAIN slice's suggest-only handoff after a reap.
//!
//! The whole decision — WHETHER to emit a "Next up" block and WHAT it says — is
//! pure over `(reaped_count, next_spec)`, so the emit case and both no-op cases
//! (nothing reaped, empty queue) are exercised here without a store, a queue, a
//! lease directory, or a live process. No test here launches a session — the
//! slice is suggest-only, and there is no execute path to reach.
//
// trace:TASK-1179 | ai:claude

use super::*;

#[test]
fn suggests_next_spec_when_a_reap_landed_and_queue_has_one() {
    // Acceptance 1: after a reap removes a finished session, if there's a next
    // queued spec the output names the exact launch command for that spec.
    let block = next_up_suggestion(1, Some("TASK-2")).expect("a suggestion is emitted");
    assert!(
        block.contains("TASK-2"),
        "block names the next spec: {block}"
    );
    assert!(
        block.contains("aida worktree enter TASK-2"),
        "block names the fresh-worktree launch command: {block}"
    );
    assert!(
        block.contains("aida agent new claude --spec TASK-2"),
        "block names the agent-launch alternative: {block}"
    );
}

#[test]
fn suggest_only_never_implies_a_launch() {
    // Acceptance 2: suggest-only — the block explicitly states no session was
    // started, so nothing in the output can be mistaken for an auto-spawn.
    let block = next_up_suggestion(3, Some("STORY-9")).expect("a suggestion is emitted");
    assert!(
        block.contains("suggestion only"),
        "block makes the suggest-only posture explicit: {block}"
    );
    assert!(
        block.contains("no session was started"),
        "block states no session was started: {block}"
    );
}

#[test]
fn nothing_reaped_emits_no_suggestion() {
    // Acceptance 3a: when nothing was reaped, no suggestion is printed even
    // though the queue has a next spec.
    assert_eq!(next_up_suggestion(0, Some("TASK-2")), None);
}

#[test]
fn empty_queue_emits_no_suggestion() {
    // Acceptance 3b: when a reap landed but the queue is empty, no suggestion.
    assert_eq!(next_up_suggestion(2, None), None);
    // And the degenerate both-empty case is likewise silent.
    assert_eq!(next_up_suggestion(0, None), None);
}

#[test]
fn formatter_is_stable_and_names_the_spec_operand() {
    // The spec id is the operand the operator types, so it must appear verbatim
    // in both launch commands.
    let block = format_next_up_suggestion("BUG-42");
    assert_eq!(block.matches("BUG-42").count(), 3);
}
