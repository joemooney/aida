use super::*;
use std::path::PathBuf;

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

    // The lease records the factory lane owner but a dead / transient PID (e.g. 99999).
    let mut lease = ps_lease("lease-2010", "BUG-2010", lane_name, wt.clone());
    lease.creator_pid = Some(99999);
    lease.active_pid = Some(99999);

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

    // Live factory agent (Codex or Antigravity) running with a live PID
    let agent = agent_entry("antigravity", lane_name, live_pid, Some("BUG-2010"), wt);
    let live_agents = vec![agent];

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

/// B1 & B4: Separately necessary conditions for factory lane lease attribution.
/// Disabling any single condition must fail attribution.
// trace:BUG-2010 | ai:antigravity
#[test]
fn test_resolve_session_agent_separately_necessary_conditions() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let live_pid = std::process::id();

    // Base valid entry
    let agent = agent_entry("codex", "codex-lane-1", live_pid, None, root.to_path_buf());
    agent_registry::write_entry(root, &agent).unwrap();

    // Valid case: all conditions satisfied -> resolves
    {
        let _guard = crate::test_env::EnvVarGuard::set("AIDA_AGENT_NAME", "codex-lane-1");
        let res = agent_registry::resolve_session_agent_for_lease(root);
        assert!(res.is_some(), "all valid conditions must resolve");
        assert_eq!(res.unwrap().pid, live_pid);
    }

    // Condition 1: AIDA_AGENT_NAME unset -> must NOT resolve
    {
        let _guard = crate::test_env::EnvVarGuard::unset("AIDA_AGENT_NAME");
        assert!(
            agent_registry::resolve_session_agent_for_lease(root).is_none(),
            "Condition 1 failed: must not resolve when AIDA_AGENT_NAME is unset"
        );
    }

    // Condition 2: name mismatch -> must NOT resolve
    {
        let _guard = crate::test_env::EnvVarGuard::set("AIDA_AGENT_NAME", "unregistered-lane");
        assert!(
            agent_registry::resolve_session_agent_for_lease(root).is_none(),
            "Condition 2 failed: must not resolve when name does not match registry entry"
        );
    }

    // Condition 3: source != "agent-launcher" (e.g. "session-start-hook") -> must NOT resolve
    {
        let mut non_launcher = agent.clone();
        non_launcher.source = "session-start-hook".to_string();
        non_launcher.name = Some("codex-hook-lane".to_string());
        non_launcher.id = format!("codex-hook-{}", live_pid);
        agent_registry::write_entry(root, &non_launcher).unwrap();

        let _guard = crate::test_env::EnvVarGuard::set("AIDA_AGENT_NAME", "codex-hook-lane");
        assert!(
            agent_registry::resolve_session_agent_for_lease(root).is_none(),
            "Condition 3 failed: must not attribute lease to non-agent-launcher entry"
        );
    }

    // Condition 4: agent_type == "claude" -> must NOT resolve (never Claude operator session)
    {
        let mut claude_entry = agent.clone();
        claude_entry.agent_type = "claude".to_string();
        claude_entry.name = Some("claude-operator".to_string());
        claude_entry.id = format!("claude-{}", live_pid);
        agent_registry::write_entry(root, &claude_entry).unwrap();

        let _guard = crate::test_env::EnvVarGuard::set("AIDA_AGENT_NAME", "claude-operator");
        assert!(
            agent_registry::resolve_session_agent_for_lease(root).is_none(),
            "Condition 4 failed: must not attribute lease to Claude operator session"
        );
    }

    // Condition 5: dead PID -> must NOT resolve
    {
        let dead_agent = agent_entry("codex", "codex-dead-lane", 999999, None, root.to_path_buf());
        agent_registry::write_entry(root, &dead_agent).unwrap();

        let _guard = crate::test_env::EnvVarGuard::set("AIDA_AGENT_NAME", "codex-dead-lane");
        assert!(
            agent_registry::resolve_session_agent_for_lease(root).is_none(),
            "Condition 5 failed: must not resolve dead agent lane"
        );
    }

    // Condition 6: spoofed AIDA_AGENT_NAME (PID not current and not ancestor) -> must NOT resolve
    {
        // Another process PID (e.g. 88888) that is alive or dead but not in this process's hierarchy
        let spoofed_agent = agent_entry(
            "codex",
            "codex-remote-lane",
            88888,
            None,
            root.to_path_buf(),
        );
        agent_registry::write_entry(root, &spoofed_agent).unwrap();

        let _guard = crate::test_env::EnvVarGuard::set("AIDA_AGENT_NAME", "codex-remote-lane");
        assert!(
            agent_registry::resolve_session_agent_for_lease(root).is_none(),
            "Condition 6 failed: spoofed AIDA_AGENT_NAME must be rejected by process hierarchy check"
        );
    }
}

