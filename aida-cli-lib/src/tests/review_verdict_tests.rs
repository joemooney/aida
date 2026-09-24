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
        recorded_by: None,
        closed_by_merge: None,
        closed_at: None,
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
    assert_eq!(VerdictKind::parse("mostly fine"), VerdictKind::Unknown);
}

#[test]
fn only_request_changes_and_rejected_block_done() {
    assert!(VerdictKind::RequestChanges.blocks_done());
    assert!(VerdictKind::Rejected.blocks_done());
    assert!(!VerdictKind::Approved.blocks_done());
    assert!(!VerdictKind::Unknown.blocks_done());
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

// An unrecognised verdict word must refuse, not silently proceed as though
// it were an approval — the same fail-open shape as a Skipped preflight
// guard funnelling into Open.
// trace:BUG-1507 | ai:claude (PRIN-5: absent is not good evidence)
#[test]
fn gate_refuses_an_unrecognised_verdict_word() {
    let mystery = rc("mostly fine", Some("e49317ecafe0"));
    match queue_done_verdict_gate("TASK-5", Some(&mystery), TipRelation::AtReviewedSha) {
        VerdictGate::Refuse(lines) => {
            let joined = lines.join("\n");
            assert!(joined.contains("unrecognised verdict"), "{joined}");
            assert!(
                joined.contains("mostly fine"),
                "names the raw word: {joined}"
            );
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
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
    // Exact, not `<= 1`: the permissive form admits 1, which is precisely the
    // value the defect produces, so the assertion whose message names the
    // invariant would not have enforced it.
    assert_eq!(
        rounds, 0,
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

// STORY-1391: 42 of the 501 verdict files on disk record the reviewed commit as
// `head` rather than `reviewed_sha`. Round identity must read both, or it covers
// 8% of the corpus and every legacy round is treated as unidentifiable.
//
// The fixture goes STRAIGHT from the legacy file to a new head. An earlier
// version re-recorded the same head first, which wrote `reviewed_sha` and meant
// the fallback was never exercised — the test passed with the fallback removed.
// trace:STORY-1391 | ai:claude
#[test]
fn the_older_head_key_identifies_a_round_too() {
    let tmp = tempfile::tempdir().unwrap();
    let path = verdict_path(tmp.path(), "PR-8");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // legacy shape: `head`, no `reviewed_sha`
    std::fs::write(
        &path,
        r#"{"verdict":"RequestChanges","head":"aaa1111","findings":["share the marker constant"]}"#,
    )
    .unwrap();

    // a genuinely NEW head, recorded directly against the legacy file
    record_verdict(
        tmp.path(),
        "PR-8",
        Some("RequestChanges"),
        Some("bbb2222"),
        Some("b"),
        None,
        &["share the marker constant".to_string()],
        "reviewer",
    )
    .unwrap();

    let body = std::fs::read_to_string(&path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        v["rounds"].as_array().map(|a| a.len()),
        Some(1),
        "the head-keyed legacy round is identifiable and must be retained"
    );
    assert_eq!(
        findings_surviving_round(&body),
        vec!["share the marker constant".to_string()],
        "so the repeated finding is reported as surviving"
    );
}

// 419 of 501 files carry NEITHER key, and BUG-1466's handshake writer still
// produces them. An unidentifiable round must not be archived: doing so makes a
// re-record its own predecessor and every finding reads as surviving. A missed
// survivor costs one wasted round; a false one corrupts the signal.
// trace:STORY-1391 trace:BUG-1466 | ai:claude
#[test]
fn an_unidentifiable_round_is_not_archived_and_produces_no_false_survivor() {
    let tmp = tempfile::tempdir().unwrap();
    let path = verdict_path(tmp.path(), "PR-9");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"verdict":"RequestChanges","findings":["share the marker constant"]}"#,
    )
    .unwrap();

    record_verdict(
        tmp.path(),
        "PR-9",
        Some("RequestChanges"),
        None,
        Some("b"),
        None,
        &["share the marker constant".to_string()],
        "reviewer",
    )
    .unwrap();

    let body = std::fs::read_to_string(&path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(
        v.get("rounds").is_none(),
        "an unidentifiable round must not be archived"
    );
    assert!(
        findings_surviving_round(&body).is_empty(),
        "and must not manufacture a survivor"
    );
}

// THE MOTIVATING INCIDENT (PR #2040): the drain recorded a verdict at one head
// and a second reviewer recorded its own at the SAME head minutes later,
// destroying four findings. Same-head is the shape of a collision by
// construction, so a retention rule keyed on "is the head different?" cannot
// see it. `recorded_by` can.
// trace:STORY-1391 | ai:claude
#[test]
fn a_second_reviewer_at_the_same_head_does_not_destroy_the_first_verdict() {
    let tmp = tempfile::tempdir().unwrap();
    record_verdict(
        tmp.path(),
        "PR-9",
        Some("RequestChanges"),
        Some("a3049c50bf"),
        Some("b"),
        Some("drain verdict"),
        &[
            "unique to the drain".to_string(),
            "shared point".to_string(),
        ],
        "drain",
    )
    .unwrap();
    record_verdict(
        tmp.path(),
        "PR-9",
        Some("RequestChanges"),
        Some("a3049c50bf"),
        Some("b"),
        Some("human verdict"),
        &["shared point".to_string()],
        "claude-reviewer-1",
    )
    .unwrap();

    let body = std::fs::read_to_string(verdict_path(tmp.path(), "PR-9")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let rounds = v["rounds"].as_array().cloned().unwrap_or_default();
    assert_eq!(rounds.len(), 1, "the overwritten verdict must be retained");
    assert_eq!(rounds[0]["recorded_by"], "drain");
    let kept: Vec<&str> = rounds[0]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap())
        .collect();
    assert!(
        kept.contains(&"unique to the drain"),
        "the finding unique to the overwritten verdict is the one that was lost: {kept:?}"
    );

    // A collision is NOT a round. `shared point` appears in both verdicts, but
    // no rework happened between them, so reporting it as a survivor would send
    // someone to rewrite a brief that was fine.
    assert!(
        findings_surviving_round(&body).is_empty(),
        "two reviewers at one head must not manufacture survivors"
    );
}

// ── BUG-1516 criteria 2 + 3: normalize + backfill abbreviated shas ─────────

fn git(repo: &std::path::Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("git runs")
}

fn git_ok(repo: &std::path::Path, args: &[&str]) {
    let out = git(repo, args);
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// THE bug (BUG-1516 criterion 2): a caller-supplied abbreviated sha used to
/// pass straight through to disk unchanged. `record_verdict` must now resolve
/// it to the full 40-character sha at the write boundary, when this repo can.
// trace:BUG-1516 | ai:claude
#[test]
fn record_verdict_expands_a_resolvable_abbreviated_sha_to_full_length() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    git_ok(repo, &["init", "--initial-branch=main", "--quiet"]);
    git_ok(repo, &["commit", "--allow-empty", "-m", "root", "--quiet"]);
    let full = String::from_utf8_lossy(&git(repo, &["rev-parse", "HEAD"]).stdout)
        .trim()
        .to_string();
    let short = &full[..10];
    assert_ne!(
        short.len(),
        40,
        "the test input must actually be abbreviated"
    );

    record_verdict(
        repo,
        "TASK-20",
        Some("approved"),
        Some(short),
        Some("main"),
        None,
        &[],
        "test",
    )
    .unwrap();

    let v = read_recorded_verdict(repo, "TASK-20").unwrap();
    assert_eq!(
        v.reviewed_sha.as_deref(),
        Some(full.as_str()),
        "a resolvable abbreviated sha must be expanded to the full 40 characters at write time"
    );
}

/// A sha this repo cannot resolve (never seen the commit) must be kept
/// verbatim rather than dropped or replaced with something invented —
/// "the write must keep the original string," per the spec's own caveat.
// trace:BUG-1516 | ai:claude
#[test]
fn record_verdict_keeps_an_unresolvable_sha_verbatim() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    git_ok(repo, &["init", "--initial-branch=main", "--quiet"]);
    git_ok(repo, &["commit", "--allow-empty", "-m", "root", "--quiet"]);

    record_verdict(
        repo,
        "TASK-21",
        Some("approved"),
        Some("deadbee0"),
        Some("main"),
        None,
        &[],
        "test",
    )
    .unwrap();

    let v = read_recorded_verdict(repo, "TASK-21").unwrap();
    assert_eq!(
        v.reviewed_sha.as_deref(),
        Some("deadbee0"),
        "an unresolvable sha must be preserved, not dropped or invented"
    );
}

/// BUG-1516 criterion 3: the one-time repair sweep expands every resolvable
/// abbreviated `reviewed_sha` on disk and marks the rest unresolvable —
/// distinctly, never silently, and never by inventing a value.
// trace:BUG-1516 | ai:claude
#[test]
fn backfill_resolves_reachable_shas_and_marks_the_rest_unresolvable() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    git_ok(repo, &["init", "--initial-branch=main", "--quiet"]);
    git_ok(repo, &["commit", "--allow-empty", "-m", "root", "--quiet"]);
    let full = String::from_utf8_lossy(&git(repo, &["rev-parse", "HEAD"]).stdout)
        .trim()
        .to_string();
    let short = full[..9].to_string();

    let vd = repo.join(".aida").join("review-verdicts");
    std::fs::create_dir_all(&vd).unwrap();
    std::fs::write(
        vd.join("PR-1.json"),
        format!(r#"{{"verdict":"approved","reviewed_sha":"{short}"}}"#),
    )
    .unwrap();
    std::fs::write(
        vd.join("PR-2.json"),
        r#"{"verdict":"approved","reviewed_sha":"0000000dead"}"#,
    )
    .unwrap();
    std::fs::write(
        vd.join("PR-3.json"),
        format!(r#"{{"verdict":"approved","reviewed_sha":"{full}"}}"#),
    )
    .unwrap();
    std::fs::write(
        vd.join("PR-4.json"),
        r#"{"verdict":"request-changes","summary":"no sha here"}"#,
    )
    .unwrap();

    let report = backfill_abbreviated_shas(repo, false).unwrap();
    assert_eq!(report.resolved, vec!["PR-1.json".to_string()]);
    assert_eq!(report.unresolvable, vec!["PR-2.json".to_string()]);
    assert_eq!(report.already_full, 1);
    assert_eq!(report.skipped_no_sha, 1);

    let resolved = read_recorded_verdict(repo, "PR-1").unwrap();
    assert_eq!(resolved.reviewed_sha.as_deref(), Some(full.as_str()));

    let unresolvable_body = std::fs::read_to_string(vd.join("PR-2.json")).unwrap();
    let unresolvable: serde_json::Value = serde_json::from_str(&unresolvable_body).unwrap();
    assert_eq!(unresolvable["reviewed_sha"], "0000000dead");
    assert_eq!(unresolvable["reviewed_sha_unresolvable"], true);
}

/// `--dry-run` must compute the same report without writing anything — the
/// operator can preview a sweep over the real corpus before committing to it.
// trace:BUG-1516 | ai:claude
#[test]
fn backfill_dry_run_reports_without_writing() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    git_ok(repo, &["init", "--initial-branch=main", "--quiet"]);
    git_ok(repo, &["commit", "--allow-empty", "-m", "root", "--quiet"]);
    let full = String::from_utf8_lossy(&git(repo, &["rev-parse", "HEAD"]).stdout)
        .trim()
        .to_string();
    let short = full[..9].to_string();

    let vd = repo.join(".aida").join("review-verdicts");
    std::fs::create_dir_all(&vd).unwrap();
    let original = format!(r#"{{"verdict":"approved","reviewed_sha":"{short}"}}"#);
    std::fs::write(vd.join("PR-1.json"), &original).unwrap();

    let report = backfill_abbreviated_shas(repo, true).unwrap();
    assert_eq!(report.resolved, vec!["PR-1.json".to_string()]);

    let untouched = std::fs::read_to_string(vd.join("PR-1.json")).unwrap();
    assert_eq!(
        untouched, original,
        "dry-run must report what it would do without changing the file"
    );
}

// BUG-1581: two independent reviewers can reach opposite conclusions at the
// same commit.  The retained evidence must make that disagreement explicit;
// whichever review happened to be written last must not decide the answer.
// trace:BUG-1581 | ai:codex
#[test]
fn same_sha_opposite_reviewers_conflict_in_either_artifact_order() {
    let approved = serde_json::json!({
        "verdict": "approved",
        "reviewed_sha": "ac772eaca9d389fa762a232156df996023bfdf7a",
        "recorded_by": "reviewer-a"
    });
    let blocked = serde_json::json!({
        "verdict": "request-changes",
        "reviewed_sha": "ac772eaca9",
        "recorded_by": "reviewer-b"
    });

    for (current, archived) in [(&approved, &blocked), (&blocked, &approved)] {
        let mut artifact = current.clone();
        artifact["rounds"] = serde_json::json!([archived]);
        let conflict = verdict_conflict_for_current_sha(&artifact.to_string())
            .expect("opposite verdicts by different reviewers at one sha must conflict");
        assert!(conflict.contains("reviewer-a"), "{conflict}");
        assert!(conflict.contains("reviewer-b"), "{conflict}");
    }
}

// trace:BUG-1581 | ai:codex
#[test]
fn stale_opposite_and_non_opposing_current_rounds_are_controls() {
    let stale = serde_json::json!({
        "verdict": "request-changes",
        "reviewed_sha": "1111111111111111111111111111111111111111",
        "recorded_by": "reviewer-b"
    });
    let same_verdict = serde_json::json!({
        "verdict": "approved",
        "reviewed_sha": "222222222",
        "recorded_by": "reviewer-b"
    });
    let current = serde_json::json!({
        "verdict": "approved",
        "reviewed_sha": "2222222222222222222222222222222222222222",
        "recorded_by": "reviewer-a",
        "rounds": [stale, same_verdict]
    });
    assert_eq!(verdict_conflict_for_current_sha(&current.to_string()), None);
}

// trace:BUG-1581 | ai:codex
#[test]
fn explicit_path_writer_preserves_displaced_reviewer_evidence() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join(".aida/review-verdicts/PR-2066.json");
    record_verdict_at_path(
        tmp.path(),
        &path,
        Some("approved"),
        Some("ac772eaca9"),
        Some("topic"),
        Some("first"),
        &[],
        "reviewer-a",
    )
    .unwrap();
    record_verdict_at_path(
        tmp.path(),
        &path,
        Some("request-changes"),
        Some("ac772eaca9"),
        Some("topic"),
        Some("second"),
        &["real omission".into()],
        "reviewer-b",
    )
    .unwrap();

    let body = std::fs::read_to_string(path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["rounds"][0]["recorded_by"], "reviewer-a");
    assert_eq!(value["rounds"][0]["verdict"], "approved");
    assert!(verdict_conflict_for_current_sha(&body).is_some());
}

// trace:BUG-1508 | ai:claude
#[test]
fn no_verdict_needs_review() {
    assert_eq!(
        review_actionability(None, TipRelation::Unknown),
        ReviewActionability::NeedsReview
    );
}

// trace:BUG-1508 | ai:claude
#[test]
fn blocking_verdict_at_current_head_is_awaiting_rework() {
    let v = rc("request-changes", Some("aaaa"));
    assert_eq!(
        review_actionability(Some(&v), TipRelation::AtReviewedSha),
        ReviewActionability::AwaitingRework
    );
}

// trace:BUG-1508 | ai:claude
#[test]
fn approving_verdict_at_current_head_is_resolved() {
    let v = rc("approved", Some("aaaa"));
    assert_eq!(
        review_actionability(Some(&v), TipRelation::AtReviewedSha),
        ReviewActionability::Resolved
    );
}

// trace:BUG-1508 | ai:claude
#[test]
fn head_advanced_past_a_blocking_verdict_needs_review_again() {
    // Criterion 3: once the head moves past what was reviewed, the entry is
    // actionable again -- even though the last word was "changes requested".
    let v = rc("request-changes", Some("aaaa"));
    assert_eq!(
        review_actionability(Some(&v), TipRelation::AdvancedPast),
        ReviewActionability::NeedsReview
    );
}

// trace:BUG-1508 | ai:claude
#[test]
fn rewritten_branch_needs_review_even_with_a_prior_approval() {
    let v = rc("approved", Some("aaaa"));
    assert_eq!(
        review_actionability(Some(&v), TipRelation::Rewritten),
        ReviewActionability::NeedsReview
    );
}

// trace:BUG-1508 | ai:claude
#[test]
fn permanently_indeterminate_verdict_is_treated_as_absent() {
    // Criterion 8: a verdict with no reviewed_sha can never be placed against
    // a head. classify_tip_relation already reports this as Unknown, and
    // Unknown must NOT be read as "covers the current head" -- the reassuring
    // reading is exactly the PRIN-5 violation this spec exists to prevent.
    let v = rc("request-changes", None);
    let relation = classify_tip_relation(v.reviewed_sha.as_deref(), Some("bbbb"), None);
    assert_eq!(relation, TipRelation::Unknown);
    assert_eq!(
        review_actionability(Some(&v), relation),
        ReviewActionability::NeedsReview
    );
}

// ────────────────────────────────────────────────────────────────────
// BUG-1529: closing a refusal's verdict record when its reworked PR merges.
// ────────────────────────────────────────────────────────────────────

#[test]
fn close_verdict_on_merge_stamps_a_blocking_verdict() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    record_verdict(
        root,
        "STORY-1",
        Some("request-changes"),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        None,
        Some("three blocking defects"),
        &[],
        "reviewer-a",
    )
    .unwrap();

    let closed = close_verdict_on_merge(root, "STORY-1", "cccccccccccc").unwrap();
    assert!(closed, "a blocking verdict must be closeable");

    let v = read_recorded_verdict(root, "STORY-1").expect("verdict still parses");
    assert!(v.is_closed());
    assert_eq!(v.closed_by_merge.as_deref(), Some("cccccccccccc"));
    assert!(v.closed_at.is_some());
    // The refusal itself is untouched -- criterion 2: closing is not a fresh
    // approving review.
    assert_eq!(v.kind, VerdictKind::RequestChanges);
    assert_eq!(v.summary.as_deref(), Some("three blocking defects"));
}

#[test]
fn close_verdict_on_merge_is_a_no_op_for_a_non_blocking_verdict() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    record_verdict(
        root,
        "STORY-2",
        Some("approved"),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        None,
        None,
        &[],
        "reviewer-a",
    )
    .unwrap();

    assert!(!close_verdict_on_merge(root, "STORY-2", "cccccccccccc").unwrap());
    let v = read_recorded_verdict(root, "STORY-2").unwrap();
    assert!(!v.is_closed());
}

#[test]
fn close_verdict_on_merge_never_overwrites_the_first_closer() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    record_verdict(
        root,
        "STORY-3",
        Some("request-changes"),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        None,
        None,
        &[],
        "reviewer-a",
    )
    .unwrap();

    assert!(close_verdict_on_merge(root, "STORY-3", "first-sha").unwrap());
    // A second call (e.g. a re-run of the auto-bump scan) must not clobber
    // the original closing reference.
    assert!(!close_verdict_on_merge(root, "STORY-3", "second-sha").unwrap());
    let v = read_recorded_verdict(root, "STORY-3").unwrap();
    assert_eq!(v.closed_by_merge.as_deref(), Some("first-sha"));
}

