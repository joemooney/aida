//! BUG-2123 + TASK-1757: a Rework hold composed from refusals on two specs
//! keeps both, and auto-release needs an approval answering each one. The
//! release's second pass runs under the per-PR hold lock, and a second pass
//! that refuses supersedes the clearance it recorded. Verdicts go through the
//! `aida review record` write boundary under a real reviewer grant, as in the
//! BUG-2186 tests.
// trace:TASK-1951 trace:BUG-2123 trace:TASK-1757 | ai:claude

use super::*;
use crate::test_env::AmbientGuard;
use std::cell::{Cell, RefCell};

const OLD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const MOVED: &str = "cccccccccccccccccccccccccccccccccccccccc";
const PR: u64 = 51;
const A: &str = "TASK-51";
const B: &str = "TASK-52";
const REVIEWER: &str = "claude reviewer seat";

fn record(root: &Path, key: &str, verdict: &str, sha: &str) {
    let path = rv::verdict_path(root, key);
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
        Some(&key.to_ascii_lowercase()),
        Some("round summary"),
        &findings,
        REVIEWER,
    )
    .unwrap();
    stamp_record_grant(root, &mut obj);
    rv::write_verdict_object(&path, &obj).unwrap();
}

fn rework(key: &str) -> mh::MergeHoldRecord {
    let mut hold = mh::typed_hold(
        PR,
        mh::HoldReasonKind::Rework,
        &format!("CHANGES REQUESTED for {key}"),
        Some(OLD.into()),
    );
    hold.verdict_ref = Some(mh::VerdictRef::new(
        key,
        Some(PR),
        Some(OLD.into()),
        Some(REVIEWER.into()),
    ));
    hold.spec = Some(key.into());
    hold
}

/// Refusals on A then B, each arming the hold; the second composes onto
/// the first.
fn refuse_both(root: &Path) {
    for key in [A, B] {
        record(root, key, "request-changes", OLD);
        mh::write_typed_hold(root, &rework(key)).unwrap();
        std::fs::remove_file(rv::verdict_path(root, key)).unwrap();
    }
}

fn store() -> aida_core::RequirementsStore {
    let mut store = aida_core::RequirementsStore::default();
    for key in [A, B] {
        let mut req = aida_core::Requirement::new("Improve a list".into(), "fixture".into());
        req.req_type = aida_core::RequirementType::Task;
        req.execution_mode = Some(aida_core::ExecutionMode::Drive);
        req.spec_id = Some(key.into());
        store.requirements.push(req);
    }
    store
}

/// One auto-release attempt. `heads` is the sequence of live heads the
/// forge reports, the last one repeating.
fn release(root: &Path, heads: &[&str]) -> (Result<Outcome>, Vec<String>) {
    release_hooked(root, heads, &|_| {})
}

/// [`release`], calling `on_read(i)` before the forge answers its `i`th
/// head read (0: eligibility, 1: first pass, 2: second pass, under the lock).
fn release_hooked(
    root: &Path,
    heads: &[&str],
    on_read: &dyn Fn(usize),
) -> (Result<Outcome>, Vec<String>) {
    let hold = mh::read_hold_record(root, PR).expect("an armed hold");
    let comments = RefCell::new(Vec::new());
    let reads = Cell::new(0usize);
    let store = store();
    let result = auto_release_with(
        root,
        &hold,
        AutoReleaseIo {
            live_head: &|| {
                on_read(reads.get());
                let i = reads.get().min(heads.len() - 1);
                reads.set(reads.get() + 1);
                Ok(Some(mh::PinnedChange {
                    merged: false,
                    state: mh::ChangeState::Open,
                    head_sha: Some(heads[i].into()),
                    hold_label: true,
                    recorded_label: true,
                    cleared_label: false,
                }))
            },
            read_hold: &|| mh::read_hold_record(root, PR),
            covered_specs: &|| Ok((vec![A.into(), B.into()], store.clone())),
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
            labels: &|| Ok(()),
        },
    );
    (result, comments.into_inner())
}

