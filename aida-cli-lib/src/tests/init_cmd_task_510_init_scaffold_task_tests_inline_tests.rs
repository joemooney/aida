use super::*;
use aida_core::{RequirementPriority, RequirementStatus, RequirementType, Storage};
use tempfile::TempDir;

#[cfg(unix)]
fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

#[cfg(unix)]
fn make_non_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o644);
    std::fs::set_permissions(path, perms).unwrap();
}

#[test]
fn plan_template_scaffold_is_lazy_and_idempotent() {
    let tmp = TempDir::new().unwrap();
    let plans_dir = tmp.path().join("docs/plans");

    assert!(!tmp.path().join("docs").exists());

    ensure_plan_template_scaffold(&plans_dir, false).unwrap();
    let template = plans_dir.join("_TEMPLATE.md");
    assert!(template.exists());
    let original = std::fs::read_to_string(&template).unwrap();
    assert!(original.contains("AIDA plan template"));

    std::fs::write(&template, "custom\n").unwrap();
    ensure_plan_template_scaffold(&plans_dir, false).unwrap();
    assert_eq!(std::fs::read_to_string(&template).unwrap(), "custom\n");

    ensure_plan_template_scaffold(&plans_dir, true).unwrap();
    assert_eq!(std::fs::read_to_string(&template).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn post_init_hooks_run_executable_sh_files_in_lexical_order_with_context() {
    use std::io::Write;

    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("project");
    let hooks = tmp.path().join("home/.aida/init.d");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&hooks).unwrap();

    let first = hooks.join("00-first.sh");
    let mut first_file = std::fs::File::create(&first).unwrap();
    first_file
        .write_all(b"#!/bin/sh\nprintf '1:%s:%s:%s:%s:%s:%s\\n' \"$PWD\" \"$AIDA_INIT_PROJECT_ROOT\" \"$AIDA_INIT_PROJECT_NAME\" \"$AIDA_INIT_LANG\" \"$AIDA_INIT_REMOTE_URL\" \"$AIDA_INIT_FORGE\" >> \"$AIDA_INIT_PROJECT_ROOT/order.txt\"\n")
        .unwrap();
    drop(first_file);
    make_executable(&first);

    let second = hooks.join("10-second.sh");
    let mut second_file = std::fs::File::create(&second).unwrap();
    second_file
        .write_all(b"#!/bin/sh\nprintf '2\\n' >> \"$AIDA_INIT_PROJECT_ROOT/order.txt\"\n")
        .unwrap();
    drop(second_file);
    make_executable(&second);

    let ignored = hooks.join("05-ignored.sh");
    std::fs::write(
        &ignored,
        "#!/bin/sh\nprintf 'ignored\\n' >> \"$AIDA_INIT_PROJECT_ROOT/order.txt\"\n",
    )
    .unwrap();
    make_non_executable(&ignored);
    let ignored_ext = hooks.join("06-ignored.txt");
    std::fs::write(
        &ignored_ext,
        "#!/bin/sh\nprintf 'ignored\\n' >> \"$AIDA_INIT_PROJECT_ROOT/order.txt\"\n",
    )
    .unwrap();
    make_executable(&ignored_ext);

    // trace:BUG-1165 | ai:codex
    // Keep this CI-flaky fixture hermetic: the hook set and mode bits must
    // be fully determined before the runner starts filtering executable .sh files.
    let mut fixture = std::fs::read_dir(&hooks)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                is_executable_file(&path),
            )
        })
        .collect::<Vec<_>>();
    fixture.sort();
    assert_eq!(
        fixture,
        vec![
            ("00-first.sh".to_string(), true),
            ("05-ignored.sh".to_string(), false),
            ("06-ignored.txt".to_string(), true),
            ("10-second.sh".to_string(), true),
        ]
    );

    let context = PostInitHookContext {
        project_root: root.clone(),
        project_name: "demo".to_string(),
        lang: "rust".to_string(),
        remote_url: "git@github.com:org/demo.git".to_string(),
        forge: "github".to_string(),
    };
    run_post_init_hooks_from_dir(&context, &hooks).unwrap();

    let out = std::fs::read_to_string(root.join("order.txt")).unwrap();
    let mut lines = out.lines();
    let first_line = lines.next().unwrap();
    let canonical_root = root.canonicalize().unwrap();
    // trace:BUG-1021 | ai:claude
    // macOS: the hook's `$PWD` is the realpath (`/private/var/...`) while
    // the env var carries the path as passed (`/var/...`). Both must
    // resolve to the project, so compare canonicalized, not textually.
    let mut fields = first_line.splitn(4, ':');
    assert_eq!(fields.next(), Some("1"), "{first_line}");
    for label in ["cwd", "AIDA_INIT_PROJECT_ROOT"] {
        let raw = fields.next().unwrap_or_default();
        let resolved = std::path::Path::new(raw).canonicalize().ok();
        assert_eq!(
            resolved.as_deref(),
            Some(canonical_root.as_path()),
            "{label} must point at the project: {first_line}"
        );
    }
    assert!(
        first_line.ends_with(":demo:rust:git@github.com:org/demo.git:github"),
        "hook context env should be complete: {first_line}"
    );
    assert_eq!(lines.next(), Some("2"));
    assert_eq!(lines.next(), None);
}

