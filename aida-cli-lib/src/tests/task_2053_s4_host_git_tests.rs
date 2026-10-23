//! TASK-2053 (BUG-2169 slice S4): the host paths that read a session's tree
//! (`aida status`/`ps`/statusbar, burndown status, lane liveness, doctor's
//! lease heal, `agent new` re-entry, the drain's implementer push) never run
//! git in a contained seat's private clone. Each is run against a clone with a
//! logging `git` first on PATH; every invocation inside the clone must be the
//! sandboxed accessor's own (tagged), and a negative control shows the check
//! flags an untagged one.
//! trace:TASK-2053 | ai:claude

use super::tests::{clone_invocations, fixture, Fixture};
use super::*;
use std::path::{Path, PathBuf};

/// A logging `git` shim first on PATH; returns (shim dir, log, real git).
fn git_shim(fx: &Fixture) -> (PathBuf, PathBuf, String) {
    let real_git = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join("git"))
        .find(|candidate| candidate.is_file())
        .expect("git on PATH")
        .display()
        .to_string();
    let shim_dir = fx.root.join("shim");
    std::fs::create_dir_all(&shim_dir).unwrap();
    let log = fx.root.join("git.log");
    std::fs::write(&log, "").unwrap();
    crate::test_exec::write_executable(
        &shim_dir.join("git"),
        format!(
            "#!/bin/sh\n{{ printf '%s\\t%s\\t' \"${{{ACCESSOR_ENV}:-0}}\" \"$PWD\"; \
             for a in \"$@\"; do printf '%s\\037' \"$a\"; done; printf '\\n'; }} >> '{}'\n\
             exec '{real_git}' \"$@\"\n",
            log.display()
        ),
    );
    (shim_dir, log, real_git)
}

/// A seat clone at its real location (a sibling of main) and its `clone`
/// lease.
fn leased_clone(fx: &Fixture, name: &str, id: &str) -> PathBuf {
    let clone = fx.root.join(format!("{CLONE_DIR_PREFIX}aida-{name}"));
    create(&fx.main, &clone, "feat", "origin/main").unwrap();
    let path = crate::lease_path(&fx.main, id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        format!(
            "id = \"{id}\"\nscope = \"TASK-2053\"\nslug = \"task-2053\"\nowner = \"test\"\n\
             worktree_path = {}\nbranch = \"feat\"\nstarted_at = \"2026-10-21T00:00:00Z\"\n\
             hostname = \"h\"\ntree_kind = \"clone\"\n",
            aida_core::toml_quote::toml_string(&clone.display().to_string())
        ),
    )
    .unwrap();
    clone.canonicalize().unwrap()
}

fn untagged_in(log: &Path, clone: &Path) -> Vec<String> {
    let body = std::fs::read_to_string(log).unwrap();
    clone_invocations(&body, clone)
        .into_iter()
        .filter(|(tagged, _)| !tagged)
        .map(|(_, line)| line)
        .collect()
}

#[cfg(unix)]
#[test]
fn routine_status_and_drain_paths_run_no_host_git_in_a_seat_clone() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = fixture();
    let clone = leased_clone(&fx, "s4", "c0ffee000041");
    let (shim_dir, log, real_git) = git_shim(&fx);
    let _path = crate::test_env::EnvVarGuard::prepend_path(&shim_dir);

    let _ = crate::dispatch_health_ps::probe_worktree(&clone);
    assert_eq!(crate::dispatch_health_ps::current_branch_at(&clone), None);
    let _ = crate::dispatch_health_ps::is_protected_branch_at(&clone, "feat");
    assert_eq!(crate::worktree_gc::current_branch_at(&clone), None);
    assert_eq!(crate::worktree_gc::detect_default_branch_ref(&clone), None);
    assert!(crate::worktree_gc::git_output_lossy(&clone, &["diff", "HEAD"]).is_empty());
    let _ = crate::burndown_run::worktree_last_activity(&clone);
    assert!(crate::lane_liveness::branch_latest_commit_ts(&clone, "feat").is_none());
    assert!(crate::orchestrator_leases::git_output_checked(&clone, &["log", "-1"]).is_err());
    assert!(
        crate::orchestrator_leases::push_branch_from_implementer_worktree(&clone, "feat").is_err()
    );
    assert!(!aida_core::git_ops::is_git_repo(&clone));

    let violations = untagged_in(&log, &clone);
    assert!(
        violations.is_empty(),
        "host git ran in a seat clone: {violations:#?}"
    );
    // Negative control: an untagged host git in the clone is flagged.
    let _ = std::process::Command::new(&real_git)
        .arg("-C")
        .arg(&clone)
        .args(["status", "--porcelain"])
        .output();
    let _ = std::process::Command::new(shim_dir.join("git"))
        .arg("-C")
        .arg(&clone)
        .args(["status", "--porcelain"])
        .output();
    assert!(!untagged_in(&log, &clone).is_empty());
}