#[test]
fn close_verdict_on_merge_is_a_no_op_with_no_file_or_empty_ref() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    assert!(!close_verdict_on_merge(root, "STORY-4", "some-sha").unwrap());

    record_verdict(
        root,
        "STORY-5",
        Some("rejected"),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        None,
        None,
        &[],
        "reviewer-a",
    )
    .unwrap();
    assert!(!close_verdict_on_merge(root, "STORY-5", "   ").unwrap());
}

#[test]
fn outstanding_refusal_query_excludes_closed_and_completed_specs() {
    // A live refusal, spec still open: outstanding.
    let live = rc("request-changes", Some("aaaa"));
    assert!(is_outstanding_refusal(&live, false));

    // Same refusal, but the spec is Completed -- criterion 4's fallback for
    // the pre-existing corpus this fix cannot retroactively rewrite.
    assert!(!is_outstanding_refusal(&live, true));

    // A refusal closed by a merge is not outstanding even while the spec
    // status is unknown/open in the caller's view.
    let mut closed = rc("request-changes", Some("aaaa"));
    closed.closed_by_merge = Some("deadbeef".to_string());
    assert!(!is_outstanding_refusal(&closed, false));

    // An approval was never a refusal.
    let approved = rc("approved", Some("aaaa"));
    assert!(!is_outstanding_refusal(&approved, false));
}

