use super::*;
use uuid::Uuid;

fn summary(
    id: Uuid,
    spec_id: Option<&str>,
    agreed_id: Option<&str>,
) -> aida_core::RequirementSummary {
    aida_core::RequirementSummary {
        id,
        spec_id: spec_id.map(|s| s.to_string()),
        agreed_id: agreed_id.map(|s| s.to_string()),
        title: format!("title for {id}"),
        description: String::new(),
        status: "approved".to_string(),
        priority: "medium".to_string(),
        owner: String::new(),
        assignee: None,
        feature: String::new(),
        req_type: "bug".to_string(),
        tags: Vec::new(),
        created_at: String::new(),
        modified_at: String::new(),
        archived: false,
        archived_at: None,
        deferred: false,
        deferred_at: None,
        deferred_until: None,
        in_degree: 0,
        out_degree: 0,
        heft: 0,
        // trace:TASK-902 | ai:claude
        blocked: false,
        // trace:TASK-1065 | ai:claude
        has_pending_decision: false,
        execution_mode: None,
        weight: None,
        origin: None,
        yaml_path: String::new(),
    }
}

fn entry(requirement_id: Uuid, for_role: Option<&str>) -> aida_core::QueueEntry {
    aida_core::QueueEntry {
        user_id: "u".to_string(),
        requirement_id,
        position: 0,
        added_by: "u".to_string(),
        note: None,
        added_at: chrono::Utc::now(),
        for_role: for_role.map(|s| s.to_string()),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    }
}

#[test]
fn emits_exact_keys_and_queue_order() {
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let summaries = vec![
        summary(b, Some("BUG-7-002"), None),
        summary(a, Some("BUG-7-001"), Some("BUG-2")),
    ];
    // Queue order is a, then b — the output must follow it, not the
    // summary order.
    let entries = vec![entry(a, Some("implementer")), entry(b, None)];

    let rows = queue_json_rows(&entries, &summaries);
    assert_eq!(rows.len(), 2);

    // Row 0 (entry a): agreed_id wins over spec_id; for_role carried.
    let r0 = &rows[0];
    // Exactly the four keys the TUI cockpit parser expects, no more.
    let mut keys: Vec<&str> = r0.as_object().unwrap().keys().map(|k| k.as_str()).collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["for_role", "spec_id", "status", "title"]);
    assert_eq!(r0["spec_id"], "BUG-2");
    assert_eq!(r0["title"], format!("title for {a}"));
    assert_eq!(r0["status"], "approved");
    assert_eq!(r0["for_role"], "implementer");

    // Row 1 (entry b): no agreed_id -> spec_id; for_role null.
    let r1 = &rows[1];
    assert_eq!(r1["spec_id"], "BUG-7-002");
    assert!(r1["for_role"].is_null());
}

#[test]
fn drops_entries_with_no_matching_summary() {
    let known = Uuid::new_v4();
    let orphan = Uuid::new_v4();
    let summaries = vec![summary(known, Some("BUG-9-001"), None)];
    let entries = vec![entry(orphan, None), entry(known, None)];

    let rows = queue_json_rows(&entries, &summaries);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["spec_id"], "BUG-9-001");
}

#[test]
fn missing_both_ids_falls_back_to_question_mark() {
    let id = Uuid::new_v4();
    let summaries = vec![summary(id, None, None)];
    let rows = queue_json_rows(&[entry(id, None)], &summaries);
    assert_eq!(rows[0]["spec_id"], "?");
}

#[test]
fn queue_list_json_requested_honors_global_format_pin() {
    // BUG-1149: `--format json` / AIDA_OUTPUT_FORMAT=json must select the same
    // JSON projection as the command-local `--json`, not fall through to TOON.
    // trace:BUG-1149 | ai:codex
    assert!(queue_list_json_requested(false, Some(OutputFormat::Json)));
    assert!(queue_list_json_requested(true, Some(OutputFormat::Toon)));
    assert!(queue_list_json_requested(true, Some(OutputFormat::Human)));
    assert!(!queue_list_json_requested(false, Some(OutputFormat::Toon)));
    assert!(!queue_list_json_requested(false, Some(OutputFormat::Human)));
    assert!(!queue_list_json_requested(false, None));
}

