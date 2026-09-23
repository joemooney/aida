// TASK-1442 (containment for BUG-1510): before a PR is opened for a spec,
// the commit trailers on the branch must be checked against that spec id — a
// branch carrying no commit trailered for the leased spec must not get a PR
// opened under it. This is Guard 2, distinct from STORY-469's Guard 1
// (`validate_trailer_references`), which only checks that a trailer resolves
// to a LIVE spec, never that it matches the spec the branch/PR is FOR.
//
// Incident: STORY-1391's drain opened PR #2043 whose commits were all
// trailered BUG-1420 — every trailer was live, so Guard 1 passed, but the PR
// was misattributed to the wrong spec. `pr_open_spec_guard_violation` is the
// pure core `run_pr_open_spec_guard` uses to refuse that case.
// trace:TASK-1442 | ai:claude

use super::*;

fn commit(sha: &str, subject: &str) -> (String, String) {
    (sha.to_string(), subject.to_string())
}

#[test]
fn matching_trailer_is_not_a_violation() {
    let commits = vec![
        commit("aaa1111", "feat(pr): guard PR-open attribution (TASK-1442)"),
        commit("bbb2222", "fix(x): unrelated cleanup (BUG-1)"),
    ];
    assert_eq!(pr_open_spec_guard_violation(&commits, "TASK-1442"), None);
}

#[test]
fn matching_trailer_is_case_insensitive() {
    let commits = vec![commit("aaa1111", "fix(x): whatever (task-1442)")];
    assert_eq!(pr_open_spec_guard_violation(&commits, "TASK-1442"), None);
}

/// The BUG-1510 incident shape: a branch leased for one spec (STORY-1391)
/// whose commits are all trailered for a different, live spec (BUG-1420).
/// The refusal must name both ids so a human can diagnose it — this test
/// asserts the violation payload carries the offending id.
#[test]
fn mismatched_trailer_is_a_violation_naming_both_ids() {
    let commits = vec![
        commit(
            "aaa1111",
            "fix(orchestrator): shelve on RequestChanges (BUG-1420)",
        ),
        commit(
            "bbb2222",
            "fix(orchestrator): retry punt routing (BUG-1420)",
        ),
    ];
    let violation = pr_open_spec_guard_violation(&commits, "STORY-1391");
    assert_eq!(violation, Some(vec!["BUG-1420".to_string()]));
}

#[test]
fn commit_with_no_trailer_at_all_is_a_violation_with_empty_other_ids() {
    let commits = vec![commit("aaa1111", "chore: bump lockfile")];
    let violation = pr_open_spec_guard_violation(&commits, "STORY-1391");
    assert_eq!(violation, Some(vec![]));
}

#[test]
fn multiple_offending_ids_are_deduped_and_all_reported() {
    let commits = vec![
        commit("aaa1111", "fix(a): first (BUG-1420)"),
        commit("bbb2222", "fix(b): second (BUG-1420)"),
        commit("ccc3333", "fix(c): third (TASK-99)"),
    ];
    let violation = pr_open_spec_guard_violation(&commits, "STORY-1391").unwrap();
    assert_eq!(
        violation,
        vec!["BUG-1420".to_string(), "TASK-99".to_string()]
    );
}

#[test]
fn plan_commits_are_exempt_like_guard_1() {
    // A `docs(plans): ...` commit's trailer names what is PLANNED, not what
    // shipped, so it must not count as attribution evidence either way.
    let commits = vec![commit(
        "aaa1111",
        "docs(plans): save implementation plan (BUG-1420)",
    )];
    let violation = pr_open_spec_guard_violation(&commits, "STORY-1391").unwrap();
    assert_eq!(violation, Vec::<String>::new());
}
