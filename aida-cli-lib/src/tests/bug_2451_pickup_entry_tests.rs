//! Exercise session setup, including the queue's resolved-PR handoff.
// trace:BUG-2451 | ai:codex
use super::tests::{fixture, git_ok};
use crate::session_start::{session_start_with_receipt, PathReentry};
use std::path::Path;

fn start(
    scope: &str,
    path: &Path,
    contained: bool,
    resolved: Option<Option<(u64, String)>>,
) -> anyhow::Result<crate::session_start::SessionStartReceipt> {
    session_start_with_receipt(
        scope,
        None,
        None,
        false,
        path.to_str(),
        None,
        "auto",
        false,
        None,
        false,
        None,
        None,
        false,
        Some("implementer".into()),
        false,
        Some(false),
        true,
        None,
        contained,
        false,
        PathReentry::None,
        resolved,
    )
}

fn forge_fixture(main: &Path, root: &Path, reply: &str) -> std::path::PathBuf {
    std::fs::create_dir_all(main.join(".aida")).unwrap();
    std::fs::write(
        main.join(".aida/config.toml"),
        "[forge]\nprovider = \"github\"\n",
    )
    .unwrap();
    git_ok(main, &["init", "-q", ".aida-store"]);
    let gh = root.join("fake-gh");
    crate::test_exec::write_executable(&gh, format!(
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'gh test'; exit 0; fi\nif [ \"$1 $2\" = 'pr list' ]; then printf '%s\\n' '{reply}'; exit 0; fi\nexit 1\n"
    ));
    gh
}

#[test]
fn bug_2451_real_session_refuses_a_held_head_in_worktree_and_clone_modes() {
    let fx = fixture();
    let mut ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let gh = forge_fixture(
        &fx.main,
        &fx.root,
        "42\tfix (BUG-2451)\thttps://example.invalid/42\toriginal-pr",
    );
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    let holder = fx.root.join("holder");
    git_ok(
        &fx.main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "original-pr",
            holder.to_str().unwrap(),
        ],
    );
    for contained in [false, true] {
        let error = start("BUG-2451", &fx.root.join("new-seat"), contained, None)
            .err()
            .expect("a held PR head must refuse")
            .to_string();
        assert!(error.contains(&holder.display().to_string()), "{error}");
        assert!(error.contains("held by worktree"), "{error}");
        assert!(!fx.root.join("new-seat").exists());
        assert!(crate::list_leases(&fx.main).is_empty());
        assert!(!super::tests::ref_exists(&fx.main, "refs/heads/bug-2451-2"));
    }
}

#[test]
fn bug_2451_real_session_seats_origin_only_head_after_stale_seat_removal() {
    let fx = fixture();
    let mut ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let gh = forge_fixture(
        &fx.main,
        &fx.root,
        "42\tgeneric fix\thttps://example.invalid/42\toriginal-pr",
    );
    // The hold remains positive association evidence even with generic metadata.
    use crate::gate::merge_hold_core as mh;
    let mut hold = mh::typed_hold(42, mh::HoldReasonKind::Rework, "rework", None);
    hold.spec = Some("BUG-2451".into());
    mh::place_hold(&fx.main, &hold).unwrap();
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    let old = fx.root.join("old-seat");
    git_ok(
        &fx.main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "original-pr",
            old.to_str().unwrap(),
        ],
    );
    std::fs::write(old.join("original-work"), "must survive\n").unwrap();
    git_ok(&old, &["add", "original-work"]);
    git_ok(
        &old,
        &[
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "commit",
            "-qm",
            "original fix (BUG-2451)",
        ],
    );
    let head = git_ok(&old, &["rev-parse", "HEAD"]);
    git_ok(
        &old,
        &[
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "push",
            "-q",
            "origin",
            "HEAD:original-pr",
        ],
    );
    git_ok(&fx.main, &["worktree", "remove", old.to_str().unwrap()]);
    git_ok(&fx.main, &["branch", "-D", "original-pr"]);
    let seated = fx.root.join("new-seat");
    let mut receipt = start("BUG-2451", &seated, false, None).unwrap();
    receipt.disarm();
    assert_eq!(git_ok(&seated, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        git_ok(&seated, &["branch", "--show-current"]),
        "original-pr"
    );
    assert_eq!(
        std::fs::read_to_string(seated.join("original-work")).unwrap(),
        "must survive\n"
    );
    assert_eq!(crate::list_leases(&fx.main)[0].branch, "original-pr");
}

