use super::*;

fn git(repo: &std::path::Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("git {:?} failed to spawn: {e}", args));
    assert!(
        out.status.success(),
        "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

// --- TASK-480: intermediate-only gitignore-backed wiring ---

#[test]
fn git_path_is_ignored_by_tree_rules_honors_dot_gitignore() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    git(repo, &["init", "-q"]);
    std::fs::write(repo.join(".gitignore"), "generated/\n*.gen.rs\n").unwrap();

    assert!(git_path_is_ignored_by_tree_rules(repo, "generated/api.rs"));
    assert!(git_path_is_ignored_by_tree_rules(repo, "foo.gen.rs"));
    assert!(!git_path_is_ignored_by_tree_rules(repo, "src/main.rs"));
}

#[test]
fn classify_with_gitignore_refuses_project_specific_generated_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    git(repo, &["init", "-q"]);
    // A project-specific generated dir NOT in the built-in heuristic
    // list, only known via the project's own .gitignore.
    std::fs::write(repo.join(".gitignore"), "proto-gen/\n").unwrap();

    let only_generated = vec!["proto-gen/api.pb.rs".to_string()];
    assert!(matches!(
        classify_intermediate_only_with_gitignore(repo, &only_generated),
        pr_rebase::IntermediateOnlyOutcome::IntermediateOnly { .. }
    ));

    let with_source = vec![
        "proto-gen/api.pb.rs".to_string(),
        "src/handler.rs".to_string(),
        "src/lib.rs".to_string(),
    ];
    // 1 intermediate : 2 source → minority → clean.
    assert_eq!(
        classify_intermediate_only_with_gitignore(repo, &with_source),
        pr_rebase::IntermediateOnlyOutcome::Clean
    );
}

#[test]
fn classify_with_gitignore_refuses_heuristic_target_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    git(repo, &["init", "-q"]);
    // Empty .gitignore — the target/ refusal must come from the
    // built-in heuristic, not the project's ignore rules.
    std::fs::write(repo.join(".gitignore"), "\n").unwrap();

    let only_target = vec![
        "target/debug/foo.bin".to_string(),
        "target/release/aida".to_string(),
    ];
    assert!(matches!(
        classify_intermediate_only_with_gitignore(repo, &only_target),
        pr_rebase::IntermediateOnlyOutcome::IntermediateOnly { .. }
    ));
}

// --- BUG-b6b1278890774ef993d2ebc327862470-009: local excludes never classify tracked paths ---

// trace:BUG-b6b1278890774ef993d2ebc327862470-009 | ai:claude
fn committed_repo(files: &[(&str, &str)]) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    git(repo, &["init", "-q"]);
    git(repo, &["config", "user.email", "aida@example.test"]);
    git(repo, &["config", "user.name", "AIDA Test"]);
    for (path, body) in files {
        let full = repo.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, body).unwrap();
    }
    git(repo, &["add", "-f", "."]);
    git(repo, &["commit", "-q", "-m", "base"]);
    tmp
}

#[test]
fn tracked_path_matching_only_local_info_exclude_classifies_clean() {
    let tmp = committed_repo(&[(".aida/config.toml", "x = 1\n")]);
    let repo = tmp.path();
    std::fs::write(repo.join(".git/info/exclude"), "/.aida/\n/.aida/*\n").unwrap();

    assert!(!git_path_is_ignored_by_tree_rules(
        repo,
        ".aida/config.toml"
    ));
    assert_eq!(
        classify_intermediate_only_with_gitignore(repo, &[".aida/config.toml".to_string()]),
        pr_rebase::IntermediateOnlyOutcome::Clean
    );
}

#[test]
fn tracked_path_matching_committed_gitignore_stays_intermediate_only() {
    let tmp = committed_repo(&[(".gitignore", "target/\n"), ("target/x.o", "obj\n")]);
    let repo = tmp.path();

    assert!(git_path_is_ignored_by_tree_rules(repo, "target/x.o"));
    assert!(matches!(
        classify_intermediate_only_with_gitignore(repo, &["target/x.o".to_string()]),
        pr_rebase::IntermediateOnlyOutcome::IntermediateOnly { .. }
    ));
}

#[test]
fn tracked_path_local_ancestor_dir_exclude_does_not_mask_committed_gitignore() {
    let tmp = committed_repo(&[(".gitignore", "*.o\n"), ("build/x.o", "obj\n")]);
    let repo = tmp.path();
    std::fs::write(repo.join(".git/info/exclude"), "/build/\n").unwrap();

    assert!(git_path_is_ignored_by_tree_rules(repo, "build/x.o"));
    assert!(matches!(
        classify_intermediate_only_with_gitignore(repo, &["build/x.o".to_string()]),
        pr_rebase::IntermediateOnlyOutcome::IntermediateOnly { .. }
    ));
}

