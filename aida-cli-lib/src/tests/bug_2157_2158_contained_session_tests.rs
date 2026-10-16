//! BUG-2157 / BUG-2158: more containment escapes under a contained parent.
//!
//! Ruling on BUG-2151 (monotonic containment): a contained parent's env
//! (`AIDA_CONTAINED`, parsed strictly) ALWAYS wins and no flag overrides it.
//!
//! - BUG-2157: `aida session resume`, `session new` and `session start
//!   --launch` launched an uncontained claude (AIDA_CONTAINED stripped,
//!   `--permission-mode` honoured).
//! - BUG-2158: the claude passthrough check was a denylist, so widening flags
//!   (`--add-dir`, `--allowedTools`, `--tools`, `--plugin-dir/url`,
//!   `--mcp-config`, `--agents`) reached a contained child. It is now an
//!   allowlist.
// trace:BUG-2157 BUG-2158 | ai:claude
use crate::session::{
    build_contained_resume_command, refuse_uncontained_session_under_contained_parent, ResumeTarget,
};
use crate::test_env::EnvVarsGuard;
use crate::{
    apply_agent_default_flags, refuse_uncontained_child_of_contained_parent,
    resolve_claude_new_posture, resolve_interactive_launch_mode,
    resolve_interactive_launch_mode_under_parent, AgentDefaultFlagOptions, AgentLaunchConfig,
    AgentPromptStyle,
};

/// A temp project with `agents_toml` as its `.aida/agents.toml`.
fn project(tmp: &std::path::Path, agents_toml: &str) -> (std::path::PathBuf, String) {
    let home = tmp.join("home");
    let root = tmp.join("project");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(root.join(".aida/agents.toml"), agents_toml).unwrap();
    (root, home.to_string_lossy().into_owned())
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|a| a.to_string()).collect()
}

// ── BUG-2157: session new / session start --launch ─────────────────────────

#[test]
fn session_launch_under_a_contained_parent_is_contained_whatever_the_knobs() {
    let tmp = tempfile::tempdir().unwrap();
    for toml in [
        "[agents]\n",
        "[agents]\nbypass = true\n",
        "[agents]\ncontained = false\n",
    ] {
        let (root, _) = project(tmp.path(), toml);
        let mode = resolve_interactive_launch_mode_under_parent(None, false, Some(&root), true)
            .unwrap_or_else(|e| panic!("{toml}: {e:#}"));
        assert!(mode.contained, "{toml}: must be contained");
        assert_eq!(mode.mode, None, "{toml}: no --permission-mode");
    }
}

#[test]
fn session_new_and_start_launch_read_the_parent_env() {
    let _env = EnvVarsGuard::apply(&[("AIDA_CONTAINED", Some("1"))]);
    // `session new` and `session start --launch` both pass launches=true.
    let mode = resolve_interactive_launch_mode(None, false, true).unwrap();
    assert!(mode.contained, "a contained parent contains the launch");
    assert_eq!(mode.mode, None);
}

#[test]
fn session_launch_under_a_contained_parent_refuses_permission_mode() {
    let _env = EnvVarsGuard::apply(&[("AIDA_CONTAINED", Some("1"))]);
    for sandbox in [false, true] {
        let err = resolve_interactive_launch_mode(Some("bypassPermissions"), sandbox, true)
            .expect_err("a contained parent refuses --permission-mode");
        assert!(format!("{err:#}").contains("AIDA_CONTAINED"), "{err:#}");
    }
}

#[test]
fn session_launch_under_a_malformed_aida_contained_fails_closed() {
    let _env = EnvVarsGuard::apply(&[("AIDA_CONTAINED", Some("ture"))]);
    let err = resolve_interactive_launch_mode(None, false, true)
        .expect_err("a typo must not read as uncontained");
    assert!(format!("{err:#}").contains("not recognised"), "{err:#}");
}

#[test]
fn session_start_without_launch_is_not_governed_by_the_parent() {
    let _env = EnvVarsGuard::apply(&[("AIDA_CONTAINED", Some("1"))]);
    // No claude is started, so --permission-mode is not a posture change.
    let mode = resolve_interactive_launch_mode(Some("acceptEdits"), false, false).unwrap();
    assert_eq!(mode.mode.as_deref(), Some("acceptEdits"));
}

#[test]
fn an_uncontained_parent_keeps_the_session_launch_posture() {
    let _env = EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    let mode = resolve_interactive_launch_mode(Some("acceptEdits"), false, true).unwrap();
    assert_eq!(mode.mode.as_deref(), Some("acceptEdits"));
    assert!(!mode.contained);
}

