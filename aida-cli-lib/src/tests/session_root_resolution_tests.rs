//! trace:BUG-75 BUG-76 | ai:claude
use super::*;
use std::process::Command;
use tempfile::TempDir;

fn git(p: &std::path::Path, args: &[&str]) {
    let o = Command::new("git")
        .arg("-C")
        .arg(p)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(o.status.success(), "git {:?} failed", args);
}

fn fixture_with_linked_worktree() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let main = tmp.path().join("repo");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "--initial-branch=main", "--quiet"]);
    std::fs::write(main.join("a.txt"), "x").unwrap();
    git(&main, &["add", "a.txt"]);
    git(&main, &["commit", "-m", "base", "--quiet"]);
    git(&main, &["checkout", "-b", "feature", "--quiet"]);
    std::fs::write(main.join("a.txt"), "y").unwrap();
    git(&main, &["add", "a.txt"]);
    git(&main, &["commit", "-m", "feature", "--quiet"]);
    git(&main, &["checkout", "main", "--quiet"]);

    let linked = tmp.path().join("repo-feature");
    git(
        &main,
        &[
            "worktree",
            "add",
            linked.to_str().unwrap(),
            "feature",
            "--quiet",
        ],
    );
    (tmp, main, linked)
}

fn fixture_with_aida_submodule() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let child_src = tmp.path().join("child-src");
    std::fs::create_dir_all(&child_src).unwrap();
    git(&child_src, &["init", "--initial-branch=main", "--quiet"]);
    std::fs::write(child_src.join("lib.txt"), "submodule").unwrap();
    git(&child_src, &["add", "lib.txt"]);
    git(&child_src, &["commit", "-m", "child base", "--quiet"]);

    let super_root = tmp.path().join("super");
    std::fs::create_dir_all(&super_root).unwrap();
    git(&super_root, &["init", "--initial-branch=main", "--quiet"]);
    std::fs::write(super_root.join("root.txt"), "super").unwrap();
    git(&super_root, &["add", "root.txt"]);
    git(&super_root, &["commit", "-m", "super base", "--quiet"]);
    git(
        &super_root,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            child_src.to_str().unwrap(),
            "vendor/lib",
        ],
    );
    git(&super_root, &["commit", "-am", "add submodule", "--quiet"]);

    let submodule = super_root.join("vendor/lib");
    std::fs::create_dir_all(super_root.join(".aida")).unwrap();
    std::fs::write(super_root.join(".aida/config.toml"), "[storage]\n").unwrap();
    std::fs::create_dir_all(submodule.join(".aida")).unwrap();
    std::fs::write(submodule.join(".aida/config.toml"), "[storage]\n").unwrap();

    (tmp, super_root, submodule)
}

/// BUG-75: from inside a linked worktree, main_worktree_root_from
/// returns the MAIN worktree path, not the linked one.
#[test]
fn main_worktree_root_resolves_from_linked() {
    let (_tmp, main, linked) = fixture_with_linked_worktree();
    let resolved = main_worktree_root_from(&linked);
    assert_eq!(
        std::fs::canonicalize(&resolved).unwrap(),
        std::fs::canonicalize(&main).unwrap(),
        "expected main worktree, got {}",
        resolved.display()
    );
}

/// BUG-75: from the main worktree, the helper returns the same path
/// (no regression).
#[test]
fn main_worktree_root_returns_main_unchanged() {
    let (_tmp, main, _linked) = fixture_with_linked_worktree();
    let resolved = main_worktree_root_from(&main);
    assert_eq!(
        std::fs::canonicalize(&resolved).unwrap(),
        std::fs::canonicalize(&main).unwrap()
    );
}

// trace:BUG-1093 | ai:codex
#[test]
fn agent_launch_root_keeps_submodule_working_tree() {
    let (_tmp, _super_root, submodule) = fixture_with_aida_submodule();
    let nested = submodule.join("src");
    std::fs::create_dir_all(&nested).unwrap();

    let discovered = find_aida_project_root_from(&nested).unwrap();
    let launch_root = agent_launch_project_root_from(&discovered);

    assert_eq!(
        std::fs::canonicalize(&launch_root).unwrap(),
        std::fs::canonicalize(&submodule).unwrap(),
        "agent launches from the submodule working tree, not .git/modules metadata"
    );
    ensure_agent_launch_cwd_not_git_metadata(&launch_root).unwrap();
}

// trace:BUG-1093 | ai:codex
#[test]
fn agent_launch_root_preserves_linked_worktree_promotion() {
    let (_tmp, main, linked) = fixture_with_linked_worktree();
    std::fs::create_dir_all(main.join(".aida")).unwrap();
    std::fs::write(main.join(".aida/config.toml"), "[storage]\n").unwrap();
    std::fs::create_dir_all(linked.join(".aida")).unwrap();
    std::fs::write(linked.join(".aida/config.toml"), "[storage]\n").unwrap();

    let launch_root = agent_launch_project_root_from(&linked);

    assert_eq!(
        std::fs::canonicalize(&launch_root).unwrap(),
        std::fs::canonicalize(&main).unwrap(),
        "ordinary linked worktrees keep the existing main-worktree launch behavior"
    );
}

// trace:BUG-1093 | ai:codex
#[test]
fn agent_launch_context_is_rooted_under_submodule_aida_dir() {
    let (_tmp, _super_root, submodule) = fixture_with_aida_submodule();
    let config = AgentLaunchConfig {
        agent_type: "codex",
        binary: "codex",
        default_args: Vec::new(),
        prompt_style: AgentPromptStyle::Positional,
    };
    let plan =
        prepare_agent_launch_dry(&submodule, Some("implementer".into()), None, "codex", None)
            .unwrap();

    let context =
        prepare_agent_launch_context(&config, &plan, AgentContextOptions::new(true, false))
            .unwrap()
            .unwrap();
    let rendered = std::fs::read_to_string(&context.path).unwrap();

    assert!(context
        .path
        .starts_with(submodule.join(".aida/agents/context")));
    assert!(rendered.contains(&format!("- Project root: {}", submodule.display())));
    assert!(rendered.contains(&format!("- Working directory: {}", submodule.display())));
}

// trace:BUG-1093 | ai:codex
#[test]
fn agent_launch_rejects_git_metadata_cwd() {
    let tmp = TempDir::new().unwrap();
    let metadata = tmp.path().join("repo/.git/modules/vendor/lib");
    let err = ensure_agent_launch_cwd_not_git_metadata(&metadata).unwrap_err();
    assert!(
        err.to_string().contains("refusing to launch agent"),
        "{err}"
    );
}

/// BUG-76: detect_default_branch_ref picks origin/main when present,
/// falls back to local main, returns None when neither exists.
#[test]
fn default_branch_ref_prefers_origin_main() {
    // Origin-less repo with a local main → falls back to local.
    let (_tmp, main, _linked) = fixture_with_linked_worktree();
    let resolved = detect_default_branch_ref(&main);
    assert_eq!(resolved.as_deref(), Some("main"));
}

/// BUG-76: current_branch_at returns the branch checked out at path.
#[test]
fn current_branch_at_returns_branch_name() {
    let (_tmp, main, linked) = fixture_with_linked_worktree();
    assert_eq!(current_branch_at(&main).as_deref(), Some("main"));
    assert_eq!(current_branch_at(&linked).as_deref(), Some("feature"));
}
