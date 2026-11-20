// TASK-1472: bare `aida` is a context-aware shortcut for `aida agent new`.
// trace:TASK-1472 | ai:claude
use super::*;

#[test]
fn interactive_initialized_project_opens_the_agent_picker() {
    assert_eq!(
        bare_aida_action(false, true, true),
        BareAidaAction::AgentPicker
    );
}

#[test]
fn agent_output_mode_keeps_the_status_snapshot() {
    // Agent mode wins regardless of terminal or project state (TASK-970).
    for interactive in [false, true] {
        for initialized in [false, true] {
            assert_eq!(
                bare_aida_action(true, interactive, initialized),
                BareAidaAction::StatusSnapshot,
                "interactive={interactive} initialized={initialized}"
            );
        }
    }
}

#[test]
fn outside_a_project_shows_onboarding() {
    assert_eq!(
        bare_aida_action(false, true, false),
        BareAidaAction::Onboarding
    );
}

#[test]
fn non_interactive_human_output_never_opens_the_picker() {
    // A piped stdin cannot answer a picker, even inside a project.
    assert_eq!(
        bare_aida_action(false, false, true),
        BareAidaAction::Onboarding
    );
}

#[test]
fn initialized_project_markers() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert!(!project_is_aida_initialized(root), "plain directory");

    std::fs::create_dir_all(root.join(".aida")).unwrap();
    assert!(
        !project_is_aida_initialized(root),
        "an empty .aida directory is not an initialized project"
    );
    std::fs::write(root.join(".aida").join("config.toml"), "").unwrap();
    assert!(project_is_aida_initialized(root), ".aida/config.toml");

    let legacy = tempfile::tempdir().unwrap();
    std::fs::write(legacy.path().join("requirements.db"), "").unwrap();
    assert!(
        project_is_aida_initialized(legacy.path()),
        "requirements.db"
    );

    let store = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(store.path().join(".aida-store")).unwrap();
    assert!(project_is_aida_initialized(store.path()), ".aida-store");

    #[cfg(unix)]
    {
        let dangling = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(
            dangling.path().join("missing-store"),
            dangling.path().join(".aida-store"),
        )
        .unwrap();
        assert!(
            project_is_aida_initialized(dangling.path()),
            "a dangling .aida-store symlink still marks the project"
        );
    }
}

#[test]
fn picker_shortcut_parses_as_agent_new() {
    let argv = bare_aida_picker_argv("aida");
    assert_eq!(argv, vec!["aida", "agent", "new"]);
    // The rewritten argv is exactly the explicit `aida agent new` form, so it
    // reaches the same picker (no tool given → the agent-type picker).
    let explicit = vec!["aida".to_string(), "agent".to_string(), "new".to_string()];
    assert_eq!(rewrite_agent_default_new(&argv), explicit);
    crate::TEST_CLI
        .try_parse_from(argv)
        .expect("bare picker argv parses");
}
