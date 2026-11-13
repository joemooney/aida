// trace:TASK-3780 | ai:codex
use super::*;
use clap::Parser;

fn bug_1677_store_gitignore(contents: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(store.join(".gitignore"), contents).unwrap();
    (dir, store)
}

fn bug_563_runtime_patterns() -> String {
    format!(
        "# Per-clone runtime state — must never be tracked on the orphan aida-store branch\n{}",
        STORE_RUNTIME_GITIGNORE_PATTERNS
            .iter()
            .map(|pattern| format!("{pattern}\n"))
            .collect::<String>()
    )
}

fn task_1547_linked_store(branch: &str, contents: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    let run_git = |cwd: &std::path::Path, args: &[&str]| {
        let output = crate::test_git::git_command()
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run_git(project, &["init", "--initial-branch=main", "-q"]);
    run_git(project, &["config", "user.name", "AIDA Fixture"]);
    run_git(
        project,
        &["config", "user.email", "aida-fixture@example.invalid"],
    );
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        project.join(".aida/config.toml"),
        "[deployment]\nstore_path = \".aida-store\"\n",
    )
    .unwrap();
    run_git(project, &["add", ".aida/config.toml"]);
    run_git(project, &["commit", "-qm", "fixture project"]);
    let store = project.join(".aida-store");
    run_git(
        project,
        &["worktree", "add", "-b", branch, store.to_str().unwrap()],
    );
    std::fs::write(store.join(".gitignore"), contents).unwrap();
    run_git(&store, &["add", ".gitignore"]);
    run_git(&store, &["commit", "-qm", "fixture store"]);
    (dir, store)
}

fn task_202_git(cwd: &std::path::Path, args: &[&str]) {
    let output = crate::test_git::git_command()
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn task_202_project(root: &std::path::Path) {
    std::fs::create_dir_all(root).unwrap();
    task_202_git(root, &["-c", "init.defaultBranch=main", "init", "-q"]);
    task_202_git(root, &["config", "user.name", "AIDA Fixture"]);
    task_202_git(
        root,
        &["config", "user.email", "aida-fixture@example.invalid"],
    );
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[deployment]\nstore_path = \".aida-store\"\n",
    )
    .unwrap();
    task_202_git(root, &["add", ".aida/config.toml"]);
    task_202_git(root, &["commit", "-qm", "fixture project"]);
}

// trace:TASK-202 | ai:codex
#[test]
fn is_intended_store_accepts_linked_aida_store_worktree() {
    let tmp = tempfile::TempDir::new().unwrap();
    let project = tmp.path().join("project");
    task_202_project(&project);
    let store = project.join(".aida-store");
    task_202_git(&project, &["checkout", "--orphan", "aida-store"]);
    task_202_git(
        &project,
        &["commit", "--allow-empty", "-qm", "fixture store branch"],
    );
    task_202_git(&project, &["checkout", "main"]);
    task_202_git(
        &project,
        &["worktree", "add", store.to_str().unwrap(), "aida-store"],
    );
    assert!(is_intended_store_worktree(&project, &store));
}

// trace:TASK-202 | ai:codex
#[test]
fn is_intended_store_rejects_foreign_linked_worktree() {
    let tmp = tempfile::TempDir::new().unwrap();
    let project = tmp.path().join("project");
    task_202_project(&project);
    let foreign = tmp.path().join("foreign");
    task_202_project(&foreign);
    let store = project.join(".aida-store");
    task_202_git(
        &foreign,
        &[
            "worktree",
            "add",
            "-b",
            "aida-store",
            store.to_str().unwrap(),
        ],
    );

    // Establish that checks 1–4 hold, so only common-dir ownership rejects it.
    assert_eq!(
        aida_core::store_locate::detect_distributed_store_from(&project)
            .unwrap()
            .canonicalize()
            .unwrap(),
        store.canonicalize().unwrap()
    );
    let git_value = |args: &[&str]| {
        let output = crate::test_git::git_command()
            .arg("-C")
            .arg(&store)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    assert_eq!(
        std::fs::canonicalize(git_value(&["rev-parse", "--show-toplevel"])).unwrap(),
        store.canonicalize().unwrap()
    );
    assert!(std::fs::symlink_metadata(store.join(".git"))
        .unwrap()
        .file_type()
        .is_file());
    assert_ne!(
        git_value(&["rev-parse", "--git-dir"]),
        git_value(&["rev-parse", "--git-common-dir"])
    );
    assert_eq!(
        git_value(&["symbolic-ref", "--quiet", "--short", "HEAD"]),
        "aida-store"
    );
    assert!(!is_intended_store_worktree(&project, &store));
}

// trace:TASK-202 | ai:codex
#[test]
fn is_intended_store_rejects_standalone_repository() {
    let tmp = tempfile::TempDir::new().unwrap();
    let project = tmp.path().join("project");
    task_202_project(&project);
    let store = project.join(".aida-store");
    std::fs::create_dir_all(&store).unwrap();
    task_202_git(
        &store,
        &["-c", "init.defaultBranch=aida-store", "init", "-q"],
    );
    task_202_git(&store, &["config", "user.name", "AIDA Fixture"]);
    task_202_git(
        &store,
        &["config", "user.email", "aida-fixture@example.invalid"],
    );
    std::fs::write(store.join("seed"), "fixture").unwrap();
    task_202_git(&store, &["add", "seed"]);
    task_202_git(&store, &["commit", "-qm", "fixture store"]);
    assert!(store.join(".git").is_dir());
    assert!(!is_intended_store_worktree(&project, &store));
}

#[test]
fn bug_1677_appends_staging_ignores_to_bug_563_guarded_store() {
    let runtime_patterns = bug_563_runtime_patterns();
    let (_dir, store) = bug_1677_store_gitignore(&runtime_patterns);

    assert!(ensure_store_tracked_runtime_gitignore(&store).unwrap());

    let content = std::fs::read_to_string(store.join(".gitignore")).unwrap();
    assert_eq!(
        content,
        format!(
            "{runtime_patterns}\n{}",
            aida_core::fs_atomic::store_staging_ignore_block()
        )
    );
    for pattern in aida_core::fs_atomic::STORE_STAGING_IGNORE_PATTERNS {
        assert!(
            content.lines().any(|line| line.trim() == *pattern),
            "missing staging ignore pattern {pattern:?} in:\n{content}"
        );
    }
}

#[test]
fn bug_1677_both_gitignore_guards_present_is_noop() {
    let contents = format!(
        "{}\n{}",
        bug_563_runtime_patterns(),
        aida_core::fs_atomic::store_staging_ignore_block()
    );
    let (_dir, store) = bug_1677_store_gitignore(&contents);

    assert!(!ensure_store_tracked_runtime_gitignore(&store).unwrap());
    assert_eq!(
        std::fs::read_to_string(store.join(".gitignore")).unwrap(),
        contents
    );
}

#[test]
fn bug_1677_fresh_store_gets_both_gitignore_guards() {
    let (_dir, store) = bug_1677_store_gitignore("");

    assert!(ensure_store_tracked_runtime_gitignore(&store).unwrap());
    let content = std::fs::read_to_string(store.join(".gitignore")).unwrap();
    assert!(content.contains(".aida/node.toml"));
    for pattern in aida_core::fs_atomic::STORE_STAGING_IGNORE_PATTERNS {
        assert!(content.lines().any(|line| line.trim() == *pattern));
    }
}

#[test]
fn task_1547_finding_fires_for_bug_563_healed_store() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        project.join(".aida/config.toml"),
        "mode = \"distributed\"\n",
    )
    .unwrap();
    let store = project.join(".aida-store");
    std::fs::create_dir_all(store.join("objects")).unwrap();
    std::fs::write(store.join(".gitignore"), bug_563_runtime_patterns()).unwrap();
    assert!(crate::test_git::git_command()
        .arg("-C")
        .arg(&store)
        .args(["init", "-q"])
        .status()
        .unwrap()
        .success());

    let findings =
        collect_doctor_findings(project, &aida_core::RequirementsStore::default(), None).unwrap();
    assert!(findings
        .iter()
        .any(|finding| finding.category == "store-staging-ignore"));
    assert!(detect_store_tracked_runtime(project).is_empty());
}

#[test]
fn task_1547_user_lines_keep_content_and_order_and_repair_is_idempotent() {
    let original = "# user's first rule\nnotes.tmp.txt\ncustom/**\n";
    let (_dir, store) = bug_1677_store_gitignore(original);

    assert!(ensure_store_tracked_runtime_gitignore(&store).unwrap());
    let repaired = std::fs::read_to_string(store.join(".gitignore")).unwrap();
    assert!(repaired.starts_with(original));
    assert!(repaired
        .lines()
        .any(|line| line.trim() == "objects/**/*.tmp.*"));
    assert!(!repaired.lines().any(|line| line.trim() == "*.tmp.*"));
    assert!(!ensure_store_tracked_runtime_gitignore(&store).unwrap());
    assert_eq!(
        std::fs::read_to_string(store.join(".gitignore")).unwrap(),
        repaired
    );
}

#[test]
fn task_1547_crlf_user_bytes_are_preserved_with_one_separator() {
    let original = b"# user rule\r\ncustom/**\r\n";
    let (_dir, store) = bug_1677_store_gitignore(std::str::from_utf8(original).unwrap());
    assert!(ensure_store_tracked_runtime_gitignore(&store).unwrap());
    let repaired = std::fs::read(store.join(".gitignore")).unwrap();
    assert!(repaired.starts_with(original));
    assert_eq!(&repaired[original.len()..original.len() + 1], b"\n");
    assert!(String::from_utf8_lossy(&repaired[original.len()..])
        .starts_with("\n# Per-clone runtime state"));
}

#[test]
fn task_1547_missing_final_newline_is_added_as_one_separator() {
    let original = b"# user rule\ncustom/**";
    let (_dir, store) = bug_1677_store_gitignore(std::str::from_utf8(original).unwrap());
    assert!(ensure_store_tracked_runtime_gitignore(&store).unwrap());
    let repaired = std::fs::read(store.join(".gitignore")).unwrap();
    assert!(repaired.starts_with(original));
    assert_eq!(&repaired[original.len()..original.len() + 1], b"\n");
    assert!(String::from_utf8_lossy(&repaired[original.len()..])
        .starts_with("\n# Per-clone runtime state"));
}

#[test]
fn task_1547_non_worktree_store_fails_closed_without_writes_or_commit() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    let store = project.join(".aida-store");
    std::fs::create_dir_all(store.join("objects")).unwrap();
    std::fs::write(store.join(".gitignore"), bug_563_runtime_patterns()).unwrap();
    let run_git = |cwd: &std::path::Path, args: &[&str]| {
        crate::test_git::git_command()
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .unwrap()
    };
    assert!(run_git(project, &["init", "-q"]).status.success());
    assert!(run_git(project, &["config", "user.name", "AIDA Fixture"])
        .status
        .success());
    assert!(run_git(
        project,
        &["config", "user.email", "aida-fixture@example.invalid"]
    )
    .status
    .success());
    assert!(run_git(project, &["add", ".aida-store/.gitignore"])
        .status
        .success());
    assert!(run_git(project, &["commit", "-qm", "parent baseline"])
        .status
        .success());
    let before = std::fs::read(store.join(".gitignore")).unwrap();
    let head_before = run_git(project, &["rev-parse", "HEAD"]);
    let finding = DoctorFinding {
        category: "store-staging-ignore".into(),
        id: ".gitignore".into(),
        summary: "missing staging ignores".into(),
        action: "append staging ignores".into(),
        safe_heal: true,
    };

    let result = heal_doctor_store_staging_ignore(project, &finding).unwrap();

    assert_eq!(result.status, "failed");
    assert_eq!(std::fs::read(store.join(".gitignore")).unwrap(), before);
    assert_eq!(
        run_git(project, &["rev-parse", "HEAD"]).stdout,
        head_before.stdout
    );
    assert!(run_git(project, &["diff", "--quiet"]).status.success());
}

#[test]
fn task_1547_standalone_store_repo_fails_closed_without_file_index_or_head_changes() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    let store = project.join(".aida-store");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        project.join(".aida/config.toml"),
        "[deployment]\nstore_path = \".aida-store\"\n",
    )
    .unwrap();
    std::fs::write(store.join(".gitignore"), bug_563_runtime_patterns()).unwrap();
    let git = |args: &[&str]| {
        let output = crate::test_git::git_command()
            .arg("-C")
            .arg(&store)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };
    assert!(crate::test_git::git_command()
        .arg("-C")
        .arg(&store)
        .args(["init", "-b", "aida-store", "-q"])
        .status()
        .unwrap()
        .success());
    git(&["config", "user.name", "AIDA Fixture"]);
    git(&["config", "user.email", "aida-fixture@example.invalid"]);
    git(&["add", ".gitignore"]);
    git(&["commit", "-qm", "fixture baseline"]);
    let before_file = std::fs::read(store.join(".gitignore")).unwrap();
    let before_head = git(&["rev-parse", "HEAD"]).stdout;
    let before_index = git(&["write-tree"]).stdout;
    let finding = DoctorFinding {
        category: "store-staging-ignore".into(),
        id: ".gitignore".into(),
        summary: "missing staging ignores".into(),
        action: "append staging ignores".into(),
        safe_heal: true,
    };

    let result = heal_doctor_store_staging_ignore(project, &finding).unwrap();

    assert_eq!(result.status, "failed");
    assert_eq!(
        std::fs::read(store.join(".gitignore")).unwrap(),
        before_file
    );
    assert_eq!(git(&["rev-parse", "HEAD"]).stdout, before_head);
    assert_eq!(git(&["write-tree"]).stdout, before_index);
}

#[test]
fn task_1547_foreign_linked_store_fails_closed_without_file_index_or_head_changes() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    let foreign = dir.path().join("foreign");
    let store = project.join(".aida-store");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&foreign).unwrap();
    let git = |cwd: &std::path::Path, args: &[&str]| {
        let output = crate::test_git::git_command()
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };
    git(&project, &["init", "--initial-branch=main", "-q"]);
    git(&project, &["config", "user.name", "AIDA Fixture"]);
    git(
        &project,
        &["config", "user.email", "aida-fixture@example.invalid"],
    );
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        project.join(".aida/config.toml"),
        "[deployment]\nstore_path = \".aida-store\"\n",
    )
    .unwrap();
    git(&project, &["add", ".aida/config.toml"]);
    git(&project, &["commit", "-qm", "fixture project"]);

    git(&foreign, &["init", "--initial-branch=main", "-q"]);
    git(&foreign, &["config", "user.name", "AIDA Fixture"]);
    git(
        &foreign,
        &["config", "user.email", "aida-fixture@example.invalid"],
    );
    std::fs::write(foreign.join("README"), "foreign repository\n").unwrap();
    git(&foreign, &["add", "README"]);
    git(&foreign, &["commit", "-qm", "fixture foreign repository"]);
    git(
        &foreign,
        &[
            "worktree",
            "add",
            "-b",
            "aida-store",
            store.to_str().unwrap(),
        ],
    );
    std::fs::write(store.join(".gitignore"), bug_563_runtime_patterns()).unwrap();
    git(&store, &["add", ".gitignore"]);
    git(&store, &["commit", "-qm", "fixture store"]);

    let before_file = std::fs::read(store.join(".gitignore")).unwrap();
    let before_head = git(&store, &["rev-parse", "HEAD"]).stdout;
    let before_index = git(&store, &["write-tree"]).stdout;
    let finding = DoctorFinding {
        category: "store-staging-ignore".into(),
        id: ".gitignore".into(),
        summary: "missing staging ignores".into(),
        action: "append staging ignores".into(),
        safe_heal: true,
    };

    let result = heal_doctor_store_staging_ignore(&project, &finding).unwrap();

    assert_eq!(result.status, "failed");
    assert_eq!(
        std::fs::read(store.join(".gitignore")).unwrap(),
        before_file
    );
    assert_eq!(git(&store, &["rev-parse", "HEAD"]).stdout, before_head);
    assert_eq!(git(&store, &["write-tree"]).stdout, before_index);
    assert!(git(&store, &["status", "--porcelain"]).stdout.is_empty());
}

#[test]
fn task_1547_wrong_branch_store_fails_closed_without_file_index_or_head_changes() {
    let (_dir, store) = task_1547_linked_store("wrong-store", &bug_563_runtime_patterns());
    let project = store.parent().unwrap();
    let git = |args: &[&str]| {
        crate::test_git::git_command()
            .arg("-C")
            .arg(&store)
            .args(args)
            .output()
            .unwrap()
    };
    let before_file = std::fs::read(store.join(".gitignore")).unwrap();
    let before_head = git(&["rev-parse", "HEAD"]).stdout;
    let before_index = git(&["write-tree"]).stdout;
    let finding = DoctorFinding {
        category: "store-staging-ignore".into(),
        id: ".gitignore".into(),
        summary: "missing staging ignores".into(),
        action: "append staging ignores".into(),
        safe_heal: true,
    };

    let result = heal_doctor_store_staging_ignore(project, &finding).unwrap();

    assert_eq!(result.status, "failed");
    assert_eq!(
        std::fs::read(store.join(".gitignore")).unwrap(),
        before_file
    );
    assert_eq!(git(&["rev-parse", "HEAD"]).stdout, before_head);
    assert_eq!(git(&["write-tree"]).stdout, before_index);
}

#[test]
fn task_1547_fully_guarded_store_is_byte_identical() {
    let contents = format!(
        "{}\n{}",
        bug_563_runtime_patterns(),
        aida_core::fs_atomic::store_staging_ignore_block()
    );
    let (_dir, store) = bug_1677_store_gitignore(&contents);
    let project = store.parent().unwrap();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        project.join(".aida/config.toml"),
        "mode = \"distributed\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(store.join("objects")).unwrap();
    let before = std::fs::read(store.join(".gitignore")).unwrap();

    assert!(crate::detect_store_missing_staging_ignores(project).is_empty());
    assert!(!ensure_store_tracked_runtime_gitignore(&store).unwrap());
    assert_eq!(std::fs::read(store.join(".gitignore")).unwrap(), before);
}

#[test]
fn task_1547_repair_commits_store_gitignore_on_orphan_branch() {
    let (_dir, store) = task_1547_linked_store("aida-store", &bug_563_runtime_patterns());
    let project = store.parent().unwrap();
    let git = |args: &[&str]| {
        crate::test_git::git_command()
            .arg("-C")
            .arg(&store)
            .args(args)
            .output()
            .unwrap()
    };
    assert!(git(&["init", "-q"]).status.success());
    assert!(git(&["config", "user.name", "AIDA Fixture"])
        .status
        .success());
    assert!(
        git(&["config", "user.email", "aida-fixture@example.invalid"])
            .status
            .success()
    );
    assert!(git(&["add", ".gitignore"]).status.success());
    std::fs::write(store.join("unrelated.txt"), "baseline\n").unwrap();
    assert!(git(&["add", "unrelated.txt"]).status.success());
    assert!(git(&["commit", "-qm", "fixture baseline"]).status.success());
    std::fs::write(store.join("unrelated.txt"), "user staged change\n").unwrap();
    assert!(git(&["add", "unrelated.txt"]).status.success());

    let finding = DoctorFinding {
        category: "store-staging-ignore".to_string(),
        id: ".gitignore".to_string(),
        summary: "store .gitignore is missing the staging-ignore patterns".to_string(),
        action: "append staging ignores".to_string(),
        safe_heal: true,
    };
    let result = heal_doctor_store_staging_ignore(project, &finding).unwrap();

    assert_eq!(result.status, "healed");
    assert!(git(&["diff", "--quiet"]).status.success());
    assert!(!git(&["diff", "--cached", "--quiet"]).status.success());
    assert_eq!(
        String::from_utf8(git(&["diff", "--cached", "--name-only"]).stdout)
            .unwrap()
            .trim(),
        "unrelated.txt"
    );
    let head = String::from_utf8(git(&["log", "-1", "--format=%s"]).stdout).unwrap();
    assert!(head.contains("TASK-1547 TASK-1556"));
    let content = std::fs::read_to_string(store.join(".gitignore")).unwrap();
    for pattern in aida_core::fs_atomic::STORE_STAGING_IGNORE_PATTERNS {
        assert!(content.lines().any(|line| line.trim() == *pattern));
    }
}

