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

// trace:BUG-2531 | ai:codex
// trace:BUG-2542 | ai:claude
#[test]
fn bug_2531_doctor_warns_at_half_gate_ledger_budget() {
    use crate::gate::ledger::checked::{MAX_LEDGER_BYTES, MAX_SELECTION_BYTES};
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join(".aida/gate-ledger");
    std::fs::create_dir_all(&dir).unwrap();
    // Eight ledgers each under half the per-file cap, so only the aggregate
    // finding can fire. Sparse files keep the fixture cheap.
    let half = MAX_SELECTION_BYTES / 2;
    let each = half / 8;
    assert!(each < MAX_LEDGER_BYTES / 2);
    let files: Vec<_> = (1..=8)
        .map(|n| std::fs::File::create(dir.join(format!("PR-{n}.jsonl"))).unwrap())
        .collect();
    for f in &files[..7] {
        f.set_len(each).unwrap();
    }
    files[7].set_len(each - 1).unwrap();
    assert!(scan_gate_ledger_capacity(tmp.path()).is_empty());
    files[7].set_len(each).unwrap();
    let findings = scan_gate_ledger_capacity(tmp.path());
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].id, "gate-ledger-capacity");
    assert!(!findings[0].safe_heal);
    assert!(findings[0].summary.contains("PR-1.jsonl"), "{findings:?}");
    assert!(
        findings[0].action.contains("gate-ledger-quarantine"),
        "{findings:?}"
    );
    // Archived/non-change files do not contribute to active capacity.
    std::fs::rename(dir.join("PR-1.jsonl"), dir.join("archived.jsonl")).unwrap();
    assert!(scan_gate_ledger_capacity(tmp.path()).is_empty());
}

// trace:BUG-2542 | ai:claude
#[test]
fn bug_2542_doctor_names_oversized_and_near_cap_change_ledgers() {
    use crate::gate::ledger::checked::MAX_LEDGER_BYTES;
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join(".aida/gate-ledger");
    std::fs::create_dir_all(&dir).unwrap();
    let big = std::fs::File::create(dir.join("PR-3888.jsonl")).unwrap();
    big.set_len(MAX_LEDGER_BYTES / 2 - 1).unwrap();
    assert!(scan_gate_ledger_capacity(tmp.path()).is_empty());

    big.set_len(MAX_LEDGER_BYTES / 2).unwrap();
    let findings = scan_gate_ledger_capacity(tmp.path());
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].id, "gate-ledger-near-cap:PR-3888.jsonl");

    big.set_len(MAX_LEDGER_BYTES + 1).unwrap();
    let findings = scan_gate_ledger_capacity(tmp.path());
    assert_eq!(findings.len(), 1, "{findings:?}");
    let f = &findings[0];
    assert_eq!(f.id, "gate-ledger-oversized:PR-3888.jsonl");
    assert!(!f.safe_heal);
    assert!(f.summary.contains("PR-3888.jsonl"), "{f:?}");
    assert!(f.summary.contains("every spec"), "{f:?}");
    assert!(f.action.contains("mv '"), "{f:?}");
    assert!(f.action.contains("gate-ledger-quarantine"), "{f:?}");

    // Following the prescribed fix clears the finding and keeps the evidence.
    let q = tmp.path().join(".aida/gate-ledger-quarantine");
    std::fs::create_dir_all(&q).unwrap();
    std::fs::rename(dir.join("PR-3888.jsonl"), q.join("PR-3888.jsonl")).unwrap();
    assert!(scan_gate_ledger_capacity(tmp.path()).is_empty());
    assert!(q.join("PR-3888.jsonl").exists());
}
