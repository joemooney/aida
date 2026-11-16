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
    queue_work_with(storage, list_sessions, no_pull, None, false)
}

// trace:STORY-2768 | ai:claude
fn queue_work_with(
    storage: &crate::Storage,
    list_sessions: bool,
    no_pull: bool,
    branch: Option<&str>,
    over_wip: bool,
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
        /* branch_override */ branch,
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
        over_wip,
    )
}

/// STORY-2768 S3b: a fresh snapshot saying `open` changes are open.
// trace:STORY-2768 | ai:claude
fn story_2768_snapshot(main: &Path, open: usize) {
    let dir = main.join(".aida").join("cache");
    std::fs::create_dir_all(&dir).unwrap();
    let snapshot = serde_json::json!({
        "version": 1,
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "pass_seconds": 1,
        "open_prs": open,
        "wip_max": 10,
        "main": { "merges_today": 0 },
        "items": [],
        "queues": {
            "approved_not_started": 0,
            "awaiting_review": 0,
            "awaiting_rework": 0
        },
        "attention": []
    });
    std::fs::write(dir.join("now.json"), snapshot.to_string()).unwrap();
}

/// STORY-2768 S3b, through `handle_queue_work`: at the limit a new spec is
/// refused before anything is claimed; under it, with `--over-wip`, or on an
/// explicit `--branch`, the pickup goes on.
// trace:STORY-2768 | ai:claude
#[test]
fn story_2768_queue_work_refuses_a_new_spec_at_the_open_change_limit() {
    let gh_quiet = "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'gh test'; fi\nexit 0\n";
    let setup = |open: usize| {
        let fx = fixture();
        let mut ambient =
            crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
        let gh = forge_fixture(&fx.main, &fx.root, "");
        crate::test_exec::write_executable(&gh, gh_quiet);
        ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
        story_2768_snapshot(&fx.main, open);
        let (storage, _) = super::orphan_reentry_tests::queued_scope(&fx.main, "BUG-2451");
        (fx, ambient, storage)
    };

    // At the limit: refused, and nothing was claimed.
    let (fx, _ambient, storage) = setup(10);
    let refused = queue_work_with(&storage, false, true, None, false)
        .unwrap_err()
        .to_string();
    assert!(refused.contains("10 changes are open"), "{refused}");
    assert!(refused.contains("--over-wip"), "{refused}");
    assert!(crate::list_leases(&fx.main).is_empty(), "nothing claimed");
    // Listing sessions allocates nothing and is never limited.
    queue_work_with(&storage, true, true, None, false).unwrap();
    drop(_ambient);

    // The same count with --over-wip: the pickup takes its lease.
    let (fx, _ambient, storage) = setup(10);
    queue_work_with(&storage, false, true, None, true).unwrap();
    assert_eq!(crate::list_leases(&fx.main).len(), 1, "--over-wip");
    drop(_ambient);

    // Under the limit: no refusal.
    let (fx, _ambient, storage) = setup(9);
    queue_work_with(&storage, false, true, None, false).unwrap();
    assert_eq!(crate::list_leases(&fx.main).len(), 1, "under the limit");
    drop(_ambient);

    // An explicit --branch names existing work: exempt at the limit. Whatever
    // the pickup then decides about that branch, it is not the WIP refusal.
    let (fx, _ambient, storage) = setup(10);
    git_ok(&fx.main, &["branch", "bug-2451-existing"]);
    if let Err(error) = queue_work_with(&storage, false, true, Some("bug-2451-existing"), false) {
        let error = format!("{error:#}");
        assert!(!error.contains("changes are open"), "{error}");
    }
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
    std::fs::write(
        fx.main.join("post-merge-work"),
        "unfinished implementation\n",
    )
    .unwrap();
    git_ok(&fx.main, &["add", "post-merge-work"]);
    git_ok(
        &fx.main,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "commit",
            "-qm",
            "unfinished after merge",
        ],
    );
    git_ok(&fx.main, &["checkout", "main"]);
    let (storage, _) = super::orphan_reentry_tests::queued_scope(&fx.main, "BUG-2451");
    let head = git_ok(&fx.main, &["rev-parse", "bug-2451"]);
    assert_ne!(head, merged_head);
    // Keep origin at the merged head: local unfinished work takes precedence
    // over unchanged remote residue when selecting the canonical resume tip.
    git_ok(
        &fx.main,
        &["update-ref", "refs/remotes/origin/bug-2451", &merged_head],
    );
    // Historical merge evidence must remain harmless on repeated discovery.
    for _ in 0..2 {
        assert_eq!(
            crate::queue_cmd::canonical_resume_branch(&fx.main, "BUG-2451", true).unwrap(),
            Some("bug-2451".into())
        );
    }
    // Origin-only retained work also resumes, despite the historical merge.
    git_ok(
        &fx.main,
        &[
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "push",
            "-q",
            "origin",
            "bug-2451",
        ],
    );
    git_ok(&fx.main, &["branch", "-D", "bug-2451"]);
    assert_eq!(
        crate::queue_cmd::canonical_resume_branch(&fx.main, "BUG-2451", true).unwrap(),
        Some("bug-2451".into())
    );
    queue_work(&storage, false).unwrap();
    let lease = crate::list_leases(&fx.main).remove(0);
    assert_eq!(lease.branch, "bug-2451");
    assert_eq!(git_ok(&lease.worktree_path, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        std::fs::read_to_string(lease.worktree_path.join("post-merge-work")).unwrap(),
        "unfinished implementation\n"
    );
}

