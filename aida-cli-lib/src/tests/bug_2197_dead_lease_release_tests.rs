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
fn only_a_durable_owner_identity_proves_death() {
    let dead = |pid: u32, _: Option<&str>| pid != 7;
    let id = Some("linux-starttime:1");
    assert_eq!(
        recorded_lease_owner(None, None, dead),
        RecordedLeaseOwner::Unrecorded
    );
    // active_pid + start identity, dead: proven.
    assert_eq!(
        recorded_lease_owner(Some((1, id)), None, dead),
        RecordedLeaseOwner::Dead
    );
    assert_eq!(
        recorded_lease_owner(Some((1, id)), Some((2, id)), dead),
        RecordedLeaseOwner::Dead
    );
    // A dead creator shell alone is NOT durable: a per-command `bash -c`
    // exits seconds after minting a lease whose session keeps running.
    assert_eq!(
        recorded_lease_owner(None, Some((2, id)), dead),
        RecordedLeaseOwner::Unrecorded
    );
    // A dead active pid without its start identity is not durable either.
    assert_eq!(
        recorded_lease_owner(Some((1, None)), None, dead),
        RecordedLeaseOwner::Unrecorded
    );
    // Any recorded pid still alive keeps the lease, unknown-owner or not.
    assert_eq!(
        recorded_lease_owner(Some((1, id)), Some((7, id)), dead),
        RecordedLeaseOwner::NotProvablyDead
    );
    assert_eq!(
        recorded_lease_owner(Some((7, id)), None, dead),
        RecordedLeaseOwner::NotProvablyDead
    );
    assert_eq!(
        recorded_lease_owner(None, Some((7, None)), dead),
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
    let with_pid = lease_json("a0000000wpid", &root, live_creator());
    assert_eq!(
        attributable_live_sessions(&with_pid, &sessions, &inspector).len(),
        1
    );
    assert!(!lease_owner_unknown(&with_pid, None));
}

/// The exclusion lives inside `lease_state_for`, so every caller (`aida ps`,
/// status, role, driver views) gets it without pre-filtering.
#[test]
fn lease_state_for_excludes_the_inspector_for_pidless_leases() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let now = chrono::Utc::now();
    let fresh = serde_json::json!({ "started_at": now.to_rfc3339() });
    let pidless = lease_json("a0000000pidl", &root, fresh);
    let me = vec![live(std::process::id(), &root)];
    assert_eq!(lease_state_for(&pidless, &me, now), LeaseState::Dormant);
    // Another live claude in the tree still makes it live.
    let other = vec![live(4_000_001, &root)];
    assert_eq!(lease_state_for(&pidless, &other, now), LeaseState::Live);
}

#[test]
fn pidless_lease_does_not_pin_handoff_compaction() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let pidless = lease_json("a0000000pidl", &root, serde_json::json!({}));
    let with_pid = lease_json("a0000000wpid", &root, live_creator());
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
        serde_json::json!({ "active_pid": pid, "active_pid_start_time": identity }),
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
        serde_json::json!({ "active_pid": pid, "active_pid_start_time": identity }),
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

/// Review finding 1: `aida session start` from an agent's shell tool records
/// `creator_pid` = a per-command `bash -c` that exits seconds later, with no
/// `active_pid`, while the session itself is LIVE. That lease is unknown owner:
/// never released by the default lane, even with `--yes`.
#[cfg(unix)]
#[test]
fn shell_minted_lease_of_a_live_session_is_not_released() {
    let (_tmp, root) = primary_checkout();
    let mut shell = std::process::Command::new("bash")
        .args(["-c", "sleep 30"])
        .spawn()
        .expect("spawn bash -c");
    let shell_pid = shell.id();
    let shell_identity = process_probe::process_start_identity(shell_pid);
    let lease = lease_json(
        "5e11000000aa",
        &root,
        serde_json::json!({
            "creator_pid": shell_pid,
            "creator_pid_start_time": shell_identity,
        }),
    );
    write_lease(&root, &lease);
    // The shell exits; the session it minted keeps working.
    shell.kill().unwrap();
    shell.wait().unwrap();

    let mut report = scan_reapable(&root);
    assert!(report.releasable.is_empty());
    assert_eq!(report.unknown_owner.len(), 1);
    let mut out = Vec::new();
    release_protected_leases_with(&root, &mut report, &lane_opts(), &mut out, |_| {
        panic!("--yes never prompts")
    })
    .unwrap();
    assert!(lease_path(&root, "5e11000000aa").exists());
    assert!(report.unknown_owner[0].outcome.is_none());
}