#[test]
fn composing_rework_holds_keeps_every_refusal() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    mh::write_typed_hold(root, &rework(A)).unwrap();
    mh::write_typed_hold(root, &rework(B)).unwrap();
    let hold = mh::read_hold_record(root, PR).unwrap();
    assert_eq!(hold.verdict_ref.as_ref().unwrap().key, B);
    let shadows: Vec<_> = hold
        .absorbed
        .iter()
        .map(|a| {
            (
                a.reason_kind,
                a.verdict_ref.as_ref().map(|v| v.key.as_str()),
            )
        })
        .collect();
    assert_eq!(shadows, vec![(mh::HoldReasonKind::Rework, Some(A))]);
    // Re-arming on the same verdict replaces it rather than stacking.
    mh::write_typed_hold(root, &rework(B)).unwrap();
    assert_eq!(mh::read_hold_record(root, PR).unwrap().absorbed.len(), 1);
    assert_eq!(
        arming_refs(&mh::read_hold_record(root, PR).unwrap())
            .unwrap()
            .iter()
            .map(|r| r.key.as_str())
            .collect::<Vec<_>>(),
        vec![B, A]
    );
}

/// BUG-2123 acceptance: refusals on A and B, only B approved: not released,
/// and the refusal says which one is unanswered.
#[test]
fn an_approval_of_one_spec_does_not_release_an_unanswered_refusal_on_another() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = AmbientGuard::hermetic_with_seat(root, "reviewer", &[]);
    refuse_both(root);
    record(root, B, "approved", HEAD);

    let (result, comments) = release(root, &[HEAD]);
    let error = format!("{:#}", result.unwrap_err());
    assert!(error.contains("the refusal on TASK-51"), "{error}");
    assert!(mh::hold_path(root, PR).exists(), "the hold stays armed");
    assert!(!mh::clearance_path(root, PR).exists());
    assert!(comments.is_empty());
}

#[test]
fn approvals_answering_every_refusal_release_and_record_each() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = AmbientGuard::hermetic_with_seat(root, "reviewer", &[]);
    refuse_both(root);
    record(root, A, "approved", HEAD);
    record(root, B, "approved", HEAD);

    let (result, comments) = release(root, &[HEAD]);
    assert_eq!(result.unwrap(), Outcome::Released { mandate: None });
    assert!(!mh::hold_path(root, PR).exists());
    let audit: Value =
        serde_json::from_slice(&std::fs::read(mh::clearance_path(root, PR)).unwrap()).unwrap();
    assert_eq!(audit["released_by_verdict"]["key"], B);
    assert_eq!(audit["also_released_by"][0]["key"], A);
    assert!(comments[0].contains("TASK-51"), "{comments:?}");
}

#[test]
fn covered_specs_include_the_title_and_absorbed_holds() {
    let mut hold = rework(B);
    hold.absorbed.push(mh::AbsorbedHold {
        reason_kind: mh::HoldReasonKind::Rework,
        detail: "CHANGES REQUESTED for TASK-51".into(),
        recused_principals: Vec::new(),
        target_head_sha: Some(OLD.into()),
        spec: Some(A.into()),
        placed_by: None,
        verdict_ref: Some(mh::VerdictRef::new(A, Some(PR), Some(OLD.into()), None)),
    });
    let ids = covered_spec_ids(
        &["fix: one (TASK-53)".to_string()],
        "fix: the whole thing (TASK-54)",
        &hold,
    );
    assert_eq!(ids, vec!["TASK-51", "TASK-52", "TASK-53", "TASK-54"]);
}

