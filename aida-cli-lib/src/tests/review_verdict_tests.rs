//! Unit tests for the review-verdict record + the `queue done` gate policy.
//!
//! The whole point of BUG-775 is that a reviewer's "changes requested" must be
//! ENFORCEABLE, so the policy is pure and tested here without git, a reviewer,
//! or a queue. trace:BUG-775 | ai:claude

use super::*;
use tempfile::TempDir;

fn rc(kind_raw: &str, sha: Option<&str>) -> RecordedVerdict {
    RecordedVerdict {
        kind: VerdictKind::parse(kind_raw),
        raw: kind_raw.to_string(),
        reviewed_sha: sha.map(str::to_string),
        reviewed_branch: Some("task-5".to_string()),
        recorded_at: Some("2026-07-21T10:00:00Z".to_string()),
        summary: Some("three blocking defects".to_string()),
        comment_url: None,
        findings: Vec::new(),
    }
}

#[test]
fn verdict_words_normalize_to_kinds() {
    for w in ["approved", "APPROVE", "lgtm", "pass"] {
        assert_eq!(VerdictKind::parse(w), VerdictKind::Approved, "{w}");
    }
    for w in [
        "RequestChanges",
        "request_changes",
        "request-changes",
        "changes",
        "partial",
    ] {
        assert_eq!(VerdictKind::parse(w), VerdictKind::RequestChanges, "{w}");
    }
    for w in ["rejected", "reject", "fail"] {
        assert_eq!(VerdictKind::parse(w), VerdictKind::Rejected, "{w}");
    }
    assert_eq!(VerdictKind::parse("mostly fine"), VerdictKind::Other);
}

#[test]
fn only_request_changes_and_rejected_block_done() {
    assert!(VerdictKind::RequestChanges.blocks_done());
    assert!(VerdictKind::Rejected.blocks_done());
    assert!(!VerdictKind::Approved.blocks_done());
    assert!(!VerdictKind::Other.blocks_done());
}

// The `/aida-review` skill's file shape must keep parsing, plus the new fields.
#[test]
fn parses_verdict_file_with_and_without_the_new_fields() {
    let legacy = r#"{"verdict":"RequestChanges","summary":"three defects"}"#;
    let v = parse_recorded_verdict(legacy).expect("legacy file parses");
    assert_eq!(v.kind, VerdictKind::RequestChanges);
    assert_eq!(v.reviewed_sha, None);

    let stamped = r#"{"verdict":"RequestChanges","summary":"x","reviewed_sha":"e49317ecafe",
                      "reviewed_branch":"task-5","recorded_at":"2026-07-21T10:00:00Z",
                      "comment_url":"https://example.test/pull/42#issuecomment-1",
                      "blocking_findings":["fix the retry loop","surface the error"]}"#;
    let v = parse_recorded_verdict(stamped).expect("stamped file parses");
    assert_eq!(v.reviewed_sha.as_deref(), Some("e49317ecafe"));
    assert_eq!(v.reviewed_branch.as_deref(), Some("task-5"));
    assert_eq!(
        v.comment_url.as_deref(),
        Some("https://example.test/pull/42#issuecomment-1")
    );
    assert_eq!(v.findings, vec!["fix the retry loop", "surface the error"]);
}

#[test]
fn a_file_without_a_verdict_field_is_not_a_verdict() {
    assert!(parse_recorded_verdict(r#"{"summary":"no verdict here"}"#).is_none());
    assert!(parse_recorded_verdict("not json").is_none());
}

#[test]
fn record_round_trips_and_preserves_unknown_fields() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida").join("review-verdicts")).unwrap();
    // A field only the reviewer skill knows about.
    std::fs::write(
        verdict_path(root, "TASK-5"),
        r#"{"verdict":"RequestChanges","comment_url":"https://example/x"}"#,
    )
    .unwrap();

    record_verdict(
        root,
        "task-5",
        Some("request-changes"),
        Some("deadbeefdeadbeefdeadbeef"),
        Some("task-5"),
        Some("three blocking defects"),
        "test",
    )
    .unwrap();

    let body = std::fs::read_to_string(verdict_path(root, "TASK-5")).unwrap();
    assert!(
        body.contains("comment_url"),
        "the reviewer's own fields must survive an update: {body}"
    );
    // Lower-case spec id resolves to the same record.
    let v = read_recorded_verdict(root, "task-5").expect("record reads back");
    assert_eq!(v.kind, VerdictKind::RequestChanges);
    assert_eq!(v.reviewed_sha.as_deref(), Some("deadbeefdeadbeefdeadbeef"));
    assert!(v.recorded_at.is_some(), "a timestamp is always stamped");
}

#[test]
fn read_any_tries_each_id_form() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    record_verdict(root, "BUG-775", Some("approved"), None, None, None, "test").unwrap();
    // Agreed-id form misses, spec-id form hits.
    let v = read_recorded_verdict_any(root, &["BUG-9999", "BUG-775"]).expect("found via 2nd id");
    assert_eq!(v.kind, VerdictKind::Approved);
    assert!(read_recorded_verdict_any(root, &["???", ""]).is_none());
}

// ---- tip relation ------------------------------------------------------

