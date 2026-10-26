//! STORY-1889: the dispatch admission seams `dispatch.rs` calls into the gate
//! for. The STORY-1917 dispatch suites drive them end to end; these pin each
//! decision on its own, inside the protected floor.
// trace:STORY-1889 | ai:claude

use super::*;
use crate::gate::dispatch_ledger::RecoveryFenceEntry;

fn t0() -> DateTime<Utc> {
    "2026-10-26T12:00:00Z".parse().unwrap()
}

fn request(id: &str, action: DispatchVerb) -> DispatchRequest {
    DispatchRequest {
        schema_version: DISPATCH_SCHEMA_VERSION,
        id: id.to_string(),
        requested_at: t0(),
        principal: "joe".into(),
        requester_seat: crate::gate::mandate::MANDATE_SEAT.into(),
        requester_grant_id: "g".repeat(32),
        matched_binding: vec!["key:abc".into()],
        mandate_id: "m-1".into(),
        dedupe_key: action.dedupe_key(),
        target_key: action.target_key(),
        action,
    }
}

fn running(req: &DispatchRequest, pid: Option<u32>, token: Option<&str>) -> DispatchResult {
    DispatchResult {
        id: req.id.clone(),
        verb: req.action.verb_str().into(),
        target: req.target_key.clone(),
        dedupe_key: req.dedupe_key.clone(),
        status: DispatchStatus::Running,
        requested_at: t0(),
        started_at: Some(t0()),
        finished_at: None,
        requester_grant_prefix: "gggggggg".into(),
        mandate_id: req.mandate_id.clone(),
        worker_pid: pid,
        worker_start: Some("linux-starttime:7".into()),
        claim_token: token.map(String::from),
        spawned_session: None,
        exit_code: None,
        summary: String::new(),
        refusal_reason: None,
        log_path: String::new(),
        notified_at: None,
    }
}

fn review(pr: u64) -> DispatchVerb {
    DispatchVerb::Review { pr, vendor: None }
}

fn take(pr: u64) -> DispatchVerb {
    DispatchVerb::Take {
        pr,
        branch: "b".into(),
        spec: "TASK-1".into(),
        brief_relpath: "briefs/x.md".into(),
        brief_sha256: "0".repeat(64),
        vendor: None,
    }
}

fn fence_on(target_key: &str) -> RecoveryFenceEntry {
    RecoveryFenceEntry {
        id: "00000000-0000-0000-0000-000000000009".into(),
        reason: "lost result".into(),
        evidence_fingerprint: "f".into(),
        detected_at: t0(),
        known_verb: Some("take".into()),
        known_target_key: Some(target_key.into()),
        known_dedupe_key: None,
        known_mandate_id: None,
        known_requested_at: None,
        blocks_all_admission: false,
    }
}

const ID: &str = "00000000-0000-0000-0000-000000000001";

#[test]
fn caps_hold_each_verb_to_its_limit_and_release_gives_a_slot_back() {
    let mut caps = ClaimCaps::from_inventory(&DispatchInventory::default(), None).unwrap();
    for _ in 0..MAX_CONCURRENT_REVIEWS {
        assert!(caps.try_take(&review(1)));
    }
    assert!(!caps.try_take(&review(1)), "review cap is full");
    caps.release(&review(1));
    assert!(caps.try_take(&review(1)));
    assert!((0..10).all(|_| caps.try_take(&take(1))), "take is uncapped");
    for _ in 0..MAX_CONCURRENT_TRAINS {
        assert!(caps.try_take(&DispatchVerb::Train { prs: vec![1] }));
    }
    assert!(!caps.try_take(&DispatchVerb::Train { prs: vec![1] }));
}

#[test]
fn a_pass_leaves_serve_verbs_fenced_targets_and_full_caps_pending() {
    let mut caps = ClaimCaps::from_inventory(&DispatchInventory::default(), None).unwrap();
    let inv = DispatchInventory::default();
    let rev = request(ID, review(5));
    assert_eq!(
        pass_admission(&inv, &rev, false, &mut caps),
        PassAdmission::WaitingForServe
    );
    let fenced = request(ID, take(6));
    let inv_fenced = DispatchInventory {
        fences: vec![fence_on(&fenced.target_key)],
        ..Default::default()
    };
    assert_eq!(
        pass_admission(&inv_fenced, &fenced, false, &mut caps),
        PassAdmission::Skip
    );
    for _ in 0..MAX_CONCURRENT_REVIEWS {
        assert!(matches!(
            pass_admission(&inv, &rev, true, &mut caps),
            PassAdmission::Claim(_)
        ));
    }
    assert_eq!(
        pass_admission(&inv, &rev, true, &mut caps),
        PassAdmission::Skip
    );
}