#[test]
fn task_1556_store_with_only_first_staging_pattern_gets_the_rest() {
    let (_dir, store) = bug_1677_store_gitignore("objects/**/*.tmp.*\n");
    assert!(ensure_store_tracked_runtime_gitignore(&store).unwrap());
    let content = std::fs::read_to_string(store.join(".gitignore")).unwrap();
    for pattern in aida_core::fs_atomic::STORE_STAGING_IGNORE_PATTERNS {
        assert!(
            content.lines().any(|line| line.trim() == *pattern),
            "missing {pattern}"
        );
    }
    assert_eq!(content.matches("objects/**/*.tmp.*").count(), 1);
}

#[test]
fn task_1556_injected_sixth_pattern_is_appended_when_first_five_exist() {
    let five = aida_core::fs_atomic::STORE_STAGING_IGNORE_PATTERNS;
    let sixth = "new-area/**/*.tmp.*";
    let mut injected = five.to_vec();
    injected.push(sixth);
    let contents = format!("{}\n", five.join("\n"));
    let (_dir, store) = bug_1677_store_gitignore(&contents);

    assert!(ensure_store_tracked_runtime_gitignore_with_patterns(&store, &injected).unwrap());
    let repaired = std::fs::read_to_string(store.join(".gitignore")).unwrap();
    assert!(repaired.lines().any(|line| line.trim() == sixth));
    for pattern in five {
        assert_eq!(
            repaired
                .lines()
                .filter(|line| line.trim() == *pattern)
                .count(),
            1
        );
    }
}

// trace:STORY-2660 | ai:codex
#[test]
fn host_capacity_doctor_flags_parse_and_refuse_mutation() {
    let cli = crate::TEST_CLI
        .try_parse_from(["aida", "doctor", "--lane-capacity", "--json"])
        .unwrap();
    let DoctorArgs {
        lane_capacity,
        json,
        ..
    } = cli.command.registered::<DoctorArgs>()
    else {
        panic!("doctor args")
    };
    assert!(lane_capacity && json);
    for flag in ["--heal", "--fix-sandbox", "--contradictions"] {
        assert!(crate::TEST_CLI
            .try_parse_from(["aida", "doctor", "--lane-capacity", flag])
            .is_err());
    }
}

#[test]
fn doctor_default_flags_parse_without_subcommand() {
    let cli = crate::TEST_CLI
        .try_parse_from([
            "aida",
            "doctor",
            "--heal",
            "--yes",
            "--category",
            "stale-leases",
            "--json",
        ])
        .unwrap();
    let crate::doctor_cmd::DoctorArgs {
        heal,
        yes,
        category,
        json,
        force,
        all,
        since,
        fix_sandbox,
        cmd,
        ..
    } = cli.command.registered::<crate::doctor_cmd::DoctorArgs>()
    else {
        panic!("expected doctor command");
    };
    assert!(heal);
    assert!(yes);
    assert_eq!(category.as_deref(), Some("stale-leases"));
    assert!(json);
    assert!(!force);
    assert!(!all);
    assert!(since.is_none());
    assert!(!fix_sandbox);
    assert!(cmd.is_none());
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude
// trace:BUG-B6B1278890774EF993D2EBC327862470-046.ac602938 | ai:claude
#[test]
fn doctor_fix_id_collisions_flags_parse_and_require_fix() {
    let cli = crate::TEST_CLI
        .try_parse_from([
            "aida",
            "doctor",
            "--fix",
            "id-collisions",
            "--dry-run",
            "--mapping",
            "m.tsv",
        ])
        .unwrap();
    let crate::doctor_cmd::DoctorArgs {
        fix,
        dry_run,
        mapping,
        tree_only,
        ..
    } = cli.command.registered::<crate::doctor_cmd::DoctorArgs>()
    else {
        panic!("expected doctor command");
    };
    assert_eq!(fix.as_deref(), Some("id-collisions"));
    assert!(dry_run && !tree_only);
    assert_eq!(mapping.as_deref(), Some(std::path::Path::new("m.tsv")));
    for bad in [
        &["aida", "doctor", "--dry-run"][..],
        &["aida", "doctor", "--fix", "everything"],
        &["aida", "doctor", "--fix", "id-collisions", "--tree-only"],
    ] {
        assert!(
            crate::TEST_CLI.try_parse_from(bad.iter().copied()).is_err(),
            "{bad:?}"
        );
    }
}

#[test]
fn normalize_doctor_category_accepts_dead_agents() {
    // STORY-496
    for alias in ["dead-agents", "dead-agent", "stale-agents", "agents"] {
        assert_eq!(normalize_doctor_category(alias).unwrap(), "dead-agents");
    }
}

/// The sandbox config.lock category is reachable by every alias, selected
/// by `--category`, and its findings route to the heal that removes the
/// placeholder.
// trace:TASK-2270 | ai:claude
#[cfg(unix)]
#[test]
fn sandbox_config_lock_category_routes_from_alias_to_heal() {
    use std::os::unix::fs::PermissionsExt;
    for alias in ["sandbox-config-lock", "config-lock", "git-config-lock"] {
        assert_eq!(
            normalize_doctor_category(alias).unwrap(),
            crate::sandbox_config_lock::CATEGORY
        );
        assert!(
            doctor_category_selected(Some(alias), crate::sandbox_config_lock::CATEGORY).unwrap()
        );
    }
    assert!(
        !doctor_category_selected(Some("stale-locks"), crate::sandbox_config_lock::CATEGORY)
            .unwrap()
    );

    let dir = tempfile::tempdir().unwrap();
    let init = crate::test_git::git_command()
        .current_dir(dir.path())
        .args(["init", "-q", "-b", "main"])
        .output()
        .unwrap();
    assert!(init.status.success(), "{init:?}");
    let lock = dir.path().join(".git/config.lock");
    std::fs::write(&lock, b"").unwrap();
    std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o444)).unwrap();

    let finding = crate::sandbox_config_lock::scan(dir.path()).remove(0);
    let healed = heal_doctor_finding(dir.path(), &finding, &DoctorRunOptions::default()).unwrap();
    assert_eq!(healed.status, "healed");
    assert!(!lock.exists());
}

/// A full `aida doctor --category config-lock` run reports the planted
/// placeholder, so the scan stays wired into `doctor_multi_agent`. Runs in
/// its own process because it changes the working directory.
// trace:TASK-2270 | ai:claude
#[cfg(unix)]
#[test]
fn doctor_run_with_config_lock_category_reports_the_placeholder() {
    use std::os::unix::fs::PermissionsExt;
    if let Some(out) = crate::test_env::run_path_isolated_test_output() {
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains(&format!(
                "\"category\": \"{}\"",
                crate::sandbox_config_lock::CATEGORY
            )) && stdout.contains("config.lock"),
            "{stdout}"
        );
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let init = crate::test_git::git_command()
        .current_dir(dir.path())
        .args(["init", "-q", "-b", "main"])
        .output()
        .unwrap();
    assert!(init.status.success(), "{init:?}");
    std::fs::create_dir(dir.path().join(".aida")).unwrap();
    std::fs::create_dir(dir.path().join(".aida-store")).unwrap();
    let lock = dir.path().join(".git/config.lock");
    std::fs::write(&lock, b"").unwrap();
    std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o444)).unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    handle_doctor_command(
        false,
        false,
        Some("config-lock"),
        true,
        false,
        false,
        None,
        None,
    )
    .unwrap();
}

// ── TASK-717: stale-remote-branch classification ──

#[test]
fn normalize_doctor_category_accepts_stale_remote_branches() {
    // TASK-717
    for alias in [
        "stale-remote-branches",
        "stale-remote-branch",
        "remote-branch",
        "remote-branches",
        "remote-branch-prune",
    ] {
        assert_eq!(
            normalize_doctor_category(alias).unwrap(),
            "stale-remote-branches"
        );
    }
}

#[test]
fn normalize_doctor_category_accepts_merged_agent_worktrees() {
    // TASK-878
    for alias in [
        "merged-agent-worktrees",
        "merged-agent-worktree",
        "agent-worktree",
        "agent-worktrees",
        "worktree-gc",
        "agent-worktree-gc",
    ] {
        assert_eq!(
            normalize_doctor_category(alias).unwrap(),
            "merged-agent-worktrees"
        );
    }
}

// BUG-1535: a merge-gate agreed_id equal to another object's native
// spec_id is reported under `id-collisions`, naming BOTH objects, and is
// never marked safe to auto-heal. trace:BUG-1535 | ai:claude
#[test]
fn id_collisions_finding_names_both_objects() {
    use aida_core::models::{Requirement, RequirementsStore};
    let mut fixture = Requirement::new("[test] fixture".into(), "d".into());
    fixture.spec_id = Some("BUG-34".into());
    let fixture_uuid = fixture.id;
    let mut real = Requirement::new("real spec".into(), "d".into());
    real.spec_id = Some("BUG-2-081".into());
    real.agreed_id = Some("BUG-34".into());
    let mut clean = Requirement::new("clean".into(), "d".into());
    clean.spec_id = Some("BUG-35".into());
    clean.agreed_id = Some("BUG-35".into());
    let mut store = RequirementsStore::new();
    store.requirements = vec![real, fixture, clean];

    let findings = id_collision_findings(&store);
    // One collision finding + the unique-id count check.
    assert_eq!(findings.len(), 2, "{findings:?}");
    let count = findings.iter().find(|f| f.id == "store-count").unwrap();
    assert!(count.summary.contains("3 objects") && count.summary.contains("only 2 unique"));
    let f = findings.iter().find(|f| f.id == "BUG-34").unwrap();
    assert_eq!(f.category, "id-collisions");
    assert_eq!(f.id, "BUG-34");
    assert!(!f.safe_heal);
    assert!(f.summary.contains("[test] fixture"), "{}", f.summary);
    assert!(f.summary.contains("BUG-2-081"), "{}", f.summary);
    // The native owner's unambiguous handle is its uuid.
    assert!(
        f.summary.contains(&fixture_uuid.to_string()),
        "{}",
        f.summary
    );
    for alias in ["id-collisions", "ambiguous-ids", "duplicate_ids"] {
        assert_eq!(normalize_doctor_category(alias).unwrap(), "id-collisions");
    }
}

#[test]
fn normalize_doctor_category_accepts_ci() {
    // trace:STORY-1043 | ai:codex
    for alias in ["ci", "cross-platform", "cross-platform-ci", "nightly-red"] {
        assert_eq!(normalize_doctor_category(alias).unwrap(), "ci");
    }
}

fn remote_facts() -> RemoteBranchFacts {
    RemoteBranchFacts {
        protected: false,
        has_open_pr: false,
        ancestor_of_main: false,
        spec_referenced_on_main: false,
        spec_terminal: false,
        unique_unmerged_commits: 0,
    }
}

#[test]
fn classify_squash_merged_remote_branch_is_safe_to_delete() {
    // TASK-717: squash-merged → origin/main references the spec, branch tip
    // has a different SHA (so NOT an ancestor) but zero unique commits.
    let facts = RemoteBranchFacts {
        spec_referenced_on_main: true,
        ..remote_facts()
    };
    assert!(matches!(
        classify_stale_remote_branch(&facts),
        RemoteBranchVerdict::SafeToDelete(_)
    ));
}

#[test]
fn classify_ancestor_of_main_remote_branch_is_safe_to_delete() {
    // TASK-717: fast-forward / non-squash merge → HEAD is an ancestor.
    let facts = RemoteBranchFacts {
        ancestor_of_main: true,
        ..remote_facts()
    };
    assert!(matches!(
        classify_stale_remote_branch(&facts),
        RemoteBranchVerdict::SafeToDelete(_)
    ));
}

#[test]
fn classify_terminal_spec_remote_branch_is_safe_to_delete() {
    // TASK-717: spec is Completed/Rejected with no unique commits.
    let facts = RemoteBranchFacts {
        spec_terminal: true,
        ..remote_facts()
    };
    assert!(matches!(
        classify_stale_remote_branch(&facts),
        RemoteBranchVerdict::SafeToDelete(_)
    ));
}

#[test]
fn classify_unique_unmerged_remote_branch_is_kept() {
    // TASK-717: squash-merged BUT carries genuinely-unique unmerged commits
    // (the spock-dev migration-guide case) → KEEP, never delete.
    let facts = RemoteBranchFacts {
        spec_referenced_on_main: true,
        unique_unmerged_commits: 3,
        ..remote_facts()
    };
    assert!(matches!(
        classify_stale_remote_branch(&facts),
        RemoteBranchVerdict::Keep(_)
    ));
}

#[test]
fn classify_no_merge_signal_remote_branch_is_kept() {
    // TASK-717: no merged/terminal signal at all → never delete; flag.
    let facts = remote_facts();
    assert!(matches!(
        classify_stale_remote_branch(&facts),
        RemoteBranchVerdict::Keep(_)
    ));
}

#[test]
fn classify_open_pr_remote_branch_is_excluded() {
    // TASK-717: an open-PR branch is excluded even if it looks merged.
    let facts = RemoteBranchFacts {
        has_open_pr: true,
        spec_referenced_on_main: true,
        ..remote_facts()
    };
    assert!(matches!(
        classify_stale_remote_branch(&facts),
        RemoteBranchVerdict::Excluded(_)
    ));
}

#[test]
fn classify_protected_remote_branch_is_excluded() {
    // TASK-717: protected refs (main/master/aida-store) are excluded first,
    // even when every "merged" signal is set.
    let facts = RemoteBranchFacts {
        protected: true,
        ancestor_of_main: true,
        spec_referenced_on_main: true,
        spec_terminal: true,
        ..remote_facts()
    };
    assert!(matches!(
        classify_stale_remote_branch(&facts),
        RemoteBranchVerdict::Excluded(_)
    ));
}

fn agent_wt_facts() -> AgentWorktreeFacts {
    AgentWorktreeFacts {
        dirty: false,
        ancestor_of_main: false,
        pr_merged: false,
        unique_unmerged_commits: 0,
        content_fully_landed: false,
        spec_trailer_on_main: false,
    }
}

#[test]
fn classify_ancestor_of_main_agent_worktree_is_removable() {
    // TASK-878: fast-forward / non-squash merge → branch is an ancestor of
    // origin/main, clean, zero unique commits → safe to GC.
    let facts = AgentWorktreeFacts {
        ancestor_of_main: true,
        ..agent_wt_facts()
    };
    assert!(matches!(
        classify_agent_worktree(&facts),
        AgentWorktreeVerdict::Removable(_)
    ));
}

#[test]
fn classify_squash_merged_agent_worktree_is_removable() {
    // TASK-878: squash-merged → branch tip differs (NOT ancestor) but the
    // forge reports a merged PR and there are zero unique commits → removable.
    let facts = AgentWorktreeFacts {
        pr_merged: true,
        ..agent_wt_facts()
    };
    assert!(matches!(
        classify_agent_worktree(&facts),
        AgentWorktreeVerdict::Removable(_)
    ));
}

#[test]
fn classify_unmerged_commits_agent_worktree_is_kept() {
    // TASK-878: PR merged BUT the branch carries genuinely-unique unmerged
    // commits (extra work added after the squash merge) → KEEP, never remove.
    let facts = AgentWorktreeFacts {
        pr_merged: true,
        unique_unmerged_commits: 2,
        ..agent_wt_facts()
    };
    assert!(matches!(
        classify_agent_worktree(&facts),
        AgentWorktreeVerdict::Keep { .. }
    ));
}

#[test]
fn classify_content_verified_squash_merge_is_removable() {
    // BUG-1287: PR merged, ancestry reports a positive commit count (the
    // squash-merge case can NEVER clear this by ancestry alone — the
    // branch's original commits keep a different SHA from the squash
    // commit forever) BUT a content-level check proves every file the
    // branch touched already matches origin/main byte-for-byte → the
    // count is an ancestry artifact, not real unshipped work → Removable.
    let facts = AgentWorktreeFacts {
        pr_merged: true,
        unique_unmerged_commits: 1,
        content_fully_landed: true,
        ..agent_wt_facts()
    };
    assert!(matches!(
        classify_agent_worktree(&facts),
        AgentWorktreeVerdict::Removable(_)
    ));
}

#[test]
fn classify_partially_landed_squash_merge_is_kept() {
    // BUG-1287 companion: content_fully_landed stays false when even ONE
    // of the branch's touched files still differs from main — e.g. later
    // commits added after the PR squash-merged that never shipped. The
    // positive commit count keeps this KEPT, exactly like the plain
    // unmerged-commits case above.
    let facts = AgentWorktreeFacts {
        pr_merged: true,
        unique_unmerged_commits: 4,
        content_fully_landed: false,
        ..agent_wt_facts()
    };
    assert!(matches!(
        classify_agent_worktree(&facts),
        AgentWorktreeVerdict::Keep { .. }
    ));
}

/// BUG-1287 fixture: a real git repo with two squash-merged branches —
/// one where the squash-merge shipped ALL of the branch's content (the
/// false-positive case ancestry alone can never clear), and one where a
/// later commit was added to the branch AFTER the squash landed and never
/// shipped (genuinely unique, must stay kept). Asserts
/// `branch_content_fully_landed` tells them apart — the reapable one from
/// the not-yet-reapable one — by durable patch identity, not by ancestry
/// or a point-in-time comparison of current trees.
// trace:BUG-1287 | ai:claude
// trace:BUG-1424 | ai:codex
#[test]
fn branch_content_fully_landed_distinguishes_shipped_from_unshipped_squash_branches() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();

    let run = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };

    crate::test_git::git_command()
        .arg("init")
        .arg(&root)
        .output()
        .unwrap();
    run(&["config", "user.name", "Test"]);
    run(&["config", "user.email", "test@example.com"]);
    std::fs::write(root.join("README.md"), "base\n").unwrap();
    run(&["add", "README.md"]);
    run(&["commit", "-m", "init"]);
    run(&["branch", "-M", "main"]);

    // Branch A: two branch commits are combined into one squash commit on
    // main. Neither source commit is patch-equivalent to the cumulative
    // squash, but the branch's merge-base-to-tip patch is.
    run(&["checkout", "-b", "shipped-work"]);
    std::fs::write(root.join("a.txt"), "shipped content\n").unwrap();
    run(&["add", "a.txt"]);
    run(&["commit", "-m", "add a.txt"]);
    std::fs::write(root.join("a2.txt"), "second shipped change\n").unwrap();
    run(&["add", "a2.txt"]);
    run(&["commit", "-m", "add a2.txt"]);
    run(&["checkout", "main"]);
    // Simulate a forge squash-merge: both files land in one brand-new
    // commit, unrelated by ancestry to either branch commit.
    std::fs::write(root.join("a.txt"), "shipped content\n").unwrap();
    std::fs::write(root.join("a2.txt"), "second shipped change\n").unwrap();
    run(&["add", "a.txt", "a2.txt"]);
    run(&["commit", "-m", "squash-merge shipped-work (#1)"]);

    // Branch B: its FIRST commit is squash-merged the same way, but a
    // SECOND commit is added afterward that never ships — genuinely
    // unique, unmerged work that must never be reported as landed.
    run(&["checkout", "-b", "partial-work"]);
    run(&["reset", "--hard", "main~1"]); // back to pre-squash main tip
    std::fs::write(root.join("b.txt"), "partial content\n").unwrap();
    run(&["add", "b.txt"]);
    run(&["commit", "-m", "add b.txt"]);
    std::fs::write(root.join("b2.txt"), "never shipped\n").unwrap();
    run(&["add", "b2.txt"]);
    run(&["commit", "-m", "follow-up never merged"]);
    run(&["checkout", "main"]);
    std::fs::write(root.join("b.txt"), "partial content\n").unwrap();
    run(&["add", "b.txt"]);
    run(&["commit", "-m", "squash-merge partial-work (#2)"]);

    // Move the world after both squash merges. In particular, change a
    // file shipped-work touched: the old tree comparison now says the
    // branch differs, while its patch remains durably present in main's
    // bounded history.
    std::fs::write(root.join("a.txt"), "shipped content\nlater main edit\n").unwrap();
    run(&["add", "a.txt"]);
    run(&["commit", "-m", "advance main and revisit overlapping file"]);

    // shipped-work: ancestry can never clear this (different SHA forever),
    // and the current trees no longer match, but the landed patch is still
    // present in main's bounded history.
    let count_out = crate::test_git::git_command()
        .arg("-C")
        .arg(&root)
        .args(["rev-list", "--count", "main..shipped-work"])
        .output()
        .unwrap();
    let ancestry_count: u32 = String::from_utf8_lossy(&count_out.stdout)
        .trim()
        .parse()
        .unwrap();
    assert_eq!(
        ancestry_count, 2,
        "ancestry must still see both squash-merged branch commits as unique"
    );
    let per_commit_out = crate::test_git::git_command()
        .arg("-C")
        .arg(&root)
        .args([
            "rev-list",
            "--cherry-pick",
            "--right-only",
            "main...shipped-work",
        ])
        .output()
        .unwrap();
    assert!(per_commit_out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&per_commit_out.stdout)
            .lines()
            .count(),
        2,
        "per-commit patch matching must miss both commits combined by the squash"
    );
    assert!(
        branch_content_fully_landed(&root, "main", "shipped-work"),
        "squash-merged branch must remain fully landed after main changes an overlapping file"
    );

    // partial-work: one file matches main, but b2.txt never shipped — the
    // content check must stay conservative and report NOT fully landed.
    assert!(
        !branch_content_fully_landed(&root, "main", "partial-work"),
        "a branch carrying a genuinely-unshipped file must never be reported fully landed"
    );
}