// trace:BUG-1529 | ai:claude
#[test]
fn a_closed_verdict_reads_resolved_regardless_of_tip_relation() {
    // The head has necessarily moved past the reviewed sha by the time a
    // merge closes the verdict, so AdvancedPast/Rewritten/Unknown must not
    // reroute a closed record back to NeedsReview / AwaitingRework.
    let mut v = rc("request-changes", Some("aaaa"));
    v.closed_by_merge = Some("deadbeef".to_string());
    for relation in [
        TipRelation::AtReviewedSha,
        TipRelation::AdvancedPast,
        TipRelation::Rewritten,
        TipRelation::Unknown,
    ] {
        assert_eq!(
            review_actionability(Some(&v), relation),
            ReviewActionability::Resolved,
            "closed verdict should read Resolved at relation {relation:?}"
        );
    }
}

// BUG-1529 review fix: a new round must not inherit the previous round's
// close, and a closed refusal must not suppress a later PR.
// trace:BUG-1529 | ai:claude
#[test]
fn a_fresh_refusal_after_a_close_is_not_closed() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let rc = |sha: &str| {
        record_verdict(
            root,
            "STORY-9",
            Some("request-changes"),
            Some(sha),
            None,
            Some("blocking"),
            &[],
            "reviewer-a",
        )
        .unwrap();
    };
    rc("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    assert!(close_verdict_on_merge(root, "STORY-9", "cccccccccccc").unwrap());
    assert!(read_recorded_verdict(root, "STORY-9").unwrap().is_closed());
    rc("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    let v = read_recorded_verdict(root, "STORY-9").unwrap();
    assert!(!v.is_closed(), "a new round must not inherit the old close");
    assert_eq!(v.kind, VerdictKind::RequestChanges);
}

// trace:BUG-1529 | ai:claude
#[test]
fn a_closed_refusal_does_not_suppress_a_later_pr() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    // Sha-less refusal: were it live, it would be Unverifiable and suppress.
    record_verdict(
        root,
        "STORY-8",
        Some("request-changes"),
        None,
        None,
        Some("blocking"),
        &[],
        "reviewer-a",
    )
    .unwrap();
    let live = read_recorded_verdict(root, "STORY-8").unwrap();
    assert!(
        crate::awaiting_you::classify_pr_review(std::slice::from_ref(&live), Some("dddddddd"))
            .suppressed,
        "control: a live sha-less refusal suppresses"
    );
    assert!(close_verdict_on_merge(root, "STORY-8", "cccccccccccc").unwrap());
    let closed = read_recorded_verdict(root, "STORY-8").unwrap();
    assert!(
        !crate::awaiting_you::classify_pr_review(std::slice::from_ref(&closed), Some("dddddddd"))
            .suppressed,
        "a closed refusal must not suppress a later PR"
    );
}

