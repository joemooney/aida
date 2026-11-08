//! Explicit force-claim recovers an exited implementer clone before branch
//! occupancy blocks pickup, without losing unpublished work.
// trace:BUG-2455 | ai:codex
use super::s4_host_git_tests::leased_clone_on;
use super::tests::fixture;
use super::*;

// Keep maintenance disabled in every fixture write, including imported-helper calls.
// trace:BUG-2455 | ai:codex
fn git_ok(dir: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn set_owner(main: &Path, pid: Option<u32>) {
    let mut lease = crate::list_leases(main).into_iter().next().unwrap();
    lease.pid_namespace = crate::agent_registry::current_pid_namespace();
    lease.active_pid = pid;
    lease.active_pid_start_time = pid.and_then(crate::process_probe::process_start_identity);
    std::fs::write(
        crate::lease_path(main, &lease.id),
        toml::to_string_pretty(&lease).unwrap(),
    )
    .unwrap();
    seed_owner(main, &lease);
}

fn seed_owner(main: &Path, lease: &crate::SessionLease) {
    let mut lease = lease.clone();
    lease.hostname = crate::coordination::hostname();
    if uuid::Uuid::parse_str(&lease.scope).is_err() {
        lease.scope = uuid::Uuid::new_v4().to_string();
    }
    if lease.active_pid.is_some() && lease.active_pid_start_time.is_none() {
        lease.active_pid_start_time = Some("linux-starttime:1".into());
    }
    // Cooperative holder evidence is seeded before exercising record readers.
    // A dead fixture PID cannot be captured from procfs after its exit.
    if let Some(pid) = lease.active_pid {
        let ns = crate::agent_registry::current_pid_namespace().unwrap();
        let ticks = lease
            .active_pid_start_time
            .as_deref()
            .unwrap_or("linux-starttime:1")
            .strip_prefix("linux-starttime:")
            .unwrap();
        let identity = aida_core::proc_identity::ProcIdentity::from_record(
            &aida_core::process_probe::ProcessIdentity {
                pid,
                start: format!("linux:{}:{ticks}", aida_core::local_boot_id().unwrap()),
            },
            Some(ns),
        )
        .unwrap();
        let record = serde_json::json!({"version":1,
            "node_id": aida_core::proc_identity::local_node_id().unwrap(),
            "identity":identity,"generation":"force-claim-fixture","refusal":null});
        lease.holder_record = Some(record.to_string());
    } else {
        lease.holder_record = None;
    }
    std::fs::write(
        crate::lease_path(main, &lease.id),
        toml::to_string(&lease).unwrap(),
    )
    .unwrap();
    crate::ownership::dual::enable_for_test();
    crate::ownership::dual::lease_written(main, &lease);
}

fn dead_pid() -> u32 {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

fn reclaim(main: &Path) -> Result<()> {
    crate::queue_cmd::preflight_queue_work_branch_reclaiming(main, Some("rework"), None, true)
}

fn bwrap_live() -> bool {
    let live = std::process::Command::new("bwrap")
        .args([
            "--unshare-all",
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "true",
        ])
        .status()
        .is_ok_and(|s| s.success());
    assert!(live || std::env::var_os("AIDA_REQUIRE_BWRAP_LIVE").is_none());
    if !live {
        eprintln!("SKIP (bwrap unavailable)");
    }
    live
}

// trace:BUG-2491 | ai:codex
#[test]
fn bug_2491_bwrap_force_claim_retires_unreachable_slice_tags() {
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let _env = crate::test_env::AmbientGuard::hermetic_and(
        &fx.main,
        None,
        &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
    );
    let clone = leased_clone_on(&fx, "2491", "2491c1", "rework");
    git_ok(
        &clone,
        &[
            "push",
            fx.origin.to_str().unwrap(),
            "HEAD:refs/heads/rework",
        ],
    );
    git_ok(&clone, &["checkout", "-qb", "checkpoint"]);
    std::fs::write(clone.join("checkpoint.txt"), "pre-squash work\n").unwrap();
    git_ok(&clone, &["add", "checkpoint.txt"]);
    git_ok(
        &clone,
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-qm",
            "checkpoint",
        ],
    );
    git_ok(&clone, &["tag", "slice-00-r1"]);
    git_ok(
        &clone,
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.com",
            "tag",
            "-a",
            "slice-01-r1",
            "-m",
            "checkpoint",
        ],
    );
    git_ok(&clone, &["checkout", "-q", "rework"]);
    git_ok(&clone, &["branch", "-D", "checkpoint"]);
    set_owner(&fx.main, Some(dead_pid()));
    reclaim(&fx.main).unwrap();
    assert!(!clone.exists());
    assert!(crate::list_leases(&fx.main).is_empty());
    let salvage_dir = fx
        .main
        .join(".aida/salvage")
        .join(clone.file_name().unwrap());
    let bundle = std::fs::read_dir(salvage_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "bundle"))
        .expect("unreachable checkpoints must be bundled before teardown");
    let output = crate::test_git::git_command()
        .arg("-C")
        .arg(&fx.main)
        .args(["bundle", "list-heads"])
        .arg(bundle)
        .output()
        .unwrap();
    assert!(output.status.success());
    let heads = String::from_utf8(output.stdout).unwrap();
    assert!(heads.contains("refs/tags/slice-00-r1"), "{heads}");
    assert!(heads.contains("refs/tags/slice-01-r1"), "{heads}");
}

