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

// trace:BUG-b6b1278890774ef993d2ebc327862470-012 | ai:claude
#[test]
fn bug_012_lease_companion_names() {
    use crate::leases::is_lease_companion;
    assert!(is_lease_companion("019200000012.activity.toml"));
    assert!(is_lease_companion("019200000012.manifest.toml"));
    assert!(is_lease_companion("019200000012.tmp.4242.7"));
    assert!(!is_lease_companion("019200000012.toml"));
    assert!(!is_lease_companion("mcp-claim.FR-1.toml"));
    assert!(!is_lease_companion("lock-_tmp_.tmpAbc_wt.v2.toml"));
    assert!(!is_lease_companion("x.tmp.toml"));
}

/// A lease and its companions, all carrying `creator_pid = pid` and scoped to
/// a spec in `wt`, so only the file-name filter keeps companions out.
fn lease_and_companions(root: &std::path::Path, wt: &std::path::Path, pid: u32) {
    let sessions = crate::leases::leases_dir(root);
    std::fs::create_dir_all(&sessions).expect("sessions dir");
    let body = format!(
        "id = \"aaaa11112222\"\nscope = \"TASK-77\"\nslug = \"task-77\"\n\
         owner = \"test@example.com\"\nworktree_path = {:?}\nbranch = \"task-77\"\n\
         started_at = \"2026-10-18T00:00:00Z\"\nhostname = \"testhost\"\n\
         creator_pid = {pid}\n",
        wt.to_string_lossy()
    );
    for name in [
        "aaaa11112222.toml",
        "aaaa11112222.activity.toml",
        "aaaa11112222.manifest.toml",
    ] {
        std::fs::write(sessions.join(name), &body).expect("write");
    }
}

fn companions_untouched(root: &std::path::Path, key: &str) {
    let sessions = crate::leases::leases_dir(root);
    for name in ["aaaa11112222.activity.toml", "aaaa11112222.manifest.toml"] {
        let body = std::fs::read_to_string(sessions.join(name)).expect("read");
        assert!(!body.contains(key), "{name} was rewritten: {body}");
    }
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-012 | ai:claude
#[test]
fn bug_012_interrupt_marking_skips_companions() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let wt = tmp.path().join("wt");
    std::fs::create_dir_all(&wt).expect("wt");
    let pid = std::process::id();
    lease_and_companions(tmp.path(), &wt, pid);

    let marked = crate::drain_signal::mark_in_flight_leases_interrupted(tmp.path(), pid);
    assert_eq!(marked, vec!["aaaa11112222".to_string()]);
    let lease =
        std::fs::read_to_string(crate::leases::leases_dir(tmp.path()).join("aaaa11112222.toml"))
            .expect("lease");
    assert!(lease.contains("interrupted_at"), "{lease}");
    companions_untouched(tmp.path(), "interrupted_at");
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-012 | ai:claude
#[test]
fn bug_012_subagent_adoption_skips_companions() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let wt = tmp.path().join("wt");
    std::fs::create_dir_all(&wt).expect("wt");
    let pid = std::process::id();
    lease_and_companions(tmp.path(), &wt, pid);

    let adopted = crate::leases::adopt_spec_lease_for_subagent(tmp.path(), &wt, Some(pid));
    assert_eq!(adopted.as_deref(), Some("aaaa11112222"));
    let lease =
        std::fs::read_to_string(crate::leases::leases_dir(tmp.path()).join("aaaa11112222.toml"))
            .expect("lease");
    assert!(lease.contains("adopted_by_subagent_at"), "{lease}");
    companions_untouched(tmp.path(), "adopted_by_subagent_at");
}

/// Write an `aida lock` lock-only entry for `locked` into `root`'s sessions dir.
fn write_lock_only_entry(root: &std::path::Path, locked: &std::path::Path) {
    let sessions = crate::leases::leases_dir(root);
    std::fs::create_dir_all(&sessions).expect("sessions dir");
    let body = format!(
        "scope = \"aida:lock:wt\"\nworktree_path = {:?}\n\
         started_at = \"2026-10-28T00:00:00Z\"\nauthorized_by = \"advisor\"\n\
         aida_lock_only = true\n",
        locked.canonicalize().expect("canonical").to_string_lossy()
    );
    std::fs::write(sessions.join("lock-_tmp_wt.toml"), body).expect("lock entry");
}

// trace:TASK-b6b1278890774ef993d2ebc327862470-064 | ai:claude
#[test]
fn task_064_lock_only_entry_for_another_worktree_admits() {
    let (root, _sessions) = sessions_with_companions();
    let other = root.path().join("other-worktree");
    let target = root.path().join("target-worktree");
    std::fs::create_dir_all(&other).expect("other");
    std::fs::create_dir_all(&target).expect("target");
    write_lock_only_entry(root.path(), &other);
    let refusal =
        check_live_worktree_or_branch_occupant(root.path(), &target, Some("demo/lane"), None);
    assert_eq!(refusal, None, "a lock on another worktree must not refuse");
}

// trace:TASK-b6b1278890774ef993d2ebc327862470-064 | ai:claude
#[test]
fn task_064_lock_only_entry_for_the_target_refuses_as_occupied() {
    let (root, _sessions) = sessions_with_companions();
    let target = root.path().join("target-worktree");
    std::fs::create_dir_all(&target).expect("target");
    write_lock_only_entry(root.path(), &target);
    let refusal =
        check_live_worktree_or_branch_occupant(root.path(), &target, Some("demo/lane"), None)
            .expect("a lock on the target occupies it");
    assert!(
        refusal.contains("occupied by an aida lock") && refusal.contains("advisor"),
        "{refusal}"
    );
}

// trace:TASK-b6b1278890774ef993d2ebc327862470-064 | ai:claude
#[test]
fn task_064_corrupt_lock_only_entry_refuses() {
    let (root, sessions) = sessions_with_companions();
    std::fs::write(sessions.join("lock-_tmp_wt.toml"), "worktree_path = [").expect("corrupt");
    let target = root.path().join("target-worktree");
    std::fs::create_dir_all(&target).expect("target");
    let refusal =
        check_live_worktree_or_branch_occupant(root.path(), &target, Some("demo/lane"), None)
            .expect("corrupt lock entry fails closed");
    assert!(
        refusal.contains("malformed aida lock entry") && refusal.contains("lock-_tmp_wt.toml"),
        "{refusal}"
    );
}
