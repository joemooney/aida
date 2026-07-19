use super::*;

fn input(status: agent_registry::AgentStatus, dirty: bool) -> DispatchReportInput {
    DispatchReportInput {
        agent: "codex-1".to_string(),
        agent_type: "codex".to_string(),
        spec: Some("STORY-759".to_string()),
        status,
        liveness_stalled: if status == agent_registry::AgentStatus::Stale {
            Some(true)
        } else {
            Some(false)
        },
        paused: false,
        worktree: std::path::PathBuf::from("/tmp/story-759"),
        branch: Some("story-759-dispatch-report".to_string()),
        dirty,
        ahead_main: Some(0),
        ahead_upstream: Some(0),
        has_upstream: false,
        pending_briefs: 0,
    }
}

#[test]
fn dispatch_report_marks_dead_agent_dirty_worktree_salvageable() {
    let row = dispatch_report_row(&input(agent_registry::AgentStatus::Stale, true), &[], false);
    assert_eq!(row.state, DispatchHealthState::Salvageable);
    assert!(row.guidance.contains("salvage"));
    assert!(row.guidance.contains("dirty worktree"));
    assert!(!row.guidance.contains("delete"));
}

#[test]
fn dispatch_report_marks_pushed_branch_resumable() {
    let mut facts = input(agent_registry::AgentStatus::Stale, false);
    facts.ahead_main = Some(2);
    facts.has_upstream = true;
    facts.ahead_upstream = Some(0);
    let row = dispatch_report_row(&facts, &[], false);
    assert_eq!(row.state, DispatchHealthState::Stalled);
    assert!(row.pushed);
    assert!(
        row.guidance
            .contains("aida agent new codex --spec STORY-759 --cwd /tmp/story-759"),
        "{}",
        row.guidance
    );
}

#[test]
fn dispatch_report_skips_paused_vendor_for_fallback() {
    let paused = vec!["codex".to_string()];
    assert_eq!(
        select_dispatch_fallback("claude", &["codex", "antigravity"], &paused, false),
        Some("antigravity".to_string())
    );
    assert_eq!(
        select_dispatch_fallback("claude", &["codex", "antigravity"], &paused, true),
        Some("codex".to_string())
    );
}

#[test]
fn dispatch_policy_routes_agy_as_human_briefed() {
    let guidance = dispatch_fallback_guidance("antigravity", "STORY-759");
    assert!(guidance.contains("aida brief antigravity STORY-759 --notify"));
    assert!(!guidance.contains("headless fallback"));
}

#[test]
fn dispatch_liveness_resets_on_dirty_diff_or_commit() {
    let previous = DispatchLivenessSnapshot {
        child_reaped: false,
        head: "a".to_string(),
        dirty_fingerprint: String::new(),
    };
    let dirty_changed = DispatchLivenessSnapshot {
        child_reaped: true,
        head: "a".to_string(),
        dirty_fingerprint: " M aida-cli/src/main.rs".to_string(),
    };
    let head_changed = DispatchLivenessSnapshot {
        child_reaped: true,
        head: "b".to_string(),
        dirty_fingerprint: String::new(),
    };
    assert!(!dispatch_liveness_stalled(&previous, &dirty_changed));
    assert!(!dispatch_liveness_stalled(&previous, &head_changed));
}

#[test]
fn dispatch_liveness_fires_on_dead_child_no_progress() {
    let previous = DispatchLivenessSnapshot {
        child_reaped: false,
        head: "a".to_string(),
        dirty_fingerprint: String::new(),
    };
    let current = DispatchLivenessSnapshot {
        child_reaped: true,
        head: "a".to_string(),
        dirty_fingerprint: String::new(),
    };
    assert!(dispatch_liveness_stalled(&previous, &current));
}

#[test]
fn dispatch_liveness_snapshot_persistence_round_trip_classifies_progress_and_stall() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let agent = "codex-1".to_string();
    let previous = DispatchLivenessSnapshot {
        child_reaped: false,
        head: "a".to_string(),
        dirty_fingerprint: String::new(),
    };
    let mut ledger = DispatchSnapshotLedger::new();
    ledger.insert(agent.clone(), previous);
    persist_dispatch_snapshots(root, &ledger);

    let changed = DispatchLivenessSnapshot {
        child_reaped: true,
        head: "b".to_string(),
        dirty_fingerprint: String::new(),
    };
    let loaded = load_dispatch_snapshots(root);
    assert!(!dispatch_liveness_stalled(
        loaded.get(&agent).unwrap(),
        &changed
    ));

    let unchanged_reaped = DispatchLivenessSnapshot {
        child_reaped: true,
        head: "a".to_string(),
        dirty_fingerprint: String::new(),
    };
    assert!(dispatch_liveness_stalled(
        loaded.get(&agent).unwrap(),
        &unchanged_reaped
    ));
}

#[test]
fn dispatch_liveness_first_run_has_no_false_stall() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let loaded = load_dispatch_snapshots(root);
    let current = DispatchLivenessSnapshot {
        child_reaped: true,
        head: "a".to_string(),
        dirty_fingerprint: String::new(),
    };
    let liveness_stalled = loaded
        .get("codex-1")
        .map(|previous| dispatch_liveness_stalled(previous, &current));
    assert_eq!(liveness_stalled, None);

    let mut facts = input(agent_registry::AgentStatus::Stale, false);
    facts.liveness_stalled = liveness_stalled;
    let row = dispatch_report_row(&facts, &[], false);
    assert_eq!(row.state, DispatchHealthState::Moving);
}

#[test]
fn dispatch_cli_output_uses_toon_in_agent_mode() {
    let row = dispatch_report_row(&input(agent_registry::AgentStatus::Busy, false), &[], false);
    let out = render_dispatch_toon(&[row]);
    assert!(
            out.starts_with("dispatch[1]{agent,spec,state,vendor,branch,dirty,ahead_main,pushed,pending_briefs,guidance}:"),
            "{out}"
        );
    assert!(out.contains("STORY-759"));
    assert!(!out.contains("Dispatch health"));
}
