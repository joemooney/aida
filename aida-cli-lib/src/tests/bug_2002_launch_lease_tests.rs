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
    let _env = crate::test_env::env_lock();
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
    let _env = crate::test_env::env_lock();
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
    assert!(lease_visible_in_default_listing(&lease, LeaseState::Stale));
    let rows = leases_json_rows(&[(lease.clone(), LeaseState::Stale, None)], None);
    assert_eq!(rows[0]["id"], lease.id);
    lease.scope = worktree_lease::HARNESS_WORKTREE_SCOPE.into();
    assert!(!lease_blocks_agent_dispatch(&lease));
    assert!(!lease_visible_in_default_listing(&lease, LeaseState::Stale));
}

// trace:BUG-2002 | ai:codex
#[test]
fn agent_new_bg_help_documents_noninteractive_form() {
    use clap::CommandFactory;
    let mut command = Cli::command();
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
    lease.active_pid = Some(u32::MAX);
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
