use super::*;

fn agent_view(id: &str, status: agent_registry::AgentStatus) -> agent_registry::AgentRegistryView {
    let now = chrono::Utc::now();
    agent_registry::AgentRegistryView {
        id: id.to_string(),
        agent_type: "claude".to_string(),
        pid: 4242,
        name: None,
        tty: None,
        started_at: now,
        last_active_at: now,
        role: Some("implementer".to_string()),
        current_spec: None,
        worktree_path: std::path::PathBuf::from("/tmp/aida-fixture"),
        source: "mcp".to_string(),
        binary_version: None,
        build_sha: None,
        status,
        availability: agent_registry::Availability::Available,
        paused_since: None,
        paused_reason: None,
        expected_back: None,
    }
}

fn wt_row(branch: &str, obsolete: bool) -> WorktreeStatusRow {
    WorktreeStatusRow {
        path: std::path::PathBuf::from(format!("/tmp/wt/{branch}")),
        branch: branch.to_string(),
        tied_spec: None,
        lease_scope: None,
        has_live: false,
        dirty_count: 0,
        ahead: None,
        pr_number: None,
        pr_ci: None,
        pr_mergeable: None,
        obsolete,
    }
}

#[test]
fn agent_count_excludes_dead_pid_corpses() {
    // Fixture roster: 2 live (Busy/Idle) + 3 stale (dead-PID) corpses.
    let roster = vec![
        agent_view("live-busy", agent_registry::AgentStatus::Busy),
        agent_view("corpse-1", agent_registry::AgentStatus::Stale),
        agent_view("live-idle", agent_registry::AgentStatus::Idle),
        agent_view("corpse-2", agent_registry::AgentStatus::Stale),
        agent_view("corpse-3", agent_registry::AgentStatus::Stale),
    ];
    let (live, stale) = partition_agents_by_liveness(&roster);
    // Headline counts only the live partition (the 2 non-stale), not the 5
    // raw registrations — the dead-PID corpses are excluded.
    assert_eq!(live.len(), 2, "headline must count only live agents");
    assert_eq!(stale.len(), 3, "dead-PID corpses partitioned into stale");
    assert!(live
        .iter()
        .all(|a| a.status != agent_registry::AgentStatus::Stale));
    assert!(stale
        .iter()
        .all(|a| a.status == agent_registry::AgentStatus::Stale));
}

#[test]
fn agent_partition_all_live_yields_no_stale_footer() {
    let roster = vec![
        agent_view("a", agent_registry::AgentStatus::Busy),
        agent_view("b", agent_registry::AgentStatus::Idle),
    ];
    let (live, stale) = partition_agents_by_liveness(&roster);
    assert_eq!(live.len(), 2);
    assert!(stale.is_empty(), "no stale → no footer count");
}

#[test]
fn worktree_summary_collapses_above_threshold() {
    // A roster larger than the threshold collapses to a one-line summary
    // that reports the total + obsolete count + the `aida session gc` hint.
    let mut rows = Vec::new();
    for i in 0..(WORKTREE_SUMMARY_THRESHOLD + 5) {
        // Mark roughly half obsolete to exercise the tally.
        rows.push(wt_row(&format!("task-{i}"), i % 2 == 0));
    }
    let total = rows.len();
    let obsolete = rows.iter().filter(|r| r.obsolete).count();
    let line = worktree_summary_line(&rows);
    assert!(
        line.contains(&format!("Worktrees: {total}")),
        "summary reports the total count, got: {line}"
    );
    assert!(
        line.contains(&format!("{obsolete} obsolete")),
        "summary reports the obsolete tally, got: {line}"
    );
    assert!(
        line.contains("aida session gc"),
        "summary points at the reaper, got: {line}"
    );
    assert!(
        line.contains("--all"),
        "summary tells the operator how to see the full list, got: {line}"
    );
    // The collapse threshold itself: this many rows is over the line.
    assert!(rows.len() > WORKTREE_SUMMARY_THRESHOLD);
}

#[test]
fn worktree_summary_omits_obsolete_clause_when_none() {
    let rows: Vec<_> = (0..(WORKTREE_SUMMARY_THRESHOLD + 2))
        .map(|i| wt_row(&format!("task-{i}"), false))
        .collect();
    let line = worktree_summary_line(&rows);
    assert!(line.contains(&format!("Worktrees: {}", rows.len())));
    assert!(
        !line.contains("obsolete"),
        "no obsolete worktrees → no obsolete clause, got: {line}"
    );
}
