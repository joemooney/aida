// trace:TASK-3780 | ai:codex
use super::*;

// trace:BUG-1505 | ai:claude
#[test]
fn unknown_verdicts_get_their_own_finding_and_drift_is_aggregated() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join(".aida/review-verdicts");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
            dir.join("PR-1979-advisor.json"),
            r#"{"verdict":"CONTENT APPROVED — MERGE WITHHELD FOR INDEPENDENCE","summary":"s","head":"3ae8f937"}"#,
        )
        .unwrap();
    std::fs::write(
        dir.join("PR-5.json"),
        r#"{"verdict":"APPROVED","summary":"s"}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("PR-6.json"),
        r#"{"verdict":"approved","summary":"s","reviewed_sha":"aaaaaaa","recorded_by":"t"}"#,
    )
    .unwrap();
    let findings = scan_review_verdicts(tmp.path());
    assert_eq!(findings.len(), 2, "{findings:?}");
    assert!(findings
        .iter()
        .any(|f| f.id == "unknown-verdict:PR-1979-advisor.json"));
    let agg = findings
        .iter()
        .find(|f| f.id == "non-canonical-verdicts")
        .unwrap();
    assert!(
        agg.summary.starts_with("1 review-verdict file"),
        "{}",
        agg.summary
    );
    assert!(agg.summary.contains("PR-5.json"));
    assert_eq!(
        normalize_doctor_category("verdicts").unwrap(),
        "review-verdicts"
    );
}

// trace:BUG-1962 | ai:antigravity
#[test]
fn render_doctor_report_toon_includes_runaway_seats_still_tripped_table() {
    let mut report = DoctorReport::default();
    report.runaway_seats = Some(crate::runaway_seats::Coverage {
        verdict: "tripped".to_string(),
        still_tripped: vec!["compactions:sess-1".to_string()],
        ..Default::default()
    });
    let rendered = render_doctor_report_toon(&report);
    assert!(
        rendered.contains("runaway_seats_still_tripped"),
        "TOON output must include runaway_seats_still_tripped table: {rendered}"
    );
    assert!(
        rendered.contains("compactions:sess-1"),
        "TOON output must list still tripped finding: {rendered}"
    );
}
