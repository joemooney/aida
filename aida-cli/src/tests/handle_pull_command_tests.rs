use super::*;

fn run_git_in(repo: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git binary on PATH");
    if !out.status.success() {
        panic!(
            "git {:?} (in {:?}) failed: stdout={} stderr={}",
            args,
            repo,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Build a bare "remote" repo that already has one initial commit so
/// `--ff-only` has a valid base, plus a "clone" project repo
/// configured with `origin` pointing at it. Returns the temp guards
/// plus the bare-repo path and the project path.
fn make_remote_and_clone() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let bare_tmp = tempfile::TempDir::new().unwrap();
    let bare = bare_tmp.path().to_path_buf();
    run_git_in(
        &bare,
        &["init", "--bare", "--initial-branch=main", "--quiet"],
    );

    // Seed the bare repo via a temp clone, then drop it; the project
    // clone below will fetch this initial commit.
    let seed_tmp = tempfile::TempDir::new().unwrap();
    let seed = seed_tmp.path().to_path_buf();
    run_git_in(
        &seed,
        &[
            "clone",
            "--quiet",
            bare.to_str().unwrap(),
            seed.to_str().unwrap(),
        ],
    );
    run_git_in(&seed, &["config", "user.email", "test@example.com"]);
    run_git_in(&seed, &["config", "user.name", "Test"]);
    std::fs::write(seed.join("README.md"), "init\n").unwrap();
    run_git_in(&seed, &["add", "README.md"]);
    run_git_in(&seed, &["commit", "-m", "chore: init"]);
    run_git_in(&seed, &["push", "origin", "main", "--quiet"]);

    // Now make the actual project clone the test will operate on.
    let proj_tmp = tempfile::TempDir::new().unwrap();
    let proj = proj_tmp.path().to_path_buf();
    run_git_in(
        &proj,
        &[
            "clone",
            "--quiet",
            bare.to_str().unwrap(),
            proj.to_str().unwrap(),
        ],
    );
    run_git_in(&proj, &["config", "user.email", "test@example.com"]);
    run_git_in(&proj, &["config", "user.name", "Test"]);

    (bare_tmp, proj_tmp, bare, proj)
}

/// Push a new commit referencing `spec_id` to the bare remote so a
/// subsequent `git pull --ff-only` from the project clone fast-forwards
/// to it. Done via a fresh temp clone to avoid touching the project
/// repo under test.
fn push_remote_commit_referencing(bare: &std::path::Path, spec_id: &str) {
    let tmp = tempfile::TempDir::new().unwrap();
    let work = tmp.path().to_path_buf();
    run_git_in(
        &work,
        &[
            "clone",
            "--quiet",
            bare.to_str().unwrap(),
            work.to_str().unwrap(),
        ],
    );
    run_git_in(&work, &["config", "user.email", "test@example.com"]);
    run_git_in(&work, &["config", "user.name", "Test"]);
    std::fs::write(work.join("landed.txt"), "x\n").unwrap();
    run_git_in(&work, &["add", "landed.txt"]);
    run_git_in(
        &work,
        &[
            "commit",
            "-m",
            &format!("feat: teammate work ({})", spec_id),
        ],
    );
    run_git_in(&work, &["push", "origin", "main", "--quiet"]);
}

/// BUG-254: like `push_remote_commit_referencing`, but lets the test
/// choose the filename + content so it can set up an untracked-file
// conflict on the local clone. trace:BUG-254 | ai:claude
fn push_remote_file(bare: &std::path::Path, name: &str, content: &str, spec_id: &str) {
    let tmp = tempfile::TempDir::new().unwrap();
    let work = tmp.path().to_path_buf();
    run_git_in(
        &work,
        &[
            "clone",
            "--quiet",
            bare.to_str().unwrap(),
            work.to_str().unwrap(),
        ],
    );
    run_git_in(&work, &["config", "user.email", "test@example.com"]);
    run_git_in(&work, &["config", "user.name", "Test"]);
    std::fs::write(work.join(name), content).unwrap();
    run_git_in(&work, &["add", name]);
    run_git_in(
        &work,
        &["commit", "-m", &format!("feat: add {} ({})", name, spec_id)],
    );
    run_git_in(&work, &["push", "origin", "main", "--quiet"]);
}

fn seed_done_spec_at(store_path: &std::path::Path, spec_id: &str) {
    let storage = Storage::new(store_path);
    let mut store = storage.load().unwrap_or_default();
    let mut req = aida_core::Requirement::new(format!("test-{}", spec_id), String::new());
    req.spec_id = Some(spec_id.to_string());
    req.set_status_from_str("Done");
    store.requirements.push(req);
    storage.save(&store).unwrap();
}

/// BUG-95: `aida pull --code-only` (code_only=true, store_only=false)
/// must trigger the auto-bump on the just-pulled commits. Regression
/// guard against the bug-as-filed (claimed the conditional was in the
// wrong block). trace:BUG-95 | ai:claude
#[test]
fn pull_code_only_triggers_auto_bump() {
    let (_bare_tmp, _proj_tmp, bare, project_root) = make_remote_and_clone();
    let store_path = project_root.join("requirements.yaml");
    let storage = Storage::new(store_path.clone());
    storage
        .save(&aida_core::RequirementsStore::default())
        .unwrap();

    let spec_id = "STORY-9501".to_string();
    seed_done_spec_at(&store_path, &spec_id);

    // Remote gets the merge; our local doesn't have it yet.
    push_remote_commit_referencing(&bare, &spec_id);

    // The actual call under test: code_only=true skips the store
    // leg entirely, but the code leg + auto-bump must still run.
    handle_pull_command(&store_path, true, false, true, true, false).unwrap();

    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id(&spec_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Completed),
        "BUG-95: --code-only should have auto-bumped {} to Completed, was {:?}",
        spec_id,
        req.status
    );
}

/// BUG-404: when local main was already advanced to the merge commit
/// before the pull runs — exactly what `aida pr ship` step 3
/// (`gh pr merge --squash`) does before step 4's pull — the pull is a
/// no-op ("Already up to date") and the narrow `pre..HEAD` range is
/// empty, so the merged spec never bumped (it stayed Done until a manual
/// `reconcile-status`). The fix: a no-op pull falls back to the wide
/// scan, so a main advanced outside this pull is still covered.
// trace:BUG-404 | ai:claude
#[test]
fn pull_noop_still_auto_bumps_externally_advanced_main() {
    let (_bare_tmp, _proj_tmp, bare, project_root) = make_remote_and_clone();
    let store_path = project_root.join("requirements.yaml");
    let storage = Storage::new(store_path.clone());
    storage
        .save(&aida_core::RequirementsStore::default())
        .unwrap();

    let spec_id = "STORY-9601".to_string();
    seed_done_spec_at(&store_path, &spec_id);

    // The merge lands on the remote...
    push_remote_commit_referencing(&bare, &spec_id);
    // ...and local main is fast-forwarded to it via a RAW git pull (no
    // auto-bump) — standing in for `gh pr merge` advancing local main.
    // The spec is still Done at this point.
    run_git_in(&project_root, &["pull", "--ff-only", "origin", "main"]);

    // Now the command under test: its `git pull` is a no-op
    // ("Already up to date"), so the narrow range is empty. The BUG-404
    // fallback must still scan and bump.
    handle_pull_command(&store_path, true, false, true, true, false).unwrap();

    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id(&spec_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Completed),
        "BUG-404: a no-op pull over an externally-advanced main should still \
             auto-bump {} to Completed, was {:?}",
        spec_id,
        req.status
    );
}

