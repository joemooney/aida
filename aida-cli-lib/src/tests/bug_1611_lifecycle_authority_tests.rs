// BUG-1611: the lifecycle authority guard at the three write sites the
// STORY-1217 advisor review found skipping it:
//   1. `aida queue done` (CLI handler + MCP `queue_done`, one shared decision),
//   2. the forced reopen `aida edit X --status approved --force` on a closed spec,
//   3. `aida zen <draft>` under `[autopilot] approve = "auto"`.
// Each site gets a refusal case and a legitimate-path pass. Every store here
// is a throwaway fixture under a tempdir; none touches a real AIDA store.
// trace:BUG-1611 | ai:claude

use crate::{
    advisor_authority_from, handle_queue_command, queue_done_lifecycle_refusal,
    queue_done_lifecycle_refusal_message, status_advance_requires_advisor_authority,
    zen_auto_approve, zen_auto_approve_authorized, QueueCommand, QueueDoneLifecycleRefusal,
};
use aida_core::db::DatabaseBackend;
use aida_core::models::{Requirement, RequirementStatus as S};
use aida_core::{CachedGitBackend, Storage};

// ── Site 1: `queue done` ────────────────────────────────────────────────────

#[test]
fn queue_done_refuses_unapproved_without_authority_and_passes_approved_work() {
    // Refusal: never approved, or punted back for triage, and no authority.
    for from in [S::Draft, S::NeedsAttention] {
        assert_eq!(
            queue_done_lifecycle_refusal(&from, false),
            Some(QueueDoneLifecycleRefusal::NotApproved),
            "{from} -> Done must need approval authority"
        );
    }
    // Refusal: closed specs, whatever the caller's authority.
    for from in [S::Rejected, S::Completed, S::Superseded] {
        for authority in [false, true] {
            assert_eq!(
                queue_done_lifecycle_refusal(&from, authority),
                Some(QueueDoneLifecycleRefusal::Closed),
                "{from} -> Done must be refused (authority={authority})"
            );
        }
    }
    // Legitimate path: the drain's own `queue done` after implementing an
    // Approved / Planned / In Progress spec, and an idempotent re-run. These
    // pass with NO authority, which is what a headless implementer holds.
    for from in [S::Approved, S::Planned, S::InProgress, S::Done] {
        assert_eq!(
            queue_done_lifecycle_refusal(&from, false),
            None,
            "{from} -> Done is an execution flip"
        );
    }
    // With authority (interactive human, advisor seat, live orchestrator) the
    // lifecycle guard lets Draft through, as it does for `aida edit`.
    assert_eq!(queue_done_lifecycle_refusal(&S::Draft, true), None);
}

#[test]
fn queue_done_refusal_text_is_neutral() {
    for (from, refusal) in [
        (S::Draft, QueueDoneLifecycleRefusal::NotApproved),
        (S::Rejected, QueueDoneLifecycleRefusal::Closed),
    ] {
        let msg = queue_done_lifecycle_refusal_message("BUG-9", &from, refusal);
        assert!(msg.contains("queue done refused"), "{msg}");
        assert!(msg.contains("BUG-9"), "{msg}");
        assert!(
            !msg.contains("AIDA_SESSION_ROLE"),
            "refusals must not suggest an env-var role override: {msg}"
        );
    }
}

fn seed_yaml_store(status: S) -> (tempfile::TempDir, std::path::PathBuf, Storage) {
    let tmp = tempfile::TempDir::new().unwrap();
    let store_path = tmp.path().join("requirements.yaml");
    let storage = Storage::new(&store_path);
    let mut store = aida_core::RequirementsStore::default();
    let mut r = Requirement::new("Closed spec".into(), "desc".into());
    r.spec_id = Some("BUG-9001".into());
    r.status = status;
    store.requirements.push(r);
    storage.save(&store).unwrap();
    (tmp, store_path, storage)
}

fn done_cmd(id: &str, force: bool) -> QueueCommand {
    QueueCommand::Done {
        id: id.into(),
        user: Some("bug-1611-fixture".into()),
        yes: true,
        force,
        skip_pr_check: true,
        interface_cli: vec![],
        interface_mcp: vec![],
        interface_tui: vec![],
        interface_other: vec![],
        no_interface_change: true,
        test_plan: vec![],
        no_test_plan: true,
    }
}

