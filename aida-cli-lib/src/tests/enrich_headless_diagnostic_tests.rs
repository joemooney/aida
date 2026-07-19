use super::enrich_no_verdict_with_headless_diagnostic;
use crate::auto_complete::{FailureKind, PhaseFailure};
use std::fs;
use std::time::{Duration, SystemTime};

/// A NoVerdict failure with no headless log at all stays unchanged.
#[test]
fn no_log_means_unchanged_failure() {
    let dir = tempfile::tempdir().unwrap();
    let failure = PhaseFailure::of(FailureKind::NoVerdict, "the reviewer …");
    let out = enrich_no_verdict_with_headless_diagnostic(
        failure.clone(),
        dir.path(),
        SystemTime::now() - Duration::from_secs(60),
    );
    assert_eq!(out.reason, failure.reason);
    assert_eq!(out.kind, failure.kind);
}

/// A log without an AskUserQuestion mention leaves the failure unchanged.
#[test]
fn log_without_askuserquestion_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join(".aida").join("headless-logs");
    fs::create_dir_all(&logs).unwrap();
    fs::write(
        logs.join("pr-5-abc.jsonl"),
        r#"{"type":"assistant","content":[{"name":"Bash"}]}
"#,
    )
    .unwrap();
    let failure = PhaseFailure::of(FailureKind::NoVerdict, "the reviewer …");
    let out = enrich_no_verdict_with_headless_diagnostic(
        failure.clone(),
        dir.path(),
        SystemTime::now() - Duration::from_secs(60),
    );
    assert_eq!(out.reason, failure.reason);
}

/// A log written after `started_at` that contains AskUserQuestion
/// triggers the diagnostic enrichment.
#[test]
fn askuserquestion_in_recent_log_enriches_reason() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join(".aida").join("headless-logs");
    fs::create_dir_all(&logs).unwrap();
    let log_path = logs.join("pr-150-abc.jsonl");
    fs::write(
        &log_path,
        r#"{"type":"assistant","content":[{"type":"tool_use","name":"AskUserQuestion"}]}
"#,
    )
    .unwrap();
    let failure = PhaseFailure::of(
        FailureKind::NoVerdict,
        "the reviewer session produced no verdict file — the review did not complete",
    );
    let out = enrich_no_verdict_with_headless_diagnostic(
        failure,
        dir.path(),
        SystemTime::now() - Duration::from_secs(60),
    );
    assert_eq!(out.kind, FailureKind::NoVerdict);
    assert!(out.reason.contains("AskUserQuestion"), "{}", out.reason);
    assert!(out.reason.contains("BUG-280"), "{}", out.reason);
    assert!(
        out.reason
            .contains(log_path.file_name().unwrap().to_str().unwrap()),
        "diagnostic names the offending log: {}",
        out.reason
    );
}

/// A log written BEFORE `started_at` is skipped — the diagnostic is
/// scoped to logs from this reviewer subprocess only.
#[test]
fn old_log_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join(".aida").join("headless-logs");
    fs::create_dir_all(&logs).unwrap();
    let log_path = logs.join("pr-149-old.jsonl");
    fs::write(
        &log_path,
        r#"{"type":"assistant","content":[{"type":"tool_use","name":"AskUserQuestion"}]}
"#,
    )
    .unwrap();
    // started_at is in the future relative to the log's mtime
    let started_at = SystemTime::now() + Duration::from_secs(60);
    let failure = PhaseFailure::of(FailureKind::NoVerdict, "the reviewer …");
    let out = enrich_no_verdict_with_headless_diagnostic(failure.clone(), dir.path(), started_at);
    assert_eq!(out.reason, failure.reason);
}

/// Non-NoVerdict failures pass through unchanged — the diagnostic is
/// only relevant when the verdict file is genuinely missing.
#[test]
fn non_noverdict_failure_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join(".aida").join("headless-logs");
    fs::create_dir_all(&logs).unwrap();
    fs::write(
        logs.join("pr-1.jsonl"),
        r#"{"name":"AskUserQuestion"}
"#,
    )
    .unwrap();
    let failure = PhaseFailure::of(FailureKind::CiRed, "ci red");
    let out = enrich_no_verdict_with_headless_diagnostic(
        failure.clone(),
        dir.path(),
        SystemTime::now() - Duration::from_secs(60),
    );
    assert_eq!(out.kind, FailureKind::CiRed);
    assert_eq!(out.reason, failure.reason);
}
