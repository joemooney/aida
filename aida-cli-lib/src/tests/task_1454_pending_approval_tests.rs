use super::*;

fn marker(
    session_id: &str,
    tool: Option<&str>,
    since: chrono::DateTime<chrono::Utc>,
) -> pending_approval::PendingApprovalMarker {
    pending_approval::PendingApprovalMarker {
        session_id: session_id.to_string(),
        tool: tool.map(|s| s.to_string()),
        message: None,
        since,
    }
}

// A present marker ALWAYS wins, regardless of what the transcript/proc-state
// heuristic would otherwise say — the whole point of TASK-1454 is that this
// is ground truth, not a second vote alongside the heuristic.
// trace:TASK-1454 | ai:claude
#[test]
fn marker_present_overrides_working_tail() {
    let now = chrono::Utc::now();
    let m = marker("sess-1", Some("Bash"), now - chrono::Duration::seconds(90));
    // An empty tail (no pending tool) would otherwise classify as Working.
    let activity = seat_activity_with_marker(Some(&m), Some(&[]), false, now);
    assert_eq!(
        activity,
        SeatActivity::Blocked {
            tool: Some("Bash".to_string()),
            secs: 90,
        }
    );
}

// trace:TASK-1454 | ai:claude
#[test]
fn marker_present_overrides_suspended_process() {
    let now = chrono::Utc::now();
    let m = marker("sess-2", None, now - chrono::Duration::seconds(10));
    // proc_stopped=true would otherwise classify as Suspended.
    let activity = seat_activity_with_marker(Some(&m), None, true, now);
    assert_eq!(
        activity,
        SeatActivity::Blocked {
            tool: None,
            secs: 10,
        }
    );
}

// trace:TASK-1454 | ai:claude
#[test]
fn no_marker_falls_back_to_heuristic_classifier() {
    let now = chrono::Utc::now();
    // No marker, no tail at all -> Unknown, exactly like classify_seat_activity.
    let activity = seat_activity_with_marker(None, None, false, now);
    assert_eq!(activity, SeatActivity::Unknown);
    // No marker, proc stopped -> Suspended.
    let activity = seat_activity_with_marker(None, None, true, now);
    assert_eq!(activity, SeatActivity::Suspended);
}

// `SeatActivity::label()` must stay exhaustive and distinct for the new
// variant — the JSON/TOON surfaces both key off this string.
// trace:TASK-1454 | ai:claude
#[test]
fn blocked_label_is_distinct() {
    let activity = SeatActivity::Blocked {
        tool: Some("Write".to_string()),
        secs: 5,
    };
    assert_eq!(activity.label(), "blocked");
}

// `collect_blocked_seat_items` must degrade cleanly (no panic, no error) on
// a project root with no `.aida/pending-approval/` directory at all — the
// overwhelmingly common case on every turn.
// trace:TASK-1454 | ai:claude
#[test]
fn collect_blocked_seat_items_empty_when_no_marker_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let items = collect_blocked_seat_items(tmp.path());
    assert!(items.is_empty());
}

fn test_lease(id: &str, scope: &str, worktree: std::path::PathBuf, pid: u32) -> SessionLease {
    SessionLease {
        id: id.to_string(),
        scope: scope.to_string(),
        slug: scope.to_ascii_lowercase(),
        owner: "tester".into(),
        worktree_path: worktree,
        branch: scope.to_ascii_lowercase(),
        started_at: chrono::Utc::now(),
        hostname: "h".into(),
        role: Some("implementer".into()),
        creator_pid: None,
        creator_pid_start_time: None,
        active_pid: Some(pid),
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
    }
}

