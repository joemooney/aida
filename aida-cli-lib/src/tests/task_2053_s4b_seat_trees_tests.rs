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
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(&clone)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "maintenance.auto=false",
                "-c",
                "gc.auto=0",
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

    // The cleanup detector also reads dirty clone state and attributes it to
    // the lease. Use the probe's branch even when it differs from the lease.
    git(&["branch", "-m", "actual-seat-branch"]);
    let unreadable = leased_clone(&fx, "unreadable-wip", "c0ffee000060");
    std::fs::rename(unreadable.join(".git"), unreadable.join("old-git")).unwrap();
    std::fs::write(unreadable.join(".git"), "gitdir: old-git\n").unwrap();
    let report =
        crate::collect_cleanup_report(&fx.main, &aida_core::models::RequirementsStore::default());
    let wip = report
        .uncommitted_wip
        .iter()
        .find(|r| r.worktree_path == clone)
        .unwrap();
    assert_eq!(wip.scope.as_deref(), Some("TASK-2053"));
    assert_eq!(wip.branch, "actual-seat-branch");
    assert_eq!(wip.modified_files, 1);
    let unknown = report
        .uncommitted_wip
        .iter()
        .find(|r| r.worktree_path == unreadable)
        .unwrap();
    assert_eq!(unknown.scope.as_deref(), Some("TASK-2053"));
    assert_eq!(unknown.branch, "feat");
    assert!(unknown.modified_files > 0);
    assert!(untagged_in(&log, &unreadable).is_empty());

    let violations = untagged_in(&log, &clone);
    assert!(
        violations.is_empty(),
        "host git ran in a seat clone: {violations:#?}"
    );
}

/// TASK-2110: after a squash merge (origin has none of the branch's commits)
/// the merged PR's head counts as shipped; a commit past it, and a shipped id
/// the clone does not have, are handled without reading every commit as
/// unpushed or failing the probe.
#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_a_merged_heads_commits_count_as_shipped() {
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let clone = leased_clone(&fx, "squash", "c0ffee000055");
    let commit = |file: &str| {
        std::fs::write(clone.join(file), "work\n").unwrap();
        for args in [vec!["add", file], vec!["commit", "--no-verify", "-m", file]] {
            let out = crate::test_git::git_command()
                .arg("-C")
                .arg(&clone)
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "maintenance.auto=false",
                    "-c",
                    "gc.auto=0",
                ])
                .args(&args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    };
    let facts = |shipped: &[String]| match probe_with_shipped(&fx.main, &clone, shipped) {
        Probe::Known(facts) => facts,
        Probe::Unknown(why) | Probe::HasSubmodules(why) => panic!("probe failed: {why}"),
    };
    commit("one.txt");
    commit("two.txt");
    let merged_head = facts(&[]).head.expect("a HEAD");
    assert_eq!(facts(&[]).unpushed, 2);
    assert_eq!(facts(&[merged_head.clone()]).unpushed, 0);
    let absent = "0".repeat(merged_head.len());
    assert_eq!(facts(&[absent, "not-an-oid".to_string()]).unpushed, 2);
    commit("three.txt");
    assert_eq!(facts(&[merged_head]).unpushed, 1);
}

// trace:TASK-2191 | ai:codex
#[test]
fn task_2053_bwrap_phase_two_verifies_origin_and_only_publishes_the_host_base() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let clone = leased_clone(&fx, "phase2", "c0ffee000059");
    let (shim_dir, log, _) = git_shim(&fx);
    let _path = crate::test_env::EnvVarGuard::prepend_path(&shim_dir);
    let gate =
        |branch| crate::orchestrator_leases::ensure_implementer_branch_pushed(&clone, branch, true);
    // A never-pushed, zero-commit branch must actually be published. The
    // host can push its base pin without executing anything from the seat.
    assert!(!fx.origin.join("refs/heads/feat").exists());
    gate("feat").unwrap();
    assert!(fx.origin.join("refs/heads/feat").exists());
    assert!(untagged_in(&log, &clone).is_empty());
    std::fs::write(clone.join("phase2.txt"), "work\n").unwrap();
    let git = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(&clone)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["add", "phase2.txt"]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        "commit",
        "-qm",
        "work",
    ]);
    // Setup is allowed to run git; only gate invocations are under inspection.
    std::fs::write(&log, "").unwrap();
    assert!(gate("feat").unwrap_err().reason.contains("unpushed"));
    assert!(untagged_in(&log, &clone).is_empty());
    // Forge a tracking ref to claim the new commit is already pushed.
    // Real origin still has the base and must not be overwritten by the host.
    git(&["update-ref", "refs/remotes/origin/feat", "HEAD"]);
    assert!(
        matches!(crate::seat_clone::probe(&fx.main, &clone), Probe::Known(f) if f.unpushed == 0)
    );
    std::fs::write(&log, "").unwrap();
    assert!(gate("feat").unwrap_err().reason.contains("not published"));
    assert!(untagged_in(&log, &clone).is_empty());
    // Even removing the real remote branch cannot make forged seat refs grant
    // a host push of seat-created objects.
    std::fs::remove_file(fx.origin.join("refs/heads/feat")).unwrap();
    assert!(gate("feat").unwrap_err().reason.contains("not published"));
    git(&["push", "-u", "origin", "feat"]);
    std::fs::write(&log, "").unwrap();
    gate("feat").unwrap();
    assert!(gate("wrong")
        .unwrap_err()
        .reason
        .contains("phase 2 is driving"));
    git(&["checkout", "--detach"]);
    std::fs::write(&log, "").unwrap();
    assert!(gate("feat").unwrap_err().reason.contains("DETACHED"));
    std::fs::write(clone.join(".gitmodules"), "submodule evidence").unwrap();
    assert!(gate("feat").unwrap_err().reason.contains("submodules"));
    std::fs::remove_file(clone.join(".gitmodules")).unwrap();
    std::fs::rename(clone.join(".git"), clone.join("old-git")).unwrap();
    std::fs::write(clone.join(".git"), "gitdir: old-git\n").unwrap();
    assert!(gate("feat").is_err());
    assert!(
        untagged_in(&log, &clone).is_empty(),
        "the host ran git in the clone"
    );
}

