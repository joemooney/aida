//! Waiting blocks delivery without manufacturing implementer work.
// trace:BUG-2500 | ai:codex
use crate::gate::review_verdict as rv;
use crate::queue_cmd::resolve_rework_findings;
use aida_core::DatabaseBackend;
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

#[test]
fn bug_2500_waiting_status_routing_hold_and_merge_closure() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    let tmp = tempfile::tempdir().unwrap();
    rv::record_verdict(
        tmp.path(),
        "BUG-2500",
        Some("waiting"),
        Some(HEAD),
        None,
        Some(SUMMARY),
        &[],
        "reviewer",
    )
    .unwrap();
    let v = rv::read_recorded_verdict(tmp.path(), "BUG-2500").unwrap();
    let body = rv::current_verdict(tmp.path(), "BUG-2500").unwrap().body;
    assert!(rv::corpus_hold_at_head(&[body], Some(HEAD), false).is_none());
    for relation in [
        rv::TipRelation::AtReviewedSha,
        rv::TipRelation::AdvancedPast,
        rv::TipRelation::Rewritten,
        rv::TipRelation::Unknown,
    ] {
        assert_eq!(
            rv::review_actionability(tmp.path(), "BUG-2500", Some(&v), relation),
            if relation == rv::TipRelation::AtReviewedSha {
                rv::ReviewActionability::Waiting
            } else {
                rv::ReviewActionability::NeedsReview
            }
        );
    }
    let approved = rv::RecordedVerdict {
        kind: rv::VerdictKind::Approved,
        ..v.clone()
    };
    for candidates in [
        vec![v.clone()],
        vec![approved.clone(), v.clone()],
        vec![v.clone(), approved],
    ] {
        for head in [
            Some(HEAD),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            None,
        ] {
            let status = crate::awaiting_you::classify_pr_review(&candidates, head);
            assert!(status.suppressed);
            assert!(matches!(
                status.row,
                Some(crate::awaiting_you::PrReviewRow::Blocked {
                    reason: crate::awaiting_you::BlockedReason::Waiting,
                    ..
                })
            ));
        }
    }
    let fresh_approval = rv::RecordedVerdict {
        kind: rv::VerdictKind::Approved,
        recorded_at: Some((chrono::Utc::now() + chrono::Duration::seconds(1)).to_rfc3339()),
        ..v.clone()
    };
    assert!(
        !crate::awaiting_you::classify_pr_review(&[v.clone(), fresh_approval], Some(HEAD))
            .suppressed
    );
    let substantive = rv::RecordedVerdict {
        kind: rv::VerdictKind::RequestChanges,
        ..v.clone()
    };
    for candidates in [
        vec![v.clone(), substantive.clone()],
        vec![substantive, v.clone()],
    ] {
        assert!(matches!(
            crate::awaiting_you::classify_pr_review(&candidates, Some(HEAD)).row,
            Some(crate::awaiting_you::PrReviewRow::Blocked {
                reason: crate::awaiting_you::BlockedReason::AtHead,
                ..
            })
        ));
    }
    assert!(rv::close_verdict_on_merge(tmp.path(), "BUG-2500", "merge-sha").unwrap());
    let closed = rv::read_recorded_verdict(tmp.path(), "BUG-2500").unwrap();
    assert!(closed.is_closed());
    assert_eq!(
        rv::review_actionability(
            tmp.path(),
            "BUG-2500",
            Some(&closed),
            rv::TipRelation::AdvancedPast
        ),
        rv::ReviewActionability::Resolved
    );
    assert!(matches!(
        rv::queue_done_verdict_gate("BUG-2500", Some(&closed), rv::TipRelation::AtReviewedSha),
        rv::VerdictGate::Proceed
    ));
    assert_eq!(
        resolve_rework_findings(tmp.path(), "BUG-2500", "BUG-2500", None),
        Ok(None)
    );
}

