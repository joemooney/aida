use super::worktree_pool_hooks_from_config;

fn parse(body: &str) -> toml::Value {
    toml::from_str(body).unwrap()
}

// trace:TASK-1010 | ai:claude
#[test]
fn prewarm_flag_appends_cargo_build_to_post_create() {
    let v = parse("[worktree_pool]\nprewarm_build = true\n");
    let hooks = worktree_pool_hooks_from_config(&v, "post_create");
    assert_eq!(
        hooks,
        vec![aida_core::worktree_hooks::PREWARM_BUILD_COMMAND.to_string()]
    );
}

#[test]
fn prewarm_flag_runs_after_explicit_post_create_hooks() {
    let v = parse("[worktree_pool]\nprewarm_build = true\npost_create = [\"echo hi\"]\n");
    let hooks = worktree_pool_hooks_from_config(&v, "post_create");
    assert_eq!(
        hooks,
        vec![
            "echo hi".to_string(),
            aida_core::worktree_hooks::PREWARM_BUILD_COMMAND.to_string(),
        ]
    );
}

#[test]
fn prewarm_flag_does_not_touch_pre_destroy() {
    let v = parse("[worktree_pool]\nprewarm_build = true\npre_destroy = [\"cargo clean\"]\n");
    let hooks = worktree_pool_hooks_from_config(&v, "pre_destroy");
    assert_eq!(hooks, vec!["cargo clean".to_string()]);
}

#[test]
fn no_prewarm_flag_means_no_injected_hook() {
    let v = parse("[worktree_pool]\nenabled = true\n");
    assert!(worktree_pool_hooks_from_config(&v, "post_create").is_empty());
    let off = parse("[worktree_pool]\nprewarm_build = false\n");
    assert!(worktree_pool_hooks_from_config(&off, "post_create").is_empty());
}
