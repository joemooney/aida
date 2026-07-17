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

/// Bare remote + a project clone with one initial commit. Mirrors the
/// `handle_pull_command_tests::make_remote_and_clone` helper so tests
/// stay symmetric across the pull/fetch pair.
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

fn push_remote_commit(bare: &std::path::Path, msg: &str) -> String {
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
    std::fs::write(work.join("landed.txt"), msg).unwrap();
    run_git_in(&work, &["add", "landed.txt"]);
    run_git_in(&work, &["commit", "-m", msg]);
    run_git_in(&work, &["push", "origin", "main", "--quiet"]);
    run_git_in(&work, &["rev-parse", "HEAD"])
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

/// `aida fetch --code-only` advances `origin/main` to the remote tip
/// but does NOT advance the local working `main` (no merge, no
/// rebase). The worktree file added in the remote commit must NOT
/// appear in the local working tree.
#[test]
fn fetch_code_only_advances_origin_without_touching_worktree() {
    let (_bare_tmp, _proj_tmp, bare, project_root) = make_remote_and_clone();
    let store_path = project_root.join("requirements.yaml");
    Storage::new(store_path.clone())
        .save(&aida_core::RequirementsStore::default())
        .unwrap();

    let pre_origin = run_git_in(&project_root, &["rev-parse", "origin/main"]);
    let pre_local = run_git_in(&project_root, &["rev-parse", "HEAD"]);

    let new_sha = push_remote_commit(&bare, "feat: remote landed");

    // store_path doesn't exist as a worktree — store leg will skip
    // cleanly. code_only=true makes that skip the intended path.
    handle_fetch_command(&store_path, true, false, true).unwrap();

    let post_origin = run_git_in(&project_root, &["rev-parse", "origin/main"]);
    let post_local = run_git_in(&project_root, &["rev-parse", "HEAD"]);

    assert_eq!(
        post_origin, new_sha,
        "origin/main should advance after fetch"
    );
    assert_ne!(post_origin, pre_origin, "origin/main must have moved");
    assert_eq!(post_local, pre_local, "local HEAD must NOT move on fetch");
    assert!(
        !project_root.join("landed.txt").exists(),
        "fetch must not touch the worktree"
    );
}

/// `aida fetch` MUST NOT trigger the Done → Completed auto-bump.
/// That's pull-only behavior — fetch is read-only. Regression guard
/// against accidentally inheriting auto-bump from a shared helper.
#[test]
fn fetch_does_not_auto_bump() {
    let (_bare_tmp, _proj_tmp, bare, project_root) = make_remote_and_clone();
    let store_path = project_root.join("requirements.yaml");
    let storage = Storage::new(store_path.clone());
    storage
        .save(&aida_core::RequirementsStore::default())
        .unwrap();

    let spec_id = "STORY-9510".to_string();
    seed_done_spec_at(&store_path, &spec_id);
    push_remote_commit(&bare, &format!("feat: teammate work ({})", spec_id));

    handle_fetch_command(&store_path, true, false, true).unwrap();

    let after = storage.load().unwrap();
    let req = after.get_requirement_by_spec_id(&spec_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Done),
        "fetch must NOT auto-bump (that's pull's job), status was {:?}",
        req.status
    );
}

/// `aida fetch --store-only` skips the code leg even when there are
/// new commits on origin/main. origin/main stays at its pre-fetch SHA.
#[test]
fn fetch_store_only_skips_code_leg() {
    let (_bare_tmp, _proj_tmp, bare, project_root) = make_remote_and_clone();
    let store_path = project_root.join("requirements.yaml");
    Storage::new(store_path.clone())
        .save(&aida_core::RequirementsStore::default())
        .unwrap();

    let pre_origin = run_git_in(&project_root, &["rev-parse", "origin/main"]);
    push_remote_commit(&bare, "feat: new on remote");

    // store_only=true → code leg skipped entirely.
    handle_fetch_command(&store_path, false, true, true).unwrap();

    let post_origin = run_git_in(&project_root, &["rev-parse", "origin/main"]);
    assert_eq!(
        pre_origin, post_origin,
        "--store-only must NOT fetch the code leg"
    );
}