#[cfg(unix)]
#[test]
fn post_init_hook_failure_continues_to_later_hooks() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("project");
    let hooks = tmp.path().join("home/.aida/init.d");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&hooks).unwrap();

    let bad = hooks.join("00-bad.sh");
    std::fs::write(&bad, "#!/bin/sh\nexit 7\n").unwrap();
    make_executable(&bad);
    let good = hooks.join("10-good.sh");
    std::fs::write(&good, "#!/bin/sh\nprintf 'still ran\\n' > marker.txt\n").unwrap();
    make_executable(&good);

    let context = PostInitHookContext {
        project_root: root.clone(),
        project_name: "demo".to_string(),
        lang: String::new(),
        remote_url: String::new(),
        forge: "none".to_string(),
    };
    run_post_init_hooks_from_dir(&context, &hooks).unwrap();

    assert_eq!(
        std::fs::read_to_string(root.join("marker.txt")).unwrap(),
        "still ran\n"
    );
}

#[test]
fn init_forge_detection_uses_known_remote_hosts_and_none_otherwise() {
    assert_eq!(
        detect_init_forge(None, "git@github.com:org/demo.git"),
        "github"
    );
    assert_eq!(
        detect_init_forge(None, "https://gitlab.com/org/demo.git"),
        "gitlab"
    );
    assert_eq!(
        detect_init_forge(None, "ssh://example.com/demo.git"),
        "none"
    );
    assert_eq!(detect_init_forge(Some("github"), ""), "github");
}

#[test]
fn test_enqueue_initial_scaffold_task_success_and_idempotency() {
    let tmp = TempDir::new().unwrap();
    let db_path = std::path::Path::new(".aida/cache.db");
    let full_db_dir = tmp.path().join(".aida");
    std::fs::create_dir_all(&full_db_dir).unwrap();

    // 1. Initial enqueueing
    enqueue_initial_scaffold_task(tmp.path(), db_path).unwrap();

    // Load and assert
    let storage = Storage::new(tmp.path().join(db_path));
    let store = storage.load().unwrap();
    assert_eq!(store.requirements.len(), 1);

    let req = &store.requirements[0];
    assert_eq!(req.title, "Commit AIDA scaffolding (initial setup)");
    assert_eq!(req.req_type, RequirementType::Task);
    assert_eq!(req.status, RequirementStatus::Approved);
    assert_eq!(req.priority, RequirementPriority::High);
    assert!(req.tags.contains("from-aida-init"));
    assert!(req.tags.contains("first-task"));
    assert!(req.tags.contains("scaffolding"));

    let spec_id = req.spec_id.as_deref().unwrap_or("TASK-1");
    assert!(req
        .description
        .contains("After aida init, the scaffolded files are untracked."));
    assert!(req
        .description
        .contains(&format!("Run 'aida queue done {}'", spec_id)));

    // 2. Idempotency check: run it again and verify we don't duplicate
    enqueue_initial_scaffold_task(tmp.path(), db_path).unwrap();
    let store2 = storage.load().unwrap();
    assert_eq!(
        store2.requirements.len(),
        1,
        "Idempotency failed: task was duplicated"
    );
}

/// BUG-445: the durable guardrail. The seeded scaffolding-commit task
/// must be flagged `human_only` so `aida queue work` never worktree-
/// isolates it. A worktree branched off the initial commit lacks the
/// uncommitted scaffolding entirely, and a bare `git add .` there would
/// embed the `.aida-store` gitlink + machine-specific cache/session
/// symlinks into history. `human_only` makes the pickability gate refuse
/// the spec at every queue-work pickup site, keeping it primary-worktree-
/// only.
///
/// This asserts on the builder (`build_initial_scaffold_requirement`) +
/// the pickability gate directly, NOT on a storage round-trip: the legacy
/// SQLite cache projection is lossy on `human_only` (the canonical
/// git-backend YAML round-trip preserves it, per
/// `RequirementsStore`/`models.rs`), so testing through `Storage::load`
/// on a `.db` path would assert the cache's lossiness rather than the
// seeding decision. trace:BUG-445 | ai:claude
#[test]
fn scaffold_task_is_primary_worktree_only_and_not_pickable() {
    use aida_core::pickability::{pickability, BlockedReason, Pickability};
    use aida_core::RequirementsStore;

    let req = build_initial_scaffold_requirement();

    // The guardrail flag itself.
    assert!(
        req.human_only,
        "scaffolding-commit task must be human_only so queue-work never \
             worktree-isolates it (BUG-445)"
    );

    // And the consequence: the pickability gate (consulted by every
    // queue-work pickup site — head pickup, batch drain, cluster drain)
    // refuses it, so it can never route through worktree isolation.
    let mut store = RequirementsStore::default();
    store.requirements.push(req.clone());
    assert_eq!(
        pickability(&req, &store),
        Pickability::Blocked(BlockedReason::HumanOnly),
        "scaffolding-commit task must be un-pickable (human-only) so \
             `aida queue work` cannot isolate it into a worktree (BUG-445)"
    );
}

