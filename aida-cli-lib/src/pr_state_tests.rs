// trace:STORY-2688 | ai:codex
use super::*;
fn facts() -> PrFacts {
    PrFacts {
        observed_at: "2026-11-11T10:00:00Z".into(),
        project: "github:example/repo".into(),
        number: 1,
        head: "a".repeat(40),
        observed_head: "a".repeat(40),
        base: "b".repeat(40),
        workflow: Observed::Known("workflow".into()),
        workflow_fresh: true,
        open: Observed::Known(true),
        mergeable: Observed::Known(true),
        behind: false,
        ci: Observed::Known(Ci::Green),
        held: Observed::Known(false),
        operation_owner: None,
        unsafe_evidence: None,
        policy_ruling: None,
        verdict: Some(Verdict {
            head: "a".repeat(40),
            kind: VerdictKind::Approved,
            identity: "round1".into(),
            attested: true,
        }),
        attempts: 0,
        retry_backoff: false,
        retry_exhausted: false,
        owners: Owners {
            advisor: vec!["advisor-seat".into()],
            implementer: vec!["impl-seat".into()],
            reviewer: vec!["review-seat".into()],
            train: vec!["train-seat".into()],
        },
    }
}
fn set(f: &mut PrFacts, state: PrState) {
    use PrState::*;
    match state {
        Gone => f.open = Observed::Known(false),
        EvidencePending => f.open = Observed::Transient("network unavailable".into()),
        Held => f.held = Observed::Known(true),
        InFlight => f.operation_owner = Some("recorded-seat".into()),
        NeedsRuling => f.policy_ruling = Some("judgment required".into()),
        Conflict => f.mergeable = Observed::Known(false),
        BehindBase => f.behind = true,
        NeedsRework => f.verdict.as_mut().unwrap().kind = VerdictKind::RequestChanges,
        RedCi => f.ci = Observed::Known(Ci::Red),
        CiPendingThenVerdict => {
            f.verdict = None;
            f.ci = Observed::Known(Ci::Pending);
        }
        NeedsVerdict => {
            f.verdict = None;
            f.ci = Observed::Known(Ci::Green);
        }
        ApprovedCiPending => f.ci = Observed::Known(Ci::Pending),
        ReadyForTrain => (),
    }
}
#[test]
fn pr_state_every_state_and_owner() {
    use PrState::*;
    for (state, owner) in [
        (Gone, "reconciler"),
        (EvidencePending, "reconciler"),
        (Held, "advisor-seat"),
        (InFlight, "recorded-seat"),
        (NeedsRuling, "advisor-seat"),
        (Conflict, "impl-seat"),
        (BehindBase, "impl-seat"),
        (NeedsRework, "impl-seat"),
        (RedCi, "impl-seat"),
        (CiPendingThenVerdict, "review-seat"),
        (NeedsVerdict, "review-seat"),
        (ApprovedCiPending, "reconciler"),
        (ReadyForTrain, "train-seat"),
    ] {
        let mut f = facts();
        set(&mut f, state);
        let d = classify(f);
        assert_eq!(d.state, state);
        assert_eq!(d.owner.as_deref(), Some(owner));
        assert!(!d.reason.is_empty());
        assert!(!d.evidence_key.is_empty());
    }
}
#[test]
fn pr_state_every_adjacent_precedence_pair() {
    use PrState::*;
    let states = [
        Gone,
        EvidencePending,
        Held,
        InFlight,
        NeedsRuling,
        Conflict,
        BehindBase,
        NeedsRework,
        RedCi,
        CiPendingThenVerdict,
        NeedsVerdict,
        ApprovedCiPending,
        ReadyForTrain,
    ];
    for pair in states.windows(2) {
        let mut f = facts();
        set(&mut f, pair[1]);
        set(&mut f, pair[0]);
        assert_eq!(classify(f).state, pair[0], "{pair:?}");
    }
}
#[test]
fn pr_state_unknown_transient_and_moving_head() {
    for transient in [false, true] {
        let mut f = facts();
        f.mergeable = if transient {
            Observed::Transient("timeout".into())
        } else {
            Observed::Pending("UNKNOWN".into())
        };
        assert_eq!(classify(f).state, PrState::EvidencePending);
    }
    let mut f = facts();
    f.observed_head = "c".repeat(40);
    assert_eq!(classify(f).state, PrState::EvidencePending);
    let mut f = facts();
    f.workflow = Observed::Transient("workflow timeout".into());
    assert_eq!(classify(f).state, PrState::EvidencePending);
}
#[test]
fn pr_state_stale_head_workflow_and_old_verdict() {
    let mut f = facts();
    f.workflow_fresh = false;
    assert_eq!(classify(f).state, PrState::BehindBase);
    for kind in [
        VerdictKind::Approved,
        VerdictKind::RequestChanges,
        VerdictKind::Rejected,
    ] {
        let mut f = facts();
        let v = f.verdict.as_mut().unwrap();
        v.head = "old".into();
        v.kind = kind;
        assert_eq!(classify(f).state, PrState::NeedsVerdict);
    }
}
#[test]
fn pr_state_waiting_and_no_verdict_retries() {
    for waiting in [false, true] {
        for ci in [Ci::Green, Ci::Pending] {
            let mut f = facts();
            if waiting {
                f.verdict.as_mut().unwrap().kind = VerdictKind::Waiting;
            } else {
                f.verdict = None;
            }
            f.ci = Observed::Known(ci);
            f.attempts = 2;
            let d = classify(f.clone());
            assert_eq!(d.attempt_count, 2);
            assert_eq!(
                d.state,
                if ci == Ci::Green {
                    PrState::NeedsVerdict
                } else {
                    PrState::CiPendingThenVerdict
                }
            );
            f.retry_backoff = true;
            assert_eq!(classify(f.clone()).state, PrState::EvidencePending);
            f.retry_backoff = false;
            f.retry_exhausted = true;
            assert_eq!(classify(f).state, PrState::NeedsRuling);
        }
    }
}
#[test]
fn pr_state_missing_ambiguous_and_duplicate_owners() {
    let mut f = facts();
    f.owners.train.clear();
    assert_eq!(classify(f.clone()).state, PrState::NeedsRuling);
    f.owners.advisor.clear();
    let d = classify(f);
    assert!(d.owner.is_none());
    assert!(d.reason.contains("configuration refusal"));
    let mut f = facts();
    f.verdict = None;
    f.owners.reviewer.push("other-seat".into());
    assert_eq!(classify(f).state, PrState::NeedsRuling);
    let mut f = facts();
    f.owners.train.push("train-seat".into());
    assert_eq!(classify(f).state, PrState::ReadyForTrain);
}
#[test]
fn pr_state_unsafe_and_unattested_are_rulings() {
    let mut f = facts();
    f.unsafe_evidence = Some("symlink".into());
    assert_eq!(classify(f).state, PrState::NeedsRuling);
    let mut f = facts();
    f.verdict.as_mut().unwrap().attested = false;
    assert_eq!(classify(f).state, PrState::NeedsRuling);
}
#[test]
fn pr_state_round_and_workflow_change_evidence_key() {
    let f = facts();
    let key = classify(f.clone()).evidence_key;
    let mut changed = f.clone();
    changed.verdict.as_mut().unwrap().identity = "round2".into();
    assert_ne!(key, classify(changed).evidence_key);
    let mut changed = f;
    changed.workflow = Observed::Known("new-workflow".into());
    assert_ne!(key, classify(changed).evidence_key);
}
#[test]
fn pr_state_twenty_rows_classify_within_budget() {
    let start = std::time::Instant::now();
    for n in 1..=20 {
        let mut f = facts();
        f.number = n;
        assert_eq!(classify(f).state, PrState::ReadyForTrain);
    }
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
}
#[test]
fn pr_state_cli_registration() {
    let cli = crate::command_registry::try_parse_from(["aida", "pr", "state", "--format", "json"])
        .unwrap();
    assert!(matches!(
        cli.globals.format,
        Some(crate::cli::OutputFormat::Json)
    ));
    let args = cli.command.registered::<crate::pr_cmd::PrArgs>();
    assert!(matches!(args.inner, crate::pr_cmd::PrCommand::State(_)));
    let cli = crate::command_registry::try_parse_from(["aida", "pr", "state", "--json"]).unwrap();
    let args = cli.command.registered::<crate::pr_cmd::PrArgs>();
    assert!(matches!(&args.inner, crate::pr_cmd::PrCommand::State(args) if args.json));
    assert!(crate::command_registry::try_parse_from(["aida", "pr", "state", "--execute"]).is_err());
}

