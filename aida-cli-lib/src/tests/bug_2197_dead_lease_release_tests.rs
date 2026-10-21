//! BUG-2197 — a dead-pid lease held on the main checkout is releasable, and a
//! lease that records no pid is never attributed to the inspecting process.
//!
//! Death is proven only from the lease's own recorded pids and their kernel
//! start identity (BUG-2130): a reused pid is dead, an unreadable identity is
//! not. The release removes the lease file and nothing else; the checkout it
//! names stays byte-identical.
//
// trace:BUG-2197 | ai:claude

use super::*;
use aida_core::liveness::{process_provably_dead_with, recorded_lease_owner, RecordedLeaseOwner};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

const RECORDED: &str = "linux-starttime:4242";

fn no_probe(_: u32) -> Option<String> {
    panic!("an exact record must not probe the legacy identity")
}

#[test]
fn absent_pid_is_provably_dead() {
    assert!(process_provably_dead_with(
        42,
        Some(RECORDED),
        |_| false,
        no_probe,
        |_| panic!("a dead pid needs no identity probe"),
    ));
    // Even without a recorded identity: the pid is not in the process table.
    assert!(process_provably_dead_with(
        42,
        None,
        |_| false,
        no_probe,
        |_| None
    ));
}

#[test]
fn reused_pid_is_provably_dead() {
    assert!(process_provably_dead_with(
        42,
        Some(RECORDED),
        |_| true,
        no_probe,
        |_| Some("linux-starttime:9999".to_string()),
    ));
}

#[test]
fn same_or_unknown_identity_is_not_dead() {
    // Same process.
    assert!(!process_provably_dead_with(
        42,
        Some(RECORDED),
        |_| true,
        no_probe,
        |_| Some(RECORDED.to_string()),
    ));
    // Live identity unreadable: fail closed.
    assert!(!process_provably_dead_with(
        42,
        Some(RECORDED),
        |_| true,
        no_probe,
        |_| None
    ));
    // Live identity in another scheme: not comparable, fail closed.
    assert!(!process_provably_dead_with(
        42,
        Some(RECORDED),
        |_| true,
        no_probe,
        |_| Some("sysinfo-start:2026-01-01T00:00:00+00:00".to_string()),
    ));
    // Unrecognised record: fail closed.
    assert!(!process_provably_dead_with(
        42,
        Some("garbage"),
        |_| true,
        |_| None,
        |_| None
    ));
    // Live pid with no recorded identity: cannot prove it is someone else.
    assert!(!process_provably_dead_with(
        42,
        None,
        |_| true,
        no_probe,
        |_| None
    ));
    // pid 0 is never a recorded owner and proves nothing.
    assert!(!process_provably_dead_with(
        0,
        None,
        |_| false,
        no_probe,
        |_| None
    ));
}

#[test]
fn recorded_owner_requires_every_recorded_pid_dead() {
    let dead = |pid: u32, _: Option<&str>| pid != 7;
    assert_eq!(
        recorded_lease_owner(None, None, dead),
        RecordedLeaseOwner::Unrecorded
    );
    assert_eq!(
        recorded_lease_owner(Some((1, None)), Some((2, None)), dead),
        RecordedLeaseOwner::Dead
    );
    assert_eq!(
        recorded_lease_owner(None, Some((2, None)), dead),
        RecordedLeaseOwner::Dead
    );
    assert_eq!(
        recorded_lease_owner(Some((1, None)), Some((7, None)), dead),
        RecordedLeaseOwner::NotProvablyDead
    );
    assert_eq!(
        recorded_lease_owner(Some((7, None)), None, dead),
        RecordedLeaseOwner::NotProvablyDead
    );
}

#[test]
fn protected_lease_verdict_matrix() {
    use RecordedLeaseOwner::*;
    assert!(matches!(
        classify_protected_lease(true, true, Dead),
        ProtectedLeaseVerdict::Release(_)
    ));
    assert!(matches!(
        classify_protected_lease(true, true, Unrecorded),
        ProtectedLeaseVerdict::UnknownOwner(_)
    ));
    assert!(matches!(
        classify_protected_lease(true, true, NotProvablyDead),
        ProtectedLeaseVerdict::Keep(_)
    ));
    // Another host's pids cannot be probed here.
    for owner in [Dead, Unrecorded, NotProvablyDead] {
        assert!(matches!(
            classify_protected_lease(true, false, owner),
            ProtectedLeaseVerdict::Keep(_)
        ));
        // A worktree-less advisory lease keeps its own lifecycle.
        assert!(matches!(
            classify_protected_lease(false, true, owner),
            ProtectedLeaseVerdict::Keep(_)
        ));
    }
}