#[test]
fn new_session_guard_refuses_an_uncontained_launch_under_a_contained_parent() {
    let _env = EnvVarsGuard::apply(&[("AIDA_CONTAINED", Some("1"))]);
    for (mode, contained) in [(None, false), (Some("bypassPermissions"), false)] {
        let err = refuse_uncontained_session_under_contained_parent(mode, contained)
            .expect_err("uncontained under a contained parent");
        assert!(format!("{err:#}").contains("AIDA_CONTAINED"), "{err:#}");
    }
    refuse_uncontained_session_under_contained_parent(Some("default"), true)
        .expect_err("--permission-mode alongside containment is refused too");
    refuse_uncontained_session_under_contained_parent(None, true).unwrap();
}

#[test]
fn new_session_guard_is_inert_without_a_contained_parent() {
    let _env = EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    refuse_uncontained_session_under_contained_parent(Some("bypassPermissions"), false).unwrap();
    refuse_uncontained_session_under_contained_parent(None, false).unwrap();
}

// ── BUG-2157: session resume ────────────────────────────────────────────────

fn settings_value(args: &[String]) -> String {
    args.windows(2)
        .find(|w| w[0] == "--settings")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| panic!("no --settings in {args:?}"))
}

fn args_of(cmd: &std::process::Command) -> Vec<String> {
    cmd.get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

fn env_of(cmd: &std::process::Command, key: &str) -> Option<Option<String>> {
    cmd.get_envs()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
}

/// A git worktree (`.git` marker) under `tmp`, canonicalised.
fn worktree(tmp: &std::path::Path, name: &str) -> std::path::PathBuf {
    let wt = std::fs::canonicalize(tmp).unwrap().join(name);
    std::fs::create_dir_all(wt.join(".git")).unwrap();
    wt
}

#[test]
fn contained_resume_keeps_aida_contained_and_bounds_writes_to_the_session_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let session_wt = worktree(tmp.path(), "session-wt");
    let launcher = worktree(tmp.path(), "launcher");
    let target = ResumeTarget {
        agent: "claude".to_string(),
        id: "018f-session".to_string(),
        worktree: Some(session_wt.clone()),
    };
    let cmd = build_contained_resume_command(&target, &launcher).unwrap();
    let args = args_of(&cmd);
    assert_eq!(&args[..2], &s(&["--resume", "018f-session"]), "{args:?}");
    assert!(args.iter().any(|a| a == "--setting-sources"), "{args:?}");
    assert!(
        !args.iter().any(|a| a == "--permission-mode"),
        "no posture override: {args:?}"
    );
    assert_eq!(
        env_of(&cmd, "AIDA_CONTAINED"),
        Some(Some("1".to_string())),
        "the child must keep AIDA_CONTAINED=1"
    );
    assert_eq!(cmd.get_current_dir(), Some(session_wt.as_path()));
    let settings = settings_value(&args);
    assert_eq!(
        settings,
        settings_value(&crate::session::claude_contained_flags_for(&session_wt)),
        "write roots must be the session's worktree"
    );
    assert!(
        !settings.contains(&*launcher.to_string_lossy()),
        "the launcher cwd must not be a write root: {settings}"
    );
}

#[test]
fn contained_resume_falls_back_to_the_launcher_cwd_when_the_session_checkout_is_gone() {
    let tmp = tempfile::tempdir().unwrap();
    let launcher = worktree(tmp.path(), "launcher");
    for worktree in [None, Some(tmp.path().join("deleted"))] {
        let target = ResumeTarget {
            agent: "claude".to_string(),
            id: "018f-session".to_string(),
            worktree,
        };
        let cmd = build_contained_resume_command(&target, &launcher).unwrap();
        assert_eq!(cmd.get_current_dir(), Some(launcher.as_path()));
        assert_eq!(
            settings_value(&args_of(&cmd)),
            settings_value(&crate::session::claude_contained_flags_for(&launcher))
        );
    }
}

#[test]
fn contained_resume_refuses_codex_and_agy_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let launcher = worktree(tmp.path(), "launcher");
    for agent in ["codex", "antigravity"] {
        let target = ResumeTarget {
            agent: agent.to_string(),
            id: "01a067ec".to_string(),
            worktree: None,
        };
        let err = build_contained_resume_command(&target, &launcher)
            .expect_err("codex/agy cannot resume contained");
        assert!(
            format!("{err:#}").contains("refusing to resume"),
            "{agent}: {err:#}"
        );
    }
}

