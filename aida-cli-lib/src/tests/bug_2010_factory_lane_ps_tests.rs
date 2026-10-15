use super::*;
use std::path::{Path, PathBuf};

// trace:BUG-2010 | ai:antigravity
fn ps_lease(id: &str, scope: &str, owner: &str, worktree: PathBuf) -> SessionLease {
    SessionLease {
        id: id.to_string(),
        scope: scope.to_string(),
        slug: scope.to_ascii_lowercase(),
        owner: owner.into(),
        worktree_path: worktree,
        branch: scope.to_ascii_lowercase(),
        started_at: chrono::Utc::now(),
        hostname: "test-host".into(),
        role: Some("implementer".into()),
        creator_pid: None,
        creator_pid_start_time: None,
        active_pid: None,
        active_pid_start_time: None,
        cargo_target_dir: None,
        parent_project_root: None,
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: false,
        claim_verb: false,
        manual_enter_at: None,
        interrupted_at: None,
        interrupted_reason: None,
        driver: None,
    }
}

// trace:BUG-2010 | ai:antigravity
fn agent_entry(
    agent_type: &str,
    name: &str,
    pid: u32,
    current_spec: Option<&str>,
    worktree: PathBuf,
) -> agent_registry::AgentRegistryEntry {
    let now = chrono::Utc::now();
    agent_registry::AgentRegistryEntry {
        id: format!("{agent_type}-{pid}"),
        agent_type: agent_type.to_string(),
        pid,
        pid_start: crate::process_probe::process_start_identity(pid),
        name: Some(name.to_string()),
        description: None,
        tty: None,
        terminal: None,
        started_at: now,
        last_active_at: now,
        role: Some("implementer".to_string()),
        current_spec: current_spec.map(str::to_string),
        worktree_path: worktree,
        source: "agent-launcher".to_string(),
        binary_version: None,
        build_sha: None,
        availability: agent_registry::Availability::Available,
        paused_since: None,
        paused_reason: None,
        expected_back: None,
        native_session_id: None,
        claude_session_id: None,
        ended_at: None,
        spec_status_at_end: None,
        resumed_from: None,
    }
}

// trace:BUG-2010 | ai:antigravity
fn init_git_repo(path: &Path) {
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["init", "-q", "-b", "main"])
        .status();
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["config", "user.email", "test@example.com"])
        .status();
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["config", "user.name", "AIDA Test"])
        .status();
    std::fs::write(path.join("README.md"), "seed\n").unwrap();
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["add", "README.md"])
        .status();
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["commit", "-qm", "seed"])
        .status();
}