/// The real CLI handler refuses a Rejected spec before any write, even with
/// `--force`, and leaves the status untouched. (Rejected is refused whatever
/// the caller's authority, so this handler-level check does not depend on
/// whether the test process has a TTY.)
#[test]
fn cli_queue_done_handler_refuses_a_rejected_spec_even_with_force() {
    let (_tmp, store_path, storage) = seed_yaml_store(S::Rejected);
    let err = handle_queue_command(&done_cmd("BUG-9001", true), &storage, &store_path)
        .expect_err("queue done on a Rejected spec must refuse");
    let msg = err.to_string();
    assert!(msg.contains("queue done refused"), "{msg}");
    assert!(!msg.contains("AIDA_SESSION_ROLE"), "{msg}");
    let store = storage.load().unwrap();
    assert_eq!(
        store.get_requirement_by_spec_id("BUG-9001").unwrap().status,
        S::Rejected
    );
}

// ── Site 2: forced reopen via `aida edit --status approved --force` ─────────

#[test]
fn forced_reopen_runs_the_approval_authority_predicate() {
    // `--force` only clears the TASK-47 reopen guard; the edit path then asks
    // this predicate, which now names a closed source as an authority act.
    for from in [S::Rejected, S::Completed, S::Superseded] {
        assert!(
            status_advance_requires_advisor_authority(&from, &S::Approved),
            "{from} -> Approved is a reopen into the pipeline"
        );
        // Refusal: a headless implementer (no role, no TTY, not orchestrated).
        let authorized = advisor_authority_from("implementer", false, false);
        assert!(
            !(authorized || !status_advance_requires_advisor_authority(&from, &S::Approved)),
            "{from} -> Approved must refuse without authority"
        );
        // Legitimate path: an interactive human or the advisor seat.
        assert!(advisor_authority_from("implementer", true, false));
        assert!(advisor_authority_from("advisor", false, false));
    }
    // Closing and idempotent re-flips stay free, so `--force` recovery that
    // does not reopen into the pipeline is unaffected.
    assert!(!status_advance_requires_advisor_authority(
        &S::Completed,
        &S::Completed
    ));
    assert!(!status_advance_requires_advisor_authority(
        &S::Rejected,
        &S::Draft
    ));
    assert!(!status_advance_requires_advisor_authority(
        &S::Approved,
        &S::Rejected
    ));
}

// ── Site 3: `aida zen` auto-approve ─────────────────────────────────────────

fn git_store_with_draft() -> (tempfile::TempDir, std::path::PathBuf, Requirement) {
    let dir = tempfile::tempdir().unwrap();
    let store_root = dir.path().join("store");
    let cache_path = dir.path().join(".aida").join("cache.db");
    std::fs::create_dir_all(&store_root).unwrap();
    aida_core::git_ops::init(&store_root).unwrap();
    aida_core::git_ops::configure_user(&store_root, "fixture", "fixture@localhost").unwrap();
    let backend = CachedGitBackend::open(&store_root, &cache_path).unwrap();
    let mut r = Requirement::new("Zen draft".into(), "desc".into());
    r.spec_id = Some("TASK-9001".into());
    let added = backend.add_requirement(r).unwrap();
    assert_eq!(added.status, S::Draft);
    (dir, store_root, added)
}

fn status_on_disk(store_root: &std::path::Path, spec: &str) -> S {
    let cache_path = store_root.parent().unwrap().join(".aida").join("cache.db");
    let backend = CachedGitBackend::open(store_root, &cache_path).unwrap();
    backend
        .get_requirement_by_spec_id(spec)
        .unwrap()
        .unwrap()
        .status
}

#[test]
fn zen_auto_approve_predicate_needs_authority_for_a_draft() {
    assert!(!zen_auto_approve_authorized(&S::Draft, false));
    assert!(!zen_auto_approve_authorized(&S::NeedsAttention, false));
    assert!(zen_auto_approve_authorized(&S::Draft, true));
}

#[test]
fn zen_auto_approve_refuses_without_authority_and_approves_with_it() {
    // Refusal: `approve = "auto"` alone cannot flip the draft.
    let (_dir, store_root, draft) = git_store_with_draft();
    let err = zen_auto_approve(&store_root, &draft, false)
        .expect_err("auto-approve without authority must refuse");
    assert!(!err.to_string().contains("AIDA_SESSION_ROLE"), "{err}");
    assert_eq!(status_on_disk(&store_root, "TASK-9001"), S::Draft);

    // Legitimate path: the same write with approval authority lands Approved.
    zen_auto_approve(&store_root, &draft, true).expect("authorized auto-approve");
    assert_eq!(status_on_disk(&store_root, "TASK-9001"), S::Approved);
}