fn lease_json(id: &str, worktree: &Path, extra: serde_json::Value) -> SessionLease {
    let mut v = serde_json::json!({
        "id": id,
        "scope": "harness-worktree",
        "slug": "harness-worktree",
        "owner": "test",
        "worktree_path": worktree,
        "branch": "main",
        "started_at": "2026-10-03T00:00:00Z",
        "hostname": crate::coordination::hostname(),
    });
    for (k, val) in extra.as_object().unwrap() {
        v[k] = val.clone();
    }
    serde_json::from_value(v).expect("fixture lease parses")
}

fn live(pid: u32, cwd: &Path) -> process_probe::LiveSession {
    process_probe::LiveSession {
        pid,
        cwd: cwd.to_path_buf(),
        jsonl: None,
        stale_cwd: false,
    }
}

#[test]
fn pidless_lease_is_never_attributed_to_the_inspector() {
    let root = PathBuf::from("/checkout");
    let inspector: HashSet<u32> = [100, 101].into_iter().collect();
    let sessions = vec![live(101, &root)];
    let now = chrono::Utc::now();

    let pidless = lease_json("a0000000pidl", &root, serde_json::json!({}));
    let attributable = attributable_live_sessions(&pidless, &sessions, &inspector);
    assert!(
        attributable.is_empty(),
        "the inspector must be filtered out"
    );
    assert_ne!(
        lease_state_for(&pidless, &attributable, now),
        LeaseState::Live
    );
    assert!(lease_owner_unknown(&pidless, None));

    // A different live process in the tree still counts (the existing rule).
    let other = vec![live(555, &root)];
    assert_eq!(
        attributable_live_sessions(&pidless, &other, &inspector).len(),
        1
    );
    assert!(!lease_owner_unknown(&pidless, Some(555)));

    // A lease that records a pid keeps the full list; it is not "unknown".
    let with_pid = lease_json(
        "a0000000wpid",
        &root,
        serde_json::json!({ "creator_pid": 4_000_000_u32 }),
    );
    assert_eq!(
        attributable_live_sessions(&with_pid, &sessions, &inspector).len(),
        1
    );
    assert!(!lease_owner_unknown(&with_pid, None));
}

#[test]
fn pidless_lease_does_not_pin_handoff_compaction() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let pidless = lease_json("a0000000pidl", &root, serde_json::json!({}));
    let with_pid = lease_json(
        "a0000000wpid",
        &root,
        serde_json::json!({ "creator_pid": 4_000_000_u32 }),
    );
    let (pins, unknown) = crate::session_misc::active_handoff_leases(&[pidless], Some(&root));
    assert!(pins.is_empty());
    assert_eq!(unknown.len(), 1);
    let (pins, unknown) = crate::session_misc::active_handoff_leases(&[with_pid], Some(&root));
    assert_eq!(pins.len(), 1);
    assert!(unknown.is_empty());
}

// ---------------------------------------------------------------------------
// The primary-checkout case, end to end against a throwaway repo.
// ---------------------------------------------------------------------------

fn git(root: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A main checkout with a tracked file, an uncommitted edit, and an untracked
/// file, so "untouched" covers every kind of checkout content.
fn primary_checkout() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("README.md"), "base\n").unwrap();
    git(&root, &["add", "README.md"]);
    git(&root, &["commit", "-q", "-m", "init"]);
    git(&root, &["branch", "-M", "main"]);
    std::fs::write(root.join("README.md"), "base\nuncommitted edit\n").unwrap();
    std::fs::write(root.join("scratch.txt"), "untracked\n").unwrap();
    std::fs::create_dir_all(root.join(".aida/sessions")).unwrap();
    let root = root.canonicalize().unwrap();
    (tmp, root)
}

