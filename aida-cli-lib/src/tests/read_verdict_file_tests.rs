use super::{read_verdict_file, read_verdict_file_for_head};
use crate::auto_complete::{ReviewerOutcome, Verdict};

/// Write `json` to a temp verdict file and read it back.
fn read(json: &str) -> Result<ReviewerOutcome, crate::auto_complete::PhaseFailure> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("PR-1.json");
    std::fs::write(&path, json).unwrap();
    read_verdict_file(&path)
}

#[test]
fn verdict_file_reads_merge_escalated_to_human() {
    // STORY-306: the reviewer wrote a verdict AND escalated the merge.
    let outcome = read(
            r#"{"verdict":"Approved","merge":"escalated-to-human","summary":"irreversible schema change — a human should merge"}"#,
        )
        .expect("escalation parses");
    match outcome {
        ReviewerOutcome::EscalatedToHuman { reason } => {
            assert!(reason.contains("irreversible"), "{reason}");
        }
        other => panic!("expected EscalatedToHuman, got {other:?}"),
    }
}

#[test]
fn verdict_file_without_merge_field_is_plain_verdict() {
    // Regression: a STORY-263 verdict file with no `merge` field parses
    // to a plain verdict exactly as before.
    assert_eq!(
        read(r#"{"verdict":"Approved","summary":"all good"}"#).unwrap(),
        ReviewerOutcome::Verdict(Verdict::Approved),
    );
    assert_eq!(
        read(r#"{"verdict":"RequestChanges"}"#).unwrap(),
        ReviewerOutcome::Verdict(Verdict::RequestChanges),
    );
}

#[test]
fn escalation_without_summary_falls_back_to_a_generic_reason() {
    let outcome = read(r#"{"verdict":"Approved","merge":"escalated-to-human"}"#).unwrap();
    assert!(matches!(outcome, ReviewerOutcome::EscalatedToHuman { .. }));
}

#[test]
fn missing_verdict_file_is_a_no_verdict_failure() {
    let dir = tempfile::tempdir().unwrap();
    let err = read_verdict_file(&dir.path().join("absent.json")).unwrap_err();
    assert_eq!(err.kind, crate::auto_complete::FailureKind::NoVerdict);
}

// These exercise the parser used by the live drain phase, not the separate
// queue-done verdict gate. Before BUG-1466/BUG-1538 they both returned an
// Approved outcome solely from the verdict spelling.
// trace:BUG-1466 | ai:codex
// trace:BUG-1538 | ai:codex
#[test]
fn live_phase3_refuses_approved_at_a_stale_sha() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("PR-1.json");
    std::fs::write(
        &path,
        r#"{"verdict":"Approved","reviewed_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
    )
    .unwrap();
    let err = read_verdict_file_for_head(&path, Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"))
        .unwrap_err();
    assert!(err.reason.contains("stale"), "{}", err.reason);
}

#[test]
fn live_phase3_refuses_sha_less_approval() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("PR-1.json");
    std::fs::write(&path, r#"{"verdict":"Approved"}"#).unwrap();
    let err = read_verdict_file_for_head(&path, Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"))
        .unwrap_err();
    assert!(err.reason.contains("UNPROVEN"), "{}", err.reason);
}

#[test]
fn live_phase3_accepts_same_commit_with_safe_abbreviation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("PR-1.json");
    std::fs::write(
        &path,
        r#"{"verdict":"Approved","reviewed_sha":"abcdef0123"}"#,
    )
    .unwrap();
    assert_eq!(
        read_verdict_file_for_head(&path, Some("abcdef0123456789abcdef0123456789abcdef01"))
            .unwrap(),
        ReviewerOutcome::Verdict(Verdict::Approved)
    );
}

// BUG-1581: this is the merge-facing parser, not merely the retention helper.
// A top-level approval cannot become merge permission while an independent
// blocking verdict for the same commit survives in the artifact.
// trace:BUG-1581 | ai:codex
#[test]
fn merge_surface_refuses_conflicting_same_sha_verdicts_visibly() {
    let err = read(
        r#"{
          "verdict":"Approved",
          "reviewed_sha":"ac772eaca9d389fa762a232156df996023bfdf7a",
          "recorded_by":"reviewer-a",
          "rounds":[{
            "verdict":"RequestChanges",
            "reviewed_sha":"ac772eaca9",
            "recorded_by":"reviewer-b"
          }]
        }"#,
    )
    .unwrap_err();
    assert_eq!(err.kind, crate::auto_complete::FailureKind::NoVerdict);
    assert!(
        err.reason.contains("conflicting review verdicts"),
        "{}",
        err.reason
    );
    assert!(err.reason.contains("reviewer-a"), "{}", err.reason);
    assert!(err.reason.contains("reviewer-b"), "{}", err.reason);
}
