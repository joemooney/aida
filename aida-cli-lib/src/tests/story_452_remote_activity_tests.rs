use super::*;

// STORY-452: for-each-ref output → RemoteCommit rows; origin/ prefix
// stripped, origin/HEAD skipped, missing/blank subjects dropped.
#[test]
fn parse_remote_branch_commits_strips_prefix_and_skips_head() {
    let stdout = "origin/HEAD\t2026-06-01T10:00:00+00:00\tpointer\n\
                      origin/bug-250\t2026-06-01T09:00:00+00:00\t[AI:codex] fix: x (BUG-250)\n\
                      origin/story-431\t2026-06-01T08:00:00+00:00\t[AI:antigravity] feat: y\n";
    let commits = parse_remote_branch_commits(stdout);
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].branch, "bug-250");
    assert!(commits[0].subject.contains("BUG-250"));
    assert!(commits[0].when.is_some());
    assert_eq!(commits[1].branch, "story-431");
}

// STORY-452: a malformed date leaves `when` None but still yields a row.
#[test]
fn parse_remote_branch_commits_tolerates_bad_dates() {
    let stdout = "origin/feat-x\tnot-a-date\t[AI:codex] feat: x (TASK-1)\n";
    let commits = parse_remote_branch_commits(stdout);
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].branch, "feat-x");
    assert!(commits[0].when.is_none());
}

// STORY-452: end-to-end over parsed git output — agent-attributed
// lease-less branches become remote-activity rows.
#[test]
fn parsed_commits_feed_inference() {
    let stdout = "origin/bug-250\t2026-06-01T09:00:00+00:00\t[AI:codex] fix: x (BUG-250)\n\
                      origin/main\t2026-06-01T08:00:00+00:00\tchore: human merge\n";
    let commits = parse_remote_branch_commits(stdout);
    let rows = remote_activity::infer_remote_activity(&commits, &[], 10);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].agent_type, "codex");
    assert_eq!(rows[0].branch, "bug-250");
}
