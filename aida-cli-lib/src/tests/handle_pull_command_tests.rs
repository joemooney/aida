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

#[test]
fn post_pull_config_validation_quarantines_conflict_and_restores_pre_pull_config() {
    let tmp = tempfile::TempDir::new().unwrap();
    let project_root = tmp.path();
    let aida = project_root.join(".aida");
    std::fs::create_dir_all(&aida).unwrap();
    let config = aida.join("config.toml");
    let before = "[deployment]\nmode = \"distributed\"\n";
    std::fs::write(&config, before).unwrap();

    let snapshots = snapshot_known_project_configs(project_root);
    let conflicted = format!(
        "[deployment]\n{} Updated upstream\nmode = \"distributed\"\n{}\nmode = \"centralized\"\n{} Stashed changes\n",
        "<<<<<<<", "=======", ">>>>>>>"
    );
    std::fs::write(&config, &conflicted).unwrap();

    // trace:BUG-1025 | ai:codex
    let err = validate_and_restore_project_configs_after_pull(&snapshots)
        .expect_err("conflicted config must make pull fail loudly");
    let msg = format!("{err:?}");
    // trace:BUG-1021 | ai:claude — the path renders with the platform separator.
    let config_rel = std::path::Path::new(".aida").join("config.toml");
    assert!(
        msg.contains(&format!("{}:2: conflict marker", config_rel.display())),
        "error should name conflicted file and line: {msg}"
    );
    assert!(
        msg.contains("Quarantined the conflicted version"),
        "error should name quarantine step: {msg}"
    );
    assert!(
        msg.contains("Manual merge step"),
        "error should include manual merge instruction: {msg}"
    );
    assert_eq!(std::fs::read_to_string(&config).unwrap(), before);
    assert_eq!(
        std::fs::read_to_string(aida.join("config.toml.conflicted")).unwrap(),
        conflicted
    );
}

#[test]
fn store_sync_config_parse_error_names_file_line_and_fix_hint() {
    let tmp = tempfile::TempDir::new().unwrap();
    let aida = tmp.path().join(".aida");
    std::fs::create_dir_all(&aida).unwrap();
    std::fs::write(
        aida.join("config.toml"),
        "[store]\n[store.sync]\nauto_push = \"manual\"\n[broken\n",
    )
    .unwrap();

    let err = read_store_sync_config(tmp.path())
        .expect_err("invalid config TOML must be returned as a loud error");
    let msg = format!("{err:?}");
    // trace:BUG-1021 | ai:claude — the path renders with the platform separator.
    let config_rel = std::path::Path::new(".aida").join("config.toml");
    assert!(
        msg.contains(&format!("{}:4", config_rel.display())),
        "error should include config path and line: {msg}"
    );
    assert!(
        msg.contains("Fix the TOML syntax"),
        "error should include a fix hint: {msg}"
    );
}

// ----- BUG-1625: the store leg pulls BEFORE code-leg reconcile -----

/// A git-canonical store "hub" (bare, branch `aida-store`) plus the OTHER
/// machine's clone of it, seeded with one Draft spec. Temp repos only.
fn make_store_hub_with_draft_spec(
    spec_id: &str,
) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    use aida_core::db::DatabaseBackend;
    let tmp = tempfile::TempDir::new().unwrap();
    let hub = tmp.path().join("store-hub.git");
    std::fs::create_dir_all(&hub).unwrap();
    run_git_in(
        &hub,
        &["init", "--bare", "--initial-branch=aida-store", "--quiet"],
    );
    let other = tmp.path().join("other-machine-store");
    std::fs::create_dir_all(&other).unwrap();
    run_git_in(&other, &["init", "--initial-branch=aida-store", "--quiet"]);
    run_git_in(&other, &["config", "user.email", "test@example.com"]);
    run_git_in(&other, &["config", "user.name", "Test"]);
    run_git_in(&other, &["remote", "add", "origin", hub.to_str().unwrap()]);
    let backend = aida_core::db::GitBackend::new(&other).unwrap();
    let mut store = aida_core::RequirementsStore::default();
    let mut req = aida_core::Requirement::new(format!("test-{spec_id}"), String::new());
    req.spec_id = Some(spec_id.to_string());
    req.set_status_from_str("Draft");
    store.requirements.push(req);
    backend.save(&store).unwrap();
    run_git_in(&other, &["push", "--quiet", "origin", "aida-store"]);
    (tmp, hub, other)
}

