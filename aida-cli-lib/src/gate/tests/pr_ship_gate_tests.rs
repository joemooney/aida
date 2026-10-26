// trace:TASK-1606 | ai:codex
#[test]
fn ship_exit_codes_survive_error_context() {
    use super::ShipRefusal::*;
    for (reason, code) in [
        (CiRed, 20),
        (CiTimeout, 21),
        (NotMergeable, 22),
        (Review, 23),
        (StaleDefinition, 24),
        (MergeHold, 25),
        (BehindBase, 26),
    ] {
        let error = reason.error("refused").context("ship failed");
        assert_eq!(crate::exit_code_for_error(&error), code);
    }
    assert_eq!(
        crate::exit_code_for_error(&anyhow::anyhow!("unclassified")),
        1
    );
}

// BUG-1566: pr ship's hold release obeys the integrity floor exactly like
// `merge-hold clear` — no hold → no gate; hold + human → release; hold
// without a human → refuse, naming the human release path.
#[test]
fn ship_hold_release_requires_the_integrity_floor() {
    assert_eq!(ship_hold_release_refusal(false, false, 7), None);
    assert_eq!(ship_hold_release_refusal(false, true, 7), None);
    assert_eq!(ship_hold_release_refusal(true, true, 7), None);
    let msg = ship_hold_release_refusal(true, false, 7).expect("must refuse");
    assert!(msg.contains("aida merge-hold clear 7"), "{msg}");
    assert!(msg.contains("human"), "{msg}");
}

// BUG-1566: the real ship path must consult the floor BEFORE it clears
// the marker or drops the label — never after, never not at all.
#[test]
fn ship_release_site_gates_on_the_floor_before_clearing() {
    let src = include_str!("../../pr_cmd.rs");
    // STORY-1698: the release step's gate is the last call site (the
    // preflight's takes the authority the handler decided).
    let gate = src
        .rfind("ship::ship_hold_release_refusal_for(")
        .expect("pr ship must gate the hold release");
    // STORY-1698: the authority is the human floor, or an advisor an
    // active fire-fighting mandate covers (re-read under the merge lease).
    // trace:STORY-1698 | ai:claude
    assert!(src.contains(
        "let floor_authority = crate::gate::role_authority::has_integrity_floor_authority();"
    ));
    let floor = src[gate..]
        .find("floor_authority || mandate_release.is_some()")
        .expect("the gate must be fed the integrity-floor (or mandate) authority");
    assert!(floor < 400, "authority must be the gate's own argument");
    let clear = src
        .find("crate::gate::merge_hold_core::clear_hold(&hold_root, pr_number)")
        .expect("release site present");
    let unlabel = src
        .find("crate::merge_hold::sync_label(&hold_root, pr_number, false)")
        .expect("label drop present");
    assert!(
        gate < clear && gate < unlabel,
        "gate must precede the release"
    );
}

use super::*;

// trace:TASK-1602 | ai:codex
fn drive_fixture() -> (tempfile::TempDir, crate::drain_state::DrainState) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida/headless-logs")).unwrap();
    std::fs::write(
        crate::drain_lock::drain_lock_path(dir.path()),
        serde_json::json!({
            "pid": std::process::id(), "started_at_utc": chrono::Utc::now().to_rfc3339(),
            "command": "aida queue work --auto-complete", "host": "test", "wave_id": "wave-test"
        })
        .to_string(),
    )
    .unwrap();
    let mut state = crate::drain_state::DrainState::new_single("TASK-1602", "run-test", false);
    state.members[0].state = "in-phase-3".into();
    state.members[0].pr = Some(1602);
    state.current_phase = Some("3 (reviewer)".into());
    state.current_session_id = Some("session-test".into());
    state.current_vendor = Some("codex".into());
    std::fs::write(
        dir.path()
            .join(".aida/headless-logs/task-1602-session-test.jsonl"),
        "{}\n",
    )
    .unwrap();
    state.write(dir.path()).unwrap();
    (dir, state)
}

