//! BUG-2169: a contained session in a linked worktree can run git there.
//!
//! - Unit: the resolved grant for a linked worktree is exactly its gitdir and
//!   the common dir's `objects`, `refs` and `logs`; config, hooks, info,
//!   redirect pointers and other worktrees' checked-out branches are denied.
//! - Harness: the generated settings, checked over real paths with the BUG-1929
//!   model of Claude Code's Linux sandbox (deny wins over an enclosing allow).
//! - Packed refs: every protected branch (checked out in another worktree,
//!   or the default branch) gets a loose ref and reflog before launch, since
//!   the sandbox skips a deny on a missing path.
//! - Fail closed: a primary checkout keeps its recursive `.git` deny; a
//!   symlinked or path-traversing gitdir, or a symlinked `objects`, gets no
//!   git grant at all.
//!
//! trace:BUG-2169 | ai:claude

use super::store_tests::{allow_write, bug_1929_git, deny_write, sandbox_may_write};
use super::*;
use std::path::{Path, PathBuf};

/// `<base>/main` (a repository with one commit on `main`, a store directory
/// and a cache dir) and a linked worktree `<base>/wt` on branch `wt`.
fn linked_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(tmp.path()).unwrap();
    let main = base.join("main");
    std::fs::create_dir_all(&main).unwrap();
    bug_1929_git(&main, &["init", "-q", "-b", "main"]);
    bug_1929_git(&main, &["commit", "-q", "--allow-empty", "-m", "init"]);
    std::fs::create_dir_all(main.join(".aida-store")).unwrap();
    std::fs::create_dir_all(main.join(".aida")).unwrap();
    let worktree = base.join("wt");
    bug_1929_git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wt",
            worktree.to_str().unwrap(),
        ],
    );
    (tmp, main, worktree)
}

fn settings_for(worktree: &Path, main: &Path) -> serde_json::Value {
    let git = contained_git_write_paths(worktree);
    let json = crate::session::contained_settings_json_with_git(
        &[],
        false,
        &contained_write_roots(worktree),
        &contained_store_write_paths(main),
        git.as_ref(),
    );
    serde_json::from_str(&json).unwrap()
}

// UNIT: the grant is the gitdir plus objects/refs/logs, nothing else; the
// denies name config, hooks, info, the gitdir's pointers and the main
// checkout's branch, and never the seat's own branch.
#[test]
fn bug_2169_linked_worktree_grant_is_gitdir_objects_refs_logs() {
    let (_tmp, main, worktree) = linked_fixture();
    let common = main.join(".git");
    let gitdir = common.join("worktrees/wt");
    let git = resolve_contained_git_paths(&worktree)
        .unwrap()
        .expect("linked worktree gets a git grant");
    assert_eq!(git.common, common);
    assert_eq!(git.gitdir, gitdir);
    assert_eq!(
        git.allow,
        vec![
            gitdir.clone(),
            common.join("objects"),
            common.join("refs"),
            common.join("logs"),
        ]
    );
    for denied in [
        common.join("config"),
        common.join("hooks"),
        common.join("info"),
        gitdir.join("config.worktree"),
        gitdir.join("commondir"),
        gitdir.join("gitdir"),
        worktree.join(".git"),
        common.join("refs/heads/main"),
        common.join("logs/refs/heads/main"),
    ] {
        assert!(git.deny.contains(&denied), "{denied:?} in {:?}", git.deny);
    }
    assert!(
        !git.deny.contains(&common.join("refs/heads/wt")),
        "{:?}",
        git.deny
    );
    assert!(
        !git.deny.contains(&common.join("logs/refs/heads/wt")),
        "{:?}",
        git.deny
    );
}