fn clone_store_into(hub: &std::path::Path, dest: &std::path::Path) {
    let parent = dest.parent().unwrap();
    run_git_in(
        parent,
        &[
            "clone",
            "--quiet",
            "--branch",
            "aida-store",
            hub.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
    );
    run_git_in(dest, &["config", "user.email", "test@example.com"]);
    run_git_in(dest, &["config", "user.name", "Test"]);
}

/// BUG-1625 acceptance: the remote store already has the spec Completed, the
/// local store is stale (Draft), and the code pull brings in a commit that
/// names the spec. Before the fix the code leg auto-bumped the STALE local
/// Draft → Done first, and the later store merge picked the newer Done over
/// the remote Completed. After `aida pull` the spec must still be Completed
/// and no Draft→Done transition may have been written.
// trace:BUG-1625 | ai:claude
#[test]
fn bug1625_pull_syncs_store_before_code_reconcile_and_keeps_completed() {
    use aida_core::db::DatabaseBackend;
    let spec_id = "BUG-9725";
    let (_code_bare_tmp, _proj_tmp, code_bare, project_root) = make_remote_and_clone();
    let (_store_tmp, hub, other) = make_store_hub_with_draft_spec(spec_id);

    // Local store: a clone taken while the spec was still Draft (stale).
    let store_path = project_root.join(".aida-store");
    clone_store_into(&hub, &store_path);

    // The other machine completes the spec and publishes it.
    let other_backend = aida_core::db::GitBackend::new(&other).unwrap();
    let mut r = other_backend
        .get_requirement_by_spec_id(spec_id)
        .unwrap()
        .unwrap();
    let prior = r.status.clone();
    r.set_status_from_str("Completed");
    r.record_change(
        "aida-auto-bump".to_string(),
        vec![aida_core::Requirement::field_change(
            "status",
            format!("{prior:?}"),
            format!("{:?}", r.status),
        )],
    );
    other_backend.update_requirement(&r).unwrap();
    run_git_in(&other, &["push", "--quiet", "origin", "aida-store"]);

    // The code remote gains a trailered commit naming the spec.
    push_remote_commit_referencing(&code_bare, spec_id);

    handle_pull_command(&store_path, false, false, true, true, false).unwrap();

    let after = Storage::new(store_path.clone()).load().unwrap();
    let req = after.get_requirement_by_spec_id(spec_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Completed),
        "BUG-1625: {spec_id} must stay Completed after `aida pull`, was {:?}",
        req.status
    );
    let regressed = req.history.iter().any(|h| {
        h.changes
            .iter()
            .any(|c| c.field_name == "status" && c.new_value == "Done")
    });
    assert!(
        !regressed,
        "BUG-1625: no Draft→Done transition may be written against a stale store: {:?}",
        req.history
    );
    // The fresh store is committed; the code leg left nothing to publish.
    assert_eq!(
        run_git_in(&store_path, &["status", "--porcelain"]),
        "",
        "store worktree should be clean"
    );
}

/// BUG-1625: when the store leg fails, the code-derived reconcile must stay
/// unapplied (the local store may be stale) — a Done spec named by the pulled
/// commit is NOT bumped, and pull reports the failure.
// trace:BUG-1625 | ai:claude
#[test]
fn bug1625_store_pull_failure_leaves_code_reconcile_unapplied() {
    use aida_core::db::DatabaseBackend;
    let spec_id = "BUG-9726";
    let (_code_bare_tmp, _proj_tmp, code_bare, project_root) = make_remote_and_clone();

    let store_path = project_root.join(".aida-store");
    std::fs::create_dir_all(&store_path).unwrap();
    run_git_in(
        &store_path,
        &["init", "--initial-branch=aida-store", "--quiet"],
    );
    run_git_in(&store_path, &["config", "user.email", "test@example.com"]);
    run_git_in(&store_path, &["config", "user.name", "Test"]);
    // An origin that cannot be reached: the store pull fails.
    let missing = project_root.join("no-such-store-hub.git");
    run_git_in(
        &store_path,
        &["remote", "add", "origin", missing.to_str().unwrap()],
    );
    let backend = aida_core::db::GitBackend::new(&store_path).unwrap();
    let mut store = aida_core::RequirementsStore::default();
    let mut req = aida_core::Requirement::new(format!("test-{spec_id}"), String::new());
    req.spec_id = Some(spec_id.to_string());
    req.set_status_from_str("Done");
    store.requirements.push(req);
    backend.save(&store).unwrap();

    push_remote_commit_referencing(&code_bare, spec_id);

    let result = handle_pull_command(&store_path, false, false, true, true, false);
    assert!(result.is_err(), "a failed store leg must fail the pull");

    let after = Storage::new(store_path.clone()).load().unwrap();
    let req = after.get_requirement_by_spec_id(spec_id).unwrap();
    assert!(
        matches!(req.status, RequirementStatus::Done),
        "BUG-1625: reconcile must not run when the store did not sync, was {:?}",
        req.status
    );
}

/// BUG-1625: a followup that already exists in the (freshly pulled) store —
/// as a child of the parent, or tagged with followup provenance — is
/// recognised as already filed rather than reported as declined.
// trace:BUG-1625 | ai:claude
#[test]
fn bug1625_followup_filed_in_store_recognises_existing_followups() {
    let mut parent = aida_core::Requirement::new("parent".to_string(), String::new());
    parent.spec_id = Some("BUG-9727".to_string());
    let mut child = aida_core::Requirement::new("Tighten the widget".to_string(), String::new());
    child.spec_id = Some("TASK-9728".to_string());
    parent.relationships.push(aida_core::Relationship {
        rel_type: aida_core::RelationshipType::Parent,
        target_id: child.id,
        created_at: None,
        created_by: None,
    });
    let mut tagged = aida_core::Requirement::new("Document the knob".to_string(), String::new());
    tagged.spec_id = Some("TASK-9729".to_string());
    tagged
        .tags
        .insert(format!("{FOLLOWUP_SRC_TAG_PREFIX}docs/plans/x.md"));
    let unrelated = aida_core::Requirement::new("Unrelated work".to_string(), String::new());
    let store = aida_core::RequirementsStore {
        requirements: vec![parent, child, tagged, unrelated],
        ..Default::default()
    };
    let plan = "docs/plans/x.md";
    assert!(followup_filed_in_store(
        &store,
        "BUG-9727",
        "  tighten the WIDGET ",
        plan
    ));
    assert!(followup_filed_in_store(
        &store,
        "BUG-9727",
        "Document the knob",
        plan
    ));
    assert!(!followup_filed_in_store(
        &store,
        "BUG-9727",
        "Unrelated work",
        plan
    ));
    assert!(!followup_filed_in_store(
        &store,
        "BUG-9727",
        "Never filed",
        plan
    ));
    // BUG-1633: a same-titled spec filed from ANOTHER plan is not this
    // plan's followup. trace:BUG-1633 | ai:claude
    assert!(!followup_filed_in_store(
        &store,
        "BUG-9727",
        "Document the knob",
        "docs/plans/other.md"
    ));
}