// BUG-1288: `branch_content_fully_landed`'s squash-merge fallback used to
// spawn a `git show | git patch-id` subprocess PAIR per commit between a
// branch's merge-base and the default branch. On the aida repo itself,
// one call with a 2,496-commit range measured 3m41s of wall clock — the
// dominant cause of `aida awaiting --json` / `aida status --full`
// blocking for 70-90s. This fixture reproduces the same shape at a
// CI-affordable scale (500+ commits) and pins two things at once: the
// deep-history branch must not block the caller (the new
// `MAX_SQUASH_FALLBACK_COMMITS` bound bails out instead of walking the
// whole range), and bailing out must stay conservative — a genuinely
// unshipped branch still reads `false` ("not confirmed landed"), never a
// false `true`, so real unshipped work can never be hidden by this bound
// (PRIN-5 / BUG-1288 acceptance #6). The 30s budget is deliberately
// generous: this pins "does not regress back to unbounded", not a tight
// perf target — this bug's own history (the candidate population roughly
// 8x'd between when the spec was filed and when this fix landed) is the
// reason a tight wall-clock assertion would be the wrong thing to pin in
// CI. trace:BUG-1288 | ai:claude
#[test]
fn branch_content_fully_landed_bails_out_on_a_deep_history_range() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();

    let run = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };

    crate::test_git::git_command()
        .arg("init")
        .arg(&root)
        .output()
        .unwrap();
    run(&["config", "user.name", "Test"]);
    run(&["config", "user.email", "test@example.com"]);
    std::fs::write(root.join("README.md"), "base\n").unwrap();
    run(&["add", "README.md"]);
    run(&["commit", "-m", "init"]);
    run(&["branch", "-M", "main"]);

    // Branch off right away — this commit is the merge-base the filler
    // history below piles up past.
    run(&["checkout", "-b", "deep-history-work"]);
    std::fs::write(root.join("never-shipped.txt"), "genuinely unshipped\n").unwrap();
    run(&["add", "never-shipped.txt"]);
    run(&["commit", "-m", "add never-shipped.txt"]);
    run(&["checkout", "main"]);

    // Push main past MAX_SQUASH_FALLBACK_COMMITS (500) commits since the
    // branch's merge-base — the shape that took 3m41s pre-fix on real
    // history.
    for i in 0..520 {
        run(&["commit", "--allow-empty", "-m", &format!("filler {i}")]);
    }

    let merge_base_out = crate::test_git::git_command()
        .arg("-C")
        .arg(&root)
        .args(["merge-base", "main", "deep-history-work"])
        .output()
        .unwrap();
    let merge_base = String::from_utf8_lossy(&merge_base_out.stdout)
        .trim()
        .to_string();
    let count_out = crate::test_git::git_command()
        .arg("-C")
        .arg(&root)
        .args(["rev-list", "--count", &format!("{merge_base}..main")])
        .output()
        .unwrap();
    let default_range_count: u32 = String::from_utf8_lossy(&count_out.stdout)
        .trim()
        .parse()
        .unwrap();
    assert!(
            default_range_count > 500,
            "fixture must exceed MAX_SQUASH_FALLBACK_COMMITS to exercise the bound, got {default_range_count}"
        );

    // Warm the fixture (git's loose-object access, this process's page
    // cache, …) before the timed call, so the assertion below measures
    // the bounded algorithm's own cost, not first-touch overhead.
    let _ = branch_content_fully_landed(&root, "main", "deep-history-work");

    let budget = std::time::Duration::from_secs(30);
    let started = std::time::Instant::now();
    let landed = branch_content_fully_landed(&root, "main", "deep-history-work");
    let elapsed = started.elapsed();

    assert!(
        elapsed < budget,
        "deep-history squash-fallback took {elapsed:?}, expected well under the \
             {budget:?} regression budget (pre-fix this shape measured 3m41s on real history)"
    );
    assert!(
        !landed,
        "a genuinely-unshipped branch must stay reported as NOT landed even when the \
             expensive proof is skipped for being too large — never silently true"
    );
}

#[test]
fn classify_dirty_agent_worktree_is_kept_even_when_merged() {
    // TASK-878: clean != no work, but DIRTY is unambiguously work — a
    // worktree with uncommitted changes is KEPT even when its branch merged.
    let facts = AgentWorktreeFacts {
        dirty: true,
        ancestor_of_main: true,
        pr_merged: true,
        ..agent_wt_facts()
    };
    assert!(matches!(
        classify_agent_worktree(&facts),
        AgentWorktreeVerdict::Keep { .. }
    ));
}

#[test]
fn classify_no_merge_signal_agent_worktree_is_kept() {
    // TASK-878: no merge signal at all (unmerged work in flight) → never
    // remove; flag for the operator.
    let facts = AgentWorktreeFacts {
        unique_unmerged_commits: 5,
        ..agent_wt_facts()
    };
    assert!(matches!(
        classify_agent_worktree(&facts),
        AgentWorktreeVerdict::Keep { .. }
    ));
}

// ----- BUG-614: conservative worktree GC predicate -----------------------

/// The all-safe baseline: agent-managed, merged-or-gone, clean, unlocked,
/// no active lease → the only combination the GC will remove.
fn gc_eligible_facts() -> WorktreeGcFacts {
    WorktreeGcFacts {
        is_agent_managed: true,
        merged_or_gone: true,
        dirty: false,
        locked: false,
        has_active_lease: false,
    }
}

#[test]
fn gc_merged_clean_unlocked_no_lease_is_eligible() {
    // BUG-614: all four safety gates pass → Eligible.
    assert_eq!(
        classify_worktree_gc(&gc_eligible_facts()),
        WorktreeGcVerdict::Eligible
    );
}

#[test]
fn gc_dirty_worktree_is_preserved() {
    // BUG-614: uncommitted work is unambiguously "has work" — preserved even
    // when its branch merged.
    let facts = WorktreeGcFacts {
        dirty: true,
        ..gc_eligible_facts()
    };
    assert!(matches!(
        classify_worktree_gc(&facts),
        WorktreeGcVerdict::Preserve(_)
    ));
}

#[test]
fn gc_locked_worktree_is_preserved() {
    // BUG-614: a locked worktree is operator-protected — never removed.
    let facts = WorktreeGcFacts {
        locked: true,
        ..gc_eligible_facts()
    };
    assert!(matches!(
        classify_worktree_gc(&facts),
        WorktreeGcVerdict::Preserve(_)
    ));
}

#[test]
fn gc_unmerged_branch_worktree_is_preserved() {
    // BUG-614: a branch that is NOT merged into the default branch carries
    // unmerged work — preserved.
    let facts = WorktreeGcFacts {
        merged_or_gone: false,
        ..gc_eligible_facts()
    };
    assert!(matches!(
        classify_worktree_gc(&facts),
        WorktreeGcVerdict::Preserve(_)
    ));
}

#[test]
fn gc_active_lease_worktree_is_preserved() {
    // BUG-614: a live process / active session-lease pins the worktree —
    // never removed, even merged + clean.
    let facts = WorktreeGcFacts {
        has_active_lease: true,
        ..gc_eligible_facts()
    };
    assert!(matches!(
        classify_worktree_gc(&facts),
        WorktreeGcVerdict::Preserve(_)
    ));
}

#[test]
fn gc_non_agent_worktree_is_preserved() {
    // BUG-614: the GC only ever touches agent-managed worktrees; a normal
    // work-branch worktree is preserved regardless of the other facts.
    let facts = WorktreeGcFacts {
        is_agent_managed: false,
        ..gc_eligible_facts()
    };
    assert!(matches!(
        classify_worktree_gc(&facts),
        WorktreeGcVerdict::Preserve(_)
    ));
}

#[test]
fn gc_dirty_takes_priority_in_reason_over_unmerged() {
    // BUG-614: when several gates fail, the costliest objection (dirty) is
    // reported — losing uncommitted work is the worst outcome.
    let facts = WorktreeGcFacts {
        dirty: true,
        locked: true,
        merged_or_gone: false,
        has_active_lease: true,
        ..gc_eligible_facts()
    };
    match classify_worktree_gc(&facts) {
        WorktreeGcVerdict::Preserve(reason) => assert!(
            reason.contains("uncommitted"),
            "dirty objection must win, got: {reason}"
        ),
        other => panic!("expected Preserve, got {other:?}"),
    }
}

#[test]
fn gc_worktree_is_active_matches_self_and_descendants() {
    // BUG-614: an active path equal to the worktree OR beneath it pins it.
    let tmp = tempfile::TempDir::new().unwrap();
    let wt = tmp.path().join("agent-abc");
    std::fs::create_dir_all(wt.join("nested")).unwrap();
    let wt_canon = wt.canonicalize().unwrap();
    let nested_canon = wt.join("nested").canonicalize().unwrap();

    // Exact match.
    let mut active = HashSet::new();
    active.insert(wt_canon.clone());
    assert!(worktree_is_active(&wt, &active));

    // Descendant (a live claude one level down) still pins it.
    let mut active = HashSet::new();
    active.insert(nested_canon);
    assert!(worktree_is_active(&wt, &active));

    // Unrelated path does not pin it.
    let other = tmp.path().join("agent-other");
    std::fs::create_dir_all(&other).unwrap();
    let mut active = HashSet::new();
    active.insert(other.canonicalize().unwrap());
    assert!(!worktree_is_active(&wt, &active));
}

#[test]
fn is_agent_managed_worktree_matches_path_and_branch() {
    // TASK-878: scoped to the Agent-tool isolation worktrees by EITHER the
    // path segment or the branch convention.
    use std::path::Path;
    assert!(is_agent_managed_worktree(
        Path::new("/repo/.claude/worktrees/agent-abc123"),
        Some("worktree-agent-abc123"),
    ));
    // Path-only match (branch missing / detached).
    assert!(is_agent_managed_worktree(
        Path::new("/repo/.claude/worktrees/agent-deadbeef"),
        None,
    ));
    // Branch-only match (oddly-pathed but conventional branch).
    assert!(is_agent_managed_worktree(
        Path::new("/tmp/scratch"),
        Some("worktree-agent-9f9f"),
    ));
    // A normal work-branch worktree is NOT in scope.
    assert!(!is_agent_managed_worktree(
        Path::new("/repo/wt/task-281"),
        Some("task-281-foo"),
    ));
}

#[test]
fn merged_agent_worktree_heal_is_gated_in_autonomous_context() {
    // TASK-878 + STORY-666: a merged-agent-worktree finding is destructive
    // (safe_heal=false), so the per-category disposition GATES it in an
    // autonomous context even with --force --yes, and PROCEEDS interactively.
    // matches the finding's classification (destructive → never safe_heal)
    let safe_heal = false;
    // Unattended (autonomous) → fail-closed refusal.
    assert_eq!(
        doctor_heal_disposition(safe_heal, /*force*/ true, /*yes*/ true, /*auto*/ true),
        HealDisposition::GateAutonomous,
    );
    // Interactive TTY sign-off → proceeds.
    assert_eq!(
        doctor_heal_disposition(safe_heal, /*force*/ true, /*yes*/ true, /*auto*/ false),
        HealDisposition::Proceed,
    );
    // Without --force --yes it's the pre-existing manual-decision skip.
    assert_eq!(
        doctor_heal_disposition(safe_heal, /*force*/ false, /*yes*/ false, /*auto*/ false),
        HealDisposition::SkipNeedsForce,
    );
}

#[test]
fn spec_id_from_work_branch_derives_spec_id() {
    // TASK-717
    assert_eq!(
        spec_id_from_work_branch("task-281-foo"),
        Some("TASK-281".to_string())
    );
    assert_eq!(
        spec_id_from_work_branch("task-1-127"),
        Some("TASK-1-127".to_string())
    );
    assert_eq!(
        spec_id_from_work_branch("task-1-127-fix"),
        Some("TASK-1-127".to_string())
    );
    assert_eq!(
        spec_id_from_work_branch("bug-100"),
        Some("BUG-100".to_string())
    );
    assert_eq!(
        spec_id_from_work_branch("STORY-86"),
        Some("STORY-86".to_string())
    );
    // Non-work branches yield nothing.
    assert_eq!(spec_id_from_work_branch("spock-dev"), None);
    assert_eq!(spec_id_from_work_branch("pr-271"), None);
}

#[test]
fn doctor_detects_and_reaps_dead_agent_registry_entry() {
    // STORY-496: a registry entry whose pid is dead is reported under
    // `dead-agents` and reaped by the heal.
    let tmp = tempfile::TempDir::new().unwrap();
    let project = tmp.path().to_path_buf();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    let dead_pid = 4_294_967_294u32; // far above any real pid → never alive
    assert!(!process_probe::pid_is_alive(dead_pid));
    agent_registry::register_spawned_agent(
        &project,
        "claude",
        dead_pid,
        None,
        None,
        project.clone(),
        None,
        None,
        None,
        None,
        None,
    )
    .unwrap();

    // Detect.
    let store = aida_core::models::RequirementsStore::new();
    let findings = collect_doctor_findings(&project, &store, Some("dead-agents")).unwrap();
    assert_eq!(findings.len(), 1, "one dead-agent finding");
    assert_eq!(findings[0].category, "dead-agents");
    assert_eq!(findings[0].id, format!("claude#{dead_pid}"));
    assert!(findings[0].safe_heal);

    // Heal.
    let result = heal_doctor_dead_agent(&project, &findings[0]).unwrap();
    assert_eq!(result.status, "healed");

    // Reaped → no longer reported.
    let after = collect_doctor_findings(&project, &store, Some("dead-agents")).unwrap();
    assert!(after.is_empty(), "reaped entry must not reappear");
}

#[test]
fn heal_doctor_lease_is_idempotent_when_lease_already_gone() {
    // BUG-471: a stale-lease finding whose lease no longer exists (ended
    // earlier in the same heal run, or by a concurrent session) must heal to
    // a no-op skip — NOT an error that aborts the whole heal.
    let tmp = tempfile::TempDir::new().unwrap();
    let project = tmp.path().to_path_buf();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    let finding = DoctorFinding {
        category: "stale-leases".to_string(),
        id: "deadbeef0000".to_string(),
        summary: "stale lease".to_string(),
        action: "end lease deadbeef0000".to_string(),
        safe_heal: true,
    };
    let result = heal_doctor_lease(&project, &finding).expect("absent lease must not error");
    assert_eq!(result.status, "skipped");
    assert!(result
        .detail
        .as_deref()
        .unwrap_or_default()
        .contains("already ended"));
}

#[test]
fn doctor_heal_reaps_stale_lease_before_spec_status_bump() {
    // BUG-1136: a single all-category heal must converge. The original
    // findings can contain both "Approved + lease => bump" and
    // "stale lease => reap"; after reaping, the stale status action is no
    // longer valid and must not leave the spec In Progress with no lease.
    let tmp = tempfile::TempDir::new().unwrap();
    let project = tmp.path();
    std::fs::create_dir_all(project.join(".aida").join("sessions")).unwrap();
    std::fs::create_dir_all(project.join(".aida-store")).unwrap();

    let storage = crate::test_env::home_fixture_storage(project.join(".aida-store"));
    let mut req = Requirement::new("lease/status drift".into(), String::new());
    req.spec_id = Some("TASK-1207".into());
    req.status = RequirementStatus::Approved;
    let mut store = aida_core::models::RequirementsStore::new();
    store.requirements = vec![req];
    storage.save(&store).unwrap();

    std::fs::write(
        project
            .join(".aida")
            .join("sessions")
            .join("deadbeef0000.toml"),
        r#"
id = "deadbeef0000"
scope = "TASK-1207"
slug = "task-1207"
owner = "tester"
worktree_path = ""
branch = "bug-1136"
started_at = "2026-09-13T00:00:00Z"
hostname = "localhost"
"#,
    )
    .unwrap();

    let stale_lease = DoctorFinding {
        category: "stale-leases".to_string(),
        id: "deadbeef0000".to_string(),
        summary: "deadbeef0000 owns TASK-1207".to_string(),
        action: "save patch if dirty, then end lease deadbeef0000".to_string(),
        safe_heal: true,
    };
    let stale_status_bump = DoctorFinding {
        category: "spec-status-drift".to_string(),
        id: "TASK-1207".to_string(),
        summary: "TASK-1207 is Approved but lease deadbeef0000 is active".to_string(),
        action: "bump spec to In Progress".to_string(),
        safe_heal: true,
    };

    let results = heal_doctor_findings(
        project,
        &[stale_status_bump, stale_lease],
        &DoctorRunOptions {
            heal: true,
            yes: true,
            category: None,
            json: false,
            force: false,
            all: false,
            since: None,
            fail_on_findings: false,
            quiet_output: false,
        },
        None,
    )
    .unwrap();

    assert_eq!(results[0].category, "stale-leases");
    assert_eq!(results[1].category, "spec-status-drift");
    assert_eq!(results[1].status, "skipped");
    assert!(!project
        .join(".aida")
        .join("sessions")
        .join("deadbeef0000.toml")
        .exists());
    let reloaded = storage.load().unwrap();
    assert_eq!(reloaded.requirements[0].status, RequirementStatus::Approved);
}

#[test]
fn doctor_check_and_heal_subcommands_parse() {
    let check = crate::TEST_CLI
        .try_parse_from(["aida", "doctor", "check", "OBE-briefs", "--json", "--all"])
        .unwrap();
    let crate::doctor_cmd::DoctorArgs {
        cmd:
            Some(cli::DoctorCommand::Check {
                category,
                all,
                json,
                ..
            }),
        ..
    } = check.command.registered::<crate::doctor_cmd::DoctorArgs>()
    else {
        panic!("expected doctor check");
    };
    assert_eq!(category, "OBE-briefs");
    assert!(all);
    assert!(json);

    let heal = crate::TEST_CLI
        .try_parse_from([
            "aida",
            "doctor",
            "heal",
            "orphan-branches",
            "--yes",
            "--force",
            "--all",
        ])
        .unwrap();
    let crate::doctor_cmd::DoctorArgs {
        cmd:
            Some(cli::DoctorCommand::Heal {
                category,
                yes,
                force,
                all,
                json,
            }),
        ..
    } = heal.command.registered::<crate::doctor_cmd::DoctorArgs>()
    else {
        panic!("expected doctor heal");
    };
    assert_eq!(category, "orphan-branches");
    assert!(yes);
    assert!(force);
    assert!(all);
    assert!(!json);
}

// trace:TASK-1124 — codex_prompts_drift flags a stale deployed prompt,
// ignores a matching one, and never nags about a prompt not deployed.
#[test]
fn codex_prompts_drift_flags_only_stale_deployed_prompts() {
    let dir = tempfile::tempdir().unwrap();
    let expected = aida_core::scaffolding::codex_prompts::expected_codex_prompts();
    assert!(expected.len() >= 2, "need a couple of prompts to test");
    // Deploy the first prompt correctly (matches source).
    let (fresh_name, fresh_body) = &expected[0];
    std::fs::write(
        dir.path().join(format!("{fresh_name}.md")),
        fresh_body.replace('\n', "\r\n"),
    )
    .unwrap();
    // Deploy the second prompt STALE (content differs).
    let (stale_name, _) = &expected[1];
    std::fs::write(
        dir.path().join(format!("{stale_name}.md")),
        "stale content\n",
    )
    .unwrap();
    // Every other prompt is never deployed (missing) — must NOT be flagged.

    let drifted = codex_prompts_drift(dir.path());
    assert!(
        drifted.contains(stale_name),
        "stale prompt must be flagged: {drifted:?}"
    );
    assert!(
        !drifted.contains(fresh_name),
        "a matching prompt must NOT be flagged: {drifted:?}"
    );
    assert_eq!(
        drifted.len(),
        1,
        "only the one stale prompt should be flagged (missing = opt-out): {drifted:?}"
    );
}