// End-to-end: a lease + its session-manifest claude_session_id join + a
// pending-approval marker on disk, read through the REAL `gather_running_work`
// (the same function `aida ps` and `aida awaiting` call) — not a hand-built
// row. Proves the on-disk wiring, not just the pure classifier.
// trace:TASK-1454 | ai:claude
#[test]
fn gather_running_work_classifies_blocked_from_a_real_marker_file() {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path();
    let worktree = project_root.join("wt");
    std::fs::create_dir_all(&worktree).unwrap();

    let lease_id = "l-block-e2e";
    let claude_session_id = "claude-sess-e2e";
    let lease = test_lease(lease_id, "TASK-9999", worktree.clone(), std::process::id());
    std::fs::create_dir_all(leases_dir(project_root)).unwrap();
    std::fs::write(
        lease_path(project_root, lease_id),
        toml::to_string_pretty(&lease).unwrap(),
    )
    .unwrap();

    // The STORY-153 join: a session manifest recording which `claude`
    // conversation this AIDA lease corresponds to.
    let manifest = session_manifest::SessionManifest {
        session_id: lease_id.to_string(),
        planned_at: chrono::Utc::now(),
        plan_source: "test".to_string(),
        claude_session_id: Some(claude_session_id.to_string()),
        batch_name: None,
        plan: None,
        items: Vec::new(),
    };
    session_manifest::save(
        &session_manifest::manifest_path(project_root, lease_id),
        &manifest,
    )
    .unwrap();

    // The marker the Notification hook would have written, keyed by the
    // CLAUDE session id (not the AIDA lease id).
    pending_approval::write_marker(
        project_root,
        claude_session_id,
        Some("Bash"),
        Some("Claude needs your permission to use Bash"),
        chrono::Utc::now() - chrono::Duration::seconds(120),
    )
    .unwrap();

    let (rows, _orphans) = gather_running_work(project_root);
    let row = rows
        .iter()
        .find(|r| r.lease.id == lease_id)
        .expect("the lease we just wrote must produce a row");
    match &row.activity {
        Some(SeatActivity::Blocked { tool, secs }) => {
            assert_eq!(tool.as_deref(), Some("Bash"));
            assert!(*secs >= 120, "secs={secs}");
        }
        other => panic!("expected Blocked, got {other:?}"),
    }

    // And the `aida awaiting` projection reuses this same classification —
    // no second implementation to drift from it.
    let blocked = collect_blocked_seat_items(project_root);
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0].spec.as_deref(), Some("TASK-9999"));
    assert_eq!(blocked[0].tool.as_deref(), Some("Bash"));
}

// A STALE marker (past PENDING_APPROVAL_STALE_SECS) must NOT classify as
// Blocked — it is dropped by `pending_approval::list_active` before
// `gather_running_work` ever sees it.
// trace:TASK-1454 | ai:claude
#[test]
fn gather_running_work_ignores_a_stale_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path();
    let worktree = project_root.join("wt");
    std::fs::create_dir_all(&worktree).unwrap();

    let lease_id = "l-stale-e2e";
    let claude_session_id = "claude-sess-stale";
    let lease = test_lease(lease_id, "TASK-8888", worktree.clone(), std::process::id());
    std::fs::create_dir_all(leases_dir(project_root)).unwrap();
    std::fs::write(
        lease_path(project_root, lease_id),
        toml::to_string_pretty(&lease).unwrap(),
    )
    .unwrap();

    let manifest = session_manifest::SessionManifest {
        session_id: lease_id.to_string(),
        planned_at: chrono::Utc::now(),
        plan_source: "test".to_string(),
        claude_session_id: Some(claude_session_id.to_string()),
        batch_name: None,
        plan: None,
        items: Vec::new(),
    };
    session_manifest::save(
        &session_manifest::manifest_path(project_root, lease_id),
        &manifest,
    )
    .unwrap();

    pending_approval::write_marker(
        project_root,
        claude_session_id,
        Some("Bash"),
        None,
        chrono::Utc::now()
            - chrono::Duration::seconds(pending_approval::PENDING_APPROVAL_STALE_SECS + 60),
    )
    .unwrap();

    let (rows, _orphans) = gather_running_work(project_root);
    let row = rows.iter().find(|r| r.lease.id == lease_id);
    // Either the row is missing the Blocked classification (falls back to
    // the heuristic — Unknown, since there's no real transcript) or it is
    // simply not Blocked. Either way, never Blocked on a stale marker.
    if let Some(row) = row {
        assert!(
            !matches!(row.activity, Some(SeatActivity::Blocked { .. })),
            "a stale marker must not classify as Blocked"
        );
    }
    assert!(collect_blocked_seat_items(project_root).is_empty());
}