/// TASK-1757 (3): the head moves between the clearance write and the
/// second pass. The clearance is superseded, never read as a release: a
/// later hand-deletion of the marker is still reported as tampering.
#[test]
fn a_refused_second_pass_supersedes_its_clearance() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = AmbientGuard::hermetic_with_seat(root, "reviewer", &[]);
    refuse_both(root);
    record(root, A, "approved", HEAD);
    record(root, B, "approved", HEAD);

    // Reads: eligibility, first validate, second validate (moved).
    let (result, comments) = release(root, &[HEAD, HEAD, MOVED]);
    let error = format!("{:#}", result.unwrap_err());
    assert!(error.contains("PR head moved before release"), "{error}");
    assert!(mh::hold_path(root, PR).exists(), "the hold stays armed");
    assert!(
        !mh::clearance_path(root, PR).exists(),
        "the withdrawn clearance must not stay where readers find a release"
    );
    let dir = mh::clearance_path(root, PR).parent().unwrap().to_path_buf();
    let kept: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        kept.iter().any(|n| n.starts_with("PR-51.superseded-")),
        "{kept:?}"
    );
    assert!(
        comments.last().unwrap().contains("authorization withdrawn"),
        "{comments:?}"
    );

    std::fs::remove_file(mh::hold_path(root, PR)).unwrap();
    assert_eq!(mh::unrecorded_marker_removals(root), vec![PR]);
}

/// TASK-1757 (1): a contended hold lock refuses with a named reason, for a
/// placement and for a release alike, and changes nothing.
#[test]
fn a_contended_hold_lock_refuses_placement_and_release() {
    use fs2::FileExt;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = AmbientGuard::hermetic_with_seat(root, "reviewer", &[]);
    refuse_both(root);
    record(root, A, "approved", HEAD);
    record(root, B, "approved", HEAD);
    mh::HOLD_LOCK_WAIT_OVERRIDE.with(|w| w.set(Some(std::time::Duration::from_millis(200))));

    let lock_path = mh::hold_lock_path(root, PR);
    let holder = std::fs::OpenOptions::new()
        .write(true)
        .open(&lock_path)
        .unwrap();
    holder.lock_exclusive().unwrap();

    let placed = mh::write_typed_hold(root, &rework("TASK-53")).unwrap_err();
    assert!(
        placed.to_string().contains("merge-hold PR-51 is busy"),
        "{placed}"
    );
    let (released, _) = release(root, &[HEAD]);
    let released = format!("{:#}", released.unwrap_err());
    assert!(released.contains("merge-hold PR-51 is busy"), "{released}");
    assert!(mh::hold_path(root, PR).exists());
    assert!(
        !mh::clearance_path(root, PR).exists(),
        "a busy release leaves no clearance"
    );
    assert_eq!(mh::read_hold_record(root, PR).unwrap().absorbed.len(), 1);

    FileExt::unlock(&holder).unwrap();
    let (result, _) = release(root, &[HEAD]);
    assert_eq!(result.unwrap(), Outcome::Released { mandate: None });
    mh::HOLD_LOCK_WAIT_OVERRIDE.with(|w| w.set(None));
}

/// TASK-1757 (1), review round 1: a panic inside the lock, caught by a
/// long-lived thread, releases the flock and the thread's re-entrant entry,
/// so the next call on that thread still waits on (and refuses past) a
/// holder instead of taking the re-entrant shortcut unlocked.
// trace:TASK-1951 trace:TASK-1757 | ai:claude
#[test]
fn a_caught_panic_inside_the_hold_lock_leaves_no_reentrant_entry() {
    use fs2::FileExt;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = AmbientGuard::hermetic_with_seat(root, "reviewer", &[]);
    mh::HOLD_LOCK_WAIT_OVERRIDE.with(|w| w.set(Some(std::time::Duration::from_millis(200))));

    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        mh::with_hold_lock(root, PR, || panic!("inside the hold lock"))
    }));
    assert!(panicked.is_err());

    let holder = std::fs::OpenOptions::new()
        .write(true)
        .open(mh::hold_lock_path(root, PR))
        .unwrap();
    holder
        .try_lock_exclusive()
        .expect("the panic released the flock");
    let again = mh::with_hold_lock(root, PR, || ()).unwrap_err();
    assert!(
        again.to_string().contains("merge-hold PR-51 is busy"),
        "the thread kept no re-entrant entry: {again}"
    );
    FileExt::unlock(&holder).unwrap();
    mh::with_hold_lock(root, PR, || ()).unwrap();
    mh::HOLD_LOCK_WAIT_OVERRIDE.with(|w| w.set(None));
}