/// Every file under the checkout except the lease dir, with its bytes.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let p = entry.path();
            let rel = p.strip_prefix(root).unwrap().to_path_buf();
            if rel.starts_with(".aida/sessions") {
                continue;
            }
            if p.is_dir() {
                walk(&p, root, out);
            } else {
                out.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn write_lease(root: &Path, lease: &SessionLease) {
    std::fs::write(
        lease_path(root, &lease.id),
        toml::to_string_pretty(lease).unwrap(),
    )
    .unwrap();
}

/// A pid that is provably dead, with the start identity it had while alive.
fn dead_pid_with_identity() -> (u32, Option<String>) {
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn sleep");
    let pid = child.id();
    let identity = process_probe::process_start_identity(pid);
    child.kill().unwrap();
    child.wait().unwrap();
    (pid, identity)
}

#[cfg(unix)]
#[test]
fn dead_pid_lease_on_primary_checkout_is_released_and_checkout_untouched() {
    let (_tmp, root) = primary_checkout();
    let (pid, identity) = dead_pid_with_identity();
    let dead = lease_json(
        "adad1e080000",
        &root,
        serde_json::json!({ "creator_pid": pid, "creator_pid_start_time": identity }),
    );
    // A lease whose recorded pid is this very test process: alive, kept.
    let me = std::process::id();
    let alive = lease_json(
        "a11fe0000000",
        &root,
        serde_json::json!({
            "creator_pid": me,
            "creator_pid_start_time": process_probe::process_start_identity(me),
        }),
    );
    let pidless = lease_json("01a0c96c0000", &root, serde_json::json!({}));
    for l in [&dead, &alive, &pidless] {
        write_lease(&root, l);
    }
    let before = snapshot(&root);

    let report = scan_reapable(&root);
    assert!(
        report.reapable.is_empty(),
        "the main checkout is never reaped"
    );
    assert_eq!(
        report
            .releasable
            .iter()
            .map(|r| r.session.as_str())
            .collect::<Vec<_>>(),
        vec!["adad1e080000"]
    );
    assert_eq!(
        report
            .unknown_owner
            .iter()
            .map(|r| r.session.as_str())
            .collect::<Vec<_>>(),
        vec!["01a0c96c0000"]
    );
    assert!(report.skipped.iter().any(|r| r.session == "a11fe0000000"));

    // Dead owner: released. Unknown owner without the flag, and a live owner:
    // kept even when asked.
    assert!(release_protected_lease(&root, "adad1e080000", false).starts_with("released"));
    assert!(release_protected_lease(&root, "01a0c96c0000", false).starts_with("kept"));
    assert!(release_protected_lease(&root, "a11fe0000000", true).starts_with("kept"));
    assert!(!lease_path(&root, "adad1e080000").exists());
    assert!(lease_path(&root, "01a0c96c0000").exists());
    assert!(lease_path(&root, "a11fe0000000").exists());

    // Unknown owner with the explicit flag: released.
    assert!(release_protected_lease(&root, "01a0c96c0000", true).starts_with("released"));
    assert!(!lease_path(&root, "01a0c96c0000").exists());

    assert_eq!(
        snapshot(&root),
        before,
        "the checkout must stay byte-identical"
    );
    assert!(root.join(".git").is_dir());
}

#[cfg(unix)]
#[test]
fn lease_restamped_live_after_scan_is_kept() {
    let (_tmp, root) = primary_checkout();
    let (pid, identity) = dead_pid_with_identity();
    let mut lease = lease_json(
        "adad1e080001",
        &root,
        serde_json::json!({ "creator_pid": pid, "creator_pid_start_time": identity }),
    );
    write_lease(&root, &lease);
    assert_eq!(scan_reapable(&root).releasable.len(), 1);
    // Between scan and release a live process re-stamps the lease.
    let me = std::process::id();
    lease.active_pid = Some(me);
    lease.active_pid_start_time = process_probe::process_start_identity(me);
    write_lease(&root, &lease);
    assert!(release_protected_lease(&root, "adad1e080001", true).starts_with("kept"));
    assert!(lease_path(&root, "adad1e080001").exists());
}