#[test]
fn bug_2500_early_admission_ignores_non_waiting_selection_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(".aida/review-verdicts");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("PR-3698.json"), "invalid JSON").unwrap();
    assert!(resolve_rework_findings(tmp.path(), "BUG-2500", "BUG-2500", Some("PR-3698")).is_err());
    assert_eq!(
        crate::queue_cmd::refuse_waiting_review(
            tmp.path(),
            "BUG-2500",
            "BUG-2500",
            Some("PR-3698")
        ),
        Ok(())
    );
}

#[test]
fn bug_2500_waiting_sibling_cannot_hide_code_findings_in_either_order() {
    for words in [
        ["waiting", "request-changes"],
        ["request-changes", "waiting"],
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("drive");
        std::fs::create_dir(&root).unwrap();
        for (index, word) in words.iter().enumerate() {
            let dir = tmp
                .path()
                .join(format!("sibling-{index}/.aida/review-verdicts"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("PR-3698.json"),
                serde_json::json!({
                    "verdict": word, "reviewed_sha": HEAD, "summary": SUMMARY
                })
                .to_string(),
            )
            .unwrap();
        }
        let result = crate::phase_gates::sibling_verdict_sweep_for_phase3(
            &root,
            3698,
            "BUG-2500",
            std::time::UNIX_EPOCH,
            None,
        )
        .unwrap();
        assert!(matches!(
            result,
            Some(crate::auto_complete::ReviewerOutcome::Verdict(
                crate::auto_complete::Verdict::RequestChanges
            ))
        ));
    }
}

#[test]
fn bug_2500_rework_waiting_preserves_store_but_reviewer_route_and_warning_proceed() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir(root.join(".git")).unwrap();
    std::fs::create_dir(root.join(".aida")).unwrap();
    let store_root = root.join(".aida-store");
    let backend = crate::test_env::home_fixture_backend(&store_root).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let common = aida_core::allocation_authority::common_dir(&store_root).unwrap();
    std::fs::set_permissions(
        common.join(aida_core::allocation_authority::CONFIG_FILE),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let storage = aida_core::Storage::new(&store_root);
    let mut req = aida_core::Requirement::new("external wait".into(), String::new());
    req.spec_id = Some("BUG-2500".into());
    req.status = aida_core::RequirementStatus::Done;
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(req);
    backend.save(&store).unwrap();
    rv::record_verdict(
        root,
        "BUG-2500",
        Some("waiting"),
        Some(HEAD),
        None,
        Some(SUMMARY),
        &[],
        "reviewer",
    )
    .unwrap();
    let original_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(root).unwrap();
    let before = serde_json::to_value(backend.load().unwrap()).unwrap();
    for _ in 0..3 {
        assert!(crate::queue_cmd::handle_queue_rework(
            &storage,
            "BUG-2500",
            false,
            Some("implementer"),
            false,
            None,
            None,
            false,
            false,
            false,
            None,
            true,
            Some("codex")
        )
        .is_err());
        assert_eq!(
            serde_json::to_value(backend.load().unwrap()).unwrap(),
            before
        );
        assert!(storage.queue_users().unwrap().is_empty());
        assert!(!root.join(".aida/merge-holds").exists());
    }
    crate::queue_cmd::handle_queue_rework(
        &storage,
        "BUG-2500",
        false,
        Some("reviewer"),
        false,
        None,
        None,
        false,
        false,
        false,
        None,
        true,
        Some("codex"),
    )
    .unwrap();
    assert!(storage
        .queue_list("codex", true)
        .unwrap()
        .iter()
        .any(|entry| entry.for_role.as_deref() == Some("reviewer")));

    // Multiple bound refusals still follow the existing warning path.
    let mut store = backend.load().unwrap();
    let mut other = aida_core::Requirement::new("ambiguous review evidence".into(), String::new());
    other.spec_id = Some("BUG-2501".into());
    other.status = aida_core::RequirementStatus::Approved;
    store.requirements.push(other);
    backend.save(&store).unwrap();
    for pr in [3700, 3701] {
        rv::record_verdict(
            root,
            &format!("PR-{pr}"),
            Some("request-changes"),
            Some(HEAD),
            None,
            Some("substantive finding"),
            &["fix the code".into()],
            "reviewer",
        )
        .unwrap();
        crate::gate::ledger::project::index_spec(
            &crate::gate::ledger::store_root(root).unwrap(),
            "BUG-2501",
            crate::gate::ledger::ChangeKey::pr(pr),
        )
        .unwrap();
    }
    assert!(resolve_rework_findings(root, "BUG-2501", "BUG-2501", None).is_err());
    crate::queue_cmd::handle_queue_rework(
        &storage,
        "BUG-2501",
        false,
        Some("implementer"),
        false,
        None,
        None,
        false,
        false,
        false,
        None,
        true,
        Some("codex"),
    )
    .unwrap();
    assert!(storage
        .queue_list("codex", true)
        .unwrap()
        .iter()
        .any(|entry| entry.for_role.as_deref() == Some("implementer")));
    std::env::set_current_dir(original_cwd).unwrap();
}

#[test]
fn bug_2500_ambiguous_alias_waiting_refuses_admission() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(".aida/review-verdicts");
    std::fs::create_dir_all(&dir).unwrap();
    for (key, word) in [("BUG-2500", "waiting"), ("BUG-NATIVE-2500", "waiting")] {
        std::fs::write(
            dir.join(format!("{key}.json")),
            serde_json::json!({
                "verdict": word, "reviewed_sha": HEAD, "summary": format!("{SUMMARY} ({key})"),
                "recorded_at": "2026-11-09T01:00:00Z"
            })
            .to_string(),
        )
        .unwrap();
    }
    let err =
        crate::queue_cmd::refuse_waiting_review(tmp.path(), "BUG-2500", "BUG-NATIVE-2500", None)
            .unwrap_err();
    assert!(err.contains("WAITING") && err.contains("Host:"), "{err}");
}