/// TASK-1757 (4): the production `auto_release`, not `auto_release_with`
/// with injected fixtures. A real repository (GitHub origin), a real
/// distributed store and the real verdict files; only `gh` is faked, on
/// PATH. Each refusal can only come from a production reader: no marker
/// reads as no hold, the forge head/headlines/title are fetched, a missing
/// arming verdict is reported from disk, and a spec absent from the store is
/// reported from the store.
// trace:TASK-1951 trace:TASK-1757 | ai:claude
#[test]
fn production_auto_release_reads_the_forge_hold_verdict_and_store() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let git = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .current_dir(&root)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["remote", "add", "origin", "https://github.com/o/r.git"]);
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[forge]\nprovider = \"github\"\n\n[deployment]\nmode = \"distributed\"\nstore_path = \".aida-store\"\n",
    )
    .unwrap();
    use aida_core::db::DatabaseBackend;
    let store_dir = root.join(".aida-store");
    std::fs::create_dir_all(&store_dir).unwrap();
    assert!(crate::test_git::git_command()
        .current_dir(&store_dir)
        .args(["init", "-q", "-b", "aida-store"])
        .status()
        .unwrap()
        .success());
    let mut other = aida_core::Requirement::new("Other".into(), "fixture".into());
    other.spec_id = Some("TASK-62".into());
    aida_core::db::GitBackend::new(&store_dir)
        .unwrap()
        .save(&aida_core::RequirementsStore {
            requirements: vec![other],
            ..Default::default()
        })
        .unwrap();

    let bin = root.join("fake-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = root.join("gh-calls");
    let gh = bin.join("gh");
    crate::test_exec::write_executable(
        &gh,
        format!(
            r#"#!/bin/sh
echo "$*" >> '{log}'
case "$*" in
  *headRefOid,labels*) echo '{{"url":"https://github.com/o/r/pull/61","state":"OPEN","headRefOid":"{HEAD}","labels":[]}}' ;;
  *messageHeadline*) echo 'fix: wiring (TASK-61)' ;;
  *) echo '{{"url":"https://github.com/o/r/pull/61","state":"OPEN","title":"fix: wiring (TASK-61)","baseRefName":"main","headRefName":"task-61","headRefOid":"{HEAD}","isDraft":false,"isCrossRepository":false,"author":{{"login":"a"}}}}' ;;
esac
"#,
            log = log.display()
        ),
    );
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let _env = AmbientGuard::hermetic_with_seat_and(
        &root,
        "reviewer",
        &[],
        &[
            ("PATH", Some(path.as_str())),
            ("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap())),
        ],
    );

    // Hold read: no marker in the primary is no hold, and no forge call.
    assert_eq!(auto_release(&root, 61).unwrap(), Outcome::NoHold);
    assert!(!log.exists(), "no forge call without a hold");

    let mut hold = mh::typed_hold(61, mh::HoldReasonKind::Rework, "rework", Some(OLD.into()));
    hold.verdict_ref = Some(mh::VerdictRef::new(
        "TASK-61",
        Some(61),
        Some(OLD.into()),
        Some(REVIEWER.into()),
    ));
    hold.spec = Some("TASK-61".into());
    mh::write_typed_hold(&root, &hold).unwrap();

    // Verdict read: the arming verdict is looked up on disk.
    let missing = format!("{:#}", auto_release(&root, 61).unwrap_err());
    assert!(
        missing.contains("arming verdict TASK-61 is missing"),
        "{missing}"
    );
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(
        calls.contains("url,state,headRefOid,labels"),
        "forge head: {calls}"
    );
    assert!(calls.contains("messageHeadline"), "headlines: {calls}");

    // Requirement lookup: the store holds TASK-62, not TASK-61.
    record(&root, "TASK-61", "request-changes", OLD);
    std::fs::remove_file(rv::verdict_path(&root, "TASK-61")).unwrap();
    record(&root, "TASK-61", "approved", HEAD);
    let unresolved = format!("{:#}", auto_release(&root, 61).unwrap_err());
    assert!(
        unresolved.contains("covered spec is missing or ambiguous: TASK-61 resolves to 0"),
        "{unresolved}"
    );
    assert!(mh::hold_path(&root, 61).exists());
}

