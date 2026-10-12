//! BUG-1986: `session end` re-checks for live processes immediately before
//! it resets or removes the worktree, so a process that starts after the
//! first check (while the operator sits at the confirmation prompt) still
//! stops the teardown.
// trace:BUG-1986 | ai:claude

use super::*;
use aida_core::worktree_holders::{HoldKind, WorktreeHolder, WorktreeInUse};
use std::cell::Cell;
use std::path::Path;

fn holder_at(worktree: &Path) -> WorktreeInUse {
    WorktreeInUse {
        worktree: worktree.to_path_buf(),
        holders: vec![WorktreeHolder {
            pid: 4242,
            command: "cargo build".into(),
            hold: HoldKind::Cwd,
        }],
        unverified: None,
        pid_namespace: false,
    }
}

/// An injected scan that is clean on the first call (the early check) and
/// sees a holder on the second (the recheck): the recheck refuses, naming
/// the holder and the dedicated override, not `--force`.
#[test]
fn a_holder_that_appears_after_the_first_check_is_caught() {
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path();
    let calls = Cell::new(0);
    let scan = |p: &Path| {
        calls.set(calls.get() + 1);
        if calls.get() == 1 {
            Ok(())
        } else {
            Err(holder_at(p))
        }
    };
    assert!(recheck_holders_before_teardown(wt, false, scan).is_ok());
    let refusal = recheck_holders_before_teardown(wt, false, scan)
        .expect_err("the recheck must catch the new holder");
    assert!(refusal.contains("pid 4242"), "{refusal}");
    assert!(refusal.contains("cargo build"), "{refusal}");
    assert!(refusal.contains("--include-in-use"), "{refusal}");
    assert!(!refusal.contains("--force"), "{refusal}");
    assert_eq!(calls.get(), 2);
}

/// `--include-in-use` skips the recheck; an empty path never scans.
#[test]
fn include_in_use_and_worktree_less_leases_skip_the_recheck() {
    let tmp = tempfile::tempdir().unwrap();
    let never = |_: &Path| -> Result<(), WorktreeInUse> { panic!("must not scan") };
    assert!(recheck_holders_before_teardown(tmp.path(), true, never).is_ok());
    assert!(recheck_holders_before_teardown(Path::new(""), false, never).is_ok());
}

/// The same race against the real `/proc` scan: the tree is free at the
/// first check, then a `sleep` starts inside it, and the recheck sees it.
#[cfg(target_os = "linux")]
#[test]
fn the_real_scan_catches_a_process_started_after_the_first_check() {
    struct KillOnDrop(std::process::Child);
    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().join("wt");
    std::fs::create_dir_all(&wt).unwrap();
    let check = aida_core::worktree_holders::check_worktree_removal_interactive;
    assert!(
        recheck_holders_before_teardown(&wt, false, check).is_ok(),
        "free at the first check"
    );
    let child = KillOnDrop(
        std::process::Command::new("sleep")
            .arg("600")
            .current_dir(&wt)
            .spawn()
            .unwrap(),
    );
    let refusal = recheck_holders_before_teardown(&wt, false, check)
        .expect_err("a process started after the first check must be caught");
    assert!(
        refusal.contains(&format!("pid {}", child.0.id())),
        "{refusal}"
    );
    assert!(refusal.contains("--include-in-use"), "{refusal}");
}

/// The dirty gate now runs while the session's runtime links are still in
/// place, so it must not count those links as uncommitted work. A real file
/// at a link path, or any other untracked entry, still counts.
// trace:BUG-1986 | ai:claude
#[cfg(unix)]
#[test]
fn the_dirty_gate_ignores_only_the_sessions_own_runtime_links() {
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path();
    let target = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::create_dir_all(wt.join(".aida")).unwrap();
    std::os::unix::fs::symlink(&target, wt.join(".aida-store")).unwrap();
    std::os::unix::fs::symlink(&target, wt.join(".aida/sessions")).unwrap();
    std::os::unix::fs::symlink(target.join("c"), wt.join(".aida/cache.db")).unwrap();
    // A real file at a runtime path is not a link, so it is work.
    std::fs::write(wt.join(".aida/roles"), "real").unwrap();
    let entries = vec![
        "?? .aida-store".to_string(),
        "?? .aida/sessions".to_string(),
        "?? .aida/cache.db".to_string(),
        "?? .aida/roles".to_string(),
        "?? notes.txt".to_string(),
        " M src/lib.rs".to_string(),
    ];
    assert_eq!(
        dirty_entries_ignoring_runtime_links(wt, entries),
        vec![
            "?? .aida/roles".to_string(),
            "?? notes.txt".to_string(),
            " M src/lib.rs".to_string(),
        ]
    );
}

