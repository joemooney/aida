//! BUG-2144 / BUG-2145 / BUG-2146: containment is monotonic and fails closed.
//!
//! Advisor ruling on BUG-2133 (2026-10-14): a contained drain contains EVERY
//! agent child, and a containment request that cannot be honoured is refused —
//! never silently downgraded.
//!
//! - BUG-2144: a contained launch on a vendor that cannot apply containment
//!   (codex, agy) is refused instead of running uncontained.
//! - BUG-2145: `queue work --auto-complete --sandbox` contains the drain.
//! - BUG-2146: a malformed `agents.toml` refuses the launch, naming the file,
//!   instead of reading as "uncontained".
// trace:BUG-2144 BUG-2145 BUG-2146 | ai:claude
use crate::session::{self, HeadlessVendor};
use crate::test_env::EnvVarsGuard;

// ── BUG-2144 ────────────────────────────────────────────────────────────────

#[test]
fn contained_request_on_codex_or_agy_is_refused_and_claude_passes() {
    for vendor in [HeadlessVendor::Codex, HeadlessVendor::Agy] {
        let err = session::require_vendor_containment(vendor, true, "queue-work session")
            .expect_err("contained codex/agy must be refused");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("refusing to launch the queue-work session uncontained"),
            "{msg}"
        );
        assert!(msg.contains(vendor.as_str()), "{msg}");
        assert!(
            msg.contains("--vendor claude"),
            "hint names the recovery: {msg}"
        );
        // Uncontained launches on those vendors are untouched.
        session::require_vendor_containment(vendor, false, "queue-work session").unwrap();
    }
    session::require_vendor_containment(HeadlessVendor::Claude, true, "x").unwrap();
    session::require_vendor_containment(HeadlessVendor::Claude, false, "x").unwrap();
}

#[test]
fn compose_headless_command_refuses_contained_codex_and_agy() {
    for vendor in [HeadlessVendor::Codex, HeadlessVendor::Agy] {
        let err = session::compose_headless_command(vendor, "p", "sid", true, None, None)
            .expect_err("the headless choke point refuses a contained codex/agy argv");
        assert!(format!("{err:#}").contains("refusing to launch"), "{err:#}");
    }
}

#[test]
fn contained_codex_reviewer_plan_is_refused() {
    let err = session::interactive_reviewer_launch_plan(
        HeadlessVendor::Codex,
        None,
        None,
        "review",
        "sid",
        true,
    )
    .expect_err("contained codex reviewer is refused");
    assert!(
        format!("{err:#}").contains("refusing to launch the reviewer session"),
        "{err:#}"
    );
}

// ── BUG-2146 ────────────────────────────────────────────────────────────────

fn malformed_project(tmp: &std::path::Path) -> (std::path::PathBuf, String) {
    let home = tmp.join("home");
    let root = tmp.join("project");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    // Unterminated table header + a contained knob the operator meant to set.
    std::fs::write(
        root.join(".aida/agents.toml"),
        "[agents\ncontained = true\n",
    )
    .unwrap();
    (root, home.to_string_lossy().into_owned())
}

#[test]
fn malformed_agents_toml_is_a_hard_error_naming_the_file_for_the_drain() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home_s) = malformed_project(tmp.path());
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home_s)), ("AIDA_CONTAINED", None)]);
    let err = session::drain_contained(&root)
        .expect_err("a malformed agents.toml must not read as uncontained");
    let msg = format!("{err:#}");
    assert!(msg.contains("refusing to launch"), "{msg}");
    assert!(
        msg.contains(&root.join(".aida/agents.toml").display().to_string()),
        "the error names the file: {msg}"
    );
}

#[test]
fn malformed_agents_toml_refuses_the_interactive_launch_posture() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home_s) = malformed_project(tmp.path());
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home_s)), ("AIDA_CONTAINED", None)]);
    let err =
        crate::agent_launch::resolve_interactive_launch_mode_for_root(None, false, Some(&root))
            .err()
            .expect("a malformed agents.toml refuses the launch");
    assert!(
        format!("{err:#}").contains(&root.join(".aida/agents.toml").display().to_string()),
        "{err:#}"
    );
}

#[test]
fn a_malformed_agents_toml_never_constructs_an_uncontained_drain_driver() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home_s) = malformed_project(tmp.path());
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home_s)), ("AIDA_CONTAINED", None)]);
    let d = crate::phase_driver::RealPhaseDriver::new(
        root,
        "BUG-2146".into(),
        "test".into(),
        None,
        true,
        None,
        crate::phase_driver::AutonomyMode::Default,
        "tok".into(),
        false,
        false,
        false,
        false,
        crate::auto_complete::LifecycleSkip::none(),
        crate::auto_complete::AutoCompleteVariant::Full,
    );
    assert!(d.contained, "a residual read error fails CLOSED");
}

// ── BUG-2145 ────────────────────────────────────────────────────────────────