/// Acceptance Criteria 1, 2, 3 and Blockers 2 & 3:
/// Call session_start inside an agent session to mint the lease.
/// Verify lease records agent PID for both creator_pid and active_pid (AC1).
/// Verify agent's registry entry is updated with current_spec and worktree_path (Blocker 2).
/// Verify session_leases classifies lease as Live (Blocker 3).
/// Verify gather_running_work recognizes lane as Live and excludes it from orphans (AC2 & AC3).
// trace:BUG-2010 | ai:antigravity
#[test]
fn test_session_start_mints_factory_lane_lease_with_agent_pid_and_session_leases_live() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    init_git_repo(&root);

    let live_pid = std::process::id();
    let lane_name = "antigravity-worker-1";
    let agent = agent_entry("antigravity", lane_name, live_pid, None, root.clone());
    agent_registry::write_entry(&root, &agent).unwrap();

    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &root,
        "implementer",
        &[],
        &[
            ("AIDA_AGENT_NAME", Some(lane_name)),
            ("AIDA_AGENT_TYPE", Some("antigravity")),
        ],
    );

    // Call session_start to mint the lease
    crate::session_start::session_start(
        "TASK-2010",
        None,
        None,
        false,
        None,
        None,
        "auto",
        false,
        None,
        false,
        None,
        None,
        false,
        None,
        false,
        Some(false),
        /* self_claim */ true,
    )
    .unwrap();

    // Verify AC1: lease owner, creator_pid, and active_pid match the live agent process
    let leases = crate::leases::list_leases(&root);
    assert_eq!(leases.len(), 1, "expected 1 minted lease");
    let lease = &leases[0];
    assert_eq!(lease.scope, "TASK-2010");
    assert_eq!(lease.owner, lane_name);
    assert_eq!(lease.creator_pid, Some(live_pid));
    assert_eq!(lease.creator_pid_start_time, agent.pid_start);
    assert_eq!(lease.active_pid, Some(live_pid));
    assert_eq!(lease.active_pid_start_time, agent.pid_start);

    // Verify Blocker 2: agent registry entry was updated with current_spec and worktree_path
    let entries = agent_registry::load_entries(&root);
    let (_, updated_agent) = entries
        .into_iter()
        .find(|(_, e)| e.id == agent.id)
        .expect("agent entry must exist");
    assert_eq!(updated_agent.current_spec.as_deref(), Some("TASK-2010"));
    assert_eq!(updated_agent.worktree_path, lease.worktree_path);

    // Verify direct classification through classify_session_leases
    let classified = crate::leases::classify_session_leases(&root, &leases);
    assert_eq!(classified.len(), 1);
    assert_eq!(classified[0].1, LeaseState::Live);
    assert_eq!(classified[0].2, Some(live_pid));

    // Verify aida ps (gather_running_work) recognizes lane as live and prunes from orphans
    let (rows, orphans) = gather_running_work(&root);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, LeaseState::Live);
    assert_eq!(rows[0].pid, Some(live_pid));
    assert!(
        orphans.iter().all(|o| o.spec != "TASK-2010"),
        "orphans must exclude specs whose lease belongs to a live registered agent: {orphans:?}"
    );
}

/// Blocker 1: Recycled lane names must NOT revive dead leases.
/// Even if agent.name matches lease.owner, if the lease was started before the agent
/// and does not match active_pid with start-time identity, the lease must remain dead.
// trace:BUG-2010 | ai:antigravity
#[test]
fn test_recycled_lane_name_does_not_revive_dead_lease() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();

    let live_pid = std::process::id();
    let lane_name = "codex-lane-recycled";

    // Old dead lease from a previous session on the same lane name (started 2 hours ago)
    let old_started = chrono::Utc::now() - chrono::Duration::hours(2);
    let mut old_lease = ps_lease("lease-old", "TASK-100", lane_name, root.join("old-wt"));
    old_lease.started_at = old_started;
    old_lease.creator_pid = Some(99999);
    old_lease.active_pid = Some(99999);
    old_lease.active_pid_start_time = None;

    // New agent started just now with a different live PID
    let new_agent = agent_entry("codex", lane_name, live_pid, Some("TASK-200"), root.clone());
    let live_agents = vec![new_agent];

    // Must NOT match: old lease was started before new agent and active_pid doesn't match
    let matched = agent_registry::live_agent_for_lease_or_spec(
        &live_agents,
        Some(&old_lease),
        Some("TASK-100"),
    );
    assert!(
        matched.is_none(),
        "recycled lane name must not revive dead lease from previous session"
    );

    // Also test when active_pid is None (e.g. legacy lease without active_pid):
    let mut old_lease_no_active_pid = old_lease.clone();
    old_lease_no_active_pid.active_pid = None;
    let matched_no_active = agent_registry::live_agent_for_lease_or_spec(
        &live_agents,
        Some(&old_lease_no_active_pid),
        Some("TASK-100"),
    );
    assert!(
        matched_no_active.is_none(),
        "recycled lane name must not revive old lease even if active_pid is None"
    );

    // Test recycled PID without lease start time:
    // If lease has matching active_pid but NO active_pid_start_time, it must NOT match.
    let mut recycled_pid_lease = old_lease.clone();
    recycled_pid_lease.active_pid = Some(live_pid);
    recycled_pid_lease.active_pid_start_time = None;
    let matched_recycled_no_start = agent_registry::live_agent_for_lease_or_spec(
        &live_agents,
        Some(&recycled_pid_lease),
        Some("TASK-100"),
    );
    assert!(
        matched_recycled_no_start.is_none(),
        "recycled pid without lease start time must not match"
    );

    // Test recycled PID with different start time:
    let mut recycled_pid_diff_start = old_lease.clone();
    recycled_pid_diff_start.active_pid = Some(live_pid);
    recycled_pid_diff_start.active_pid_start_time = Some("1999-01-01T00:00:00Z".to_string());
    let matched_diff_start = agent_registry::live_agent_for_lease_or_spec(
        &live_agents,
        Some(&recycled_pid_diff_start),
        Some("TASK-100"),
    );
    assert!(
        matched_diff_start.is_none(),
        "recycled pid with non-matching start time must not match"
    );

    // Test started_recently without active_pid:
    // Even if started within 5s of agent, without matching active_pid and start time it must NOT match
    let mut quick_relaunch_lease = old_lease.clone();
    quick_relaunch_lease.started_at = chrono::Utc::now();
    let matched_quick = agent_registry::live_agent_for_lease_or_spec(
        &live_agents,
        Some(&quick_relaunch_lease),
        Some("TASK-100"),
    );
    assert!(
        matched_quick.is_none(),
        "quick relaunch must not match without active_pid and start-time identity"
    );

    // Direct classification: old lease must remain non-Live
    let classified = crate::leases::classify_session_leases(&root, &[old_lease.clone()]);
    assert_ne!(
        classified[0].1,
        LeaseState::Live,
        "old lease must not be classified as Live"
    );
}

