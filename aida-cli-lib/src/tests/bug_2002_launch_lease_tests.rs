use super::*;

// trace:BUG-2002 | ai:codex
fn fixture() -> (tempfile::TempDir, SessionLease) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(leases_dir(root)).unwrap();
    std::fs::write(root.join(".aida/config.toml"), "").unwrap();
    let worktree = root.join("owned");
    std::fs::create_dir(&worktree).unwrap();
    assert!(std::process::Command::new("/usr/bin/git")
        .args(["init", "-q", "-b", "bug-2002-work"])
        .arg(&worktree)
        .status()
        .unwrap()
        .success());
    let lease: SessionLease = serde_json::from_value(serde_json::json!({
        "id": "launch-owner-2002", "scope": "BUG-2002", "slug": "bug2002",
        "owner": "tester", "worktree_path": worktree, "branch": "bug-2002-work",
        "started_at": (chrono::Utc::now() - chrono::Duration::days(2)).to_rfc3339(),
        "hostname": "test", "role": "implementer"
    }))
    .unwrap();
    persist(root, &lease);
    (tmp, lease)
}

fn persist(root: &std::path::Path, lease: &SessionLease) {
    std::fs::write(lease_path(root, &lease.id), toml::to_string(lease).unwrap()).unwrap();
}

fn launch(root: &std::path::Path, background: bool, preview: bool) -> Result<()> {
    let config = AgentLaunchConfig {
        agent_type: "claude",
        binary: "/usr/bin/true",
        default_args: vec!["--permission-mode".into(), "bypassPermissions".into()],
        prompt_style: AgentPromptStyle::Positional,
        contained: false,
    };
    let launch = if background {
        agent_new_bg_dispatch
    } else {
        agent_new_with_config
    };
    launch(
        config,
        Some("implementer".into()),
        Some("BUG-2002".into()),
        false,
        Some(root),
        AgentContextOptions::new(false, false),
        false,
        false,
        preview,
        false,
        AgentPromptOptions::new(Some("fresh work".into()), false),
        AgentResumeOptions::new(true, None, false, true),
        AgentDefaultFlagOptions::new(false, Vec::new(), None),
        Some("launch-lease-test".into()),
        None,
        false,
    )
}

// The bypass and resume prompts are enabled. Before the fix these entrypoints
// encounter their prompt/consent path rather than returning this ownership error.
// trace:BUG-2002 | ai:codex
#[test]
fn agent_new_foreground_conflict_precedes_all_prompts() {
    let _env = crate::test_env::EnvVarGuard::set("AIDA_BUILD_PROFILE", "invalid-for-test");
    // The pre-prompt binary-validation barrier fails if ownership is checked late.
    assert!(aida_bin::process().is_err());
    let (tmp, lease) = fixture();
    let before = std::fs::read(lease_path(tmp.path(), &lease.id)).unwrap();
    let error = launch(tmp.path(), false, false).unwrap_err().to_string();
    assert!(
        error.contains("already owned by session launch-owner-2002"),
        "{error}"
    );
    assert!(error.contains("--cwd"), "{error}");
    assert!(error.contains("aida session leases --all"), "{error}");
    assert!(
        !error.contains("BUG-2002"),
        "no internal spec id in refusal: {error}"
    );
    assert_eq!(
        std::fs::read(lease_path(tmp.path(), &lease.id)).unwrap(),
        before
    );
}

// trace:BUG-2002 | ai:codex
#[test]
fn agent_new_background_conflict_precedes_all_prompts() {
    let _env = crate::test_env::EnvVarGuard::set("AIDA_BUILD_PROFILE", "invalid-for-test");
    // The pre-prompt binary-validation barrier fails if ownership is checked late.
    assert!(aida_bin::process().is_err());
    let (tmp, _) = fixture();
    let error = launch(tmp.path(), true, false).unwrap_err().to_string();
    assert!(
        error.contains("already owned by session launch-owner-2002"),
        "{error}"
    );
    assert!(error.contains("--no-resume"), "{error}");
}

