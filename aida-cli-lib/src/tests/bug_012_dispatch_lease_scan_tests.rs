//! BUG-b6b1278890774ef993d2ebc327862470-012 regression: the dispatch occupancy
//! scan reads only real session leases, so `.activity.toml` / `.manifest.toml`
//! companions no longer refuse every launch, while a corrupt lease still
//! fails closed.
// trace:BUG-b6b1278890774ef993d2ebc327862470-012 | ai:claude

use crate::dispatch::check_live_worktree_or_branch_occupant;

const VALID_LEASE: &str = "id = \"019200000012\"\n\
     scope = \"BUG-012\"\n\
     slug = \"bug-012\"\n\
     owner = \"test@example.com\"\n\
     worktree_path = \"nonexistent-bug-012\"\n\
     branch = \"claude/bug-012\"\n\
     started_at = \"2026-10-18T00:00:00Z\"\n\
     hostname = \"testhost\"\n";

fn sessions_with_companions() -> (tempfile::TempDir, std::path::PathBuf) {
    let root = tempfile::tempdir().expect("tempdir");
    let sessions = crate::leases::leases_dir(root.path());
    std::fs::create_dir_all(&sessions).expect("sessions dir");
    std::fs::write(sessions.join("019200000012.toml"), VALID_LEASE).expect("lease");
    std::fs::write(
        sessions.join("019200000012.activity.toml"),
        "last_tool_at = \"2026-10-18T00:00:00Z\"\n",
    )
    .expect("activity");
    std::fs::write(
        sessions.join("019200000012.manifest.toml"),
        "[[files]]\npath = \"x\"\n",
    )
    .expect("manifest");
    (root, sessions)
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-012 | ai:claude
#[test]
fn bug_012_occupancy_scan_skips_lease_companions() {
    let (root, _sessions) = sessions_with_companions();
    let target = root.path().join("target-worktree");
    std::fs::create_dir_all(&target).expect("target");
    let refusal =
        check_live_worktree_or_branch_occupant(root.path(), &target, Some("demo/lane"), None);
    assert_eq!(refusal, None, "companions must not read as leases");
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-012 | ai:claude
#[test]
fn bug_012_occupancy_scan_refuses_corrupt_lease() {
    let (root, sessions) = sessions_with_companions();
    std::fs::write(sessions.join("019200000099.toml"), "not = [valid").expect("corrupt");
    let target = root.path().join("target-worktree");
    std::fs::create_dir_all(&target).expect("target");
    let refusal =
        check_live_worktree_or_branch_occupant(root.path(), &target, Some("demo/lane"), None)
            .expect("corrupt lease fails closed");
    assert!(
        refusal.contains("malformed session lease") && refusal.contains("019200000099.toml"),
        "{refusal}"
    );
}