// ── BUG-2158: passthrough allowlist ─────────────────────────────────────────

/// The `agent new claude` pipeline minus the spawn, as BUG-2151's tests run it.
fn claude_launch(root: &std::path::Path, extra_flags: Vec<String>) -> anyhow::Result<Vec<String>> {
    let parent = crate::session::aida_contained_env()?;
    let posture = resolve_claude_new_posture(None, false, false, parent, || {
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
    Ok(config.default_args)
}

/// Every widening flag the BUG-2158 report names, plus an unknown one, in
/// each spelling a passthrough can take.
fn widening_flag_sets() -> Vec<Vec<String>> {
    let mut sets = Vec::new();
    for (flag, value) in [
        ("--add-dir", "/"),
        ("--allowedTools", "Edit(//**)"),
        ("--allowed-tools", "Bash"),
        ("--tools", "Bash,Edit"),
        ("--plugin-dir", "/opt/plugin"),
        ("--plugin-url", "https://example.invalid/p.zip"),
        ("--mcp-config", "{}"),
        ("--agents", "{}"),
        ("--frobnicate", "x"),
    ] {
        sets.push(vec![format!("{flag}={value}")]);
        sets.push(s(&[flag, value]));
        sets.push(s(&[flag]));
    }
    sets
}

#[test]
fn contained_parent_refuses_every_widening_extra_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), "[agents]\n");
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", Some("1"))]);
    for extra in widening_flag_sets() {
        let err = claude_launch(&root, extra.clone())
            .expect_err("a widening passthrough must not reach a contained child");
        let msg = format!("{err:#}");
        assert!(msg.contains("--extra-flag"), "{extra:?}: {msg}");
        assert!(msg.contains("--model"), "names the allowlist: {msg}");
    }
}

#[test]
fn contained_parent_refuses_every_widening_agents_toml_default_flag() {
    let tmp = tempfile::tempdir().unwrap();
    for flags in widening_flag_sets() {
        let quoted: Vec<String> = flags.iter().map(|f| format!("{f:?}")).collect();
        let (root, home) = project(
            tmp.path(),
            &format!(
                "[agents]\n[agents.claude]\ndefault_flags = [{}]\n",
                quoted.join(", ")
            ),
        );
        let _env =
            EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", Some("1"))]);
        let err = claude_launch(&root, Vec::new())
            .expect_err("a widening default must not reach a contained child");
        assert!(
            format!("{err:#}").contains("agents.toml"),
            "{flags:?}: {err:#}"
        );
    }
}

#[test]
fn contained_parent_refuses_a_posture_default_instead_of_dropping_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(
        tmp.path(),
        "[agents]\n[agents.claude]\ndefault_flags = [\"--dangerously-skip-permissions\"]\n",
    );
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", Some("1"))]);
    claude_launch(&root, Vec::new()).expect_err("refused, not silently dropped");
}

#[test]
fn contained_parent_refuses_malformed_allowlisted_spellings() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), "[agents]\n");
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", Some("1"))]);
    for extra in [
        s(&["--model"]),                    // value missing
        s(&["--model", "--add-dir=/"]),     // value is a flag
        s(&["--verbose=--add-dir=/"]),      // boolean given a value
        s(&["-m", "opus"]),                 // short spelling
        s(&["stray-positional"]),           // not a flag
        s(&["--model", "opus", "--tools"]), // allowed then refused
    ] {
        claude_launch(&root, extra.clone()).expect_err(&format!("{extra:?} must be refused"));
    }
}

#[test]
fn contained_parent_accepts_the_allowlisted_flags() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(
        tmp.path(),
        "[agents]\n[agents.claude]\ndefault_flags = [\"--effort=high\"]\n",
    );
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", Some("1"))]);
    let args = claude_launch(&root, s(&["--model", "opus", "--verbose"])).unwrap();
    for want in [
        "--effort=high",
        "--model",
        "opus",
        "--verbose",
        "--settings",
    ] {
        assert!(args.iter().any(|a| a == want), "{want} missing: {args:?}");
    }
}

#[test]
fn an_uncontained_parent_passes_extra_flags_through_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), "[agents]\n");
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", None)]);
    let args = claude_launch(&root, s(&["--add-dir=/srv/extra", "--allowedTools=Edit"])).unwrap();
    assert!(args.iter().any(|a| a == "--add-dir=/srv/extra"), "{args:?}");
}