// Origin-only merged residue (no commits past the merged PR head) must not
// fail pickup: it allocates a fresh dated branch as main did before.
// trace:BUG-2549 | ai:claude
#[test]
fn bug_2549_merged_residue_allocates_a_fresh_branch() {
    let fx = fixture();
    let mut ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let gh = recovery_forge_fixture(&fx.main, &fx.root);
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    let merged_head = git_ok(&fx.main, &["rev-parse", "HEAD"]);
    git_ok(
        &fx.main,
        &[
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "push",
            "-q",
            "origin",
            "HEAD:refs/heads/bug-2451",
        ],
    );
    assert!(!super::tests::ref_exists(&fx.main, "refs/heads/bug-2451"));
    assert!(crate::branch_exists_anywhere(&fx.main, "bug-2451"));
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
    let (storage, _) = super::orphan_reentry_tests::queued_scope(&fx.main, "BUG-2451");
    queue_work(&storage, false).unwrap();
    let lease = crate::list_leases(&fx.main).remove(0);
    assert!(lease.branch.starts_with("bug-2451-"), "{}", lease.branch);
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
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "commit",
            "--allow-empty",
            "-qm",
            "detached unfinished work",
        ],
    );
    let head = git_ok(&lease.worktree_path, &["rev-parse", "HEAD"]);
    assert_ne!(git_ok(&fx.main, &["rev-parse", &lease.branch]), head);
    // A broken live HEAD must remain unknown, never fall back to the lease branch.
    let git_dir = git_ok(&lease.worktree_path, &["rev-parse", "--absolute-git-dir"]);
    let head_path = Path::new(&git_dir).join("HEAD");
    let saved_head = std::fs::read(&head_path).unwrap();
    std::fs::write(&head_path, "ref: refs/heads/missing-exit-head\n").unwrap();
    assert_eq!(
        crate::lane_recovery::exit_head(&fx.main, &lease.branch, &[lease.clone()]).unwrap(),
        None
    );
    std::fs::write(&head_path, saved_head).unwrap();
    let captured =
        crate::lane_recovery::exit_head(&fx.main, &lease.branch, &[lease.clone()]).unwrap();
    assert_eq!(captured, Some(head.clone()));
    // Detached commits are not reachable from the recorded lease branch.
    assert_eq!(
        git_ok(
            &lease.worktree_path,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "--contains",
                &head,
                "refs/heads/"
            ],
        ),
        ""
    );
    // Switching away from the lease branch must use the live tree as well.
    git_ok(&lease.worktree_path, &["checkout", "-b", "switched-exit"]);
    assert_eq!(
        crate::lane_recovery::exit_head(&fx.main, &lease.branch, &[lease.clone()]).unwrap(),
        captured
    );
    git_ok(
        &fx.main,
        &["worktree", "remove", lease.worktree_path.to_str().unwrap()],
    );
    assert_eq!(
        crate::lane_recovery::exit_head(&fx.main, &lease.branch, &[lease.clone()]).unwrap(),
        Some(git_ok(&fx.main, &["rev-parse", &lease.branch]))
    );
    // Match session end: reconcile the captured facts after tree/lease removal.
    std::fs::remove_file(crate::lease_path(&fx.main, &lease.id)).unwrap();
    crate::lane_recovery::reconcile_observed(
        &fx.main,
        "BUG-2451",
        &lease.branch,
        captured.as_deref(),
    )
    .unwrap();
    let store = storage.load().unwrap();
    let req = store.get_requirement_by_spec_id("BUG-2451").unwrap();
    assert!(req
        .comments
        .iter()
        .any(|c| c.content.contains(&head) && c.content.contains(&lease.branch)));
}