#[test]
fn bug_2451_real_session_skips_forge_for_broad_scopes_and_resolved_queue_pickup() {
    for scope in ["feature:x", "src/**", "EPIC-2451", "BUG-2451"] {
        let fx = fixture();
        let mut ambient =
            crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
        let gh = forge_fixture(&fx.main, &fx.root, "");
        crate::test_exec::write_executable(&gh, "#!/bin/sh\nexit 1\n");
        ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
        let resolved = (scope == "BUG-2451").then_some(None);
        let mut receipt = start(scope, &fx.root.join("seat"), false, resolved).unwrap();
        receipt.disarm();
    }
}

#[test]
fn bug_2451_queue_workspace_dates_confirmed_merged_retained_branch() {
    let fx = fixture();
    let mut ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let gh = forge_fixture(&fx.main, &fx.root, "");
    crate::test_exec::write_executable(
        &gh,
        r#"#!/bin/sh
if [ "$1" = --version ]; then echo 'gh test'; exit 0; fi
case "$*" in
  *'pr list'*'--state merged'*) printf '88\tmerged fix\thttps://example.invalid/88\tbug-2451\n';;
esac
exit 0
"#,
    );
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    git_ok(&fx.main, &["branch", "bug-2451"]);
    // The ordinary queue-work resolver must allocate a dated branch without
    // operator flags, while leaving the retained branch intact.
    let (_, planned) =
        crate::session_start::resolve_pickup_workspace(&fx.main, "BUG-2451", "auto").unwrap();
    assert!(planned.starts_with("bug-2451-"), "{planned}");
    let (storage, _) = super::orphan_reentry_tests::queued_scope(&fx.main, "BUG-2451");
    queue_work(&storage, false).unwrap();
    let leases = crate::list_leases(&fx.main);
    assert_eq!(leases.len(), 1);
    let seated = leases[0].worktree_path.clone();
    assert_eq!(git_ok(&seated, &["branch", "--show-current"]), planned);
    assert!(super::tests::ref_exists(&fx.main, "refs/heads/bug-2451"));
    use crate::auto_complete::PhaseDriver;
    let driver = crate::RealPhaseDriver::new(
        fx.main.clone(),
        "BUG-2451".into(),
        "test".into(),
        None,
        true,
        None,
        crate::AutonomyMode::Default,
        "test-token".into(),
        false,
        false,
        false,
        false,
        crate::auto_complete::LifecycleSkip::none(),
        crate::auto_complete::AutoCompleteVariant::ThroughCi,
    );
    let (path, branch) = driver.implementer_workspace().unwrap().unwrap();
    assert_eq!(
        Path::new(&path).canonicalize().unwrap(),
        seated.canonicalize().unwrap()
    );
    assert!(branch.starts_with("bug-2451-"), "{branch}");
}

// trace:BUG-2451 | ai:codex
fn queue_work(storage: &crate::Storage, list_sessions: bool) -> anyhow::Result<()> {
    queue_work_sync(storage, list_sessions, true)
}

// trace:BUG-2501 | ai:codex
fn queue_work_sync(
    storage: &crate::Storage,
    list_sessions: bool,
    no_pull: bool,
) -> anyhow::Result<()> {
    crate::queue_cmd::handle_queue_work(
        storage,
        None,
        crate::test_env::HERMETIC_TEST_USER,
        Some("BUG-2451"),
        None,
        false,
        /* no_launch */ true,
        crate::queue_cmd::QueueWorkRoleInput::direct(Some("implementer")),
        no_pull,
        None,
        None,
        None,
        false,
        None,
        false,
        false,
        false,
        /* force_claim */ false,
        None,
        false,
        list_sessions,
        None,
        "claude",
        false,
        crate::phase_driver::AutonomyMode::Default,
        None,
        false,
        false,
        false,
        None,
        None,
        None,
        false,
        false,
        false,
        false,
        None,
    )
}

// trace:BUG-2451 | ai:codex
#[test]
fn bug_2451_queue_list_sessions_never_contacts_offline_forge() {
    let fx = fixture();
    let mut ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let gh = forge_fixture(&fx.main, &fx.root, "");
    crate::test_exec::write_executable(
        &gh,
        "#!/bin/sh\necho contacted > \"${0%/*}/contacted\"\nexit 1\n",
    );
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    let (storage, _) = super::orphan_reentry_tests::queued_scope(&fx.main, "BUG-2451");
    queue_work(&storage, true).unwrap();
    assert!(!fx.root.join("contacted").exists());
    assert!(crate::list_leases(&fx.main).is_empty());
}

