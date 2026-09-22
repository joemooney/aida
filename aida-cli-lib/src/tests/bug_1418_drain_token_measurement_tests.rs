use super::*;
use std::time::SystemTime;

fn logs_dir(root: &std::path::Path) -> std::path::PathBuf {
    let dir = root.join(".aida/headless-logs");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// trace:BUG-1418 | ai:codex
#[test]
fn no_headless_logs_is_unknown_not_zero() {
    let root = tempfile::tempdir().unwrap();
    logs_dir(root.path());
    assert_eq!(
        measure_completed_headless_logs(root.path(), SystemTime::UNIX_EPOCH),
        None
    );
}

// trace:BUG-1418 | ai:codex
#[test]
fn unrecognized_usage_schema_is_unknown_not_zero() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        logs_dir(root.path()).join("foreign-vendor.jsonl"),
        "{\"type\":\"usage\",\"tokens\":42}\n",
    )
    .unwrap();
    assert_eq!(
        measure_completed_headless_logs(root.path(), SystemTime::UNIX_EPOCH),
        None
    );
}

// trace:BUG-1418 | ai:codex
#[test]
fn logs_outside_resolved_root_are_unknown_not_zero() {
    let resolved = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    logs_dir(resolved.path());
    std::fs::write(
        logs_dir(elsewhere.path()).join("phase.jsonl"),
        "{\"type\":\"result\",\"usage\":{\"input_tokens\":9}}\n",
    )
    .unwrap();
    assert_eq!(
        measure_completed_headless_logs(resolved.path(), SystemTime::UNIX_EPOCH),
        None
    );
}

// trace:BUG-1418 | ai:codex
#[test]
fn truncated_log_is_unknown_not_zero() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        logs_dir(root.path()).join("truncated.jsonl"),
        "{\"type\":\"assistant\",\"message\":{\"usage\":{\"input_tokens\":12}}}\n{",
    )
    .unwrap();
    assert_eq!(
        measure_completed_headless_logs(root.path(), SystemTime::UNIX_EPOCH),
        None
    );
}

// trace:BUG-1418 | ai:codex
#[test]
fn genuine_measured_zero_remains_numeric_zero() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        logs_dir(root.path()).join("zero.jsonl"),
        "{\"type\":\"result\",\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}\n",
    )
    .unwrap();
    assert_eq!(
        measure_completed_headless_logs(root.path(), SystemTime::UNIX_EPOCH),
        Some(0)
    );
}