fn git_in(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// BUG-1988: with `.aida/` NOT gitignored and nothing tracked under it, git
/// collapses it to a bare `?? .aida/`. When everything under it is a session
/// runtime link the dirty gate must not refuse; a real file under it still
/// counts, and is named on its own.
// trace:BUG-1988 | ai:claude
#[cfg(unix)]
#[test]
fn a_bare_untracked_aida_dir_of_runtime_links_is_not_dirty() {
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().join("wt");
    let parent = tmp.path().join("parent");
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::create_dir_all(parent.join("sessions")).unwrap();
    git_in(&wt, &["init", "-q", "-b", "main"]);
    git_in(
        &wt,
        &[
            "-c",
            "user.email=t@example.com",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    std::fs::create_dir_all(wt.join(".aida")).unwrap();
    std::os::unix::fs::symlink(parent.join("sessions"), wt.join(".aida/sessions")).unwrap();
    std::os::unix::fs::symlink(parent.join("cache.db"), wt.join(".aida/cache.db")).unwrap();
    std::os::unix::fs::symlink(&parent, wt.join(".aida-store")).unwrap();

    let raw = worktree_dirty_entries(&wt);
    assert!(
        raw.iter().any(|e| e == "?? .aida/"),
        "fixture must reproduce the collapsed entry: {raw:?}"
    );
    assert!(
        dirty_entries_ignoring_runtime_links(&wt, raw).is_empty(),
        "runtime links alone are not work"
    );

    std::fs::write(wt.join(".aida/notes.txt"), "real work").unwrap();
    assert_eq!(
        dirty_entries_ignoring_runtime_links(&wt, worktree_dirty_entries(&wt)),
        vec!["?? .aida/notes.txt".to_string()]
    );
}

/// BUG-1988: an unverified scan (no holder seen) is downgraded on every
/// call, but its warning is emitted once per session end. A visible holder
/// still refuses and emits nothing.
// trace:BUG-1988 | ai:claude
#[test]
fn an_unverified_scan_warns_once_across_the_three_checks() {
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path();
    let unverified = |p: &Path| -> Result<(), WorktreeInUse> {
        Err(WorktreeInUse {
            worktree: p.to_path_buf(),
            holders: Vec::new(),
            unverified: Some("separate pid namespace".into()),
            pid_namespace: true,
        })
    };
    let warned = Cell::new(false);
    let emitted = std::cell::RefCell::new(Vec::<String>::new());
    let check = interactive_holder_check(unverified, &warned, |w| {
        emitted.borrow_mut().push(w.to_string())
    });
    assert!(check(wt).is_ok());
    assert!(recheck_holders_before_teardown(wt, false, &check).is_ok());
    assert!(recheck_holders_before_teardown(wt, false, &check).is_ok());
    drop(check);
    let emitted = emitted.into_inner();
    assert_eq!(emitted.len(), 1, "{emitted:?}");
    assert!(emitted[0].contains("separate pid namespace"), "{emitted:?}");

    let warned = Cell::new(false);
    let check = interactive_holder_check(
        |p: &Path| Err(holder_at(p)),
        &warned,
        |w| panic!("a holder must not be downgraded to a warning: {w}"),
    );
    let refusal =
        recheck_holders_before_teardown(wt, false, &check).expect_err("a visible holder refuses");
    assert!(refusal.contains("pid 4242"), "{refusal}");
}

/// BUG-1988: the failed-salvage refusal says the session was not ended and
/// names the command that finishes it.
// trace:BUG-1988 | ai:claude
#[test]
fn the_failed_salvage_refusal_names_the_recovery_command() {
    let msg = session_end_salvage_failure_message(
        "20261012-abc",
        Path::new("/pool/wt-1"),
        Path::new("/repo"),
        &"Not a directory (os error 20)",
        "",
    );
    assert!(msg.contains("/pool/wt-1"), "{msg}");
    assert!(msg.contains("Not a directory"), "{msg}");
    assert!(msg.contains("NOT ended"), "{msg}");
    assert!(msg.contains("lease is kept"), "{msg}");
    assert!(msg.contains("/repo/.aida/salvage"), "{msg}");
    assert!(msg.contains("`aida session end 20261012-abc`"), "{msg}");
}
