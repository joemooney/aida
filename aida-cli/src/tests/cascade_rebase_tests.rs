//! STORY-248: cascade integration tests. Build a real git repo with
//! a stacked-branch scenario, write a fake lease + stacks.json, run
//! the cascade, assert the rebase landed. Skipped silently when
//! `git` is too old for `worktree add` semantics we need.
//! trace:STORY-248 | ai:claude
use super::*;

fn run_git(repo: &std::path::Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git spawn")
}

fn pin_identity(root: &std::path::Path) {
    for args in [
        vec!["config", "user.email", "t@x.example"],
        vec!["config", "user.name", "t"],
        vec!["config", "commit.gpgsign", "false"],
        vec!["config", "tag.gpgsign", "false"],
    ] {
        assert!(
            run_git(root, &args).status.success(),
            "git config {args:?} failed"
        );
    }
}

fn init_repo_with_remote() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    // Layout: <tmp>/origin (bare) + <tmp>/work (working repo).
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin.git");
    let work = tmp.path().join("work");

    assert!(std::process::Command::new("git")
        .args(["init", "-q", "--bare", "-b", "main"])
        .arg(&origin)
        .status()
        .unwrap()
        .success());

    assert!(std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .arg(&work)
        .status()
        .unwrap()
        .success());
    pin_identity(&work);

    std::fs::write(work.join("README.md"), b"hello\n").unwrap();
    assert!(run_git(&work, &["add", "README.md"]).status.success());
    assert!(run_git(&work, &["commit", "-qm", "init"]).status.success());
    assert!(run_git(
        &work,
        &["remote", "add", "origin", origin.to_str().unwrap()]
    )
    .status
    .success());
    assert!(run_git(&work, &["push", "-q", "-u", "origin", "main"])
        .status
        .success());

    (tmp, origin, work)
}

/// Helper: drop a SessionLease toml so `list_leases` finds a
/// pseudo-session pointing at the given worktree + branch.
fn write_fake_lease(project_root: &std::path::Path, branch: &str, worktree: &std::path::Path) {
    let leases = project_root.join(".aida").join("sessions");
    std::fs::create_dir_all(&leases).unwrap();
    let lease = SessionLease {
        id: format!("lease-{branch}"),
        scope: branch.to_string(),
        slug: branch.to_string(),
        owner: "t".into(),
        worktree_path: worktree.to_path_buf(),
        branch: branch.to_string(),
        started_at: chrono::Utc::now(),
        hostname: "h".into(),
        role: Some("implementer".into()),
        creator_pid: None,
        cargo_target_dir: None,
        parent_project_root: Some(project_root.to_path_buf()),
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: false,
        claim_verb: false,
    };
    std::fs::write(
        leases.join(format!("{}.toml", lease.id)),
        toml::to_string_pretty(&lease).unwrap(),
    )
    .unwrap();
}

#[test]
fn cascade_skips_when_no_stacked_branches() {
    let tmp = tempfile::tempdir().unwrap();
    // Empty .aida/, no stacks.json — cascade should no-op.
    cascade_rebase_stacked_branches(tmp.path(), true).unwrap();
}

#[test]
fn cascade_skips_entry_whose_parent_is_default_branch() {
    // task-y whose parent is `main` is not a "stacked behind another
    // branch" case; it's just a working branch. The cascade must
    // leave it alone even when --auto is set.
    let (_tmp, _origin, work) = init_repo_with_remote();
    let mut graph = stacks::StackGraph::default();
    stacks::add(
        &mut graph,
        stacks::StackEntry {
            branch: "task-y".into(),
            parent_branch: "main".into(),
            parent_branch_sha: "00".into(),
            spec_id: None,
            created_at: chrono::Utc::now(),
        },
    );
    stacks::save(&work, &graph).unwrap();
    cascade_rebase_stacked_branches(&work, true).unwrap();
    // Entry still there (untouched).
    let reloaded = stacks::load(&work);
    assert!(reloaded.get("task-y").is_some());
}

