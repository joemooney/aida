//! Injected-time request/escalation lifecycle tests.
// trace:TASK-3074 | ai:codex
use super::*;
use aida_core::mailbox_lifecycle::action::{ActionReceipt, Outcome};
const MIN: i64 = 60_000;

fn active() -> Snapshot {
    Snapshot {
        lanes_running: Some(false),
        ready: Some(false),
        work_waiting: Some(true),
        ..Default::default()
    }
}
fn request() -> RequestIdentity {
    RequestIdentity {
        message_id: "exact-request".into(),
        fingerprint: "a".repeat(64),
    }
}
fn receipt(outcome: Outcome, at: i64, until: Option<i64>) -> ActionReceipt {
    ActionReceipt {
        message_id: request().message_id,
        fingerprint: request().fingerprint,
        withdrawn_fingerprint: None,
        actor: "advisor".into(),
        recipient_uuid: None,
        keys: vec!["advisor".into()],
        outcome,
        recorded_at: at * MIN,
        reason: "recipient decision".into(),
        evidence: Some("PR-42".into()),
        until: until.map(|v| v * MIN),
    }
}
fn step(
    root: &Path,
    snapshot: &Snapshot,
    minute: i64,
    execute: bool,
    receipt: Option<ActionReceipt>,
    notify: &mut dyn FnMut(&str, &str) -> Result<SendResult>,
) -> Report {
    tick_with_effects(
        root,
        snapshot,
        minute * MIN,
        execute,
        30,
        &mut |body| {
            assert!(body.contains("Episode "));
            Ok(SendResult::Persisted { request: request() })
        },
        &mut |identity| {
            assert_eq!(identity, &request());
            Ok(receipt.clone())
        },
        notify,
        &mut |p, b| Ok(aida_core::write_atomic(p, b)?),
    )
    .unwrap()
}
fn start(root: &Path) {
    std::fs::create_dir(root.join(".aida")).unwrap();
    for m in [0, 10, 20, 30] {
        step(root, &active(), m, true, None, &mut |_, _| {
            panic!("too early")
        });
    }
}

#[test]
fn unresolved_boundary_exact_identity_and_one_escalation() {
    let t = tempfile::tempdir().unwrap();
    start(t.path());
    let state: State = serde_json::from_slice(
        &std::fs::read(t.path().join(".aida/pipeline-stalls.json")).unwrap(),
    )
    .unwrap();
    let e = &state.episodes["pipeline-idle"];
    assert!(uuid::Uuid::parse_str(&e.uuid).is_ok());
    assert_eq!(e.request.as_ref(), Some(&request()));
    let mut notices = Vec::new();
    let mut notify = |rule: &str, body: &str| {
        notices.push((rule.to_owned(), body.to_owned()));
        Ok(SendResult::Sent)
    };
    for m in [40, 50, 59] {
        assert!(step(t.path(), &active(), m, true, None, &mut notify)
            .escalations
            .is_empty());
    }
    assert_eq!(
        step(t.path(), &active(), 60, true, None, &mut notify).escalations["pipeline-idle"],
        SendResult::Sent
    );
    for m in [70, 80, 90] {
        step(t.path(), &active(), m, true, None, &mut notify);
    }
    assert_eq!(notices.len(), 1);
    assert_eq!(
        notices[0].0,
        format!("pipeline_escalation_{}_unresolved", e.uuid)
    );
    assert!(notices[0].1.contains("unresolved request; action unknown"));
    assert!(notices[0].1.contains("exact-request"));
}