#[test]
fn tracked_path_matching_only_global_excludes_file_classifies_clean() {
    let tmp = committed_repo(&[(".aida/config.toml", "x = 1\n")]);
    let repo = tmp.path();
    let excludes = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(excludes.path(), "/.aida/\n").unwrap();
    git(
        repo,
        &[
            "config",
            "core.excludesFile",
            excludes.path().to_str().unwrap(),
        ],
    );

    assert!(!git_path_is_ignored_by_tree_rules(
        repo,
        ".aida/config.toml"
    ));
    assert_eq!(
        classify_intermediate_only_with_gitignore(repo, &[".aida/config.toml".to_string()]),
        pr_rebase::IntermediateOnlyOutcome::Clean
    );
}

#[test]
fn committed_negation_pattern_is_not_ignored() {
    let tmp = committed_repo(&[(".gitignore", "*.toml\n!keep.toml\n"), ("keep.toml", "x\n")]);
    let repo = tmp.path();

    // Tracked path, and a path the index has never seen (`-v` branch).
    assert!(!git_path_is_ignored_by_tree_rules(repo, "keep.toml"));
    assert_eq!(
        classify_intermediate_only_with_gitignore(repo, &["keep.toml".to_string()]),
        pr_rebase::IntermediateOnlyOutcome::Clean
    );
    std::fs::write(repo.join(".gitignore"), "*.toml\n!new.toml\n").unwrap();
    assert_eq!(
        classify_intermediate_only_with_gitignore(repo, &["new.toml".to_string()]),
        pr_rebase::IntermediateOnlyOutcome::Clean
    );
}

// --- TASK-b6b1278890774ef993d2ebc327862470-034: the untracked `check-ignore -v` branch ---

// trace:TASK-b6b1278890774ef993d2ebc327862470-034 | ai:claude
#[test]
fn untracked_path_matching_only_local_info_exclude_classifies_clean() {
    let tmp = committed_repo(&[("README.md", "x\n")]);
    let repo = tmp.path();
    std::fs::write(repo.join(".git/info/exclude"), "/.aida/\n").unwrap();
    std::fs::create_dir_all(repo.join(".aida")).unwrap();
    std::fs::write(repo.join(".aida/x.md"), "x\n").unwrap();

    assert!(!git_path_is_ignored_by_tree_rules(repo, ".aida/x.md"));
    assert_eq!(
        classify_intermediate_only_with_gitignore(repo, &[".aida/x.md".to_string()]),
        pr_rebase::IntermediateOnlyOutcome::Clean
    );
}

// trace:TASK-b6b1278890774ef993d2ebc327862470-034 | ai:claude
#[test]
fn untracked_path_matching_committed_gitignore_is_intermediate_only() {
    let tmp = committed_repo(&[(".gitignore", "target/\n")]);
    let repo = tmp.path();
    std::fs::create_dir_all(repo.join("target")).unwrap();
    std::fs::write(repo.join("target/new.o"), "obj\n").unwrap();

    assert!(git_path_is_ignored_by_tree_rules(repo, "target/new.o"));
    assert!(matches!(
        classify_intermediate_only_with_gitignore(repo, &["target/new.o".to_string()]),
        pr_rebase::IntermediateOnlyOutcome::IntermediateOnly { .. }
    ));
}

// In a linked worktree `check-ignore -v` names the common dir's
// `info/exclude` by absolute path; it must still read as a local source.
// trace:TASK-b6b1278890774ef993d2ebc327862470-034 | ai:claude
#[test]
fn linked_worktree_absolute_info_exclude_source_classifies_clean() {
    let tmp = committed_repo(&[(".gitignore", "*.gen.rs\n")]);
    let repo = tmp.path();
    let wt_parent = tempfile::tempdir().unwrap();
    let wt = wt_parent.path().join("linked");
    git(
        repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "linked",
            wt.to_str().unwrap(),
        ],
    );
    std::fs::write(repo.join(".git/info/exclude"), "/.aida/\n").unwrap();
    std::fs::create_dir_all(wt.join(".aida")).unwrap();
    std::fs::write(wt.join(".aida/x.md"), "x\n").unwrap();

    let verbose = git(&wt, &["check-ignore", "-v", "--no-index", ".aida/x.md"]);
    let common_dir = git(
        &wt,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    );
    assert!(
        verbose.starts_with(&format!("{common_dir}/info/exclude")),
        "expected the absolute info/exclude source under {common_dir}, got {verbose:?}"
    );
    assert!(!git_path_is_ignored_by_tree_rules(&wt, ".aida/x.md"));
    assert_eq!(
        classify_intermediate_only_with_gitignore(&wt, &[".aida/x.md".to_string()]),
        pr_rebase::IntermediateOnlyOutcome::Clean
    );
    // The committed `.gitignore` still decides in the linked worktree.
    assert!(git_path_is_ignored_by_tree_rules(&wt, "api.gen.rs"));
}

