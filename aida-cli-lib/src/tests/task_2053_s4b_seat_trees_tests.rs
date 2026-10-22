//! TASK-2053 (BUG-2169 slice S4b): host views that enumerate a session's
//! trees see contained seats' private clones, from their `clone` leases,
//! and never run host git in one. Probe-backed views (status rows, the
//! unshipped-work detector) are covered under a live bwrap.
//! trace:TASK-2053 | ai:claude

use super::s4_host_git_tests::{git_shim, leased_clone, leased_clone_on, untagged_in};
use super::tests::fixture;
use super::*;

fn bwrap_live() -> bool {
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
    }
    live
}

#[cfg(unix)]
#[test]
fn seat_trees_and_branch_lookups_see_a_clone_without_host_git_in_it() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = fixture();
    let clone = leased_clone_on(&fx, "s4b", "c0ffee000051", "task-2053-work");
    let (shim_dir, log, _) = git_shim(&fx);
    let _path = crate::test_env::EnvVarGuard::prepend_path(&shim_dir);

    // The enumerator: worktrees from git, then the clone from its lease.
    assert!(crate::list_worktrees(&fx.main).iter().all(|wt| !wt.clone));
    let trees = crate::seat_trees(&fx.main);
    let found = trees
        .iter()
        .find(|wt| wt.path == clone)
        .expect("the leased clone is listed");
    assert!(found.clone);
    assert_eq!(found.branch.as_deref(), Some("task-2053-work"));

    // A brief points at the seat, not at a fresh lineage or a worktree recipe.
    assert_eq!(
        crate::brief::resolve_brief_branch_target(&fx.main, "TASK-2053"),
        crate::brief::BriefBranchTarget::SeatClone {
            branch: "task-2053-work".to_string(),
            clone: clone.clone(),
        }
    );
    // `aida show` linkage names the clone.
    let linkage = crate::trace::collect_git_linkage(&fx.main, &["TASK-2053".to_string()]);
    assert_eq!(linkage.branch.as_deref(), Some("task-2053-work"));
    assert_eq!(linkage.worktree, Some(clone.display().to_string()));
    // Pickup on the clone's branch is refused, and never offers `git worktree remove`.
    let err = crate::queue_cmd::preflight_queue_work_branch(&fx.main, Some("task-2053-work"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("private clone"), "{err}");
    assert!(!err.contains("git worktree remove"), "{err}");
    // Runaway detection watches the clone seat's transcripts.
    if crate::home_dir().is_some() {
        let encoded = aida_core::liveness::encode_cwd_for_projects(&clone);
        assert!(crate::runaway_seats::default_sources(&fx.main)
            .transcript_dirs
            .iter()
            .any(|dir| dir.ends_with(&encoded)));
    }
    // Teardown and the failed-attempt unpushed count refuse / keep.
    assert!(
        aida_core::worktree_pool_destroy::teardown_worktree_path(&fx.main, &clone, &[]).is_err()
    );
    assert!(clone.exists());
    assert_eq!(
        crate::pr_lookup::local_commits_not_on_branch(&clone, "task-2053-work"),
        1
    );

    let violations = untagged_in(&log, &clone);
    assert!(
        violations.is_empty(),
        "host git ran in a seat clone: {violations:#?}"
    );
}

/// A merge-hold marker inside a clone is reported (removing the clone would
/// destroy it); a clone whose `.aida` is a symlink is not followed.
#[cfg(unix)]
#[test]
fn stray_merge_hold_markers_in_a_seat_clone_are_reported_never_through_a_symlink() {
    let fx = fixture();
    let clone = leased_clone(&fx, "holds", "c0ffee000052");
    std::fs::create_dir_all(clone.join(".aida/merge-holds")).unwrap();
    std::fs::write(clone.join(".aida/merge-holds/PR-7"), "hold\n").unwrap();
    let elsewhere = fx.root.join("elsewhere");
    std::fs::create_dir_all(elsewhere.join("merge-holds")).unwrap();
    std::fs::write(elsewhere.join("merge-holds/PR-9"), "hold\n").unwrap();
    let linked = leased_clone(&fx, "linked", "c0ffee000053");
    if linked.join(".aida").exists() {
        std::fs::remove_dir_all(linked.join(".aida")).unwrap();
    }
    std::os::unix::fs::symlink(&elsewhere, linked.join(".aida")).unwrap();

    let found: Vec<u64> = crate::merge_hold::stray_worktree_markers(&fx.main)
        .into_iter()
        .map(|(pr, _)| pr)
        .collect();
    assert!(found.contains(&7), "{found:?}");
    assert!(!found.contains(&9), "{found:?}");
}

/// Status rows and the unshipped-work detector read a clone through the
/// sandboxed probe: an unpushed commit and a dirty file show, attributed to
/// the lease's spec, with no host git in the clone.
#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_status_and_unshipped_report_a_seat_clones_work() {
    if crate::test_env::run_path_isolated_test() || !bwrap_live() {
        return;
    }
    let fx = fixture();
    let clone = leased_clone(&fx, "unshipped", "c0ffee000054");
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&clone)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    std::fs::write(clone.join("done.txt"), "work\n").unwrap();
    git(&["add", "done.txt"]);
    git(&["commit", "--no-verify", "-m", "work"]);
    std::fs::write(clone.join("wip.txt"), "unsaved\n").unwrap();
    let (shim_dir, log, _) = git_shim(&fx);
    let _path = crate::test_env::EnvVarGuard::prepend_path(&shim_dir);

    let items = crate::clone_unshipped_work_items(&fx.main);
    let item = items
        .iter()
        .find(|i| i.branch == "feat")
        .expect("the clone's unpushed commit is reported");
    assert_eq!(item.spec_id, "TASK-2053");
    assert_eq!(item.commits_ahead, 1);
    assert!(!item.pushed);
    assert!(item.recovery.contains(&clone.display().to_string()));

    let rows = crate::status_sections::collect_worktree_status_rows(&fx.main);
    let row = rows
        .iter()
        .find(|r| r.path == clone)
        .expect("the clone has a status row");
    assert_eq!(row.branch, "feat");
    assert_eq!(row.ahead, Some(1));
    assert_eq!(row.dirty_count, 1);
    assert!(!row.obsolete);

    let violations = untagged_in(&log, &clone);
    assert!(
        violations.is_empty(),
        "host git ran in a seat clone: {violations:#?}"
    );
}