// trace:BUG-2002 | ai:codex
#[test]
fn agent_new_same_worktree_adoption_preserves_ownership() {
    let _env = crate::test_env::env_lock();
    let (tmp, lease) = fixture();
    let before = std::fs::read(lease_path(tmp.path(), &lease.id)).unwrap();
    let plan = prepare_agent_launch_with_cwd(
        tmp.path(),
        Some("implementer".into()),
        Some("bug-2002".into()),
        "codex",
        Some("rework-test".into()),
        Some(&lease.worktree_path),
        /* contained_seat */ false,
    )
    .unwrap();
    assert_eq!(plan.lease_id.as_deref(), Some(lease.id.as_str()));
    assert_eq!(plan.launch_cwd, lease.worktree_path);
    assert_eq!(plan.role.as_deref(), Some("implementer"));
    assert_eq!(list_leases(tmp.path()).len(), 1);
    assert_eq!(
        std::fs::read(lease_path(tmp.path(), &lease.id)).unwrap(),
        before
    );
}

// trace:BUG-2002 | ai:codex
#[test]
fn agent_new_reentry_refuses_role_branch_and_live_owner_mismatches() {
    let _env = crate::test_env::env_lock();
    let (tmp, mut lease) = fixture();
    let check = |role| {
        preflight_agent_launch_lease(
            tmp.path(),
            Some(role),
            Some("BUG-2002"),
            Some(&lease.worktree_path),
        )
    };
    assert!(check("reviewer").is_err());
    assert!(check("implementer").is_ok());
    lease.active_pid = Some(std::process::id());
    persist(tmp.path(), &lease);
    assert!(preflight_agent_launch_lease(
        tmp.path(),
        Some("implementer"),
        Some("BUG-2002"),
        Some(&lease.worktree_path)
    )
    .is_err());
    lease.active_pid = None;
    lease.branch = "different-branch".into();
    persist(tmp.path(), &lease);
    assert!(preflight_agent_launch_lease(
        tmp.path(),
        Some("implementer"),
        Some("BUG-2002"),
        Some(&lease.worktree_path)
    )
    .is_err());
}

// trace:BUG-2002 | ai:codex
#[test]
fn agent_new_preview_stays_side_effect_free_with_conflicting_lease() {
    let _env = crate::test_env::env_lock();
    let (tmp, lease) = fixture();
    let before = std::fs::read(lease_path(tmp.path(), &lease.id)).unwrap();
    for background in [false, true] {
        launch(tmp.path(), background, true).unwrap();
    }
    assert_eq!(
        std::fs::read(lease_path(tmp.path(), &lease.id)).unwrap(),
        before
    );
    assert!(!tmp.path().join(".aida/agents").exists());
}

// trace:BUG-2002 | ai:codex
#[test]
fn final_plan_rechecks_lease_created_after_preflight() {
    let _env = crate::test_env::env_lock();
    let (tmp, lease) = fixture();
    std::fs::remove_file(lease_path(tmp.path(), &lease.id)).unwrap();
    assert!(
        preflight_agent_launch_lease(tmp.path(), Some("implementer"), Some("BUG-2002"), None)
            .unwrap()
            .is_none()
    );
    persist(tmp.path(), &lease);
    let error = prepare_agent_launch(
        tmp.path(),
        Some("implementer".into()),
        Some("BUG-2002".into()),
        "codex",
        None,
    )
    .unwrap_err();
    assert!(error.to_string().contains("already owned"), "{error}");
}

// trace:BUG-2002 | ai:codex
#[test]
fn session_leases_default_includes_dispatch_blocking_stale_lease() {
    let _env = crate::test_env::env_lock();
    let (_tmp, mut lease) = fixture();
    assert_eq!(
        lease_state_for(&lease, &[], chrono::Utc::now()),
        LeaseState::Stale
    );
    assert!(lease_blocks_agent_dispatch(&lease));
    let spec_lease = lease.clone();
    lease.id = "stale-harness".into();
    lease.scope = worktree_lease::HARNESS_WORKTREE_SCOPE.into();
    let classified = vec![
        (spec_lease.clone(), LeaseState::Stale, None),
        (lease.clone(), LeaseState::Stale, None),
    ];
    let (shown, hidden) = select_lease_listing(classified.clone(), false);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].0.id, spec_lease.id);
    assert_eq!(shown[0].1, LeaseState::Stale);
    assert_eq!(hidden.len(), 1);
    assert_eq!(hidden[0].0.id, lease.id);
    let rows = leases_json_rows(&shown, None);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], spec_lease.id);
    let (all, hidden) = select_lease_listing(classified, true);
    assert_eq!(all.len(), 2);
    assert!(hidden.is_empty());
}

