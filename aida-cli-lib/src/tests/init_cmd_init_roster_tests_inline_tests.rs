use super::*;

// trace:STORY-1540 | ai:antigravity
// trace:TASK-1970 | ai:antigravity
#[test]
fn init_at_non_tty_writes_no_team_roster() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_AGENT_NAME", Some("test-agent")),
        ("AIDA_AGENT_TYPE", Some("test")),
    ]);
    let prev_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(root).unwrap();

    crate::test_git::git_command()
        .arg("init")
        .current_dir(root)
        .status()
        .unwrap();
    crate::test_git::git_command()
        .args(["commit", "--allow-empty", "-m", "init"])
        .current_dir(root)
        .status()
        .unwrap();

    // trace:BUG-2017 | ai:codex — positive initializer fixture models HOME.
    crate::test_env::enroll_synthetic_home(root).unwrap();
    super::handle_init_distributed_worktree(
        false, false, None, false, false, false, None, false, false, None, None,
    )
    .expect("init should succeed");

    let team_toml = root.join(".aida-store/registry/team.toml");
    assert!(
        !team_toml.exists(),
        "no roster should be written without a direct TTY"
    );

    std::env::set_current_dir(prev_cwd).unwrap();
}