/// Follow-up 4: A spoofed AIDA_AGENT_NAME pointing to a real live process that is
/// NOT current process and NOT an ancestor must be rejected by resolve_session_agent_for_lease.
// trace:BUG-2010 | ai:antigravity
#[cfg(unix)]
#[test]
fn test_spoofed_agent_name_with_live_non_ancestor_pid_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // Spawn a real, live child process (e.g. sleep 30)
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn sleep child");
    let child_pid = child.id();

    let agent = agent_entry(
        "codex",
        "codex-spoofed-lane",
        child_pid,
        None,
        root.to_path_buf(),
    );
    agent_registry::write_entry(root, &agent).unwrap();

    let _guard = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_AGENT_NAME", Some("codex-spoofed-lane")),
        ("AIDA_AGENT_TYPE", Some("codex")),
    ]);

    // Child is alive, but std::process::id() != child_pid and child is not an ancestor of current process
    assert!(crate::process_probe::pid_is_alive(child_pid));
    assert_ne!(std::process::id(), child_pid);

    let resolved = agent_registry::resolve_session_agent_for_lease(root);
    assert!(
        resolved.is_none(),
        "spoofed agent name must be rejected because child_pid is not current process or ancestor"
    );

    let _ = child.kill();
    let _ = child.wait();
}