fn lane_opts() -> ReapOptions {
    ReapOptions {
        dry_run: false,
        yes: true,
        json: false,
        quiet_when_empty: false,
        release_unknown_owner: false,
        release_protected: true,
    }
}

/// A checkout holding one dead-owner lease and one unknown-owner lease.
#[cfg(unix)]
fn lane_fixture() -> (tempfile::TempDir, PathBuf) {
    let (tmp, root) = primary_checkout();
    let (pid, identity) = dead_pid_with_identity();
    write_lease(
        &root,
        &lease_json(
            "dead00000001",
            &root,
            serde_json::json!({ "active_pid": pid, "active_pid_start_time": identity }),
        ),
    );
    write_lease(
        &root,
        &lease_json("0000a0000002", &root, serde_json::json!({})),
    );
    (tmp, root)
}

fn run_lane(
    root: &Path,
    opts: &ReapOptions,
    confirm: impl FnOnce(usize) -> anyhow::Result<Option<bool>>,
) -> (ReapReport, String) {
    let mut report = scan_reapable(root);
    let mut out = Vec::new();
    release_protected_leases_with(root, &mut report, opts, &mut out, confirm).unwrap();
    (report, String::from_utf8(out).unwrap())
}

#[cfg(unix)]
#[test]
fn lane_dry_run_releases_nothing() {
    let (_tmp, root) = lane_fixture();
    let opts = ReapOptions {
        dry_run: true,
        release_unknown_owner: true,
        ..lane_opts()
    };
    let (report, out) = run_lane(&root, &opts, |_| panic!("dry run never prompts"));
    assert!(out.contains("--dry-run: no lease was released."), "{out}");
    assert!(lease_path(&root, "dead00000001").exists());
    assert!(lease_path(&root, "0000a0000002").exists());
    assert!(report.releasable.iter().all(|r| r.outcome.is_none()));
}

#[cfg(unix)]
#[test]
fn lane_without_yes_off_a_tty_releases_nothing() {
    let (_tmp, root) = lane_fixture();
    let opts = ReapOptions {
        yes: false,
        ..lane_opts()
    };
    let (_, out) = run_lane(&root, &opts, |_| Ok(None));
    assert!(out.contains("Re-run with --yes to release them."), "{out}");
    assert!(lease_path(&root, "dead00000001").exists());
    // An operator who answers no also releases nothing.
    let (_, out) = run_lane(&root, &opts, |_| Ok(Some(false)));
    assert!(out.contains("Aborted"), "{out}");
    assert!(lease_path(&root, "dead00000001").exists());
}

#[cfg(unix)]
#[test]
fn lane_releases_dead_owner_and_unknown_only_with_the_flag() {
    let (_tmp, root) = lane_fixture();
    let (report, _) = run_lane(&root, &lane_opts(), |_| panic!("--yes never prompts"));
    assert!(!lease_path(&root, "dead00000001").exists());
    assert!(lease_path(&root, "0000a0000002").exists());
    assert!(report.unknown_owner[0].outcome.is_none());

    let opts = ReapOptions {
        release_unknown_owner: true,
        ..lane_opts()
    };
    let (report, _) = run_lane(&root, &opts, |_| panic!("--yes never prompts"));
    assert!(!lease_path(&root, "0000a0000002").exists());
    assert!(report.unknown_owner[0]
        .outcome
        .as_deref()
        .is_some_and(|o| o.starts_with("released")));
}

#[cfg(unix)]
#[test]
fn lane_json_carries_outcome_and_prints_no_human_lines() {
    let (_tmp, root) = lane_fixture();
    let opts = ReapOptions {
        json: true,
        ..lane_opts()
    };
    let (report, out) = run_lane(&root, &opts, |_| panic!("--yes never prompts"));
    assert!(out.is_empty(), "json mode writes no human lines: {out}");
    let json = serde_json::to_value(&report).unwrap();
    let outcome = json["releasable"][0]["outcome"].as_str().unwrap();
    assert!(outcome.starts_with("released"), "{outcome}");
    assert_eq!(json["unknown_owner"][0]["verdict"], "unknown-owner");
    assert!(json["unknown_owner"][0].get("outcome").is_none());
}