// HARNESS: over real paths, the seat may write what commit/fetch/merge/push
// write, and may not write config, hooks, another worktree's gitdir, the main
// checkout's branch, or the gitdir's config.worktree.
#[test]
fn bug_2169_sandbox_allows_git_state_denies_config_and_hooks() {
    let (_tmp, main, worktree) = linked_fixture();
    let common = main.join(".git");
    let gitdir = common.join("worktrees/wt");
    std::fs::write(gitdir.join("config.worktree"), b"").unwrap();
    let other = main.parent().unwrap().join("other");
    bug_1929_git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "other",
            other.to_str().unwrap(),
        ],
    );
    let settings = settings_for(&worktree, &main);
    for writable in [
        gitdir.join("index.lock"),
        gitdir.join("HEAD"),
        gitdir.join("FETCH_HEAD"),
        common.join("objects/ab/cdef"),
        common.join("refs/heads/wt"),
        common.join("refs/remotes/origin/wt"),
        common.join("logs/refs/heads/wt"),
    ] {
        assert!(
            sandbox_may_write(&settings, &writable),
            "{writable:?}: {settings}"
        );
    }
    for denied in [
        common.join("config"),
        common.join("hooks/pre-commit"),
        common.join("info/exclude"),
        common.join("packed-refs"),
        common.join("HEAD"),
        common.join("worktrees/other/HEAD"),
        common.join("refs/heads/main"),
        common.join("refs/heads/other"),
        gitdir.join("config.worktree"),
        gitdir.join("commondir"),
    ] {
        assert!(
            !sandbox_may_write(&settings, &denied),
            "{denied:?}: {settings}"
        );
    }
    assert!(
        !deny_write(&settings).contains(&format!("/{}", common.display())),
        "the recursive common-dir deny would re-close objects/refs: {settings}"
    );
    // The Edit/Write tool layer still denies the whole git directory.
    let denies = settings["permissions"]["deny"].as_array().unwrap();
    for tool in WRITE_TOOLS {
        assert!(
            denies.contains(&serde_json::json!(format!(
                "{tool}(/{}/**)",
                common.display()
            ))),
            "{settings}"
        );
    }
    let allows = settings["permissions"]["allow"].to_string();
    assert!(
        !allows.contains("/.git"),
        "no tool allow rule for git paths: {allows}"
    );
}

// PRIMARY: a primary checkout gets no git grant, and its `.git` stays
// recursively denied in both layers, as before.
#[test]
fn bug_2169_primary_checkout_keeps_recursive_git_deny() {
    let (_tmp, main, _worktree) = linked_fixture();
    assert_eq!(resolve_contained_git_paths(&main).unwrap(), None);
    let json = crate::session::claude_contained_settings_json(&[main.clone()]);
    let settings: serde_json::Value = serde_json::from_str(&json).unwrap();
    let git = main.join(".git");
    assert!(
        deny_write(&settings).contains(&format!("/{}", git.display())),
        "{json}"
    );
    assert!(
        !allow_write(&settings).iter().any(|p| p.contains("/.git")),
        "{json}"
    );
    for path in [
        git.join("config"),
        git.join("hooks/pre-commit"),
        git.join("objects/x"),
    ] {
        assert!(!sandbox_may_write(&settings, &path), "{path:?}: {json}");
    }
}

// LAUNCH: the production settings builder grants the linked worktree's git
// paths and primes an empty config.worktree so its deny binds.
#[test]
fn bug_2169_launch_settings_grant_linked_worktree_git() {
    let (_tmp, main, worktree) = linked_fixture();
    let gitdir = main.join(".git/worktrees/wt");
    let _ = std::fs::remove_file(gitdir.join("config.worktree"));
    let json = crate::session::claude_contained_settings_json(&[worktree.clone()]);
    let settings: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(gitdir.join("config.worktree").is_file());
    assert!(
        sandbox_may_write(&settings, &gitdir.join("index.lock")),
        "{json}"
    );
    assert!(
        sandbox_may_write(&settings, &main.join(".git/objects/ab/cd")),
        "{json}"
    );
    assert!(
        !sandbox_may_write(&settings, &main.join(".git/config")),
        "{json}"
    );
    assert!(
        !sandbox_may_write(&settings, &gitdir.join("config.worktree")),
        "{json}"
    );
}