// TASK-1052: a summary with a chosen status + archived flag, so the GC
// predicate can be exercised over the full live/dead spread.
fn summary_st(id: Uuid, status: &str, archived: bool) -> aida_core::RequirementSummary {
    let mut s = summary(id, Some("X-1"), None);
    s.status = status.to_string();
    s.archived = archived;
    s
}

// TASK-1052: queue-GC flags exactly the dead corpses — a spec that is
// Completed, Rejected, or archived (any status). Still-actionable specs
// (Approved / InProgress / Done) survive, and an entry whose spec is
// missing from the cache is LEFT alone (that's `prune --orphaned`'s job).
// trace:TASK-1052 | ai:claude
#[test]
fn dead_queue_entries_flags_terminal_and_archived_only() {
    let completed = Uuid::new_v4();
    let rejected = Uuid::new_v4();
    let archived_draft = Uuid::new_v4();
    let approved = Uuid::new_v4();
    let in_progress = Uuid::new_v4();
    let done = Uuid::new_v4(); // Done is NOT terminal — work on a branch.
    let orphan = Uuid::new_v4(); // no summary at all

    let summaries = vec![
        summary_st(completed, "completed", false),
        summary_st(rejected, "rejected", false),
        summary_st(archived_draft, "draft", true),
        summary_st(approved, "approved", false),
        summary_st(in_progress, "in-progress", false),
        summary_st(done, "done", false),
    ];
    let entries = vec![
        entry(completed, Some("implementer")),
        entry(rejected, None),
        entry(archived_draft, None),
        entry(approved, Some("implementer")),
        entry(in_progress, None),
        entry(done, None),
        entry(orphan, None),
    ];

    let dead = dead_queue_entries(&entries, &summaries, None);
    let dead_ids: std::collections::HashSet<Uuid> = dead.iter().map(|e| e.requirement_id).collect();

    // Exactly the three corpses, by count and by membership.
    assert_eq!(dead.len(), 3, "only completed/rejected/archived are dead");
    assert!(dead_ids.contains(&completed));
    assert!(dead_ids.contains(&rejected));
    assert!(dead_ids.contains(&archived_draft));
    // Active work — and the Done-awaiting-merge entry — all survive.
    assert!(!dead_ids.contains(&approved));
    assert!(!dead_ids.contains(&in_progress));
    assert!(!dead_ids.contains(&done));
    // The orphan (no backing summary) is NOT GC's to remove.
    assert!(!dead_ids.contains(&orphan));
}

// TASK-1052: the `--for <role>` filter narrows the sweep to one routed
// role — a dead spec queued for a different role is not touched by that
// run. trace:TASK-1052 | ai:claude
#[test]
fn dead_queue_entries_respects_role_filter() {
    let dead_impl = Uuid::new_v4();
    let dead_review = Uuid::new_v4();
    let summaries = vec![
        summary_st(dead_impl, "completed", false),
        summary_st(dead_review, "completed", false),
    ];
    let entries = vec![
        entry(dead_impl, Some("implementer")),
        entry(dead_review, Some("reviewer")),
    ];

    let dead = dead_queue_entries(&entries, &summaries, Some("reviewer"));
    assert_eq!(dead.len(), 1, "only the reviewer-routed corpse matches");
    assert_eq!(dead[0].requirement_id, dead_review);
}

// BUG-1512: a summary shaped like an auto-queued review row, still Draft (no
// reviewer ever closed it) — the class the target-spec rule alone can never
// collect, because Draft is never terminal.
fn review_summary(id: Uuid, pr: u64) -> aida_core::RequirementSummary {
    let mut s = summary_st(id, "draft", false);
    s.title = format!("Review PR-{pr}: some shipped change");
    s
}