// trace:BUG-1095 — Codex CLI 0.142 ignores ~/.codex/prompts as a custom
// slash-command surface, so doctor must catch generated prompt dirs that
// users might reasonably expect to expose /aida-* commands.
#[test]
fn codex_prompt_dir_warning_is_version_gated() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("aida-capture.md"), "prompt body\n").unwrap();

    assert!(
        codex_ignores_prompt_dir_finding_for_version(dir.path(), (0, 141, 9)).is_none(),
        "older Codex versions stay quiet because the field report only verifies 0.142+"
    );
    let finding = codex_ignores_prompt_dir_finding_for_version(dir.path(), (0, 142, 0)).unwrap();
    assert_eq!(finding.id, "scaffold-drift/codex-prompts-undiscovered");
    assert!(finding.summary.contains("0.142.0"));
    assert!(finding.summary.contains("does not discover"));
    assert!(finding.action.contains("Prune"));
    assert!(finding.action.contains(".aida-bak"));
    assert!(finding.action.contains("$aida-*"));
    assert!(finding.action.contains("/skills"));
}

#[test]
fn codex_prompt_dir_warning_requires_aida_prompt_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.md"), "not an AIDA prompt\n").unwrap();
    assert!(
        codex_ignores_prompt_dir_finding_for_version(dir.path(), (0, 142, 0)).is_none(),
        "non-AIDA markdown in ~/.codex/prompts is not our scaffold state"
    );
}

/// A delivered-then-deleted skill is an opt-out, not missing-skill drift:
/// doctor neither flags it nor recommends `aida scaffold upgrade`.
// trace:TASK-1503 | ai:claude
#[test]
fn scaffold_drift_ignores_deleted_delivered_skill() {
    let dir = tempfile::tempdir().unwrap();
    let store = aida_core::RequirementsStore::new();
    let mut scaffolder = aida_core::scaffolding::Scaffolder::new(
        dir.path().to_path_buf(),
        ScaffoldConfig::default(),
    );
    let preview = scaffolder.preview(&store);
    scaffolder.apply(&preview).unwrap();
    std::fs::remove_dir_all(dir.path().join(".agents/skills/aida-commit")).unwrap();
    std::fs::remove_dir_all(dir.path().join(".claude/skills/aida-commit")).unwrap();

    let findings = scan_scaffold_drift(dir.path(), &store);
    assert!(
        findings
            .iter()
            .all(|f| f.id != "scaffold-drift/codex-skills-missing"),
        "{:?}",
        findings.iter().map(|f| &f.id).collect::<Vec<_>>()
    );
    let status = check_scaffold_status(
        &store,
        dir.path(),
        &ScaffoldConfig::default(),
        &aida_core::db::project_cache_path(&dir.path()),
    );
    assert!(
        status
            .missing
            .iter()
            .all(|p| !p.to_string_lossy().contains("aida-commit")),
        "{:?}",
        status.missing
    );
}

/// A pre-manifest (legacy) Codex pack with missing skills still gets the
/// `aida scaffold upgrade` hint: the explicit upgrade delivers them once.
// trace:TASK-1503 | ai:claude
#[test]
fn scaffold_drift_still_flags_legacy_pack_missing_skills() {
    let dir = tempfile::tempdir().unwrap();
    let store = aida_core::RequirementsStore::new();
    let req = dir.path().join(".codex/skills/aida-req");
    std::fs::create_dir_all(&req).unwrap();
    std::fs::write(req.join("SKILL.md"), "legacy\n").unwrap();
    let findings = scan_scaffold_drift(dir.path(), &store);
    let finding = findings
        .iter()
        .find(|f| f.id == "scaffold-drift/codex-skills-missing")
        .expect("legacy pack's missing skills are flagged");
    assert!(finding.action.contains("aida scaffold upgrade"));
}

/// BUG-1645 L2: a memory-lane project, including one from before the
/// footprint was saved, is not told to run `aida scaffold upgrade` for
/// the full skill set; a missing memory-lane skill is still flagged.
// trace:BUG-1645 | ai:claude
#[test]
fn bug_1645_doctor_does_not_nag_memory_lane_toward_full_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let store = aida_core::RequirementsStore::new();
    crate::init_cmd::write_memory_lane_scaffolding(root, &store, "test", false, false).unwrap();
    // Pre-TASK-1503 shape: no manifest; no saved footprint (pre-STORY-830).
    for pack in [".claude/skills", ".agents/skills"] {
        std::fs::remove_file(
            root.join(pack)
                .join(aida_core::scaffolding::refresh::DELIVERED_MANIFEST),
        )
        .unwrap();
    }
    let findings = scan_scaffold_drift(root, &store);
    assert!(
        findings
            .iter()
            .all(|f| f.id != "scaffold-drift/codex-skills-missing"),
        "{:?}",
        findings.iter().map(|f| &f.id).collect::<Vec<_>>()
    );

    // Losing a memory-lane skill is still reported.
    std::fs::remove_dir_all(root.join(".agents/skills/aida-learn")).unwrap();
    let findings = scan_scaffold_drift(root, &store);
    let finding = findings
        .iter()
        .find(|f| f.id == "scaffold-drift/codex-skills-missing")
        .expect("a missing memory-lane skill is flagged");
    assert!(finding.summary.contains("1 missing"), "{}", finding.summary);
    // The fix must not invite the full skill set.
    assert!(
        finding
            .action
            .starts_with("Memory-lane project: do not run `aida scaffold upgrade`"),
        "{}",
        finding.action
    );
    assert!(finding.action.contains("aida scaffold extract"));
    assert_eq!(finding.action, MEMORY_LANE_SKILL_ACTION);
}

/// BUG-1653: a fresh memory-lane project (footprint saved or not) has no
/// scaffold drift: its skills carry the AIDA header the full scaffold
/// writes, so scaffold status sees them as matching.
// trace:BUG-1653 | ai:claude
#[test]
fn bug_1653_doctor_fresh_memory_lane_reports_no_drift() {
    for persist_footprint in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let store = aida_core::RequirementsStore::new();
        if persist_footprint {
            std::fs::create_dir_all(root.join(".aida")).unwrap();
            crate::init_cmd::write_init_footprint(root, crate::cli::InitFootprint::MemoryLane)
                .unwrap();
        }
        crate::init_cmd::write_memory_lane_scaffolding(root, &store, "test", false, false).unwrap();
        let status = check_scaffold_status(
            &store,
            root,
            &ScaffoldConfig::default(),
            &aida_core::db::project_cache_path(&root),
        );
        let drifted: Vec<_> = status
            .modified
            .iter()
            .map(|(p, _)| p.to_string_lossy().into_owned())
            .filter(|p| p.contains("/skills/"))
            .collect();
        assert!(
            drifted.is_empty(),
            "persist={persist_footprint}: {drifted:?}"
        );
        for pack in [".claude/skills", ".agents/skills"] {
            for name in crate::init_cmd::MEMORY_LANE_SKILLS {
                let rel = std::path::PathBuf::from(format!("{pack}/{name}/SKILL.md"));
                assert!(status.matching.contains(&rel), "{}", rel.display());
            }
        }
        let findings = scan_scaffold_drift(root, &store);
        assert!(
            findings.iter().all(|f| f.id != "scaffold-drift/project"
                && f.id != "scaffold-drift/codex-skills-missing"),
            "persist={persist_footprint}: {:?}",
            findings
                .iter()
                .map(|f| (&f.id, &f.summary))
                .collect::<Vec<_>>()
        );
    }
}

/// BUG-1653: the lane drift hint never recommends `upgrade`, which
/// installs the full skill set. Since BUG-1662 it points at refresh (which
/// keeps the project a memory lane) with a manual restore for an edited
/// skill. Refresh's header-less migration clears the finding for an
/// unedited skill.
// trace:BUG-1653 | ai:claude
#[test]
fn bug_1653_doctor_memory_lane_drift_hint_is_manual_restore() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let store = aida_core::RequirementsStore::new();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    crate::init_cmd::write_init_footprint(root, crate::cli::InitFootprint::MemoryLane).unwrap();
    crate::init_cmd::write_memory_lane_scaffolding(root, &store, "test", false, false).unwrap();
    // An older memory-lane install: header-less raw template.
    let raw = aida_core::templates::EMBEDDED_TEMPLATES
        .get("skills/aida-capture.md")
        .unwrap();
    std::fs::write(root.join(".claude/skills/aida-capture/SKILL.md"), raw).unwrap();
    let findings = scan_scaffold_drift(root, &store);
    let finding = findings
        .iter()
        .find(|f| f.id == "scaffold-drift/project")
        .expect("header-less lane skill is drift");
    assert_eq!(finding.action, MEMORY_LANE_DRIFT_ACTION);
    assert!(
        finding.action.contains("aida scaffold extract"),
        "{}",
        finding.action
    );
    assert!(
        finding.action.contains("aida scaffold refresh"),
        "{}",
        finding.action
    );
    assert!(!finding.action.contains("upgrade"), "{}", finding.action);

    // The refresh migration (header-less and unedited -> wrapped) clears
    // the finding.
    crate::scaffold_refresh::refresh_agent_packs_at(root, None, None).unwrap();
    let findings = scan_scaffold_drift(root, &store);
    assert!(
        findings.iter().all(|f| f.id != "scaffold-drift/project"),
        "{:?}",
        findings.iter().map(|f| &f.id).collect::<Vec<_>>()
    );
}

/// TASK-1536: aida doctor flags a memory-lane project whose AGENTS.md
/// carries the full conventions block or a discipline pack (flooded by an
/// older refresh) and gives manual restore steps.
// trace:TASK-1536 | ai:agy
#[test]
fn task_1536_doctor_flags_flooded_memory_lane_with_manual_restore_guidance() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let store = aida_core::RequirementsStore::new();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    crate::init_cmd::write_init_footprint(root, crate::cli::InitFootprint::MemoryLane).unwrap();
    crate::init_cmd::write_memory_lane_scaffolding(root, &store, "test", false, false).unwrap();

    // 1. Clean memory-lane project has neither flooded finding.
    let findings = scan_scaffold_drift(root, &store);
    assert!(
        findings
            .iter()
            .all(|f| f.id != "scaffold-drift/memory-lane-discipline"
                && f.id != "scaffold-drift/memory-lane-agents-conventions"),
        "{:?}",
        findings.iter().map(|f| &f.id).collect::<Vec<_>>()
    );

    // 2. Flood with discipline pack (as older refresh did).
    std::fs::create_dir_all(root.join(".aida/discipline")).unwrap();
    std::fs::write(root.join(".aida/discipline/README.md"), "# Discipline\n").unwrap();
    let findings = scan_scaffold_drift(root, &store);
    let discipline_finding = findings
        .iter()
        .find(|f| f.id == "scaffold-drift/memory-lane-discipline")
        .expect("flooded discipline pack is flagged");
    assert_eq!(discipline_finding.action, MEMORY_LANE_DISCIPLINE_ACTION);
    assert!(discipline_finding
        .action
        .contains("rm -rf .aida/discipline"));
    assert!(discipline_finding.action.contains(".gitignore"));

    // 3. Flood AGENTS.md with full conventions block.
    let conventions_agents = "\
# AGENTS.md

<!-- AIDA-AUTOGEN-BEGIN -->
# AIDA Conventions

This file is the single source of truth for AIDA's coding conventions in this project.
<!-- AIDA-AUTOGEN-END -->

User content.
";
    std::fs::write(root.join("AGENTS.md"), conventions_agents).unwrap();
    let findings = scan_scaffold_drift(root, &store);
    assert!(findings
        .iter()
        .any(|f| f.id == "scaffold-drift/memory-lane-discipline"));
    let conventions_finding = findings
        .iter()
        .find(|f| f.id == "scaffold-drift/memory-lane-agents-conventions")
        .expect("flooded AGENTS.md conventions block is flagged");
    assert_eq!(conventions_finding.action, MEMORY_LANE_CONVENTIONS_ACTION);
    assert!(conventions_finding.action.contains("# AIDA Memory Lane"));

    // 4. Manually restore discipline pack: remove .aida/discipline.
    std::fs::remove_dir_all(root.join(".aida/discipline")).unwrap();
    let findings = scan_scaffold_drift(root, &store);
    assert!(findings
        .iter()
        .all(|f| f.id != "scaffold-drift/memory-lane-discipline"));
    assert!(findings
        .iter()
        .any(|f| f.id == "scaffold-drift/memory-lane-agents-conventions"));

    // 5. Manually restore AGENTS.md: replace with # AIDA Memory Lane block.
    let restored_agents = "\
# AGENTS.md

<!-- AIDA-AUTOGEN-BEGIN -->
# AIDA Memory Lane

Storage: test
<!-- AIDA-AUTOGEN-END -->

User content.
";
    std::fs::write(root.join("AGENTS.md"), restored_agents).unwrap();
    let findings = scan_scaffold_drift(root, &store);
    assert!(findings
        .iter()
        .all(|f| f.id != "scaffold-drift/memory-lane-discipline"
            && f.id != "scaffold-drift/memory-lane-agents-conventions"));

    // 6. A full-footprint project with both is NOT flagged as flooded.
    let full_dir = tempfile::tempdir().unwrap();
    let full_root = full_dir.path();
    std::fs::create_dir_all(full_root.join(".aida/discipline")).unwrap();
    std::fs::write(
        full_root.join(".aida/discipline/README.md"),
        "# Discipline\n",
    )
    .unwrap();
    std::fs::write(full_root.join("AGENTS.md"), conventions_agents).unwrap();
    crate::init_cmd::write_init_footprint(full_root, crate::cli::InitFootprint::Full).unwrap();
    let full_findings = scan_scaffold_drift(full_root, &store);
    assert!(full_findings
        .iter()
        .all(|f| f.id != "scaffold-drift/memory-lane-discipline"
            && f.id != "scaffold-drift/memory-lane-agents-conventions"));
}

/// BUG-1645 review: files behind a user-owned symlinked skill directory
/// are never written by apply/upgrade/refresh, so doctor does not report
/// them as drift (the finding would never clear).
// trace:BUG-1645 | ai:claude
#[cfg(unix)]
#[test]
fn bug_1645_doctor_treats_symlinked_skill_dir_as_user_owned() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let store = aida_core::RequirementsStore::new();
    let mut scaffolder =
        aida_core::scaffolding::Scaffolder::new(root.to_path_buf(), ScaffoldConfig::default());
    let preview = scaffolder.preview(&store);
    scaffolder.apply(&preview).unwrap();
    // A user-owned aida-pr: different SKILL.md, no examples/.
    let mine = root.join("mine/aida-pr");
    std::fs::create_dir_all(&mine).unwrap();
    std::fs::write(mine.join("SKILL.md"), "my own\n").unwrap();
    let pr = root.join(".claude/skills/aida-pr");
    std::fs::remove_dir_all(&pr).unwrap();
    std::os::unix::fs::symlink(&mine, &pr).unwrap();
    let status = check_scaffold_status(
        &store,
        root,
        &ScaffoldConfig::default(),
        &aida_core::db::project_cache_path(&root),
    );
    let flagged: Vec<_> = status
        .modified
        .iter()
        .map(|(p, _)| p)
        .chain(status.missing.iter())
        .filter(|p| p.to_string_lossy().contains("aida-pr"))
        .collect();
    assert!(flagged.is_empty(), "{flagged:?}");
    let findings = scan_scaffold_drift(root, &store);
    assert!(
        findings
            .iter()
            .all(|f| f.id != "scaffold-drift/project"
                && f.id != "scaffold-drift/codex-skills-missing"),
        "{:?}",
        findings
            .iter()
            .map(|f| (&f.id, &f.summary))
            .collect::<Vec<_>>()
    );
}

#[test]
fn scaffold_drift_flags_missing_codex_skills_with_exact_fix() {
    let dir = tempfile::tempdir().unwrap();
    let store = aida_core::RequirementsStore::new();

    let findings = scan_scaffold_drift(dir.path(), &store);
    let finding = findings
        .iter()
        .find(|f| f.id == "scaffold-drift/codex-skills-missing")
        .expect("missing .agents/skills should be flagged as scaffold drift");

    // trace:BUG-1639 | ai:claude
    assert!(finding.summary.contains(".agents/skills"));
    assert!(finding.summary.contains("$aida-*"));
    assert!(finding.action.contains("aida scaffold upgrade"));
    assert!(finding.action.contains(".agents/skills/aida-*/SKILL.md"));
    assert!(finding.action.contains("$aida-capture"));
}

/// A project whose saved agent selection names neither Codex nor
/// Antigravity is never told the portable skill pack is missing, and its
/// expected scaffold does not include that pack at all.
// trace:BUG-1639 | ai:claude
#[test]
fn scaffold_drift_skips_portable_pack_for_claude_only_selection() {
    let dir = tempfile::tempdir().unwrap();
    let store = aida_core::RequirementsStore::new();
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    std::fs::write(
        dir.path().join(".aida/config.toml"),
        "[agents]\nenabled = [\"claude\"]\n",
    )
    .unwrap();

    let findings = scan_scaffold_drift(dir.path(), &store);
    assert!(
        findings
            .iter()
            .all(|f| f.id != "scaffold-drift/codex-skills-missing"),
        "{:?}",
        findings.iter().map(|f| &f.id).collect::<Vec<_>>()
    );
    let config = crate::init_cmd::scaffold_config_for_project(dir.path());
    assert!(!config.generate_codex_skills && !config.generate_antigravity_skills);

    // Selecting Codex brings the pack (and the finding) back.
    std::fs::write(
        dir.path().join(".aida/config.toml"),
        "[agents]\nenabled = [\"claude\", \"codex\"]\n",
    )
    .unwrap();
    let findings = scan_scaffold_drift(dir.path(), &store);
    assert!(findings
        .iter()
        .any(|f| f.id == "scaffold-drift/codex-skills-missing"));
}

#[test]
fn codex_prompt_dir_warning_detects_backup_cruft() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("aida-capture.md.aida-bak"), "old prompt\n").unwrap();
    let finding = codex_ignores_prompt_dir_finding_for_version(dir.path(), (0, 154, 0)).unwrap();
    assert_eq!(finding.id, "scaffold-drift/codex-prompts-undiscovered");
    assert!(finding.action.contains(".aida-bak"));
}

#[test]
fn parse_codex_semver_accepts_common_banners() {
    assert_eq!(parse_codex_semver("codex-cli 0.142.0"), Some((0, 142, 0)));
    assert_eq!(parse_codex_semver("codex 1.2.3-beta"), Some((1, 2, 3)));
    assert_eq!(parse_codex_semver("no version here"), None);
}

#[test]
fn legacy_doctor_subcommands_still_parse() {
    let cli = crate::TEST_CLI
        .try_parse_from(["aida", "doctor", "fsck"])
        .unwrap();
    let crate::doctor_cmd::DoctorArgs {
        cmd: Some(cli::DoctorCommand::Fsck),
        ..
    } = cli.command.registered::<crate::doctor_cmd::DoctorArgs>()
    else {
        panic!("expected legacy doctor fsck");
    };
}

// trace:TASK-956 | ai:claude — the TASK-935 surface cuts hide verbs from
// `--help` with clap `hide = true` but MUST leave them dispatchable. These
// parse-tests are the bouncer: a hidden command that stops parsing would be
// a real capability regression, not just a help-text change.
#[test]
fn task_956_hidden_top_level_parents_still_dispatch() {
    // punts / worker / headless are hidden from `aida --help` but still parse.
    assert!(matches!(
        crate::TEST_CLI
            .try_parse_from(["aida", "punts", "analyze"])
            .unwrap()
            .command
            .registered::<crate::punt_done::PuntsArgs>(),
        crate::punt_done::PuntsArgs { inner: _ }
    ));
    assert!(matches!(
        crate::TEST_CLI
            .try_parse_from(["aida", "worker", "directives"])
            .unwrap()
            .command
            .registered::<crate::queue_misc::WorkerArgs>(),
        crate::queue_misc::WorkerArgs { inner: _ }
    ));
    assert!(matches!(
        crate::TEST_CLI
            .try_parse_from(["aida", "headless", "tail"])
            .unwrap()
            .command
            .registered::<crate::queue_misc::HeadlessArgs>(),
        crate::queue_misc::HeadlessArgs { inner: _ }
    ));
}