// BUG-1571: the handshake write must fail LOUDLY, not silently, when the
// artefact cannot actually land. Simulated by pointing the target path at a
// read-only directory: `create_dir_all` on an existing dir is a no-op, so
// the write itself is what trips, exactly like a permissions/quota/disk
// failure in the field would.
// trace:BUG-1571 | ai:claude
#[cfg(unix)]
#[test]
fn pr_keyed_write_reports_failure_honestly_when_the_directory_is_read_only() {
    use std::os::unix::fs::PermissionsExt;
    // Root bypasses directory permissions (CAP_DAC_OVERRIDE), so a read-only
    // directory cannot force the failure this test needs. trace:BUG-1571
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipping: running as root, directory permissions are not enforced");
        return;
    }

    let tmp = TempDir::new().unwrap();
    let locked_dir = tmp.path().join(".aida/review-verdicts");
    std::fs::create_dir_all(&locked_dir).unwrap();
    let path = locked_dir.join("PR-9001.json");

    let mut perms = std::fs::metadata(&locked_dir).unwrap().permissions();
    perms.set_mode(0o555); // read + execute, no write
    std::fs::set_permissions(&locked_dir, perms.clone()).unwrap();

    let result = record_verdict_at_path(
        tmp.path(),
        &path,
        Some("approved"),
        Some("ac772eaca9"),
        Some("topic"),
        Some("looks good"),
        &[],
        "reviewer-a",
    );

    // Restore write access so TempDir can clean itself up on drop.
    let mut restore = perms;
    restore.set_mode(0o755);
    std::fs::set_permissions(&locked_dir, restore).unwrap();

    let err = result
        .expect_err("a write that cannot land must be reported as an error, never as a success");
    let message = err.to_string();
    assert!(
        message.contains("PR-9001.json"),
        "the failure must name the artefact that did not land: {message}"
    );
    assert!(
        !path.exists(),
        "the artefact must genuinely be absent when the write is reported as failed"
    );
}