// trace:BUG-2549 | ai:codex
#[test]
fn bug_2549_unavailable_forge_preserves_exit_evidence() {
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
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "commit",
            "--allow-empty",
            "-qm",
            "private exit during outage",
        ],
    );
    let captured = crate::lane_recovery::exit_head(&fx.main, &lease.branch, &[lease.clone()])
        .unwrap()
        .unwrap();
    git_ok(
        &fx.main,
        &["worktree", "remove", lease.worktree_path.to_str().unwrap()],
    );
    std::fs::remove_file(crate::lease_path(&fx.main, &lease.id)).unwrap();
    crate::test_exec::write_executable(
        &gh,
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'gh test'; exit 0; fi\nexit 1\n",
    );
    assert!(crate::lane_recovery::reconcile_observed(
        &fx.main,
        "BUG-2451",
        "bug-2451",
        Some(&captured)
    )
    .is_err());
    let store = storage.load().unwrap();
    let req = store.get_requirement_by_spec_id("BUG-2451").unwrap();
    assert_eq!(req.status, aida_core::RequirementStatus::InProgress);
    assert!(req.comments.iter().any(|c| c.content.contains("bug-2451")
        && c.content.contains(&captured)
        && !c.content.contains("no open PR")));
    assert!(!req
        .comments
        .iter()
        .any(|c| c.content.contains("no open PR")));
    let comments = req.comments.len();
    // An outage retry must preserve one durable copy without changing status.
    assert!(crate::lane_recovery::reconcile_observed(
        &fx.main,
        "BUG-2451",
        "bug-2451",
        Some(&captured)
    )
    .is_err());
    let retry = storage.load().unwrap();
    let req = retry.get_requirement_by_spec_id("BUG-2451").unwrap();
    assert_eq!(req.comments.len(), comments);
    assert_eq!(req.status, aida_core::RequirementStatus::InProgress);
    // Reopen the store independently, as a later recovery process would:
    // evidence must be durable rather than an in-memory observation.
    let reopened = aida_core::Storage::new(storage.path().to_path_buf())
        .load()
        .unwrap();
    let persisted = reopened.get_requirement_by_spec_id("BUG-2451").unwrap();
    assert_eq!(persisted.status, aida_core::RequirementStatus::InProgress);
    assert_eq!(
        persisted
            .comments
            .iter()
            .filter(
                |c| c.content.starts_with("Lane exit evidence:") && c.content.contains(&captured)
            )
            .count(),
        1
    );
    // A successful spec search followed by a failed branch lookup is also
    // unknown evidence; the durable captured facts must survive this path.
    crate::test_exec::write_executable(
        &gh,
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'gh test'; exit 0; fi\ncase \" $* \" in *' --search '*) exit 0;; esac\nexit 1\n",
    );
    assert!(crate::lane_recovery::reconcile_observed(
        &fx.main,
        "BUG-2451",
        "bug-2451",
        Some(&captured)
    )
    .is_err());
    let branch_failure = storage.load().unwrap();
    let req = branch_failure
        .get_requirement_by_spec_id("BUG-2451")
        .unwrap();
    assert_eq!(req.comments.len(), comments);
    assert_eq!(req.status, aida_core::RequirementStatus::InProgress);
    // Only a later successful no-PR observation authorizes the transition.
    recovery_forge_fixture(&fx.main, &fx.root);
    crate::lane_recovery::reconcile_observed(&fx.main, "BUG-2451", "bug-2451", Some(&captured))
        .unwrap();
    let recovered = storage.load().unwrap();
    let req = recovered.get_requirement_by_spec_id("BUG-2451").unwrap();
    assert_eq!(req.status, aida_core::RequirementStatus::Approved);
    assert!(req
        .comments
        .iter()
        .any(|c| c.content.starts_with("Lane exit evidence:") && c.content.contains(&captured)));
}

