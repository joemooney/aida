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
        review_comment: None,
        findings: Vec::new(),
        surviving_findings: Vec::new(),
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
                      "reviewed_branch":"task-5","recorded_at":"2026-07-21T10:00:00Z"}"#;
    let v = parse_recorded_verdict(stamped).expect("stamped file parses");
    assert_eq!(v.reviewed_sha.as_deref(), Some("e49317ecafe"));
    assert_eq!(v.reviewed_branch.as_deref(), Some("task-5"));
}

#[test]
fn parses_review_comment_metadata_for_rework() {
    let body = r#"{
        "verdict":"RequestChanges",
        "summary":"BUG-814 has two issues",
        "comment_url":"https://github.com/o/r/pull/1637#issuecomment-1",
        "findings":["BUG-814 prompt omits review findings", "silent no-change pass-through"]
    }"#;
    let v = parse_recorded_verdict(body).expect("verdict parses");
    assert_eq!(
        v.comment_url.as_deref(),
        Some("https://github.com/o/r/pull/1637#issuecomment-1")
    );
    assert_eq!(v.findings.len(), 2);

    let rendered = rework_findings_comment("BUG-814", "PR #1637", &v).expect("blocking verdict");
    assert!(rendered.contains("REVIEW FINDINGS TO ADDRESS (PR #1637)"));
    assert!(rendered.contains("1. BUG-814 prompt omits review findings"));
    assert!(rendered.contains("2. silent no-change pass-through"));
    assert!(rendered.contains("Review comment: https://github.com/o/r/pull/1637#issuecomment-1"));
    assert!(rendered.contains(
        "Contract: produce at least one commit, or punt explicitly naming the finding you dispute; never pass through silently with no changes."
    ));
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
        &[
            "BUG-775 gate has no finding details".to_string(),
            "Rework prompt falls back to summary only".to_string(),
        ],
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
    assert_eq!(
        v.findings,
        vec![
            "BUG-775 gate has no finding details".to_string(),
            "Rework prompt falls back to summary only".to_string(),
        ]
    );
    assert!(v.recorded_at.is_some(), "a timestamp is always stamped");
}

#[test]
fn read_any_tries_each_id_form() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    record_verdict(
        root,
        "BUG-775",
        Some("approved"),
        None,
        None,
        None,
        &[],
        "test",
    )
    .unwrap();
    // Agreed-id form misses, spec-id form hits.
    let v = read_recorded_verdict_any(root, &["BUG-9999", "BUG-775"]).expect("found via 2nd id");
    assert_eq!(v.kind, VerdictKind::Approved);
    assert!(read_recorded_verdict_any(root, &["???", ""]).is_none());
}

#[test]
fn record_without_findings_preserves_existing_findings() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida").join("review-verdicts")).unwrap();
    std::fs::write(
        verdict_path(root, "TASK-5"),
        r#"{"verdict":"RequestChanges","findings":["keep this finding"]}"#,
    )
    .unwrap();

    record_verdict(
        root,
        "TASK-5",
        Some("request-changes"),
        None,
        None,
        Some("fresh summary"),
        &[],
        "test",
    )
    .unwrap();

    let v = read_recorded_verdict(root, "TASK-5").expect("record reads back");
    assert_eq!(v.findings, vec!["keep this finding".to_string()]);
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