/// Follow-up 5: Drain phase children and Claude sessions skip factory lane lease attribution.
/// Leases minted under drain keep active_pid None and creator_pid equal to the drain orchestrator PID,
/// ensuring BUG-1930 predecessor checks hold.
// trace:BUG-2010 | ai:antigravity
#[test]
fn test_drain_phase_child_and_claude_leases_preserve_predecessor_and_sigterm_invariants() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    init_git_repo(&root);

    // Register an agent (e.g. an implementer lane with AIDA_AGENT_NAME set)
    let live_pid = std::process::id();
    let lane_name = "antigravity-worker-drain";
    let agent = agent_entry("antigravity", lane_name, live_pid, None, root.clone());
    agent_registry::write_entry(&root, &agent).unwrap();

    // Create a corroborated drain token in root
    let token = uuid::Uuid::now_v7().to_string();
    let mut state = crate::drain_state::DrainState::new_single("BUG-1930", &token, false);
    state.orchestrator_pid = live_pid;
    state.write(&root).unwrap();
    crate::orchestrator::publish_run(
        &root,
        &token,
        &crate::orchestrator::RunMarker {
            pid: live_pid,
            spec: "BUG-1930".into(),
            started_at: state.started_at,
            zen: false,
        },
    )
    .unwrap();

    // Assert that orchestrator::detect finds an Orchestrated context
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &root,
        "implementer",
        &[],
        &[
            (crate::orchestrator::AUTO_COMPLETE_ENV, Some("1")),
            (crate::orchestrator::TOKEN_ENV, Some(&token)),
            ("AIDA_AGENT_NAME", Some(lane_name)),
            ("AIDA_AGENT_TYPE", Some("antigravity")),
        ],
    );

    assert!(
        crate::orchestrator::detect(&root).is_orchestrated(),
        "drain context must be corroborated"
    );

    // Mint the lease through session_start in the orchestrated context.
    // Even though self_claim is true and AIDA_AGENT_NAME is set,
    // the is_drain_child branch MUST prevent attribution to the agent.
    crate::session_start::session_start(
        "BUG-1930",
        None,
        None,
        false,
        None,
        None,
        "auto",
        false,
        None,
        false,
        None,
        None,
        false,
        None,
        false,
        Some(false),
        /* self_claim */ true,
    )
    .unwrap();

    let leases = crate::leases::list_leases(&root);
    let lease = leases
        .iter()
        .find(|l| l.scope == "BUG-1930")
        .expect("minted lease for BUG-1930 must exist");

    // Under drain, active_pid and active_pid_start_time must be None
    assert_eq!(
        lease.active_pid, None,
        "is_drain_child must prevent setting active_pid on lease minted under drain"
    );
    assert_eq!(
        lease.active_pid_start_time, None,
        "lease active_pid_start_time must be None under drain"
    );
    assert_ne!(
        lease.owner, lane_name,
        "lease owner must not be attributed to agent under drain"
    );

    // BUG-1930 predecessor check MUST hold
    let orch_pid = lease
        .creator_pid
        .expect("minted lease must have creator_pid");
    let orch_start = lease.creator_pid_start_time.clone();
    assert!(
        leases::lease_minted_by_returned_phase_child(lease, orch_pid, orch_start.as_deref()),
        "BUG-1930 predecessor check must succeed when active_pid is None and creator_pid matches"
    );

    // If active_pid was erroneously set (as would happen if is_drain_child branch were deleted),
    // predecessor check would fail
    let mut broken_lease = lease.clone();
    broken_lease.active_pid = Some(orch_pid);
    broken_lease.active_pid_start_time = orch_start.clone();
    assert!(
        !leases::lease_minted_by_returned_phase_child(
            &broken_lease,
            orch_pid,
            orch_start.as_deref()
        ),
        "BUG-1930 predecessor check must fail if active_pid was erroneously attributed"
    );
}

/// Item 1: Leases minted for ANOTHER agent (e.g. via `agent new --spec X`) must not
/// be attributed to the calling lane, and the calling lane's registry entry must not
/// be overwritten.
// trace:BUG-2010 | ai:antigravity
#[test]
fn test_agent_new_does_not_attribute_lease_to_calling_lane() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    init_git_repo(&root);

    let lane_a_pid = std::process::id();
    let lane_a_name = "lane-a-calling";
    let mut lane_a = agent_entry(
        "codex",
        lane_a_name,
        lane_a_pid,
        None,
        root.join("lane-a-wt"),
    );
    lane_a.current_spec = Some("TASK-1000".to_string());
    agent_registry::write_entry(&root, &lane_a).unwrap();

    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &root,
        "implementer",
        &[],
        &[
            ("AIDA_AGENT_NAME", Some(lane_a_name)),
            ("AIDA_AGENT_TYPE", Some("codex")),
        ],
    );

    // Calling lane A runs prepare_agent_launch for a new agent B on TASK-2000
    let plan = crate::agent_launch::prepare_agent_launch(
        &root,
        Some("implementer".into()),
        Some("TASK-2000".into()),
        "codex",
        None,
    )
    .expect("prepare_agent_launch should succeed");

    assert_eq!(plan.current_spec.as_deref(), Some("TASK-2000"));

    // Assert 1: Calling lane A's registry entry is completely UNCHANGED
    let entries = agent_registry::load_entries(&root);
    let (_, lane_a_after) = entries
        .into_iter()
        .find(|(_, e)| e.id == lane_a.id)
        .expect("lane A record must still exist");
    assert_eq!(
        lane_a_after.current_spec.as_deref(),
        Some("TASK-1000"),
        "lane A current_spec must NOT be overwritten by agent new"
    );
    assert_eq!(
        lane_a_after.worktree_path,
        root.join("lane-a-wt"),
        "lane A worktree_path must NOT be overwritten by agent new"
    );

    // Assert 2: Lease for TASK-2000 is NOT lane A's
    let leases = crate::leases::list_leases(&root);
    let lease_x = leases
        .iter()
        .find(|l| l.scope == "TASK-2000")
        .expect("lease for TASK-2000 must exist");

    assert_ne!(
        lease_x.owner, lane_a_name,
        "lease for TASK-2000 must not be owned by calling lane A"
    );
    assert_ne!(
        lease_x.active_pid,
        Some(lane_a_pid),
        "lease for TASK-2000 active_pid must not be calling lane A's pid"
    );
    assert_eq!(
        lease_x.active_pid, None,
        "lease for TASK-2000 active_pid must be None until child agent runs"
    );
    assert_eq!(
        lease_x.active_pid_start_time, None,
        "lease for TASK-2000 active_pid_start_time must be None until child agent runs"
    );

    // Assert 3: ps / live_agent_for_lease_or_spec does NOT attribute lease X to lane A
    let live_agents = [lane_a_after];
    let matched = agent_registry::live_agent_for_lease_or_spec(
        &live_agents,
        Some(lease_x),
        Some("TASK-2000"),
    );
    assert!(
        matched.is_none(),
        "lease X must not be matched to calling lane A"
    );
}

