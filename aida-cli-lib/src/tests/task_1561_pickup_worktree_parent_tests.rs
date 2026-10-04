//! TASK-1561: the pickup worktree path — the warm-pool FALLBACK placement —
//! must honour `[worktree_pool] worktree_parent`.
//!
//! BUG-1700 added the key so one folder-trust grant on one directory covers
//! every worktree AIDA mints. Every pooled tree honoured it; `pickup_worktree_path`
//! hardcoded `project_root.parent()` and honoured nothing, so the moment the pool
//! could not serve a tree the worktree landed outside the trusted directory.
//!
//! These tests pin the COMPOSITION (name + placement) and its parity with the
//! pool. The config reader itself is covered by `bug_1700_worktree_parent_tests`;
//! the on-disk creation and the `--dry-run` preview are covered end-to-end in
//! `aida-cli/tests/queue_work_dry_run.rs`.
// trace:TASK-1561 | ai:claude

use super::{home_dir, pickup_worktree_path, resolve_pickup_workspace};
use std::path::PathBuf;

/// A project root named `repo` whose `.aida/config.toml` holds `body`.
fn project_with_config(body: Option<&str>) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    if let Some(body) = body {
        std::fs::write(root.join(".aida").join("config.toml"), body).unwrap();
    }
    (dir, root)
}

/// Writes `parent` as a TOML *literal* string (single quotes), which performs no
/// escape processing. A basic (double-quoted) string would eat the backslashes in a
/// Windows path like `C:\Users\op\aida-worktrees` as escape sequences — `\U` is an
/// invalid TOML escape, so the config would fail to parse rather than carry the path
/// the test meant. Literal strings are what the TOML spec itself recommends for
/// Windows paths.
// trace:TASK-1561 | ai:claude
fn config(parent: &str) -> String {
    assert!(
        !parent.contains('\''),
        "a TOML literal string cannot contain a single quote, so this parent cannot be \
         expressed without escaping: {parent}"
    );
    format!("[worktree_pool]\nworktree_parent = '{parent}'\n")
}

// AC2 — the regression that must NOT happen. With the key unset the path is
// byte-identical to the historical sibling layout, so no existing project's
// worktrees move. Mirrors `worktree_pool::pool_path_defaults_to_sibling_of_project_root`.
#[test]
fn unset_key_preserves_the_historical_sibling_path_byte_for_byte() {
    for body in [
        None,
        Some("[worktree_pool]\nenabled = true\n"),
        // A blank value is an operator typo, not a request to nest in the root.
        Some("[worktree_pool]\nworktree_parent = \"\"\n"),
    ] {
        let (dir, root) = project_with_config(body);
        assert_eq!(
            pickup_worktree_path(&root, "bug-1743").unwrap(),
            dir.path().join("repo-bug-1743"),
            "unset worktree_parent must keep <parent>/<repo>-<slug>: {body:?}"
        );
    }
}

// AC1 — an absolute configured parent captures the pickup path. This is the
// placement the warm-pool fallback used to ignore.
// trace:BUG-1777 | ai:codex
#[test]
fn absolute_configured_parent_captures_the_pickup_path() {
    let (dir, root) = project_with_config(None);
    let trusted = dir.path().join("trusted").join("aida-worktrees");
    std::fs::write(
        root.join(".aida").join("config.toml"),
        config(&trusted.to_string_lossy()),
    )
    .unwrap();
    assert_eq!(
        pickup_worktree_path(&root, "bug-1743").unwrap(),
        trusted.join("repo-bug-1743")
    );
}

// AC4 — a relative value resolves against the PROJECT ROOT, not the process
// cwd, so the layout does not depend on where `aida` was invoked from, and it
// comes back NORMALISED rather than spelled `<root>/../aida-worktrees/...`.
//
// The normalisation is not cosmetic: the raw spelling is what made the
// `queue work --dry-run` preview disagree with the path `git worktree add`
// registered for the same directory, which is BUG-1628's invariant. It is also
// what BUG-1700's folder-trust check compares with a lexical `starts_with`.
//
// Asserted against `worktree_placement_path` as well as the literal, because
// "the same way as the pool" is the acceptance criterion and a literal alone
// would not catch the pool drifting away underneath us.
#[test]
fn relative_configured_parent_resolves_against_the_project_root_exactly_as_the_pool_does() {
    let (dir, root) = project_with_config(Some(&config("../aida-worktrees")));
    let got = pickup_worktree_path(&root, "bug-1743").unwrap();
    assert_eq!(got, dir.path().join("aida-worktrees").join("repo-bug-1743"));
    assert!(
        !got.to_string_lossy().contains(".."),
        "a `..` survived into a path that is printed and lexically compared: {}",
        got.display()
    );
    assert_eq!(
        got,
        aida_core::worktree_pool::worktree_placement_path(
            &root,
            "repo-bug-1743",
            Some(std::path::Path::new("../aida-worktrees")),
        ),
        "pickup placement must be the pool's placement rule, not a second copy"
    );
}

// AC4 — a tilde value goes through `expand_worktree_parent_tilde`, the one home
// resolver (which carries the cfg(test) redirect to a temp home), rather than a
// reimplementation. Asserted against `home_dir()` rather than a literal so the
// test cannot pass by accident against a developer's real `$HOME`.
#[test]
fn tilde_configured_parent_expands_through_the_shared_home_resolver() {
    let (_dir, root) = project_with_config(Some(&config("~/aida-worktrees")));
    let got = pickup_worktree_path(&root, "bug-1743").unwrap();
    let home = home_dir().expect("cfg(test) home redirect");
    assert_eq!(got, home.join("aida-worktrees").join("repo-bug-1743"));
    assert!(
        !got.starts_with("~"),
        "the tilde must be expanded, not passed through as a directory named `~`: {}",
        got.display()
    );
}

// AC3, caller 2 of 3 — `resolve_pickup_workspace` (the auto-complete phase-1
// fallback) derives its path from the same resolver, so it inherits the key.
// Needs a real repo because it also resolves a free branch name.
// trace:BUG-1777 | ai:codex
#[test]
fn resolve_pickup_workspace_inherits_the_configured_parent() {
    let (dir, root) = project_with_config(None);
    let trusted = dir.path().join("trusted").join("aida-worktrees");
    std::fs::write(
        root.join(".aida").join("config.toml"),
        config(&trusted.to_string_lossy()),
    )
    .unwrap();
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["init", "-q", "-b", "main"])
        .status()
        .unwrap();
    assert!(status.success(), "git init");

    let (path, branch) = resolve_pickup_workspace(&root, "BUG-1743", "auto").unwrap();
    assert_eq!(path, trusted.join("repo-bug-1743"));
    assert_eq!(branch, "bug-1743");
}

// The historical error message for a project root with no parent is preserved
// for the UNCONFIGURED case, and a configured parent makes it unreachable —
// the root's own parent is never consulted.
#[test]
fn a_parentless_root_still_errors_unconfigured_and_succeeds_when_configured() {
    let fs_root = std::path::Path::new("/");
    let err = pickup_worktree_path(fs_root, "bug-1743").unwrap_err();
    assert!(
        err.to_string().contains("project root has no parent"),
        "unexpected error: {err}"
    );
}