#[test]
fn acted_declined_and_latest_deferred_revision_control_deadline() {
    for outcome in [Outcome::Acted, Outcome::Declined, Outcome::Deferred] {
        let t = tempfile::tempdir().unwrap();
        start(t.path());
        let action = receipt(outcome, 35, (outcome == Outcome::Deferred).then_some(75));
        let deadline = if outcome == Outcome::Deferred { 75 } else { 65 };
        for m in [40, 50, 60, deadline - 1] {
            step(
                t.path(),
                &active(),
                m,
                true,
                Some(action.clone()),
                &mut |_, _| panic!("not due"),
            );
        }
        let r = step(
            t.path(),
            &active(),
            deadline,
            true,
            Some(action),
            &mut |_, body| {
                assert!(body.contains(if outcome == Outcome::Deferred {
                    "deferral deadline reached"
                } else {
                    "persistent stall"
                }));
                assert!(body.contains("recipient decision"));
                Ok(SendResult::Queued)
            },
        );
        assert_eq!(r.escalations["pipeline-idle"], SendResult::Queued);
        // A deferral notice does not consume the later persistent-stall notice.
        // Revising an already sent persistent notice does not resend it.
        let response_at = deadline + 1;
        for minute in [
            response_at,
            response_at + 10,
            response_at + 20,
            response_at + 29,
            response_at + 30,
            response_at + 40,
        ] {
            let report = step(
                t.path(),
                &active(),
                minute,
                true,
                Some(receipt(Outcome::Acted, response_at, None)),
                &mut |rule, body| {
                    assert_eq!(outcome, Outcome::Deferred);
                    assert_eq!(minute, response_at + 30);
                    assert!(rule.ends_with("_persistent"));
                    assert!(body.contains("persistent stall"));
                    Ok(SendResult::Sent)
                },
            );
            assert_eq!(
                report.escalations.len(),
                usize::from(outcome == Outcome::Deferred && minute == response_at + 30)
            );
        }
    }
}

#[test]
fn unknown_gap_reversal_and_recovery_do_not_fabricate_continuity() {
    for interruption in ["unknown", "gap", "reverse"] {
        let t = tempfile::tempdir().unwrap();
        start(t.path());
        step(t.path(), &active(), 40, true, None, &mut |_, _| panic!());
        let mut unknown = active();
        unknown.lanes_running = None;
        let resume = match interruption {
            "unknown" => {
                step(t.path(), &unknown, 45, true, None, &mut |_, _| panic!());
                50
            }
            "gap" => 60,
            _ => 39,
        };
        for m in [resume, resume + 10, resume + 20, resume + 29] {
            step(t.path(), &active(), m, true, None, &mut |_, _| {
                panic!("continuity restarted")
            });
        }
        assert_eq!(
            step(
                t.path(),
                &active(),
                resume + 30,
                true,
                None,
                &mut |_, _| Ok(SendResult::Sent)
            )
            .escalations
            .len(),
            1
        );
        let mut clear = active();
        clear.work_waiting = Some(false);
        step(
            t.path(),
            &clear,
            resume + 31,
            true,
            None,
            &mut |_, _| panic!(),
        );
        let state: State = serde_json::from_slice(
            &std::fs::read(t.path().join(".aida/pipeline-stalls.json")).unwrap(),
        )
        .unwrap();
        assert!(!state.episodes.contains_key("pipeline-idle"));
    }
}

#[test]
fn escalation_preview_refusal_retry_and_uncertain_dedup() {
    let t = tempfile::tempdir().unwrap();
    start(t.path());
    for m in [40, 50, 59] {
        step(t.path(), &active(), m, true, None, &mut |_, _| panic!());
    }
    let path = t.path().join(".aida/pipeline-stalls.json");
    let bytes = std::fs::read(&path).unwrap();
    let preview = step(t.path(), &active(), 60, false, None, &mut |_, _| {
        panic!("preview")
    });
    assert!(preview
        .evidence
        .iter()
        .any(|s| s.contains("escalation due")));
    assert_eq!(bytes, std::fs::read(&path).unwrap());
    assert_eq!(
        step(t.path(), &active(), 60, true, None, &mut |_, _| Ok(
            SendResult::Refused
        ))
        .escalations["pipeline-idle"],
        SendResult::Refused
    );
    assert_eq!(
        step(
            t.path(),
            &active(),
            61,
            true,
            None,
            &mut |_, _| anyhow::bail!("transport may have sent")
        )
        .escalations["pipeline-idle"],
        SendResult::Uncertain
    );
    let r = step(t.path(), &active(), 62, true, None, &mut |_, _| {
        panic!("uncertain is reserved")
    });
    assert!(r
        .unknown
        .iter()
        .any(|s| s.contains("escalation outcome unresolved")));
}