/// BUG-254: when the code-leg `git pull --ff-only` fails (here: an
/// untracked file would be overwritten by the merge), `handle_pull_command`
/// must return Err so `aida pull` exits non-zero — the orchestrator's
/// phase 5 then halts instead of falsely announcing `phase 5 complete`
/// over a stale tree (which used to break phase 6 with confusing
// missing-file errors). trace:BUG-254 | ai:claude
#[test]
fn pull_code_leg_failure_returns_err() {
    let (_bare_tmp, _proj_tmp, bare, project_root) = make_remote_and_clone();
    let store_path = project_root.join("requirements.yaml");
    let storage = Storage::new(store_path.clone());
    storage
        .save(&aida_core::RequirementsStore::default())
        .unwrap();

    // Remote adds `conflict.txt`. Locally we leave an untracked
    // `conflict.txt` with different content — `git pull --ff-only`
    // refuses (`The following untracked working tree files would be
    // overwritten by merge`).
    let spec_id = "STORY-99254".to_string();
    seed_done_spec_at(&store_path, &spec_id);
    push_remote_file(&bare, "conflict.txt", "from remote\n", &spec_id);
    std::fs::write(project_root.join("conflict.txt"), "from local untracked\n").unwrap();

    // code_only=true isolates the code leg so the test can't be
    // rescued by a successful store leg masking the code failure.
    let result = handle_pull_command(&store_path, true, false, true, true, false);
    let err = result
        .expect_err("BUG-254: handle_pull_command must return Err when code-leg ff-only fails");
    let msg = format!("{err}");
    assert!(
        msg.contains("code leg"),
        "BUG-254: error message should name the code leg, got: {msg}"
    );

    // Defensive: the Done spec must NOT have been auto-bumped — the
    // commit referencing it never landed locally.
    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id(&spec_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Done),
        "BUG-254: failed code leg must not auto-bump; status was {:?}",
        req.status
    );
}

