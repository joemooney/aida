use super::is_work_spec_branch_name;
use super::work_spec_id_from_branch;

#[test]
fn matches_canonical_work_branches() {
    assert!(is_work_spec_branch_name("task-281"));
    assert!(is_work_spec_branch_name("TASK-281"));
    assert!(is_work_spec_branch_name("task-1-127"));
    assert!(is_work_spec_branch_name("task-node-127"));
    assert!(is_work_spec_branch_name("story-86"));
    assert!(is_work_spec_branch_name("bug-100"));
    assert!(is_work_spec_branch_name("epic-20-batch7"));
    assert!(is_work_spec_branch_name("spec-409"));
    assert!(is_work_spec_branch_name("spike-7"));
}

#[test]
fn rejects_pr_and_mr_branches() {
    // pr-N / mr-N are reviewer worktrees — surfaced elsewhere.
    assert!(!is_work_spec_branch_name("pr-271"));
    assert!(!is_work_spec_branch_name("mr-9"));
}

#[test]
fn rejects_non_spec_branches() {
    assert!(!is_work_spec_branch_name("main"));
    assert!(!is_work_spec_branch_name("feature/login"));
    assert!(!is_work_spec_branch_name("claude/plan-archive-filtering"));
    assert!(!is_work_spec_branch_name("aida-store"));
    assert!(!is_work_spec_branch_name("task"));
    assert!(!is_work_spec_branch_name("task-"));
    assert!(!is_work_spec_branch_name("taskX"));
}

#[test]
fn derives_node_qualified_spec_ids_without_collapsing_segments() {
    assert_eq!(
        work_spec_id_from_branch("task-281-foo"),
        Some("TASK-281".to_string())
    );
    assert_eq!(
        work_spec_id_from_branch("task-1-127"),
        Some("TASK-1-127".to_string())
    );
    assert_eq!(
        work_spec_id_from_branch("task-1-127-fix"),
        Some("TASK-1-127".to_string())
    );
    assert_eq!(
        work_spec_id_from_branch("task-node-a-127-fix"),
        Some("TASK-NODE-A-127".to_string())
    );
}