// BUG-1571: a caller layering extra fields onto build_verdict_object (the
// orchestrator's phase-3 handshake overlay) and committing with
// write_verdict_object gets the same honest-failure guarantee as the
// single-shot record_verdict_at_path path.
// trace:BUG-1571 | ai:claude
#[cfg(unix)]
#[test]
fn layered_handshake_write_reports_failure_honestly_when_the_directory_is_read_only() {
    use std::os::unix::fs::PermissionsExt;
    // Root bypasses directory permissions (CAP_DAC_OVERRIDE), so a read-only
    // directory cannot force the failure this test needs. trace:BUG-1571
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipping: running as root, directory permissions are not enforced");
        return;
    }

    let tmp = TempDir::new().unwrap();
    let locked_dir = tmp.path().join(".aida/review-verdicts");
    std::fs::create_dir_all(&locked_dir).unwrap();
    let path = locked_dir.join("PR-9002.json");

    let obj = build_verdict_object(
        tmp.path(),
        &path,
        Some("approved"),
        Some("ac772eaca9"),
        Some("topic"),
        Some("looks good"),
        &[],
        "reviewer-a",
    )
    .unwrap();

    let mut perms = std::fs::metadata(&locked_dir).unwrap().permissions();
    perms.set_mode(0o555);
    std::fs::set_permissions(&locked_dir, perms.clone()).unwrap();

    let result = write_verdict_object(&path, &obj);

    let mut restore = perms;
    restore.set_mode(0o755);
    std::fs::set_permissions(&locked_dir, restore).unwrap();

    let err = result.expect_err("a failed layered write must surface as an error");
    assert!(
        err.to_string().contains("PR-9002.json"),
        "the failure must name the artefact that did not land: {err}"
    );
    assert!(!path.exists(), "the artefact must genuinely be absent");
}

// ── BUG-1539: every round archived by reviewed sha ─────────────────────────
// trace:BUG-1539 | ai:claude
const SHA_R1: &str = "3acf3671fd7a1111111111111111111111111111";
const SHA_R2: &str = "cd21a1dc0a9e2222222222222222222222222222";

fn record_pr(root: &Path, pr: &str, verdict: &str, sha: &str, by: &str, findings: &[String]) {
    let path = verdict_path(root, pr);
    record_verdict_at_path(
        root,
        &path,
        Some(verdict),
        Some(sha),
        Some("topic"),
        Some("summary"),
        findings,
        by,
    )
    .unwrap();
}

fn archive_files(root: &Path, key: &str) -> Vec<String> {
    let dir = verdict_archive_dir(&verdict_path(root, key)).unwrap();
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".json"))
        .collect();
    names.sort();
    names
}

// trace:BUG-1539 | ai:claude
#[test]
fn two_rounds_on_different_shas_keep_both() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    record_pr(
        root,
        "PR-2049",
        "request-changes",
        SHA_R1,
        "reviewer-a",
        &["round-1 finding".into()],
    );
    record_pr(root, "PR-2049", "approved", SHA_R2, "drain reviewer", &[]);

    assert_eq!(
        archive_files(root, "PR-2049"),
        vec![format!("{SHA_R1}.json"), format!("{SHA_R2}.json")]
    );
    let r1 = read_verdict_for_sha(root, "PR-2049", SHA_R1).expect("round 1 survives");
    assert_eq!(r1.kind, VerdictKind::RequestChanges);
    assert_eq!(r1.findings, vec!["round-1 finding".to_string()]);
    assert_eq!(r1.recorded_by.as_deref(), Some("reviewer-a"));
    let r2 = read_verdict_for_sha(root, "PR-2049", &SHA_R2[..12]).expect("round 2 by prefix");
    assert_eq!(r2.kind, VerdictKind::Approved);
    // A sha nobody reviewed has no verdict.
    assert!(read_verdict_for_sha(root, "PR-2049", "deadbeefdeadbeef").is_none());
}

// trace:BUG-1539 | ai:claude
#[test]
fn same_sha_recorded_twice_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let f = vec!["one finding".to_string()];
    record_pr(root, "PR-7", "request-changes", SHA_R1, "reviewer-a", &f);
    record_pr(root, "PR-7", "request-changes", SHA_R1, "reviewer-a", &f);

    assert_eq!(archive_files(root, "PR-7"), vec![format!("{SHA_R1}.json")]);
    let archive = verdict_archive_dir(&verdict_path(root, "PR-7"))
        .unwrap()
        .join(format!("{SHA_R1}.json"));
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&archive).unwrap()).unwrap();
    assert!(
        v.get("rounds").is_none(),
        "an identical re-record must not manufacture a round: {v}"
    );

    // A DIFFERENT reviewer at the same sha is not a duplicate: one commit,
    // two recordings, the first kept in the archive's own rounds.
    record_pr(root, "PR-7", "approved", SHA_R1, "reviewer-b", &[]);
    assert_eq!(archive_files(root, "PR-7"), vec![format!("{SHA_R1}.json")]);
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&archive).unwrap()).unwrap();
    assert_eq!(v["recorded_by"], "reviewer-b");
    assert_eq!(v["rounds"].as_array().unwrap().len(), 1);
    assert_eq!(v["rounds"][0]["recorded_by"], "reviewer-a");
}

