use super::*;

fn lease(scope: &str, worktree: &std::path::Path) -> SessionLease {
    SessionLease {
        id: "019elease456".to_string(),
        scope: scope.to_string(),
        slug: scope.to_ascii_lowercase(),
        owner: "tester".to_string(),
        worktree_path: worktree.to_path_buf(),
        branch: scope.to_ascii_lowercase(),
        started_at: chrono::Utc::now(),
        hostname: "host".to_string(),
        role: Some("implementer".to_string()),
        creator_pid: None,
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

fn pr_item(number: u64, branch: &str) -> status_cleanup::OpenPrItem {
    status_cleanup::OpenPrItem {
        number,
        title: format!("PR {number}"),
        head_branch: branch.to_string(),
        ci_rollup: Some("pass".to_string()),
        mergeable: Some("MERGEABLE".to_string()),
        review_decision: None,
    }
}

#[test]
fn infer_tied_spec_prefers_lease_scope() {
    assert_eq!(
        infer_tied_spec(Some("STORY-456"), "story-456-foo"),
        Some("STORY-456".to_string())
    );
}

#[test]
fn infer_tied_spec_falls_back_to_branch_name() {
    assert_eq!(
        infer_tied_spec(None, "task-425-2"),
        Some("TASK-425".to_string())
    );
    assert_eq!(
        infer_tied_spec(None, "story-456-status"),
        Some("STORY-456".to_string())
    );
    // No numeric segment → no inference.
    assert_eq!(infer_tied_spec(None, "feature-branch"), None);
    assert_eq!(infer_tied_spec(None, "(detached)"), None);
}

// A worktree with no lease, clean tree, nothing live, nothing ahead, and
// no open PR is the conservative OBSOLETE case. trace:STORY-456
#[test]
fn obsolete_when_all_signals_quiet() {
    let main = std::path::PathBuf::from("/repo");
    let wt = WorktreeRecord {
        path: std::path::PathBuf::from("/repo-wt"),
        branch: Some("task-999".to_string()),
    };
    let ahead = std::collections::HashMap::new(); // no ahead entry → None
    let rows = assemble_worktree_status_rows(
        std::slice::from_ref(&wt),
        &main,
        &[],
        &[],
        &std::collections::HashMap::new(),
        &ahead,
    );
    assert_eq!(rows.len(), 1);
    assert!(rows[0].obsolete, "quiet worktree should be obsolete");
    assert_eq!(rows[0].tied_spec, Some("TASK-999".to_string()));
}

// ANY of: lease / dirty / live / ahead / open-PR keeps it TIED (in flight).
#[test]
fn tied_when_commits_ahead() {
    let main = std::path::PathBuf::from("/repo");
    let wt = WorktreeRecord {
        path: std::path::PathBuf::from("/repo-wt"),
        branch: Some("task-1".to_string()),
    };
    let mut ahead = std::collections::HashMap::new();
    ahead.insert(wt.path.clone(), 3u32);
    let rows = assemble_worktree_status_rows(
        std::slice::from_ref(&wt),
        &main,
        &[],
        &[],
        &std::collections::HashMap::new(),
        &ahead,
    );
    assert!(!rows[0].obsolete, "un-merged commits ahead → in flight");
    assert_eq!(rows[0].ahead, Some(3));
}

#[test]
fn tied_when_open_pr_present() {
    let main = std::path::PathBuf::from("/repo");
    let wt = WorktreeRecord {
        path: std::path::PathBuf::from("/repo-wt"),
        branch: Some("task-2".to_string()),
    };
    let mut prs = std::collections::HashMap::new();
    prs.insert("task-2".to_string(), pr_item(77, "task-2"));
    let rows = assemble_worktree_status_rows(
        std::slice::from_ref(&wt),
        &main,
        &[],
        &[],
        &prs,
        &std::collections::HashMap::new(),
    );
    assert!(!rows[0].obsolete, "open PR → in flight");
    assert_eq!(rows[0].pr_number, Some(77));
    assert_eq!(rows[0].pr_ci.as_deref(), Some("pass"));
}

#[test]
fn tied_when_lease_covers_it() {
    let main = std::path::PathBuf::from("/repo");
    let wt_path = std::path::PathBuf::from("/repo-wt");
    let wt = WorktreeRecord {
        path: wt_path.clone(),
        branch: Some("story-456".to_string()),
    };
    let leases = vec![lease("STORY-456", &wt_path)];
    let rows = assemble_worktree_status_rows(
        std::slice::from_ref(&wt),
        &main,
        &leases,
        &[],
        &std::collections::HashMap::new(),
        &std::collections::HashMap::new(),
    );
    assert!(!rows[0].obsolete, "active lease → in flight");
    assert_eq!(rows[0].lease_scope.as_deref(), Some("STORY-456"));
}

// The main worktree and the orphan store are excluded from the rows.
#[test]
fn skips_main_and_store_worktrees() {
    let main = std::path::PathBuf::from("/repo");
    let worktrees = vec![
        WorktreeRecord {
            path: main.clone(),
            branch: Some("main".to_string()),
        },
        WorktreeRecord {
            path: std::path::PathBuf::from("/repo-store"),
            branch: Some("aida-store".to_string()),
        },
        WorktreeRecord {
            path: std::path::PathBuf::from("/repo-wt"),
            branch: Some("task-3".to_string()),
        },
    ];
    let rows = assemble_worktree_status_rows(
        &worktrees,
        &main,
        &[],
        &[],
        &std::collections::HashMap::new(),
        &std::collections::HashMap::new(),
    );
    assert_eq!(rows.len(), 1, "only the real session worktree remains");
    assert_eq!(rows[0].branch, "task-3");
}

#[test]
fn open_pr_next_step_distinguishes_ci_from_merge_from_review() {
    assert_eq!(
        open_pr_next_step("fail", "MERGEABLE", None),
        "CI failing — fix & push"
    );
    assert_eq!(
        open_pr_next_step("pass", "CONFLICTING", None),
        "merge conflict — rebase onto default"
    );
    assert_eq!(
        open_pr_next_step("pending", "MERGEABLE", None),
        "CI in progress — wait"
    );
    assert_eq!(
        open_pr_next_step("pass", "MERGEABLE", Some("CHANGES_REQUESTED")),
        "changes requested — address review"
    );
    assert_eq!(
        open_pr_next_step("pass", "MERGEABLE", Some("APPROVED")),
        "approved — ready to merge"
    );
    assert_eq!(
        open_pr_next_step("pass", "MERGEABLE", None),
        "checks green — ready to merge"
    );
}

#[test]
fn parse_recently_merged_prs_reads_gh_json() {
    let json = r#"[
            {"number": 570, "title": "feat: thing", "mergedAt": "2026-06-01T10:00:00Z"},
            {"number": 569, "title": "fix: other", "mergedAt": null}
        ]"#;
    let rows = parse_recently_merged_prs(json);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, 570);
    assert_eq!(rows[0].1, "feat: thing");
    assert!(rows[0].2.is_some(), "mergedAt humanized");
    assert_eq!(rows[1].0, 569);
    assert!(rows[1].2.is_none(), "null mergedAt → None");
}

#[test]
fn parse_recently_merged_prs_handles_garbage() {
    assert!(parse_recently_merged_prs("not json").is_empty());
    assert!(parse_recently_merged_prs("{}").is_empty());
}

#[test]
fn truncate_for_width_cuts_and_keeps_short() {
    assert_eq!(truncate_for_width("short", 10), "short");
    let cut = truncate_for_width("aaaaaaaaaa", 5);
    assert_eq!(cut.chars().count(), 5);
    assert!(cut.ends_with('…'));
}