// trace:BUG-2002 | ai:codex
#[test]
fn agent_new_bg_help_documents_noninteractive_form() {
    use clap::CommandFactory;
    let mut command = crate::command_registry::built_command();
    let help = command
        .find_subcommand_mut("agent")
        .unwrap()
        .find_subcommand_mut("new")
        .unwrap()
        .find_subcommand_mut("claude")
        .unwrap()
        .render_long_help()
        .to_string()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    for arg in [
        "--bg",
        "--no-resume",
        "--permission-mode bypassPermissions",
        "--prompt-file <FILE>",
        "confirm_bypass = false",
        "trusted project config can still require consent",
    ] {
        assert!(help.contains(arg), "missing {arg}: {help}");
    }
}

// trace:BUG-2002 | ai:codex
#[test]
fn agent_new_reentry_respects_reasoned_operator_hold() {
    let _env = crate::test_env::env_lock();
    let (tmp, lease) = fixture();
    let mut store = aida_core::RequirementsStore::default();
    let mut req = aida_core::Requirement::new("held work".into(), "".into());
    req.spec_id = Some("BUG-2002".into());
    req.deferred = true;
    req.deferred_reason = Some("waiting for operator".into());
    store.requirements.push(req);
    Storage::new(tmp.path().join(".aida-store"))
        .save(&store)
        .unwrap();
    let error = preflight_agent_launch_lease(
        tmp.path(),
        Some("implementer"),
        Some("BUG-2002"),
        Some(&lease.worktree_path),
    )
    .unwrap_err();
    assert!(error
        .downcast_ref::<defer_cmd::OperatorHoldRefusal>()
        .is_some());
}

// trace:BUG-2002 | ai:codex
#[test]
fn dead_recorded_pid_does_not_hide_observed_live_owner() {
    let _env = crate::test_env::env_lock();
    let (tmp, mut lease) = fixture();
    // Use a real reaped process, not a numeric sentinel: high unsigned PIDs
    // can become Unix process-group selectors in the legacy probe.
    let mut child = std::process::Command::new("/usr/bin/cat")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    lease.active_pid = Some(child.id());
    lease.active_pid_start_time =
        Some(process_probe::process_start_identity(child.id()).expect("live child start identity"));
    drop(child.stdin.take());
    assert!(child.wait().unwrap().success());
    assert!(!process_probe::process_identity_is_alive(
        lease.active_pid.unwrap(),
        lease.active_pid_start_time.as_deref(),
    ));
    let live = [process_probe::LiveSession {
        pid: 4242,
        cwd: lease.worktree_path.clone(),
        jsonl: None,
        stale_cwd: false,
    }];
    assert_eq!(
        lease_state_for(&lease, &live, chrono::Utc::now()),
        LeaseState::Stale
    );
    assert!(agent_launch_lease_is_active(tmp.path(), &lease, &live));
}

// trace:BUG-2002 | ai:codex
#[test]
fn agent_new_duplicate_scope_owners_refuse_even_with_owning_cwd() {
    let _env = crate::test_env::env_lock();
    let (tmp, lease) = fixture();
    let mut other = lease.clone();
    other.id = "second-owner-2002".into();
    persist(tmp.path(), &other);
    let error = preflight_agent_launch_lease(
        tmp.path(),
        Some("implementer"),
        Some("BUG-2002"),
        Some(&lease.worktree_path),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("multiple sessions own this work"),
        "{error}"
    );
    assert_eq!(list_leases(tmp.path()).len(), 2);
}

// trace:BUG-2002 | ai:codex
#[cfg(unix)]
#[test]
fn agent_new_live_owner_is_seen_through_a_symlinked_lease_path() {
    let _env = crate::test_env::env_lock();
    let (tmp, mut lease) = fixture();
    let real = lease.worktree_path.clone();
    let alias = tmp.path().join("alias");
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    lease.worktree_path = alias;
    let live = [process_probe::LiveSession {
        pid: 4242,
        cwd: real,
        jsonl: None,
        stale_cwd: false,
    }];
    assert!(agent_launch_lease_is_active(tmp.path(), &lease, &live));
}

// trace:BUG-2002 | ai:codex
#[cfg(unix)]
#[test]
fn child_lease_identity_overrides_or_removes_the_launchers_identity() {
    let _env = crate::test_env::EnvVarGuard::set("AIDA_SESSION_ID", "unrelated-parent");
    for id in [Some("new-lease"), Some("reused-lease"), None] {
        let mut command = std::process::Command::new("/bin/sh");
        command.args(["-c", "printf '%s' \"${AIDA_SESSION_ID-unset}\""]);
        apply_agent_lease_identity(&mut command, id);
        let output = command.output().unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            id.unwrap_or("unset")
        );
    }
}