// trace:BUG-1539 | ai:claude
#[test]
fn current_file_readers_are_unchanged_by_the_archive() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    record_pr(
        root,
        "PR-11",
        "request-changes",
        SHA_R1,
        "reviewer-a",
        &["f".into()],
    );
    record_pr(root, "PR-11", "approved", SHA_R2, "reviewer-b", &[]);

    // The PR-keyed current file still holds the latest round at the top
    // level, with the prior round in `rounds`, exactly as before.
    let current = read_recorded_verdict(root, "PR-11").expect("current file");
    assert_eq!(current.kind, VerdictKind::Approved);
    assert_eq!(current.reviewed_sha.as_deref(), Some(SHA_R2));
    let body = std::fs::read_to_string(verdict_path(root, "PR-11")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["rounds"][0]["reviewed_sha"], SHA_R1);

    // The archive directory is invisible to `*.json` walkers of the dir.
    let top: Vec<_> = std::fs::read_dir(root.join(".aida/review-verdicts"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    assert_eq!(top, vec![verdict_path(root, "PR-11")]);

    // The TASK-1448 merge-gate candidate set is unchanged: one PR-keyed record.
    let cands = crate::pr_ship::merge_gate_verdict_candidates(&[root], 11, &[], None);
    assert_eq!(cands.len(), 1);
    assert_eq!(cands[0].kind, VerdictKind::Approved);
}

// trace:BUG-1539 | ai:claude
#[test]
fn same_tree_under_a_second_pr_number_finds_the_review() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    record_pr(root, "PR-2043", "request-changes", SHA_R1, "drain", &[]);
    // PR-2042 carries the identical tree but has no file of its own.
    assert!(read_verdict_for_sha(root, "PR-2042", SHA_R1).is_none());
    let found = verdicts_for_sha(root, SHA_R1);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, VerdictKind::RequestChanges);
}

// trace:BUG-1539 | ai:claude
#[test]
fn a_round_with_no_reviewed_sha_is_not_archived() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let path = verdict_path(root, "PR-5");
    record_verdict_at_path(root, &path, Some("approved"), None, None, None, &[], "x").unwrap();
    assert!(!verdict_archive_dir(&path).unwrap().exists());
}

// ── TASK-1460: per-sha archive wired through close, adopt, migrate, gate ──
// trace:TASK-1460 | ai:claude
#[test]
fn close_on_merge_also_closes_the_archived_round() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    record_pr(
        root,
        "TASK-77",
        "request-changes",
        SHA_R1,
        "reviewer-a",
        &["f".into()],
    );
    assert!(!read_verdict_for_sha(root, "TASK-77", SHA_R1)
        .unwrap()
        .is_closed());
    assert!(close_verdict_on_merge(root, "TASK-77", "abc1234").unwrap());
    let archived = read_verdict_for_sha(root, "TASK-77", SHA_R1).unwrap();
    assert!(archived.is_closed(), "the archived round must read closed");
    assert!(verdicts_for_sha(root, SHA_R1).iter().all(|v| v.is_closed()));
    // Verdict word and findings are untouched by the close.
    assert_eq!(archived.kind, VerdictKind::RequestChanges);
    assert_eq!(archived.findings, vec!["f".to_string()]);
}

// trace:TASK-1460 | ai:claude
#[test]
fn close_on_merge_archives_a_pre_archive_refusal_closed() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let dir = root.join(".aida/review-verdicts");
    std::fs::create_dir_all(&dir).unwrap();
    // Written before the archive existed; the migration marker is present so
    // nothing else archives it first.
    std::fs::write(dir.join(".archive-migrated"), "0\n").unwrap();
    std::fs::write(
        dir.join("BUG-9.json"),
        format!(r#"{{"verdict":"rejected","reviewed_sha":"{SHA_R1}"}}"#),
    )
    .unwrap();
    assert!(close_verdict_on_merge(root, "BUG-9", "PR-3").unwrap());
    assert!(read_verdict_for_sha(root, "BUG-9", SHA_R1)
        .unwrap()
        .is_closed());
    let archive = verdict_archive_dir(&verdict_path(root, "BUG-9"))
        .unwrap()
        .join(format!("{SHA_R1}.json"));
    assert!(archive.is_file());
}

// trace:TASK-1460 | ai:claude
#[test]
fn a_direct_skill_write_is_adopted_into_the_archive() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let path = verdict_path(root, "PR-40");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let body = format!(
        r#"{{"verdict":"Approved","summary":"ok","mode":"orchestrator-phase-3","reviewed_sha":"{SHA_R2}"}}"#
    );
    std::fs::write(&path, &body).unwrap();
    adopt_direct_write(&path).unwrap();
    adopt_direct_write(&path).unwrap(); // idempotent
    assert_eq!(archive_files(root, "PR-40"), vec![format!("{SHA_R2}.json")]);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        body,
        "the current file is left as the skill wrote it"
    );
    assert_eq!(
        verdict_file_for_sha(root, "PR-40", SHA_R2).as_deref(),
        Some(path.as_path()),
        "the current file answers first when it is at the sha"
    );

    // No reviewed commit → unidentifiable → not archived.
    let bare = verdict_path(root, "PR-41");
    std::fs::write(&bare, r#"{"verdict":"Approved"}"#).unwrap();
    adopt_direct_write(&bare).unwrap();
    assert!(!verdict_archive_dir(&bare).unwrap().exists());
}