#[test]
fn ship_drive_diagnostics_are_bound_to_owning_member() {
    let (dir, mut state) = drive_fixture();
    let probe = || probe_ship_drive_guard(dir.path(), 1602, None, false, false).unwrap();
    let block = probe();
    assert_eq!(block.reason, ShipMergeBlockReason::DriveOwnedPr);
    assert!(block.detail.contains("wave-test"));
    assert!(block.detail.contains("run unavailable"));
    assert!(!block.detail.contains("run-test"));
    assert!(block.detail.contains("3/6 (reviewer)"));
    assert!(
        block.detail.contains("activity last written"),
        "{}",
        block.detail
    );
    state.current = Some("TASK-999".into());
    state.current_phase = Some("1 (implementer)".into());
    state.write(dir.path()).unwrap();
    let block = probe();
    assert!(block.detail.contains("3/6 (reviewer)"));
    assert!(!block.detail.contains("activity last written"));
    assert!(!block.detail.contains("run-test"));
    assert!(probe_ship_drive_guard(dir.path(), 999, None, false, false).is_none());
    assert_eq!(
        probe_ship_drive_guard(dir.path(), 999, Some("TASK-1602"), false, false)
            .unwrap()
            .reason,
        ShipMergeBlockReason::DriveSeat
    );
    assert!(probe_ship_drive_guard(dir.path(), 1602, None, true, true).is_none());
    std::fs::remove_file(crate::drain_lock::drain_lock_path(dir.path())).unwrap();
    assert!(probe_ship_drive_guard(dir.path(), 1602, None, false, false).is_none());
}

// trace:TASK-1602 | ai:codex
#[test]
fn ship_run_identity_is_unavailable_after_overlapping_sibling_phase() {
    let (dir, _) = drive_fixture();
    crate::drain_state::set_run(dir.path(), "TASK-999", "sibling-run-B", false);
    crate::drain_state::set_phase_with_tuning(
        dir.path(),
        "TASK-1602",
        3,
        "reviewer",
        None,
        None,
        None,
        None,
        Some(1602),
    );
    let state = crate::drain_state::DrainState::read(dir.path()).unwrap();
    assert_eq!(state.current.as_deref(), Some("TASK-1602"));
    assert_eq!(state.run_uuid, "sibling-run-B");
    let block = probe_ship_drive_guard(dir.path(), 1602, None, false, false).unwrap();
    assert!(block.detail.contains("TASK-1602 phase 3/6 (reviewer)"));
    assert!(block.detail.contains("run unavailable"));
    assert!(!block.detail.contains("sibling-run-B"));
}

#[test]
fn ship_wait_reprobes_and_continues_only_after_release() {
    let (dir, mut state) = drive_fixture();
    let mut polls = 0;
    let result = wait_for_drive_release(
        Some(std::time::Duration::from_secs(1)),
        std::time::Duration::ZERO,
        || {
            polls += 1;
            probe_ship_drive_guard(dir.path(), 1602, None, false, false)
        },
        |_, _| {
            state.members[0].state = "completed".into();
            state.write(dir.path()).unwrap();
        },
    );
    assert!(result.is_ok());
    assert_eq!(polls, 2);
}

#[test]
fn ship_wait_timeout_and_headless_seat_fail_closed() {
    let (dir, _) = drive_fixture();
    let block = wait_for_drive_release(
        Some(std::time::Duration::ZERO),
        std::time::Duration::ZERO,
        || probe_ship_drive_guard(dir.path(), 1602, None, false, false),
        |_, _| panic!("zero budget cannot wait"),
    )
    .unwrap_err();
    assert!(block.timed_out);
    let block = wait_for_drive_release(
        Some(std::time::Duration::from_secs(300)),
        std::time::Duration::ZERO,
        || probe_ship_drive_guard(dir.path(), 1602, None, true, false),
        |_, _| panic!("headless seats cannot wait"),
    )
    .unwrap_err();
    assert_eq!(block.reason, ShipMergeBlockReason::DriveSeat);
    assert!(!block.timed_out);
    assert!(wait_for_drive_release(
        None,
        std::time::Duration::ZERO,
        || probe_ship_drive_guard(dir.path(), 1602, None, false, false),
        |_, _| panic!("no implicit wait")
    )
    .is_err());
}