// Exercise the entrypoints all the way through spawning the harmless child.
// The child records its effective cwd and lease identity; no real vendor starts.
// trace:BUG-2002 | ai:codex
#[cfg(unix)]
#[test]
fn agent_new_foreground_and_background_reenter_with_explicit_owning_cwd() {
    for (background, scoped) in [(false, true), (true, true), (false, false), (true, false)] {
        let (tmp, lease) = fixture();
        let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
            tmp.path(),
            "advisor",
            &["implementer"],
            &[
                ("AIDA_BUILD_PROFILE", None),
                ("AIDA_BIN", None),
                ("AIDA_SESSION_ID", Some("unrelated-parent")),
                ("AIDA_OS_WRAP", Some("0")),
                ("AIDA_FOCUS_EPIC", None),
            ],
        );
        let script = tmp.path().join("record-child.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"${AIDA_SESSION_ID-unset}\" > child-record\n",
        )
        .unwrap();
        let config = AgentLaunchConfig {
            agent_type: "claude",
            binary: "/bin/sh",
            default_args: vec![script.display().to_string()],
            prompt_style: AgentPromptStyle::Positional,
            contained: false,
        };
        let launch = if background {
            agent_new_bg_dispatch
        } else {
            agent_new_with_config
        };
        let cwd = if scoped {
            lease.worktree_path.as_path()
        } else {
            tmp.path()
        };
        launch(
            config,
            scoped.then(|| "implementer".into()),
            scoped.then(|| "BUG-2002".into()),
            false,
            Some(cwd),
            AgentContextOptions::new(false, false),
            false,
            false,
            false,
            false,
            AgentPromptOptions::new(Some("fresh work".into()), false),
            AgentResumeOptions::new(false, None, false, false),
            AgentDefaultFlagOptions::new(false, Vec::new(), None),
            Some("reentry-recording-child".into()),
            None,
            false,
        )
        .unwrap();
        let record = std::fs::read_to_string(cwd.join("child-record")).unwrap();
        let lines: Vec<_> = record.lines().collect();
        assert_eq!(
            std::path::Path::new(lines[0]).canonicalize().unwrap(),
            cwd.canonicalize().unwrap()
        );
        assert_eq!(lines[1], if scoped { lease.id.as_str() } else { "unset" });
        let leases = list_leases(tmp.path());
        assert_eq!(leases.len(), 1);
        assert_eq!(leases[0].id, lease.id);
        assert_eq!(leases[0].scope, lease.scope);
        assert_eq!(leases[0].started_at, lease.started_at);
    }
}

// trace:BUG-2168 | ai:codex
#[test]
fn bug2168_explicit_cwd_without_spec_is_the_actual_launch_directory() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let (tmp, lease) = fixture();
    let _env = crate::test_env::AmbientGuard::hermetic(tmp.path(), None);
    let plan = prepare_agent_launch_with_cwd(
        tmp.path(),
        None,
        None,
        "claude",
        Some("cwd-test".into()),
        Some(&lease.worktree_path),
        false,
    )
    .unwrap();
    assert_eq!(plan.launch_cwd, lease.worktree_path.canonicalize().unwrap());
    assert!(plan.lease_id.is_none());
}

// trace:BUG-2188 | ai:codex
#[test]
fn bug2188_scrubbed_delegation_refuses_before_any_setup() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let (tmp, lease) = fixture();
    let _env = crate::test_env::AmbientGuard::hermetic(tmp.path(), Some("implementer"));
    let before = std::fs::read(lease_path(tmp.path(), &lease.id)).unwrap();
    let error = preflight_agent_new_delegation(tmp.path(), Some("implementer"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("no valid parent seat grant"), "{error}");
    assert!(
        error.contains("aida role enter advisor --delegate-seat implementer"),
        "{error}"
    );
    assert!(error.contains("same shell"), "{error}");
    assert_eq!(
        std::fs::read(lease_path(tmp.path(), &lease.id)).unwrap(),
        before
    );
}