// A fake gh reporting merged change #42 for `bug-2451`, with `view` as the
// shell body answering `pr view`.
// trace:BUG-2549 | ai:claude
fn bug_2549_merged_gh(view: &str) -> String {
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
  {view}
fi
exit 1
"#
    )
}

// trace:BUG-2549 | ai:claude
fn bug_2549_merged_view(head: &str) -> String {
    format!(
        r#"printf '%s\n' '{{"state":"MERGED","headRefOid":"{head}","headRefName":"bug-2451","baseRefName":"main"}}'; exit 0"#
    )
}

// Confirmed merged evidence whose merged head cannot be read (empty output,
// an empty head, or a failing view) must not fail pickup: it allocates a
// dated branch, as main did.
// trace:BUG-2549 | ai:claude
#[test]
fn bug_2549_unreadable_merged_head_allocates_a_dated_branch() {
    for view in [
        "exit 0".to_string(),
        bug_2549_merged_view(""),
        "exit 1".to_string(),
    ] {
        let fx = fixture();
        let mut ambient =
            crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
        let gh = recovery_forge_fixture(&fx.main, &fx.root);
        ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
        git_ok(&fx.main, &["branch", "bug-2451"]);
        crate::test_exec::write_executable(&gh, bug_2549_merged_gh(&view));
        assert_eq!(
            crate::queue_cmd::canonical_resume_branch(&fx.main, "BUG-2451", true).unwrap(),
            None,
            "{view}"
        );
        let (storage, _) = super::orphan_reentry_tests::queued_scope(&fx.main, "BUG-2451");
        queue_work(&storage, false).unwrap();
        let lease = crate::list_leases(&fx.main).remove(0);
        assert!(lease.branch.starts_with("bug-2451-"), "{}", lease.branch);
        assert!(super::tests::ref_exists(&fx.main, "refs/heads/bug-2451"));
    }
}

// A canonical branch whose tip cannot be resolved to a commit, alongside
// confirmed merged evidence, also falls back to a dated branch instead of
// aborting pickup.
// trace:BUG-2549 | ai:claude
#[test]
fn bug_2549_unresolvable_canonical_tip_allocates_a_dated_branch() {
    let fx = fixture();
    let mut ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let gh = recovery_forge_fixture(&fx.main, &fx.root);
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    let merged_head = git_ok(&fx.main, &["rev-parse", "HEAD"]);
    // The branch name exists locally but does not name a commit, and origin
    // has no such branch: neither tip source resolves.
    let tree = git_ok(&fx.main, &["rev-parse", "HEAD^{tree}"]);
    let ref_path = git_ok(
        &fx.main,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "refs/heads/bug-2451",
        ],
    );
    std::fs::write(&ref_path, format!("{tree}\n")).unwrap();
    assert!(crate::branch_exists_anywhere(&fx.main, "bug-2451"));
    crate::test_exec::write_executable(
        &gh,
        bug_2549_merged_gh(&bug_2549_merged_view(&merged_head)),
    );
    assert_eq!(
        crate::queue_cmd::canonical_resume_branch(&fx.main, "BUG-2451", true).unwrap(),
        None
    );
    let (storage, _) = super::orphan_reentry_tests::queued_scope(&fx.main, "BUG-2451");
    queue_work(&storage, false).unwrap();
    let lease = crate::list_leases(&fx.main).remove(0);
    assert!(lease.branch.starts_with("bug-2451-"), "{}", lease.branch);
}