// BUG-1499: a label-only hold is refused even for a HUMAN at a terminal
// (no clearance record would exist); a marker hold keeps the BUG-1566
// floor behaviour; no hold → no gate.
// trace:BUG-1499 | ai:claude
#[test]
fn label_only_hold_is_refused_even_for_a_human() {
    let msg = ship_hold_release_refusal_for(false, true, true, 7).expect("human refused");
    assert!(msg.contains("aida merge-hold clear 7"), "{msg}");
    assert!(msg.contains("label alone"), "{msg}");
    assert!(ship_hold_release_refusal_for(false, true, false, 7).is_some());
    assert_eq!(ship_hold_release_refusal_for(true, false, true, 7), None);
    assert_eq!(ship_hold_release_refusal_for(true, true, true, 7), None);
    assert!(ship_hold_release_refusal_for(true, false, false, 7).is_some());
    assert_eq!(ship_hold_release_refusal_for(false, false, false, 7), None);
}

// BUG-1532: a hold binds `pr ship` with NO drive running — the gate takes
// no drive input at all. Refusal holds refuse even a human; supervision
// holds still release for a human (BUG-1167); nothing releases headless.
// trace:BUG-1532 | ai:claude
#[test]
fn any_hold_binds_pr_ship_without_a_live_drive() {
    use crate::gate::merge_hold_core::{typed_hold, HoldReasonKind, RefusalRelease, VerdictRef};
    let held = |_: &crate::gate::merge_hold_core::MergeHoldRecord| {
        RefusalRelease::Held(
            "the verdict on record is CHANGES REQUESTED for BUG-1 at 3acf3671fd7a".into(),
        )
    };
    let never = |_: &crate::gate::merge_hold_core::MergeHoldRecord| -> RefusalRelease {
        panic!("release_of must only be consulted for a refusal hold with a human present")
    };
    assert_eq!(ship_hold_gate(None, false, 9, never), None);
    assert_eq!(ship_hold_gate(None, true, 9, never), None);

    let rework = typed_hold(
        9,
        HoldReasonKind::Rework,
        "CHANGES REQUESTED for BUG-1 at 3acf3671fd7a",
        None,
    );
    let msg = ship_hold_gate(Some(&rework), true, 9, held).expect("refusal refuses a human");
    assert!(msg.contains("rework hold"), "{msg}");
    assert!(msg.contains("CHANGES REQUESTED for BUG-1"), "{msg}");
    assert!(msg.contains("aida merge-hold clear 9"), "{msg}");
    assert!(msg.contains("fresh APPROVED"), "{msg}");
    assert!(msg.contains("at 3acf3671fd7a"), "{msg}");
    assert!(ship_hold_gate(Some(&rework), false, 9, never).is_some());
    // Criterion 4: a fresh APPROVED verdict at the head releases a
    // refusal for a human — and never for a headless seat.
    let fresh = |_: &crate::gate::merge_hold_core::MergeHoldRecord| {
        RefusalRelease::Released(VerdictRef::new(
            "BUG-1",
            Some(9),
            Some("cd21a1dc0a9e".into()),
            None,
        ))
    };
    assert_eq!(ship_hold_gate(Some(&rework), true, 9, fresh), None);
    assert!(ship_hold_gate(Some(&rework), false, 9, fresh).is_some());

    let mut recusal = typed_hold(9, HoldReasonKind::Recusal, "author", Some("a".into()));
    recusal.recused_principals = vec!["agent:author".into()];
    assert!(ship_hold_gate(Some(&recusal), true, 9, never).is_some());
    let unknown = typed_hold(9, HoldReasonKind::Unknown, "malformed", None);
    assert!(ship_hold_gate(Some(&unknown), true, 9, never).is_some());

    for kind in [HoldReasonKind::Supervision, HoldReasonKind::Decision] {
        let supervised = typed_hold(9, kind, "STORY-1 is marked drive", None);
        assert_eq!(
            ship_hold_gate(Some(&supervised), true, 9, never),
            None,
            "a human's explicit ship IS the decision a {kind:?} hold awaits"
        );
        let headless = ship_hold_gate(Some(&supervised), false, 9, never).expect("floor");
        assert!(headless.contains("human"), "{headless}");
    }

    let dir = tempfile::tempdir().unwrap();
    crate::gate::merge_hold_core::write_hold(dir.path(), 9, "STORY-1 is marked drive").unwrap();
    let legacy = crate::gate::merge_hold_core::read_hold_record(dir.path(), 9).unwrap();
    let msg = ship_hold_gate(Some(&legacy), true, 9, held).expect("untyped is not permission");
    assert!(msg.contains("legacy"), "{msg}");
}