#[test]
fn bug_2500_multiple_bound_waits_refuse_admission() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    for spec_record in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        if spec_record {
            rv::record_verdict(
                tmp.path(),
                "BUG-2500",
                Some("approved"),
                Some(HEAD),
                None,
                Some("older approval"),
                &[],
                "reviewer",
            )
            .unwrap();
        }
        for pr in [3700, 3701] {
            rv::record_verdict(
                tmp.path(),
                &format!("PR-{pr}"),
                Some("waiting"),
                Some(HEAD),
                None,
                Some(SUMMARY),
                &[],
                "reviewer",
            )
            .unwrap();
            crate::gate::ledger::project::index_spec(
                &crate::gate::ledger::store_root(tmp.path()).unwrap(),
                "BUG-2500",
                crate::gate::ledger::ChangeKey::pr(pr),
            )
            .unwrap();
        }
        let err = crate::queue_cmd::refuse_waiting_review(tmp.path(), "BUG-2500", "BUG-2500", None)
            .unwrap_err();
        assert!(err.contains("WAITING"), "{err}");
        assert!(!tmp.path().join(".aida/queue").exists());
    }
}

// A wait beside code defects must not steal the implementer's actionable work.
// trace:BUG-2500 | ai:codex
#[test]
fn bug_2500_mixed_waiting_imports_substantive_findings() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    for swapped in [false, true] {
        for layout in [
            "aliases",
            "spec-change",
            "bound",
            "bound-with-spec",
            "bound-with-wait",
            "spec-wait-bound-refusals",
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let (first, second) = if layout == "spec-wait-bound-refusals" {
                ("request-changes", "request-changes")
            } else if swapped {
                ("request-changes", "waiting")
            } else {
                ("waiting", "request-changes")
            };
            let keys = match layout {
                "aliases" => ["BUG-2500", "BUG-NATIVE-2500"],
                "spec-change" => ["BUG-2500", "PR-3700"],
                _ => ["PR-3700", "PR-3701"],
            };
            if matches!(
                layout,
                "bound-with-spec" | "bound-with-wait" | "spec-wait-bound-refusals"
            ) {
                rv::record_verdict(
                    tmp.path(),
                    "BUG-2500",
                    Some(if layout != "bound-with-spec" {
                        "waiting"
                    } else {
                        "approved"
                    }),
                    Some(HEAD),
                    None,
                    Some(SUMMARY),
                    &[],
                    "reviewer",
                )
                .unwrap();
            }
            for (key, word) in keys.into_iter().zip([first, second]) {
                if layout == "aliases" || layout == "spec-change" {
                    let dir = tmp.path().join(".aida/review-verdicts");
                    std::fs::create_dir_all(&dir).unwrap();
                    std::fs::write(dir.join(format!("{key}.json")), serde_json::json!({
                        "verdict": word, "reviewed_sha": HEAD, "summary": SUMMARY,
                        "findings": if word == "waiting" { vec![] } else { vec!["repair the code defect"] },
                        "recorded_at": "2026-11-09T01:00:00Z"
                    }).to_string()).unwrap();
                } else {
                    let findings = if word == "waiting" {
                        vec![]
                    } else {
                        vec!["repair the code defect".into()]
                    };
                    rv::record_verdict(
                        tmp.path(),
                        key,
                        Some(word),
                        Some(HEAD),
                        None,
                        Some(SUMMARY),
                        &findings,
                        "reviewer",
                    )
                    .unwrap();
                    if key.starts_with("PR-") {
                        crate::gate::ledger::project::index_spec(
                            &crate::gate::ledger::store_root(tmp.path()).unwrap(),
                            "BUG-2500",
                            crate::gate::ledger::ChangeKey::parse(key).unwrap(),
                        )
                        .unwrap();
                    }
                }
            }
            let native = if layout == "aliases" {
                "BUG-NATIVE-2500"
            } else {
                "BUG-2500"
            };
            let change = (layout == "spec-change").then_some("PR-3700");
            crate::queue_cmd::refuse_waiting_review(tmp.path(), "BUG-2500", native, change)
                .unwrap();
            let findings = resolve_rework_findings(tmp.path(), "BUG-2500", native, change)
                .unwrap()
                .expect(layout);
            assert!(
                findings.contains("repair the code defect"),
                "{layout}: {findings}"
            );
            assert!(findings.contains("CHANGES REQUESTED"));
            assert!(!findings.contains("Verdict: WAITING"));
        }
    }
}