// trace:BUG-2451 | ai:codex
#[test]
fn bug_2451_retained_branch_requires_confirmed_absence_of_open_pr() {
    for open_reply in [
        "echo offline >&2; exit 1",
        "printf '99\\topen fix\\thttps://example.invalid/99\\tbug-2451\\n'",
    ] {
        let fx = fixture();
        let mut ambient =
            crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
        let gh = forge_fixture(&fx.main, &fx.root, "");
        crate::test_exec::write_executable(
            &gh,
            format!(
                r#"#!/bin/sh
if [ "$1" = --version ]; then echo 'gh test'; exit 0; fi
case "$*" in
  *'--state merged'*) printf '88\tmerged fix\thttps://example.invalid/88\tbug-2451\n';;
  *) {open_reply} ;;
esac
"#
            ),
        );
        ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
        git_ok(&fx.main, &["branch", "bug-2451"]);
        let error = crate::session_start::resolve_session_branch(&fx.main, "bug-2451", "auto")
            .unwrap_err()
            .to_string();
        assert!(error.contains("pass --branch explicitly"), "{error}");
        assert!(!error.contains("--branch-style"), "{error}");
    }
}

// trace:BUG-2501 | ai:codex
fn bug_2501_private_allocation_setting(main: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let store = main.join(".aida-store");
    crate::test_env::enroll_synthetic_home(&store).unwrap();
    let common = aida_core::allocation_authority::common_dir(&store).unwrap();
    std::fs::set_permissions(
        common.join(aida_core::allocation_authority::CONFIG_FILE),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
}

// trace:BUG-2501 | ai:codex
#[test]
fn bug_2501_pull_failure_stops_pickup_before_implicit_queue_write() {
    let fx = fixture();
    let mut ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let gh = forge_fixture(&fx.main, &fx.root, "");
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    bug_2501_private_allocation_setting(&fx.main);
    let (storage, _) = super::orphan_reentry_tests::queued_scope(&fx.main, "BUG-2451");
    let user = crate::test_env::HERMETIC_TEST_USER;
    let row = storage.queue_list(user, false).unwrap().remove(0);
    storage.queue_remove(user, &row.requirement_id).unwrap();
    let before = storage
        .load()
        .unwrap()
        .get_requirement_by_spec_id("BUG-2451")
        .unwrap()
        .clone();
    let fake = fx.root.join("failing-aida");
    crate::test_exec::write_executable(&fake, "#!/bin/sh\nexit 42\n");
    let _exe = crate::kernel::test_aida_exe::set(fake);
    let error = queue_work_sync(&storage, false, false).unwrap_err();
    assert!(
        error.to_string().contains("pre-pickup pull failed"),
        "{error:#}"
    );
    assert!(storage.queue_list(user, false).unwrap().is_empty());
    let after = storage
        .load()
        .unwrap()
        .get_requirement_by_spec_id("BUG-2451")
        .unwrap()
        .clone();
    assert_eq!(before.status, after.status);
    assert_eq!(before.history, after.history);
    assert!(crate::list_leases(&fx.main).is_empty());
}

// trace:BUG-2501 | ai:codex
#[test]
fn bug_2501_burndown_residual_refuses_failed_pull() {
    let fx = fixture();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    forge_fixture(&fx.main, &fx.root, "");
    bug_2501_private_allocation_setting(&fx.main);
    let (storage, _) = super::orphan_reentry_tests::queued_scope(&fx.main, "BUG-2451");
    std::fs::write(
        fx.main.join(".aida/config.toml"),
        "store_path = '.aida-store'\n",
    )
    .unwrap();
    git_ok(storage.path(), &["add", "."]);
    git_ok(
        storage.path(),
        &[
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-qm",
            "fixture",
        ],
    );
    git_ok(
        storage.path(),
        &[
            "remote",
            "add",
            "origin",
            fx.root.join("missing.git").to_str().unwrap(),
        ],
    );
    let before = storage
        .queue_list(crate::test_env::HERMETIC_TEST_USER, false)
        .unwrap();
    let initial = crate::burndown_run::handle_burndown_run(
        "approved", None, None, None, None, None, true, false, false, true, None, None, false,
    )
    .unwrap_err();
    assert!(
        initial.to_string().contains("refusing to continue"),
        "{initial:#}"
    );
    let error =
        crate::burndown_run::probe_burndown_residual(&fx.main, "approved", None, None, &[], true)
            .unwrap_err();
    assert!(
        error.to_string().contains("refusing to continue"),
        "{error:#}"
    );
    assert_eq!(
        storage
            .queue_list(crate::test_env::HERMETIC_TEST_USER, false)
            .unwrap()
            .len(),
        before.len()
    );
}

// Recovery exercises ordinary configured store discovery, unlike setup-only
// pickup fixtures which intentionally pass an unattached store explicitly.
// trace:BUG-2549 | ai:codex
fn recovery_forge_fixture(main: &Path, root: &Path) -> std::path::PathBuf {
    let gh = forge_fixture(main, root, "");
    std::fs::write(main.join(".aida/config.toml"),
        "[forge]\nprovider = \"github\"\n[deployment]\nmode = \"distributed\"\nstore_path = \".aida-store\"\n").unwrap();
    gh
}

// trace:BUG-2549 | ai:codex
#[test]
fn bug_2549_queue_resumes_prless_canonical_head_without_force() {
    let fx = fixture();
    let mut ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let gh = recovery_forge_fixture(&fx.main, &fx.root);
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    git_ok(&fx.main, &["checkout", "-qb", "bug-2451"]);
    std::fs::write(fx.main.join("unfinished"), "keep this implementation\n").unwrap();
    git_ok(&fx.main, &["add", "unfinished"]);
    git_ok(
        &fx.main,
        &[
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "commit",
            "-qm",
            "unfinished implementation (Refs BUG-2451)",
        ],
    );
    let head = git_ok(&fx.main, &["rev-parse", "bug-2451"]);
    git_ok(&fx.main, &["checkout", "main"]);
    let (storage, _) = super::orphan_reentry_tests::queued_scope(&fx.main, "BUG-2451");
    storage
        .update_atomically(|store| {
            store
                .get_requirement_by_spec_id_mut("BUG-2451")
                .unwrap()
                .status = aida_core::RequirementStatus::InProgress;
        })
        .unwrap();
    queue_work(&storage, false).unwrap();
    let leases = crate::list_leases(&fx.main);
    assert_eq!(leases.len(), 1);
    assert_eq!(leases[0].branch, "bug-2451");
    assert_eq!(
        git_ok(&leases[0].worktree_path, &["rev-parse", "HEAD"]),
        head
    );
    assert_eq!(
        std::fs::read_to_string(leases[0].worktree_path.join("unfinished")).unwrap(),
        "keep this implementation\n"
    );
    assert!(!super::tests::ref_exists(&fx.main, "refs/heads/bug-2451-2"));
    let store = storage.load().unwrap();
    let req = store.get_requirement_by_spec_id("BUG-2451").unwrap();
    assert!(req
        .comments
        .iter()
        .any(|comment| comment.content.contains(&head)));
}

// trace:BUG-2549 | ai:codex
#[test]
fn bug_2549_queue_refuses_dirty_canonical_worktree_without_status_mutation() {
    let fx = fixture();
    let mut ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let gh = recovery_forge_fixture(&fx.main, &fx.root);
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    let held = fx.root.join("held");
    git_ok(
        &fx.main,
        &["worktree", "add", "-qb", "bug-2451", held.to_str().unwrap()],
    );
    std::fs::write(held.join("pending"), "retain me").unwrap();
    let (storage, _) = super::orphan_reentry_tests::queued_scope(&fx.main, "BUG-2451");
    let error = queue_work(&storage, false).unwrap_err().to_string();
    assert!(error.contains("held by worktree"), "{error}");
    assert!(crate::list_leases(&fx.main).is_empty());
    assert_eq!(
        std::fs::read_to_string(held.join("pending")).unwrap(),
        "retain me"
    );
    assert_eq!(
        storage
            .load()
            .unwrap()
            .get_requirement_by_spec_id("BUG-2451")
            .unwrap()
            .status,
        aida_core::RequirementStatus::Approved
    );
}

// trace:BUG-2549 | ai:codex
#[test]
fn bug_2549_reference_merge_returns_unheld_spec_and_preserves_open_pr_claims() {
    let fx = fixture();
    let mut ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let gh = recovery_forge_fixture(&fx.main, &fx.root);
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    let (storage, set_status) = super::orphan_reentry_tests::queued_scope(&fx.main, "BUG-2451");
    set_status(aida_core::RequirementStatus::InProgress);
    crate::lane_recovery::reconcile_reference(&fx.main, "BUG-2451", "merged-slice-sha").unwrap();
    let store = storage.load().unwrap();
    let req = store.get_requirement_by_spec_id("BUG-2451").unwrap();
    assert_eq!(req.status, aida_core::RequirementStatus::Approved);
    assert!(req
        .comments
        .iter()
        .any(|c| c.content.contains("merged-slice-sha")));
    for reply in [
        "printf '42\\tfix (BUG-2451)\\thttps://example.invalid/42\\tbug-2451\\n'",
        "exit 1",
    ] {
        crate::test_exec::write_executable(&gh, format!("#!/bin/sh\n{reply}\n"));
        set_status(aida_core::RequirementStatus::InProgress);
        let observation =
            crate::lane_recovery::reconcile_reference(&fx.main, "BUG-2451", "merged-slice-sha");
        if reply == "exit 1" {
            assert!(observation.is_err());
        } else {
            observation.unwrap();
        }
        assert_eq!(
            storage
                .load()
                .unwrap()
                .get_requirement_by_spec_id("BUG-2451")
                .unwrap()
                .status,
            aida_core::RequirementStatus::InProgress
        );
    }
}

// trace:BUG-2549 | ai:codex
#[test]
fn bug_2549_prless_lane_records_lease_head_then_resets_after_release() {
    let fx = fixture();
    let mut ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let gh = recovery_forge_fixture(&fx.main, &fx.root);
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    let (storage, set_status) = super::orphan_reentry_tests::queued_scope(&fx.main, "BUG-2451");
    queue_work(&storage, false).unwrap();
    // The setup-only fixture has no attached CLI backend for the launch bump.
    set_status(aida_core::RequirementStatus::InProgress);
    let lease = crate::list_leases(&fx.main).remove(0);
    let head = git_ok(&lease.worktree_path, &["rev-parse", "HEAD"]);
    crate::lane_recovery::reconcile(&fx.main, "BUG-2451", &lease.branch, None).unwrap();
    assert_eq!(
        storage
            .load()
            .unwrap()
            .get_requirement_by_spec_id("BUG-2451")
            .unwrap()
            .status,
        aida_core::RequirementStatus::InProgress
    );
    crate::lane_recovery::reconcile_reference(&fx.main, "BUG-2451", "merged-slice-sha").unwrap();
    assert_eq!(
        storage
            .load()
            .unwrap()
            .get_requirement_by_spec_id("BUG-2451")
            .unwrap()
            .status,
        aida_core::RequirementStatus::InProgress
    );
    std::fs::remove_file(crate::lease_path(&fx.main, &lease.id)).unwrap();
    crate::lane_recovery::reconcile(&fx.main, "BUG-2451", &lease.branch, Some(&head)).unwrap();
    let store = storage.load().unwrap();
    let req = store.get_requirement_by_spec_id("BUG-2451").unwrap();
    assert_eq!(req.status, aida_core::RequirementStatus::Approved);
    assert!(req
        .comments
        .iter()
        .any(|c| c.content.contains(&head) && c.content.contains(&lease.branch)));
}

// trace:BUG-2549 | ai:codex
#[test]
fn bug_2549_canonical_branch_with_foreign_lease_refuses() {
    let fx = fixture();
    let mut ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let gh = recovery_forge_fixture(&fx.main, &fx.root);
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    let (storage, _) = super::orphan_reentry_tests::queued_scope(&fx.main, "BUG-2451");
    queue_work(&storage, false).unwrap();
    let mut lease = crate::list_leases(&fx.main).remove(0);
    lease.scope = "BUG-9999".into();
    std::fs::write(
        crate::lease_path(&fx.main, &lease.id),
        toml::to_string(&lease).unwrap(),
    )
    .unwrap();
    let error = crate::queue_cmd::canonical_resume_branch(&fx.main, "BUG-2451", true)
        .unwrap_err()
        .to_string();
    assert!(error.contains("still has a lease"), "{error}");
    assert_eq!(crate::list_leases(&fx.main)[0].scope, "BUG-9999");
}

// trace:BUG-2549 | ai:codex
#[test]
fn bug_2549_shared_claim_and_corrupt_registry_retain_in_progress() {
    let fx = fixture();
    let mut ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let gh = recovery_forge_fixture(&fx.main, &fx.root);
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    let (storage, set_status) = super::orphan_reentry_tests::queued_scope(&fx.main, "BUG-2451");
    let dir = fx.main.join(".aida-store/coordination/leases");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("foreign.toml");
    for body in [
        "scope = 'BUG-2451'\nnode_id = 'foreign'\nclone_path = '/foreign/clone'\nhost = 'other-host'\npid = 1\nagent = 'implementer'\nstarted_at = '2026-11-13T00:00:00Z'\nheartbeat_at = '2026-11-13T00:00:00Z'\nttl_secs = 1800\n",
        "torn = ",
    ] {
        std::fs::write(&path, body).unwrap();
        set_status(aida_core::RequirementStatus::InProgress);
        crate::lane_recovery::reconcile_observed(&fx.main, "BUG-2451", "bug-2451", Some("exit-head")).unwrap();
        crate::lane_recovery::reconcile_reference(&fx.main, "BUG-2451", "merge").unwrap();
        let store = storage.load().unwrap();
        let req = store.get_requirement_by_spec_id("BUG-2451").unwrap();
        assert_eq!(req.status, aida_core::RequirementStatus::InProgress);
        assert!(req.comments.iter().any(|c| c.content.contains("exit-head")));
    }
    std::fs::remove_file(path).unwrap();
    crate::lane_recovery::reconcile_reference(&fx.main, "BUG-2451", "merge").unwrap();
    assert_eq!(
        storage
            .load()
            .unwrap()
            .get_requirement_by_spec_id("BUG-2451")
            .unwrap()
            .status,
        aida_core::RequirementStatus::Approved
    );
}

// trace:BUG-2549 | ai:codex
#[test]
fn bug_2549_exit_and_reference_recovery_use_legacy_store() {
    for file in ["requirements.yaml", "requirements.db", "selected.yaml"] {
        let fx = fixture();
        let mut ambient =
            crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
        let gh = forge_fixture(&fx.main, &fx.root, "");
        ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
        std::fs::remove_dir_all(fx.main.join(".aida-store")).unwrap();
        let storage = aida_core::Storage::new(fx.main.join(file));
        let mut store = aida_core::RequirementsStore::default();
        let mut req = aida_core::Requirement::new("retry".into(), String::new());
        req.spec_id = Some("BUG-2451".into());
        req.status = aida_core::RequirementStatus::InProgress;
        store.requirements.push(req);
        storage.save(&store).unwrap();
        if file == "selected.yaml" {
            crate::lane_recovery::reconcile_in(
                &fx.main,
                &storage,
                "BUG-2451",
                "deleted-branch",
                Some("captured-before-removal"),
            )
            .unwrap();
        } else {
            crate::lane_recovery::reconcile_observed(
                &fx.main,
                "BUG-2451",
                "deleted-branch",
                Some("captured-before-removal"),
            )
            .unwrap();
        }
        let recovered = storage.load().unwrap();
        let req = recovered.get_requirement_by_spec_id("BUG-2451").unwrap();
        assert_eq!(req.status, aida_core::RequirementStatus::Approved);
        assert!(req
            .comments
            .iter()
            .any(|c| c.content.contains("captured-before-removal")));
        storage
            .update_atomically(|store| {
                store
                    .get_requirement_by_spec_id_mut("BUG-2451")
                    .unwrap()
                    .status = aida_core::RequirementStatus::InProgress
            })
            .unwrap();
        if file == "selected.yaml" {
            crate::lane_recovery::reconcile_reference_in(
                &fx.main,
                &storage,
                "BUG-2451",
                "legacy-merge",
            )
            .unwrap();
        } else {
            crate::lane_recovery::reconcile_reference(&fx.main, "BUG-2451", "legacy-merge")
                .unwrap();
        }
        assert_eq!(
            storage
                .load()
                .unwrap()
                .get_requirement_by_spec_id("BUG-2451")
                .unwrap()
                .status,
            aida_core::RequirementStatus::Approved
        );
        assert!(!fx.main.join(".aida-store").exists());
    }
}

// trace:BUG-2549 | ai:codex
#[test]
fn bug_2549_post_merge_commits_resume_canonical_branch() {
    let fx = fixture();
    let mut ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let gh = recovery_forge_fixture(&fx.main, &fx.root);
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    git_ok(&fx.main, &["branch", "bug-2451"]);
    let merged_head = git_ok(&fx.main, &["rev-parse", "bug-2451"]);
    crate::test_exec::write_executable(
        &gh,
        format!(
            r#"#!/bin/sh
if [ "$1" = --version ]; then echo 'gh test'; exit 0; fi
if [ "$1 $2" = 'pr list' ]; then
  case " $* " in
    *' --state merged '*) printf '42\tRefs BUG-2451\thttps://example.invalid/42\tbug-2451\n';;
  esac
  exit 0
fi
if [ "$1 $2" = 'pr view' ]; then
  printf '%s\n' '{{"state":"MERGED","headRefOid":"{merged_head}","headRefName":"bug-2451","baseRefName":"main"}}'
  exit 0
fi
exit 1
"#
        ),
    );
    assert_eq!(
        crate::queue_cmd::canonical_resume_branch(&fx.main, "BUG-2451", true).unwrap(),
        None
    );
    git_ok(&fx.main, &["checkout", "bug-2451"]);
    git_ok(
        &fx.main,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "--allow-empty",
            "-qm",
            "unfinished after merge",
        ],
    );
    git_ok(&fx.main, &["checkout", "main"]);
    let (storage, _) = super::orphan_reentry_tests::queued_scope(&fx.main, "BUG-2451");
    let head = git_ok(&fx.main, &["rev-parse", "bug-2451"]);
    queue_work(&storage, false).unwrap();
    let lease = crate::list_leases(&fx.main).remove(0);
    assert_eq!(lease.branch, "bug-2451");
    assert_eq!(git_ok(&lease.worktree_path, &["rev-parse", "HEAD"]), head);
}

