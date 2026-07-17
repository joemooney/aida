use super::resolve_worktree_pool_enabled;

fn project_with_config(body: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    std::fs::write(dir.path().join(".aida").join("config.toml"), body).unwrap();
    dir
}

#[test]
fn flag_overrides_config_both_ways() {
    let on = project_with_config("[worktree_pool]\nenabled = true\n");
    // --no-pool (Some(false)) wins over enabled = true
    assert!(!resolve_worktree_pool_enabled(Some(false), on.path()));
    let off = project_with_config("[worktree_pool]\nenabled = false\n");
    // --pool (Some(true)) wins over enabled = false
    assert!(resolve_worktree_pool_enabled(Some(true), off.path()));
}

#[test]
fn config_decides_when_no_flag() {
    let on = project_with_config("[worktree_pool]\nenabled = true\n");
    assert!(resolve_worktree_pool_enabled(None, on.path()));
    let off = project_with_config("[worktree_pool]\nenabled = false\n");
    assert!(!resolve_worktree_pool_enabled(None, off.path()));
}

#[test]
fn defaults_on_when_absent() {
    // TASK-985: pooling is ON by default — an absent [worktree_pool]
    // section (or missing config file) resolves to enabled.
    let bare = project_with_config("[other]\nx = 1\n");
    assert!(resolve_worktree_pool_enabled(None, bare.path()));
    let empty = tempfile::tempdir().unwrap();
    assert!(resolve_worktree_pool_enabled(None, empty.path()));
    // But an explicit `enabled = false` still opts out.
    let off = project_with_config("[worktree_pool]\nenabled = false\n");
    assert!(!resolve_worktree_pool_enabled(None, off.path()));
}