#[test]
fn bug_2500_fresh_spec_approval_clears_single_bound_wait() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    let tmp = tempfile::tempdir().unwrap();
    rv::record_verdict(
        tmp.path(),
        "PR-3700",
        Some("waiting"),
        Some(HEAD),
        None,
        Some(SUMMARY),
        &[],
        "reviewer",
    )
    .unwrap();
    crate::gate::ledger::project::index_spec(
        &crate::gate::ledger::store_root(tmp.path()).unwrap(),
        "BUG-2500",
        crate::gate::ledger::ChangeKey::pr(3700),
    )
    .unwrap();
    rv::record_verdict(
        tmp.path(),
        "BUG-2500",
        Some("approved"),
        Some(HEAD),
        None,
        Some("fresh review checked host evidence"),
        &[],
        "reviewer",
    )
    .unwrap();
    assert_eq!(
        resolve_rework_findings(tmp.path(), "BUG-2500", "BUG-2500", None),
        Ok(None)
    );
    crate::queue_cmd::refuse_waiting_review(tmp.path(), "BUG-2500", "BUG-2500", None).unwrap();
}

// A newer review supersedes findings; ties retain substantive work.
// trace:BUG-2500 | ai:codex
#[test]
fn bug_2500_newer_wait_supersedes_older_refusal_across_keys() {
    for swapped in [false, true] {
        for newer_wait in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path().join(".aida/review-verdicts");
            std::fs::create_dir_all(&dir).unwrap();
            for (index, key) in ["BUG-2500", "PR-3700"].into_iter().enumerate() {
                let waiting = (index == 0) == swapped;
                let newer = waiting == newer_wait;
                std::fs::write(dir.join(format!("{key}.json")), serde_json::json!({
                    "verdict": if waiting { "waiting" } else { "request-changes" },
                    "reviewed_sha": HEAD, "summary": SUMMARY,
                    "findings": if waiting { vec![] } else { vec!["repair the code defect"] },
                    "recorded_at": if newer { "2026-11-09T02:00:00Z" } else { "2026-11-09T01:00:00Z" }
                }).to_string()).unwrap();
            }
            let result =
                resolve_rework_findings(tmp.path(), "BUG-2500", "BUG-2500", Some("PR-3700"));
            if newer_wait {
                assert!(result.unwrap_err().contains("WAITING"));
            } else {
                assert!(result.unwrap().unwrap().contains("repair the code defect"));
            }
        }
    }
}

