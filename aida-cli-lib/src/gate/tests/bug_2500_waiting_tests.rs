//! Waiting blocks delivery without manufacturing implementer work.
// trace:BUG-2500 | ai:codex
use crate::gate::review_verdict as rv;
use crate::queue_cmd::resolve_rework_findings;
const HEAD: &str = "34031ff4a06c1d24c08846619f87aaa66b31dec7";
const SUMMARY: &str = "Host: operator must run the host probe; resume review with probe evidence";

#[test]
fn bug_2500_waiting_round_trip_and_repeated_rework_refusal() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    for key in ["BUG-2500", "PR-3698"] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        rv::record_verdict(
            root,
            key,
            Some("WAITING"),
            Some(HEAD),
            None,
            Some(SUMMARY),
            &[],
            "reviewer",
        )
        .unwrap();
        let v = rv::read_recorded_verdict(root, key).unwrap();
        assert_eq!(v.raw, "waiting");
        assert!(v.kind.blocks_done());
        let body = rv::current_verdict(root, key).unwrap().body;
        let bodies = vec![(key.to_string(), body)];
        assert!(matches!(
            crate::gate::merge_hold_core::corpus_merge_gate(3698, &bodies, Some(HEAD)),
            crate::gate::merge_hold_core::CorpusMergeGate::Refuse { arm: None, .. }
        ));
        assert!(!v.kind.requires_changes());
        assert!(!v.kind.approves());
        assert!(
            crate::review_verdict_display::rework_findings_comment("BUG-2500", key, &v).is_none()
        );
        for relation in [
            rv::TipRelation::AtReviewedSha,
            rv::TipRelation::AdvancedPast,
            rv::TipRelation::Rewritten,
            rv::TipRelation::Unknown,
        ] {
            assert!(matches!(
                rv::queue_done_verdict_gate(key, Some(&v), relation),
                rv::VerdictGate::Refuse(_)
            ));
        }
        for _ in 0..3 {
            let err = resolve_rework_findings(
                root,
                "BUG-2500",
                "BUG-2500",
                (key == "PR-3698").then_some(key),
            )
            .unwrap_err();
            assert!(err.contains("WAITING") && err.contains("Host:"), "{err}");
        }
        assert!(!root.join(".aida/queue").exists());
        assert!(!root.join(".aida/merge-holds").exists());
        // A new review, rather than a CI transition, replaces the wait.
        rv::record_verdict(
            root,
            key,
            Some("approved"),
            Some(HEAD),
            None,
            Some("host evidence checked"),
            &[],
            "reviewer",
        )
        .unwrap();
        assert_eq!(
            resolve_rework_findings(
                root,
                "BUG-2500",
                "BUG-2500",
                (key == "PR-3698").then_some(key)
            ),
            Ok(None)
        );
    }
}

#[test]
fn bug_2500_waiting_requires_summary_and_rejects_findings() {
    let tmp = tempfile::tempdir().unwrap();
    for (summary, findings) in [
        (None, vec![]),
        (Some(SUMMARY), vec!["code defect".to_string()]),
    ] {
        assert!(rv::record_verdict(
            tmp.path(),
            "BUG-2500",
            Some("waiting"),
            Some(HEAD),
            None,
            summary,
            &findings,
            "reviewer"
        )
        .is_err());
    }
    assert_eq!(
        crate::auto_complete::Verdict::parse("waiting"),
        Some(crate::auto_complete::Verdict::Waiting)
    );
}

/// Substantive refusals retain the existing recurrence guard.
#[test]
fn bug_2500_substantive_findings_still_count_as_repeated() {
    let body = serde_json::json!({"verdict":"request-changes", "reviewed_sha":HEAD,
        "summary":"code defect", "findings":["repair the failing code path"]})
    .to_string();
    let v = rv::parse_recorded_verdict(&body).unwrap();
    let block =
        crate::review_verdict_display::rework_findings_comment("BUG-2500", "PR-3698", &v).unwrap();
    let comments = vec![aida_core::Comment::new(
        "reviewer".to_string(),
        block.clone(),
    )];
    assert!(crate::queue_cmd::findings_recur_consecutively(
        &comments, &block
    ));
}