/// Doctor's lease heal and the other bulk lease cleanups go through
/// `force_cleanup_lease`: for a clone lease it is `seat_clone::destroy`
/// (salvage first), never `git worktree remove`, and the lease goes only
/// once the clone is gone.
#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_bulk_lease_cleanup_destroys_a_seat_clone_salvage_first() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let live = std::process::Command::new("bwrap")
        .args(["--unshare-all", "--ro-bind", "/", "/", "--dev", "/dev"])
        .args(["--proc", "/proc", "true"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !live {
        assert!(
            std::env::var_os("AIDA_REQUIRE_BWRAP_LIVE").is_none(),
            "AIDA_REQUIRE_BWRAP_LIVE=1 but bwrap cannot run here"
        );
        eprintln!("SKIP (bwrap unavailable)");
        return;
    }
    let fx = fixture();
    let clone = leased_clone(&fx, "heal", "c0ffee000042");
    std::fs::write(clone.join("wip.txt"), "unsaved\n").unwrap();
    let (shim_dir, log, _) = git_shim(&fx);
    let _path = crate::test_env::EnvVarGuard::prepend_path(&shim_dir);

    let patch = crate::worktree_gc::salvage_worktree_patch(&fx.main, "TASK-2053", None, &clone)
        .unwrap()
        .expect("unshipped work is salvaged");
    assert!(std::fs::read_to_string(patch).unwrap().contains("+unsaved"));

    let lease = crate::list_leases(&fx.main)
        .into_iter()
        .find(|l| l.id == "c0ffee000042")
        .unwrap();
    assert!(crate::leases::force_cleanup_lease(&fx.main, &lease));
    assert!(!clone.exists());
    assert!(crate::list_leases(&fx.main)
        .iter()
        .all(|l| l.id != lease.id));
    let violations = untagged_in(&log, &clone);
    assert!(
        violations.is_empty(),
        "host git ran in a seat clone: {violations:#?}"
    );
}

/// Review round 1, item 3: the fail-closed paths for a seat clone the probe
/// cannot read (its `.git` rewired to a `gitdir:` file reads as Unknown with
/// no bwrap run). Agent re-entry refuses; salvage errors, its caller (doctor's
/// orphan heal) stops on that error, and nothing is written or removed.
#[cfg(unix)]
#[test]
fn an_unreadable_seat_clone_fails_reentry_and_salvage_closed() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = fixture();
    let clone = leased_clone(&fx, "unreadable", "c0ffee000043");
    std::fs::rename(clone.join(".git"), clone.join("real-git")).unwrap();
    std::fs::write(clone.join(".git"), "gitdir: real-git\n").unwrap();
    let (shim_dir, log, _) = git_shim(&fx);
    let _path = crate::test_env::EnvVarGuard::prepend_path(&shim_dir);

    let error = crate::agent_launch::preflight_agent_launch_lease(
        &fx.main,
        None,
        Some("TASK-2053"),
        Some(&clone),
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("could not verify the owning seat clone's branch"),
        "{error}"
    );
    let error = crate::worktree_gc::salvage_worktree_patch(&fx.main, "TASK-2053", None, &clone)
        .unwrap_err()
        .to_string();
    assert!(error.contains("nothing was salvaged or removed"), "{error}");
    let finding = crate::doctor_root::DoctorFinding {
        category: "orphan-worktree".into(),
        id: clone.display().to_string(),
        summary: String::new(),
        action: String::new(),
        safe_heal: true,
    };
    let error = crate::doctor_cmd::heal_doctor_orphan_worktree(&fx.main, &finding)
        .unwrap_err()
        .to_string();
    assert!(error.contains("nothing was salvaged or removed"), "{error}");
    assert!(clone.join("real-git").is_dir());
    assert!(!fx.main.join(".aida/salvage").exists());
    let violations = untagged_in(&log, &clone);
    assert!(
        violations.is_empty(),
        "host git ran in a seat clone: {violations:#?}"
    );
}

/// Review round 1, item 3: a seat clone with a submodule (HasSubmodules)
/// fails agent re-entry and salvage the same way.
#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_a_submodule_clone_fails_reentry_and_salvage_closed() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let live = std::process::Command::new("bwrap")
        .args(["--unshare-all", "--ro-bind", "/", "/", "--dev", "/dev"])
        .args(["--proc", "/proc", "true"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !live {
        assert!(
            std::env::var_os("AIDA_REQUIRE_BWRAP_LIVE").is_none(),
            "AIDA_REQUIRE_BWRAP_LIVE=1 but bwrap cannot run here"
        );
        eprintln!("SKIP (bwrap unavailable)");
        return;
    }
    let fx = fixture();
    let clone = leased_clone(&fx, "submodule", "c0ffee000044");
    // Submodule evidence is a `.gitmodules` the probe finds; it need not be
    // a working submodule.
    std::fs::write(
        clone.join(".gitmodules"),
        "[submodule \"sm\"]\n\tpath = sm\n\turl = ../nowhere\n",
    )
    .unwrap();

    let error = crate::agent_launch::preflight_agent_launch_lease(
        &fx.main,
        None,
        Some("TASK-2053"),
        Some(&clone),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("submodules"), "{error}");
    let error = crate::worktree_gc::salvage_worktree_patch(&fx.main, "TASK-2053", None, &clone)
        .unwrap_err()
        .to_string();
    assert!(error.contains("has submodules"), "{error}");
    assert!(clone.join(".gitmodules").is_file());
    assert!(!fx.main.join(".aida/salvage").exists());
}
