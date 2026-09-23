//! BUG-1510: a session's lease and its brief can name different specs, and
//! nothing compared them. `dispatch_lease_brief_conflict` is the dispatch-time
//! guard — it fires only when there is exactly one pending (unacked) brief
//! for the agent and that brief disagrees with the spec about to be leased.
//! trace:BUG-1510 | ai:claude
use super::*;

fn brief(spec_id: &str, acked: bool) -> BriefListEntry {
    BriefListEntry {
        path: std::path::PathBuf::from("agent-briefs").join(format!("{spec_id}.md")),
        spec_id: spec_id.to_string(),
        agent: "claude".to_string(),
        generated_at: "2026-09-20T01:00:00Z".to_string(),
        depends_on: None,
        acked,
        authorized_by: None,
    }
}

// trace:BUG-1510 | ai:claude
#[test]
fn no_pending_briefs_is_not_a_conflict() {
    assert!(dispatch_lease_brief_conflict(&[], "STORY-1391").is_none());
}

// trace:BUG-1510 | ai:claude
#[test]
fn single_pending_brief_matching_spec_is_not_a_conflict() {
    let briefs = vec![brief("STORY-1391", false)];
    assert!(dispatch_lease_brief_conflict(&briefs, "STORY-1391").is_none());
}

// BUG-1510's exact observed incident: lease about to be taken for
// STORY-1391, the one live pending brief for this agent names BUG-1420.
// trace:BUG-1510 | ai:claude
#[test]
fn single_pending_brief_naming_a_different_spec_is_caught_at_dispatch() {
    let briefs = vec![brief("BUG-1420", false)];
    let conflict = dispatch_lease_brief_conflict(&briefs, "STORY-1391")
        .expect("mismatched lease/brief spec ids must be caught at dispatch");
    assert_eq!(conflict.spec_id, "BUG-1420");
}

// An already-acked brief is stale instruction, not the live one driving this
// dispatch — it must not block a legitimate lease on a different spec.
// trace:BUG-1510 | ai:claude
#[test]
fn acked_brief_is_excluded_from_the_check() {
    let briefs = vec![brief("BUG-1420", true)];
    assert!(dispatch_lease_brief_conflict(&briefs, "STORY-1391").is_none());
}

// Two or more live pending briefs means there is no single "the brief"
// driving this dispatch — an agent's ordinary backlog of future work must
// not be mistaken for a mismatch, so the check stays silent.
// trace:BUG-1510 | ai:claude
#[test]
fn multiple_pending_briefs_are_ambiguous_and_not_flagged() {
    let briefs = vec![brief("BUG-1420", false), brief("TASK-1289", false)];
    assert!(dispatch_lease_brief_conflict(&briefs, "STORY-1391").is_none());
}

// trace:BUG-1510 | ai:claude
#[test]
fn spec_id_comparison_is_case_insensitive() {
    let briefs = vec![brief("story-1391", false)];
    assert!(dispatch_lease_brief_conflict(&briefs, "STORY-1391").is_none());
}