#[test]
fn gate_refuses_an_unverifiable_approval() {
    let approved = rc("approved", None);
    match queue_done_verdict_gate("TASK-5", Some(&approved), TipRelation::Unknown) {
        VerdictGate::Refuse(lines) => {
            let joined = lines.join("\n");
            assert!(joined.contains("UNVERIFIABLE"), "{joined}");
            assert!(joined.contains("reviewed_sha"), "{joined}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
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
fn notice_flags_a_verdict_without_a_reviewed_sha() {
    let line = verdict_notice_line(&rc("approved", None));
    assert!(line.contains("UNVERIFIABLE"), "{line}");
    assert!(line.contains("missing reviewed_sha"), "{line}");
}

// STORY-1391: a finding that survives a round is evidence about the BRIEF, and
// nothing could detect it because record_verdict overwrote the prior round in
// place. These cover retention, the survivor comparison, and the two ways the
// comparison could lie.
// trace:STORY-1391 | ai:claude
#[test]
fn recording_a_second_round_archives_the_first_instead_of_destroying_it() {
    let tmp = tempfile::tempdir().unwrap();
    record_verdict(
        tmp.path(),
        "PR-1",
        Some("RequestChanges"),
        Some("aaa1111"),
        Some("b"),
        Some("round one"),
        &["share the marker constant".to_string()],
        "reviewer",
    )
    .unwrap();
    record_verdict(
        tmp.path(),
        "PR-1",
        Some("RequestChanges"),
        Some("bbb2222"),
        Some("b"),
        Some("round two"),
        &["share the marker constant".to_string()],
        "reviewer",
    )
    .unwrap();

    let body = std::fs::read_to_string(verdict_path(tmp.path(), "PR-1")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();

    // current round stays at the top level — existing readers untouched
    assert_eq!(v["reviewed_sha"], "bbb2222");
    // the first round survives rather than being overwritten
    let rounds = v["rounds"].as_array().expect("rounds array");
    assert_eq!(rounds.len(), 1, "exactly the round that was replaced");
    assert_eq!(rounds[0]["reviewed_sha"], "aaa1111");
    assert_eq!(rounds[0]["summary"], "round one");
}

// trace:STORY-1391 | ai:claude
#[test]
fn a_finding_repeated_across_rounds_is_reported_as_surviving() {
    let tmp = tempfile::tempdir().unwrap();
    for (sha, findings) in [
        ("aaa1111", vec!["share the marker constant", "add a test"]),
        ("bbb2222", vec!["share the marker constant"]),
    ] {
        record_verdict(
            tmp.path(),
            "PR-2",
            Some("RequestChanges"),
            Some(sha),
            Some("b"),
            None,
            &findings.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            "reviewer",
        )
        .unwrap();
    }
    let body = std::fs::read_to_string(verdict_path(tmp.path(), "PR-2")).unwrap();
    let survivors = findings_surviving_round(&body);
    assert_eq!(survivors, vec!["share the marker constant".to_string()]);
}

// A NEW finding must not be reported as surviving — a false survivor sends
// someone to rewrite a brief that was fine.
// trace:STORY-1391 | ai:claude
#[test]
fn a_finding_raised_for_the_first_time_is_not_reported_as_surviving() {
    let tmp = tempfile::tempdir().unwrap();
    for (sha, findings) in [
        ("aaa1111", vec!["share the marker constant"]),
        ("bbb2222", vec!["something else entirely"]),
    ] {
        record_verdict(
            tmp.path(),
            "PR-3",
            Some("RequestChanges"),
            Some(sha),
            Some("b"),
            None,
            &findings.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            "reviewer",
        )
        .unwrap();
    }
    let body = std::fs::read_to_string(verdict_path(tmp.path(), "PR-3")).unwrap();
    assert!(findings_surviving_round(&body).is_empty());
}

// Re-recording the SAME round — a reviewer correcting its own summary before
// the head moves — must not manufacture a round, or every finding would read
// as surviving on the next real round.
// trace:STORY-1391 | ai:claude
#[test]
fn re_recording_the_same_round_does_not_manufacture_a_survivor() {
    let tmp = tempfile::tempdir().unwrap();
    let f = vec!["share the marker constant".to_string()];
    record_verdict(
        tmp.path(),
        "PR-4",
        Some("RequestChanges"),
        Some("aaa1111"),
        Some("b"),
        Some("first wording"),
        &f,
        "reviewer",
    )
    .unwrap();
    let first = std::fs::read_to_string(verdict_path(tmp.path(), "PR-4")).unwrap();
    let recorded_at = serde_json::from_str::<serde_json::Value>(&first).unwrap()["recorded_at"]
        .as_str()
        .unwrap()
        .to_string();

    // same sha, same recorded_at => same round
    let path = verdict_path(tmp.path(), "PR-4");
    let mut obj: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&first).unwrap();
    obj.insert("summary".into(), "reworded".into());
    obj.insert("recorded_at".into(), recorded_at.clone().into());
    std::fs::write(&path, serde_json::to_string_pretty(&obj).unwrap()).unwrap();

    record_verdict(
        tmp.path(),
        "PR-4",
        Some("RequestChanges"),
        Some("aaa1111"),
        Some("b"),
        Some("reworded again"),
        &f,
        "reviewer",
    )
    .unwrap();

    let body = std::fs::read_to_string(&path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let rounds = v["rounds"].as_array().map(|a| a.len()).unwrap_or(0);
    assert!(
        rounds <= 1,
        "re-recording one round must not append a second: got {rounds}"
    );
    assert!(
        findings_surviving_round(&body).is_empty(),
        "one round cannot produce a survivor"
    );
}

// A finding raised in round 1, FIXED in round 2, and reappearing in round 3 is
// a REGRESSION, not a surviving finding — and the two want opposite responses.
// A survivor means "the brief may be unimplementable, supply a mechanism"; a
// regression means "this was done and came undone". Comparing against the
// previous round distinguishes them; comparing against all rounds cannot.
// trace:STORY-1391 | ai:claude
#[test]
fn a_finding_fixed_then_regressed_is_not_reported_as_surviving() {
    let tmp = tempfile::tempdir().unwrap();
    for (sha, findings) in [
        ("aaa1111", vec!["share the marker constant"]),
        ("bbb2222", vec!["unrelated second-round point"]),
        ("ccc3333", vec!["share the marker constant"]),
    ] {
        record_verdict(
            tmp.path(),
            "PR-5",
            Some("RequestChanges"),
            Some(sha),
            Some("b"),
            None,
            &findings.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            "reviewer",
        )
        .unwrap();
    }
    let body = std::fs::read_to_string(verdict_path(tmp.path(), "PR-5")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        v["rounds"].as_array().map(|a| a.len()),
        Some(2),
        "two prior rounds retained"
    );
    assert!(
        findings_surviving_round(&body).is_empty(),
        "round 3 repeats round 1, not round 2 — that is a regression, not a survivor"
    );
}

// STORY-1391 criteria 2 and 3: the surviving-finding signal must REACH the
// rework prompt, and its wording is load-bearing. The spec exists because
// escalating firmness on an unimplementable requirement failed three times, so
// the text must say what to DO — establish implementability — and must not read
// as an implementer-performance complaint.
// trace:STORY-1391 | ai:claude
#[test]
fn the_rework_prompt_carries_surviving_findings_and_says_what_to_do() {
    let tmp = tempfile::tempdir().unwrap();
    for (sha, findings) in [
        ("aaa1111", vec!["share the marker constant", "add a test"]),
        ("bbb2222", vec!["share the marker constant"]),
    ] {
        record_verdict(
            tmp.path(),
            "PR-6",
            Some("RequestChanges"),
            Some(sha),
            Some("b"),
            None,
            &findings.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            "reviewer",
        )
        .unwrap();
    }
    let body = std::fs::read_to_string(verdict_path(tmp.path(), "PR-6")).unwrap();
    let verdict = parse_recorded_verdict(&body).expect("verdict parses");

    // criterion 2: parse carries the signal, so every existing caller gets it
    assert_eq!(
        verdict.surviving_findings,
        vec!["share the marker constant".to_string()]
    );

    let prompt = rework_findings_comment("STORY-9", "PR #6", &verdict).expect("blocking verdict");
    assert!(
        prompt.contains("Survived the previous round"),
        "the signal must reach the implementer, not just the struct"
    );
    assert!(
        prompt.contains("share the marker constant"),
        "the surviving finding is named"
    );

    // criterion 3: says what to DO, and does not blame
    assert!(
        prompt.contains("AS WRITTEN"),
        "must direct at implementability, not at effort"
    );
    for blaming in ["again", "still", "failed to", "you did not"] {
        assert!(
            !prompt.to_lowercase().contains(blaming),
            "performance framing `{blaming}` produces the escalating firmness \
             this spec exists to prevent"
        );
    }
}

// A first round must produce no signal at all — a false survivor sends someone
// to rewrite a brief that was fine.
// trace:STORY-1391 | ai:claude
#[test]
fn a_first_round_rework_prompt_carries_no_survival_signal() {
    let tmp = tempfile::tempdir().unwrap();
    record_verdict(
        tmp.path(),
        "PR-7",
        Some("RequestChanges"),
        Some("aaa1111"),
        Some("b"),
        None,
        &["share the marker constant".to_string()],
        "reviewer",
    )
    .unwrap();
    let body = std::fs::read_to_string(verdict_path(tmp.path(), "PR-7")).unwrap();
    let verdict = parse_recorded_verdict(&body).expect("verdict parses");
    assert!(verdict.surviving_findings.is_empty());
    let prompt = rework_findings_comment("STORY-9", "PR #7", &verdict).expect("blocking verdict");
    assert!(!prompt.contains("Survived the previous round"));
}