// trace:BUG-2188 BUG-2168 | ai:codex
fn receipt_project() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("project");
    let tree = tmp.path().join("requested-worker");
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[worktree_pool]\nenabled = true\n",
    )
    .unwrap();
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["config", "user.name", "launch fixture"],
        vec!["config", "user.email", "launch@example.test"],
        vec!["commit", "-q", "--allow-empty", "-m", "init"],
    ] {
        assert!(std::process::Command::new("/usr/bin/git")
            .current_dir(&root)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    (tmp, root, tree)
}

// trace:BUG-2188 BUG-2168 | ai:codex
#[test]
fn bug2188_fresh_receipt_owns_exact_lease_and_rolls_back_requested_tree() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let (_tmp, root, tree) = receipt_project();
    let _env = crate::test_env::AmbientGuard::hermetic(&root, Some("implementer"));
    let (plan, receipt) = prepare_agent_launch_attempt(
        &root,
        Some("implementer".into()),
        Some("launch-fixture".into()),
        "claude",
        Some("receipt-test".into()),
        Some(&tree),
        false,
    )
    .unwrap();
    assert_eq!(plan.launch_cwd, tree);
    let receipt = receipt.unwrap();
    assert_eq!(receipt.lease_id, plan.lease_id);
    assert!(lease_path(&root, plan.lease_id.as_deref().unwrap()).exists());
    assert!(tree.exists());
    drop(receipt); // simulate a subsequent launch-context/spawn refusal
    assert!(!tree.exists());
    assert!(!lease_path(&root, plan.lease_id.as_deref().unwrap()).exists());
    assert!(!branch_exists_anywhere(&root, "launch-fixture"));
}

// trace:BUG-2188 | ai:codex
#[test]
fn bug2188_prelease_failure_removes_materialized_tree_and_names_cause() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let (_tmp, root, tree) = receipt_project();
    let _env = crate::test_env::AmbientGuard::hermetic(&root, Some("implementer"));
    crate::seat_clone::failpoint::fail_before_lease(true);
    let result = prepare_agent_launch_attempt(
        &root,
        Some("implementer".into()),
        Some("launch-fixture".into()),
        "claude",
        Some("receipt-test".into()),
        Some(&tree),
        false,
    );
    crate::seat_clone::failpoint::fail_before_lease(false);
    let error = result.err().expect("injected setup failure").to_string();
    assert!(
        error.contains("injected failure before the lease write"),
        "{error}"
    );
    assert!(error.contains(&tree.display().to_string()), "{error}");
    assert!(!tree.exists());
    assert!(list_leases(&root).is_empty());
    assert!(!branch_exists_anywhere(&root, "launch-fixture"));
}

// trace:BUG-2188 BUG-2168 | ai:codex
#[test]
fn bug2188_adopted_receipt_never_deletes_an_existing_lease() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let (tmp, lease) = fixture();
    let _env = crate::test_env::AmbientGuard::hermetic(tmp.path(), Some("implementer"));
    let original = std::fs::read(lease_path(tmp.path(), &lease.id)).unwrap();
    let (plan, receipt) = prepare_agent_launch_attempt(
        tmp.path(),
        Some("implementer".into()),
        Some("BUG-2002".into()),
        "claude",
        Some("adopt-test".into()),
        Some(&lease.worktree_path),
        false,
    )
    .unwrap();
    drop(receipt);
    assert_eq!(plan.lease_id.as_deref(), Some(lease.id.as_str()));
    assert!(lease.worktree_path.exists());
    assert_eq!(
        std::fs::read(lease_path(tmp.path(), &lease.id)).unwrap(),
        original
    );
}

// trace:BUG-2188 | ai:codex
#[test]
fn bug2188_failed_command_revokes_only_its_issued_grant() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let (_tmp, root, _tree) = receipt_project();
    let _env =
        crate::test_env::AmbientGuard::hermetic_with_seat(&root, "advisor", &["implementer"]);
    let parent = seat_authority::current_grant(&root).unwrap().id;
    let child =
        seat_authority::issue_child(&root, "implementer", crate::test_env::HERMETIC_TEST_USER)
            .unwrap();
    let mut command = std::process::Command::new("/missing/agent");
    command.env(seat_authority::GRANT_ENV, &child.id);
    let guard = IssuedLaunchGrant::from_command(&command);
    assert!(command.spawn().is_err());
    drop(guard);
    assert!(
        !seat_authority::revoke_by_id(&child.id).unwrap(),
        "already revoked"
    );
    assert_eq!(seat_authority::current_grant(&root).unwrap().id, parent);
}

