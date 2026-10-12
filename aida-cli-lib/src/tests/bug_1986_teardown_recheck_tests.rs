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