#[test]
fn escalation_pre_and_post_send_write_failure_preserve_reservation() {
    for fail_at in [1, 2] {
        let t = tempfile::tempdir().unwrap();
        start(t.path());
        for m in [40, 50, 59] {
            step(t.path(), &active(), m, true, None, &mut |_, _| panic!());
        }
        let mut writes = 0;
        let mut sent = 0;
        assert!(tick_with_effects(
            t.path(),
            &active(),
            60 * MIN,
            true,
            30,
            &mut |_| panic!("no second advisor mail"),
            &mut |_| Ok(None),
            &mut |_, _| {
                sent += 1;
                Ok(SendResult::Sent)
            },
            &mut |p, b| {
                writes += 1;
                if writes == fail_at {
                    anyhow::bail!("injected write failure")
                }
                Ok(aida_core::write_atomic(p, b)?)
            }
        )
        .is_err());
        assert_eq!(sent, usize::from(fail_at == 2));
        step(t.path(), &active(), 61, true, None, &mut |_, _| {
            assert_eq!(fail_at, 1);
            Ok(SendResult::Sent)
        });
    }
}

#[test]
fn initial_uncertainty_never_duplicates_advisor_and_escalates() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir(t.path().join(".aida")).unwrap();
    for m in [0, 10, 20, 30] {
        tick_with_effects(
            t.path(),
            &active(),
            m * MIN,
            true,
            30,
            &mut |_| Ok(SendResult::Uncertain),
            &mut |_| panic!("no persisted identity"),
            &mut |_, _| panic!("early"),
            &mut |p, b| Ok(aida_core::write_atomic(p, b)?),
        )
        .unwrap();
    }
    for m in [40, 50, 60] {
        tick_with_effects(
            t.path(),
            &active(),
            m * MIN,
            true,
            30,
            &mut |_| panic!("duplicate request"),
            &mut |_| panic!("no identity"),
            &mut |_, body| {
                assert_eq!(m, 60);
                assert!(body.contains("Request: unknown"));
                Ok(SendResult::Sent)
            },
            &mut |p, b| Ok(aida_core::write_atomic(p, b)?),
        )
        .unwrap();
    }
}

#[test]
fn latest_revision_postpones_then_resolves_response_without_clearing_stall() {
    let t = tempfile::tempdir().unwrap();
    start(t.path());
    for m in [40, 50] {
        step(t.path(), &active(), m, true, None, &mut |_, _| panic!());
    }
    for m in [59, 60, 70] {
        step(
            t.path(),
            &active(),
            m,
            true,
            Some(receipt(Outcome::Deferred, 59, Some(100))),
            &mut |_, _| panic!("deferred"),
        );
    }
    for m in [71, 80, 90, 100] {
        step(
            t.path(),
            &active(),
            m,
            true,
            Some(receipt(Outcome::Declined, 71, None)),
            &mut |_, _| panic!("new response deadline"),
        );
    }
    let report = step(
        t.path(),
        &active(),
        101,
        true,
        Some(receipt(Outcome::Declined, 71, None)),
        &mut |_, body| {
            assert!(body.contains("persistent stall; declined"));
            Ok(SendResult::Sent)
        },
    );
    assert_eq!(report.active, ["pipeline-idle"]);
    assert_eq!(report.escalations.len(), 1);
}