#[test]
fn bug_2455_bwrap_force_claim_retires_clean_pushed_dead_clone_and_lease() {
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let _env = crate::test_env::AmbientGuard::hermetic_and(
        &fx.main,
        None,
        &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
    );
    let clone = leased_clone_on(&fx, "2455", "2455c1", "rework");
    // A seat-local commit must be covered on origin, not merely in main.
    std::fs::write(clone.join("done.txt"), "published\n").unwrap();
    git_ok(&clone, &["add", "done.txt"]);
    git_ok(
        &clone,
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-qm",
            "work",
        ],
    );
    git_ok(
        &clone,
        &[
            "push",
            fx.origin.to_str().unwrap(),
            "HEAD:refs/heads/rework",
        ],
    );
    set_owner(&fx.main, Some(dead_pid()));
    let pin = pin_ref(&clone).unwrap();
    assert!(crate::queue_cmd::preflight_queue_work_branch(&fx.main, Some("rework")).is_err());
    assert!(clone.exists());
    reclaim(&fx.main).unwrap();
    assert!(!clone.exists());
    assert!(crate::list_leases(&fx.main).is_empty());
    assert!(!super::tests::ref_exists(&fx.main, &pin));
}

#[test]
fn bug_2455_bwrap_dirty_clone_refuses_with_path_and_diff_stat_and_keeps_work() {
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let _env = crate::test_env::AmbientGuard::hermetic_and(
        &fx.main,
        None,
        &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
    );
    let clone = leased_clone_on(&fx, "2455dirty", "2455c2", "rework");
    set_owner(&fx.main, Some(dead_pid()));
    std::fs::write(clone.join("README.md"), "seed\npending\n").unwrap();
    std::fs::write(clone.join("new.txt"), "untracked\n").unwrap();
    let lease_before = std::fs::read(crate::lease_path(&fx.main, "2455c2")).unwrap();
    let err = reclaim(&fx.main).unwrap_err().to_string();
    assert!(err.contains(&clone.display().to_string()), "{err}");
    assert!(
        err.contains("Diff stat:") && err.contains("insertion"),
        "{err}"
    );
    assert!(
        err.contains("README.md") && err.contains("new.txt"),
        "{err}"
    );
    assert_eq!(
        std::fs::read_to_string(clone.join("README.md")).unwrap(),
        "seed\npending\n"
    );
    assert!(clone.join("new.txt").exists());
    assert_eq!(
        std::fs::read(crate::lease_path(&fx.main, "2455c2")).unwrap(),
        lease_before
    );
}

