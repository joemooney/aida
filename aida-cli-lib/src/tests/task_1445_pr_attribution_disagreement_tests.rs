// TASK-1445 (containment for BUG-1510 AC5): `aida drain status` and the
// unshipped-work detector must agree on a PR's owning spec, or the
// disagreement must be surfaced. Incident: STORY-1391's drain opened PR
// #2043 whose commits were all trailered BUG-1420 — drain status (lease)
// said STORY-1391, the PR's own commits said BUG-1420, and the split sat
// unreported for 52 seconds before a RequestChanges verdict landed on the
// wrong spec. `pr_attribution_disagreement` is the pure core the drain
// surfaces this from — reusing TASK-1444's `decide_shelve_attribution` so
// the two guards never disagree about what "trailer evidence" means.
// trace:TASK-1445 | ai:claude

use super::*;

fn commit(sha: &str, subject: &str) -> (String, String) {
    (sha.to_string(), subject.to_string())
}

#[test]
fn matching_trailer_is_not_a_disagreement() {
    let commits = vec![commit("aaa1111", "fix(x): address findings (STORY-1391)")];
    assert_eq!(
        pr_attribution_disagreement(2043, &commits, "STORY-1391"),
        None
    );
}

/// The BUG-1510 incident shape: the lease says STORY-1391, the PR's own
/// commits confidently say BUG-1420 instead. This must surface — both
/// claimed owners, named by the evidence each used.
#[test]
fn mismatched_trailer_is_a_disagreement_naming_both_owners() {
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
    let disagreement = pr_attribution_disagreement(2043, &commits, "STORY-1391")
        .expect("mismatched trailer must surface a disagreement");
    assert_eq!(disagreement.pr, 2043);
    assert_eq!(disagreement.lease_spec, "STORY-1391");
    assert_eq!(disagreement.trailer_spec, "BUG-1420");
}

#[test]
fn no_trailer_at_all_is_not_a_confident_disagreement() {
    let commits = vec![commit("aaa1111", "chore: bump lockfile")];
    assert_eq!(
        pr_attribution_disagreement(2043, &commits, "STORY-1391"),
        None
    );
}

#[test]
fn multiple_distinct_specs_named_is_not_a_confident_disagreement() {
    let commits = vec![
        commit("aaa1111", "fix(a): part one (BUG-1420)"),
        commit("bbb2222", "fix(b): part two (BUG-1421)"),
    ];
    assert_eq!(
        pr_attribution_disagreement(2043, &commits, "STORY-1391"),
        None
    );
}

#[test]
fn matching_trailer_is_case_insensitive() {
    let commits = vec![commit("aaa1111", "fix(x): whatever (story-1391)")];
    assert_eq!(
        pr_attribution_disagreement(2043, &commits, "STORY-1391"),
        None
    );
}