// BUG-1532: a verdict-released hold merges PINNED to the head the verdict
// covers, even when the TASK-1448 pin is None (the verdict's key was not a
// candidate); no readable head → refused. Otherwise the pin is untouched.
// trace:BUG-1532 | ai:claude
#[test]
fn verdict_released_hold_always_merges_pinned_to_the_head() {
    assert_eq!(
        refusal_release_pin(true, None, Some("cd21a1dc0a9e"), 7),
        Ok(Some("cd21a1dc0a9e".to_string()))
    );
    assert_eq!(
        refusal_release_pin(true, Some("old".into()), Some("cd21a1dc0a9e"), 7),
        Ok(Some("cd21a1dc0a9e".to_string()))
    );
    let err = refusal_release_pin(true, None, None, 7).unwrap_err();
    assert!(err.contains("hold stays"), "{err}");
    assert!(refusal_release_pin(true, None, Some("  "), 7).is_err());
    assert_eq!(refusal_release_pin(false, None, None, 7), Ok(None));
    assert_eq!(
        refusal_release_pin(false, Some("abc".into()), Some("def"), 7),
        Ok(Some("abc".to_string()))
    );
    // Wiring: the pin is applied to merge_opts BEFORE the hold is cleared.
    let src = include_str!("../../pr_cmd.rs");
    let pin = src
        .find("ship::refusal_release_pin(")
        .expect("ship applies the release pin");
    let assign = src
        .find("Ok(pin) => merge_opts.match_head = pin")
        .expect("pin assigned to the merge");
    let clear = src
        .find("crate::gate::merge_hold_core::clear_hold(&hold_root, pr_number)")
        .expect("release site");
    let merge = src[clear..]
        .find("&merge_opts,")
        .map(|i| i + clear)
        .expect("merge call");
    assert!(pin < assign && assign < clear && clear < merge);
}

// BUG-1532: the real ship path consults the hold gate BEFORE the CI watch
// (so a held PR's red gate is never discounted) and the gate is fed the
// marker itself, not a drive-liveness or reason-substring predicate.
#[test]
fn ship_hold_gate_runs_before_ci_and_ignores_drive_liveness() {
    let src = include_str!("../../pr_cmd.rs");
    let gate = src
        .find("ship::ship_hold_gate(")
        .expect("pr ship must consult the hold gate");
    let args = &src[gate..gate + 300];
    assert!(args.contains("read_hold_record(&main_worktree, pr_number)"));
    // STORY-1698: the human floor, or an active mandate's cover.
    // trace:STORY-1698 | ai:claude
    assert!(args.contains("floor_authority || mandate_cover.is_ok()"));
    assert!(src.contains(
        "let floor_authority = crate::gate::role_authority::has_integrity_floor_authority();"
    ));
    assert!(!args.contains("drive_specs"));
    // Ship's single deadline now owns registration and settlement.
    // trace:TASK-1606 | ai:codex
    let ci = src
        .find("crate::ci_gate::wait_for_ship_ci(")
        .expect("ci wait present");
    let clear = src
        .find("crate::gate::merge_hold_core::clear_hold(&hold_root, pr_number)")
        .expect("release site present");
    assert!(
        gate < ci && gate < clear,
        "hold gate must precede CI + release"
    );
}

// ── TASK-1448: approval-covers-head merge gate ─────────────────────────
// trace:TASK-1448 | ai:claude
const HEAD: &str = "1aca4e3e9251aaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OLD: &str = "08834c6045a9bbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn verdict(
    kind: &str,
    sha: Option<&str>,
    at: Option<&str>,
) -> crate::gate::review_verdict::RecordedVerdict {
    crate::gate::review_verdict::RecordedVerdict {
        kind: crate::gate::review_verdict::VerdictKind::parse(kind),
        raw: kind.to_string(),
        reviewed_sha: sha.map(str::to_string),
        recorded_at: at.map(str::to_string),
        ..Default::default()
    }
}