#[test]
fn a_direct_run_refuses_a_fenced_target_and_a_full_cap_by_name() {
    let rev = request(ID, review(5));
    let busy: Vec<_> = (0..MAX_CONCURRENT_REVIEWS)
        .map(|i| {
            let r = request(
                &format!("00000000-0000-0000-0000-00000000010{i}"),
                review(10 + i as u64),
            );
            let res = running(&r, Some(1), Some("t"));
            (r, res)
        })
        .collect();
    let inv = DispatchInventory {
        pairs: busy,
        ..Default::default()
    };
    let err = run_admission(&inv, ID, &Ok(rev.clone())).unwrap_err();
    assert_eq!(
        err.to_string(),
        format!("cannot run dispatch request {ID}: review concurrency cap ({MAX_CONCURRENT_REVIEWS}) reached")
    );
    let inv = DispatchInventory {
        fences: vec![fence_on(&rev.target_key)],
        ..Default::default()
    };
    let err = run_admission(&inv, ID, &Ok(rev)).unwrap_err().to_string();
    assert!(err.contains("is reserved by dispatch record"), "{err}");
    // An unreadable request is refused later, by the tamper check.
    assert!(run_admission(&inv, ID, &Err(anyhow::anyhow!("bad"))).is_ok());
}

#[test]
fn a_running_claim_belongs_only_to_its_token_and_worker() {
    let req = request(ID, take(3));
    let start = Some("linux-starttime:7".to_string());
    let res = running(&req, Some(42), None);
    let err = verify_running_claim(ID, &res, Some("t"), 42, &start).unwrap_err();
    assert!(err.to_string().contains("without a claim token"), "{err}");

    let res = running(&req, Some(42), Some("t"));
    let err = verify_running_claim(ID, &res, Some("other"), 42, &start).unwrap_err();
    assert!(err.to_string().contains("claim token mismatch"), "{err}");

    assert_eq!(
        verify_running_claim(ID, &res, Some("t"), 42, &start).unwrap(),
        RunningClaim::Owned(WorkerClaim {
            worker_pid: 42,
            worker_start: start.clone(),
            claim_token: "t".into(),
        })
    );

    let unpublished = running(&req, None, Some("t"));
    match verify_running_claim(ID, &unpublished, Some("t"), 42, &start).unwrap() {
        RunningClaim::Unpublished { reason, refusal } => {
            assert!(reason.contains("worker_pid is None"), "{reason}");
            assert!(refusal.ends_with(&reason), "{refusal}");
        }
        other => panic!("expected Unpublished, got {other:?}"),
    }

    crate::gate::dispatch_authority::TEST_FORCE_WORKER_PROBE
        .with(|c| *c.borrow_mut() = Some(aida_core::process_probe::Probe::Dead("gone".into())));
    let foreign = verify_running_claim(ID, &res, Some("t"), 43, &start).unwrap();
    crate::gate::dispatch_authority::TEST_FORCE_WORKER_PROBE.with(|c| *c.borrow_mut() = None);
    match foreign {
        RunningClaim::Foreign { dead, refusal } => {
            assert!(dead);
            assert!(refusal.contains("refusing duplicate or replacement execution"));
        }
        other => panic!("expected Foreign, got {other:?}"),
    }
}

#[test]
fn boundary_b_refuses_serve_verbs_under_tick_before_any_other_check() {
    let tmp = tempfile::tempdir().unwrap();
    let req = request(ID, review(5));
    let refusal = admit_execution(tmp.path(), tmp.path(), &req, false, t0()).unwrap_err();
    assert_eq!(
        refusal,
        ExecutionRefusal {
            reason: "verb `review` requires an advisor seat grant and can only run under `aida dispatch serve` (refused by `dispatch tick`)".into(),
            outcome: Outcome::Refused,
        }
    );
}