/// BUG-445 guard (c): the seeded task instruction must never emit a bare
/// `git add .` — it stages only AIDA's own paths, with `.gitignore`
/// listed first so deny-by-default rules apply. Asserts on the persisted
/// description (which the cache DOES round-trip, unlike `human_only`).
// trace:BUG-445 | ai:claude
#[test]
fn scaffold_task_instruction_never_bare_git_add_dot() {
    let tmp = TempDir::new().unwrap();
    let db_path = std::path::Path::new(".aida/cache.db");
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();

    enqueue_initial_scaffold_task(tmp.path(), db_path).unwrap();

    let storage = Storage::new(tmp.path().join(db_path));
    let store = storage.load().unwrap();
    let desc = &store.requirements[0].description;

    // Match the bare form precisely: `git add .gitignore ...` is the
    // *correct* scoped form and legitimately starts with the same chars.
    assert!(
        !desc.contains("git add . "),
        "instruction must never emit a bare `git add .` (BUG-445 c); got:\n{desc}"
    );
    assert!(
        !desc.contains("git add .\n") && !desc.ends_with("git add ."),
        "instruction must never emit a bare `git add .` (BUG-445 c); got:\n{desc}"
    );
    assert!(
        desc.contains("git add .gitignore"),
        "instruction must stage scoped paths starting with .gitignore (BUG-445 c)"
    );
    assert!(
        !desc.contains(" docs/plans"),
        "fresh init no longer creates docs/plans, so the onboarding task must not stage it"
    );
}

#[test]
fn identity_checklist_only_runs_with_both_ttys() {
    assert!(init_identity_checklist_is_interactive(true, true));
    assert!(!init_identity_checklist_is_interactive(true, false));
    assert!(!init_identity_checklist_is_interactive(false, true));
    assert!(!init_identity_checklist_is_interactive(false, false));
}

#[test]
fn project_thesis_creation_writes_a_draft_vision() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join(".aida/cache.db");
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();

    let spec_id =
        create_project_thesis_vision(&db_path, "manual upkeep should be suspect").unwrap();

    let storage = Storage::new(&db_path);
    let store = storage.load().unwrap();
    let req = store
        .requirements
        .iter()
        .find(|r| r.spec_id.as_deref() == Some(spec_id.as_str()))
        .expect("created VISION should round-trip");
    assert_eq!(req.req_type, RequirementType::Vision);
    assert_eq!(req.status, RequirementStatus::Draft);
    assert!(req.description.contains("What bet is this project making?"));
    assert!(req.description.contains("manual upkeep should be suspect"));
    assert!(req.tags.contains("project-thesis"));
}

#[test]
fn antigravity_profile_json_detects_aida_mcp_registration() {
    let value: serde_json::Value =
        serde_json::from_str(r#"{"mcpServers":{"aida":{"command":"aida","args":["mcp-serve"]}}}"#)
            .unwrap();
    assert!(json_value_has_aida_mcp(&value));

    let stale: serde_json::Value = serde_json::from_str(
        r#"{"mcpServers":{"aida":{"command":"/tmp/aida","args":["mcp-serve"]}}}"#,
    )
    .unwrap();
    assert!(!json_value_has_aida_mcp(&stale));
}

#[test]
fn antigravity_mcp_offer_captures_command_and_marks_registered_once() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let argv = tmp.path().join("argv.txt");
    let argv_s = argv.display().to_string();
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_HOME", home.to_str().unwrap()),
        ("AIDA_TEST_ANTIGRAVITY_BINARY", "1"),
        ("AIDA_TEST_ANTIGRAVITY_MCP_TTY", "1"),
        ("AIDA_TEST_ANTIGRAVITY_MCP_ACCEPT", "1"),
        ("AIDA_TEST_ANTIGRAVITY_MCP_ARGV_OUT", &argv_s),
    ]);

    maybe_offer_antigravity_mcp_registration().unwrap();
    assert_eq!(
        std::fs::read_to_string(&argv).unwrap(),
        antigravity_aida_mcp_add_command()
    );
    assert!(antigravity_aida_mcp_registered());

    std::fs::write(&argv, "unchanged").unwrap();
    maybe_offer_antigravity_mcp_registration().unwrap();
    assert_eq!(std::fs::read_to_string(&argv).unwrap(), "unchanged");
}

#[test]
fn antigravity_mcp_non_tty_does_not_register() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let argv = tmp.path().join("argv.txt");
    let argv_s = argv.display().to_string();
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_HOME", home.to_str().unwrap()),
        ("AIDA_TEST_ANTIGRAVITY_BINARY", "1"),
        ("AIDA_TEST_ANTIGRAVITY_MCP_ARGV_OUT", &argv_s),
    ]);

    maybe_offer_antigravity_mcp_registration().unwrap();

    assert!(!argv.exists(), "non-TTY init must not run --add-mcp");
    assert!(!antigravity_aida_mcp_registered());
}
