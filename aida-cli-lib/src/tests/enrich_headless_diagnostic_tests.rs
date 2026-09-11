use super::{
    enrich_headless_wait_failure, enrich_no_verdict_with_headless_diagnostic, headless_wait_text,
    last_headless_assistant_text,
};
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

// trace:BUG-1063 | ai:codex
#[test]
fn fake_headless_waiting_transcript_classifies_as_headless_wait() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join(".aida").join("headless-logs");
    fs::create_dir_all(&logs).unwrap();
    let log_path = logs.join("bug-1063-abc.jsonl");
    fs::write(
        &log_path,
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"CI is still running; I armed a monitor and am waiting on that notification."}]}}
{"type":"result","subtype":"success","is_error":false,"result":"Still waiting on that notification."}
"#,
    )
    .unwrap();

    let failure = PhaseFailure::of(FailureKind::NoPr, "the implementer opened no PR");
    let out = enrich_headless_wait_failure(
        failure,
        dir.path(),
        SystemTime::now() - Duration::from_secs(60),
    );
    assert_eq!(out.kind, FailureKind::HeadlessWait);
    assert_eq!(out.kind.cause_slug(), "headless-wait");
    assert!(out.reason.contains("BUG-1063"), "{}", out.reason);
    assert!(
        out.reason
            .contains(log_path.file_name().unwrap().to_str().unwrap()),
        "diagnostic names the offending log: {}",
        out.reason
    );
}

// trace:BUG-1063 | ai:codex
#[test]
fn headless_wait_classifier_uses_last_assistant_text() {
    let log = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Earlier I mentioned waiting on the old run."}]}}
{"type":"assistant","message":{"content":[{"type":"text","text":"Final: verdict written."}]}}
"#;
    let last = last_headless_assistant_text(log).expect("last text");
    assert_eq!(last, "Final: verdict written.");
    assert!(!headless_wait_text(&last));
}