// trace:TASK-1460 | ai:claude
#[test]
fn migration_archives_existing_sidecars_once_and_idempotently() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let dir = root.join(".aida/review-verdicts");
    std::fs::create_dir_all(&dir).unwrap();
    let legacy = format!(
        r#"{{"verdict":"approved","reviewed_sha":"{SHA_R2}","recorded_by":"b","recorded_at":"2026-09-02T00:00:00Z",
            "rounds":[{{"verdict":"request-changes","head":"{SHA_R1}","recorded_by":"a","recorded_at":"2026-09-01T00:00:00Z","findings":["x"]}}]}}"#
    );
    std::fs::write(dir.join("PR-3.json"), &legacy).unwrap();
    std::fs::write(dir.join("PR-4.json"), r#"{"verdict":"approved"}"#).unwrap();
    std::fs::write(dir.join("PR-5.json"), "not json").unwrap();

    assert_eq!(migrate_sidecars_to_archive(&dir).unwrap(), 2);
    assert_eq!(
        archive_files(root, "PR-3"),
        vec![format!("{SHA_R1}.json"), format!("{SHA_R2}.json")]
    );
    let r1 = read_verdict_for_sha(root, "PR-3", SHA_R1).unwrap();
    assert_eq!(r1.kind, VerdictKind::RequestChanges);
    assert_eq!(r1.findings, vec!["x".to_string()]);
    assert!(!verdict_archive_dir(&dir.join("PR-4.json"))
        .unwrap()
        .exists());
    // Top-level files are untouched; a re-run adds nothing.
    assert_eq!(
        std::fs::read_to_string(dir.join("PR-3.json")).unwrap(),
        legacy
    );
    assert_eq!(migrate_sidecars_to_archive(&dir).unwrap(), 0);

    // The lazy one-shot runs once, then the marker short-circuits it.
    assert_eq!(ensure_sidecars_archived(&dir).unwrap(), 0);
    assert!(dir.join(".archive-migrated").is_file());
    std::fs::remove_dir_all(dir.join("PR-3")).unwrap();
    assert_eq!(ensure_sidecars_archived(&dir).unwrap(), 0);
    assert!(!dir.join("PR-3").exists(), "marker makes it one-shot");
}

// trace:TASK-1460 | ai:claude
#[test]
fn the_record_path_migrates_existing_sidecars_lazily() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let dir = root.join(".aida/review-verdicts");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("PR-8.json"),
        format!(r#"{{"verdict":"approved","reviewed_sha":"{SHA_R1}"}}"#),
    )
    .unwrap();
    record_pr(root, "PR-9", "approved", SHA_R2, "r", &[]);
    assert_eq!(archive_files(root, "PR-8"), vec![format!("{SHA_R1}.json")]);
    assert!(dir.join(".archive-migrated").is_file());
}

// trace:TASK-1460 | ai:claude
#[test]
fn merge_gate_candidates_include_the_archived_verdict_at_head() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    record_pr(root, "PR-20", "approved", SHA_R1, "reviewer-a", &[]);
    // A later round at a different commit replaces the current file.
    record_pr(root, "PR-20", "request-changes", SHA_R2, "reviewer-b", &[]);

    let without_head = crate::pr_ship::merge_gate_verdict_candidates(&[root], 20, &[], None);
    assert_eq!(without_head.len(), 1);
    let at_head = crate::pr_ship::merge_gate_verdict_candidates(&[root], 20, &[], Some(SHA_R1));
    assert_eq!(at_head.len(), 2);
    assert!(at_head
        .iter()
        .any(|v| v.kind == VerdictKind::Approved && v.reviewed_sha.as_deref() == Some(SHA_R1)));
    // The approval covers the head, so the TASK-1448 gate passes.
    assert_eq!(
        crate::pr_ship::approval_head_refusal(&at_head, Some(SHA_R1)),
        None
    );
    // When the current file is already at head nothing is duplicated.
    let current_head =
        crate::pr_ship::merge_gate_verdict_candidates(&[root], 20, &[], Some(SHA_R2));
    assert_eq!(current_head.len(), 1);
}

// ---------------------------------------------------------------------------
// BUG-1505: one canonical verdict vocabulary + one normalizing parser.
// trace:BUG-1505 | ai:claude
// ---------------------------------------------------------------------------

/// Every verdict spelling observed in `.aida/review-verdicts/` (surveyed
/// 2026-09-23, 666 files), with the kind the canonical parser must read it as.
const OBSERVED_SPELLINGS: &[(&str, VerdictKind)] = &[
    ("Approved", VerdictKind::Approved),
    ("APPROVED", VerdictKind::Approved),
    ("approved", VerdictKind::Approved),
    ("approve", VerdictKind::Approved),
    ("Approve", VerdictKind::Approved),
    ("CHANGES REQUESTED", VerdictKind::RequestChanges),
    ("RequestChanges", VerdictKind::RequestChanges),
    ("request-changes", VerdictKind::RequestChanges),
    // The two qualified values: both contain APPROVED, neither is an approval.
    (
        "APPROVED pending cross-platform green",
        VerdictKind::Unknown,
    ),
    (
        "CONTENT APPROVED — MERGE WITHHELD FOR INDEPENDENCE",
        VerdictKind::Unknown,
    ),
];

#[test]
fn bug_1505_every_observed_spelling_maps_to_its_canonical_kind() {
    for (raw, want) in OBSERVED_SPELLINGS {
        assert_eq!(VerdictKind::parse(raw), *want, "{raw}");
    }
}

#[test]
fn bug_1505_qualified_approvals_never_approve_anywhere() {
    for raw in [
        "APPROVED pending cross-platform green",
        "CONTENT APPROVED — MERGE WITHHELD FOR INDEPENDENCE",
        "approved, but",
        "not approved",
        "",
    ] {
        let kind = VerdictKind::parse(raw);
        assert_eq!(kind, VerdictKind::Unknown, "{raw}");
        assert!(!kind.approves(), "{raw}");
        assert_eq!(kind.canonical(), None, "{raw}");
        // The orchestrator's and the summary's readers agree.
        assert_eq!(crate::auto_complete::Verdict::parse(raw), None, "{raw}");
    }
}