// trace:TASK-2053 | ai:codex
#[cfg(unix)]
#[test]
fn worktree_list_json_lists_clone_without_reading_its_focus_marker() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = fixture();
    let clone = leased_clone(&fx, "list", "c0ffee000061");
    std::fs::create_dir_all(clone.join(".aida")).unwrap();
    // This seat-written value must never become host focus context.
    std::fs::write(clone.join(".aida/focus"), "EPIC-999\n").unwrap();
    let (shim_dir, log, _) = git_shim(&fx);
    let _path = crate::test_env::EnvVarGuard::prepend_path(&shim_dir);
    let rows = crate::worktree_gc::worktree_list_rows(&fx.main).unwrap();
    let json = crate::worktree_gc::worktree_list_json(&rows);
    let row = json
        .iter()
        .find(|v| v["path"] == clone.display().to_string())
        .unwrap();
    assert_eq!(row["kind"], "clone");
    assert_eq!(row["branch"], "feat");
    assert!(row["focus"].is_null());
    assert!(untagged_in(&log, &clone).is_empty());
}

// trace:TASK-2053 | ai:codex
#[test]
fn ship_clone_skips_post_merge_pull_and_records_skipped_step() {
    let fx = fixture();
    let clone = leased_clone(&fx, "ship", "c0ffee000062");
    assert!(crate::pr_cmd::skip_seat_clone_post_merge_pull(
        &clone, &fx.main, 42
    ));
    let log = fx.main.join(".aida/advisor-activity.jsonl");
    let text = std::fs::read_to_string(&log).unwrap();
    let event: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(event["step"], "pr-pull");
    assert_eq!(event["status"], "skipped");
    assert_eq!(event["pr"], 42);
    assert!(event["detail"].as_str().unwrap().contains("seat clone"));
    assert!(!crate::pr_cmd::skip_seat_clone_post_merge_pull(
        &fx.main, &fx.main, 43
    ));
    assert_eq!(std::fs::read_to_string(log).unwrap(), text);
}

// trace:TASK-2053 | ai:codex
#[cfg(unix)]
#[test]
fn merged_pr_heads_filters_oids_and_fails_soft_on_failure_and_timeout() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = fixture();
    let bin = fx.root.join("gh-bin");
    std::fs::create_dir(&bin).unwrap();
    let gh = bin.join("gh");
    let sha1 = "a".repeat(40);
    let sha256 = "B".repeat(64);
    crate::test_exec::write_executable(
        &gh,
        format!(
            "#!/bin/sh\nprintf '%s\\n' ' {sha1} ' '{sha256}' '1234567' '{}' '{}'\n",
            "g".repeat(40),
            "c".repeat(41),
        ),
    );
    let _path = crate::test_env::EnvVarGuard::prepend_path(&bin);
    assert_eq!(merged_pr_heads(&fx.main, "feat"), vec![sha1, sha256]);
    crate::test_exec::write_executable(&gh, "#!/bin/sh\nprintf '%040d\\n' 1\nexit 1\n");
    assert!(merged_pr_heads(&fx.main, "feat").is_empty());
    // exec avoids leaving a grandchild holding stdout after the timeout kill.
    crate::test_exec::write_executable(&gh, "#!/bin/sh\nexec sleep 5\n");
    let start = std::time::Instant::now();
    assert!(
        merged_pr_heads_with_timeout(&fx.main, "feat", std::time::Duration::from_millis(50))
            .is_empty()
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
    assert!(merged_pr_heads(&fx.main, "").is_empty());
}

// trace:TASK-2053 | ai:codex
#[cfg(unix)]
#[test]
fn bounded_clone_scan_counts_skipped_clones_and_caps_inflight_probe() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = fixture();
    leased_clone(&fx, "deadline-a", "c0ffee000063");
    leased_clone_on(&fx, "deadline-b", "c0ffee000064", "other");
    let expired = Some(std::time::Instant::now());
    let (items, scan) =
        crate::collect_unshipped_work_items_bounded(&fx.main, &[], true, false, expired);
    assert!(items.is_empty());
    assert!(!scan.complete);
    assert!(scan.candidates >= 2);
    assert_eq!(scan.scanned, 0);

    let bin = fx.root.join("bwrap-bin");
    std::fs::create_dir(&bin).unwrap();
    let started = fx.root.join("probe-started");
    crate::test_exec::write_executable(
        &bin.join("bwrap"),
        format!(
            "#!/bin/sh\nprintf 'started\\n' >> '{}'\nexec sleep 5\n",
            started.display(),
        ),
    );
    let _path = crate::test_env::EnvVarGuard::prepend_path(&bin);
    let start = std::time::Instant::now();
    let (items, scan) = crate::status_prs::clone_unshipped_work_items_bounded(
        &fx.main,
        Some(start + std::time::Duration::from_millis(100)),
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
    assert!(items.is_empty());
    assert!(!scan.complete);
    assert_eq!(scan.scanned, 0);
    assert_eq!(scan.candidates, 2);
    assert_eq!(std::fs::read_to_string(started).unwrap().lines().count(), 1);
}