#[test]
fn task_1448_approval_at_head_proceeds() {
    let v = [verdict(
        "approved",
        Some(HEAD),
        Some("2026-09-21T04:00:00Z"),
    )];
    assert_eq!(approval_head_refusal(&v, Some(HEAD)), None);
    // An abbreviated approval sha matching the head's prefix is the same commit.
    let short = [verdict("approved", Some(&HEAD[..12]), None)];
    assert_eq!(approval_head_refusal(&short, Some(HEAD)), None);
}

#[test]
fn task_1448_approval_behind_head_refuses_naming_both_shas() {
    let v = [verdict("approved", Some(OLD), Some("2026-09-21T04:00:00Z"))];
    let refusal = approval_head_refusal(&v, Some(HEAD)).expect("must refuse");
    assert_eq!(
        refusal,
        ApprovalHeadRefusal::Moved {
            reviewed_sha: OLD.to_string(),
            head_sha: HEAD.to_string()
        }
    );
    let msg = approval_head_refusal_message(2048, &refusal);
    assert!(msg.contains("PR-2048"), "{msg}");
    assert!(
        msg.contains(&OLD[..12]) && msg.contains(&HEAD[..12]),
        "{msg}"
    );
    assert!(msg.contains("Re-review the current head"), "{msg}");
}

#[test]
fn task_1448_approval_without_sha_refuses() {
    let v = [verdict("approved", None, Some("2026-09-21T04:00:00Z"))];
    let refusal = approval_head_refusal(&v, Some(HEAD)).expect("must refuse");
    assert_eq!(
        refusal,
        ApprovalHeadRefusal::NoReviewedSha {
            head_sha: HEAD.to_string()
        }
    );
    // A blank sha is the same as none.
    let blank = [verdict("approved", Some("  "), None)];
    assert!(matches!(
        approval_head_refusal(&blank, Some(HEAD)),
        Some(ApprovalHeadRefusal::NoReviewedSha { .. })
    ));
    let msg = approval_head_refusal_message(7, &refusal);
    assert!(msg.contains("records no reviewed commit"), "{msg}");
    assert!(msg.contains("Re-review the current head"), "{msg}");
}

#[test]
fn task_1448_head_unreadable_refuses() {
    let v = [verdict("approved", Some(HEAD), None)];
    for head in [None, Some(""), Some("   ")] {
        let refusal = approval_head_refusal(&v, head).expect("must refuse");
        assert_eq!(
            refusal,
            ApprovalHeadRefusal::HeadUnreadable {
                reviewed_sha: HEAD.to_string()
            }
        );
    }
    let msg = approval_head_refusal_message(
        7,
        &ApprovalHeadRefusal::HeadUnreadable {
            reviewed_sha: HEAD.to_string(),
        },
    );
    assert!(msg.contains("could not be read"), "{msg}");
}

#[test]
fn task_1448_incomparable_sha_refuses() {
    let v = [verdict("approved", Some("1ac"), None)];
    assert_eq!(
        approval_head_refusal(&v, Some(HEAD)),
        Some(ApprovalHeadRefusal::Incomparable {
            reviewed_sha: "1ac".to_string(),
            head_sha: HEAD.to_string()
        })
    );
}

#[test]
fn task_1448_no_approval_is_not_this_gates_business() {
    assert_eq!(approval_head_refusal(&[], Some(HEAD)), None);
    assert_eq!(approval_head_refusal(&[], None), None);
    // A refusal alone is gated elsewhere; this gate classifies approvals only.
    let refusal_only = [verdict("request-changes", Some(OLD), None)];
    assert_eq!(approval_head_refusal(&refusal_only, Some(HEAD)), None);
}