// BUG-1512: the pure title-parsing half of the merged-PR sweep — extracts a
// PR number from every "Review PR-N: ..." summary not already claimed by the
// target-spec rule, and ignores everything else (non-review titles, and
// review rows the target-spec rule already flagged dead).
// trace:BUG-1512 | ai:claude
#[test]
fn review_summaries_pending_merge_check_parses_titles_and_skips_already_dead() {
    let open_review = Uuid::new_v4();
    let already_dead_review = Uuid::new_v4();
    let non_review = Uuid::new_v4();

    let summaries = vec![
        review_summary(open_review, 1986),
        review_summary(already_dead_review, 42),
        summary_st(non_review, "approved", false),
    ];
    let mut already_dead = std::collections::HashSet::new();
    already_dead.insert(already_dead_review);

    let candidates = review_summaries_pending_merge_check(&summaries, &already_dead);
    assert_eq!(
        candidates.len(),
        1,
        "only the still-open review row qualifies"
    );
    assert_eq!(candidates[0].0.id, open_review);
    assert_eq!(candidates[0].1, 1986);
}

// BUG-1512 AC1/AC5: a routed review entry becomes collectable once its PR is
// confirmed merged, regardless of the review STORY's own status — here the
// story is Draft (the pre-fix `dead_queue_entries` rule alone would never
// flag it, since Draft is never terminal). AC2: additive to the target-spec
// rule, not a replacement — a genuinely-dead entry from that rule is not
// double-counted here even if it also happens to parse as a merged PR.
// trace:BUG-1512 | ai:claude
#[test]
fn merged_pr_review_entries_collects_merged_pr_rows_regardless_of_story_status() {
    let merged_draft_review = Uuid::new_v4(); // Draft + PR merged -> collectable
    let open_pr_review = Uuid::new_v4(); // Draft + PR NOT merged -> survives
    let already_dead_but_merged = Uuid::new_v4(); // caught by the other rule already

    let entries = vec![
        entry(merged_draft_review, Some("reviewer")),
        entry(open_pr_review, Some("reviewer")),
        entry(already_dead_but_merged, Some("reviewer")),
    ];

    let mut merged_pr_ids = std::collections::HashSet::new();
    merged_pr_ids.insert(merged_draft_review);
    merged_pr_ids.insert(already_dead_but_merged);

    let mut already_dead = std::collections::HashSet::new();
    already_dead.insert(already_dead_but_merged);

    let collectable = merged_pr_review_entries(&entries, &merged_pr_ids, &already_dead, None);
    assert_eq!(collectable.len(), 1, "only the not-already-dead merged row");
    assert_eq!(collectable[0].requirement_id, merged_draft_review);
    assert!(
        !collectable
            .iter()
            .any(|e| e.requirement_id == open_pr_review),
        "an entry whose PR has not merged must survive"
    );
    assert!(
        !collectable
            .iter()
            .any(|e| e.requirement_id == already_dead_but_merged),
        "already-dead entries are not double-counted by the merged-PR rule"
    );
}

// BUG-1512: the role filter narrows the merged-PR sweep exactly like it
// narrows `dead_queue_entries`.
#[test]
fn merged_pr_review_entries_respects_role_filter() {
    let reviewer_row = Uuid::new_v4();
    let implementer_row = Uuid::new_v4();
    let entries = vec![
        entry(reviewer_row, Some("reviewer")),
        entry(implementer_row, Some("implementer")),
    ];
    let mut merged_pr_ids = std::collections::HashSet::new();
    merged_pr_ids.insert(reviewer_row);
    merged_pr_ids.insert(implementer_row);
    let already_dead = std::collections::HashSet::new();

    let collectable =
        merged_pr_review_entries(&entries, &merged_pr_ids, &already_dead, Some("reviewer"));
    assert_eq!(collectable.len(), 1);
    assert_eq!(collectable[0].requirement_id, reviewer_row);
}
