//! Tests of the review-verdict display helpers (STORY-1889, moved from
//! `review_verdict_tests.rs`).
// trace:STORY-1889 | ai:claude
use super::*;
use crate::gate::review_verdict::*;

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
        inherited_findings: false,
        surviving_findings: Vec::new(),
        recorded_by: None,
        closed_by_merge: None,
        closed_at: None,
        attestation: None,
    }
}

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