#[test]
fn task_1448_refusal_does_not_shadow_a_stale_approval() {
    let v = [
        verdict("request-changes", Some(HEAD), Some("2026-09-21T05:00:00Z")),
        verdict("approved", Some(OLD), Some("2026-09-21T04:00:00Z")),
    ];
    assert!(matches!(
        approval_head_refusal(&v, Some(HEAD)),
        Some(ApprovalHeadRefusal::Moved { .. })
    ));
}

#[test]
fn task_1448_newest_approval_decides() {
    // Re-review at the new head clears an older stale approval…
    let cleared = [
        verdict("approved", Some(OLD), Some("2026-09-21T04:00:00Z")),
        verdict("approved", Some(HEAD), Some("2026-09-21T05:00:00Z")),
    ];
    assert_eq!(approval_head_refusal(&cleared, Some(HEAD)), None);
    // …but an older at-head approval does not clear a newer stale one.
    let stale = [
        verdict("approved", Some(HEAD), Some("2026-09-21T04:00:00Z")),
        verdict("approved", Some(OLD), Some("2026-09-21T05:00:00Z")),
    ];
    assert!(approval_head_refusal(&stale, Some(HEAD)).is_some());
}

// ── TASK-1458: closed approvals, merge pin, override audit ─────────────
// trace:TASK-1458 | ai:claude

fn closed(
    mut v: crate::gate::review_verdict::RecordedVerdict,
) -> crate::gate::review_verdict::RecordedVerdict {
    v.closed_by_merge = Some("PR-9".into());
    v
}

#[test]
fn task_1458_closed_stale_approval_does_not_refuse() {
    // A stale approval closed by an earlier PR's merge is history.
    let v = [closed(verdict(
        "approved",
        Some(OLD),
        Some("2026-09-21T05:00:00Z"),
    ))];
    assert_eq!(approval_head_refusal(&v, Some(HEAD)), None);
}

#[test]
fn task_1458_closed_approval_cannot_shadow_or_cover() {
    // A newer CLOSED approval at the head must not hide an open stale one…
    let shadow = [
        verdict("approved", Some(OLD), Some("2026-09-21T04:00:00Z")),
        closed(verdict(
            "approved",
            Some(HEAD),
            Some("2026-09-21T05:00:00Z"),
        )),
    ];
    assert!(matches!(
        approval_head_refusal(&shadow, Some(HEAD)),
        Some(ApprovalHeadRefusal::Moved { .. })
    ));
    // …and a closed approval alone is not an approval to pin the merge to.
    let only_closed = [closed(verdict(
        "approved",
        Some(HEAD),
        Some("2026-09-21T05:00:00Z"),
    ))];
    assert_eq!(approved_match_head(&only_closed, Some(HEAD)), None);
}

#[test]
fn task_1458_match_head_is_the_approved_head() {
    let v = [verdict(
        "approved",
        Some(HEAD),
        Some("2026-09-21T04:00:00Z"),
    )];
    assert_eq!(approved_match_head(&v, Some(HEAD)).as_deref(), Some(HEAD));
    // An abbreviated reviewed sha still pins the FULL head sha.
    let short = [verdict(
        "approved",
        Some(&HEAD[..12]),
        Some("2026-09-21T04:00:00Z"),
    )];
    assert_eq!(
        approved_match_head(&short, Some(HEAD)).as_deref(),
        Some(HEAD)
    );
}

#[test]
fn task_1458_no_match_head_without_a_covering_approval() {
    let stale = [verdict("approved", Some(OLD), Some("2026-09-21T04:00:00Z"))];
    assert_eq!(approved_match_head(&stale, Some(HEAD)), None);
    assert_eq!(approved_match_head(&[], Some(HEAD)), None);
    let ok = [verdict(
        "approved",
        Some(HEAD),
        Some("2026-09-21T04:00:00Z"),
    )];
    assert_eq!(approved_match_head(&ok, None), None);
}

#[test]
fn task_1458_override_event_names_both_shas() {
    let refusal = ApprovalHeadRefusal::Moved {
        reviewed_sha: OLD.into(),
        head_sha: HEAD.into(),
    };
    let line = format_stale_approval_override_event("2026-09-23T00:00:00Z", 77, &refusal);
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["step"], "pr-merge-override-stale-approval");
    assert_eq!(v["status"], "overridden");
    assert_eq!(v["pr"], 77);
    assert_eq!(v["reviewed_sha"], OLD);
    assert_eq!(v["head_sha"], HEAD);
    assert!(!line.contains('\n'), "one JSONL line");
}