// trace:BUG-2549 | ai:codex
#[test]
fn bug_2549_detached_worktree_exit_captures_actual_head() {
    let fx = fixture();
    let mut ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let gh = recovery_forge_fixture(&fx.main, &fx.root);
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    let (storage, _) = super::orphan_reentry_tests::queued_scope(&fx.main, "BUG-2451");
    queue_work(&storage, false).unwrap();
    let lease = crate::list_leases(&fx.main).remove(0);
    git_ok(&lease.worktree_path, &["checkout", "--detach"]);
    git_ok(
        &lease.worktree_path,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "--allow-empty",
            "-qm",
            "detached unfinished work",
        ],
    );
    let head = git_ok(&lease.worktree_path, &["rev-parse", "HEAD"]);
    assert_ne!(git_ok(&fx.main, &["rev-parse", &lease.branch]), head);
    assert_eq!(
        crate::lane_recovery::exit_head(&fx.main, &lease.branch, &[lease.clone()]).unwrap(),
        Some(head.clone())
    );
    git_ok(
        &fx.main,
        &["worktree", "remove", lease.worktree_path.to_str().unwrap()],
    );
    assert_eq!(
        crate::lane_recovery::exit_head(&fx.main, &lease.branch, &[lease.clone()]).unwrap(),
        Some(git_ok(&fx.main, &["rev-parse", &lease.branch]))
    );
}

// trace:BUG-2549 | ai:codex
#[test]
fn bug_2549_unavailable_forge_preserves_exit_evidence() {
    let fx = fixture();
    let mut ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let gh = recovery_forge_fixture(&fx.main, &fx.root);
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    let (storage, set_status) = super::orphan_reentry_tests::queued_scope(&fx.main, "BUG-2451");
    set_status(aida_core::RequirementStatus::InProgress);
    crate::test_exec::write_executable(
        &gh,
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'gh test'; exit 0; fi\nexit 1\n",
    );
    assert!(crate::lane_recovery::reconcile_observed(
        &fx.main,
        "BUG-2451",
        "bug-2451",
        Some("detached-exit-head")
    )
    .is_err());
    let store = storage.load().unwrap();
    let req = store.get_requirement_by_spec_id("BUG-2451").unwrap();
    assert_eq!(req.status, aida_core::RequirementStatus::InProgress);
    assert!(req.comments.iter().any(|c| c.content.contains("bug-2451")
        && c.content.contains("detached-exit-head")
        && !c.content.contains("no open PR")));
}
