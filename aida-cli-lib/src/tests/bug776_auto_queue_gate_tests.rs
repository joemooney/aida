//! BUG-776: the review-story auto-file must not fire when there is nothing
//! reviewable — a repo with no `origin` remote, a forge with no PR concept
//! (synthetic change id 0), or a story that would cover no specs. Before this
//! gate, `aida session end` minted a `Review PR-0: ` story with an empty
//! subject covering no specs on EVERY session end of a remote-less repo.
// trace:BUG-776 | ai:claude

use super::auto_queue_skip_reason;

#[test]
fn no_origin_remote_skips_the_auto_file() {
    let reason = auto_queue_skip_reason(false, None, None)
        .expect("a repo with no origin remote must skip the auto-file");
    assert!(
        reason.contains("no `origin` remote"),
        "skip reason should name the missing remote: {reason}"
    );
    assert!(
        !reason.contains("PR #0"),
        "the remote-less skip must not mention a phantom PR #0: {reason}"
    );
}

#[test]
fn no_origin_remote_wins_even_when_a_pr_number_is_known() {
    // Defense in depth: the remote check short-circuits first, so a stale
    // forge answer can't smuggle a story through.
    assert!(auto_queue_skip_reason(false, Some(42), Some(3)).is_some());
}

#[test]
fn synthetic_pr_zero_skips_the_auto_file() {
    let reason = auto_queue_skip_reason(true, Some(0), None)
        .expect("change id 0 is the pure-git synthetic ref — nothing to review");
    assert!(
        reason.contains("no pull-request concept"),
        "skip reason should explain the synthetic id: {reason}"
    );
}

#[test]
fn zero_covered_specs_skips_the_auto_file() {
    let reason = auto_queue_skip_reason(true, Some(17), Some(0))
        .expect("a story covering no specs is unlinked noise in the reviewer queue");
    assert!(
        reason.contains("PR #17"),
        "skip reason should name the PR: {reason}"
    );
    assert!(
        reason.contains("(REQ-ID)"),
        "skip reason should name the fix — add a trailer: {reason}"
    );
}

#[test]
fn real_remote_real_pr_with_specs_still_files() {
    // The working path must not regress: a remote, a real PR number, and at
    // least one delivered spec ⇒ no skip.
    assert_eq!(auto_queue_skip_reason(true, Some(1577), Some(1)), None);
    assert_eq!(auto_queue_skip_reason(true, Some(9), Some(4)), None);
}

#[test]
fn a_real_pr_is_not_skipped_before_its_coverage_is_known() {
    // The mid-flight call site (PR resolved, commit range not yet scanned)
    // must let a real PR through so the coverage scan can run.
    assert_eq!(auto_queue_skip_reason(true, Some(1577), None), None);
}
