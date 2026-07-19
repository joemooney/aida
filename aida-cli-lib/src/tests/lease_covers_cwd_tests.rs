use super::*;
use std::path::{Path, PathBuf};

fn lease_with_worktree(path: PathBuf) -> SessionLease {
    SessionLease {
        id: "abc123".into(),
        scope: "TASK-474".into(),
        slug: "task-474".into(),
        owner: "tester".into(),
        worktree_path: path,
        branch: "task-474".into(),
        started_at: chrono::Utc::now(),
        hostname: "imac".into(),
        role: Some("implementer".into()),
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

#[test]
fn matches_when_cwd_equals_worktree() {
    let lease = lease_with_worktree(PathBuf::from("/home/joe/ai/aida-task-474"));
    assert!(lease_covers_cwd(
        &lease,
        Path::new("/home/joe/ai/aida-task-474")
    ));
}

#[test]
fn matches_when_cwd_is_descendant_of_worktree() {
    let lease = lease_with_worktree(PathBuf::from("/home/joe/ai/aida-task-474"));
    assert!(lease_covers_cwd(
        &lease,
        Path::new("/home/joe/ai/aida-task-474/aida-cli/src"),
    ));
}

#[test]
fn rejects_unrelated_cwd() {
    let lease = lease_with_worktree(PathBuf::from("/home/joe/ai/aida-task-474"));
    assert!(!lease_covers_cwd(&lease, Path::new("/tmp")));
}

/// `Path::starts_with` respects path components, so a sibling worktree
/// whose name happens to start with the lease's worktree name does NOT
/// match — protects against `/home/joe/ai/aida` being treated as
/// covering `/home/joe/ai/aida-task-474`.
#[test]
fn sibling_with_shared_prefix_does_not_match() {
    let lease = lease_with_worktree(PathBuf::from("/home/joe/ai/aida"));
    assert!(!lease_covers_cwd(
        &lease,
        Path::new("/home/joe/ai/aida-task-474"),
    ));
}

/// TASK-474: a lease with an empty `worktree_path` (the MCP `claim_task`
/// shape when the agent did not pass its cwd) must NOT match — otherwise
/// `Path::starts_with(empty)` returns true for every cwd and misroutes
/// "this session owns scope X" hints to unrelated shells.
// trace:TASK-474 | ai:claude
#[test]
fn empty_worktree_lease_matches_no_cwd() {
    let lease = lease_with_worktree(PathBuf::new());
    assert!(!lease_covers_cwd(&lease, Path::new("/home/joe/ai/aida")));
    assert!(!lease_covers_cwd(&lease, Path::new("/tmp")));
    assert!(!lease_covers_cwd(&lease, Path::new("/")));
}