#[test]
fn tip_relation_classifies_the_four_cases() {
    assert_eq!(
        classify_tip_relation(Some("aaa"), Some("aaa"), Some(true)),
        TipRelation::AtReviewedSha
    );
    assert_eq!(
        classify_tip_relation(Some("aaa"), Some("bbb"), Some(true)),
        TipRelation::AdvancedPast
    );
    assert_eq!(
        classify_tip_relation(Some("aaa"), Some("bbb"), Some(false)),
        TipRelation::Rewritten
    );
    assert_eq!(
        classify_tip_relation(Some("aaa"), Some("bbb"), None),
        TipRelation::Unknown
    );
    // No sha recorded at all → unknowable.
    assert_eq!(
        classify_tip_relation(None, Some("bbb"), Some(true)),
        TipRelation::Unknown
    );
}

// ---- the gate ----------------------------------------------------------

#[test]
fn gate_proceeds_with_no_verdict_or_a_passing_one() {
    assert_eq!(
        queue_done_verdict_gate("TASK-5", None, TipRelation::Unknown),
        VerdictGate::Proceed
    );
    let approved = rc("approved", Some("aaa"));
    assert_eq!(
        queue_done_verdict_gate("TASK-5", Some(&approved), TipRelation::AtReviewedSha),
        VerdictGate::Proceed
    );
}

/// THE bug: the branch tip is still the exact commit the reviewer rejected.
#[test]
fn gate_refuses_when_tip_is_still_the_reviewed_commit() {
    let v = rc("RequestChanges", Some("e49317ecafe0"));
    match queue_done_verdict_gate("TASK-5", Some(&v), TipRelation::AtReviewedSha) {
        VerdictGate::Refuse(lines) => {
            let joined = lines.join("\n");
            assert!(joined.contains("refused"), "{joined}");
            assert!(joined.contains("CHANGES REQUESTED"), "{joined}");
            assert!(joined.contains("e49317ecafe0"), "names the sha: {joined}");
            assert!(joined.contains("--force"), "names the override: {joined}");
            assert!(
                joined.contains("three blocking defects"),
                "carries the reviewer's rationale: {joined}"
            );
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// A rejected verdict blocks exactly like request-changes.
#[test]
fn gate_refuses_on_a_rejected_verdict_too() {
    let v = rc("rejected", Some("e49317ecafe0"));
    assert!(matches!(
        queue_done_verdict_gate("TASK-5", Some(&v), TipRelation::AtReviewedSha),
        VerdictGate::Refuse(_)
    ));
}

/// Once real commits land on top of the reviewed one, the gate opens.
#[test]
fn gate_allows_once_the_tip_advances_past_the_reviewed_commit() {
    let v = rc("RequestChanges", Some("e49317ecafe0"));
    assert_eq!(
        queue_done_verdict_gate("TASK-5", Some(&v), TipRelation::AdvancedPast),
        VerdictGate::Proceed
    );
}

/// Unknowable ⇒ refuse. A gate that cannot answer must not wave work through
/// — the silent skip is the defect this fixes.
#[test]
fn gate_refuses_when_advancement_cannot_be_established() {
    let no_sha = rc("RequestChanges", None);
    match queue_done_verdict_gate("TASK-5", Some(&no_sha), TipRelation::Unknown) {
        VerdictGate::Refuse(lines) => {
            let joined = lines.join("\n");
            assert!(joined.contains("could not establish"), "{joined}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// Rebased/amended history: allowed, but never silently.
#[test]
fn gate_warns_but_proceeds_when_the_branch_was_rewritten() {
    let v = rc("RequestChanges", Some("e49317ecafe0"));
    match queue_done_verdict_gate("TASK-5", Some(&v), TipRelation::Rewritten) {
        VerdictGate::Warn(lines) => {
            let joined = lines.join("\n");
            assert!(joined.contains("warning:"), "{joined}");
            assert!(joined.contains("e49317ecafe0"), "{joined}");
        }
        other => panic!("expected a warning, got {other:?}"),
    }
}

#[test]
fn notice_line_is_one_readable_line() {
    let v = rc("RequestChanges", Some("e49317ecafe0deadbeef"));
    let line = verdict_notice_line(&v);
    assert!(line.starts_with("CHANGES REQUESTED"), "{line}");
    assert!(line.contains("e49317ecafe0"), "{line}");
    assert!(!line.contains('\n'), "{line}");
}

#[test]
fn rework_context_prefers_structured_findings_and_falls_back_to_summary() {
    let structured = RecordedVerdict {
        findings: vec!["first blocker".to_string(), "second blocker".to_string()],
        comment_url: Some("https://example.test/pull/7#issuecomment-1".to_string()),
        ..rc("RequestChanges", Some("abc"))
    };
    let ctx = rework_findings_context(&structured, Some(7)).expect("context");
    assert_eq!(ctx.verdict, "CHANGES REQUESTED");
    assert_eq!(ctx.pr, Some(7));
    assert_eq!(ctx.findings, vec!["first blocker", "second blocker"]);
    assert_eq!(
        ctx.comment_url.as_deref(),
        Some("https://example.test/pull/7#issuecomment-1")
    );

    let fallback = rc("RequestChanges", Some("abc"));
    let ctx = rework_findings_context(&fallback, None).expect("summary fallback");
    assert_eq!(ctx.findings, vec!["three blocking defects"]);

    let approved = rc("Approved", Some("abc"));
    assert!(rework_findings_context(&approved, None).is_none());
}