/// Item 1 (queue work): Leases minted via `queue work` (which launches by default)
/// must not be attributed to the calling lane, and the calling lane's registry entry
/// must not be overwritten. With `--no-launch`, the lease IS attributed to the caller.
// trace:BUG-2010 | ai:antigravity
#[test]
fn test_queue_work_respects_no_launch_for_calling_lane_attribution() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    init_git_repo(&root);

    let lane_a_pid = std::process::id();
    let lane_a_name = "lane-a-calling";
    let mut lane_a = agent_entry(
        "codex",
        lane_a_name,
        lane_a_pid,
        None,
        root.join("lane-a-wt"),
    );
    lane_a.current_spec = Some("TASK-1000".to_string());
    agent_registry::write_entry(&root, &lane_a).unwrap();

    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &root,
        "implementer",
        &["implementer"],
        &[
            ("AIDA_AGENT_NAME", Some(lane_a_name)),
            ("AIDA_AGENT_TYPE", Some("codex")),
            ("AIDA_AGENT_CMD", Some("/bin/sh")),
        ],
    );

    let store_root = root.join(".aida-store");
    let backend = aida_core::GitBackend::new(&store_root).unwrap();
    let storage = Storage::new(&store_root);

    let registry_dir = store_root.join("registry");
    std::fs::create_dir_all(&registry_dir).unwrap();
    std::fs::write(
        registry_dir.join("team.toml"),
        format!(
            "[members]\n\"{}\" = [\"implementer\"]\n",
            crate::test_env::HERMETIC_TEST_USER
        ),
    )
    .unwrap();

    let mut store = aida_core::RequirementsStore::default();
    let mut req1 = aida_core::Requirement::new("Spec for launch".to_string(), String::new());
    req1.spec_id = Some("TASK-2000".to_string());
    req1.status = aida_core::RequirementStatus::Approved;
    store.requirements.push(req1);

    let mut req2 = aida_core::Requirement::new("Spec for no-launch".to_string(), String::new());
    req2.spec_id = Some("TASK-3000".to_string());
    req2.status = aida_core::RequirementStatus::Approved;
    store.requirements.push(req2);
    backend.save(&store).unwrap();

    let prev_cwd = std::env::current_dir().unwrap();

    // 1. Launch case at the session_start(self_claim=false) level (as queue work passes when no_launch: false)
    std::env::set_current_dir(&root).unwrap();
    let res1 = crate::session_start::session_start(
        "TASK-2000",
        None,
        None,
        /* reuse_branch */ false,
        None,
        /* forge_override */ None,
        /* branch_style */ "auto",
        /* launch_claude */ false,
        None,
        false,
        None,
        None,
        false,
        Some("implementer".to_string()),
        /* force_claim */ true,
        /* use_pool */ None,
        /* self_claim */ false,
    );
    assert!(
        res1.is_ok(),
        "session_start(self_claim=false) should succeed: {res1:?}"
    );

    // Assert 1: Calling lane A's registry entry is completely UNCHANGED
    let entries = agent_registry::load_entries(&root);
    let (_, lane_a_after) = entries
        .into_iter()
        .find(|(_, e)| e.id == lane_a.id)
        .expect("lane A record must still exist");
    assert_eq!(
        lane_a_after.current_spec.as_deref(),
        Some("TASK-1000"),
        "lane A current_spec must NOT be overwritten by queue work with launch"
    );
    assert_eq!(
        lane_a_after.worktree_path,
        root.join("lane-a-wt"),
        "lane A worktree_path must NOT be overwritten by queue work with launch"
    );

    // Assert 2: Lease for TASK-2000 is NOT lane A's
    let leases = crate::leases::list_leases(&root);
    let lease_x = leases
        .iter()
        .find(|l| l.scope == "TASK-2000")
        .expect("lease for TASK-2000 must exist");

    assert_ne!(
        lease_x.owner, lane_a_name,
        "lease for TASK-2000 must not be owned by calling lane A"
    );
    assert_ne!(
        lease_x.active_pid,
        Some(lane_a_pid),
        "lease for TASK-2000 active_pid must not be calling lane A's pid"
    );
    assert_eq!(
        lease_x.active_pid, None,
        "lease for TASK-2000 active_pid must be None until child agent runs"
    );
    assert_eq!(
        lease_x.active_pid_start_time, None,
        "lease for TASK-2000 active_pid_start_time must be None until child agent runs"
    );

    // Assert 3: ps / live_agent_for_lease_or_spec does NOT attribute lease X to lane A
    let live_agents = [lane_a_after];
    let matched = agent_registry::live_agent_for_lease_or_spec(
        &live_agents,
        Some(lease_x),
        Some("TASK-2000"),
    );
    assert!(
        matched.is_none(),
        "lease X must not be matched to calling lane A"
    );

    // 2. With --no-launch (no_launch: true) - calling lane A runs queue work for TASK-3000
    std::env::set_current_dir(&root).unwrap();
    let res = crate::queue_cmd::handle_queue_work(
        &storage,
        crate::test_env::HERMETIC_TEST_USER,
        Some("TASK-3000"),
        None,
        false,
        /* no_launch */ true,
        Some("implementer"),
        true,
        None,
        None,
        None,
        false,
        None,
        false,
        false,
        false,
        false,
        None,
        false,
        false,
        None,
        "codex",
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
    );
    assert!(
        res.is_ok(),
        "queue work --no-launch should succeed: {res:?}"
    );

    // Assert 4: With --no-launch, the lease IS attributed to lane A
    let leases = crate::leases::list_leases(&root);
    let lease_y = leases
        .iter()
        .find(|l| l.scope == "TASK-3000")
        .expect("lease for TASK-3000 must exist");

    assert_eq!(
        lease_y.owner, lane_a_name,
        "lease for TASK-3000 must be owned by calling lane A under --no-launch"
    );
    assert_eq!(
        lease_y.active_pid,
        Some(lane_a_pid),
        "lease for TASK-3000 active_pid must be calling lane A's pid under --no-launch"
    );
    assert_eq!(
        lease_y.active_pid_start_time, lane_a.pid_start,
        "lease for TASK-3000 active_pid_start_time must match calling lane A's start time under --no-launch"
    );

    // Assert 5: Calling lane A's registry entry IS updated with TASK-3000 and the worktree path
    let entries = agent_registry::load_entries(&root);
    let (_, lane_a_final) = entries
        .into_iter()
        .find(|(_, e)| e.id == lane_a.id)
        .expect("lane A record must still exist");
    assert_eq!(
        lane_a_final.current_spec.as_deref(),
        Some("TASK-3000"),
        "lane A current_spec must be updated to TASK-3000 under --no-launch"
    );
    assert_eq!(
        lane_a_final.worktree_path, lease_y.worktree_path,
        "lane A worktree_path must be updated to lease worktree path under --no-launch"
    );

    // Assert 6: ps / live_agent_for_lease_or_spec DOES attribute lease Y to lane A
    let live_agents_final = [lane_a_final];
    let matched_no_launch = agent_registry::live_agent_for_lease_or_spec(
        &live_agents_final,
        Some(lease_y),
        Some("TASK-3000"),
    );
    assert!(
        matched_no_launch.is_some(),
        "lease Y must be matched to calling lane A under --no-launch"
    );

    let _ = std::env::set_current_dir(prev_cwd);
}