// trace:BUG-2188 | ai:codex
#[test]
fn bug2188_pool_cleanup_keeps_the_lease_when_a_live_process_holds_the_tree() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let (_tmp, root, _tree) = receipt_project();
    let _env = crate::test_env::AmbientGuard::hermetic(&root, Some("implementer"));
    let (plan, receipt) = prepare_agent_launch_attempt(
        &root,
        Some("implementer".into()),
        Some("launch-fixture".into()),
        "claude",
        Some("pool-receipt-test".into()),
        None,
        false,
    )
    .unwrap();
    assert!(aida_core::worktree_pool::is_pool_worktree(
        &root,
        &plan.launch_cwd
    ));
    let mut holder = std::process::Command::new("/bin/sleep")
        .arg("60")
        .current_dir(&plan.launch_cwd)
        .spawn()
        .unwrap();
    drop(receipt);
    assert!(lease_path(&root, plan.lease_id.as_deref().unwrap()).exists());
    assert!(aida_core::worktree_pool::read_state(&root)
        .unwrap()
        .entries
        .iter()
        .any(|entry| entry.path == plan.launch_cwd && entry.leased));
    holder.kill().unwrap();
    holder.wait().unwrap();
    aida_core::worktree_pool::return_to_pool(&root, &plan.launch_cwd).unwrap();
    assert!(remove_session_lease_record_only(
        &root,
        plan.lease_id.as_deref().unwrap()
    ));
}

// trace:BUG-2188 | ai:codex
#[test]
fn bug2188_private_clone_cleanup_removes_the_clone_pin_and_lease() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let (_tmp, root, _tree) = receipt_project();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[contained]\nseat_tree = \"clone\"\n",
    )
    .unwrap();
    assert!(std::process::Command::new("/usr/bin/git")
        .current_dir(&root)
        .args(["remote", "add", "origin"])
        .arg(&root)
        .status()
        .unwrap()
        .success());
    let _env = crate::test_env::AmbientGuard::hermetic(&root, Some("implementer"));
    let tree = root.parent().unwrap().join(format!(
        "{}receipt-fixture",
        crate::seat_clone::CLONE_DIR_PREFIX
    ));
    let (plan, receipt) = prepare_agent_launch_attempt(
        &root,
        Some("implementer".into()),
        Some("launch-fixture".into()),
        "claude",
        Some("clone-receipt-test".into()),
        Some(&tree),
        true,
    )
    .unwrap();
    let pin = crate::seat_clone::pin_ref(&tree).unwrap();
    let exists = || {
        std::process::Command::new("/usr/bin/git")
            .current_dir(&root)
            .args(["show-ref", "--verify", "--quiet", &pin])
            .status()
            .unwrap()
            .success()
    };
    assert!(tree.join(".git").is_dir());
    assert!(exists());
    drop(receipt);
    assert!(!tree.exists());
    assert!(!exists());
    assert!(!lease_path(&root, plan.lease_id.as_deref().unwrap()).exists());
}

// trace:BUG-2188 | ai:codex
#[test]
fn bug2188_post_checkout_partial_materialization_rolls_back() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let (_tmp, root, tree) = receipt_project();
    let _env = crate::test_env::AmbientGuard::hermetic(&root, Some("implementer"));
    crate::test_exec::write_executable(
        &root.join(".git/hooks/post-checkout"),
        "#!/bin/sh\necho injected-post-checkout-failure >&2\nexit 73\n",
    );
    let result = prepare_agent_launch_attempt(
        &root,
        Some("implementer".into()),
        Some("launch-fixture".into()),
        "claude",
        Some("partial-test".into()),
        Some(&tree),
        false,
    );
    let error = result.err().expect("post-checkout must refuse").to_string();
    assert!(error.contains("injected-post-checkout-failure"), "{error}");
    assert!(!tree.exists());
    assert!(!branch_exists_anywhere(&root, "launch-fixture"));
    assert!(list_leases(&root).is_empty());
    assert!(
        !std::fs::read_dir(root.parent().unwrap()).unwrap().any(|p| p
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("aida-launch-"))
    );
}

