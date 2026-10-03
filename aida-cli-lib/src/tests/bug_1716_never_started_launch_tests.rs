//! BUG-1716: a never-started vendor launch (an argv/usage rejection that
//! exits before the session log is even created) must classify as a LAUNCH
//! failure, spend no STORY-975 transient retry, and leave the spec
//! re-queueable — never as implementer work that failed.
//!
//! These are the pure/unit legs; the end-to-end stub-vendor wiring lives in
//! `real_phase_driver_wiring_tests.rs`.
// trace:BUG-1716 | ai:claude

use crate::{empty_launch_decision, headless_launch_log_evidence, EmptyLaunchLog};

/// The log evidence classifier: no file at all is `Missing` (the argv-
/// rejection signature — the vendor never got far enough to create it), an
/// empty file is BUG-826's `ZeroBytes`, and any content means the vendor
/// demonstrably ran.
#[test]
fn log_evidence_classifies_missing_zero_byte_and_content() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    let session = "019e9a07-8e2d-7800-ad4e-735340341716";

    // No .aida/headless-logs directory at all → Missing.
    assert_eq!(
        headless_launch_log_evidence(root, session),
        Some(EmptyLaunchLog::Missing)
    );

    // Directory exists, but no log for THIS session → still Missing.
    let dir = root.join(".aida").join("headless-logs");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("task-x-other-session.jsonl"), "noise\n").unwrap();
    assert_eq!(
        headless_launch_log_evidence(root, session),
        Some(EmptyLaunchLog::Missing)
    );

    // A zero-byte log for the session → ZeroBytes.
    let logp = dir.join(format!("bug-1716-{session}.jsonl"));
    std::fs::write(&logp, "").unwrap();
    assert_eq!(
        headless_launch_log_evidence(root, session),
        Some(EmptyLaunchLog::ZeroBytes)
    );

    // Content → the vendor ran; no empty-launch evidence.
    std::fs::write(&logp, "{\"type\":\"system\"}\n").unwrap();
    assert_eq!(headless_launch_log_evidence(root, session), None);
}

/// The empty-launch decision, mutated in both directions: a missing log is an
/// empty launch ONLY when the session's lease branch carries no commits
/// (misrouted logging while a real implementer worked must fall through to
/// the BUG-1140 substrate verification), while a zero-byte log keeps BUG-826's
/// face-value classification regardless.
#[test]
fn missing_log_is_an_empty_launch_only_without_commits() {
    assert!(empty_launch_decision(Some(EmptyLaunchLog::Missing), false));
    assert!(
        !empty_launch_decision(Some(EmptyLaunchLog::Missing), true),
        "commits on the lease branch prove an agent worked — not an empty launch"
    );
    assert!(empty_launch_decision(
        Some(EmptyLaunchLog::ZeroBytes),
        false
    ));
    assert!(
        empty_launch_decision(Some(EmptyLaunchLog::ZeroBytes), true),
        "a zero-byte log keeps BUG-826's face-value classification"
    );
    assert!(!empty_launch_decision(None, false));
    assert!(!empty_launch_decision(None, true));
}

/// AC: a launch failure must not consume the STORY-975 transient retry budget.
/// The TASK-204 incident burned attempt 3/3 precisely because the never-
/// started exit was classified `Failed` ("tool-exit", which IS transient-
/// retryable); `LaunchNoOutput` must stay outside that cause set.
#[test]
fn launch_no_output_spends_no_transient_retry() {
    use crate::auto_complete::{is_transient_retry_cause, FailureKind};
    assert!(
        !is_transient_retry_cause(FailureKind::LaunchNoOutput.cause_slug()),
        "a launch failure must not burn the whole-phase transient budget"
    );
    assert!(
        !is_transient_retry_cause(FailureKind::LaunchRefused.cause_slug()),
        "a pre-launch refusal must not burn the whole-phase transient budget"
    );
    // The control (the regression shape): a genuine launched-then-failed
    // exit IS transient-retryable, so the discrimination is real.
    assert!(is_transient_retry_cause(FailureKind::Failed.cause_slug()));
}