#[test]
fn task_1448_candidates_read_pr_and_spec_keyed_verdicts_once() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join(".aida").join("review-verdicts");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("PR-12.json"),
        format!(r#"{{"verdict":"approved","reviewed_sha":"{OLD}"}}"#),
    )
    .unwrap();
    std::fs::write(
        dir.join("TASK-9.json"),
        format!(r#"{{"verdict":"approved","reviewed_sha":"{HEAD}"}}"#),
    )
    .unwrap();
    std::fs::write(dir.join("TASK-10.json"), "not json").unwrap();
    // The same root twice (worktree == main clone) reads each file once.
    let got = merge_gate_verdict_candidates(
        &[tmp.path(), tmp.path()],
        12,
        &[
            "task-9".to_string(),
            "TASK-10".to_string(),
            "TASK-9".to_string(),
        ],
        None,
    );
    let mut shas: Vec<_> = got.iter().filter_map(|v| v.reviewed_sha.clone()).collect();
    shas.sort();
    let mut want = vec![OLD.to_string(), HEAD.to_string()];
    want.sort();
    assert_eq!(shas, want);
}

#[test]
fn draft_only_tag_detection_is_case_insensitive() {
    // STORY-529: the ship-time draft gate fires on this tag, case-insensitive.
    assert!(is_draft_only_tagged(&["review:draft-only".to_string()]));
    assert!(is_draft_only_tagged(&[
        "batch:x".to_string(),
        "Review:Draft-Only".to_string()
    ]));
    assert!(!is_draft_only_tagged(&[
        "review".to_string(),
        "draft-only".to_string(),
        "papercut".to_string()
    ]));
    assert!(!is_draft_only_tagged(&[]));
}

/// BUG-727: only `drain` licenses an unattended merge; every other
/// execution mode — and NO mode at all (fail safe) — requires supervision.
#[test]
fn merge_requires_supervision_truth_table() {
    use aida_core::ExecutionMode as M;
    assert!(!merge_requires_supervision(Some(M::Drain)));
    assert!(merge_requires_supervision(Some(M::Drive)));
    assert!(merge_requires_supervision(Some(M::Guided)));
    assert!(merge_requires_supervision(Some(M::Operator)));
    assert!(merge_requires_supervision(Some(M::Decide)));
    // Fail safe: a spec with no mode set is treated as supervised.
    assert!(merge_requires_supervision(None));
}

/// BUG-727: a marked (non-drain) spec shipped by automation holds the
/// merge — this is the refusal that parks the PR open for the advisor.
#[test]
fn supervised_merge_holds_refuses_marked_spec_for_automation() {
    use aida_core::ExecutionMode as M;
    let specs = vec![
        ("TASK-1080".to_string(), Some(M::Drive)),
        ("BUG-714".to_string(), None),
    ];
    let holds = supervised_merge_holds(&specs, false);
    assert_eq!(
        holds,
        vec![
            ("TASK-1080".to_string(), "drive".to_string()),
            ("BUG-714".to_string(), "unset (supervised)".to_string()),
        ]
    );
}

/// BUG-727 regression guard: a `drain`-mode spec preserves the current
/// auto-merge — the normal automation path is unaffected.
#[test]
fn supervised_merge_holds_drain_mode_spec_still_auto_merges() {
    use aida_core::ExecutionMode as M;
    let specs = vec![("TASK-500".to_string(), Some(M::Drain))];
    assert!(supervised_merge_holds(&specs, false).is_empty());
    // No specs behind the ship (no store / no trailers) ⇒ nothing to hold.
    assert!(supervised_merge_holds(&[], false).is_empty());
}

/// BUG-727: an interactive human at a TTY IS the explicit human/advisor
/// merge — the gate never fires there, whatever the mode.
#[test]
fn supervised_merge_holds_interactive_tty_may_always_merge() {
    use aida_core::ExecutionMode as M;
    let specs = vec![
        ("TASK-1080".to_string(), Some(M::Guided)),
        ("BUG-714".to_string(), None),
    ];
    assert!(supervised_merge_holds(&specs, true).is_empty());
}