// trace:BUG-2525 | ai:codex
#[test]
fn bug_2525_terminal_refs_batch_wait_admits_next_pickup_without_open_pr() {
    use crate::forge::ChangeState;
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    for state in [ChangeState::Merged, ChangeState::Closed] {
        for key in ["TASK-3448", "PR-4273"] {
            let tmp = tempfile::tempdir().unwrap();
            assert!(std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(tmp.path())
                .status()
                .unwrap()
                .success());
            assert!(std::process::Command::new("git")
                .current_dir(tmp.path())
                .args([
                    "remote",
                    "add",
                    "origin",
                    "https://github.com/owner/repo.git"
                ])
                .status()
                .unwrap()
                .success());
            let dir = tmp.path().join(".aida/review-verdicts");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join(format!("{key}.json")),
                serde_json::json!({
                    "verdict": "waiting", "reviewed_sha": HEAD, "summary": SUMMARY,
                    "comment_url": "https://github.com/owner/repo/pull/4273#issuecomment-1"
                })
                .to_string(),
            )
            .unwrap();
            if key.starts_with("PR-") {
                crate::gate::ledger::project::index_spec(
                    &crate::gate::ledger::store_root(tmp.path()).unwrap(),
                    "TASK-3448",
                    crate::gate::ledger::ChangeKey::pr(4273),
                )
                .unwrap();
            }
            crate::review_waiting::reconcile_waiting_verdicts_with(tmp.path(), None, |change| {
                assert_eq!(change.number, 4273);
                // A terminal named PR also retires a wait from an earlier head.
                Some((state, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()))
            });
            // No open PR, no Done -> Completed transition, and no new approval.
            crate::queue_cmd::refuse_waiting_review(tmp.path(), "TASK-3448", "TASK-3448", None)
                .unwrap();
            let closed = rv::read_recorded_verdict(tmp.path(), key).unwrap();
            assert!(closed.is_closed());
            assert_eq!(closed.kind, rv::VerdictKind::Waiting);
            assert_eq!(closed.closed_by_merge.as_deref(), Some("PR-4273"));
            assert_eq!(closed.summary.as_deref(), Some(SUMMARY));
            crate::review_waiting::reconcile_waiting_verdicts_with(tmp.path(), None, |_| {
                panic!("already closed")
            });
        }
    }
}