#[test]
fn initial_operator_queue_is_not_sent_or_a_second_advisor_request() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir(t.path().join(".aida")).unwrap();
    for m in [0, 10, 20] {
        tick(t.path(), &active(), m * MIN, true, 30, &mut |_| panic!()).unwrap();
    }
    let report = tick(t.path(), &active(), 30 * MIN, true, 30, &mut |_| {
        Ok(SendResult::Queued)
    })
    .unwrap();
    assert!(report.accepted.is_empty());
    assert_eq!(report.deliveries["pipeline-idle"], SendResult::Queued);
    tick(t.path(), &active(), 31 * MIN, true, 30, &mut |_| {
        panic!("queued is already accepted by transport")
    })
    .unwrap();
}

#[test]
fn legacy_preview_preserves_bytes_and_execute_reports_unknown_then_closes() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir(t.path().join(".aida")).unwrap();
    let path = t.path().join(".aida/pipeline-stalls.json");
    let bytes = br#"{"version":1,"episodes":{"pipeline-idle":{"first_ms":0,"observed_ms":0,"accepted":true}}}"#;
    std::fs::write(&path, bytes).unwrap();
    for execute in [false, true] {
        let r = step(t.path(), &active(), 1, execute, None, &mut |_, _| {
            panic!("legacy never escalates")
        });
        assert!(r
            .unknown
            .iter()
            .any(|s| s.contains("action unknown; legacy episode closed")));
        if !execute {
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }
    let state: State = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(state.episodes.is_empty());
}

#[test]
fn unreadable_action_evidence_escalates_as_unknown_without_asserting_action() {
    let t = tempfile::tempdir().unwrap();
    start(t.path());
    for m in [40, 50, 60] {
        let report = tick_with_effects(
            t.path(),
            &active(),
            m * MIN,
            true,
            30,
            &mut |_| panic!("duplicate advisor request"),
            &mut |_| anyhow::bail!("unsafe action ledger"),
            &mut |_, body| {
                assert_eq!(m, 60);
                assert!(body.contains("action unavailable"));
                assert!(!body.contains("persistent stall"));
                Ok(SendResult::Sent)
            },
            &mut |p, b| Ok(aida_core::write_atomic(p, b)?),
        )
        .unwrap();
        assert!(report
            .unknown
            .iter()
            .any(|s| s.contains("unsafe action ledger")));
    }
}

// trace:TASK-3074 | ai:codex
#[test]
fn unresolved_then_acted_gets_separate_persistent_escalation_once() {
    let t = tempfile::tempdir().unwrap();
    start(t.path());
    let mut notices = Vec::new();
    let mut notify = |rule: &str, body: &str| {
        notices.push((rule.to_owned(), body.to_owned()));
        Ok(SendResult::Sent)
    };
    for m in [40, 50, 60] {
        step(t.path(), &active(), m, true, None, &mut notify);
    }
    for m in [65, 75, 85, 94, 95, 105, 115, 125] {
        let report = step(
            t.path(),
            &active(),
            m,
            true,
            Some(receipt(Outcome::Acted, 65, None)),
            &mut notify,
        );
        assert_eq!(report.escalations.len(), usize::from(m == 95));
    }
    assert_eq!(notices.len(), 2);
    assert!(notices[0].0.ends_with("_unresolved"));
    assert!(notices[1].0.ends_with("_persistent"));
    assert_ne!(notices[0].0, notices[1].0);
    assert!(notices[1].1.contains("persistent stall; acted"));
    assert!(notices[1].1.contains("advisor"));
    assert!(notices[1].1.contains("recipient decision"));
    assert!(notices[1].1.contains("PR-42"));
}

#[test]
fn operator_direct_escalation_has_no_fictitious_request() {
    for delivery in [SendResult::Sent, SendResult::Queued] {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join(".aida")).unwrap();
        let mut notices = 0;
        for m in [0, 10, 20, 30, 40, 50, 60, 70, 80, 90] {
            tick_with_effects(
                t.path(),
                &active(),
                m * MIN,
                true,
                30,
                &mut |_| {
                    assert_eq!(m, 30);
                    Ok(delivery.clone())
                },
                &mut |_| panic!("no advisor request"),
                &mut |_, body| {
                    assert_eq!(m, 60);
                    assert!(body
                        .contains("stall persists; no advisor request (operator-direct delivery)"));
                    assert!(!body.contains("unresolved request"));
                    assert!(!body.contains("Request: unknown"));
                    notices += 1;
                    Ok(SendResult::Sent)
                },
                &mut |p, b| Ok(aida_core::write_atomic(p, b)?),
            )
            .unwrap();
        }
        assert_eq!(notices, 1);
    }
}