#[test]
fn bug_1505_canonical_spellings_round_trip() {
    for kind in [
        VerdictKind::Approved,
        VerdictKind::RequestChanges,
        VerdictKind::Rejected,
    ] {
        let c = kind.canonical().unwrap();
        assert_eq!(VerdictKind::parse(c), kind);
        assert_eq!(canonical_verdict_word(c), c);
    }
    assert_eq!(
        canonical_verdict_word("CHANGES REQUESTED"),
        "request-changes"
    );
    assert_eq!(canonical_verdict_word(" APPROVED "), "approved");
    // An unknown word is preserved verbatim, never guessed into a canonical one.
    assert_eq!(
        canonical_verdict_word("APPROVED pending cross-platform green"),
        "APPROVED pending cross-platform green"
    );
}

#[test]
fn bug_1505_queue_done_gate_refuses_a_qualified_approval_at_the_head() {
    let v = rc(
        "CONTENT APPROVED — MERGE WITHHELD FOR INDEPENDENCE",
        Some("3ae8f937"),
    );
    assert!(matches!(
        queue_done_verdict_gate("PR-1979", Some(&v), TipRelation::AtReviewedSha),
        VerdictGate::Refuse(_)
    ));
}

#[test]
fn bug_1505_reader_resolves_legacy_key_aliases() {
    let legacy = r#"{"verdict":"APPROVED","summary":"s","head":"c87ac9e4",
                     "reviewer":"claude advisor","date":"2026-09-19",
                     "blocking_findings":["f1"]}"#;
    let v = parse_recorded_verdict(legacy).unwrap();
    assert_eq!(v.kind, VerdictKind::Approved);
    assert_eq!(v.reviewed_sha.as_deref(), Some("c87ac9e4"));
    assert_eq!(v.recorded_by.as_deref(), Some("claude advisor"));
    assert_eq!(v.recorded_at.as_deref(), Some("2026-09-19"));
    assert_eq!(v.findings, vec!["f1".to_string()]);

    let head_sha = r#"{"verdict":"approved","summary":"s","head_sha":"abcdef1234"}"#;
    assert_eq!(
        parse_recorded_verdict(head_sha)
            .unwrap()
            .reviewed_sha
            .as_deref(),
        Some("abcdef1234")
    );
    // The canonical key wins over an alias.
    let both =
        r#"{"verdict":"approved","summary":"s","reviewed_sha":"aaaaaaa1","head":"bbbbbbb2"}"#;
    assert_eq!(
        parse_recorded_verdict(both)
            .unwrap()
            .reviewed_sha
            .as_deref(),
        Some("aaaaaaa1")
    );
}

#[test]
fn bug_1505_unverifiable_marker_drops_the_sha_and_the_gate_refuses() {
    let body = r#"{"verdict":"APPROVED","summary":"s","unverifiable":true,
                   "unverifiable_reason":"legacy","head":"c87ac9e4"}"#;
    let v = parse_recorded_verdict(body).unwrap();
    assert_eq!(v.reviewed_sha, None);
    assert!(matches!(
        queue_done_verdict_gate("PR-1976", Some(&v), TipRelation::Unknown),
        VerdictGate::Refuse(_)
    ));
}

#[test]
fn bug_1505_writer_persists_the_canonical_spelling() {
    let tmp = TempDir::new().unwrap();
    for (raw, want) in [
        ("APPROVED", "approved"),
        ("CHANGES REQUESTED", "request-changes"),
        ("RequestChanges", "request-changes"),
        ("Rejected", "rejected"),
    ] {
        let path = record_verdict(
            tmp.path(),
            "TASK-9",
            Some(raw),
            None,
            None,
            Some("s"),
            &[],
            "test",
        )
        .unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(v["verdict"], want, "{raw}");
    }
}

#[test]
fn bug_1505_verdictless_stamp_canonicalizes_the_word_on_disk() {
    // The drain reviewer stamps provenance with no verdict of its own; the
    // reviewer-written spelling must still land canonical.
    let tmp = TempDir::new().unwrap();
    let path = verdict_path(tmp.path(), "PR-7");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, r#"{"verdict":"CHANGES REQUESTED","summary":"s"}"#).unwrap();
    record_verdict(tmp.path(), "PR-7", None, None, None, None, &[], "drain").unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["verdict"], "request-changes");

    // An unknown word is left verbatim for a human.
    let odd = r#"{"verdict":"APPROVED pending cross-platform green","summary":"s"}"#;
    std::fs::write(&path, odd).unwrap();
    record_verdict(tmp.path(), "PR-7", None, None, None, None, &[], "drain").unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["verdict"], "APPROVED pending cross-platform green");
}

#[test]
fn bug_1505_audit_classifies_every_observed_spelling() {
    for (raw, kind) in OBSERVED_SPELLINGS {
        let body = serde_json::json!({
            "verdict": raw, "summary": "s",
            "reviewed_sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "recorded_by": "t",
        })
        .to_string();
        let (got_raw, got_kind, issues) = audit_verdict_body(&body);
        assert_eq!(got_raw, *raw);
        assert_eq!(got_kind, *kind);
        let canonical = kind.canonical() == Some(*raw);
        assert_eq!(issues.is_empty(), canonical, "{raw}: {issues:?}");
        if *kind == VerdictKind::Unknown {
            assert!(issues.iter().any(|i| i.contains("UNKNOWN")), "{issues:?}");
        }
    }
}

#[test]
fn bug_1505_audit_flags_legacy_keys_and_missing_sha_without_rewriting() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join(".aida/review-verdicts");
    std::fs::create_dir_all(&dir).unwrap();
    let legacy = r#"{"verdict":"Approved","summary":"s","reviewer":"x"}"#;
    let canonical =
        r#"{"verdict":"approved","summary":"s","reviewed_sha":"aaaaaaa","recorded_by":"t"}"#;
    std::fs::write(dir.join("PR-1.json"), legacy).unwrap();
    std::fs::write(dir.join("PR-2.json"), canonical).unwrap();
    let rows = audit_verdict_dir(tmp.path());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].file, "PR-1.json");
    let joined = rows[0].issues.join("\n");
    assert!(joined.contains("not canonical"), "{joined}");
    assert!(joined.contains("legacy key `reviewer`"), "{joined}");
    assert!(joined.contains("no reviewed sha"), "{joined}");
    // Report-only: the file is byte-identical afterwards.
    assert_eq!(
        std::fs::read_to_string(dir.join("PR-1.json")).unwrap(),
        legacy
    );
}