// trace:TASK-956 | ai:claude
#[test]
fn task_956_hidden_subcommands_still_dispatch() {
    // doctor verify-relationships — hidden under `doctor`, still parses.
    assert!(matches!(
        crate::TEST_CLI
            .try_parse_from(["aida", "doctor", "verify-relationships", "--repair"])
            .unwrap()
            .command
            .registered::<crate::doctor_cmd::DoctorArgs>(),
        crate::doctor_cmd::DoctorArgs {
            cmd: Some(cli::DoctorCommand::VerifyRelationships { repair: true, .. }),
            ..
        }
    ));
    // queue load — hidden under `queue`, still parses.
    assert!(matches!(
        crate::TEST_CLI
            .try_parse_from(["aida", "queue", "load"])
            .unwrap()
            .command
            .registered::<crate::queue_cmd::QueueArgs>(),
        crate::queue_cmd::QueueArgs {
            inner: cli::QueueCommand::Load { .. }
        }
    ));
    // role repair — hidden under `role`, still parses.
    assert!(matches!(
        crate::TEST_CLI
            .try_parse_from(["aida", "role", "repair"])
            .unwrap()
            .command
            .registered::<crate::role_cmd::RoleArgs>(),
        crate::role_cmd::RoleArgs {
            inner: cli::RoleCommand::Repair { .. }
        }
    ));
}

// trace:TASK-956 | ai:claude — the two "merge" twins keep their own
// distinct behavior (they are NOT collapsed into the canonical sibling's
// logic, which would drop flags / change the output sink) but are hidden
// from their parent's `--help`. The canonical sibling stays visible.
#[test]
fn task_956_merged_twins_still_dispatch_their_own_logic() {
    // changelog generate — hidden, but keeps its stdout/range surface
    // distinct from the canonical `changelog refresh`.
    assert!(matches!(
        crate::TEST_CLI
            .try_parse_from(["aida", "changelog", "generate", "--since", "v0.7.0"])
            .unwrap()
            .command
            .registered::<crate::changelog_cmd::ChangelogArgs>(),
        crate::changelog_cmd::ChangelogArgs {
            inner: cli::ChangelogCommand::Generate { since: Some(_), .. }
        }
    ));
    // The canonical `changelog refresh` is unchanged and still parses.
    assert!(matches!(
        crate::TEST_CLI
            .try_parse_from(["aida", "changelog", "refresh"])
            .unwrap()
            .command
            .registered::<crate::changelog_cmd::ChangelogArgs>(),
        crate::changelog_cmd::ChangelogArgs {
            inner: cli::ChangelogCommand::Refresh { .. }
        }
    ));
    // doctor fsck — hidden, but keeps its own full-suite logic distinct
    // from the canonical `doctor check` (which requires a category arg).
    assert!(matches!(
        crate::TEST_CLI
            .try_parse_from(["aida", "doctor", "fsck"])
            .unwrap()
            .command
            .registered::<crate::doctor_cmd::DoctorArgs>(),
        crate::doctor_cmd::DoctorArgs {
            cmd: Some(cli::DoctorCommand::Fsck),
            ..
        }
    ));
}

#[test]
fn doctor_category_aliases_normalize() {
    assert_eq!(normalize_doctor_category("leases").unwrap(), "stale-leases");
    assert_eq!(
        normalize_doctor_category("OBE_briefs").unwrap(),
        "OBE-briefs"
    );
    assert_eq!(normalize_doctor_category("locks").unwrap(), "stale-locks");
    assert_eq!(
        normalize_doctor_category("agent-wiring").unwrap(),
        "agents-wiring"
    );
    assert_eq!(
        normalize_doctor_category("container-gitdir").unwrap(),
        "worktree-container-gitdir"
    );
    assert!(normalize_doctor_category("not-a-category").is_err());
}

// BUG-1554: the user-facing "valid categories" list in the
// unknown-category error was a hand-typed string literal that silently
// fell behind the normalizer's alias table — at the tree this bug was
// filed against, `external-import-bleed` and `project-manifest` both
// dispatched and normalized correctly but were absent from the error
// message, so a user who mistyped one of those names got an
// authoritative-looking list that didn't contain the name they wanted.
//
// This asserts the two are structurally impossible to omit going
// forward: the error message is built by mapping over
// `DOCTOR_CATEGORY_ALIASES`, the same table `normalize_doctor_category`
// matches against, so every canonical name is guaranteed present with
// no second edit.
// trace:BUG-1554 | ai:claude
#[test]
fn unknown_doctor_category_error_lists_every_canonical_category() {
    let err = normalize_doctor_category("not-a-real-category")
        .unwrap_err()
        .to_string();
    for (_, canonical) in DOCTOR_CATEGORY_ALIASES {
        assert!(
            err.contains(canonical),
            "error message missing canonical category `{canonical}`: {err}"
        );
    }
    // The two categories BUG-1554 found missing at HEAD 21c393f4da.
    assert!(err.contains("external-import-bleed"));
    assert!(err.contains("project-manifest"));
}

#[test]
fn doctor_reports_worktree_container_gitdir() {
    // BUG-915
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(root.join(".aida-store")).unwrap();
    std::fs::write(root.join("Dockerfile"), "FROM scratch\n").unwrap();

    let run = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    std::fs::create_dir_all(&root).unwrap();
    crate::test_git::git_command()
        .arg("init")
        .arg(&root)
        .output()
        .unwrap();
    run(&["config", "user.name", "Test"]);
    run(&["config", "user.email", "test@example.com"]);
    run(&["add", "Dockerfile"]);
    run(&["commit", "-m", "init"]);

    let worktree = tmp.path().join("repo-bug915");
    run(&[
        "worktree",
        "add",
        worktree.to_str().unwrap(),
        "-b",
        "bug915",
    ]);

    let findings = collect_doctor_findings(
        &root,
        &aida_core::RequirementsStore::default(),
        Some("worktree-container-gitdir"),
    )
    .unwrap();
    assert_eq!(findings.len(), 1);
    let f = &findings[0];
    assert_eq!(f.category, "worktree-container-gitdir");
    assert!(f.summary.contains("Dockerfile"), "{:?}", f.summary);
    assert!(f.summary.contains(".git/worktrees"), "{:?}", f.summary);
    assert!(f.action.contains("mount"), "{:?}", f.action);
}

#[test]
fn doctor_reports_antigravity_wiring_gaps_when_profile_enabled() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let home = tmp.path().join("home");
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_HOME", home.to_str().unwrap()),
        ("AIDA_TEST_ANTIGRAVITY_BINARY", "1"),
        ("AIDA_TEST_ANTIGRAVITY_MCP_REGISTERED", "0"),
    ]);
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[agents]\nenabled = [\"antigravity\"]\n",
    )
    .unwrap();

    let findings = collect_doctor_findings(
        root,
        &aida_core::models::RequirementsStore::new(),
        Some("agents-wiring"),
    )
    .unwrap();

    let ids: std::collections::HashSet<&str> = findings.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(findings.len(), 2, "{findings:?}");
    assert!(ids.contains("antigravity/agents-md"));
    assert!(ids.contains("antigravity/mcp"));
    assert!(findings.iter().any(|f| {
            f.action
                == "antigravity --add-mcp '{\"name\":\"aida\",\"command\":\"aida\",\"args\":[\"mcp-serve\"]}'"
        }));
}

// ---- TASK-673: completed-without-commit integrity tripwire ----

#[test]
fn normalize_doctor_category_accepts_completed_without_commit() {
    for alias in [
        "completed-without-commit",
        "completed-no-commit",
        "uncorroborated-completed",
        "integrity",
    ] {
        assert_eq!(
            normalize_doctor_category(alias).unwrap(),
            "completed-without-commit"
        );
    }
}

#[test]
fn doctor_category_selected_honours_filter() {
    // No filter selects everything.
    assert!(doctor_category_selected(None, "completed-without-commit").unwrap());
    // Matching (including via alias) selects.
    assert!(doctor_category_selected(Some("integrity"), "completed-without-commit").unwrap());
    // Non-matching filter excludes.
    assert!(!doctor_category_selected(Some("stale-leases"), "completed-without-commit").unwrap());
    // Unknown filter is an error, not a silent false.
    assert!(doctor_category_selected(Some("bogus"), "completed-without-commit").is_err());
}

#[test]
fn parse_trace_id_token_extracts_spec_id() {
    assert_eq!(
        parse_trace_id_token("trace:TASK-673"),
        Some("TASK-673".into())
    );
    assert_eq!(
        parse_trace_id_token("trace:TASK-673"),
        Some("TASK-673".to_string()),
        "bare ids must keep the byte-identical normalized form"
    );
    assert_eq!(
        parse_trace_id_token("trace:TASK-673."),
        Some("TASK-673".to_string()),
        "sentence punctuation after a bare id must not become an empty criterion suffix"
    );
    assert_eq!(
        parse_trace_id_token("trace:TASK-673,"),
        Some("TASK-673".to_string())
    );
    assert_eq!(
        parse_trace_id_token("trace:TASK-673 | ai:claude"),
        Some("TASK-673".to_string())
    );
    // git grep on a ref may keep a `ref:file:` prefix — take the last trace:.
    assert_eq!(
        parse_trace_id_token("main:src/a.rs:trace:STORY-86"),
        Some("STORY-86".into())
    );
    // Hierarchical ids (FR-1-042) survive.
    assert_eq!(
        parse_trace_id_token("trace:FR-1-042"),
        Some("FR-1-042".into())
    );
    // Criterion-suffixed ids ride the same parser for test traces.
    assert_eq!(
        parse_trace_id_token("trace:STORY-1178.A1"),
        Some("STORY-1178.A1".into())
    );
    assert_eq!(
        parse_trace_id_token("trace:STORY-1178.ac1a2b3"),
        Some("STORY-1178.AC1A2B3".into())
    );
    // Lower-case input normalizes up.
    assert_eq!(parse_trace_id_token("trace:task-9"), Some("TASK-9".into()));
    // Malformed (no hyphen / leading digit) → None.
    assert_eq!(parse_trace_id_token("trace:nope"), None);
    assert_eq!(parse_trace_id_token("no trace here"), None);
}

/// Helper: a code repo on `main` plus an attached `.aida-store` whose specs
/// are saved through `Storage`. Returns (project_root tempdir, storage).
fn integrity_fixture() -> (tempfile::TempDir, Storage) {
    fn g(root: &std::path::Path, args: &[&str]) {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .expect("git");
        assert!(out.status.success(), "git {:?} failed", args);
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    g(root, &["init", "-q", "-b", "main"]);
    g(root, &["config", "user.email", "t@t.t"]);
    g(root, &["config", "user.name", "t"]);
    std::fs::create_dir_all(root.join(".aida-store")).unwrap();
    let storage = crate::test_env::home_fixture_storage(root.join(".aida-store"));
    (tmp, storage)
}

fn completed_spec(spec_id: &str) -> Requirement {
    let mut req = Requirement::new(format!("work for {spec_id}"), String::new());
    req.spec_id = Some(spec_id.to_string());
    req.status = RequirementStatus::Completed;
    req
}

#[test]
fn integrity_flags_completed_spec_without_any_reference() {
    let (tmp, storage) = integrity_fixture();
    let root = tmp.path();
    let g = |args: &[&str]| {
        crate::test_git::git_command()
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
    };
    // A real commit corroborates TASK-100; TASK-200 has no commit at all.
    std::fs::write(root.join("a.txt"), "x").unwrap();
    g(&["add", "."]);
    g(&["commit", "-qm", "feat: do the thing (TASK-100)"]);

    let mut store = aida_core::models::RequirementsStore::new();
    store.requirements = vec![completed_spec("TASK-100"), completed_spec("TASK-200")];
    storage.save(&store).unwrap();

    let findings = scan_completed_without_commit(root, &store, None);
    let ids: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["TASK-200"],
        "only the uncorroborated spec flagged"
    );
    assert_eq!(findings[0].category, "completed-without-commit");
    assert!(!findings[0].safe_heal, "re-open is never a safe auto-fix");
}

#[test]
fn integrity_trace_comment_corroborates() {
    let (tmp, storage) = integrity_fixture();
    let root = tmp.path();
    let g = |args: &[&str]| {
        crate::test_git::git_command()
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
    };
    // No (SPEC-ID) in the subject, but a tracked file carries the trace.
    std::fs::write(
        root.join("lib.rs"),
        "// trace:TASK-300 | ai:claude\nfn f() {}\n",
    )
    .unwrap();
    g(&["add", "."]);
    g(&["commit", "-qm", "chore: scaffold"]);

    let mut store = aida_core::models::RequirementsStore::new();
    store.requirements = vec![completed_spec("TASK-300")];
    storage.save(&store).unwrap();

    let findings = scan_completed_without_commit(root, &store, None);
    assert!(
        findings.is_empty(),
        "a tracked // trace:SPEC-ID is corroboration, expected no findings, got {findings:?}"
    );
}

#[test]
fn integrity_ignores_aida_store_bookkeeping_commits() {
    // The orphan-store-style bare "update TASK-NNN" subject must NOT count
    // as a reference — it always exists and would mask every violation.
    let (tmp, storage) = integrity_fixture();
    let root = tmp.path();
    let g = |args: &[&str]| {
        crate::test_git::git_command()
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
    };
    std::fs::write(root.join("a.txt"), "x").unwrap();
    g(&["add", "."]);
    // Bare "update TASK-400" — no parenthesized trailer.
    g(&["commit", "-qm", "update TASK-400"]);

    let mut store = aida_core::models::RequirementsStore::new();
    store.requirements = vec![completed_spec("TASK-400")];
    storage.save(&store).unwrap();

    let findings = scan_completed_without_commit(root, &store, None);
    assert_eq!(
        findings.len(),
        1,
        "bare 'update TASK-NNN' must not corroborate"
    );
    assert_eq!(findings[0].id, "TASK-400");
}

#[test]
fn integrity_done_specs_are_not_flagged() {
    // Done is the queue's "awaiting commit" transient, surfaced elsewhere —
    // this check targets only Completed-without-corroboration.
    let (tmp, storage) = integrity_fixture();
    let root = tmp.path();
    crate::test_git::git_command()
        .arg("-C")
        .arg(root)
        .args(["commit", "--allow-empty", "-qm", "init"])
        .output()
        .unwrap();

    let mut done = completed_spec("TASK-500");
    done.status = RequirementStatus::Done;
    let mut store = aida_core::models::RequirementsStore::new();
    store.requirements = vec![done];
    storage.save(&store).unwrap();

    assert!(scan_completed_without_commit(root, &store, None).is_empty());
}

// trace:TASK-1089 — the default cutoff is the fixed git-canonical migration
// date, so pre-migration import-cohort completions are exempt by default.
#[test]
fn default_cutoff_is_the_fixed_migration_date() {
    assert_eq!(
        default_completed_without_commit_recent_cutoff().as_deref(),
        Some("2026-06-01T00:00:00Z"),
        "the default completed-without-commit cutoff must be the migration date"
    );
}

// trace:TASK-1089 — an ops/cleanup task opts out of the scan via the
// explicit `doctor:no-code` tag; other tags / no tags do not.
#[test]
fn doctor_no_code_tag_opts_a_spec_out_of_the_scan() {
    use std::collections::HashSet;
    let with_optout: HashSet<String> = ["orchestrator", "doctor:no-code"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let without: HashSet<String> = ["orchestrator"].iter().map(|s| s.to_string()).collect();
    assert!(completed_without_commit_opted_out(&with_optout));
    assert!(!completed_without_commit_opted_out(&without));
    assert!(!completed_without_commit_opted_out(&HashSet::new()));
}

#[test]
fn integrity_since_exempts_legacy_specs() {
    let (tmp, storage) = integrity_fixture();
    let root = tmp.path();
    crate::test_git::git_command()
        .arg("-C")
        .arg(root)
        .args(["commit", "--allow-empty", "-qm", "init"])
        .output()
        .unwrap();

    // TASK-1089: the legacy-exemption cutoff compares created_at, not
    // modified_at (the git-canonical migration bulk-reset modified_at to a
    // recent timestamp, defeating a modified_at cutoff). Set created_at.
    let mut legacy = completed_spec("TASK-600");
    legacy.created_at = "2020-01-01T00:00:00Z".parse().unwrap();
    let mut recent = completed_spec("TASK-601");
    recent.created_at = chrono::Utc::now();
    let mut store = aida_core::models::RequirementsStore::new();
    store.requirements = vec![legacy, recent];
    storage.save(&store).unwrap();

    // No cutoff → both flagged.
    assert_eq!(scan_completed_without_commit(root, &store, None).len(), 2);
    // Cutoff after the legacy spec → only the recent one flagged.
    let findings = scan_completed_without_commit(root, &store, Some("2023-01-01"));
    let ids: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, vec!["TASK-601"]);
}

// trace:TASK-755 | ai:codex
#[test]
fn integrity_excludes_non_code_spec_types() {
    let (tmp, storage) = integrity_fixture();
    let root = tmp.path();
    crate::test_git::git_command()
        .arg("-C")
        .arg(root)
        .args(["commit", "--allow-empty", "-qm", "init"])
        .output()
        .unwrap();

    let mut task = completed_spec("TASK-700");
    task.req_type = RequirementType::Task;
    let mut decision = completed_spec("ADR-1");
    decision.req_type = RequirementType::Decision;
    let mut principle = completed_spec("PRIN-1");
    principle.req_type = RequirementType::Principle;
    let mut term = completed_spec("TERM-1");
    term.req_type = RequirementType::Term;
    let mut constraint = completed_spec("CON-1");
    constraint.req_type = RequirementType::Constraint;
    let mut vision = completed_spec("VIS-1");
    vision.req_type = RequirementType::Vision;
    let mut doc = completed_spec("DOC-1");
    doc.req_type = RequirementType::Doc;
    let mut meta = completed_spec("META-1");
    meta.req_type = RequirementType::Meta;
    let mut folder = completed_spec("FOLDER-1");
    folder.req_type = RequirementType::Folder;
    let mut epic = completed_spec("EPIC-1");
    epic.req_type = RequirementType::Epic;
    let mut spike = completed_spec("SPIKE-1");
    spike.req_type = RequirementType::Spike;

    let mut store = aida_core::models::RequirementsStore::new();
    store.requirements = vec![
        task, decision, principle, term, constraint, vision, doc, meta, folder, epic, spike,
    ];
    storage.save(&store).unwrap();

    let findings = scan_completed_without_commit(root, &store, None);
    let ids: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, vec!["TASK-700"]);
}

// trace:TASK-755 | ai:codex
#[test]
fn integrity_collapses_historical_tail_by_default() {
    let (tmp, storage) = integrity_fixture();
    let root = tmp.path();
    crate::test_git::git_command()
        .arg("-C")
        .arg(root)
        .args(["commit", "--allow-empty", "-qm", "init"])
        .output()
        .unwrap();

    // TASK-1089: the historical-tail cutoff compares created_at, not
    // modified_at (the migration bulk-reset modified_at to a recent value).
    let mut old = completed_spec("TASK-710");
    old.created_at = "2020-01-01T00:00:00Z".parse().unwrap();
    let mut recent = completed_spec("TASK-711");
    recent.created_at = chrono::Utc::now();
    let mut store = aida_core::models::RequirementsStore::new();
    store.requirements = vec![old, recent];
    storage.save(&store).unwrap();

    let scan = scan_completed_without_commit_with_options(root, &store, Some("2023-01-01"), false);
    let ids: Vec<&str> = scan.findings.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, vec!["TASK-711"]);
    assert_eq!(scan.hidden_older, 1);
}

