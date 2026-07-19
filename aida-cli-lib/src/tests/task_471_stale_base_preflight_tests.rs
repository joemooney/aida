use super::*;

fn git(repo: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
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
fn git_path_is_ignored_honors_dot_gitignore() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    git(repo, &["init", "-q"]);
    std::fs::write(repo.join(".gitignore"), "generated/\n*.gen.rs\n").unwrap();

    assert!(git_path_is_ignored(repo, "generated/api.rs"));
    assert!(git_path_is_ignored(repo, "foo.gen.rs"));
    assert!(!git_path_is_ignored(repo, "src/main.rs"));
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

#[cfg(unix)]
fn fake_gh(dir: &std::path::Path, head_oid: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join("gh");
    std::fs::write(
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
        )
        .unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
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