// trace:BUG-2525 | ai:codex
#[test]
fn bug_2525_wait_requires_terminal_matching_head_evidence() {
    use crate::forge::ChangeState;
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    for observation in [
        None,
        Some((ChangeState::Open, HEAD.into())),
        Some((
            ChangeState::Merged,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        )),
        Some((ChangeState::Closed, String::new())),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        rv::record_verdict(
            tmp.path(),
            "TASK-3448",
            Some("waiting"),
            Some(HEAD),
            None,
            Some(SUMMARY),
            &[],
            "reviewer",
        )
        .unwrap();
        crate::gate::ledger::project::index_spec(
            &crate::gate::ledger::store_root(tmp.path()).unwrap(),
            "TASK-3448",
            crate::gate::ledger::ChangeKey::pr(4273),
        )
        .unwrap();
        crate::review_waiting::reconcile_waiting_verdicts_with(tmp.path(), None, |_| {
            observation.clone()
        });
        let verdict = rv::read_recorded_verdict(tmp.path(), "TASK-3448").unwrap();
        assert!(!verdict.is_closed(), "{observation:?}");
        assert!(matches!(
            rv::queue_done_verdict_gate(
                "TASK-3448",
                Some(&verdict),
                rv::TipRelation::AtReviewedSha
            ),
            rv::VerdictGate::Refuse(_)
        ));
    }
}

// trace:BUG-2525 | ai:codex
#[test]
fn bug_2525_terminal_observation_does_not_close_a_newer_round() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    let tmp = tempfile::tempdir().unwrap();
    rv::record_verdict(
        tmp.path(),
        "PR-4273",
        Some("waiting"),
        Some(HEAD),
        None,
        Some(SUMMARY),
        &[],
        "reviewer",
    )
    .unwrap();
    let next = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    crate::review_waiting::reconcile_waiting_verdicts_with(tmp.path(), None, |_| {
        rv::record_verdict(
            tmp.path(),
            "PR-4273",
            Some("waiting"),
            Some(next),
            None,
            Some("new evidence required"),
            &[],
            "reviewer",
        )
        .unwrap();
        Some((crate::forge::ChangeState::Merged, HEAD.into()))
    });
    let verdict = rv::read_recorded_verdict(tmp.path(), "PR-4273").unwrap();
    assert!(!verdict.is_closed());
    assert_eq!(verdict.reviewed_sha.as_deref(), Some(next));
}

// trace:BUG-2525 | ai:codex
#[test]
fn bug_2525_indexed_spec_wait_closes_only_matching_batch_in_pickup_scope() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    let tmp = tempfile::tempdir().unwrap();
    for key in ["TASK-3448", "TASK-3428"] {
        rv::record_verdict(
            tmp.path(),
            key,
            Some("waiting"),
            Some(HEAD),
            None,
            Some(SUMMARY),
            &[],
            "reviewer",
        )
        .unwrap();
        for number in [4272, 4273, 4274] {
            crate::gate::ledger::project::index_spec(
                &crate::gate::ledger::store_root(tmp.path()).unwrap(),
                key,
                crate::gate::ledger::ChangeKey::pr(number),
            )
            .unwrap();
        }
    }
    crate::review_waiting::reconcile_waiting_verdicts_with(
        tmp.path(),
        Some(&["TASK-3448".into()]),
        |change| {
            Some((
                crate::forge::ChangeState::Merged,
                if change.number == 4273 {
                    HEAD.into()
                } else {
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()
                },
            ))
        },
    );
    let verdict = rv::read_recorded_verdict(tmp.path(), "TASK-3448").unwrap();
    assert_eq!(verdict.closed_by_merge.as_deref(), Some("PR-4273"));
    assert!(
        rv::current_verdict(tmp.path(), "PR-4274").is_none(),
        "the newest batch is not overwritten by the old batch closure"
    );
    assert!(!rv::read_recorded_verdict(tmp.path(), "TASK-3428")
        .unwrap()
        .is_closed());
    crate::queue_cmd::refuse_waiting_review(tmp.path(), "TASK-3448", "TASK-3448", None).unwrap();
}

