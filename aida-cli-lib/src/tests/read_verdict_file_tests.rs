use super::read_verdict_file;
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
