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
fn foreign_vendor_terminal_shape_is_reported_not_treated_as_truncated() {
    // A genuinely foreign terminal shape (not claude's `result`, not codex's
    // `turn.completed` — which TASK-1334 now measures) is reported by shape,
    // never treated as truncated or fabricated as zero.
    let record =
        "{\"type\":\"session.finished\",\"usage\":{\"input_tokens\":4,\"output_tokens\":2}}\n";
    assert_eq!(
        drain_caps::completed_log_tokens(record),
        drain_caps::CompletedLogTokens::Unrecognized {
            shape: "type=session.finished".to_string()
        }
    );
    let diagnostic = unrecognized_usage_diagnostic("foreign-phase.jsonl", "type=session.finished");
    assert_eq!(
        diagnostic,
        "token usage unrecognized in headless log foreign-phase.jsonl (shape: type=session.finished)"
    );
    assert!(!diagnostic.contains("input_tokens"));

    let root = tempfile::tempdir().unwrap();
    std::fs::write(logs_dir(root.path()).join("foreign-phase.jsonl"), record).unwrap();
    assert_eq!(
        measure_completed_headless_logs(root.path(), SystemTime::UNIX_EPOCH),
        None
    );
}

/// TASK-1334: a codex headless log (`turn.completed` usage events) is a
/// measured completed log — the "token usage unrecognized ... shape:
/// type=turn.completed" degradation that forced "tokens: unknown (collection
/// incomplete)" onto whole codex waves is the regression under test.
// trace:TASK-1334 | ai:claude
#[test]
fn codex_turn_completed_log_is_measured_not_unknown() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        logs_dir(root.path()).join("pr-2443-codex.jsonl"),
        "{\"type\":\"thread.started\",\"thread_id\":\"t1\"}\n\
         {\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":100,\"cached_input_tokens\":40,\"output_tokens\":7}}\n",
    )
    .unwrap();
    assert_eq!(
        measure_completed_headless_logs(root.path(), SystemTime::UNIX_EPOCH),
        Some(107)
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
