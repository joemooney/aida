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