// trace:BUG-2525 | ai:codex
#[test]
fn bug_2525_closed_wait_annotation_does_not_clear_code_findings() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    for wait_key in ["TASK-3448", "PR-4273"] {
        let tmp = tempfile::tempdir().unwrap();
        let code_key = if wait_key == "TASK-3448" {
            "PR-4273"
        } else {
            "TASK-3448"
        };
        rv::record_verdict(
            tmp.path(),
            wait_key,
            Some("waiting"),
            Some(HEAD),
            None,
            Some(SUMMARY),
            &[],
            "reviewer",
        )
        .unwrap();
        rv::record_verdict(
            tmp.path(),
            code_key,
            Some("request-changes"),
            Some(HEAD),
            None,
            Some("code defect"),
            &["repair the code defect".into()],
            "reviewer",
        )
        .unwrap();
        // Close last: the annotation is newer than the substantive review.
        rv::close_verdict_on_merge(tmp.path(), wait_key, "PR-4272").unwrap();
        let findings =
            resolve_rework_findings(tmp.path(), "TASK-3448", "TASK-3448", Some("PR-4273"))
                .unwrap()
                .expect("the code findings remain actionable");
        assert!(findings.contains("repair the code defect"));
    }
}

// trace:BUG-2525 | ai:codex
#[test]
fn bug_2525_spec_closure_preserves_independent_pr_review() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    let tmp = tempfile::tempdir().unwrap();
    rv::record_verdict(
        tmp.path(),
        "TASK-3448",
        Some("waiting"),
        Some(HEAD),
        None,
        Some(SUMMARY),
        &[],
        "reviewer",
    )
    .unwrap();
    rv::record_verdict(
        tmp.path(),
        "PR-4273",
        Some("request-changes"),
        Some(HEAD),
        None,
        Some("code defect"),
        &["repair the code defect".into()],
        "reviewer",
    )
    .unwrap();
    let source = rv::current_verdict(tmp.path(), "TASK-3448").unwrap();
    assert!(rv::close_terminal_wait(
        tmp.path(),
        "TASK-3448",
        crate::gate::ledger::ChangeKey::pr(4273),
        &source
    )
    .unwrap());
    let pr = rv::read_recorded_verdict(tmp.path(), "PR-4273").unwrap();
    assert_eq!(pr.kind, rv::VerdictKind::RequestChanges);
    assert!(!pr.is_closed());
    assert!(
        resolve_rework_findings(tmp.path(), "TASK-3448", "TASK-3448", Some("PR-4273"))
            .unwrap()
            .unwrap()
            .contains("repair the code defect")
    );
}

// trace:BUG-2525 | ai:codex
#[test]
fn bug_2525_closed_spec_wait_cannot_hide_new_batch_wait() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    let tmp = tempfile::tempdir().unwrap();
    rv::record_verdict(
        tmp.path(),
        "PR-4274",
        Some("waiting"),
        Some(HEAD),
        None,
        Some("new batch evidence"),
        &[],
        "reviewer",
    )
    .unwrap();
    rv::record_verdict(
        tmp.path(),
        "TASK-3448",
        Some("waiting"),
        Some(HEAD),
        None,
        Some(SUMMARY),
        &[],
        "reviewer",
    )
    .unwrap();
    let source = rv::current_verdict(tmp.path(), "TASK-3448").unwrap();
    rv::close_terminal_wait(
        tmp.path(),
        "TASK-3448",
        crate::gate::ledger::ChangeKey::pr(4273),
        &source,
    )
    .unwrap();
    let err = crate::queue_cmd::refuse_waiting_review(
        tmp.path(),
        "TASK-3448",
        "TASK-3448",
        Some("PR-4274"),
    )
    .unwrap_err();
    assert!(err.contains("new batch evidence"), "{err}");
}

// trace:BUG-2525 | ai:codex
#[test]
fn bug_2525_writer_refuses_changed_source_at_same_head() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    let tmp = tempfile::tempdir().unwrap();
    rv::record_verdict(
        tmp.path(),
        "PR-4273",
        Some("waiting"),
        Some(HEAD),
        None,
        Some(SUMMARY),
        &[],
        "reviewer",
    )
    .unwrap();
    let old = rv::current_verdict(tmp.path(), "PR-4273").unwrap();
    rv::record_verdict(
        tmp.path(),
        "PR-4273",
        Some("waiting"),
        Some(HEAD),
        None,
        Some("new evidence"),
        &[],
        "reviewer",
    )
    .unwrap();
    assert!(!rv::close_terminal_wait(
        tmp.path(),
        "PR-4273",
        crate::gate::ledger::ChangeKey::pr(4273),
        &old
    )
    .unwrap());
    assert!(!rv::read_recorded_verdict(tmp.path(), "PR-4273")
        .unwrap()
        .is_closed());
}