// trace:TASK-755 | ai:codex
#[test]
fn integrity_all_expands_historical_tail() {
    let (tmp, storage) = integrity_fixture();
    let root = tmp.path();
    crate::test_git::git_command()
        .arg("-C")
        .arg(root)
        .args(["commit", "--allow-empty", "-qm", "init"])
        .output()
        .unwrap();

    let mut old = completed_spec("TASK-720");
    old.modified_at = "2020-01-01T00:00:00Z".parse().unwrap();
    let mut recent = completed_spec("TASK-721");
    recent.modified_at = chrono::Utc::now();
    let mut store = aida_core::models::RequirementsStore::new();
    store.requirements = vec![old, recent];
    storage.save(&store).unwrap();

    let scan = scan_completed_without_commit_with_options(root, &store, Some("2023-01-01"), true);
    let ids: Vec<&str> = scan.findings.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, vec!["TASK-720", "TASK-721"]);
    assert_eq!(scan.hidden_older, 0);
}

/// BUG-1637: the completed-without-commit heal never reopens the
/// Completed spec. It reports, leaves the status, `completed_at` and the
/// history untouched, and points at the recorded human reopen.
// trace:TASK-673 trace:TASK-1477 trace:BUG-1637 | ai:claude
#[test]
fn integrity_heal_refuses_to_reopen_completed() {
    let (tmp, storage) = integrity_fixture();
    let root = tmp.path();
    let mut req = completed_spec("TASK-700");
    let stamp = chrono::Utc::now() - chrono::Duration::days(10);
    req.implementation_info = Some(aida_core::ImplementationInfo {
        completed_at: Some(stamp),
        completion_sha: Some("deadbeef".to_string()),
        ..Default::default()
    });
    let mut store = aida_core::models::RequirementsStore::new();
    store.requirements = vec![req];
    storage.save(&store).unwrap();

    let finding = DoctorFinding {
        category: "completed-without-commit".to_string(),
        id: "TASK-700".to_string(),
        summary: "Completed spec TASK-700 has no commit ...".to_string(),
        action: "re-open".to_string(),
        safe_heal: false,
    };
    let result = heal_doctor_completed_without_commit(root, &finding).unwrap();
    assert_eq!(result.status, "skipped");
    assert!(
        result.action.contains("never reopens a terminal status"),
        "{result:?}"
    );
    assert!(
        result
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("aida edit TASK-700 --status done --force"),
        "the report names the recorded human reopen: {result:?}"
    );

    let reloaded = storage.load().unwrap();
    let r = &reloaded.requirements[0];
    assert_eq!(r.status, RequirementStatus::Completed);
    assert!(r.history.is_empty(), "nothing was written");
    assert_eq!(
        r.implementation_info.as_ref().unwrap().completed_at,
        Some(stamp)
    );
}

/// BUG-1637: the doctor's status-drift heal records its move under the
/// doctor's automated author, so a terminal status the other clone reached
/// meanwhile survives the merge (both orientations). It only moves
/// Approved/In Progress, so it never leaves a terminal status itself.
// trace:BUG-1637 | ai:claude
#[test]
fn doctor_status_heal_records_automated_author_and_keeps_terminal_on_merge() {
    let (tmp, storage) = integrity_fixture();
    let root = tmp.path();
    let mut base = completed_spec("TASK-1637");
    base.status = RequirementStatus::InProgress;
    base.modified_at = chrono::Utc::now() - chrono::Duration::hours(1);
    let mut store = aida_core::models::RequirementsStore::new();
    store.requirements = vec![base.clone()];
    storage.save(&store).unwrap();

    let finding = DoctorFinding {
        category: "spec-status-drift".to_string(),
        id: "TASK-1637".to_string(),
        summary: "In Progress with no active lease".to_string(),
        action: "revert to Approved (no active lease)".to_string(),
        safe_heal: true,
    };
    let result = heal_doctor_spec_status(root, &finding).unwrap();
    assert_eq!(result.status, "healed", "{result:?}");
    let healed = storage.load().unwrap().requirements[0].clone();
    assert_eq!(healed.status, RequirementStatus::Approved);
    let entry = healed.history.last().expect("a status history entry");
    assert_eq!(entry.author, aida_core::conflict::DOCTOR_AUTHOR);
    assert!(aida_core::conflict::is_automated_status_author(
        &entry.author
    ));

    let mut completed = base.clone();
    aida_core::conflict::set_status_recorded(&mut completed, RequirementStatus::Completed, "joe");
    completed.modified_at = base.modified_at + chrono::Duration::minutes(1);
    for (ours, theirs) in [(&healed, &completed), (&completed, &healed)] {
        assert_eq!(
            aida_core::conflict::merge_spec_three_way(&base, ours, theirs).status,
            RequirementStatus::Completed
        );
    }

    // A terminal spec is never touched by this heal.
    let mut store = storage.load().unwrap();
    store.requirements[0].status = RequirementStatus::Rejected;
    storage.save(&store).unwrap();
    let again = heal_doctor_spec_status(root, &finding).unwrap();
    assert_eq!(again.status, "skipped");
    assert_eq!(
        storage.load().unwrap().requirements[0].status,
        RequirementStatus::Rejected
    );
}

/// BUG-407: `confirm_doctor_category` must NOT block on stdin in a
/// non-interactive shell (the `aida doctor --heal` hang). Under `cargo
/// test` stdin is non-interactive, so this returns Ok(false) immediately
/// and the test completing at all proves it doesn't block. Also locks the
/// contract: non-interactive declines (never silently auto-confirms).
/// The open-socket hang condition itself is verified empirically (a
// background `--heal` run with a finding present). trace:BUG-407 | ai:claude
#[test]
fn confirm_doctor_category_declines_in_non_interactive_shell() {
    assert!(!confirm_doctor_category("stale-leases", 3).unwrap());
}

// ------------------------------------------------------------------
// STORY-666: destructive `--heal` requires sign-off in autonomous
// contexts; safe fixes proceed everywhere. trace:STORY-666 | ai:claude
// ------------------------------------------------------------------

/// The pure disposition matrix — the single place the fail-closed invariant
/// lives. Locks every cell.
#[test]
fn doctor_heal_disposition_matrix() {
    use HealDisposition::*;
    // Safe fixes ALWAYS proceed — every context, regardless of flags.
    for &force in &[false, true] {
        for &yes in &[false, true] {
            for &auto in &[false, true] {
                assert_eq!(
                    doctor_heal_disposition(true, force, yes, auto),
                    Proceed,
                    "safe heal must proceed (force={force} yes={yes} auto={auto})"
                );
            }
        }
    }
    // Destructive without the --force --yes opt-in → the pre-existing
    // needs-a-decision skip, in EVERY context (unchanged).
    for &auto in &[false, true] {
        assert_eq!(
            doctor_heal_disposition(false, false, false, auto),
            SkipNeedsForce
        );
        assert_eq!(
            doctor_heal_disposition(false, true, false, auto),
            SkipNeedsForce
        );
        assert_eq!(
            doctor_heal_disposition(false, false, true, auto),
            SkipNeedsForce
        );
    }
    // Destructive + --force --yes + INTERACTIVE (TTY) = explicit human
    // sign-off → proceeds (req #3: interactive path unchanged).
    assert_eq!(doctor_heal_disposition(false, true, true, false), Proceed);
    // Destructive + --force --yes + AUTONOMOUS = fail-closed gate (the
    // invariant): the heal must NOT execute.
    assert_eq!(
        doctor_heal_disposition(false, true, true, true),
        GateAutonomous
    );
}

/// The autonomous-context detector: no TTY OR a live orchestrator.
#[test]
fn doctor_context_is_autonomous_signals() {
    // Interactive TTY, no orchestrator → attended.
    assert!(!doctor_context_is_autonomous(true, false));
    // No TTY (piped / CI) → autonomous.
    assert!(doctor_context_is_autonomous(false, false));
    // Live orchestrator drain even at a "TTY" → autonomous.
    assert!(doctor_context_is_autonomous(true, true));
    assert!(doctor_context_is_autonomous(false, true));
}

/// End-to-end: in a simulated non-interactive context (under `cargo test`
/// stdin is not a TTY, and the tempdir has no live drain → autonomous),
/// a DESTRUCTIVE finding requested with `--force --yes` is GATED — the heal
/// never executes and the spec is left untouched. This is the heart of the
/// safety rail: it FAILS against the old code (which would re-open the spec)
// and PASSES with the gate. trace:STORY-666 | ai:claude
#[test]
fn destructive_heal_is_gated_in_autonomous_context() {
    let (tmp, storage) = integrity_fixture();
    let root = tmp.path();
    let mut store = aida_core::models::RequirementsStore::new();
    store.requirements = vec![completed_spec("TASK-666")];
    storage.save(&store).unwrap();

    // `completed-without-commit` is a destructive (safe_heal=false) category
    // (its heal used to re-open a Completed spec; since BUG-1637 it only
    // reports, but the category stays gated).
    let finding = DoctorFinding {
        category: "completed-without-commit".to_string(),
        id: "TASK-666".to_string(),
        summary: "Completed spec TASK-666 has no commit".to_string(),
        action: "re-open".to_string(),
        safe_heal: false,
    };

    // --force --yes requested, but we are unattended (no TTY under test).
    let results = heal_doctor_findings(
        root,
        &[finding],
        &DoctorRunOptions {
            heal: true,
            yes: true,
            category: Some("completed-without-commit".to_string()),
            json: false,
            force: true,
            all: false,
            since: None,
            fail_on_findings: false,
            quiet_output: false,
        },
        None,
    )
    .unwrap();

    // The category was gated (skipped), not healed.
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, "skipped");
    assert!(
        results[0].action.contains("gated"),
        "expected a gate result, got: {:?}",
        results[0]
    );
    assert!(
        results[0]
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("--force --yes --category"),
        "gate detail must name the interactive resume command"
    );

    // The destructive action did NOT run: the spec is still Completed.
    let reloaded = storage.load().unwrap();
    assert_eq!(
        reloaded.requirements[0].status,
        RequirementStatus::Completed,
        "destructive heal must NOT have executed in the autonomous context"
    );
}

/// End-to-end companion: in the SAME autonomous context, a SAFE
/// (reversible) fix DOES proceed — the gate is scoped to the destructive
/// subset only and must never over-gate routine reversible fixes.
// trace:STORY-666 | ai:claude
#[test]
fn safe_heal_proceeds_in_autonomous_context() {
    let dir = tempfile::tempdir().unwrap();
    let project_root = dir.path();
    std::fs::create_dir_all(project_root.join(".aida")).unwrap();
    std::fs::create_dir_all(project_root.join(".aida-store")).unwrap();
    let storage = crate::test_env::home_fixture_storage(project_root.join(".aida-store"));
    let mut req = Requirement::new("orphaned in-progress".into(), String::new());
    req.spec_id = Some("TASK-667".into());
    req.status = RequirementStatus::InProgress;
    let mut store = aida_core::models::RequirementsStore::new();
    store.requirements = vec![req];
    storage.save(&store).unwrap();

    // `spec-status-drift` (in-progress with no lease → revert to Approved)
    // is a SAFE (safe_heal=true) reversible fix.
    let finding = DoctorFinding {
        category: "spec-status-drift".to_string(),
        id: "TASK-667".to_string(),
        summary: "TASK-667 is In Progress but no active lease holds it".to_string(),
        action: "revert spec to Approved (no active lease found)".to_string(),
        safe_heal: true,
    };

    // Note: no --force, --yes set (so the safe-category TTY confirm is
    // skipped) — and we are non-interactive (autonomous) under test.
    let results = heal_doctor_findings(
        project_root,
        &[finding],
        &DoctorRunOptions {
            heal: true,
            yes: true,
            category: Some("spec-status-drift".to_string()),
            json: false,
            force: false,
            all: false,
            since: None,
            fail_on_findings: false,
            quiet_output: false,
        },
        None,
    )
    .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].status, "healed",
        "safe reversible fix must proceed in the autonomous context, got: {:?}",
        results[0]
    );
    let reloaded = storage.load().unwrap();
    assert_eq!(reloaded.requirements[0].status, RequirementStatus::Approved);
}

// trace:STORY-1128 | ai:codex
#[test]
fn permission_posture_heal_applies_contained_writer() {
    let dir = tempfile::tempdir().unwrap();
    let finding = DoctorFinding {
        category: "permission-posture".to_string(),
        id: "codex-full-access-no-sandbox".to_string(),
        summary: "Codex full access without sandbox".to_string(),
        action: "apply contained posture".to_string(),
        safe_heal: true,
    };

    let result = heal_doctor_finding(
        dir.path(),
        &finding,
        &DoctorRunOptions {
            heal: true,
            yes: true,
            category: Some("permission-posture".to_string()),
            json: false,
            force: false,
            all: false,
            since: None,
            fail_on_findings: false,
            quiet_output: false,
        },
    )
    .unwrap();

    assert_eq!(result.status, "healed");
    let agents = std::fs::read_to_string(dir.path().join(".aida/agents.toml")).unwrap();
    let codex = std::fs::read_to_string(dir.path().join(".codex/config.toml")).unwrap();
    assert!(agents.contains("contained = true"), "{agents}");
    assert!(
        codex.contains("sandbox_mode = \"workspace-write\""),
        "{codex}"
    );
    assert!(dir.path().join(".aida/agents.toml.bak").exists());
    assert!(dir.path().join(".codex/config.toml.bak").exists());
}

#[test]
fn doctor_detects_and_heals_stale_cache_lock_info() {
    let dir = tempfile::tempdir().unwrap();
    let project_root = dir.path();
    std::fs::create_dir_all(project_root.join(".aida")).unwrap();
    std::fs::create_dir_all(project_root.join(".aida-store")).unwrap();
    let cache_path =
        aida_core::CachedGitBackend::default_cache_path(&project_root.join(".aida-store"));
    let lock_info_path = aida_core::cache_lock_info_path(&cache_path);
    // TASK-2707: the cache directory exists once the cache has been opened.
    std::fs::create_dir_all(lock_info_path.parent().unwrap()).unwrap();
    let info = aida_core::CacheLockInfo {
        pid: BUG_1644_DEAD_PID,
        command: "aida list".to_string(),
        started_at: (chrono::Utc::now() - chrono::Duration::minutes(10)).to_rfc3339(),
        user: "tester".to_string(),
        session_id: None,
        ..Default::default()
    };
    std::fs::write(&lock_info_path, serde_json::to_string(&info).unwrap()).unwrap();

    let findings = collect_doctor_findings(
        project_root,
        &aida_core::models::RequirementsStore::new(),
        Some("stale-locks"),
    )
    .unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].category, "stale-locks");
    assert!(findings[0].summary.contains("dead pid"));

    let result = heal_doctor_finding(
        project_root,
        &findings[0],
        &DoctorRunOptions {
            heal: true,
            yes: true,
            category: Some("stale-locks".to_string()),
            json: false,
            force: false,
            all: false,
            since: None,
            fail_on_findings: false,
            quiet_output: false,
        },
    )
    .unwrap();
    assert_eq!(result.status, "healed");
    assert!(!lock_info_path.exists());
}

// BUG-HOME-041: `aida doctor --heal --category stale-locks` against the
// incident's sidecar (a holder in another PID namespace, heartbeat silent
// for hours) finds and removes it with no store and no cache.
// trace:BUG-HOME-041 | ai:claude
#[test]
fn bug_home_041_bounded_heal_removes_a_foreign_ns_stale_heartbeat_holder() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::create_dir_all(root.join(".aida-store")).unwrap();
    let cache = aida_core::CachedGitBackend::default_cache_path(&root.join(".aida-store"));
    let path = aida_core::cache_lock_info_path(&cache);
    // TASK-2707: the cache directory exists once the cache has been opened.
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let seen = (chrono::Utc::now() - chrono::Duration::hours(10)).to_rfc3339();
    let info = aida_core::CacheLockInfo {
        pid: 19,
        pid_ns: Some("not-a-local-ns".into()),
        command: "aida cache refresh --worker".into(),
        started_at: seen.clone(),
        heartbeat_at: Some(seen),
        expected_duration_secs: Some(120),
        ..Default::default()
    };
    std::fs::write(&path, serde_json::to_string(&info).unwrap()).unwrap();
    let opts = DoctorRunOptions {
        heal: true,
        yes: true,
        category: Some("stale-locks".to_string()),
        ..Default::default()
    };
    let budget = std::time::Duration::from_secs(20);
    let report =
        doctor_stale_locks_report(root, &opts, budget, std::time::Instant::now() + budget).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert!(report.findings[0].summary.contains("another PID namespace"));
    assert_eq!(report.healed.len(), 1, "{:?}", report.healed);
    assert_eq!(report.healed[0].status, "healed", "{:?}", report.healed);
    assert!(!path.exists());
    assert!(stale_locks_timed_out(root, "healing", budget)
        .to_string()
        .contains("timed out after 20s healing the cache lock-info"));
}

// BUG-HOME-041: the deadline helper abandons a parked worker and the
// budget reads AIDA_STALE_LOCKS_DEADLINE_SECS. No real waiting: the
// parked worker's deadline is already now.
// trace:BUG-HOME-041 | ai:claude
#[test]
fn bug_home_041_stale_locks_deadline_abandons_a_parked_step() {
    let now = std::time::Instant::now();
    let (_never, parked) = std::sync::mpsc::channel::<()>();
    assert_eq!(
        within_deadline(now, now, move || parked.recv().ok()),
        None,
        "a step parked past the deadline is abandoned"
    );
    assert_eq!(
        within_deadline(now + std::time::Duration::from_secs(5), now, || 42),
        Some(42)
    );
    assert_eq!(stale_locks_budget(None), STALE_LOCKS_DEADLINE);
    assert_eq!(
        stale_locks_budget(Some("3")),
        std::time::Duration::from_secs(3)
    );
    assert_eq!(stale_locks_budget(Some("soon")), STALE_LOCKS_DEADLINE);
}

/// A PID that cannot name a live process: above every platform's pid_max
/// (Linux caps it at 4194304), the same value the cache_lock tests use.
/// A plausible PID such as 999_999 can belong to a live process.
// trace:BUG-1644 | ai:claude
const BUG_1644_DEAD_PID: u32 = 0x7fff_fffe;

// BUG-1644: the two BUG-52 symlink sites (session start's
// `link_worktree_runtime_state`, session end's
// `unlink_worktree_aida_runtime`) retire a DEAD owner's unshared lock-info
// left beside the worktree's symlinked cache.db and keep a LIVE owner's.
// trace:BUG-1644 | ai:claude
#[cfg(unix)]
#[test]
fn bug_1644_symlink_sites_retire_dead_stray_and_keep_live_stray() {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let main = base.join("main");
    std::fs::create_dir_all(main.join(".aida")).unwrap();
    std::fs::create_dir_all(main.join(".aida-store")).unwrap();
    // TASK-2707: a legacy main cache; linking migrates it.
    std::fs::write(main.join(".aida").join("cache.db"), b"").unwrap();
    let record = |pid: u32| {
        serde_json::to_string(&aida_core::CacheLockInfo {
            pid,
            command: "aida list (pre-BUG-1644 binary)".to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            user: "tester".to_string(),
            ..Default::default()
        })
        .unwrap()
    };

    for (label, pid, expect_kept) in [("dead", BUG_1644_DEAD_PID, false), ("live", 1, true)] {
        // Session start into a reused worktree that still holds a stray.
        let wt = base.join(format!("wt-start-{label}"));
        let stray = wt.join(".aida").join("cache.db.lock-info");
        std::fs::create_dir_all(wt.join(".aida")).unwrap();
        crate::link_worktree_runtime_state(&main, &wt).unwrap();
        // TASK-2707: one directory link to main's cache directory.
        assert!(wt.join(".aida").join("cache-db").is_symlink());
        std::fs::write(&stray, record(pid)).unwrap();
        // Re-running the start site (a reused worktree) retires it.
        crate::link_worktree_runtime_state(&main, &wt).unwrap();
        assert_eq!(stray.exists(), expect_kept, "session start, {label} owner");

        // Session end on a worktree that holds a stray.
        let wt = base.join(format!("wt-end-{label}"));
        let stray = wt.join(".aida").join("cache.db.lock-info");
        std::fs::create_dir_all(&wt).unwrap();
        crate::link_worktree_runtime_state(&main, &wt).unwrap();
        std::fs::write(&stray, record(pid)).unwrap();
        crate::unlink_worktree_aida_runtime(&wt.join(".aida"));
        assert!(!wt.join(".aida").join("cache-db").exists());
        assert_eq!(stray.exists(), expect_kept, "session end, {label} owner");
    }
    // The shared cache (migrated into its own directory) is untouched.
    assert!(aida_core::db::project_cache_path(&main).exists());
}

