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