/// TASK-1757 (5), observed on #2638: the approval was recorded under the
/// spec while the refusal armed `PR-N`. The refusal names the key the
/// release reads and the command that answers it.
// trace:TASK-1951 trace:TASK-1757 | ai:claude
#[test]
fn an_approval_under_another_key_names_the_key_the_release_reads() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = AmbientGuard::hermetic_with_seat(root, "reviewer", &[]);
    record(root, A, "request-changes", OLD);
    mh::write_typed_hold(root, &rework(A)).unwrap();
    std::fs::remove_file(rv::verdict_path(root, A)).unwrap();
    record(root, A, "request-changes", OLD);
    record(root, B, "approved", HEAD);
    let hold = mh::read_hold_record(root, PR).unwrap();
    assert_eq!(arming_refs(&hold).unwrap().len(), 1);
    let (result, _) = release(root, &[HEAD]);
    let error = format!("{:#}", result.unwrap_err());
    assert!(
        error.contains("the latest verdict recorded under TASK-51 is"),
        "{error}"
    );
    assert!(error.contains("aida review record TASK-51"), "{error}");
}

/// TASK-2285: a second auto-release of the same PR finishes while the first
/// waits for the lock. The first's second pass refuses (the hold is gone) but
/// must not withdraw the winner's clearance, or the released PR reads as a
/// hand-deleted marker and every merge refuses until a human clears it.
// trace:TASK-1951 trace:TASK-2285 | ai:claude
#[test]
fn an_overlapping_release_keeps_the_winners_clearance() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = AmbientGuard::hermetic_with_seat(root, "reviewer", &[]);
    refuse_both(root);
    record(root, A, "approved", HEAD);
    record(root, B, "approved", HEAD);

    let winner = RefCell::new(None);
    let (loser, comments) = release_hooked(root, &[HEAD], &|i| {
        if i == 2 {
            // The lock is re-entrant per thread, so the winner runs to
            // completion here, as if it had held the lock first.
            *winner.borrow_mut() = Some(release(root, &[HEAD]).0);
        }
    });
    assert_eq!(
        winner.into_inner().unwrap().unwrap(),
        Outcome::Released { mandate: None }
    );
    let error = format!("{:#}", loser.unwrap_err());
    assert!(error.contains("already removed the hold"), "{error}");
    assert!(!mh::hold_path(root, PR).exists());
    assert!(
        mh::clearance_path(root, PR).exists(),
        "the winner's clearance stands"
    );
    assert!(
        !comments
            .iter()
            .any(|c| c.contains("authorization withdrawn")),
        "{comments:?}"
    );
    assert!(mh::unrecorded_marker_removals(root).is_empty());
}

/// TASK-2285: a release whose clearance was withdrawn by a concurrent one
/// (after the busy refusal) keeps the marker armed instead of removing it
/// without a clearance.
// trace:TASK-1951 trace:TASK-2285 | ai:claude
#[test]
fn a_withdrawn_clearance_keeps_the_hold_armed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = AmbientGuard::hermetic_with_seat(root, "reviewer", &[]);
    refuse_both(root);
    record(root, A, "approved", HEAD);
    record(root, B, "approved", HEAD);

    let (result, _) = release_hooked(root, &[HEAD], &|i| {
        if i == 2 {
            std::fs::remove_file(mh::clearance_path(root, PR)).unwrap();
        }
    });
    let error = format!("{:#}", result.unwrap_err());
    assert!(
        error.contains("withdrawn by a concurrent release"),
        "{error}"
    );
    assert!(mh::hold_path(root, PR).exists(), "the hold stays armed");
    assert!(mh::unrecorded_marker_removals(root).is_empty());
}