#[cfg(unix)]
#[test]
fn lane_quiet_pass_names_only_what_it_acts_on() {
    let (_tmp, root) = lane_fixture();
    let opts = ReapOptions {
        quiet_when_empty: true,
        ..lane_opts()
    };
    let (_, out) = run_lane(&root, &opts, |_| panic!("--yes never prompts"));
    assert!(out.contains("dead0000"), "{out}");
    assert!(
        !out.contains("0000a000"),
        "unknown-owner rows stay quiet: {out}"
    );
}

/// The post-merge pass inside `aida pr ship` never runs the release lane.
#[cfg(unix)]
#[test]
fn lane_is_off_for_the_pr_ship_pass() {
    let (_tmp, root) = lane_fixture();
    let opts = ReapOptions {
        quiet_when_empty: true,
        release_protected: false,
        release_unknown_owner: true,
        ..lane_opts()
    };
    let (_, out) = run_lane(&root, &opts, |_| panic!("never prompts"));
    assert!(out.is_empty(), "{out}");
    assert!(lease_path(&root, "dead00000001").exists());
    assert!(lease_path(&root, "0000a0000002").exists());
}

/// A creator pid that is provably still alive: this test process.
fn live_creator() -> serde_json::Value {
    let me = std::process::id();
    serde_json::json!({
        "creator_pid": me,
        "creator_pid_start_time": process_probe::process_start_identity(me),
    })
}

/// Round-2 review: ONE durable-owner rule. A lease whose only record is a dead
/// creator shell is unknown owner on every surface — reap, `session leases` /
/// `aida ps` (state, pid attribution, owner_unknown), and `handoff --check`.
#[cfg(unix)]
#[test]
fn dead_creator_only_lease_is_unknown_owner_on_every_surface() {
    let (_tmp, root) = primary_checkout();
    let (pid, identity) = dead_pid_with_identity();
    let now = chrono::Utc::now();
    let lease = lease_json(
        "dc00000000aa",
        &root,
        serde_json::json!({
            "creator_pid": pid,
            "creator_pid_start_time": identity,
            "started_at": now.to_rfc3339(),
        }),
    );
    write_lease(&root, &lease);
    assert!(!lease_has_durable_owner(&lease));

    // reap: unknown owner, not releasable.
    let report = scan_reapable(&root);
    assert!(report.releasable.is_empty());
    assert_eq!(report.unknown_owner.len(), 1);

    // leases / ps: never credited to the inspector, reported unknown owner.
    let me = vec![live(std::process::id(), &root)];
    assert!(attributable_live_sessions(&lease, &me, inspector_pids()).is_empty());
    assert_eq!(lease_state_for(&lease, &me, now), LeaseState::Dormant);
    assert!(lease_owner_unknown(&lease, None));

    // handoff --check: not a compact pin.
    let (pins, unknown) = crate::session_misc::active_handoff_leases(&[lease], Some(&root));
    assert!(pins.is_empty());
    assert_eq!(unknown.len(), 1);
}

/// BUG-2206: an attested-reviewer lease (review verb, its own review
/// worktree, minted minutes ago) whose creator is provably dead is Stale in
/// `aida ps`, not Dormant; a live session in its worktree or a live creator
/// keeps it from being called over.
// trace:TASK-1955 | ai:claude (BUG-2206)
#[cfg(unix)]
#[test]
fn dead_creator_reviewer_lease_with_a_worktree_is_stale() {
    let tmp = tempfile::tempdir().unwrap();
    let worktree = tmp.path().canonicalize().unwrap().join("aida-pr-2776");
    std::fs::create_dir_all(&worktree).unwrap();
    let now = chrono::Utc::now();
    let reviewer = |creator: u32, identity: Option<String>| {
        lease_json(
            "01a15ac34759",
            &worktree,
            serde_json::json!({
                "scope": "PR-2776",
                "role": "reviewer",
                "review_verb": true,
                "started_at": now.to_rfc3339(),
                "creator_pid": creator,
                "creator_pid_start_time": identity,
            }),
        )
    };
    let (pid, identity) = dead_pid_with_identity();
    let dead = reviewer(pid, identity);
    assert_eq!(lease_state_for(&dead, &[], now), LeaseState::Stale);
    // A reviewer session still sitting in the worktree is live.
    assert_eq!(
        lease_state_for(&dead, &[live(4_000_001, &worktree)], now),
        LeaseState::Live
    );
    // A live creator is never called over.
    let me = std::process::id();
    let alive = reviewer(me, process_probe::process_start_identity(me));
    assert_ne!(lease_state_for(&alive, &[], now), LeaseState::Stale);
}