// trace:BUG-2525 | ai:codex
#[test]
fn bug_2525_annotation_cannot_override_replaced_legacy_source() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(".aida/review-verdicts");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("PR-4273.json");
    let mut body = serde_json::json!({"verdict":"waiting", "reviewed_sha":HEAD, "summary":SUMMARY, "recorded_at":"2026-11-11T00:00:00Z"});
    std::fs::write(&path, body.to_string()).unwrap();
    let old = rv::current_verdict(tmp.path(), "PR-4273").unwrap();
    rv::close_terminal_wait(
        tmp.path(),
        "PR-4273",
        crate::gate::ledger::ChangeKey::pr(4273),
        &old,
    )
    .unwrap();
    assert!(rv::read_recorded_verdict(tmp.path(), "PR-4273")
        .unwrap()
        .is_closed());
    body["summary"] = "replacement round at the same timestamp".into();
    std::fs::write(&path, body.to_string()).unwrap();
    let current = rv::read_recorded_verdict(tmp.path(), "PR-4273").unwrap();
    assert!(!current.is_closed());
    assert_eq!(
        current.summary.as_deref(),
        Some("replacement round at the same timestamp")
    );
}

// trace:BUG-2525 | ai:codex
#[test]
fn bug_2525_raw_gate_projection_rejects_annotation_of_another_round() {
    use crate::gate::ledger::{self, project};
    let tmp = tempfile::tempdir().unwrap();
    let source = serde_json::json!({"verdict":"waiting", "reviewed_sha":HEAD, "summary":SUMMARY});
    let independent = serde_json::json!({"verdict":"request-changes", "reviewed_sha":HEAD, "findings":["independent defect"]});
    let annotation = serde_json::json!({"verdict":"waiting", "reviewed_sha":HEAD, "closed_by_merge":"PR-4273", "terminal_wait_key":"TASK-3448", "terminal_wait_source":source});
    let events = [&independent, &annotation]
        .into_iter()
        .map(|v| {
            ledger::event::verdict_event(
                tmp.path(),
                ledger::ChangeKey::pr(4273),
                v.as_object().unwrap(),
                None,
            )
        })
        .collect();
    let selected = project::newest_of(events).unwrap();
    assert_eq!(selected.verdict_record(), independent.as_object());
}

// trace:BUG-2525 | ai:codex
#[test]
fn bug_2525_closed_alias_wait_cannot_hide_active_alias_findings() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    let tmp = tempfile::tempdir().unwrap();
    rv::record_verdict(
        tmp.path(),
        "TASK-3448",
        Some("request-changes"),
        Some(HEAD),
        None,
        Some("code defect"),
        &["repair the code defect".into()],
        "reviewer",
    )
    .unwrap();
    rv::record_verdict(
        tmp.path(),
        "TASK-NATIVE-3448",
        Some("waiting"),
        Some(HEAD),
        None,
        Some(SUMMARY),
        &[],
        "reviewer",
    )
    .unwrap();
    let source = rv::current_verdict(tmp.path(), "TASK-NATIVE-3448").unwrap();
    rv::close_terminal_wait(
        tmp.path(),
        "TASK-NATIVE-3448",
        crate::gate::ledger::ChangeKey::pr(4273),
        &source,
    )
    .unwrap();
    let findings = resolve_rework_findings(tmp.path(), "TASK-3448", "TASK-NATIVE-3448", None)
        .unwrap()
        .unwrap();
    assert!(findings.contains("repair the code defect"));
}