// trace:BUG-2188 | ai:codex
#[test]
fn bug2188_preexisting_path_and_branch_survive_failed_setup() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let (_tmp, root, tree) = receipt_project();
    let _env = crate::test_env::AmbientGuard::hermetic(&root, Some("implementer"));
    std::fs::create_dir(&tree).unwrap();
    std::fs::write(tree.join("keep"), "existing path").unwrap();
    assert!(std::process::Command::new("/usr/bin/git")
        .current_dir(&root)
        .args(["branch", "launch-fixture"])
        .status()
        .unwrap()
        .success());
    let before = std::process::Command::new("/usr/bin/git")
        .current_dir(&root)
        .args(["rev-parse", "launch-fixture"])
        .output()
        .unwrap()
        .stdout;
    let result = prepare_agent_launch_attempt(
        &root,
        Some("implementer".into()),
        Some("launch-fixture".into()),
        "claude",
        Some("existing-test".into()),
        Some(&tree),
        false,
    );
    assert!(result.is_err());
    assert_eq!(
        std::fs::read_to_string(tree.join("keep")).unwrap(),
        "existing path"
    );
    assert_eq!(
        std::process::Command::new("/usr/bin/git")
            .current_dir(&root)
            .args(["rev-parse", "launch-fixture"])
            .output()
            .unwrap()
            .stdout,
        before
    );
}

// trace:BUG-2188 | ai:codex
#[test]
fn bug2188_changed_worktree_identity_is_kept() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let (_tmp, root, tree) = receipt_project();
    let _env = crate::test_env::AmbientGuard::hermetic(&root, Some("implementer"));
    let (plan, receipt) = prepare_agent_launch_attempt(
        &root,
        Some("implementer".into()),
        Some("launch-fixture".into()),
        "claude",
        Some("identity-test".into()),
        Some(&tree),
        false,
    )
    .unwrap();
    let marker = std::fs::read(tree.join(".git")).unwrap();
    std::fs::write(tree.join(".git"), "gitdir: /changed/identity\n").unwrap();
    drop(receipt);
    assert!(tree.exists());
    assert!(lease_path(&root, plan.lease_id.as_deref().unwrap()).exists());
    std::fs::write(tree.join(".git"), marker).unwrap();
}

// trace:BUG-2188 | ai:codex
#[test]
fn bug2188_changed_branch_identity_survives_cleanup() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let (_tmp, root, tree) = receipt_project();
    let _env = crate::test_env::AmbientGuard::hermetic(&root, Some("implementer"));
    let (_plan, receipt) = prepare_agent_launch_attempt(
        &root,
        Some("implementer".into()),
        Some("launch-fixture".into()),
        "claude",
        Some("branch-test".into()),
        Some(&tree),
        false,
    )
    .unwrap();
    assert!(std::process::Command::new("/usr/bin/git")
        .current_dir(&tree)
        .args([
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@test",
            "commit",
            "--allow-empty",
            "-m",
            "successor"
        ])
        .status()
        .unwrap()
        .success());
    drop(receipt);
    assert!(branch_exists_anywhere(&root, "launch-fixture"));
}