// trace:TASK-3799 | ai:codex
#[test]
fn story_2768_auto_complete_refuses_before_queue_registration_or_status_bump() {
    for status in [
        aida_core::RequirementStatus::Draft,
        aida_core::RequirementStatus::Approved,
        aida_core::RequirementStatus::Planned,
    ] {
        let fx = fixture();
        let mut ambient =
            crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
        let gh = forge_fixture(&fx.main, &fx.root, "");
        ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
        story_2768_snapshot(&fx.main, 10);
        let storage = aida_core::Storage::new(fx.root.join("requirements.db"));
        let mut req = aida_core::Requirement::new("fresh work".into(), String::new());
        req.spec_id = Some("BUG-2451".into());
        req.status = status.clone();
        req.execution_mode = Some(aida_core::ExecutionMode::Drain);
        let mut store = aida_core::RequirementsStore::default();
        store.requirements.push(req);
        storage.save(&store).unwrap();
        let error = crate::ensure_queued_for_implementer(
            &storage, &fx.main, "test", "BUG-2451", false, false,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("10 changes are open"),
            "{error:#}"
        );
        assert!(storage.queue_list("test", false).unwrap().is_empty());
        let token = uuid::Uuid::now_v7().to_string();
        let error = crate::prepare_registered_auto_complete_phase1(
            &storage, &fx.main, "BUG-2451", &token, false, true, false, false, None,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("10 changes are open"),
            "{error:#}"
        );
        assert!(!crate::orchestrator::run_is_live(&fx.main, &token));
        assert_eq!(
            storage
                .load()
                .unwrap()
                .get_requirement_by_spec_id("BUG-2451")
                .unwrap()
                .status,
            status
        );
        assert!(crate::list_leases(&fx.main).is_empty());
        assert!(!fx.main.join(".aida/orchestrator-runs").exists());
        let events_path = crate::events::events_path(&fx.main);
        let before = std::fs::read_to_string(&events_path)
            .unwrap()
            .lines()
            .count();
        // Exercise the in-process member boundary: this must return to the
        // caller so a batch can continue and print its summary.
        let result = crate::run_auto_complete(
            &storage,
            "test",
            "BUG-2451",
            crate::auto_complete::AutoCompleteVariant::Full,
            false,
            None,
            None,
            crate::auto_complete::EscalateMode::Blocks,
            false,
            false,
            false,
            false,
            false,
            false,
            None,
        );
        assert_eq!(
            result.failed_phase,
            Some(crate::auto_complete::Phase::Implementer)
        );
        let events = std::fs::read_to_string(&events_path).unwrap();
        assert_eq!(events.lines().count(), before + 1, "{events}");
        assert!(
            events
                .lines()
                .last()
                .unwrap()
                .contains("10 changes are open"),
            "{events}"
        );
        assert!(storage.queue_list("test", false).unwrap().is_empty());
        assert_eq!(
            storage
                .load()
                .unwrap()
                .get_requirement_by_spec_id("BUG-2451")
                .unwrap()
                .status,
            status
        );
    }
}

// trace:TASK-3799 | ai:codex
#[test]
fn story_2768_approved_canonical_branch_resumes_at_limit_with_audit() {
    let fx = fixture();
    let mut ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let gh = forge_fixture(&fx.main, &fx.root, "");
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    story_2768_snapshot(&fx.main, 10);
    git_ok(&fx.main, &["branch", "bug-2451"]);
    let (storage, _) = super::orphan_reentry_tests::queued_scope(&fx.main, "BUG-2451");
    assert_eq!(
        storage
            .load()
            .unwrap()
            .get_requirement_by_spec_id("BUG-2451")
            .unwrap()
            .status,
        aida_core::RequirementStatus::Approved
    );
    // The orchestrator preflight and ordinary queue pickup both recognize it.
    crate::wip_limit::admit_phase1(&fx.main, &storage, "BUG-2451", false).unwrap();
    queue_work_with(&storage, false, true, None, false).unwrap();
    let leases = crate::list_leases(&fx.main);
    assert_eq!(leases.len(), 1);
    assert_eq!(leases[0].branch, "bug-2451");
    let events = std::fs::read_to_string(crate::events::events_path(&fx.main)).unwrap();
    assert!(events.contains("wip-limit-override"), "{events}");
    assert!(events.contains("selected for existing work"), "{events}");
}

// A reconciled later-phase drain resume is existing work even if a deliberate
// status reset or completed merge left no open PR or canonical branch.
// trace:TASK-3799 | ai:codex
#[test]
fn story_2768_reconciled_drain_resume_is_exempt_at_limit() {
    let fx = fixture();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let (storage, _) = super::orphan_reentry_tests::queued_scope(&fx.main, "BUG-2451");
    story_2768_snapshot(&fx.main, 10);
    crate::ensure_queued_for_implementer(
        &storage,
        &fx.main,
        crate::test_env::HERMETIC_TEST_USER,
        "BUG-2451",
        true,
        false,
    )
    .unwrap();
    let token = uuid::Uuid::now_v7().to_string();
    crate::prepare_registered_auto_complete_phase1(
        &storage, &fx.main, "BUG-2451", &token, false, true, true, false, None,
    )
    .unwrap();
    assert!(crate::orchestrator::run_is_live(&fx.main, &token));
    crate::drain_state::clear_run(&fx.main, &token);
}