/// Blocker 2: update_agent_current_spec updates only the target agent's entry
/// and preserves availability / paused_reason metadata. Peer entries are untouched.
// trace:BUG-2010 | ai:antigravity
#[test]
fn test_update_agent_current_spec_preserves_metadata_and_leaves_peers_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // Peer agent (PID 77777 != target agent)
    let peer_pid = 77777;
    let peer_agent = agent_entry(
        "codex",
        "peer-agent-lane",
        peer_pid,
        None,
        root.join("peer-wt"),
    );
    agent_registry::write_entry(root, &peer_agent).unwrap();

    // Target agent with paused availability
    let my_pid = std::process::id();
    let mut my_agent = agent_entry("antigravity", "my-lane", my_pid, None, root.join("old-wt"));
    my_agent.availability = agent_registry::Availability::Paused;
    my_agent.paused_reason = Some(agent_registry::PauseReason::Budget);
    agent_registry::write_entry(root, &my_agent).unwrap();

    // Update target agent's current_spec
    let update_res = agent_registry::update_agent_current_spec(
        root,
        &my_agent.id,
        "TASK-9999",
        &root.join("my-new-wt"),
    );
    assert!(update_res.is_ok());

    // Peer agent's record must be COMPLETELY UNTOUCHED
    let entries = agent_registry::load_entries(root);
    let (_, peer_after) = entries
        .iter()
        .find(|(_, e)| e.pid == peer_pid)
        .expect("peer record must still exist");
    assert_eq!(
        peer_after.current_spec, None,
        "peer agent current_spec must NOT be rewritten"
    );
    assert_eq!(
        peer_after.worktree_path,
        root.join("peer-wt"),
        "peer agent worktree_path must NOT be rewritten"
    );

    // Target agent's entry: current_spec and worktree_path updated, paused metadata preserved
    let (_, my_after) = entries
        .iter()
        .find(|(_, e)| e.pid == my_pid)
        .expect("my record must exist");
    assert_eq!(
        my_after.current_spec,
        Some("TASK-9999".to_string()),
        "target agent current_spec should be updated"
    );
    assert_eq!(
        my_after.worktree_path,
        root.join("my-new-wt"),
        "target agent worktree_path should be updated"
    );
    assert_eq!(
        my_after.availability,
        agent_registry::Availability::Paused,
        "pause-state must be preserved"
    );
    assert_eq!(
        my_after.paused_reason,
        Some(agent_registry::PauseReason::Budget),
        "paused-reason must be preserved"
    );
}

