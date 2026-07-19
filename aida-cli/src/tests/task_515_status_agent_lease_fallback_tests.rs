use super::*;
use chrono::TimeZone;

fn lease_for_status(scope: &str, worktree_path: std::path::PathBuf) -> SessionLease {
    SessionLease {
        id: "019elease515".to_string(),
        scope: scope.to_string(),
        slug: scope.to_ascii_lowercase(),
        owner: "tester".to_string(),
        worktree_path,
        branch: scope.to_ascii_lowercase(),
        started_at: chrono::Utc.with_ymd_and_hms(2026, 5, 24, 17, 0, 0).unwrap(),
        hostname: "host".to_string(),
        role: Some("implementer".to_string()),
        creator_pid: Some(4242),
        active_pid: None,
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
    }
}

// TASK-515: raw agent launches may never write `.aida/agents/*.toml`.
// Active leases still need to surface under `aida status` as lease-backed
// agent rows, preserving agent type when session-start captured it.
// trace:TASK-515 | ai:codex
#[test]
fn status_agents_falls_back_to_active_leases_when_registry_empty() {
    let project = tempfile::tempdir().unwrap();
    let worktree = tempfile::tempdir().unwrap();
    let aida_dir = worktree.path().join(".aida");
    std::fs::create_dir_all(&aida_dir).unwrap();
    std::fs::write(
        aida_dir.join("session-env.sh"),
        "export CARGO_TARGET_DIR='/tmp/target'\nexport AIDA_AGENT_TYPE='codex'\n",
    )
    .unwrap();
    let lease = lease_for_status("TASK-515", worktree.path().to_path_buf());
    let ctx = agent_registry::AgentClassifyContext::new(lease.started_at, 30, vec![]);

    let views = merge_agent_views_with_lease_fallback(project.path(), &[lease], vec![], &ctx);

    assert_eq!(views.len(), 1);
    let view = &views[0];
    assert_eq!(view.source, "lease");
    assert_eq!(view.agent_type, "codex");
    assert_eq!(view.current_spec.as_deref(), Some("TASK-515"));
    assert_eq!(view.status, agent_registry::AgentStatus::Busy);
    let line = agent_registry::format_agent_status_lines(&views).remove(0);
    assert!(line.contains("(via lease)"), "{line}");
}

// TASK-515: rich launcher/MCP registry metadata wins over the lease fallback
// for the same scope/worktree to avoid duplicate Active Agents rows.
#[test]
fn status_agents_registry_entry_wins_over_matching_lease() {
    let project = tempfile::tempdir().unwrap();
    let worktree = tempfile::tempdir().unwrap();
    let lease = lease_for_status("TASK-515", worktree.path().to_path_buf());
    let registry = agent_registry::AgentRegistryView {
        id: "codex-123".to_string(),
        agent_type: "codex".to_string(),
        pid: 123,
        name: None,
        tty: None,
        started_at: lease.started_at,
        last_active_at: lease.started_at,
        role: Some("implementer".to_string()),
        current_spec: Some("TASK-515".to_string()),
        worktree_path: worktree.path().to_path_buf(),
        source: "agent-launcher".to_string(),
        binary_version: Some("0.9.1".to_string()),
        build_sha: Some("abc123".to_string()),
        status: agent_registry::AgentStatus::Busy,
        availability: agent_registry::Availability::Available,
        paused_since: None,
        paused_reason: None,
        expected_back: None,
    };
    let ctx = agent_registry::AgentClassifyContext::new(lease.started_at, 30, vec![]);

    let views =
        merge_agent_views_with_lease_fallback(project.path(), &[lease], vec![registry], &ctx);

    assert_eq!(views.len(), 1);
    assert_eq!(views[0].id, "codex-123");
    assert_eq!(views[0].source, "agent-launcher");
}

#[test]
fn test_status_hygiene_section_healthy() {
    let project = tempfile::tempdir().unwrap();
    let store = aida_core::models::RequirementsStore::new();
    let res = print_status_hygiene_section(project.path(), &store, false, false);
    assert!(res.is_ok());
}
