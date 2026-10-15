//! BUG-2151: `aida agent new` honours a contained parent, and `--bg` is not a
//! posture flag.
//!
//! Ruling on BUG-2133 (monotonic containment): a contained parent's env
//! (`AIDA_CONTAINED`, parsed strictly) ALWAYS wins and no flag overrides it;
//! only real operator posture flags (`--permission-mode`, `--bypass-sandbox`)
//! may override the `[agents] contained` CONFIG knob, never the env; `--bg`
//! is not a posture flag.
// trace:BUG-2151 | ai:claude
use crate::test_env::EnvVarsGuard;
use crate::{
    apply_agent_default_flags, refuse_uncontained_child_of_contained_parent,
    resolve_claude_new_posture, AgentDefaultFlagOptions, AgentLaunchConfig, AgentPromptStyle,
};

fn has_bypass(args: &[String]) -> bool {
    args.iter().any(|a| {
        a == "bypassPermissions" || a.starts_with("--dangerously") || a == "--permission-mode"
    })
}

/// A temp project whose `[agents] contained` knob is `knob`.
fn project(tmp: &std::path::Path, knob: bool) -> (std::path::PathBuf, String) {
    let home = tmp.join("home");
    let root = tmp.join("project");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/agents.toml"),
        format!("[agents]\ncontained = {knob}\n"),
    )
    .unwrap();
    (root, home.to_string_lossy().into_owned())
}

/// The `agent new claude` pipeline minus the spawn: posture → agents.toml
/// defaults → the contained-parent guard.
fn claude_launch(
    root: &std::path::Path,
    permission_mode: Option<&str>,
    bg: bool,
    extra_flags: Vec<String>,
) -> anyhow::Result<AgentLaunchConfig> {
    let parent = crate::session::aida_contained_env()?;
    let posture = resolve_claude_new_posture(permission_mode, false, bg, parent, || {
        crate::agent_config::load_agents_contained(root)
    })?;
    let mut config = AgentLaunchConfig {
        agent_type: "claude",
        binary: "claude",
        default_args: posture.default_args,
        prompt_style: AgentPromptStyle::Positional,
        contained: posture.contained,
    };
    apply_agent_default_flags(
        &mut config,
        root,
        AgentDefaultFlagOptions::new(true, extra_flags, None),
        posture.explicit,
    )?;
    refuse_uncontained_child_of_contained_parent(&config)?;
    Ok(config)
}

// ── (1) a contained parent contains the child ───────────────────────────────

#[test]
fn contained_parent_contains_claude_child_with_the_knob_off() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), false);
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", Some("1"))]);
    let cfg = claude_launch(&root, None, false, Vec::new()).unwrap();
    assert!(cfg.contained, "the child must be contained");
    assert!(crate::agent_launch::is_contained_launch(&cfg));
    assert!(
        cfg.default_args.iter().any(|a| a == "--settings"),
        "{:?}",
        cfg.default_args
    );
    assert!(!has_bypass(&cfg.default_args), "{:?}", cfg.default_args);
}

#[test]
fn contained_parent_refuses_a_permission_mode_override() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), false);
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", Some("1"))]);
    let err = claude_launch(&root, Some("bypassPermissions"), false, Vec::new())
        .expect_err("a contained parent refuses --permission-mode");
    assert!(format!("{err:#}").contains("AIDA_CONTAINED"), "{err:#}");
}

#[test]
fn contained_parent_refuses_a_passthrough_posture_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), false);
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", Some("1"))]);
    for extra in [
        vec!["--dangerously-skip-permissions".to_string()],
        vec![
            "--permission-mode".to_string(),
            "bypassPermissions".to_string(),
        ],
        vec!["--settings".to_string(), "{}".to_string()],
    ] {
        let err = claude_launch(&root, None, false, extra.clone())
            .expect_err("a `--` posture passthrough must not escape containment");
        assert!(
            format!("{err:#}").contains("refusing to launch"),
            "{extra:?}: {err:#}"
        );
    }
}

#[test]
fn contained_parent_refuses_codex_and_agy_even_with_bypass_sandbox() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), false);
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", Some("1"))]);
    for (agent_type, binary, bypass) in [
        (
            "codex",
            "codex",
            "--dangerously-bypass-approvals-and-sandbox",
        ),
        ("antigravity", "agy", "--dangerously-skip-permissions"),
    ] {
        for explicit in [false, true] {
            let mut cfg = AgentLaunchConfig {
                agent_type,
                binary,
                default_args: if explicit {
                    vec![bypass.to_string()]
                } else {
                    Vec::new()
                },
                prompt_style: AgentPromptStyle::Positional,
                contained: false,
            };
            apply_agent_default_flags(
                &mut cfg,
                &root,
                AgentDefaultFlagOptions::new(true, Vec::new(), None),
                explicit,
            )
            .unwrap();
            let err = refuse_uncontained_child_of_contained_parent(&cfg)
                .expect_err("a contained parent never launches codex/agy");
            assert!(
                format!("{err:#}").contains("cannot run an AIDA-contained session"),
                "{agent_type}: {err:#}"
            );
        }
    }
}

#[test]
fn a_malformed_aida_contained_fails_closed_on_agent_new() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), false);
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", Some("ture"))]);
    let err = claude_launch(&root, None, false, Vec::new()).expect_err("typo must refuse");
    assert!(format!("{err:#}").contains("not recognised"), "{err:#}");
    let cfg = AgentLaunchConfig {
        agent_type: "codex",
        binary: "codex",
        default_args: Vec::new(),
        prompt_style: AgentPromptStyle::Positional,
        contained: false,
    };
    assert!(refuse_uncontained_child_of_contained_parent(&cfg).is_err());
}

#[test]
fn an_uncontained_parent_is_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), false);
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", None)]);
    let cfg = claude_launch(&root, Some("acceptEdits"), false, Vec::new()).unwrap();
    assert!(!cfg.contained);
    let cfg = claude_launch(&root, None, false, Vec::new()).unwrap();
    assert!(!cfg.contained);
}

// ── (2) `--bg` is not a posture flag ────────────────────────────────────────

#[test]
fn bg_under_a_contained_parent_stays_contained() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), false);
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", Some("1"))]);
    let cfg = claude_launch(&root, None, true, Vec::new()).unwrap();
    assert!(cfg.contained);
    assert!(!has_bypass(&cfg.default_args), "{:?}", cfg.default_args);
}

#[test]
fn bg_under_the_contained_knob_stays_contained() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), true);
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", None)]);
    let cfg = claude_launch(&root, None, true, Vec::new()).unwrap();
    assert!(cfg.contained, "--bg must not escape [agents] contained");
    assert!(crate::agent_launch::is_contained_launch(&cfg));
    assert!(!has_bypass(&cfg.default_args), "{:?}", cfg.default_args);
}

#[test]
fn bg_without_containment_still_forces_bypass() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), false);
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", None)]);
    let cfg = claude_launch(&root, None, true, Vec::new()).unwrap();
    assert!(!cfg.contained);
    assert!(cfg
        .default_args
        .windows(2)
        .any(|w| w == ["--permission-mode", "bypassPermissions"]));
}

#[test]
fn an_operator_posture_flag_still_overrides_the_config_knob() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), true);
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", None)]);
    let cfg = claude_launch(&root, Some("acceptEdits"), true, Vec::new()).unwrap();
    assert!(!cfg.contained);
    assert!(cfg
        .default_args
        .windows(2)
        .any(|w| w == ["--permission-mode", "acceptEdits"]));
}