// BUG-1644: from a sibling worktree whose `.aida/cache.db` symlinks to the
// main checkout's cache, doctor inspects the SHARED sidecar, and also
// reports an older binary's stray per-worktree sidecar (healing it only
// when its owner is dead).
// trace:BUG-1644 | ai:claude
#[cfg(unix)]
#[test]
fn bug_1644_doctor_checks_shared_and_stray_worktree_lock_info() {
    let dir = tempfile::tempdir().unwrap();
    // Canonical root: on macOS the temp dir is under the `/var` symlink.
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let main_aida = base.join("main").join(".aida");
    let wt = base.join("wt-sibling");
    std::fs::create_dir_all(&main_aida).unwrap();
    std::fs::create_dir_all(wt.join(".aida")).unwrap();
    std::fs::create_dir_all(wt.join(".aida-store")).unwrap();
    // TASK-2707: the worktree reaches main's cache directory by one link.
    let main_cache = aida_core::db::cache_dir_in(&main_aida).join("cache.db");
    std::fs::create_dir_all(main_cache.parent().unwrap()).unwrap();
    std::fs::write(&main_cache, b"").unwrap();
    std::os::unix::fs::symlink(
        main_cache.parent().unwrap(),
        aida_core::db::cache_dir_in(&wt.join(".aida")),
    )
    .unwrap();
    let wt_cache = aida_core::CachedGitBackend::default_cache_path(&wt.join(".aida-store"));
    assert_eq!(wt_cache, aida_core::db::project_cache_path(&wt));

    let record = |pid: u32| {
        serde_json::to_string(&aida_core::CacheLockInfo {
            pid,
            command: "aida list".to_string(),
            started_at: (chrono::Utc::now() - chrono::Duration::minutes(10)).to_rfc3339(),
            user: "tester".to_string(),
            ..Default::default()
        })
        .unwrap()
    };
    let findings = |root: &std::path::Path| {
        collect_doctor_findings(
            root,
            &aida_core::models::RequirementsStore::new(),
            Some("stale-locks"),
        )
        .unwrap()
    };

    // A dead writer's sidecar in the MAIN checkout is found from the worktree.
    let shared = main_cache.with_file_name("cache.db.lock-info");
    std::fs::write(&shared, record(BUG_1644_DEAD_PID)).unwrap();
    let found = findings(&wt);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(
        std::fs::canonicalize(&found[0].id).unwrap(),
        std::fs::canonicalize(&shared).unwrap()
    );
    std::fs::remove_file(&shared).unwrap();

    // A stray per-worktree sidecar from a LIVE older binary: reported, not healed.
    let stray = wt.join(".aida").join("cache.db.lock-info");
    std::fs::write(&stray, record(1)).unwrap();
    let found = findings(&wt);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(std::path::Path::new(&found[0].id), stray.as_path());
    assert!(
        found[0]
            .summary
            .contains("unshared lock-info beside a symlinked cache"),
        "{found:?}"
    );
    assert!(!found[0].safe_heal);
    crate::retire_stray_worktree_lock_info(&wt.join(".aida"));
    assert!(stray.exists(), "a live owner's stray sidecar must be kept");

    // A DEAD owner's stray sidecar: a safe heal, and retired at the
    // worktree symlink sites.
    std::fs::write(&stray, record(BUG_1644_DEAD_PID)).unwrap();
    let found = findings(&wt);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].safe_heal && found[0].summary.contains("dead pid"));
    crate::retire_stray_worktree_lock_info(&wt.join(".aida"));
    assert!(!stray.exists());
    assert!(findings(&wt).is_empty());
}

// TASK-1484: a LIVE owner past its expected duration is reported as
// diagnostic evidence but never healed; a reused PID reads as dead.
// trace:TASK-1484 | ai:claude
// trace:TASK-1526 | ai:codex
#[test]
fn doctor_reports_unknown_foreign_lock_overrun_without_healing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::create_dir_all(root.join(".aida-store")).unwrap();
    let cache = aida_core::CachedGitBackend::default_cache_path(&root.join(".aida-store"));
    let path = aida_core::cache_lock_info_path(&cache);
    // TASK-2707: the cache directory exists once the cache has been opened.
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let info = aida_core::CacheLockInfo {
        pid: u32::MAX,
        boot_id: Some("foreign boot".into()),
        started_at: (chrono::Utc::now() - chrono::Duration::minutes(10)).to_rfc3339(),
        expected_duration_secs: Some(60),
        ..Default::default()
    };
    std::fs::write(&path, serde_json::to_vec(&info).unwrap()).unwrap();
    let findings = collect_doctor_findings(
        root,
        &aida_core::models::RequirementsStore::new(),
        Some("stale-locks"),
    )
    .unwrap();
    assert_eq!(findings.len(), 1);
    assert!(!findings[0].safe_heal);
    assert!(findings[0].summary.contains("unknown owner identity"));
    assert_eq!(
        heal_doctor_stale_lock(&findings[0]).unwrap().status,
        "skipped"
    );
    assert!(path.exists());
}

#[cfg(unix)]
#[test]
fn doctor_reports_live_lock_overrun_without_healing_it() {
    let dir = tempfile::tempdir().unwrap();
    let project_root = dir.path();
    std::fs::create_dir_all(project_root.join(".aida")).unwrap();
    std::fs::create_dir_all(project_root.join(".aida-store")).unwrap();
    let cache_path =
        aida_core::CachedGitBackend::default_cache_path(&project_root.join(".aida-store"));
    let lock_info_path = aida_core::cache_lock_info_path(&cache_path);
    // TASK-2707: the cache directory exists once the cache has been opened.
    std::fs::create_dir_all(lock_info_path.parent().unwrap()).unwrap();
    // pid 1 is always alive; a legacy (identity-less) record is PID-only.
    let info = aida_core::CacheLockInfo {
        pid: 1,
        command: "aida cache rebuild".to_string(),
        started_at: (chrono::Utc::now() - chrono::Duration::minutes(10)).to_rfc3339(),
        user: "tester".to_string(),
        expected_duration_secs: Some(60),
        phase: Some("rebuild cache".to_string()),
        ..Default::default()
    };
    std::fs::write(&lock_info_path, serde_json::to_string(&info).unwrap()).unwrap();

    let findings = collect_doctor_findings(
        project_root,
        &aida_core::models::RequirementsStore::new(),
        Some("stale-locks"),
    )
    .unwrap();
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(!findings[0].safe_heal, "a live lock is never a safe heal");
    assert!(
        findings[0].summary.contains("live pid 1"),
        "{}",
        findings[0].summary
    );
    assert!(
        findings[0].summary.contains("diagnostic only"),
        "{}",
        findings[0].summary
    );

    let result = heal_doctor_stale_lock(&findings[0]).unwrap();
    assert_eq!(result.status, "skipped");
    assert!(
        lock_info_path.exists(),
        "live owner's lock-info must remain"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn doctor_treats_reused_pid_lock_info_as_dead() {
    let dir = tempfile::tempdir().unwrap();
    let project_root = dir.path();
    std::fs::create_dir_all(project_root.join(".aida")).unwrap();
    std::fs::create_dir_all(project_root.join(".aida-store")).unwrap();
    let cache_path =
        aida_core::CachedGitBackend::default_cache_path(&project_root.join(".aida-store"));
    let lock_info_path = aida_core::cache_lock_info_path(&cache_path);
    // TASK-2707: the cache directory exists once the cache has been opened.
    std::fs::create_dir_all(lock_info_path.parent().unwrap()).unwrap();
    let info = aida_core::CacheLockInfo {
        pid: 1,
        command: "aida schedule tick --hook".to_string(),
        started_at: (chrono::Utc::now() - chrono::Duration::minutes(10)).to_rfc3339(),
        user: "tester".to_string(),
        pid_start_identity: Some("linux-starttime:18446744073709551615".to_string()),
        ..Default::default()
    };
    std::fs::write(&lock_info_path, serde_json::to_string(&info).unwrap()).unwrap();

    let findings = collect_doctor_findings(
        project_root,
        &aida_core::models::RequirementsStore::new(),
        Some("stale-locks"),
    )
    .unwrap();
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(findings[0].safe_heal);
    assert!(findings[0].summary.contains("different process"));
    let result = heal_doctor_stale_lock(&findings[0]).unwrap();
    assert_eq!(result.status, "healed");
    assert!(!lock_info_path.exists());
}

#[test]
fn doctor_heals_in_progress_without_lease_back_to_approved() {
    let dir = tempfile::tempdir().unwrap();
    let project_root = dir.path();
    std::fs::create_dir_all(project_root.join(".aida")).unwrap();
    std::fs::create_dir_all(project_root.join(".aida-store")).unwrap();
    let storage = crate::test_env::home_fixture_storage(project_root.join(".aida-store"));
    let mut req = Requirement::new("orphaned in-progress".into(), String::new());
    req.spec_id = Some("TASK-561".into());
    req.status = RequirementStatus::InProgress;
    let mut store = aida_core::models::RequirementsStore::new();
    store.requirements = vec![req];
    storage.save(&store).unwrap();

    let finding = DoctorFinding {
        category: "spec-status-drift".to_string(),
        id: "TASK-561".to_string(),
        summary: "TASK-561 is In Progress but no active lease holds it".to_string(),
        action: "revert spec to Approved (no active lease found)".to_string(),
        safe_heal: true,
    };
    let result = heal_doctor_finding(
        project_root,
        &finding,
        &DoctorRunOptions {
            heal: true,
            yes: true,
            category: Some("spec-status-drift".to_string()),
            json: false,
            force: false,
            all: false,
            since: None,
            fail_on_findings: false,
            quiet_output: false,
        },
    )
    .unwrap();

    assert_eq!(result.status, "healed");
    assert_eq!(
        result.action,
        "reverted TASK-561 from In Progress to Approved (no active lease found)"
    );
    let reloaded = storage.load().unwrap();
    assert_eq!(reloaded.requirements[0].status, RequirementStatus::Approved);
}

#[test]
fn doctor_skips_stale_in_progress_orphan_finding_after_manual_fix() {
    let dir = tempfile::tempdir().unwrap();
    let project_root = dir.path();
    std::fs::create_dir_all(project_root.join(".aida")).unwrap();
    std::fs::create_dir_all(project_root.join(".aida-store")).unwrap();
    let storage = crate::test_env::home_fixture_storage(project_root.join(".aida-store"));
    let mut req = Requirement::new("already fixed".into(), String::new());
    req.spec_id = Some("TASK-561".into());
    req.status = RequirementStatus::Approved;
    let mut store = aida_core::models::RequirementsStore::new();
    store.requirements = vec![req];
    storage.save(&store).unwrap();

    let finding = DoctorFinding {
        category: "spec-status-drift".to_string(),
        id: "TASK-561".to_string(),
        summary: "TASK-561 is In Progress but no active lease holds it".to_string(),
        action: "revert spec to Approved (no active lease found)".to_string(),
        safe_heal: true,
    };
    let result = heal_doctor_finding(
        project_root,
        &finding,
        &DoctorRunOptions {
            heal: true,
            yes: true,
            category: Some("spec-status-drift".to_string()),
            json: false,
            force: false,
            all: false,
            since: None,
            fail_on_findings: false,
            quiet_output: false,
        },
    )
    .unwrap();

    assert_eq!(result.status, "skipped");
    let reloaded = storage.load().unwrap();
    assert_eq!(reloaded.requirements[0].status, RequirementStatus::Approved);
}

// TASK-570: doctor detects orphan queue entries — rows whose backing
// spec is no longer in the store. trace:TASK-570 | ai:claude
#[test]
fn doctor_detects_orphan_queue_entries() {
    // STORY-1429: `current_user_id` reads AIDA_USER, which sibling tests
    // set under the test env lock; hold it so the queue key cannot change
    // between this test's write and the doctor's read.
    // trace:STORY-1429 | ai:claude
    let _env = crate::test_env::env_lock();
    let dir = tempfile::tempdir().unwrap();
    let project_root = dir.path();
    std::fs::create_dir_all(project_root.join(".aida")).unwrap();
    std::fs::create_dir_all(project_root.join(".aida-store")).unwrap();

    let storage = crate::test_env::home_fixture_storage(project_root.join(".aida-store"));
    let store = aida_core::models::RequirementsStore::new();
    storage.save(&store).unwrap();

    let user_id = current_user_id(None);
    let orphan_id = uuid::Uuid::new_v4();
    let entry = aida_core::models::QueueEntry {
        user_id: user_id.clone(),
        requirement_id: orphan_id,
        position: 1,
        added_by: user_id.clone(),
        note: Some("auto-queued by aida pr".into()),
        added_at: chrono::Utc::now(),
        for_role: Some("reviewer".into()),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    };
    storage.queue_add(entry).unwrap();

    let findings =
        collect_doctor_findings(project_root, &store, Some("orphan-queue-entries")).unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].category, "orphan-queue-entries");
    assert_eq!(findings[0].id, orphan_id.to_string());
    assert!(findings[0].summary.contains("position 1"));
    assert!(findings[0].summary.contains("[for:reviewer]"));
    assert!(findings[0].summary.contains("auto-queued by aida pr"));
    assert!(findings[0].safe_heal);
}

// TASK-570: --heal removes the orphan queue entry, mirroring the
// behaviour of `aida queue prune --orphaned`. Re-running on a clean
// queue is a no-op (skipped). trace:TASK-570 | ai:claude
#[test]
fn doctor_heals_orphan_queue_entries() {
    // Same AIDA_USER race as the detect test above. trace:STORY-1429 | ai:claude
    let _env = crate::test_env::env_lock();
    let dir = tempfile::tempdir().unwrap();
    let project_root = dir.path();
    std::fs::create_dir_all(project_root.join(".aida")).unwrap();
    std::fs::create_dir_all(project_root.join(".aida-store")).unwrap();

    let storage = crate::test_env::home_fixture_storage(project_root.join(".aida-store"));
    let store = aida_core::models::RequirementsStore::new();
    storage.save(&store).unwrap();

    let user_id = current_user_id(None);
    let orphan_id = uuid::Uuid::new_v4();
    let entry = aida_core::models::QueueEntry {
        user_id: user_id.clone(),
        requirement_id: orphan_id,
        position: 1,
        added_by: user_id.clone(),
        note: None,
        added_at: chrono::Utc::now(),
        for_role: Some("reviewer".into()),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    };
    storage.queue_add(entry).unwrap();

    let finding = DoctorFinding {
        category: "orphan-queue-entries".to_string(),
        id: orphan_id.to_string(),
        summary: "queue position 1 [for:reviewer] points at deleted spec".to_string(),
        action: "remove orphan queue entry (aida queue prune --orphaned)".to_string(),
        safe_heal: true,
    };
    let opts = DoctorRunOptions {
        heal: true,
        yes: true,
        category: Some("orphan-queue-entries".to_string()),
        json: false,
        force: false,
        all: false,
        since: None,
        fail_on_findings: false,
        quiet_output: false,
    };
    let result = heal_doctor_finding(project_root, &finding, &opts).unwrap();
    assert_eq!(result.status, "healed");
    assert_eq!(result.action, "removed orphan queue entry");

    let entries = storage.queue_list(&user_id, false).unwrap();
    assert!(
        entries.iter().all(|e| e.requirement_id != orphan_id),
        "orphan entry should be removed from the queue"
    );

    // Idempotent: a second heal on an already-clean queue reports skipped.
    let result2 = heal_doctor_finding(project_root, &finding, &opts).unwrap();
    assert_eq!(result2.status, "skipped");
}

// TASK-570: the orphan-queue-entries alias set normalizes the way the
// other doctor categories do. trace:TASK-570 | ai:claude
#[test]
fn doctor_orphan_queue_aliases_normalize() {
    assert_eq!(
        normalize_doctor_category("orphan-queue").unwrap(),
        "orphan-queue-entries"
    );
    assert_eq!(
        normalize_doctor_category("queue-orphans").unwrap(),
        "orphan-queue-entries"
    );
    assert_eq!(
        normalize_doctor_category("orphan_queue_entry").unwrap(),
        "orphan-queue-entries"
    );
}

#[test]
fn salvage_component_sanitizes_path_like_values() {
    assert_eq!(sanitize_salvage_component("TASK-515/../x"), "TASK-515-x");
    assert_eq!(sanitize_salvage_component("advisor role"), "advisor-role");
}

#[test]
fn salvage_worktree_patch_writes_diff_before_cleanup() {
    let project = tempfile::tempdir().unwrap();
    let worktree = tempfile::tempdir().unwrap();
    crate::test_git::git_command()
        .arg("-C")
        .arg(worktree.path())
        .arg("init")
        .output()
        .unwrap();
    crate::test_git::git_command()
        .arg("-C")
        .arg(worktree.path())
        .args(["config", "user.email", "t@example.com"])
        .output()
        .unwrap();
    crate::test_git::git_command()
        .arg("-C")
        .arg(worktree.path())
        .args(["config", "user.name", "Test"])
        .output()
        .unwrap();
    let file = worktree.path().join("notes.txt");
    std::fs::write(&file, "before\n").unwrap();
    crate::test_git::git_command()
        .arg("-C")
        .arg(worktree.path())
        .args(["add", "notes.txt"])
        .output()
        .unwrap();
    crate::test_git::git_command()
        .arg("-C")
        .arg(worktree.path())
        .args(["commit", "-m", "init"])
        .output()
        .unwrap();
    std::fs::write(&file, "after\n").unwrap();

    let patch = salvage_worktree_patch(project.path(), "TASK-515", Some("codex"), worktree.path())
        .unwrap()
        .expect("dirty worktree should produce salvage patch");
    let body = std::fs::read_to_string(&patch).unwrap();
    assert!(patch
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("TASK-515-codex-attempt-"));
    assert!(body.contains("# AIDA salvage patch"));
    assert!(body.contains("-before"));
    assert!(body.contains("+after"));
}

// BUG-696: the salvage patch must capture untracked files' CONTENT, not
// just their names — otherwise `aida doctor --heal` loses them when it
// tears down the orphan-worktree after salvage.
#[test]
fn salvage_worktree_patch_captures_untracked_file_content() {
    let project = tempfile::tempdir().unwrap();
    let worktree = tempfile::tempdir().unwrap();
    for args in [
        vec!["init"],
        vec!["config", "user.email", "t@example.com"],
        vec!["config", "user.name", "Test"],
    ] {
        crate::test_git::git_command()
            .arg("-C")
            .arg(worktree.path())
            .args(&args)
            .output()
            .unwrap();
    }
    // Commit a file, then dirty it (guarantees salvage runs) AND drop an
    // untracked file whose content must survive.
    let tracked = worktree.path().join("tracked.txt");
    std::fs::write(&tracked, "before\n").unwrap();
    crate::test_git::git_command()
        .arg("-C")
        .arg(worktree.path())
        .args(["add", "tracked.txt"])
        .output()
        .unwrap();
    crate::test_git::git_command()
        .arg("-C")
        .arg(worktree.path())
        .args(["commit", "-m", "init"])
        .output()
        .unwrap();
    std::fs::write(&tracked, "after\n").unwrap();
    std::fs::write(
        worktree.path().join("new_idea.txt"),
        "SALVAGE_ME_UNIQUE_MARKER untracked body\n",
    )
    .unwrap();

    let patch = salvage_worktree_patch(project.path(), "TASK-1", None, worktree.path())
        .unwrap()
        .expect("dirty worktree should produce salvage patch");
    let body = std::fs::read_to_string(&patch).unwrap();
    assert!(body.contains("new_idea.txt"), "untracked filename indexed");
    assert!(
        body.contains("SALVAGE_ME_UNIQUE_MARKER"),
        "untracked file CONTENT must be captured, not just the name:\n{body}"
    );
}

// ── TASK-752: legacy-store-cruft detection + guard ──

#[test]
fn normalize_doctor_category_accepts_legacy_store_cruft() {
    // TASK-752
    for alias in [
        "legacy-store-cruft",
        "legacy-store",
        "store-cruft",
        "legacy-cruft",
        "requirements-yaml",
    ] {
        assert_eq!(
            normalize_doctor_category(alias).unwrap(),
            "legacy-store-cruft"
        );
    }
}