fn unrelated_pr_from_plain_shell_is_allowed_during_drain() {
    assert_eq!(ship_merge_block_reason(false, false, false), None);
}

#[test]
fn drive_owned_pr_from_plain_shell_is_blocked_with_owned_reason() {
    assert_eq!(
        ship_merge_block_reason(false, true, false),
        Some(ShipMergeBlockReason::DriveOwnedPr)
    );
}

#[test]
fn drive_seat_blocks_any_pr_with_seat_reason() {
    assert_eq!(
        ship_merge_block_reason(true, false, false),
        Some(ShipMergeBlockReason::DriveSeat)
    );
    assert_eq!(ship_merge_block_reason(true, true, true), None);
}

// TASK-1292: the pure PR-keyed liveness decision. trace:TASK-1292 | ai:claude
#[test]
fn reviewer_liveness_on_this_pr_when_a_live_member_is_bound_to_it() {
    let members = [(true, Some(1948))];
    assert_eq!(
        reviewer_liveness_for_pr(members, 1948),
        ReviewerLiveness::OnThisPr
    );
}

#[test]
fn reviewer_liveness_elsewhere_when_live_member_bound_to_a_different_pr() {
    let members = [(true, Some(1965))];
    assert_eq!(
        reviewer_liveness_for_pr(members, 1948),
        ReviewerLiveness::Elsewhere
    );
}

#[test]
fn reviewer_liveness_none_with_no_drive_members() {
    let members: [(bool, Option<u32>); 0] = [];
    assert_eq!(
        reviewer_liveness_for_pr(members, 1948),
        ReviewerLiveness::None
    );
}

#[test]
fn reviewer_liveness_none_when_the_only_member_bound_to_this_pr_is_terminal() {
    // A member that finished (`completed`/`failed`, not `in-phase-*`)
    // keeps its `pr` field, but it is no longer LIVE — its reviewer
    // is not running, so it must not block a ship.
    let members = [(false, Some(1948))];
    assert_eq!(
        reviewer_liveness_for_pr(members, 1948),
        ReviewerLiveness::None
    );
}

#[test]
fn reviewer_liveness_ignores_live_member_with_no_pr_bound_yet() {
    // An implementer phase that hasn't discovered a PR yet is live but
    // unbound — it must not read as "elsewhere" (which would be a false
    // "some other reviewer is busy" signal) or "on this PR".
    let members = [(true, None)];
    assert_eq!(
        reviewer_liveness_for_pr(members, 1948),
        ReviewerLiveness::None
    );
}

// TASK-1292 regression fixture: the sibling-spec case from the spec's
// acceptance criteria — a reviewer live on PR-N under spec A must refuse
// a ship of PR-N even though the drive's "current spec" is B.
// trace:TASK-1292 | ai:claude
#[test]
fn sibling_spec_reviewer_still_blocks_the_pr_it_is_reviewing() {
    // spec A (BUG-1236) is live-reviewing PR #1948; spec B (STORY-1221)
    // is the drive's nominal "current" member but is shelved/terminal
    // and carries no PR binding of its own — exactly the 2026-09-18
    // near-miss shape.
    let members = [(false, None), (true, Some(1948))];
    assert_eq!(
        reviewer_liveness_for_pr(members, 1948),
        ReviewerLiveness::OnThisPr,
        "a live reviewer on PR-1948 under a sibling spec must still be found"
    );
    let is_owned = reviewer_liveness_for_pr(members, 1948) == ReviewerLiveness::OnThisPr;
    assert!(
        ship_merge_block_reason(false, is_owned, false).is_some(),
        "aida pr ship must refuse PR-1948 while the sibling-spec reviewer is live"
    );
    // A ship of an UNRELATED PR (not the one being reviewed) still goes
    // through — this is not a blanket "any live drive blocks everything".
    let unrelated_owned = reviewer_liveness_for_pr(members, 9999) == ReviewerLiveness::OnThisPr;
    assert!(ship_merge_block_reason(false, unrelated_owned, false).is_none());
}