#[test]
fn cascade_rebases_clean_chain_onto_main_and_removes_entry() {
    // Setup:
    //   main:   init  → A   (pushed)
    //   task-x: init  → A → B (forked from A, then `git push origin
    //                          --delete task-x` simulates squash-merge)
    //   task-y: init  → A → B → C (stacked on task-x)
    //   New commit on main: D
    //   After cascade: task-y rebased --onto origin/main <B-sha> task-y
    //   → main D + C; stacks.json entry for task-y removed.
    let (_tmp, _origin, work) = init_repo_with_remote();

    // Commit A on main (already there).
    let a_sha = aida_core::git_ops::head_sha(&work).unwrap();
    let _ = a_sha;

    // Branch task-x, commit B (the would-be-merged content).
    assert!(run_git(&work, &["checkout", "-q", "-b", "task-x"])
        .status
        .success());
    std::fs::write(work.join("x.txt"), b"x\n").unwrap();
    assert!(run_git(&work, &["add", "x.txt"]).status.success());
    assert!(run_git(&work, &["commit", "-qm", "task-x: add x"])
        .status
        .success());
    let task_x_sha = aida_core::git_ops::head_sha(&work).unwrap();

    // Branch task-y off task-x, commit C.
    assert!(run_git(&work, &["checkout", "-q", "-b", "task-y"])
        .status
        .success());
    std::fs::write(work.join("y.txt"), b"y\n").unwrap();
    assert!(run_git(&work, &["add", "y.txt"]).status.success());
    assert!(run_git(&work, &["commit", "-qm", "task-y: add y"])
        .status
        .success());

    // Simulate the squash-merge: main gets a new commit with x.txt's
    // content + a new file (D), then task-x is deleted both locally
    // and from origin. We never pushed task-x — so deleting it
    // locally is enough.
    assert!(run_git(&work, &["checkout", "-q", "main"]).status.success());
    std::fs::write(work.join("x.txt"), b"x\n").unwrap();
    std::fs::write(work.join("d.txt"), b"d\n").unwrap();
    assert!(run_git(&work, &["add", "x.txt", "d.txt"]).status.success());
    assert!(
        run_git(&work, &["commit", "-qm", "squash-merge task-x + D"])
            .status
            .success()
    );
    assert!(run_git(&work, &["push", "-q", "origin", "main"])
        .status
        .success());
    // Remove task-x (-D because it's "not merged" by git's lights —
    // squash-merge breaks the ancestry check).
    assert!(run_git(&work, &["branch", "-q", "-D", "task-x"])
        .status
        .success());

    // Record task-y in stacks.json + write a fake lease pointing at `work`.
    let mut graph = stacks::StackGraph::default();
    stacks::add(
        &mut graph,
        stacks::StackEntry {
            branch: "task-y".into(),
            parent_branch: "task-x".into(),
            parent_branch_sha: task_x_sha.clone(),
            spec_id: Some("TASK-Y".into()),
            created_at: chrono::Utc::now(),
        },
    );
    stacks::save(&work, &graph).unwrap();
    write_fake_lease(&work, "task-y", &work);

    // Run the cascade with --auto. It should classify behind-only
    // (clean rebase) and run the rebase successfully.
    cascade_rebase_stacked_branches(&work, true).unwrap();

    // Post-conditions:
    // 1. The stacks.json entry for task-y is gone (it was rebased
    //    onto main and is no longer stacked).
    let reloaded = stacks::load(&work);
    assert!(
        reloaded.get("task-y").is_none(),
        "task-y should be removed from stacks.json after successful rebase, graph={:?}",
        reloaded.entries
    );
    // 2. task-y now contains the post-rebase commit (y.txt) AND the
    //    new d.txt that landed on main. Switch to it and check.
    assert!(run_git(&work, &["checkout", "-q", "task-y"])
        .status
        .success());
    assert!(work.join("y.txt").exists(), "y.txt missing after rebase");
    assert!(
        work.join("d.txt").exists(),
        "d.txt (from main) missing after rebase"
    );
}
