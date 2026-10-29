//! BUG-2186: a rework hold armed by REQUEST CHANGES releases on a fresh
//! APPROVE at a new head with the verdict files the writer actually
//! produces. Every verdict here goes through the `aida review record` write
//! boundary (`build_verdict_object` → `stamp_record_grant` →
//! `write_verdict_object`) under a real minted reviewer grant, and the
//! reviewer launchers' clear of the current file runs between rounds — the
//! step that leaves the current file with no inline `rounds`. No hand-built
//! verdict JSON.
// trace:BUG-2186 | ai:claude

use super::*;
use crate::test_env::AmbientGuard;
use std::cell::RefCell;

const OLD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PR: u64 = 41;
const KEY: &str = "PR-41";
const REVIEWER: &str = "claude reviewer seat";

/// One `aida review record KEY --verdict <verdict> --sha <sha> --pr 41`,
/// through the record handler's write boundary.
fn record(root: &Path, verdict: &str, sha: &str) {
    let path = rv::verdict_path(root, KEY);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let findings = if verdict == "approved" {
        Vec::new()
    } else {
        vec!["the fix is untested".to_string()]
    };
    let mut obj = rv::build_verdict_object(
        root,
        &path,
        Some(verdict),
        Some(sha),
        Some("bug-41"),
        Some("round summary"),
        &findings,
        REVIEWER,
    )
    .unwrap();
    stamp_record_grant(root, &mut obj);
    rv::write_verdict_object(&path, &obj).unwrap();
}

/// The hold a refusing `aida review record` arms, referencing its verdict.
fn arm(root: &Path) {
    let mut hold = mh::typed_hold(PR, mh::HoldReasonKind::Rework, "rework", Some(OLD.into()));
    hold.verdict_ref = Some(mh::VerdictRef::new(
        KEY,
        Some(PR),
        Some(OLD.into()),
        Some(REVIEWER.into()),
    ));
    hold.spec = Some(KEY.into());
    mh::place_hold(root, &hold).unwrap();
}

/// What the reviewer launchers do before each review run: clear the
/// current verdict file so the run's summary cannot read a stale one. M1
/// slice 2: the record path writes no current file (the round is a gate
/// ledger event, which is never cleared), so there is nothing to clear.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
fn launch_reviewer(root: &Path) {
    assert!(
        !rv::verdict_path(root, KEY).exists(),
        "no legacy file is written"
    );
}

/// The current record for KEY, as the gates weigh it (the gate ledger).
fn current(root: &Path) -> Value {
    serde_json::from_str(
        &rv::current_verdict(root, KEY)
            .expect("a current verdict")
            .body,
    )
    .unwrap()
}

/// A rework hold armed with no recorded refusal behind it: what a fresh
/// reviewer worktree that kept no round history leaves (BUG-2216). The
/// ledger never loses a round, so "the history is gone" is a refusal that
/// never reached the shared record.
fn arm_without_history(root: &Path) {
    arm(root);
}

/// REQUEST CHANGES at OLD, a fresh reviewer launch, then APPROVE at HEAD.
fn rework_cycle(root: &Path) {
    record(root, "request-changes", OLD);
    arm(root);
    launch_reviewer(root);
    record(root, "approved", HEAD);
}

/// One auto-release attempt (the in-place call `review record` makes, or a
/// `merge-hold auto-release` retry), reading the verdict and its retained
/// history from disk and revalidating the recorded grant for real. The
/// BUG-1918 attestation gate and the forge are pinned by the STORY-1697
/// tests; `labels_fail` models a transient forge failure in place.
fn release(root: &Path, labels_fail: bool) -> (Result<Outcome>, Vec<String>) {
    release_reading(root, labels_fail, None)
}

/// [`release`], with the arming-verdict read answering `body_override`.
fn release_reading(
    root: &Path,
    labels_fail: bool,
    body_override: Option<Value>,
) -> (Result<Outcome>, Vec<String>) {
    let hold = mh::read_hold_record(root, PR).expect("an armed hold");
    let mut req = aida_core::Requirement::new("Improve a list".into(), "fixture".into());
    req.req_type = aida_core::RequirementType::Task;
    req.execution_mode = Some(aida_core::ExecutionMode::Drive);
    req.spec_id = Some("TASK-41".into());
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(req);
    let comments = RefCell::new(Vec::new());
    let result = auto_release_with(
        root,
        &hold,
        AutoReleaseIo {
            live_head: &|| {
                Ok(Some(mh::PinnedChange {
                    merged: false,
                    state: mh::ChangeState::Open,
                    head_sha: Some(HEAD.into()),
                    hold_label: true,
                    recorded_label: true,
                    cleared_label: false,
                }))
            },
            read_hold: &|| mh::read_hold_record(root, PR),
            covered_specs: &|| Ok((vec!["TASK-41".into()], store.clone())),
            read_body: &|key| match &body_override {
                Some(body) => Ok(body.clone()),
                None => Ok(serde_json::from_str(
                    &rv::current_verdict(root, key)
                        .context("arming verdict is missing")?
                        .body,
                )?),
            },
            read_history: &|key| RetainedHistory::projected(root, key),
            read_corpus: &|ids| {
                mh::corpus_gate_bodies(&[root], PR, ids).map_err(anyhow::Error::msg)
            },
            grant_check: &|id, subject| sa::grant_check_for(root, id, subject),
            approval_authority: &|_| Ok(()),
            mandate: &|| bail!("no mandate is active"),
            // The real release-condition check over this root's verdicts, so
            // the no-history fallback is exercised end to end.
            // trace:BUG-2216 | ai:claude
            refusal_release: &|record, head| {
                crate::pr_cmd::ship_refusal_release(&[root], record, Some(head))
            },
            comment: &|body| {
                comments.borrow_mut().push(body.to_string());
                Ok(())
            },
            labels: &|| {
                ensure!(!labels_fail, "release labels failed: forge unavailable");
                Ok(())
            },
        },
    );
    (result, comments.into_inner())
}

