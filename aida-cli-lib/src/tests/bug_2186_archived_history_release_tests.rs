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
    mh::write_typed_hold(root, &hold).unwrap();
}

/// What the reviewer launchers do before each review run: clear the
/// current verdict file so the run's summary cannot read a stale one.
fn launch_reviewer(root: &Path) {
    std::fs::remove_file(rv::verdict_path(root, KEY)).unwrap();
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
                }))
            },
            read_hold: &|| mh::read_hold_record(root, PR),
            covered_specs: &|| Ok((vec!["TASK-41".into()], store.clone())),
            read_body: &|key| {
                Ok(serde_json::from_slice(&std::fs::read(rv::verdict_path(
                    root, key,
                ))?)?)
            },
            read_history: &|key| RetainedHistory::beside(&rv::verdict_path(root, key)),
            read_corpus: &|ids| {
                mh::corpus_gate_bodies(&[root], PR, ids).map_err(anyhow::Error::msg)
            },
            grant_check: &|id, subject| sa::grant_check_for(root, id, subject),
            approval_authority: &|_| Ok(()),
            mandate: &|| bail!("no mandate is active"),
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

fn assert_released(root: &Path, result: Result<Outcome>) {
    assert_eq!(result.unwrap(), Outcome::Released { mandate: None });
    assert!(
        !mh::hold_path(root, PR).exists(),
        "the rework marker must be removed"
    );
    let audit: Value =
        serde_json::from_slice(&std::fs::read(mh::clearance_path(root, PR)).unwrap()).unwrap();
    assert_eq!(audit["cleared_by"], PRINCIPAL);
    assert_eq!(audit["target_head_sha"], HEAD);
}

#[test]
fn approval_at_a_new_head_releases_in_place_from_the_archived_refusal() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = AmbientGuard::hermetic_with_seat(root, "reviewer", &[]);
    rework_cycle(root);
    // The schema the release reader must accept: the writer kept the
    // refusal only in the per-commit archive, not in inline `rounds`.
    let current: Value =
        serde_json::from_slice(&std::fs::read(rv::verdict_path(root, KEY)).unwrap()).unwrap();
    assert!(current.get("rounds").is_none(), "{current}");
    assert!(current.get("release_grant").is_some(), "{current}");

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
        mh::hold_path(root, PR).exists(),
        "a failed release keeps the marker"
    );

    let (retry, comments) = release(root, false);
    assert_released(root, retry);
    assert!(comments.is_empty(), "the audit comment was already posted");
}

#[test]
fn missing_refusal_history_stays_held_and_names_where_it_looked() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = AmbientGuard::hermetic_with_seat(root, "reviewer", &[]);
    record(root, "request-changes", OLD);
    arm(root);
    launch_reviewer(root);
    // The refusal's history is gone; only the approval will be on record.
    let archive = rv::verdict_archive_dir(&rv::verdict_path(root, KEY)).unwrap();
    std::fs::remove_dir_all(&archive).unwrap();
    record(root, "approved", HEAD);

    let (result, _) = release(root, false);
    let error = result.unwrap_err().to_string();
    assert!(
        error.contains("no earlier request-changes/rejected"),
        "{error}"
    );
    assert!(error.contains(&archive.display().to_string()), "{error}");
    assert!(error.contains(env!("CARGO_PKG_VERSION")), "{error}");
    assert!(mh::hold_path(root, PR).exists());
}

#[test]
fn an_unreadable_archived_round_stays_held() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = AmbientGuard::hermetic_with_seat(root, "reviewer", &[]);
    rework_cycle(root);
    let archive = rv::verdict_archive_dir(&rv::verdict_path(root, KEY)).unwrap();
    std::fs::write(archive.join(format!("{OLD}.json")), "not json").unwrap();

    let (result, _) = release(root, false);
    let error = result.unwrap_err().to_string();
    assert!(error.contains("history is unreadable"), "{error}");
    assert!(mh::hold_path(root, PR).exists());
}

#[test]
fn a_refusal_recorded_after_the_approval_stays_held() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = AmbientGuard::hermetic_with_seat(root, "reviewer", &[]);
    rework_cycle(root);
    // A later refusal at an unrelated sha lands in the archive only.
    let path = rv::verdict_path(root, KEY);
    let approval = std::fs::read(&path).unwrap();
    record(root, "rejected", "cccccccccccccccccccccccccccccccccccccccc");
    std::fs::write(&path, approval).unwrap();

    let (result, _) = release(root, false);
    let error = result.unwrap_err().to_string();
    assert!(error.contains("strictly newer"), "{error}");
    assert!(mh::hold_path(root, PR).exists());
}
