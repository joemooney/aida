// TASK-1444 (containment for BUG-1510 AC4): a reviewer-verdict shelve must
// record against the spec the VERDICT is about, or explicitly mark the
// attribution uncertain — never silently the lease's spec.
//
// Incident: STORY-1391's drain got a RequestChanges verdict whose findings
// were about BUG-1420 (the PR's commits were all trailered BUG-1420, per the
// TASK-1442 guard), and the orchestrator shelved it onto STORY-1391 — the
// lease's spec — with no hint the attribution had never been checked.
// `decide_shelve_attribution` is the pure core `RealPhaseDriver::
// shelve_on_failure` uses to fix that. trace:TASK-1444 | ai:claude

use super::*;

fn commit(sha: &str, subject: &str) -> (String, String) {
    (sha.to_string(), subject.to_string())
}

#[test]
fn matching_trailer_confirms_the_lease() {
    let commits = vec![commit(
        "aaa1111",
        "fix(x): address review findings (STORY-1391)",
    )];
    assert_eq!(
        decide_shelve_attribution(&commits, "STORY-1391"),
        ShelveAttribution::Confirmed("STORY-1391".to_string())
    );
}

/// The BUG-1510 incident shape: a verdict whose subject (via the PR's commit
/// trailers) differs from the lease's spec. The recorded attribution must be
/// the verdict's spec (BUG-1420), never the lease's (STORY-1391).
#[test]
fn mismatched_trailer_reattributes_to_the_verdicts_spec() {
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
    let attribution = decide_shelve_attribution(&commits, "STORY-1391");
    assert_eq!(
        attribution,
        ShelveAttribution::Reattributed("BUG-1420".to_string())
    );
    // Never the lease's spec, silently or otherwise.
    assert_ne!(
        attribution,
        ShelveAttribution::Confirmed("STORY-1391".to_string())
    );
}

#[test]
fn no_trailer_at_all_is_explicitly_uncertain_not_the_lease() {
    let commits = vec![commit("aaa1111", "chore: bump lockfile")];
    let attribution = decide_shelve_attribution(&commits, "STORY-1391");
    match attribution {
        ShelveAttribution::Uncertain(note) => {
            assert!(
                note.contains("no commit"),
                "expected the note to say no trailer was found, got: {note}"
            );
        }
        other => panic!("expected Uncertain, got {other:?}"),
    }
}

#[test]
fn multiple_distinct_specs_named_is_uncertain() {
    let commits = vec![
        commit("aaa1111", "fix(a): part one (BUG-1420)"),
        commit("bbb2222", "fix(b): part two (BUG-1421)"),
    ];
    let attribution = decide_shelve_attribution(&commits, "STORY-1391");
    match attribution {
        ShelveAttribution::Uncertain(note) => {
            assert!(note.contains("BUG-1420") && note.contains("BUG-1421"));
        }
        other => panic!("expected Uncertain, got {other:?}"),
    }
}

#[test]
fn matching_trailer_is_case_insensitive() {
    let commits = vec![commit("aaa1111", "fix(x): whatever (story-1391)")];
    assert_eq!(
        decide_shelve_attribution(&commits, "STORY-1391"),
        ShelveAttribution::Confirmed("STORY-1391".to_string())
    );
}
