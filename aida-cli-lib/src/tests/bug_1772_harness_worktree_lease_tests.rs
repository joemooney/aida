use super::*;

#[test]
// trace:BUG-1772.ac3dfbab | ai:antigravity
// trace:BUG-1772.acec6aa8 | ai:antigravity
fn bug_1772_harness_worktree_lease_omitted_for_project_root() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    let _ambient = crate::test_env::AmbientGuard::hermetic(root, None);
    std::fs::create_dir_all(root.join(".aida").join("sessions")).unwrap();
    super::session_harness_worktree_register(
        "test-agent-123",
        root.to_str().unwrap(),
        Some("codex"),
        Some("main"),
        None,
    )
    .unwrap();
    let leases = super::list_leases(&root);
    assert_eq!(
        leases.len(),
        0,
        "No lease should be written for project root cwd"
    );
}

#[test]
// trace:BUG-1772.ac912d14 | ai:antigravity
// trace:BUG-1772.ac53764b | ai:antigravity
// trace:BUG-1772.acec6aa8 | ai:antigravity
fn bug_1772_prune_stale_reaps_immortal_harness_leases() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    let _ambient = crate::test_env::AmbientGuard::hermetic(root, None);
    let sessions_dir = root.join(".aida").join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    let old_date = chrono::Utc::now() - chrono::Duration::days(90);
    // Since SessionLease has many fields, we can just write the TOML directly to avoid missing fields
    let toml_content = format!(
        r#"
id = "stale-harness-lease"
scope = "harness-worktree"
slug = "harness-worktree"
owner = "test"
worktree_path = '{}'
branch = "main"
started_at = "{}"
hostname = "test-host"
"#,
        root.display(),
        old_date.to_rfc3339()
    );
    std::fs::write(sessions_dir.join("stale-harness-lease.toml"), toml_content).unwrap();
    assert_eq!(super::list_leases(&root).len(), 1);
    let result = super::session_leases_prune_stale(true);
    assert!(result.is_ok());
    assert_eq!(
        super::list_leases(&root).len(),
        0,
        "Stale lease must be pruned"
    );
}
