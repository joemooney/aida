use super::*;

fn lease_for(scope: &str) -> SessionLease {
    SessionLease {
        id: "019ebug574xyz".to_string(),
        scope: scope.to_string(),
        slug: scope.to_ascii_lowercase(),
        owner: "tester".to_string(),
        worktree_path: std::path::PathBuf::from("/tmp/aida-bug574"),
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

/// Re-running `aida session start <scope>` when THIS clone already holds a
/// lease for that scope is benign idempotent re-entry. The handler resolves
/// the existing lease via this pure predicate and exits 0 (reporting it)
/// rather than bailing — that decision is what these assertions pin.
// trace:BUG-574 | ai:claude
#[test]
fn existing_lease_for_scope_recognizes_reentry() {
    let leases = vec![lease_for("STORY-574"), lease_for("TASK-100")];

    // Exact match → re-entry recognized.
    let found = existing_lease_for_scope(&leases, "STORY-574");
    assert!(
        found.is_some(),
        "an existing lease for the scope must be recognized so re-entry exits 0"
    );
    assert_eq!(found.unwrap().scope, "STORY-574");

    // Case-insensitive on the raw scope (mirrors the old guard).
    assert!(
        existing_lease_for_scope(&leases, "story-574").is_some(),
        "scope match is case-insensitive"
    );
}

/// A scope with NO existing lease must NOT be treated as re-entry — the
/// handler proceeds to actually create the worktree/lease, and a genuine
// failure there stays non-zero. trace:BUG-574 | ai:claude
#[test]
fn existing_lease_for_scope_none_when_unleased() {
    let leases = vec![lease_for("TASK-100")];
    assert!(
        existing_lease_for_scope(&leases, "STORY-574").is_none(),
        "an unleased scope is a fresh start, not re-entry"
    );
    // Empty registry: also no re-entry.
    assert!(existing_lease_for_scope(&[], "ANYTHING-1").is_none());
}