#[test]
fn invalid_state_names_file_and_recovery_remedy_without_mutation() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir(t.path().join(".aida")).unwrap();
    let path = t.path().join(".aida/pipeline-stalls.json");
    for bytes in ["invalid", r#"{"version":3,"episodes":{}}"#] {
        std::fs::write(&path, bytes).unwrap();
        let error = tick(t.path(), &active(), 0, true, 30, &mut |_| {
            panic!("invalid state sends")
        })
        .unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains(path.to_str().unwrap()));
        assert!(text.contains("inspect, then remove the file"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes);
    }
}

#[test]
fn initial_known_refusal_retries_but_publication_uncertainty_reserves() {
    for result in [SendResult::Refused, SendResult::Uncertain] {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join(".aida")).unwrap();
        for m in [0, 10, 20, 30] {
            tick(t.path(), &active(), m * MIN, true, 30, &mut |_| {
                if result == SendResult::Uncertain {
                    anyhow::bail!("transport may have published");
                }
                Ok(result.clone())
            })
            .unwrap();
        }
        let mut sends = 0;
        let report = tick(t.path(), &active(), 31 * MIN, true, 30, &mut |_| {
            sends += 1;
            Ok(SendResult::Sent)
        })
        .unwrap();
        assert_eq!(sends, usize::from(result == SendResult::Refused));
        if result == SendResult::Uncertain {
            assert!(report
                .unknown
                .iter()
                .any(|s| s.contains("outcome unresolved")));
        }
    }
}

// trace:TASK-b6b1278890774ef993d2ebc327862470-868 | ai:codex
#[test]
fn late_deferral_expiry_has_its_own_once_per_episode_notice() {
    let t = tempfile::tempdir().unwrap();
    start(t.path());
    let mut notices = Vec::new();
    let mut notify = |rule: &str, body: &str| {
        notices.push((rule.to_owned(), body.to_owned()));
        Ok(SendResult::Sent)
    };
    for m in [40, 50, 60] {
        step(t.path(), &active(), m, true, None, &mut notify);
    }
    let action = receipt(Outcome::Deferred, 65, Some(180));
    for m in (65..180).step_by(10).chain([179, 180, 181, 191]) {
        let report = step(
            t.path(),
            &active(),
            m,
            true,
            Some(action.clone()),
            &mut notify,
        );
        assert_eq!(report.escalations.len(), usize::from(m == 180));
    }
    // A later revised deferral still belongs to the same episode.
    for m in [200, 210, 220] {
        step(
            t.path(),
            &active(),
            m,
            true,
            Some(receipt(Outcome::Deferred, 192, Some(210))),
            &mut notify,
        );
    }
    // The independent persistent-stall notice remains available after action.
    for m in [221, 231, 241, 250, 251, 261] {
        let report = step(
            t.path(),
            &active(),
            m,
            true,
            Some(receipt(Outcome::Acted, 221, None)),
            &mut notify,
        );
        assert_eq!(report.escalations.len(), usize::from(m == 251));
    }
    assert_eq!(notices.len(), 3);
    assert!(notices[0].0.ends_with("_unresolved"));
    assert!(notices[1].0.ends_with("_deferral"));
    assert!(notices[2].0.ends_with("_persistent"));
    assert!(notices[1].1.contains("deferral deadline reached"));
    assert!(notices[1].1.contains("exact-request"));
}