/// A release attempt once the hold is gone: the stored hold record is reused.
fn release_after_release(root: &Path) -> (Result<Outcome>, Vec<String>) {
    let mut hold = mh::typed_hold(PR, mh::HoldReasonKind::Rework, "rework", Some(OLD.into()));
    hold.verdict_ref = Some(mh::VerdictRef::new(
        KEY,
        Some(PR),
        Some(OLD.into()),
        Some(REVIEWER.into()),
    ));
    hold.spec = Some(KEY.into());
    let comments = RefCell::new(Vec::new());
    let result = release_with(
        root,
        &hold,
        &Release {
            verdict: mh::VerdictRef::new(KEY, Some(PR), Some(HEAD.into()), Some(REVIEWER.into())),
            mandate: None,
        },
        &[],
        || Ok(HEAD.into()),
        |body| {
            comments.borrow_mut().push(body.to_string());
            Ok(())
        },
        || Ok(()),
    )
    .map(|()| Outcome::Released { mandate: None });
    (result, comments.into_inner())
}

fn assert_released(root: &Path, result: Result<Outcome>) {
    assert_eq!(result.unwrap(), Outcome::Released { mandate: None });
    assert!(
        !mh::hold_is_open(root, PR),
        "the rework marker must be removed"
    );
    let audit = Value::Object(mh::recorded_clearance(root, PR).expect("a recorded release"));
    assert_eq!(audit["cleared_by"], PRINCIPAL);
    assert_eq!(audit["target_head_sha"], HEAD);
    // BUG-2216: the release emits the hold-release event exactly once, with
    // the auto-release principal and no human attestation.
    // trace:BUG-2216 | ai:claude
    let releases: Vec<_> = crate::events::read_all(root)
        .into_iter()
        .filter(|e| {
            matches!(
                &e.kind,
                crate::events::EventKind::MergeHoldChanged {
                    pr: 41,
                    placed: false,
                    principal: Some(principal),
                    human_attested: false,
                    ..
                } if principal == PRINCIPAL
            )
        })
        .collect();
    assert_eq!(releases.len(), 1, "exactly one release event");
}

#[test]
fn approval_at_a_new_head_releases_in_place_from_the_archived_refusal() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = AmbientGuard::hermetic_with_seat(root, "reviewer", &[]);
    rework_cycle(root);
    // The schema the release reader must accept: the writer kept the
    // refusal as its own round on the gate ledger (M1 slice 2).
    let current = current(root);
    assert!(current.get("release_grant").is_some(), "{current}");
    let history = RetainedHistory::projected(root, KEY).unwrap();
    assert!(
        history
            .archived
            .iter()
            .any(|r| r["verdict"] == "request-changes" && r["reviewed_sha"] == OLD),
        "{:?}",
        history.archived
    );

    let (result, comments) = release(root, false);
    assert_released(root, result);
    assert_eq!(comments.len(), 1);
}

#[test]
fn auto_release_retry_releases_after_the_in_place_attempt_failed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = AmbientGuard::hermetic_with_seat(root, "reviewer", &[]);
    rework_cycle(root);

    let (in_place, _) = release(root, true);
    assert!(in_place.is_err(), "{in_place:?}");
    assert!(
        mh::hold_is_open(root, PR),
        "a failed release keeps the marker"
    );

    let (retry, comments) = release(root, false);
    assert_released(root, retry);
    assert!(comments.is_empty(), "the audit comment was already posted");
}

/// BUG-2216: a fresh reviewer worktree keeps no round history, so the hold
/// marker is the arming evidence and the approval at the live head releases
/// through the same path (#2885, #2886, #2868 needed a human clear).
// trace:BUG-2216 | ai:claude
#[test]
fn missing_refusal_history_releases_on_the_hold_markers_own_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = AmbientGuard::hermetic_with_seat(root, "reviewer", &[]);
    // The refusal's history is gone; only the approval will be on record.
    arm_without_history(root);
    launch_reviewer(root);
    record(root, "approved", HEAD);

    let (result, comments) = release(root, false);
    assert_released(root, result);
    assert_eq!(comments.len(), 1);
}