#[test]
fn the_admitted_record_takes_its_authority_from_the_checks() {
    let mandate = Mandate {
        id: "m-7".into(),
        principal: "joe".into(),
        seat: crate::gate::mandate::MANDATE_SEAT.into(),
        scope: crate::gate::mandate::SCOPE_SHIP_HELD.into(),
        include_supervised: false,
        reason: None,
        issued_at: t0(),
        expires_at: t0(),
        revoked_at: None,
        revoked_by: None,
        scopes: Default::default(),
        kind: None,
    };
    let caller = AuthorizedDispatchCaller {
        mandate,
        grant_id: "grant-1".into(),
        matched_binding: vec!["key:k".into()],
    };
    let action = take(9);
    let req = caller.admitted_request(
        ID.into(),
        t0(),
        "  joe ",
        action.dedupe_key(),
        action.target_key(),
        action,
    );
    assert_eq!(req.schema_version, DISPATCH_SCHEMA_VERSION);
    assert_eq!(req.principal, "joe");
    assert_eq!(req.requester_seat, crate::gate::mandate::MANDATE_SEAT);
    assert_eq!(req.requester_grant_id, "grant-1");
    assert_eq!(req.matched_binding, vec!["key:k".to_string()]);
    assert_eq!(req.mandate_id, "m-7");
}

#[test]
fn only_success_is_green_ci() {
    use crate::forge::CiState;
    assert!(ci_state_is_green(1, CiState::Success).is_ok());
    assert_eq!(
        ci_state_is_green(1, CiState::None).unwrap_err().to_string(),
        "PR #1 has no CI checks; flow reports green only after CI passes"
    );
    assert_eq!(
        ci_state_is_green(1, CiState::Failed)
            .unwrap_err()
            .to_string(),
        "CI failed for PR #1"
    );
}

// The reaper releases only positively dead `Running` claims; `Unknown` and
// `Alive` claims keep their slot (TASK-2344 tri-state fence).
// trace:STORY-1889 trace:TASK-2344 | ai:claude
#[test]
fn only_positively_dead_running_claims_are_released() {
    let gone = u32::MAX - 1;
    let me = std::process::id();
    let my_start = crate::process_probe::process_start_identity(me);
    let dead = request("00000000-0000-0000-0000-00000000000a", take(1));
    let unknown = request("00000000-0000-0000-0000-00000000000b", take(2));
    let alive = request("00000000-0000-0000-0000-00000000000c", take(3));
    let done = request("00000000-0000-0000-0000-00000000000d", take(4));
    let mut alive_res = running(&alive, Some(me), Some("t"));
    alive_res.worker_start = my_start;
    let mut done_res = running(&done, Some(gone), Some("t"));
    done_res.status = DispatchStatus::Done;
    let inv = DispatchInventory {
        pairs: vec![
            (dead.clone(), running(&dead, Some(gone), Some("t"))),
            (unknown.clone(), running(&unknown, None, Some("t"))),
            (alive, alive_res),
            (done, done_res),
        ],
        ..Default::default()
    };
    let released = dead_claims(&inv);
    assert_eq!(released.len(), 1, "{released:?}");
    assert_eq!(released[0].0, dead.id);
    assert!(
        released[0]
            .1
            .contains("exited unexpectedly without recording completion"),
        "{released:?}"
    );
}

// trace:STORY-1889 | ai:claude
#[test]
fn only_an_approved_verdict_lets_flow_ship() {
    assert_eq!(
        flow_ship_decision(Some(VerdictKind::Approved)),
        FlowVerdict::Ship
    );
    for kind in [VerdictKind::RequestChanges, VerdictKind::Rejected] {
        assert_eq!(
            flow_ship_decision(Some(kind)),
            FlowVerdict::ChangesRequested
        );
    }
    assert_eq!(flow_ship_decision(None), FlowVerdict::NoVerdict);
    assert_eq!(
        flow_ship_decision(Some(VerdictKind::default())),
        FlowVerdict::NoVerdict
    );
}

fn with_status(mut res: DispatchResult, status: DispatchStatus) -> DispatchResult {
    res.status = status;
    res
}

fn claim_admission() -> ClaimAdmission {
    run_admission(&DispatchInventory::default(), ID, &Ok(request(ID, take(3)))).unwrap()
}