// trace:TASK-b6b1278890774ef993d2ebc327862470-868 | ai:codex
#[test]
fn missing_deferral_deadline_is_reported_and_uses_fixed_fallback() {
    let t = tempfile::tempdir().unwrap();
    start(t.path());
    let action = receipt(Outcome::Deferred, 35, None);
    for m in [40, 50, 60, 64, 65, 75] {
        let report = step(
            t.path(),
            &active(),
            m,
            true,
            Some(action.clone()),
            &mut |rule, body| {
                assert_eq!(m, 65);
                assert!(rule.ends_with("_deferral"));
                assert!(body.contains("deferral deadline missing"));
                Ok(SendResult::Sent)
            },
        );
        assert!(report
            .unknown
            .iter()
            .any(|s| s.contains("deferred action missing deadline")));
        assert_eq!(report.escalations.len(), usize::from(m == 65));
    }
}

// trace:TASK-b6b1278890774ef993d2ebc327862470-868 | ai:codex
#[test]
fn deferral_preview_refusal_retry_and_uncertainty_preserve_dedup() {
    let t = tempfile::tempdir().unwrap();
    start(t.path());
    let action = receipt(Outcome::Deferred, 35, Some(60));
    for m in [40, 50, 59] {
        step(
            t.path(),
            &active(),
            m,
            true,
            Some(action.clone()),
            &mut |_, _| panic!("early"),
        );
    }
    let path = t.path().join(".aida/pipeline-stalls.json");
    let bytes = std::fs::read(&path).unwrap();
    let report = step(
        t.path(),
        &active(),
        60,
        false,
        Some(action.clone()),
        &mut |_, _| panic!("preview"),
    );
    assert!(report
        .evidence
        .iter()
        .any(|s| s.contains("deferral deadline reached")));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    for (m, outcome) in [(60, SendResult::Refused), (61, SendResult::Uncertain)] {
        let report = step(
            t.path(),
            &active(),
            m,
            true,
            Some(action.clone()),
            &mut |rule, _| {
                assert!(rule.ends_with("_deferral"));
                Ok(outcome.clone())
            },
        );
        assert_eq!(report.escalations["pipeline-idle"], outcome);
    }
    let report = step(t.path(), &active(), 62, true, Some(action), &mut |_, _| {
        panic!("reserved")
    });
    assert!(report.escalations.is_empty());
    assert!(report
        .unknown
        .iter()
        .any(|s| s.contains("escalation outcome unresolved")));
}

// trace:TASK-b6b1278890774ef993d2ebc327862470-868 | ai:codex
#[test]
fn deferral_pre_and_post_send_write_failures_preserve_reservation() {
    for fail_at in [1, 2] {
        let t = tempfile::tempdir().unwrap();
        start(t.path());
        let action = receipt(Outcome::Deferred, 35, Some(60));
        for m in [40, 50, 59] {
            step(
                t.path(),
                &active(),
                m,
                true,
                Some(action.clone()),
                &mut |_, _| panic!("early"),
            );
        }
        let mut writes = 0;
        let mut sent = 0;
        assert!(tick_with_effects(
            t.path(),
            &active(),
            60 * MIN,
            true,
            30,
            &mut |_| panic!("no second advisor mail"),
            &mut |_| Ok(Some(action.clone())),
            &mut |rule, _| {
                assert!(rule.ends_with("_deferral"));
                sent += 1;
                Ok(SendResult::Sent)
            },
            &mut |path, bytes| {
                writes += 1;
                if writes == fail_at {
                    anyhow::bail!("injected write failure");
                }
                Ok(aida_core::write_atomic(path, bytes)?)
            },
        )
        .is_err());
        assert_eq!(sent, usize::from(fail_at == 2));
        let report = step(t.path(), &active(), 61, true, Some(action), &mut |_, _| {
            assert_eq!(fail_at, 1);
            Ok(SendResult::Sent)
        });
        assert_eq!(report.escalations.len(), usize::from(fail_at == 1));
        if fail_at == 2 {
            assert!(report
                .unknown
                .iter()
                .any(|s| s.contains("escalation outcome unresolved")));
        }
    }
}