/// Assert `worktree` gets no git grant: an error from the resolver, and no
/// `.git` path in the generated `allowWrite`.
fn assert_no_git_grant(worktree: &Path, main: &Path) {
    let refused = resolve_contained_git_paths(worktree);
    assert!(refused.is_err(), "{refused:?}");
    let settings = settings_for(worktree, main);
    assert!(
        !allow_write(&settings).iter().any(|p| p.contains("/.git")),
        "{settings}"
    );
    assert!(
        deny_write(&settings).contains(&format!("/{}", main.join(".git").display())),
        "the store's recursive common-dir deny stays: {settings}"
    );
}

// FAIL CLOSED: a gitdir reached through a symlink in `<common>/worktrees`
// resolves outside it, so the seat falls back to the worktree-only boundary.
#[cfg(unix)]
#[test]
fn bug_2169_symlinked_gitdir_falls_back_to_worktree_only() {
    let (_tmp, main, worktree) = linked_fixture();
    let gitdir = main.join(".git/worktrees/wt");
    let elsewhere = main.parent().unwrap().join("elsewhere-gitdir");
    std::fs::rename(&gitdir, &elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &gitdir).unwrap();
    std::fs::write(
        elsewhere.join("commondir"),
        format!("{}\n", main.join(".git").display()),
    )
    .unwrap();
    assert_no_git_grant(&worktree, &main);
}

// FAIL CLOSED: a `.git` pointer that climbs out of `<common>/worktrees` with
// `..` resolves outside it.
#[test]
fn bug_2169_traversing_gitdir_falls_back_to_worktree_only() {
    let (_tmp, main, worktree) = linked_fixture();
    let gitdir = main.join(".git/worktrees/wt");
    let escaped = main.parent().unwrap().join("escaped");
    std::fs::rename(&gitdir, &escaped).unwrap();
    std::fs::write(
        escaped.join("commondir"),
        format!("{}\n", main.join(".git").display()),
    )
    .unwrap();
    std::fs::write(
        worktree.join(".git"),
        format!(
            "gitdir: {}/.git/worktrees/../../../escaped\n",
            main.display()
        ),
    )
    .unwrap();
    assert_no_git_grant(&worktree, &main);
}

// FAIL CLOSED: a symlinked `objects` would grant its target; refuse.
#[cfg(unix)]
#[test]
fn bug_2169_symlinked_objects_falls_back_to_worktree_only() {
    let (_tmp, main, worktree) = linked_fixture();
    let objects = main.join(".git/objects");
    let moved = main.parent().unwrap().join("objects-elsewhere");
    std::fs::rename(&objects, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &objects).unwrap();
    assert_no_git_grant(&worktree, &main);
}

/// `linked_fixture` plus a worktree `<base>/other` on branch `other` and a
/// branch `trunk` checked out nowhere, with every branch PACKED (no loose ref
/// and, for `main`, no reflog file): the normal state after `git gc`.
fn packed_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let (tmp, main, worktree) = linked_fixture();
    let other = main.parent().unwrap().join("other");
    bug_1929_git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "other",
            other.to_str().unwrap(),
        ],
    );
    bug_1929_git(&main, &["branch", "trunk"]);
    bug_1929_git(&main, &["pack-refs", "--all", "--prune"]);
    let common = main.join(".git");
    let _ = std::fs::remove_file(common.join("logs/refs/heads/main"));
    for branch in ["main", "other", "wt", "trunk"] {
        assert!(!common.join("refs/heads").join(branch).exists(), "{branch}");
    }
    (tmp, main, worktree)
}

fn settings_with(worktree: &Path, main: &Path, git: &ContainedGitPaths) -> serde_json::Value {
    let json = crate::session::contained_settings_json_with_git(
        &[],
        false,
        &contained_write_roots(worktree),
        &contained_store_write_paths(main),
        Some(git),
    );
    serde_json::from_str(&json).unwrap()
}