/// B3: A session in the main checkout does NOT revive a dead `harness-worktree` lease,
/// and that lease stays on the orphan list.
// trace:BUG-2010 | ai:antigravity
#[test]
fn test_main_checkout_session_does_not_revive_dead_harness_lease() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();

    // Dead harness lease rooted at project root with owner "tester"
    let mut dead_harness_lease = ps_lease("harness-lease", "HARNESS-1", "tester", root.clone());
    dead_harness_lease.creator_pid = Some(99999);
    dead_harness_lease.active_pid = Some(99999);

    let row = PsRow {
        lease: dead_harness_lease.clone(),
        state: LeaseState::Stale,
        role: Some("implementer".into()),
        lease_role: Some("implementer".into()),
        pid: None,
        pid_started_at: None,
        elapsed_secs: 500,
        spec: Some("HARNESS-1".into()),
        dispatch: None,
        locked_by: None,
        mail_identity: None,
        activity: None,
        driver: None,
    };
    let mut rows = vec![row];
    let mut orphans = vec![PsOrphan {
        spec: "HARNESS-1".into(),
        title: "Dead harness spec".into(),
        stale_lease: true,
        likely_fanout: false,
        possibly_subagent: false,
        driver: None,
    }];

    // Live agent running in the main checkout (same worktree path!)
    let live_pid = std::process::id();
    let agent = agent_entry(
        "antigravity",
        "antigravity-main-lane",
        live_pid,
        Some("BUG-2010"),
        root,
    );
    let live_agents = vec![agent];

    // Must NOT match because owner ("tester") != agent name ("antigravity-main-lane")
    assert!(
        agent_registry::live_agent_for_lease_or_spec(
            &live_agents,
            Some(&dead_harness_lease),
            Some("HARNESS-1"),
        )
        .is_none(),
        "exact owner match must reject mismatched lease owner even with identical worktree path"
    );

    apply_registered_agent_overlay(
        &live_agents,
        &[dead_harness_lease],
        &mut rows,
        &mut orphans,
        |_| Some(chrono::Utc::now()),
        |_| MailIdentityStatus::Attributed,
    );

    // Row must REMAIN Stale
    assert_eq!(rows[0].state, LeaseState::Stale);
    assert_eq!(rows[0].pid, None);

    // Dead harness lease must STAY on the orphan list
    assert_eq!(
        orphans.len(),
        1,
        "dead harness lease must stay on orphan list: {orphans:?}"
    );
    assert_eq!(orphans[0].spec, "HARNESS-1");
}