/// BUG-2206: `aida session reap` reaps a dead attested reviewer whose PR is
/// merged or closed (a PR scope has no spec to finish), adopting its
/// worktree merge-hold marker into the primary checkout first (BUG-2176).
/// An unknown PR state reaps nothing. Fixture repo only; never the live root.
// trace:TASK-1955 | ai:claude (BUG-2206)
#[cfg(unix)]
#[test]
fn dead_reviewer_of_a_merged_pr_is_reaped_and_its_hold_adopted() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let root = base.join("repo");
    std::fs::create_dir_all(root.join(".aida/sessions")).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("README.md"), "base\n").unwrap();
    git(&root, &["add", "README.md"]);
    git(&root, &["commit", "-q", "-m", "init"]);
    // Like the real clone: per-checkout `.aida/` state is not tracked.
    std::fs::write(root.join(".git/info/exclude"), "/.aida/\n").unwrap();
    let review = base.join("aida-pr-7");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "review/pr-7",
            review.to_str().unwrap(),
        ],
    );
    let mut hold = crate::merge_hold::typed_hold(
        7,
        crate::merge_hold::HoldReasonKind::Rework,
        "CHANGES REQUESTED for BUG-9 at 1111111",
        Some("1111111aaaa".into()),
    );
    hold.spec = Some("BUG-9".into());
    let marker = crate::merge_hold::local_hold_path(&review, 7);
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
    std::fs::write(&marker, serde_json::to_vec_pretty(&hold).unwrap()).unwrap();

    let (pid, identity) = dead_pid_with_identity();
    let pr_head = git_out(&review, &["rev-parse", "HEAD"]);
    let lease = lease_json(
        "01a15ac30007",
        &review,
        serde_json::json!({
            "scope": "PR-7",
            "slug": "pr-7",
            "branch": "review/pr-7",
            "role": "reviewer",
            "review_verb": true,
            "pr_head_sha": pr_head,
            "started_at": (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339(),
            "creator_pid": pid,
            "creator_pid_start_time": identity,
        }),
    );
    write_lease(&root, &lease);

    let unknown = scan_reapable_with(&root, |_| None);
    assert!(
        unknown.reapable.iter().all(|r| r.session != lease.id),
        "an unknown PR state never reaps"
    );
    // TASK-2095: an unpushed commit past the recorded PR head keeps it.
    // trace:TASK-1955 | ai:claude (TASK-2095)
    std::fs::write(review.join("fix.txt"), "fix-forward\n").unwrap();
    git(&review, &["add", "fix.txt"]);
    git(&review, &["commit", "-q", "-m", "fix-forward"]);
    assert!(
        scan_reapable_with(&root, |_| Some(true))
            .reapable
            .iter()
            .all(|r| r.session != lease.id),
        "a commit past the PR head is never reaped away"
    );
    git(&review, &["reset", "-q", "--hard", &pr_head]);

    let report = scan_reapable_with(&root, |pr| Some(pr == 7));
    let row = report
        .reapable
        .iter()
        .find(|r| r.session == lease.id)
        .unwrap_or_else(|| panic!("reviewer lease is reapable: {report:?}"));
    assert!(row.spec_finished, "not listed as an unfinished spec");

    let outcome = reap_one(&root, &lease, row.branch_tip.as_deref());
    assert!(!review.exists(), "review worktree removed: {outcome}");
    assert!(!lease_path(&root, &lease.id).exists(), "lease released");
    let adopted = crate::merge_hold::read_hold_record(&root, 7).expect("hold adopted");
    assert_eq!(adopted.spec.as_deref(), Some("BUG-9"));
}

fn git_out(repo: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// TASK-2098: `aida session leases` agrees with `aida ps`: a dead creator
/// pid is not the row's process.
// trace:TASK-1955 | ai:claude (TASK-2098)
#[cfg(unix)]
#[test]
fn session_leases_row_never_names_a_dead_creator_pid() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let worktree = root.join("aida-pr-9");
    std::fs::create_dir_all(&worktree).unwrap();
    let (pid, identity) = dead_pid_with_identity();
    let lease = lease_json(
        "01a15ac30009",
        &worktree,
        serde_json::json!({
            "scope": "PR-9",
            "role": "reviewer",
            "review_verb": true,
            "started_at": chrono::Utc::now().to_rfc3339(),
            "creator_pid": pid,
            "creator_pid_start_time": identity,
        }),
    );
    let rows = crate::leases::classify_session_leases(&root, &[lease]);
    assert_eq!(rows[0].2, None, "the dead creator is not the row's pid");
}