#[test]
fn is_legacy_store_cruft_path_matches_top_level_only() {
    // TASK-752: top-level legacy artifacts match.
    assert!(is_legacy_store_cruft_path("requirements.yaml"));
    assert!(is_legacy_store_cruft_path(
        "requirements_20251206_205840.yaml"
    ));
    assert!(is_legacy_store_cruft_path("scaffold-report.html"));
    // Nested paths and unrelated files never match.
    assert!(!is_legacy_store_cruft_path(
        "tests/fixtures/requirements.yaml"
    ));
    assert!(!is_legacy_store_cruft_path("src/requirements.yaml"));
    assert!(!is_legacy_store_cruft_path("my-requirements.yaml"));
    assert!(!is_legacy_store_cruft_path("requirements.json"));
    assert!(!is_legacy_store_cruft_path("docs/scaffold-report.html"));
}

fn git752(root: &std::path::Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {:?}: {:?}", args, out);
}

/// A git-canonical project root, ready for the BUG-1734 gates: distributed
/// config plus a present `.aida-store`. Tempdir-only — nothing here reads
/// the developer's machine, which is the point: the condition this check
/// names happens to exist on one host, and a test that depended on it would
/// pass there and nowhere else.
// trace:BUG-1734 | ai:claude
fn git_canonical_root_1734(root: &std::path::Path) {
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[store]\nmode = \"distributed\"\nstore_path = \".aida-store\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".aida-store")).unwrap();
}

/// AC1: distributed mode + `requirements.db` in the root → one warning
/// naming the file and its mtime, wired through to `aida doctor`'s own
/// finding list, and never auto-healable.
// trace:BUG-1734 | ai:claude
#[test]
fn legacy_store_in_root_is_flagged_on_a_git_canonical_project() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git752(root, &["init", "-q", "-b", "main"]);
    git752(root, &["config", "user.email", "t@t.t"]);
    git752(root, &["config", "user.name", "t"]);
    git_canonical_root_1734(root);
    std::fs::write(root.join("requirements.db"), b"SQLite format 3\0").unwrap();

    let hits = detect_legacy_store_in_root(root);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].name, "requirements.db");
    assert_eq!(hits[0].len, 16);
    // AC1 names the mtime. Asserted as a shape, not a value: pinning the
    // date would only restate the clock the fixture just used.
    let when = hits[0].modified.as_deref().expect("mtime not reported");
    assert_eq!(when.len(), 10, "expected YYYY-MM-DD, got {when:?}");

    // Wiring: it reaches the finding list `aida doctor` prints, under the
    // category the alias table resolves, and is not auto-healable.
    let store = aida_core::models::RequirementsStore::new();
    let findings = collect_doctor_findings(root, &store, Some("legacy-store-in-root")).unwrap();
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].category, "legacy-store-in-root");
    assert_eq!(findings[0].id, "requirements.db");
    assert!(!findings[0].safe_heal, "must never be auto-healed");
    assert!(findings[0].summary.contains("requirements.db"));
    assert!(findings[0].summary.contains("BUG-1732"));

    // AC4's other half, mechanically: the heal dispatcher has no arm for
    // this category, so a `--heal` run reports it and leaves the file
    // alone. Asserted rather than assumed, because sharing
    // `legacy-store-cruft`'s category would have routed it into a `git rm`.
    let healed = heal_doctor_finding(root, &findings[0], &DoctorRunOptions::default()).unwrap();
    assert_eq!(healed.status, "skipped", "{healed:?}");
    assert!(
        root.join("requirements.db").exists(),
        "the heal path removed a file this check must never touch"
    );
}

/// AC2: the same project with no legacy store in its root says nothing.
// trace:BUG-1734 | ai:claude
#[test]
fn a_git_canonical_root_without_a_legacy_store_is_silent() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git_canonical_root_1734(root);
    assert!(detect_legacy_store_in_root(root).is_empty());
}

/// AC3: in legacy mode the file IS the store, so the check must be silent.
///
/// Both legacy shapes: a config that does not declare distributed mode, and
/// no `.aida/config.toml` at all. The second is what pins the gate to THIS
/// root: swapping the root-local read for `distributed_mode_declared_from`
/// climbs to the distributed parent and warns about a file that is not that
/// parent's concern.
// trace:BUG-1734 | ai:claude
#[test]
fn a_legacy_mode_root_with_a_requirements_db_is_silent() {
    let dir = tempfile::tempdir().unwrap();

    let declared = dir.path().join("declared-legacy");
    std::fs::create_dir_all(declared.join(".aida")).unwrap();
    std::fs::write(
        declared.join(".aida/config.toml"),
        "[store]\nmode = \"local\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(declared.join(".aida-store")).unwrap();
    std::fs::write(declared.join("requirements.db"), b"x").unwrap();
    assert!(
        detect_legacy_store_in_root(&declared).is_empty(),
        "a config that does not declare distributed mode must be silent"
    );

    // No config at all, nested under a distributed parent. The child gets
    // its own `.aida-store` so the second gate cannot be what makes this
    // pass: the only thing left to stop it is gate 1 reading THIS root's
    // config and finding none.
    let parent = dir.path().join("distributed-parent");
    git_canonical_root_1734(&parent);
    let child = parent.join("legacy-child");
    std::fs::create_dir_all(child.join(".aida-store")).unwrap();
    std::fs::write(child.join("requirements.db"), b"x").unwrap();
    assert!(
        detect_legacy_store_in_root(&child).is_empty(),
        "a root with no config of its own must not inherit the parent's mode"
    );
}

/// The second gate, beyond the filed acceptance: a config that declares
/// distributed mode before its store exists is mid-migration, and there the
/// legacy file may still be what actually gets read. Telling that operator
/// to relocate their live store would be worse than saying nothing.
// trace:BUG-1734 | ai:claude
#[test]
fn a_distributed_config_with_no_store_yet_is_silent() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[store]\nmode = \"distributed\"\nstore_path = \".aida-store\"\n",
    )
    .unwrap();
    std::fs::write(root.join("requirements.db"), b"x").unwrap();
    // No `.aida-store/` and no `aida-store` branch.
    assert!(detect_legacy_store_in_root(root).is_empty());
}

/// The partition with `legacy-store-cruft`: a file that detector reports
/// (a cruft-shaped name that is TRACKED) is left to it, so one file never
/// yields two findings with contradictory remedies — `git rm` there,
/// relocate here. An UNTRACKED `requirements.yaml` is invisible to
/// `git ls-files` and therefore ours.
// trace:BUG-1734 | ai:claude
#[test]
fn a_tracked_requirements_yaml_is_left_to_legacy_store_cruft() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git752(root, &["init", "-q", "-b", "main"]);
    git752(root, &["config", "user.email", "t@t.t"]);
    git752(root, &["config", "user.name", "t"]);
    git_canonical_root_1734(root);
    std::fs::write(root.join("requirements.yaml"), "legacy: store\n").unwrap();

    // Untracked → ours.
    assert_eq!(
        detect_legacy_store_in_root(root)
            .iter()
            .map(|h| h.name.clone())
            .collect::<Vec<_>>(),
        vec!["requirements.yaml".to_string()]
    );

    // Tracked → theirs, and we stand down.
    git752(root, &["add", "requirements.yaml"]);
    git752(root, &["commit", "-qm", "track it"]);
    assert!(
        detect_legacy_store_in_root(root).is_empty(),
        "a tracked cruft-shaped path must be left to legacy-store-cruft"
    );
}

/// AC4: the operator is told to relocate, and never to delete.
// trace:BUG-1734 | ai:claude
#[test]
fn the_action_says_relocate_and_never_delete() {
    let hit = LegacyStoreInRoot {
        name: "requirements.db".to_string(),
        len: 1_700_000,
        modified: Some("2026-03-18".to_string()),
    };
    let action = legacy_store_in_root_action(&hit);
    assert!(action.contains("move"), "{action}");
    assert!(action.contains("do not delete it"), "{action}");
    // Not merely "mentions delete somewhere": no imperative to delete.
    assert!(!action.contains("rm "), "{action}");
    assert!(!action.contains("delete it\n"), "{action}");

    let summary = legacy_store_in_root_summary(&hit);
    assert!(summary.contains("requirements.db"), "{summary}");
    assert!(summary.contains("1700000 bytes"), "{summary}");
    // Labelled UTC: an operator comparing against `ls` (local time) may
    // otherwise read a one-day discrepancy as a wrong answer.
    assert!(
        summary.contains("last modified 2026-03-18 UTC"),
        "{summary}"
    );
    assert!(summary.contains("does not read it"), "{summary}");
    assert!(summary.contains("BUG-1732"), "{summary}");

    // The mtime is optional, and its absence must not leave a dangling
    // clause in the middle of the sentence.
    let no_mtime = LegacyStoreInRoot {
        modified: None,
        ..hit
    };
    let summary = legacy_store_in_root_summary(&no_mtime);
    assert!(!summary.contains("last modified"), "{summary}");
    assert!(summary.contains("bytes) sits in the root"), "{summary}");
}

/// TASK-752: a distributed/git-canonical project (config declares
/// `mode = "distributed"` AND the orphan `aida-store` branch exists) with a
/// tracked `requirements.yaml` + `scaffold-report.html` flags both, and the
/// heal git-rm's them + appends the gitignore guard.
#[test]
fn detects_and_heals_legacy_store_cruft_on_git_canonical_project() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git752(root, &["init", "-q", "-b", "main"]);
    git752(root, &["config", "user.email", "t@t.t"]);
    git752(root, &["config", "user.name", "t"]);
    // git-canonical: config declares distributed mode...
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[store]\nmode = \"distributed\"\nstore_path = \".aida-store\"\n",
    )
    .unwrap();
    // ...and the orphan aida-store branch exists.
    std::fs::write(root.join("README.md"), "x\n").unwrap();
    git752(root, &["add", "."]);
    git752(root, &["commit", "-qm", "init"]);
    git752(root, &["branch", "aida-store"]);
    // Tracked legacy-store artifacts.
    std::fs::write(root.join("requirements.yaml"), "legacy: store\n").unwrap();
    std::fs::write(root.join("scaffold-report.html"), "<html/>\n").unwrap();
    git752(root, &["add", "requirements.yaml", "scaffold-report.html"]);
    git752(root, &["commit", "-qm", "cruft"]);

    // Detect.
    let store = aida_core::models::RequirementsStore::new();
    let findings = collect_doctor_findings(root, &store, Some("legacy-store-cruft")).unwrap();
    assert_eq!(findings.len(), 2, "both cruft files flagged: {findings:?}");
    assert!(findings.iter().all(|f| f.category == "legacy-store-cruft"));
    assert!(findings.iter().all(|f| f.safe_heal));
    let ids: std::collections::HashSet<&str> = findings.iter().map(|f| f.id.as_str()).collect();
    assert!(ids.contains("requirements.yaml"));
    assert!(ids.contains("scaffold-report.html"));

    // Heal both.
    for finding in &findings {
        let result = heal_doctor_legacy_store_cruft(root, finding).unwrap();
        assert_eq!(result.status, "healed", "{result:?}");
    }
    // git-rm'd → no longer tracked → no longer flagged.
    let after = collect_doctor_findings(root, &store, Some("legacy-store-cruft")).unwrap();
    assert!(
        after.is_empty(),
        "healed cruft must not reappear: {after:?}"
    );
    // gitignore guard appended exactly PR-651's block.
    let gi = std::fs::read_to_string(root.join(".gitignore")).unwrap();
    assert!(gi.contains("requirements*.yaml"));
    assert!(gi.contains("scaffold-report.html"));
}

/// TASK-752 GUARD: a legacy `--centralized` project that legitimately USES
/// `requirements.yaml` as its active store (config does NOT declare
/// distributed mode, no orphan aida-store branch) is a NO-OP — we must never
/// flag/nuke an active centralized backend.
#[test]
fn legacy_store_cruft_is_noop_on_centralized_project() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git752(root, &["init", "-q", "-b", "main"]);
    git752(root, &["config", "user.email", "t@t.t"]);
    git752(root, &["config", "user.name", "t"]);
    // Legacy centralized: requirements.yaml is the ACTIVE store, no
    // distributed-mode config, no orphan aida-store branch.
    std::fs::write(
        root.join("requirements.yaml"),
        "active: centralized store\n",
    )
    .unwrap();
    git752(root, &["add", "."]);
    git752(root, &["commit", "-qm", "centralized"]);

    // No distributed-mode declaration → no orphan branch → no-op.
    assert!(
        detect_legacy_store_cruft(root).is_empty(),
        "must not flag an active centralized requirements.yaml"
    );
    let store = aida_core::models::RequirementsStore::new();
    let findings = collect_doctor_findings(root, &store, Some("legacy-store-cruft")).unwrap();
    assert!(findings.is_empty(), "centralized project: no findings");
}

/// TASK-752: even with distributed-mode config, if the orphan aida-store
/// branch is MISSING (half-migrated config), stay silent — Gate 2.
#[test]
fn legacy_store_cruft_is_noop_without_orphan_branch() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git752(root, &["init", "-q", "-b", "main"]);
    git752(root, &["config", "user.email", "t@t.t"]);
    git752(root, &["config", "user.name", "t"]);
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[store]\nmode = \"distributed\"\n",
    )
    .unwrap();
    std::fs::write(root.join("requirements.yaml"), "x\n").unwrap();
    git752(root, &["add", "."]);
    git752(root, &["commit", "-qm", "init"]);
    // No `aida-store` branch created.
    assert!(
        detect_legacy_store_cruft(root).is_empty(),
        "missing orphan branch → no-op"
    );
}

// ── BUG-563: store-tracked per-clone runtime detection + heal ──

#[test]
fn normalize_doctor_category_accepts_store_tracked_runtime() {
    // BUG-563
    for alias in [
        "store-tracked-runtime",
        "store-runtime",
        "tracked-runtime",
        "store-node-toml",
        "store-runtime-cruft",
    ] {
        assert_eq!(
            normalize_doctor_category(alias).unwrap(),
            "store-tracked-runtime"
        );
    }
}

#[test]
fn is_store_tracked_runtime_path_matches_per_clone_runtime() {
    // BUG-563: the per-clone runtime set under top-level `.aida/`.
    assert!(is_store_tracked_runtime_path(".aida/node.toml"));
    assert!(is_store_tracked_runtime_path(".aida/dispenser.toml"));
    assert!(is_store_tracked_runtime_path(".aida/dispenser.lock"));
    assert!(is_store_tracked_runtime_path(".aida/sync.lock"));
    assert!(is_store_tracked_runtime_path(".aida/cache.db"));
    assert!(is_store_tracked_runtime_path(".aida/cache.db-journal"));
    assert!(is_store_tracked_runtime_path(".aida/cache.db-wal"));
    assert!(is_store_tracked_runtime_path(".aida/cache.db-shm"));
    // BUG-2378: the dispense lock beside blocks.yaml, one level only.
    assert!(is_store_tracked_runtime_path("registry/blocks.lock"));
    assert!(!is_store_tracked_runtime_path("registry/blocks.yaml"));
    assert!(!is_store_tracked_runtime_path("registry/sub/blocks.lock"));
    // Shared, legitimately-tracked store files never match.
    assert!(!is_store_tracked_runtime_path(".aida/config.toml"));
    assert!(!is_store_tracked_runtime_path("metadata.yaml"));
    assert!(!is_store_tracked_runtime_path(
        "objects/TASK/000/TASK-1.yaml"
    ));
    assert!(!is_store_tracked_runtime_path(
        "registry/agreed_counters.toml"
    ));
    // Not under .aida/, or nested deeper than top-level — never match.
    assert!(!is_store_tracked_runtime_path("node.toml"));
    assert!(!is_store_tracked_runtime_path(".aida/sub/node.toml"));
    assert!(!is_store_tracked_runtime_path("src/.aida/node.toml"));
}

/// BUG-563: build a project that declares distributed mode with an attached
/// `.aida-store` worktree (orphan `aida-store` branch) whose tree TRACKS
/// `.aida/node.toml` + `.aida/cache.db` + a `.aida/*.lock`. The detector
/// flags all three; the heal `git rm --cached`'s + gitignores + commits on
/// the store worktree, after which they no longer reappear and the working
/// copies survive (per-clone state stays on disk).
#[test]
fn detects_and_heals_store_tracked_runtime_on_git_canonical_project() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git752(root, &["init", "-q", "-b", "main"]);
    git752(root, &["config", "user.email", "t@t.t"]);
    git752(root, &["config", "user.name", "t"]);
    // git-canonical: config declares distributed mode.
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[store]\nmode = \"distributed\"\nstore_path = \".aida-store\"\n",
    )
    .unwrap();
    std::fs::write(root.join("README.md"), "x\n").unwrap();
    git752(root, &["add", "."]);
    git752(root, &["commit", "-qm", "init"]);

    // Attach the orphan `aida-store` worktree at `.aida-store/`.
    git752(
        root,
        &["worktree", "add", "-q", "-b", "aida-store", ".aida-store"],
    );
    let store = root.join(".aida-store");
    // Make it look like a real store (the detector gates on objects/).
    std::fs::create_dir_all(store.join("objects")).unwrap();
    std::fs::write(store.join("objects/.keep"), "").unwrap();
    std::fs::create_dir_all(store.join(".aida")).unwrap();
    // Per-clone runtime files WRONGLY tracked on the orphan branch.
    std::fs::write(store.join(".aida/node.toml"), "node_id = 1\n").unwrap();
    std::fs::write(store.join(".aida/dispenser.toml"), "next = 5\n").unwrap();
    std::fs::write(store.join(".aida/sync.lock"), "").unwrap();
    std::fs::write(store.join(".aida/cache.db"), "binary").unwrap();
    git752(&store, &["add", "-A"]);
    git752(&store, &["commit", "-qm", "store w/ tracked runtime"]);

    // Detect: all four per-clone runtime files flagged.
    let req_store = aida_core::models::RequirementsStore::new();
    let findings =
        collect_doctor_findings(root, &req_store, Some("store-tracked-runtime")).unwrap();
    assert_eq!(findings.len(), 4, "all runtime files flagged: {findings:?}");
    assert!(findings
        .iter()
        .all(|f| f.category == "store-tracked-runtime"));
    assert!(findings.iter().all(|f| f.safe_heal));
    let ids: std::collections::HashSet<&str> = findings.iter().map(|f| f.id.as_str()).collect();
    assert!(ids.contains(".aida/node.toml"));
    assert!(ids.contains(".aida/dispenser.toml"));
    assert!(ids.contains(".aida/sync.lock"));
    assert!(ids.contains(".aida/cache.db"));

    // Heal each.
    for finding in &findings {
        let result = heal_doctor_store_tracked_runtime(root, finding).unwrap();
        assert_eq!(result.status, "healed", "{result:?}");
    }
    // git-rm --cached'd → no longer tracked → no longer flagged.
    let after = collect_doctor_findings(root, &req_store, Some("store-tracked-runtime")).unwrap();
    assert!(
        after.is_empty(),
        "healed runtime must not reappear: {after:?}"
    );
    // Working copies survive (per-clone state stays on disk).
    assert!(store.join(".aida/node.toml").exists());
    assert!(store.join(".aida/cache.db").exists());
    // gitignore guard appended on the STORE worktree.
    let gi = std::fs::read_to_string(store.join(".gitignore")).unwrap();
    assert!(gi.contains(".aida/node.toml"));
    assert!(gi.contains(".aida/dispenser.toml"));
    assert!(gi.contains(".aida/*.lock"));
    assert!(gi.contains(".aida/cache.db"));
    for pat in aida_core::fs_atomic::STORE_STAGING_IGNORE_PATTERNS {
        assert!(gi.lines().any(|l| l.trim() == *pat), "missing {pat}");
    }
    assert!(
        !gi.lines().any(|l| l.trim() == "*.tmp.*"),
        "the staging ignore must stay store-scoped"
    );
}

/// BUG-563 GUARD: with distributed-mode config but NO attached `.aida-store`
/// worktree, the detector is a NO-OP (Gate 2) — nothing to scan.
#[test]
fn store_tracked_runtime_is_noop_without_store_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git752(root, &["init", "-q", "-b", "main"]);
    git752(root, &["config", "user.email", "t@t.t"]);
    git752(root, &["config", "user.name", "t"]);
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[store]\nmode = \"distributed\"\n",
    )
    .unwrap();
    std::fs::write(root.join("README.md"), "x\n").unwrap();
    git752(root, &["add", "."]);
    git752(root, &["commit", "-qm", "init"]);
    // No `.aida-store/` worktree attached.
    assert!(
        detect_store_tracked_runtime(root).is_empty(),
        "no store worktree → no-op"
    );
}