// trace:STORY-1889 | ai:claude
#[test]
fn only_a_pending_request_cancels() {
    let req = request(ID, take(3));
    let mut pending = with_status(running(&req, None, None), DispatchStatus::Pending);
    pending.cancel(&req, t0()).unwrap();
    assert_eq!(pending.status(), DispatchStatus::Cancelled);
    assert_eq!(pending.finished_at(), Some(t0()));
    assert_eq!(pending.summary(), "cancelled take PR #3");
    let mut busy = running(&req, Some(1), Some("t"));
    let err = busy.cancel(&req, t0()).unwrap_err().to_string();
    assert!(
        err.contains("only `pending` requests can be cancelled"),
        "{err}"
    );
    assert_eq!(busy.status(), DispatchStatus::Running);
}

// trace:STORY-1889 | ai:claude
#[test]
fn a_claim_starts_only_from_pending_and_ends_only_from_running() {
    let req = request(ID, take(3));
    let mut res = with_status(running(&req, None, None), DispatchStatus::Pending);
    res.begin_running(
        claim_admission(),
        Some(&req),
        Some((9, None)),
        "t".into(),
        t0(),
    )
    .unwrap();
    assert_eq!(res.status(), DispatchStatus::Running);
    assert_eq!(res.started_at(), Some(t0()));
    assert!(res.claim_is(9, None, Some("t")));
    assert!(res
        .begin_running(claim_admission(), None, None, "u".into(), t0())
        .is_err());

    assert!(res
        .finish(DispatchStatus::Pending, "x".into(), None, None, None, t0())
        .is_err());
    res.finish(DispatchStatus::Done, "ok".into(), None, None, Some(0), t0())
        .unwrap();
    assert_eq!(res.status(), DispatchStatus::Done);
    assert!(res
        .finish(
            DispatchStatus::Failed,
            "again".into(),
            None,
            None,
            None,
            t0()
        )
        .is_err());
    assert!(res
        .fail_running("x".into(), "x".into(), true, t0())
        .is_err());
    assert!(res
        .execution_entry(ID)
        .unwrap_err()
        .to_string()
        .contains("terminal state `done`"));
}

// trace:STORY-1889 | ai:claude
#[test]
fn a_spawn_failure_fails_the_claim_and_releases_it() {
    let req = request(ID, take(3));
    let mut res = running(&req, Some(1), Some("t"));
    res.fail_running("boom".into(), "boom".into(), true, t0())
        .unwrap();
    assert_eq!(res.status(), DispatchStatus::Failed);
    assert!(res.is_unclaimed());
    assert_eq!(res.refusal_reason.as_deref(), Some("boom"));
}

// trace:STORY-1889 | ai:claude
#[test]
fn grant_expiry_reverts_only_this_process_claim() {
    let req = request(ID, take(3));
    let mut res = running(&req, Some(42), Some("t"));
    let start = Some("linux-starttime:7");
    assert!(!res.revert_unstarted_claim(42, start, Some("other")));
    assert!(!res.revert_unstarted_claim(43, start, Some("t")));
    assert_eq!(res.status(), DispatchStatus::Running);
    assert!(res.revert_unstarted_claim(42, start, Some("t")));
    assert_eq!(res.status(), DispatchStatus::Pending);
    assert_eq!(res.started_at(), None);
    assert!(res.is_unclaimed());
    assert!(!res.revert_unstarted_claim(42, start, Some("t")));
}

// trace:STORY-1889 | ai:claude
#[test]
fn an_active_duplicate_dedupes_and_an_active_target_refuses() {
    let req = request(ID, take(3));
    let done = with_status(running(&req, Some(1), Some("t")), DispatchStatus::Done);
    let inv = DispatchInventory {
        pairs: vec![(req.clone(), done)],
        ..Default::default()
    };
    assert_eq!(
        submission_collision(&inv, &req.dedupe_key, &req.target_key),
        None,
        "a finished request never collides"
    );
    let active = running(&req, Some(1), Some("t"));
    let inv = DispatchInventory {
        pairs: vec![(req.clone(), active.clone())],
        ..Default::default()
    };
    assert_eq!(
        submission_collision(&inv, &req.dedupe_key, &req.target_key),
        Some(SubmissionCollision::Duplicate {
            request: req.clone(),
            result: active,
        })
    );
    match submission_collision(&inv, "other", &req.target_key) {
        Some(SubmissionCollision::TargetBusy(msg)) => {
            assert!(
                msg.contains("already has an active dispatch request"),
                "{msg}"
            );
            assert!(msg.contains("status: running"), "{msg}");
        }
        other => panic!("expected TargetBusy, got {other:?}"),
    }
    assert_eq!(submission_collision(&inv, "other", "elsewhere"), None);
}