/// A live factory lane (e.g. Codex or Antigravity) whose process is not the lease holder's pid
/// must not be reported stale in `aida ps` and must not appear in "Orphaned In-Progress specs".
// trace:BUG-2010 | ai:antigravity
#[test]
fn test_live_factory_lane_not_reported_stale_when_lease_pid_differs() {
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().join("worktree-2010");
    std::fs::create_dir_all(&wt).unwrap();

    let live_pid = std::process::id();
    let lane_name = "antigravity-factory-1";

    // Live factory agent (Codex or Antigravity) running with a live PID
    let agent = agent_entry(
        "antigravity",
        lane_name,
        live_pid,
        Some("BUG-2010"),
        wt.clone(),
    );
    let live_agents = vec![agent.clone()];

    // The lease was minted with the factory lane owner and active_pid = agent.pid,
    // but creator_pid was a transient process (e.g. 99999) that died.
    let mut lease = ps_lease("lease-2010", "BUG-2010", lane_name, wt.clone());
    lease.creator_pid = Some(99999);
    lease.active_pid = Some(live_pid);
    lease.active_pid_start_time = agent.pid_start.clone();

    let row = PsRow {
        lease: lease.clone(),
        state: LeaseState::Stale,
        role: Some("implementer".into()),
        lease_role: Some("implementer".into()),
        pid: None,
        pid_started_at: None,
        elapsed_secs: 100,
        spec: Some("BUG-2010".into()),
        dispatch: Some(PsDispatch {
            state: dispatch_health_ps::DispatchState::Salvageable,
            hint: Some("salvage".into()),
            dirty: true,
            ahead_of_main: 0,
            untracked_only: false,
            commit_movement_from: None,
        }),
        locked_by: None,
        mail_identity: None,
        activity: None,
        driver: None,
    };
    let mut rows = vec![row];
    let mut orphans = vec![PsOrphan {
        spec: "BUG-2010".into(),
        title: "Test bug".into(),
        stale_lease: true,
        likely_fanout: false,
        possibly_subagent: false,
        driver: None,
    }];

    apply_registered_agent_overlay(
        &live_agents,
        &[lease],
        &mut rows,
        &mut orphans,
        |_| Some(chrono::Utc::now()),
        |_| MailIdentityStatus::Attributed,
    );

    // Row must now be Live with the agent's PID, and not Salvageable or Stalled
    assert_eq!(rows[0].state, LeaseState::Live);
    assert_eq!(rows[0].pid, Some(live_pid));
    assert_eq!(
        rows[0].dispatch.as_ref().map(|d| d.state),
        Some(dispatch_health_ps::DispatchState::Moving)
    );

    // "Orphaned In-Progress" must exclude this spec
    assert!(
        orphans.is_empty(),
        "orphans must exclude specs whose lease belongs to a live registered agent: {orphans:?}"
    );
}