fn row(n: u64) -> serde_json::Value {
    serde_json::json!({"number": n, "url": format!("https://github.com/example/repo/pull/{n}"),
        "title": "Change", "headRefOid": "a".repeat(40), "baseRefOid": "b".repeat(40), "headRefName": "feature",
        "baseRefName": "main", "mergeStateStatus": "CLEAN", "mergeable": "MERGEABLE", "state": "OPEN", "isDraft": false,
        "labels": [], "statusCheckRollup": [{"name": "Build", "conclusion": "SUCCESS", "workflowName": "CI"}]})
}
#[test]
fn pr_state_batch_collection_twenty_prs_is_read_only_and_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let rows: Vec<_> = (1..=20).map(row).collect();
    let mut calls = 0;
    let mut workflow_calls = 0;
    let start = std::time::Instant::now();
    let facts = collect_with(
        temp.path(),
        |_, budget| {
            assert!(budget <= std::time::Duration::from_secs(3));
            calls += 1;
            Ok(rows.clone())
        },
        |_, batch, _| {
            workflow_calls += 1;
            assert_eq!(batch.len(), 20);
            let mut values = serde_json::Map::new();
            for i in 0..20 {
                for suffix in ["h", "b"] {
                    values.insert(
                        format!("p{i}{suffix}"),
                        serde_json::json!({"oid": "same-workflow"}),
                    );
                }
            }
            Ok(values.into())
        },
    )
    .unwrap();
    assert_eq!(facts.len(), 20);
    assert_eq!(calls, 2);
    assert_eq!(workflow_calls, 1);
    assert!(facts
        .iter()
        .all(|f| f.workflow_fresh && f.ci == Observed::Known(Ci::Green)));
    assert!(start.elapsed() < std::time::Duration::from_secs(10));
    assert_eq!(
        std::fs::read_dir(temp.path()).unwrap().count(),
        0,
        "collection must not create runtime or store files"
    );
}
#[test]
fn pr_state_collector_rechecks_heads_and_preserves_transient_workflows() {
    let temp = tempfile::tempdir().unwrap();
    let mut calls = 0;
    let result = collect_with(
        temp.path(),
        |_, _| {
            calls += 1;
            let mut r = row(1);
            if calls == 2 {
                r["headRefOid"] = serde_json::json!("c".repeat(40));
            }
            Ok(vec![r])
        },
        |_, _, _| anyhow::bail!("network timeout"),
    )
    .unwrap();
    assert_ne!(result[0].head, result[0].observed_head);
    assert_eq!(classify(result[0].clone()).state, PrState::EvidencePending);
}
#[test]
fn pr_state_required_and_informational_ci() {
    let mut r = row(1);
    let cfg = crate::ci_gate::CiGateConfig::default();
    assert_eq!(
        parse_row(&r, &["Missing".into()], &cfg).unwrap().ci,
        Observed::Known(Ci::Pending)
    );
    r["statusCheckRollup"] =
        serde_json::json!([{"name":"Build", "conclusion":"FAILURE", "workflowName":"CI"}]);
    assert_eq!(
        parse_row(&r, &[], &cfg).unwrap().ci,
        Observed::Known(Ci::Red)
    );
    let cfg = crate::ci_gate::CiGateConfig {
        informational_workflows: vec!["CI".into()],
        informational_checks: vec![],
    };
    assert_eq!(
        parse_row(&r, &[], &cfg).unwrap().ci,
        Observed::Known(Ci::Green)
    );
    assert_eq!(
        parse_row(&r, &["Build".into()], &cfg).unwrap().ci,
        Observed::Known(Ci::Red)
    );
}

#[test]
fn pr_state_overlapping_receipts_require_one_concrete_owner() {
    let mut f = facts();
    f.observe_operation(Some("lane-seat".into()));
    f.observe_operation(Some("review-seat".into()));
    assert_eq!(classify(f).state, PrState::NeedsRuling);
    let mut f = facts();
    f.observe_operation(Some("lane-seat".into()));
    f.observe_operation(Some("lane-seat".into()));
    assert_eq!(classify(f).state, PrState::InFlight);
}
#[test]
fn pr_state_approval_supersedes_old_retry_exhaustion() {
    let mut f = facts();
    f.retry_exhausted = true;
    f.retry_backoff = true;
    assert_eq!(classify(f).state, PrState::ReadyForTrain);
}
#[test]
fn pr_state_unrecognized_lifecycle_cannot_retire() {
    let mut r = row(1);
    r["state"] = serde_json::json!("UNRECOGNIZED");
    assert_eq!(
        classify(parse_row(&r, &[], &crate::ci_gate::CiGateConfig::default()).unwrap()).state,
        PrState::EvidencePending
    );
}