/// BUG-2216: with no history AND no approval at the live head, the fallback
/// fails closed and says why the release condition is not met.
// trace:BUG-2216 | ai:claude
#[test]
fn missing_history_with_a_stale_approval_stays_held() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = AmbientGuard::hermetic_with_seat(root, "reviewer", &[]);
    arm_without_history(root);
    launch_reviewer(root);
    // The approval is at the arming head, not the live head.
    record(root, "approved", OLD);

    let (result, _) = release(root, false);
    let error = result.unwrap_err().to_string();
    assert!(error.contains("stale or unverifiable head"), "{error}");
    assert!(mh::hold_is_open(root, PR));
}

/// BUG-2216: the release happens exactly once — a second attempt after the
/// marker is gone reports no hold and records nothing new.
// trace:BUG-2216 | ai:claude
#[test]
fn a_released_hold_is_released_exactly_once() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = AmbientGuard::hermetic_with_seat(root, "reviewer", &[]);
    arm_without_history(root);
    launch_reviewer(root);
    record(root, "approved", HEAD);

    let (first, comments) = release(root, false);
    assert_released(root, first);
    assert_eq!(comments.len(), 1);
    let clearance = mh::recorded_clearance(root, PR).unwrap();
    let events = std::fs::read(mh::hold_ledger_file(root, PR)).unwrap();

    let hold = mh::read_hold_record(root, PR);
    assert!(hold.is_none(), "the marker is gone after the release");
    let (again, _) = release_after_release(root);
    assert!(again.is_err(), "a second attempt finds no hold");
    assert_eq!(
        mh::recorded_clearance(root, PR).unwrap(),
        clearance,
        "nothing rewrites the recorded clearance"
    );
    assert_eq!(
        std::fs::read(mh::hold_ledger_file(root, PR)).unwrap(),
        events,
        "a second attempt records nothing new"
    );
}

/// BUG-2216: holds of any other kind never take the no-history fallback —
/// the Rework-only eligibility refuses them before it is consulted.
// trace:BUG-2216 | ai:claude
#[test]
fn only_rework_holds_release_on_the_markers_evidence() {
    for kind in [
        mh::HoldReasonKind::Recusal,
        mh::HoldReasonKind::Supervision,
        mh::HoldReasonKind::Decision,
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let _env = AmbientGuard::hermetic_with_seat(root, "reviewer", &[]);
        record(root, "request-changes", OLD);
        let mut hold = mh::typed_hold(PR, kind, "held", Some(OLD.into()));
        hold.verdict_ref = Some(mh::VerdictRef::new(
            KEY,
            Some(PR),
            Some(OLD.into()),
            Some(REVIEWER.into()),
        ));
        hold.spec = Some(KEY.into());
        if kind == mh::HoldReasonKind::Recusal {
            hold.recused_principals = vec!["advisor:recused".into()];
        }
        mh::place_hold(root, &hold).unwrap();
        launch_reviewer(root);
        record(root, "approved", HEAD);

        let (result, _) = release(root, false);
        let error = result.unwrap_err().to_string();
        assert!(error.contains("Rework-only"), "{kind:?}: {error}");
        assert!(mh::hold_is_open(root, PR), "{kind:?}");
    }

    // A legacy (plaintext) hold is refused the same way.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = AmbientGuard::hermetic_with_seat(root, "reviewer", &[]);
    record(root, "request-changes", OLD);
    // A legacy marker an older binary left in the primary checkout.
    let marker = mh::hold_path(root, PR);
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
    std::fs::write(&marker, format!("CHANGES REQUESTED for {KEY} at {OLD}\n")).unwrap();
    launch_reviewer(root);
    record(root, "approved", HEAD);
    let (result, _) = release(root, false);
    // A legacy marker never reaches eligibility: it carries no typed arming
    // verdict reference, so the release has nothing to answer.
    let error = result.unwrap_err().to_string();
    assert!(error.contains("no arming verdict reference"), "{error}");
    assert!(mh::hold_is_open(root, PR));
}

#[test]
fn an_unreadable_archived_round_stays_held() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = AmbientGuard::hermetic_with_seat(root, "reviewer", &[]);
    rework_cycle(root);
    // A legacy per-commit archive entry, read through, that does not parse.
    let archive = rv::verdict_archive_dir(&rv::verdict_path(root, KEY)).unwrap();
    std::fs::create_dir_all(&archive).unwrap();
    std::fs::write(archive.join(format!("{OLD}.json")), "not json").unwrap();

    let (result, _) = release(root, false);
    let error = result.unwrap_err().to_string();
    assert!(error.contains("history is unreadable"), "{error}");
    assert!(mh::hold_is_open(root, PR));
}

#[test]
fn a_refusal_recorded_after_the_approval_stays_held() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = AmbientGuard::hermetic_with_seat(root, "reviewer", &[]);
    rework_cycle(root);
    // A later refusal at an unrelated sha lands in the retained history,
    // while the arming read still answers the approval.
    let approval = current(root);
    record(root, "rejected", "cccccccccccccccccccccccccccccccccccccccc");

    let (result, _) = release_reading(root, false, Some(approval));
    let error = result.unwrap_err().to_string();
    assert!(error.contains("strictly newer"), "{error}");
    assert!(mh::hold_is_open(root, PR));
}