// MODEL + BYPASS: with packed branches, the deny on a missing loose ref binds
// nothing, so without materialization the seat could write a loose
// `refs/heads/main` (which overrides packed-refs). After the launch-time
// materialization the loose ref and reflog exist, hold the packed value, and
// are denied; the seat's own branch and remote-tracking refs stay writable.
// trace:BUG-2169 | ai:claude
#[test]
fn bug_2169_packed_protected_branches_are_materialized_and_denied() {
    let (_tmp, main, worktree) = packed_fixture();
    let common = main.join(".git");
    let main_sha = bug_1929_git(&main, &["rev-parse", "main"]);
    let git = resolve_contained_git_paths(&worktree).unwrap().unwrap();
    let names: Vec<&str> = git.refs.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["refs/heads/main", "refs/heads/other"], "{git:?}");

    // Without materialization the model reports the packed branches writable:
    // a deny on a non-existent path is skipped.
    let before = settings_with(&worktree, &main, &git);
    for bypass in [
        common.join("refs/heads/main"),
        common.join("refs/heads/other"),
        common.join("logs/refs/heads/main"),
    ] {
        assert!(sandbox_may_write(&before, &bypass), "{bypass:?}: {before}");
    }

    materialize_protected_refs(&git).unwrap();
    let after = settings_with(&worktree, &main, &git);
    for denied in [
        common.join("refs/heads/main"),
        common.join("refs/heads/other"),
        common.join("logs/refs/heads/main"),
        common.join("logs/refs/heads/other"),
    ] {
        assert!(denied.is_file(), "{denied:?}");
        assert!(!sandbox_may_write(&after, &denied), "{denied:?}: {after}");
    }
    assert_eq!(
        std::fs::read_to_string(common.join("refs/heads/main"))
            .unwrap()
            .trim(),
        main_sha.trim()
    );
    assert_eq!(bug_1929_git(&main, &["rev-parse", "main"]), main_sha);
    for writable in [
        common.join("refs/heads/wt"),
        common.join("refs/heads/trunk"),
        common.join("refs/remotes/origin/main"),
    ] {
        assert!(!git.deny.contains(&writable), "{writable:?}");
        assert!(
            sandbox_may_write(&after, &writable),
            "{writable:?}: {after}"
        );
    }
    // Idempotent: a second launch leaves the loose refs as they are.
    materialize_protected_refs(&git).unwrap();
    assert_eq!(bug_1929_git(&main, &["rev-parse", "main"]), main_sha);
}

// DEFAULT BRANCH: `origin/HEAD`'s target is protected even when no worktree
// has it checked out, and the production launch path materializes it.
// trace:BUG-2169 | ai:claude
#[test]
fn bug_2169_default_branch_is_protected_when_packed() {
    let (_tmp, main, worktree) = packed_fixture();
    let common = main.join(".git");
    bug_1929_git(
        &main,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/trunk",
        ],
    );
    let git = resolve_contained_git_paths(&worktree).unwrap().unwrap();
    assert!(
        git.refs.iter().any(|r| r.name == "refs/heads/trunk"),
        "{git:?}"
    );
    let json = crate::session::claude_contained_settings_json(&[worktree.clone()]);
    let settings: serde_json::Value = serde_json::from_str(&json).unwrap();
    for denied in [
        common.join("refs/heads/trunk"),
        common.join("refs/heads/main"),
        common.join("logs/refs/heads/trunk"),
    ] {
        assert!(denied.is_file(), "{denied:?}");
        assert!(!sandbox_may_write(&settings, &denied), "{denied:?}: {json}");
    }
    assert!(
        sandbox_may_write(&settings, &common.join("refs/heads/wt")),
        "{json}"
    );
}

// FAIL CLOSED: a default branch that does not resolve, or a protected ref
// that cannot be materialized (its lock is held), gets no git grant.
// trace:BUG-2169 | ai:claude
#[test]
fn bug_2169_unmaterializable_protected_ref_falls_back_to_worktree_only() {
    let (_tmp, main, worktree) = packed_fixture();
    let common = main.join(".git");
    std::fs::write(common.join("refs/heads/other.lock"), b"").unwrap();
    assert_eq!(contained_git_write_paths(&worktree), None);
    assert!(!common.join("refs/heads/other").exists());
    std::fs::remove_file(common.join("refs/heads/other.lock")).unwrap();

    bug_1929_git(
        &main,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/missing",
        ],
    );
    assert!(resolve_contained_git_paths(&worktree).is_err());
    assert_eq!(contained_git_write_paths(&worktree), None);
}
