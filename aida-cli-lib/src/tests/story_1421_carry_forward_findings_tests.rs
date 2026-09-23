use super::*;
use crate::review_verdict::{RecordedVerdict, VerdictKind};
use std::collections::HashSet;

fn approved_verdict(findings: &[&str]) -> RecordedVerdict {
    RecordedVerdict {
        kind: VerdictKind::Approved,
        raw: "approved".to_string(),
        findings: findings.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

/// An APPROVED verdict carrying 2 non-blocking findings, with nothing filed
/// yet, needs a successor for both of them.
// trace:STORY-1421 | ai:claude
#[test]
fn approved_verdict_with_two_findings_needs_two_successors() {
    let verdict = approved_verdict(&["F2: doc comment misplaced", "F3: same defect, second site"]);
    let to_file = findings_needing_a_successor(&verdict, &HashSet::new());
    assert_eq!(to_file.len(), 2);
    let texts: Vec<&str> = to_file.iter().map(|(t, _)| t.as_str()).collect();
    assert!(texts.contains(&"F2: doc comment misplaced"));
    assert!(texts.contains(&"F3: same defect, second site"));
    // Each gets a distinct hash tag — the idempotency key.
    assert_ne!(to_file[0].1, to_file[1].1);
}

/// Re-running the planner with the hashes the first pass already produced
/// (simulating a re-observed auto-bump scan on an already-completed spec)
/// yields nothing — no duplicates.
// trace:STORY-1421 | ai:claude
#[test]
fn rerun_with_already_filed_hashes_yields_no_duplicates() {
    let verdict = approved_verdict(&["F2: doc comment misplaced", "F3: same defect, second site"]);
    let first_pass = findings_needing_a_successor(&verdict, &HashSet::new());
    assert_eq!(first_pass.len(), 2);

    let already_filed: HashSet<String> = first_pass.iter().map(|(_, hash)| hash.clone()).collect();
    let second_pass = findings_needing_a_successor(&verdict, &already_filed);
    assert!(
        second_pass.is_empty(),
        "a re-run against the same verdict must not refile already-filed findings"
    );
}

/// An APPROVED verdict with no findings at all has nothing to carry forward.
// trace:STORY-1421 | ai:claude
#[test]
fn approved_verdict_with_no_findings_yields_none() {
    let verdict = approved_verdict(&[]);
    let to_file = findings_needing_a_successor(&verdict, &HashSet::new());
    assert!(to_file.is_empty());
}

/// A blocking verdict's findings are rework, not a carry-forward case — even
/// with findings present, nothing is emitted.
// trace:STORY-1421 | ai:claude
#[test]
fn request_changes_verdict_never_emits_findings() {
    let verdict = RecordedVerdict {
        kind: VerdictKind::RequestChanges,
        raw: "request-changes".to_string(),
        findings: vec!["blocking defect".to_string()],
        ..Default::default()
    };
    let to_file = findings_needing_a_successor(&verdict, &HashSet::new());
    assert!(to_file.is_empty());
}

/// A duplicate line within a single verdict's findings list is only carried
/// forward once — the within-call de-dupe.
// trace:STORY-1421 | ai:claude
#[test]
fn duplicate_finding_text_in_one_verdict_files_once() {
    let verdict = approved_verdict(&["same text", "same text"]);
    let to_file = findings_needing_a_successor(&verdict, &HashSet::new());
    assert_eq!(to_file.len(), 1);
}