#[test]
fn auto_complete_sandbox_contains_the_drain() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let root = tmp.path().join("project");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    let home_s = home.to_string_lossy().into_owned();

    // Without --sandbox (and no env / knob): the drain stays uncontained.
    {
        let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home_s)), ("AIDA_CONTAINED", None)]);
        crate::queue_cmd::honour_auto_complete_sandbox(false, None).unwrap();
        assert!(std::env::var_os("AIDA_CONTAINED").is_none());
        assert!(!session::drain_contained(&root).unwrap());
    }
    // With --sandbox: the drain (and every child via the env) is contained.
    {
        let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home_s)), ("AIDA_CONTAINED", None)]);
        crate::queue_cmd::honour_auto_complete_sandbox(true, None).unwrap();
        assert_eq!(std::env::var("AIDA_CONTAINED").as_deref(), Ok("1"));
        assert!(session::drain_contained(&root).unwrap());
    }
}

#[test]
fn auto_complete_sandbox_with_permission_mode_is_refused() {
    let _env = EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    let err = crate::queue_cmd::honour_auto_complete_sandbox(true, Some("bypassPermissions"))
        .expect_err("--sandbox and --permission-mode are exclusive");
    assert!(format!("{err:#}").contains("mutually exclusive"), "{err:#}");
    assert!(
        std::env::var_os("AIDA_CONTAINED").is_none(),
        "a refused combination must not leave the env half-applied"
    );
}

// ── #2665 review round 2 ────────────────────────────────────────────────────

#[test]
fn wrong_typed_contained_knob_is_a_hard_error_naming_file_and_key() {
    for bad in ["contained = \"true\"", "contained = 1"] {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let root = tmp.path().join("project");
        std::fs::create_dir_all(home.join(".aida")).unwrap();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(root.join(".aida/agents.toml"), format!("[agents]\n{bad}\n")).unwrap();
        let home_s = home.to_string_lossy().into_owned();
        let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home_s)), ("AIDA_CONTAINED", None)]);
        let err = crate::agent_config::load_agents_contained(&root)
            .expect_err("a wrong-typed contained knob must not read as false");
        let msg = format!("{err:#}");
        assert!(msg.contains("`[agents] contained`"), "{bad}: {msg}");
        assert!(msg.contains("must be a boolean"), "{bad}: {msg}");
        assert!(
            msg.contains(&root.join(".aida/agents.toml").display().to_string()),
            "{bad}: {msg}"
        );
        assert!(session::drain_contained(&root).is_err(), "{bad}");
    }
}

#[test]
fn wrong_typed_contained_enable_in_config_toml_is_a_hard_error() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let root = tmp.path().join("project");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[contained]\nenable = \"yes\"\n",
    )
    .unwrap();
    let home_s = home.to_string_lossy().into_owned();
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home_s)), ("AIDA_CONTAINED", None)]);
    let err = crate::agent_config::load_agents_contained(&root).unwrap_err();
    assert!(
        format!("{err:#}").contains("`[contained] enable`"),
        "{err:#}"
    );
}

#[test]
fn aida_contained_env_is_case_insensitive_and_rejects_unknown_values() {
    for on in ["1", "true", "TRUE", "Yes", "on", "ON", " true "] {
        assert!(session::parse_aida_contained(on).unwrap(), "{on:?}");
    }
    for off in ["", "0", "false", "False", "no", "OFF"] {
        assert!(!session::parse_aida_contained(off).unwrap(), "{off:?}");
    }
    for bad in ["ture", "2", "enabled", "y"] {
        let err = session::parse_aida_contained(bad).expect_err(bad);
        assert!(format!("{err:#}").contains("not recognised"), "{err:#}");
    }
    let _env = EnvVarsGuard::apply(&[("AIDA_CONTAINED", Some("ture"))]);
    assert!(session::aida_contained_env().is_err());
    assert!(session::drain_contained(std::path::Path::new("/nonexistent")).is_err());
}

#[test]
fn contained_agent_new_refuses_codex_agy_and_unknown_tools_even_with_per_tool_flags() {
    for tool in ["codex", "antigravity", "agy", "something-else"] {
        let err = crate::refuse_uncontainable_agent_tool(tool).expect_err(tool);
        assert!(
            format!("{err:#}").contains("refusing to launch it"),
            "{err:#}"
        );
    }
    crate::refuse_uncontainable_agent_tool("claude").unwrap();

    // Per-tool default_flags (e.g. agy's --dangerously-skip-permissions) must
    // not slip a contained project past the refusal.
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let root = tmp.path().join("project");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/agents.toml"),
        "[agents]\ncontained = true\n\n[agents.antigravity]\ndefault_flags = [\"--dangerously-skip-permissions\"]\n",
    )
    .unwrap();
    let home_s = home.to_string_lossy().into_owned();
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home_s)), ("AIDA_CONTAINED", None)]);
    let mut cfg = crate::AgentLaunchConfig {
        agent_type: "antigravity",
        binary: "agy",
        default_args: Vec::new(),
        prompt_style: crate::AgentPromptStyle::Positional,
        contained: false,
    };
    let err = crate::apply_agent_default_flags(
        &mut cfg,
        &root,
        crate::AgentDefaultFlagOptions::new(true, Vec::new(), None),
        false,
    )
    .expect_err("contained agy with per-tool flags is refused");
    assert!(
        format!("{err:#}").contains("refusing to launch it"),
        "{err:#}"
    );
}
