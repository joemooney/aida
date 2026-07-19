use super::*;

fn row(spec_id: &str, status: &str, in_progress: bool) -> QueueRow {
    QueueRow {
        spec_id: spec_id.into(),
        title: format!("Title for {}", spec_id),
        status: status.into(),
        for_role: Some("implementer".into()),
        in_progress,
        lease_id: None,
        lease_started_at: None,
    }
}

// TASK-490 acceptance: a queue with 2 in-progress + N approved items
// surfaces both in-progress at the top, and the "Next up" counter
// reflects only the not-in-progress slice. trace:TASK-490 | ai:claude
#[test]
fn split_surfaces_in_progress_first_and_excludes_them_from_next_up_counter() {
    let head = vec![
        row("STORY-248", "Planned", false),
        row("STORY-244", "Approved", false),
        row("TASK-487", "In Progress", true),
        row("TASK-311", "Approved", false),
        row("PR-210", "In Progress", true),
    ];
    // Queue total simulates a backlog of 16: 2 in-progress + 14 not.
    let split = split_queue_view(&head, 16);
    assert_eq!(split.in_progress_total, 2, "two in-progress items");
    assert_eq!(
        split.next_up_total, 14,
        "next_up counter excludes the in-progress count"
    );
    let in_progress_ids: Vec<_> = split
        .in_progress_rows
        .iter()
        .map(|r| r.spec_id.as_str())
        .collect();
    assert_eq!(in_progress_ids, vec!["TASK-487", "PR-210"]);
    let next_up_ids: Vec<_> = split
        .next_up_rows
        .iter()
        .map(|r| r.spec_id.as_str())
        .collect();
    assert_eq!(next_up_ids, vec!["STORY-248", "STORY-244", "TASK-311"]);
}

// When no In Progress items exist, the in_progress section is empty
// (renderer omits the subsection header), and "Next up" math is
// identical to the pre-TASK-490 single-list behavior.
#[test]
fn split_with_no_in_progress_leaves_next_up_total_unchanged() {
    let head = vec![
        row("STORY-248", "Planned", false),
        row("STORY-244", "Approved", false),
        row("SPIKE-8", "Approved", false),
    ];
    let split = split_queue_view(&head, 12);
    assert_eq!(split.in_progress_total, 0);
    assert!(split.in_progress_rows.is_empty());
    assert_eq!(split.next_up_total, 12);
    assert_eq!(split.next_up_rows.len(), 3);
}

// collect_queue_snapshot pulls ALL in-progress items into the head Vec
// even when they sit past the next-up display budget (5), and attaches
// lease info when a lease's scope matches the spec id. Without this
// surfacing the in-progress item at queue position 8 was the "buried in
// '… N more'" friction TASK-490 documents.
#[test]
fn collect_queue_snapshot_pulls_in_progress_past_display_budget_with_lease() {
    use aida_core::models::{Requirement, RequirementsStore};
    use aida_core::{CachedGitBackend, DatabaseBackend, RequirementStatus};
    use chrono::TimeZone;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let project_root = dir.path();
    let store_root = project_root.join("store");
    let cache_path = project_root.join(".aida").join("cache.db");
    std::fs::create_dir_all(&store_root).unwrap();
    let backend = CachedGitBackend::open(&store_root, &cache_path).unwrap();

    let user_id = current_user_id(None);
    let mut store = RequirementsStore::new();
    let now = chrono::Utc::now();

    for i in 1..=7 {
        let mut r = Requirement::new(format!("approved {}", i), String::new());
        r.spec_id = Some(format!("TASK-{}", 9000 + i));
        r.status = RequirementStatus::Approved;
        let entry = aida_core::models::QueueEntry {
            user_id: user_id.clone(),
            requirement_id: r.id,
            position: i as i64,
            added_by: user_id.clone(),
            note: None,
            added_at: now,
            for_role: Some("implementer".into()),
            for_scope: None,
            for_session: None,
            added_by_machine: None,
        };
        backend.queue_add(entry).unwrap();
        store.requirements.push(r);
    }

    let mut buried = Requirement::new("buried in-progress".into(), String::new());
    buried.spec_id = Some("TASK-9999".into());
    buried.status = RequirementStatus::InProgress;
    let buried_entry = aida_core::models::QueueEntry {
        user_id: user_id.clone(),
        requirement_id: buried.id,
        position: 8,
        added_by: user_id.clone(),
        note: None,
        added_at: now,
        for_role: Some("implementer".into()),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    };
    backend.queue_add(buried_entry).unwrap();
    store.requirements.push(buried);

    // Drop a matching lease file so the in-progress row gets a chip.
    let lease_dir = leases_dir(project_root);
    std::fs::create_dir_all(&lease_dir).unwrap();
    let lease_id = "01900000task";
    let started = chrono::Utc.with_ymd_and_hms(2026, 5, 23, 22, 0, 0).unwrap();
    let lease_toml = format!(
        "id = \"{lease_id}\"\n\
             scope = \"TASK-9999\"\n\
             slug = \"task-9999\"\n\
             owner = \"tester\"\n\
             worktree_path = \"/tmp/x\"\n\
             branch = \"task-9999\"\n\
             started_at = \"{}\"\n\
             hostname = \"h\"\n",
        started.to_rfc3339()
    );
    std::fs::write(lease_dir.join(format!("{lease_id}.toml")), lease_toml).unwrap();
    let leases = list_leases(project_root);

    let (head, total) = collect_queue_snapshot(&backend, &store, Some("implementer"), &leases);

    assert_eq!(total, 8, "all eight non-terminal entries counted");
    let head_ids: Vec<_> = head.iter().map(|r| r.spec_id.as_str()).collect();
    assert_eq!(
        head_ids[0], "TASK-9999",
        "in-progress item surfaces first, not buried at position 8 (got {:?})",
        head_ids
    );
    let promoted = &head[0];
    assert!(promoted.in_progress);
    assert_eq!(promoted.lease_id.as_deref(), Some(lease_id));
    assert_eq!(promoted.lease_started_at, Some(started));
    // The rest are the approved head in FIFO order, up to the 5-slot budget.
    assert_eq!(head.len(), 6, "1 in-progress + 5 next-up = 6 head rows");
    for (i, row) in head[1..].iter().enumerate() {
        assert_eq!(row.spec_id, format!("TASK-{}", 9001 + i));
        assert!(!row.in_progress);
        assert!(row.lease_id.is_none());
    }
}