// trace:TASK-3799 | ai:codex
#[test]
fn story_2768_auto_complete_over_wip_reaches_queue_and_registered_phase1() {
    let fx = fixture();
    let mut ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let gh = forge_fixture(&fx.main, &fx.root, "");
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    ambient.set_key("AIDA_CONTAINED", "0");
    story_2768_snapshot(&fx.main, 10);
    let (storage, _) = super::orphan_reentry_tests::queued_scope(&fx.main, "BUG-2451");
    storage
        .update_atomically(|store| {
            store
                .requirements
                .iter_mut()
                .find(|req| req.spec_id.as_deref() == Some("BUG-2451"))
                .unwrap()
                .execution_mode = Some(aida_core::ExecutionMode::Drain);
        })
        .unwrap();
    // Stop after the queue preflight at an independent, deliberate launch
    // refusal. No phase child is needed to prove the override reached it.
    std::fs::write(fx.main.join(".aida/agents.toml"), "[contained]\nenabled = ").unwrap();
    let result = crate::run_auto_complete(
        &storage,
        crate::test_env::HERMETIC_TEST_USER,
        "BUG-2451",
        crate::auto_complete::AutoCompleteVariant::Full,
        false,
        None,
        None,
        crate::auto_complete::EscalateMode::Blocks,
        true,
        false,
        false,
        true,
        false,
        false,
        None,
    );
    assert_eq!(
        result.failed_phase,
        Some(crate::auto_complete::Phase::Implementer)
    );
    let events = std::fs::read_to_string(crate::events::events_path(&fx.main)).unwrap();
    assert!(events.contains("--over-wip"), "{events}");
    assert!(events.contains("wip-limit-override"), "{events}");
    let admission = crate::ensure_queued_for_implementer(
        &storage,
        &fx.main,
        crate::test_env::HERMETIC_TEST_USER,
        "BUG-2451",
        false,
        true,
    )
    .unwrap();
    let token = uuid::Uuid::now_v7().to_string();
    crate::prepare_registered_auto_complete_phase1(
        &storage,
        &fx.main,
        "BUG-2451",
        &token,
        false,
        true,
        false,
        true,
        Some(admission),
    )
    .unwrap();
    assert_eq!(
        storage
            .load()
            .unwrap()
            .get_requirement_by_spec_id("BUG-2451")
            .unwrap()
            .status,
        aida_core::RequirementStatus::InProgress
    );
    assert!(crate::orchestrator::run_is_live(&fx.main, &token));
    crate::drain_state::clear_run(&fx.main, &token);
}

// trace:BUG-2600 | ai:codex
#[test]
fn bug_2600_phase1_admission_records_one_override_per_member() {
    // trace:BUG-2588 | ai:codex
    // Process-bound grants must never reach another test's shared-home reads.
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    for canonical_branch in [false, true] {
        let fx = fixture();
        let mut ambient =
            crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
        let gh = forge_fixture(&fx.main, &fx.root, "");
        ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
        story_2768_snapshot(&fx.main, 10);
        if canonical_branch {
            git_ok(&fx.main, &["branch", "bug-2451"]);
        }
        let (storage, _) = super::orphan_reentry_tests::queued_scope(&fx.main, "BUG-2451");
        let admission = crate::ensure_queued_for_implementer(
            &storage,
            &fx.main,
            crate::test_env::HERMETIC_TEST_USER,
            "BUG-2451",
            false,
            !canonical_branch,
        )
        .unwrap();
        let token = uuid::Uuid::now_v7().to_string();
        crate::prepare_registered_auto_complete_phase1(
            &storage,
            &fx.main,
            "BUG-2451",
            &token,
            false,
            true,
            false,
            !canonical_branch,
            Some(admission),
        )
        .unwrap();
        let events = std::fs::read_to_string(crate::events::events_path(&fx.main)).unwrap();
        assert_eq!(
            events
                .lines()
                .filter(|line| line.contains("wip-limit-override"))
                .count(),
            1,
            "canonical_branch={canonical_branch}: {events}"
        );
        assert!(crate::orchestrator::run_is_live(&fx.main, &token));
        crate::drain_state::clear_run(&fx.main, &token);
        // trace:BUG-2588 | ai:codex
        assert!(crate::gate::seat_authority::revoke_current().unwrap());
        assert!(
            crate::gate::seat_authority::held_grants("implementer", chrono::Utc::now()).is_empty()
        );
    }
}
