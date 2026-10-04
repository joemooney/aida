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
        creator_pid_start_time: None,
        active_pid: None,
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
        interrupted_at: None,
        interrupted_reason: None,
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

// trace:BUG-1794 | ai:codex
#[cfg(windows)]
#[test]
fn windows_path_key_unifies_lease_and_canonical_cwd_spellings() {
    assert_eq!(
        windows_path_key(r"C:\Users\Tester\worker"),
        windows_path_key(r"\\?\c:\users\tester\worker\"),
    );
    assert_eq!(
        windows_path_key(r"\\server\share\worker"),
        windows_path_key(r"\\?\UNC\SERVER\SHARE\worker\"),
    );
}

/// BUG-1794 rework: Windows path matching is case-insensitive for Unicode
/// names too, so the key fold must be Unicode-aware — a lease recorded with
/// an `Åsa` component must produce the same key as a cwd spelled `åsa`.
// trace:BUG-1794 | ai:claude
#[cfg(windows)]
#[test]
fn windows_path_key_folds_unicode_casing() {
    assert_eq!(
        windows_path_key(r"C:\Users\Åsa\worker"),
        windows_path_key(r"\\?\c:\users\åsa\worker\"),
    );
    assert_eq!(
        windows_path_key(r"\\server\share\ÅSA"),
        windows_path_key(r"\\?\UNC\server\share\åsa\"),
    );
}

/// BUG-1794 rework: a lease recorded with `Åsa` covers a canonical cwd
/// spelled `åsa` (and its descendants), while the segment-boundary check
/// still rejects the sibling-prefix worktree `åsa-2`.
// trace:BUG-1794 | ai:claude
#[cfg(windows)]
#[test]
fn unicode_case_variant_lease_covers_cwd_without_matching_sibling_prefix() {
    let lease = lease_with_worktree(PathBuf::from(r"C:\Users\Tester\Åsa"));
    assert!(lease_covers_cwd(
        &lease,
        Path::new(r"\\?\C:\Users\Tester\åsa"),
    ));
    assert!(lease_covers_cwd(
        &lease,
        Path::new(r"\\?\C:\Users\Tester\åsa\src"),
    ));
    assert!(!lease_covers_cwd(
        &lease,
        Path::new(r"\\?\C:\Users\Tester\åsa-2"),
    ));
}

#[cfg(windows)]
#[test]
fn extended_windows_cwd_matches_descendant_without_matching_sibling_prefix() {
    let lease = lease_with_worktree(PathBuf::from(r"C:\Users\Tester\worker"));
    assert!(lease_covers_cwd(
        &lease,
        Path::new(r"\\?\C:\Users\Tester\worker\src"),
    ));
    assert!(!lease_covers_cwd(
        &lease,
        Path::new(r"\\?\C:\Users\Tester\worker-copy"),
    ));
}