/// B1: Leases minted under non-lane sessions (like drain phase children or Claude sessions)
/// keep `active_pid` unset (None) and `creator_pid` equal to the drain/orchestrator pid,
/// ensuring BUG-1930 predecessor checks and TASK-1518/1542 SIGTERM stamping hold.
// trace:BUG-2010 | ai:antigravity
#[test]
fn test_drain_phase_child_and_claude_leases_preserve_predecessor_and_sigterm_invariants() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // Register a Claude operator session
    let claude_pid = std::process::id();
    let claude = agent_registry::AgentRegistryEntry {
        id: format!("claude-{claude_pid}"),
        agent_type: "claude".to_string(),
        pid: claude_pid,
        name: None,
        description: None,
        tty: None,
        terminal: None,
        started_at: chrono::Utc::now(),
        last_active_at: chrono::Utc::now(),
        role: Some("implementer".to_string()),
        current_spec: None,
        worktree_path: root.to_path_buf(),
        source: "session-start-hook".to_string(),
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
    };
    agent_registry::write_entry(root, &claude).unwrap();

    // In a drain phase child or Claude session, AIDA_AGENT_NAME is not set
    let _guard = crate::test_env::EnvVarGuard::unset("AIDA_AGENT_NAME");

    // resolve_session_agent_for_lease must return None
    let resolved = agent_registry::resolve_session_agent_for_lease(root);
    assert!(
        resolved.is_none(),
        "must not resolve Claude session as session_agent"
    );

    // Simulate lease minted under drain
    let drain_orchestrator_pid = 42424;
    let mut lease = ps_lease(
        "drain-child-lease",
        "BUG-1930",
        "drain-runner",
        root.to_path_buf(),
    );
    lease.creator_pid = Some(drain_orchestrator_pid);
    lease.creator_pid_start_time = Some("2026-10-14T00:00:00Z".to_string());
    lease.active_pid = None; // unset, as required
    lease.active_pid_start_time = None;

    // BUG-1930 predecessor check MUST hold
    assert!(
        lease_minted_by_returned_phase_child(
            &lease,
            drain_orchestrator_pid,
            Some("2026-10-14T00:00:00Z")
        ),
        "BUG-1930 predecessor check must succeed when active_pid is None and creator_pid matches"
    );

    // If active_pid was erroneously set (the bug B1 fixed), predecessor check would fail!
    let mut broken_lease = lease.clone();
    broken_lease.active_pid = Some(claude_pid);
    assert!(
        !lease_minted_by_returned_phase_child(
            &broken_lease,
            drain_orchestrator_pid,
            Some("2026-10-14T00:00:00Z")
        ),
        "BUG-1930 predecessor check must fail if active_pid was erroneously attributed"
    );
}

/// B2: session_start must NOT rewrite another agent's registry record.
/// Write only the current process's own registry entry under the registry lock,
/// preserving pause-state and ended_at.
// trace:BUG-2010 | ai:antigravity
#[test]
fn test_session_start_does_not_rewrite_another_agents_registry_record() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // Peer agent (PID 77777 != current process)
    let peer_pid = 77777;
    let peer_agent = agent_entry(
        "codex",
        "peer-agent-lane",
        peer_pid,
        None,
        root.join("peer-wt"),
    );
    agent_registry::write_entry(root, &peer_agent).unwrap();

    // Call update_current_process_entry from this process
    let update_res =
        agent_registry::update_current_process_entry(root, "TASK-9999", &root.join("my-new-wt"));
    assert!(update_res.is_ok());

    // Peer agent's record must be COMPLETELY UNTOUCHED
    let entries = agent_registry::load_entries(root);
    let (_, peer_after) = entries
        .into_iter()
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

    // Current process entry: when it IS registered, updates safely under lock
    let my_pid = std::process::id();
    let mut my_agent = agent_entry("antigravity", "my-lane", my_pid, None, root.join("old-wt"));
    my_agent.availability = agent_registry::Availability::Paused;
    my_agent.paused_reason = Some(agent_registry::PauseReason::Budget);
    agent_registry::write_entry(root, &my_agent).unwrap();

    let update_mine =
        agent_registry::update_current_process_entry(root, "TASK-9999", &root.join("my-new-wt"));
    assert!(update_mine.is_ok());

    let entries2 = agent_registry::load_entries(root);
    let (_, my_after) = entries2
        .into_iter()
        .find(|(_, e)| e.pid == my_pid)
        .expect("my record must exist");
    assert_eq!(
        my_after.current_spec,
        Some("TASK-9999".to_string()),
        "current process entry should be updated with new spec"
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

/// Acceptance criterion 1: Drive session_leases with exact owner match.
// trace:BUG-2010 | ai:antigravity
#[test]
fn test_session_leases_classifies_factory_lane_lease_as_live() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let wt = root.join("wt-lane");
    std::fs::create_dir_all(&wt).unwrap();

    let live_pid = std::process::id();
    let lane_name = "codex-lane-ac1";

    // Lease attributed to factory lane owner, but recorded pid was transient / dead
    let mut lease = ps_lease("lease-ac1", "TASK-100", lane_name, wt.clone());
    lease.creator_pid = Some(live_pid);
    lease.active_pid = Some(99999); // transient PID died

    // Registered factory agent is live
    let agent = agent_entry("codex", lane_name, live_pid, Some("TASK-100"), wt);
    let live_agents = vec![agent];

    // live_agent_for_lease_or_spec must match by exact owner identity
    let matched =
        agent_registry::live_agent_for_lease_or_spec(&live_agents, Some(&lease), Some("TASK-100"));
    assert!(
        matched.is_some(),
        "must match live factory lane by exact owner"
    );
    assert_eq!(matched.unwrap().pid, live_pid);

    // In leases.rs classification:
    let now = chrono::Utc::now();
    let mut state = lease_state_for(&lease, &[], now);
    let mut pid = lease.active_pid.filter(|p| process_probe::pid_is_alive(*p));
    if !matches!(state, LeaseState::Live) {
        if let Some(a) =
            agent_registry::live_agent_for_lease_or_spec(&live_agents, Some(&lease), None)
        {
            state = LeaseState::Live;
            if pid.is_none() {
                pid = Some(a.pid);
            }
        }
    }

    assert_eq!(state, LeaseState::Live);
    assert_eq!(pid, Some(live_pid));
}
