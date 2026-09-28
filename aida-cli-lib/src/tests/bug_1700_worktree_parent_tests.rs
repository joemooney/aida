// `[worktree_pool] worktree_parent` — the opt-in single parent directory for
// AIDA-created worktrees (BUG-1700). Folder trust inherits from a parent
// directory, so nesting every worktree under one parent lets the operator grant
// trust ONCE instead of hitting a modal per fresh spec worktree.
// trace:BUG-1700 | ai:claude

use super::{expand_tilde_against, worktree_pool_config_worktree_parent};
use std::path::{Path, PathBuf};

/// A project root whose `.aida/config.toml` holds `body` (no config file when
/// `body` is None).
fn project_with_config(body: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    if let Some(body) = body {
        let aida = dir.path().join(".aida");
        std::fs::create_dir_all(&aida).unwrap();
        std::fs::write(aida.join("config.toml"), body).unwrap();
    }
    dir
}

#[test]
fn unset_when_there_is_no_config_file() {
    let dir = project_with_config(None);
    assert_eq!(worktree_pool_config_worktree_parent(dir.path()), None);
}

#[test]
fn unset_when_the_key_is_absent() {
    let dir = project_with_config(Some("[worktree_pool]\nenabled = true\nmax_trees = 4\n"));
    assert_eq!(
        worktree_pool_config_worktree_parent(dir.path()),
        None,
        "an existing [worktree_pool] table without the key must not opt in"
    );
}

#[test]
fn reads_a_relative_value_verbatim() {
    let dir = project_with_config(Some(
        "[worktree_pool]\nworktree_parent = \"../aida-worktrees\"\n",
    ));
    assert_eq!(
        worktree_pool_config_worktree_parent(dir.path()),
        Some(PathBuf::from("../aida-worktrees")),
        "relative values stay relative; the pool resolves them against the project root"
    );
}

#[test]
fn reads_an_absolute_value() {
    let dir = project_with_config(Some(
        "[worktree_pool]\nworktree_parent = \"/trusted/aida-worktrees\"\n",
    ));
    assert_eq!(
        worktree_pool_config_worktree_parent(dir.path()),
        Some(PathBuf::from("/trusted/aida-worktrees"))
    );
}

// A blank or whitespace-only value is an operator typo, not a request to nest
// worktrees in the project root itself — treat it as unset.
#[test]
fn blank_value_is_treated_as_unset() {
    for body in [
        "[worktree_pool]\nworktree_parent = \"\"\n",
        "[worktree_pool]\nworktree_parent = \"   \"\n",
    ] {
        let dir = project_with_config(Some(body));
        assert_eq!(
            worktree_pool_config_worktree_parent(dir.path()),
            None,
            "blank worktree_parent must not opt in: {body:?}"
        );
    }
}

#[test]
fn surrounding_whitespace_is_trimmed() {
    let dir = project_with_config(Some(
        "[worktree_pool]\nworktree_parent = \"  /trusted/wt  \"\n",
    ));
    assert_eq!(
        worktree_pool_config_worktree_parent(dir.path()),
        Some(PathBuf::from("/trusted/wt"))
    );
}

// Malformed TOML must not panic or abort a launch — it degrades to "unset",
// which keeps the historical sibling layout.
#[test]
fn malformed_config_is_unset_rather_than_fatal() {
    let dir = project_with_config(Some("[worktree_pool\nworktree_parent = "));
    assert_eq!(worktree_pool_config_worktree_parent(dir.path()), None);
}

#[test]
fn non_string_value_is_unset() {
    let dir = project_with_config(Some("[worktree_pool]\nworktree_parent = 42\n"));
    assert_eq!(worktree_pool_config_worktree_parent(dir.path()), None);
}

#[test]
fn tilde_expands_against_home() {
    let home = Some(PathBuf::from("/home/op"));
    assert_eq!(
        expand_tilde_against("~/aida-worktrees", home.clone()),
        PathBuf::from("/home/op/aida-worktrees")
    );
    assert_eq!(expand_tilde_against("~", home), PathBuf::from("/home/op"));
}

// With no $HOME the literal value is preserved, so the operator sees the path
// they wrote instead of a silently rehomed one.
#[test]
fn tilde_is_left_literal_when_home_is_unknown() {
    assert_eq!(
        expand_tilde_against("~/aida-worktrees", None),
        PathBuf::from("~/aida-worktrees")
    );
}

#[test]
fn a_path_merely_containing_a_tilde_is_untouched() {
    let home = Some(PathBuf::from("/home/op"));
    assert_eq!(
        expand_tilde_against("/srv/~backup/wt", home),
        Path::new("/srv/~backup/wt")
    );
}