#[test]
fn bug_2455_bwrap_clean_unpushed_clone_is_retained() {
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let _env = crate::test_env::AmbientGuard::hermetic_and(
        &fx.main,
        None,
        &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
    );
    let clone = leased_clone_on(&fx, "2455unpushed", "2455c3", "rework");
    set_owner(&fx.main, Some(dead_pid()));
    std::fs::write(clone.join("only-copy.txt"), "precious\n").unwrap();
    git_ok(&clone, &["add", "only-copy.txt"]);
    git_ok(
        &clone,
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-qm",
            "unpublished",
        ],
    );
    let err = reclaim(&fx.main).unwrap_err().to_string();
    assert!(err.contains("no remote has"), "{err}");
    assert!(err.contains("no tracked changes"), "{err}");
    assert!(!err.contains("untracked paths above"), "{err}");
    assert!(clone.join("only-copy.txt").exists());
    assert_eq!(crate::list_leases(&fx.main).len(), 1);
}

#[test]
fn bug_2455_bwrap_live_or_unprovable_owners_and_adopted_clones_are_retained() {
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let _env = crate::test_env::AmbientGuard::hermetic_and(
        &fx.main,
        None,
        &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
    );
    let clone = leased_clone_on(&fx, "2455live", "2455c4", "rework");
    set_owner(&fx.main, Some(std::process::id()));
    let err = reclaim(&fx.main).unwrap_err().to_string();
    assert!(err.contains("still live"), "{err}");
    set_owner(&fx.main, None);
    assert!(reclaim(&fx.main).is_err());
    set_owner(&fx.main, Some(dead_pid()));
    crate::queue_cmd::preflight_queue_work_branch_reclaiming(
        &fx.main,
        Some("rework"),
        Some(&clone),
        true,
    )
    .unwrap();
    assert!(clone.exists());
    assert_eq!(crate::list_leases(&fx.main).len(), 1);
}

// trace:BUG-2455 | ai:codex
#[test]
fn bug_2455_bwrap_all_clones_checked_before_any_retirement() {
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let _env = crate::test_env::AmbientGuard::hermetic_and(
        &fx.main,
        None,
        &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
    );
    let a = leased_clone_on(&fx, "2455multi-a", "2455multi-a", "rework");
    let b = leased_clone_on(&fx, "2455multi-b", "2455multi-b", "rework");
    // Make inventory ordering explicit: the safe clone precedes the refusal.
    let mut leases = crate::list_leases(&fx.main);
    for (index, lease) in leases.iter_mut().enumerate() {
        lease.pid_namespace = crate::agent_registry::current_pid_namespace();
        lease.active_pid = Some(dead_pid());
        lease.active_pid_start_time = None;
        lease.started_at += chrono::Duration::seconds(index as i64);
        std::fs::write(
            crate::lease_path(&fx.main, &lease.id),
            toml::to_string_pretty(lease).unwrap(),
        )
        .unwrap();
        seed_owner(&fx.main, lease);
    }
    let safe = &leases[0].worktree_path;
    let dirty = &leases[1].worktree_path;
    // Both heads are published; only the later clone has pending work.
    git_ok(&fx.main, &["push", "origin", "HEAD:refs/heads/rework"]);
    std::fs::write(dirty.join("README.md"), "pending\n").unwrap();
    let snapshots: Vec<_> = leases
        .iter()
        .map(|l| std::fs::read(crate::lease_path(&fx.main, &l.id)).unwrap())
        .collect();
    let err = reclaim(&fx.main).unwrap_err().to_string();
    assert!(err.contains(&dirty.display().to_string()), "{err}");
    assert!(safe.exists() && a.exists() && b.exists(), "{err}");
    for (lease, bytes) in leases.iter().zip(snapshots) {
        assert_eq!(
            std::fs::read(crate::lease_path(&fx.main, &lease.id)).unwrap(),
            bytes
        );
    }
    assert_eq!(
        std::fs::read_to_string(dirty.join("README.md")).unwrap(),
        "pending\n"
    );
}