// Exercise actual entrypoints, including issuance after setup and the
// foreground spawn / background dispatcher refusal edges.
// trace:BUG-2188 | ai:codex
#[test]
fn bug2188_production_failed_launches_rollback_setup_and_child_grant() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    for background in [false, true] {
        let (_tmp, root, _tree) = receipt_project();
        let _env = crate::test_env::AmbientGuard::hermetic_with_seat_and(
            &root,
            "advisor",
            &["implementer"],
            &[
                ("AIDA_BUILD_PROFILE", None),
                ("AIDA_BIN", None),
                ("AIDA_OS_WRAP", Some("0")),
                ("AIDA_CONTAINED", None),
            ],
        );
        let parent = seat_authority::current_grant(&root).unwrap().id;
        let script = root.join("fail-child");
        crate::test_exec::write_executable(
            &script,
            if background {
                "#!/bin/sh\nif [ \"$1\" = --version ]; then echo fixture-version; exit 0; fi\necho dispatcher-refused >&2\nexit 73\n"
            } else {
                "#!/aida-fixture-no-such-interpreter\n"
            },
        );
        let config = AgentLaunchConfig {
            agent_type: "claude",
            binary: Box::leak(script.to_string_lossy().into_owned().into_boxed_str()),
            default_args: vec![],
            prompt_style: AgentPromptStyle::Positional,
            contained: false,
        };
        let launch = if background {
            agent_new_bg_dispatch
        } else {
            agent_new_with_config
        };
        let previous_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&root).unwrap();
        let result = launch(
            config,
            Some("implementer".into()),
            Some("launch-fixture".into()),
            false,
            None,
            AgentContextOptions::new(false, false),
            false,
            false,
            false,
            false,
            AgentPromptOptions::new(Some("fixture".into()), false),
            AgentResumeOptions::new(false, None, false, false),
            AgentDefaultFlagOptions::new(false, vec![], None),
            Some("failed-launch-worker".into()),
            None,
            false,
        );
        std::env::set_current_dir(previous_cwd).unwrap();
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains(if background {
                "exited with status 73"
            } else {
                "failed to spawn"
            }),
            "{error}"
        );
        let pool = aida_core::worktree_pool::read_state(&root).unwrap();
        assert!(pool.entries.iter().all(|entry| !entry.leased));
        assert!(!branch_exists_anywhere(&root, "launch-fixture"));
        assert!(std::fs::read_dir(root.parent().unwrap())
            .unwrap()
            .all(|entry| {
                let path = entry.unwrap().path();
                path == root || pool.entries.iter().any(|entry| entry.path == path)
            }));
        assert!(list_leases(&root).is_empty());
        let children = std::fs::read_dir(seat_authority::grant_dir().unwrap())
            .unwrap()
            .map(|entry| {
                serde_json::from_slice::<seat_authority::SeatGrant>(
                    &std::fs::read(entry.unwrap().path()).unwrap(),
                )
                .unwrap()
            })
            .filter(|g| g.parent_grant_id.as_deref() == Some(&parent))
            .collect::<Vec<_>>();
        assert_eq!(
            children.len(),
            1,
            "production path must have issued a child grant"
        );
        assert!(children[0].revoked_at.is_some());
        assert_eq!(seat_authority::current_grant(&root).unwrap().id, parent);
    }
}

// trace:BUG-2188 | ai:codex
#[test]
fn bug2188_checkout_created_destination_is_never_nested_or_removed() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let (_tmp, root, tree) = receipt_project();
    let _env = crate::test_env::AmbientGuard::hermetic(&root, Some("implementer"));
    crate::test_exec::write_executable(
        &root.join(".git/hooks/post-checkout"),
        &format!(
            "#!/bin/sh\nmkdir -p '{}'\nprintf kept > '{}/keep'\n",
            tree.display(),
            tree.display()
        ),
    );
    let result = prepare_agent_launch_attempt(
        &root,
        Some("implementer".into()),
        Some("launch-fixture".into()),
        "claude",
        Some("publication-test".into()),
        Some(&tree),
        false,
    );
    let error = result
        .err()
        .expect("concurrent destination must refuse publication")
        .to_string();
    assert!(error.contains("without replacing"), "{error}");
    assert_eq!(std::fs::read_to_string(tree.join("keep")).unwrap(), "kept");
    assert_eq!(std::fs::read_dir(&tree).unwrap().count(), 1);
    assert!(!branch_exists_anywhere(&root, "launch-fixture"));
    assert!(list_leases(&root).is_empty());
}

// trace:BUG-2188 | ai:codex
#[test]
fn bug2188_pool_hook_commit_keeps_original_branch_receipt() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let (_tmp, root, _tree) = receipt_project();
    let _env = crate::test_env::AmbientGuard::hermetic(&root, Some("implementer"));
    crate::test_exec::write_executable(&root.join(".git/hooks/post-checkout"), "#!/bin/sh\nif /usr/bin/git symbolic-ref --quiet HEAD >/dev/null; then /usr/bin/git -c user.name=fixture -c user.email=fixture@test commit -q --allow-empty -m hook-created-work; fi\n");
    let (plan, receipt) = prepare_agent_launch_attempt(
        &root,
        Some("implementer".into()),
        Some("launch-fixture".into()),
        "claude",
        Some("pool-identity-test".into()),
        None,
        false,
    )
    .unwrap();
    let branch = current_branch_at(&plan.launch_cwd).unwrap();
    let before = std::process::Command::new("/usr/bin/git")
        .current_dir(&root)
        .args(["rev-parse", &branch])
        .output()
        .unwrap()
        .stdout;
    drop(receipt);
    assert!(
        branch_exists_anywhere(&root, &branch),
        "hook changed branch must survive rollback"
    );
    assert_eq!(
        std::process::Command::new("/usr/bin/git")
            .current_dir(&root)
            .args(["rev-parse", &branch])
            .output()
            .unwrap()
            .stdout,
        before
    );
}