#[cfg(unix)]
fn fake_gh(dir: &std::path::Path, head_oid: &str) -> std::path::PathBuf {
    let path = dir.join("gh");
    crate::test_exec::write_executable(
        &path,
        format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = pr ] && [ \"$2\" = view ]; then\n\
                   printf '%s\\n' '{{\"baseRefName\":\"main\",\"headRefName\":\"feature\",\"headRefOid\":\"{}\",\"isCrossRepository\":false,\"headRepository\":{{\"nameWithOwner\":\"local/aida\"}},\"isDraft\":false}}'\n\
                   exit 0\n\
                 fi\n\
                 echo unexpected gh args: \"$@\" >&2\n\
                 exit 1\n",
                head_oid
            ),
    );
    path
}

#[cfg(unix)]
#[test]
fn preflight_detects_real_stale_overlap_before_reviewer_launch() {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin.git");
    let repo = tmp.path().join("repo");

    git(tmp.path(), &["init", "--bare", origin.to_str().unwrap()]);
    git(tmp.path(), &["init", "-b", "main", repo.to_str().unwrap()]);
    git(&repo, &["config", "user.email", "aida@example.test"]);
    git(&repo, &["config", "user.name", "AIDA Test"]);

    std::fs::write(repo.join("a.txt"), "base\n").unwrap();
    git(&repo, &["add", "a.txt"]);
    git(&repo, &["commit", "-m", "base"]);
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["push", "-u", "origin", "main"]);

    git(&repo, &["checkout", "-b", "feature"]);
    std::fs::write(repo.join("a.txt"), "feature\n").unwrap();
    git(&repo, &["commit", "-am", "feature edits a.txt"]);
    let feature_sha = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["push", "origin", "HEAD:refs/pull/1/head"]);

    git(&repo, &["checkout", "main"]);
    std::fs::write(repo.join("a.txt"), "main moved\n").unwrap();
    git(&repo, &["commit", "-am", "main edits a.txt"]);
    git(&repo, &["push", "origin", "main"]);

    let gh = fake_gh(tmp.path(), &feature_sha);
    let outcome = preflight_stale_base_check_with_gh(&repo, 1, gh.as_os_str()).unwrap();
    match outcome {
        pr_rebase::StaleBaseOutcome::StaleOverlap {
            behind,
            overlap_files,
            ..
        } => {
            assert_eq!(behind, 1);
            assert_eq!(overlap_files, vec!["a.txt".to_string()]);
            let msg = pr_rebase::stale_base_block_message(1, behind, &overlap_files);
            assert!(msg.contains("refusing to launch reviewer"), "{msg}");
            assert!(msg.contains("aida pr rebase 1"), "{msg}");
            assert!(msg.contains("--allow-stale-base"), "{msg}");
        }
        other => panic!("expected stale overlap, got {other:?}"),
    }
}

#[test]
fn file_sets_exclude_base_only_files_from_pr_side() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");

    git(tmp.path(), &["init", "-b", "main", repo.to_str().unwrap()]);
    git(&repo, &["config", "user.email", "aida@example.test"]);
    git(&repo, &["config", "user.name", "AIDA Test"]);

    std::fs::write(repo.join("shared.txt"), "base\n").unwrap();
    git(&repo, &["add", "shared.txt"]);
    git(&repo, &["commit", "-m", "base"]);

    git(&repo, &["checkout", "-b", "pr-branch"]);
    std::fs::write(repo.join("pr-only.txt"), "pr\n").unwrap();
    git(&repo, &["add", "pr-only.txt"]);
    git(&repo, &["commit", "-m", "pr touches pr-only"]);

    git(&repo, &["checkout", "main"]);
    std::fs::write(repo.join("base-only.txt"), "base\n").unwrap();
    git(&repo, &["add", "base-only.txt"]);
    git(&repo, &["commit", "-m", "base touches base-only"]);

    let (pr_files, base_files) = preflight_stale_base_file_sets(&repo, "main", "pr-branch");

    assert_eq!(pr_files, vec!["pr-only.txt".to_string()]);
    assert_eq!(base_files, vec!["base-only.txt".to_string()]);
    assert_eq!(
        pr_rebase::classify_stale_base(
            1,
            &pr_files,
            &base_files,
            pr_rebase::ConflictPrediction::Clean,
        ),
        pr_rebase::StaleBaseOutcome::StaleNoOverlap { behind: 1 }
    );
}