/// BUG-476: when the code leg fails AND the store pull is skipped (no
/// orphan worktree / no `origin` — a code-only clone or not-yet-attached
/// store, common in CI), `handle_pull_command` must STILL return Err.
/// The store-pull block's early returns used to fire AFTER `code_failed`
/// was set but BEFORE the bottom BUG-254 check, so a failed code leg +
/// skipped store leg returned Ok(()) → exit 0 over a stale tree, and the
/// orchestrator's phase 5 falsely announced success. Here the store path
/// is a plain `requirements.yaml` file (not a git repo), so the first
/// early return is taken — with `code_only=false` so the store block
// actually runs. trace:BUG-476 | ai:claude
#[test]
fn pull_code_leg_failure_with_store_skipped_returns_err() {
    let (_bare_tmp, _proj_tmp, bare, project_root) = make_remote_and_clone();
    let store_path = project_root.join("requirements.yaml");
    let storage = Storage::new(store_path.clone());
    storage
        .save(&aida_core::RequirementsStore::default())
        .unwrap();

    // Force the code-leg `git pull --ff-only` to fail (untracked-file
    // conflict), exactly like the BUG-254 test.
    let spec_id = "STORY-99476".to_string();
    seed_done_spec_at(&store_path, &spec_id);
    push_remote_file(&bare, "conflict.txt", "from remote\n", &spec_id);
    std::fs::write(project_root.join("conflict.txt"), "from local untracked\n").unwrap();

    // code_only=false → the store-pull block runs. `store_path` is a
    // plain file, not a git repo, so `git_ops::is_git_repo` is false and
    // the "no orphan worktree — skipping store pull" early return is hit.
    // Pre-fix that early return swallowed the code failure and returned
    // Ok(()). Post-fix it must bail with the code-leg error.
    let result = handle_pull_command(&store_path, false, false, true, true, false);
    let err = result.expect_err(
        "BUG-476: handle_pull_command must return Err when the code leg fails and the \
             store pull is skipped, not Ok over a stale tree",
    );
    let msg = format!("{err}");
    assert!(
        msg.contains("code leg"),
        "BUG-476: error message should name the code leg, got: {msg}"
    );

    // Defensive: the Done spec must NOT have been auto-bumped.
    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id(&spec_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Done),
        "BUG-476: failed code leg must not auto-bump; status was {:?}",
        req.status
    );
}

/// BUG-95 mirror: `aida pull --store-only` should NOT touch the code
/// branch (no code pull happens), so the auto-bump correctly does
/// not fire — even if there's a Done spec that a separate code pull
/// would have bumped. Defensive guard against an over-broad fix.
// trace:BUG-95 | ai:claude
#[test]
fn pull_store_only_does_not_run_auto_bump() {
    let (_bare_tmp, _proj_tmp, _bare, project_root) = make_remote_and_clone();
    let store_path = project_root.join("requirements.yaml");
    let storage = Storage::new(store_path.clone());
    storage
        .save(&aida_core::RequirementsStore::default())
        .unwrap();

    // Lay down a local code commit that references the Done spec
    // (simulating a state where the code is already at the merge —
    // but `--store-only` shouldn't scan it).
    let spec_id = "STORY-9502".to_string();
    seed_done_spec_at(&store_path, &spec_id);
    std::fs::write(project_root.join("local.txt"), "x\n").unwrap();
    run_git_in(&project_root, &["add", "local.txt"]);
    run_git_in(
        &project_root,
        &["commit", "-m", &format!("feat: x ({})", spec_id)],
    );

    // store_only=true; no orphan store configured → store-pull branch
    // prints a note + returns Ok(()). Code leg is skipped → no auto-bump.
    handle_pull_command(&store_path, false, true, true, true, false).unwrap();

    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id(&spec_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Done),
        "--store-only should NOT have auto-bumped, status was {:?}",
        req.status
    );
}
