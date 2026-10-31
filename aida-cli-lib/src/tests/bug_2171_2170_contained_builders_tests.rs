//! BUG-2171 / BUG-2170: Contained parent escape via burndown run, autopilot intake,
//! and questions clarify builders (BUG-2171), and PURPOSE scrub + child grant revocation (BUG-2170).
//!
//! Monotonic containment ruling (BUG-2151): a contained parent's env
//! (`AIDA_CONTAINED`, parsed strictly) ALWAYS wins and no flag overrides it.
//! Under a contained parent:
//! - `--permission-mode` must be refused for burndown run and autopilot intake.
//! - The launch posture defaults to `dontAsk` for burndown run and autopilot intake.
//! - Containment flags (`claude_contained_flags().unwrap()`) are added to argv.
//! - Interactive questions clarify adds containment flags to argv.
//! - `AIDA_CONTAINED=1` is exported to child commands.
//! - BUG-050: every headless lane (contained or not) carries the headless
//!   posture: one `--settings` turning background tasks off and denying
//!   `Monitor`, and `--disallowed-tools AskUserQuestion Monitor`.
//! - `AIDA_SESSION_PURPOSE` is scrubbed from all child commands (BUG-2170).
//! - Child grants are revoked on exit and on spawn failure (BUG-2170).
// trace:BUG-2171 trace:BUG-2170 | ai:antigravity

use crate::autopilot_intake::build_autopilot_intake_command;
use crate::burndown_run::{
    build_burndown_quiet_command, build_burndown_verbose_command,
    burndown_verbose_claude_args_with_containment,
};
use crate::questions::build_questions_clarify_command;
use crate::session::resolve_headless_claude_posture;
use crate::test_env::EnvVarsGuard;

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

#[test]
fn contained_parent_burndown_run_refuses_permission_mode() {
    let _env = EnvVarsGuard::apply(&[("AIDA_CONTAINED", Some("1"))]);

    for mode in ["bypassPermissions", "acceptEdits", "dontAsk", "auto"] {
        let err = resolve_headless_claude_posture(Some(mode), true)
            .expect_err("contained parent must refuse --permission-mode for burndown run");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("[contained] this session is contained (AIDA_CONTAINED)"),
            "{mode}: {msg}"
        );
        assert!(
            msg.contains(&format!("refusing `--permission-mode {mode}`")),
            "{mode}: {msg}"
        );
    }
}

#[test]
fn contained_parent_burndown_run_resolves_to_dont_ask() {
    let _env = EnvVarsGuard::apply(&[("AIDA_CONTAINED", Some("1"))]);

    let mode = resolve_headless_claude_posture(None, true)
        .expect("contained parent without --permission-mode succeeds");
    assert_eq!(mode, "dontAsk");
}

#[test]
fn uncontained_burndown_run_preserves_mode_or_defaults_to_bypass() {
    let explicit = resolve_headless_claude_posture(Some("acceptEdits"), false).unwrap();
    assert_eq!(explicit, "acceptEdits");

    let default_mode = resolve_headless_claude_posture(None, false).unwrap();
    assert_eq!(default_mode, "bypassPermissions");
}

#[test]
fn contained_parent_autopilot_intake_refuses_permission_mode() {
    let _env = EnvVarsGuard::apply(&[("AIDA_CONTAINED", Some("1"))]);

    for mode in ["bypassPermissions", "acceptEdits", "dontAsk", "auto"] {
        let err = resolve_headless_claude_posture(Some(mode), true)
            .expect_err("contained parent must refuse --permission-mode for autopilot intake");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("[contained] this session is contained (AIDA_CONTAINED)"),
            "{mode}: {msg}"
        );
        assert!(
            msg.contains(&format!("refusing `--permission-mode {mode}`")),
            "{mode}: {msg}"
        );
    }
}

#[test]
fn contained_parent_autopilot_intake_resolves_to_dont_ask() {
    let _env = EnvVarsGuard::apply(&[("AIDA_CONTAINED", Some("1"))]);

    let mode = resolve_headless_claude_posture(None, true)
        .expect("contained parent without --permission-mode succeeds");
    assert_eq!(mode, "dontAsk");
}

#[test]
fn uncontained_autopilot_intake_preserves_mode_or_defaults_to_bypass() {
    let explicit = resolve_headless_claude_posture(Some("acceptEdits"), false).unwrap();
    assert_eq!(explicit, "acceptEdits");

    let default_mode = resolve_headless_claude_posture(None, false).unwrap();
    assert_eq!(default_mode, "bypassPermissions");
}

#[test]
fn burndown_quiet_builder_argv_and_env_contained_vs_uncontained() {
    // Contained
    let cmd =
        build_burndown_quiet_command("prompt-quiet", "dontAsk", "", "CEILING_KEY", "10", true)
            .unwrap();
    assert_eq!(
        env_of(&cmd, "AIDA_CONTAINED"),
        Some(Some("1".to_string())),
        "contained quiet burndown must set AIDA_CONTAINED=1"
    );
    assert_eq!(
        env_of(&cmd, "AIDA_SESSION_PURPOSE"),
        Some(None),
        "contained quiet burndown must strip AIDA_SESSION_PURPOSE"
    );
    assert_eq!(
        env_of(&cmd, "AIDA_BURNDOWN_LOCK_HELD"),
        Some(Some("1".to_string()))
    );
    assert_eq!(env_of(&cmd, "CEILING_KEY"), Some(Some("10".to_string())));
    let args = args_of(&cmd);
    assert!(args.contains(&"--permission-mode".to_string()));
    assert!(args.contains(&"dontAsk".to_string()));
    // trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
    crate::session::assert_headless_posture(&args, true, "contained quiet burndown");

    // Uncontained
    let cmd_uncontained = build_burndown_quiet_command(
        "prompt-quiet",
        "bypassPermissions",
        "",
        "CEILING_KEY",
        "10",
        false,
    )
    .unwrap();
    assert_eq!(
        env_of(&cmd_uncontained, "AIDA_CONTAINED"),
        Some(None),
        "uncontained quiet burndown must strip AIDA_CONTAINED"
    );
    assert_eq!(
        env_of(&cmd_uncontained, "AIDA_SESSION_PURPOSE"),
        Some(None),
        "uncontained quiet burndown must strip AIDA_SESSION_PURPOSE"
    );
    let args_uncontained = args_of(&cmd_uncontained);
    assert!(args_uncontained.contains(&"--permission-mode".to_string()));
    assert!(args_uncontained.contains(&"bypassPermissions".to_string()));
    // BUG-050: an uncontained lane is still headless: its one `--settings`
    // turns background tasks off and denies Monitor, with no sandbox.
    crate::session::assert_headless_posture(&args_uncontained, false, "uncontained quiet burndown");
}

#[test]
fn burndown_verbose_builder_argv_and_env_contained_vs_uncontained() {
    // Contained
    let cmd = build_burndown_verbose_command(
        "prompt-verbose",
        "dontAsk",
        "child-grant-1",
        "CEILING_KEY",
        "10",
        true,
    )
    .unwrap();
    assert_eq!(
        env_of(&cmd, "AIDA_CONTAINED"),
        Some(Some("1".to_string())),
        "contained verbose burndown must set AIDA_CONTAINED=1"
    );
    assert_eq!(
        env_of(&cmd, "AIDA_SESSION_PURPOSE"),
        Some(None),
        "contained verbose burndown must strip AIDA_SESSION_PURPOSE"
    );
    assert_eq!(
        env_of(&cmd, "AIDA_SESSION_ROLE"),
        Some(Some("implementer".to_string()))
    );
    assert_eq!(
        env_of(&cmd, crate::gate::seat_authority::GRANT_ENV),
        Some(Some("child-grant-1".to_string()))
    );
    let args = args_of(&cmd);
    assert!(args.contains(&"--permission-mode".to_string()));
    assert!(args.contains(&"dontAsk".to_string()));
    assert!(args.contains(&"--output-format".to_string()));
    assert!(args.contains(&"stream-json".to_string()));
    assert!(args.contains(&"--verbose".to_string()));
    assert!(args.contains(&"--include-partial-messages".to_string()));
    crate::session::assert_headless_posture(&args, true, "contained verbose burndown");
    assert_eq!(args.last().unwrap(), "prompt-verbose");

    // Uncontained
    let cmd_uncontained = build_burndown_verbose_command(
        "prompt-verbose",
        "bypassPermissions",
        "child-grant-1",
        "CEILING_KEY",
        "10",
        false,
    )
    .unwrap();
    assert_eq!(
        env_of(&cmd_uncontained, "AIDA_CONTAINED"),
        Some(None),
        "uncontained verbose burndown must strip AIDA_CONTAINED"
    );
    assert_eq!(
        env_of(&cmd_uncontained, "AIDA_SESSION_PURPOSE"),
        Some(None),
        "uncontained verbose burndown must strip AIDA_SESSION_PURPOSE"
    );
    let args_uncontained = args_of(&cmd_uncontained);
    assert!(args_uncontained.contains(&"--permission-mode".to_string()));
    assert!(args_uncontained.contains(&"bypassPermissions".to_string()));
    crate::session::assert_headless_posture(
        &args_uncontained,
        false,
        "uncontained verbose burndown",
    );
    assert_eq!(args_uncontained.last().unwrap(), "prompt-verbose");
}

#[test]
fn autopilot_intake_builder_argv_and_env_contained_vs_uncontained() {
    let cfg = crate::intake::IntakeConfig::default();

    // Contained
    let cmd = build_autopilot_intake_command(
        "prompt-intake",
        "dontAsk",
        "child-intake-grant",
        true,
        &["SPEC-1".to_string()],
        &[],
        &cfg,
        "drain",
        crate::backlog::RiskLevel::Low,
        Some(5),
        true,
    )
    .unwrap();
    assert_eq!(
        env_of(&cmd, "AIDA_CONTAINED"),
        Some(Some("1".to_string())),
        "contained autopilot intake must set AIDA_CONTAINED=1"
    );
    assert_eq!(
        env_of(&cmd, "AIDA_SESSION_PURPOSE"),
        Some(None),
        "contained autopilot intake must strip AIDA_SESSION_PURPOSE"
    );
    assert_eq!(
        env_of(&cmd, "AIDA_SESSION_ROLE"),
        Some(Some("advisor".to_string()))
    );
    assert_eq!(
        env_of(&cmd, crate::gate::seat_authority::GRANT_ENV),
        Some(Some("child-intake-grant".to_string()))
    );
    let args = args_of(&cmd);
    assert!(args.contains(&"--permission-mode".to_string()));
    assert!(args.contains(&"dontAsk".to_string()));
    crate::session::assert_headless_posture(&args, true, "contained autopilot intake");

    // Uncontained
    let cmd_uncontained = build_autopilot_intake_command(
        "prompt-intake",
        "bypassPermissions",
        "child-intake-grant",
        true,
        &["SPEC-1".to_string()],
        &[],
        &cfg,
        "drain",
        crate::backlog::RiskLevel::Low,
        Some(5),
        false,
    )
    .unwrap();
    assert_eq!(
        env_of(&cmd_uncontained, "AIDA_CONTAINED"),
        Some(None),
        "uncontained autopilot intake must strip AIDA_CONTAINED"
    );
    assert_eq!(
        env_of(&cmd_uncontained, "AIDA_SESSION_PURPOSE"),
        Some(None),
        "uncontained autopilot intake must strip AIDA_SESSION_PURPOSE"
    );
    let args_uncontained = args_of(&cmd_uncontained);
    assert!(args_uncontained.contains(&"--permission-mode".to_string()));
    assert!(args_uncontained.contains(&"bypassPermissions".to_string()));
    crate::session::assert_headless_posture(
        &args_uncontained,
        false,
        "uncontained autopilot intake",
    );
}

#[test]
fn questions_clarify_builder_argv_and_env_contained_vs_uncontained() {
    // Contained
    let cmd =
        build_questions_clarify_command("prompt-clarify", "child-clarify-grant", true).unwrap();
    assert_eq!(
        env_of(&cmd, "AIDA_CONTAINED"),
        Some(Some("1".to_string())),
        "contained questions clarify must set AIDA_CONTAINED=1"
    );
    assert_eq!(
        env_of(&cmd, "AIDA_SESSION_PURPOSE"),
        Some(None),
        "contained questions clarify must strip AIDA_SESSION_PURPOSE"
    );
    assert_eq!(
        env_of(&cmd, "AIDA_SESSION_ROLE"),
        Some(Some("advisor".to_string()))
    );
    assert_eq!(
        env_of(&cmd, crate::gate::seat_authority::GRANT_ENV),
        Some(Some("child-clarify-grant".to_string()))
    );
    let args = args_of(&cmd);
    assert!(
        args.contains(&"--settings".to_string()),
        "contained questions clarify must include --settings: {args:?}"
    );
    assert_eq!(args.last().unwrap(), "prompt-clarify");

    // Uncontained
    let cmd_uncontained =
        build_questions_clarify_command("prompt-clarify", "child-clarify-grant", false).unwrap();
    assert_eq!(
        env_of(&cmd_uncontained, "AIDA_CONTAINED"),
        Some(None),
        "uncontained questions clarify must strip AIDA_CONTAINED"
    );
    assert_eq!(
        env_of(&cmd_uncontained, "AIDA_SESSION_PURPOSE"),
        Some(None),
        "uncontained questions clarify must strip AIDA_SESSION_PURPOSE"
    );
    let args_uncontained = args_of(&cmd_uncontained);
    assert!(
        !args_uncontained.contains(&"--settings".to_string()),
        "uncontained questions clarify must not include --settings: {args_uncontained:?}"
    );
    assert_eq!(args_uncontained.last().unwrap(), "prompt-clarify");
}

#[test]
fn burndown_verbose_args_with_containment_helper() {
    let uncontained_args =
        burndown_verbose_claude_args_with_containment("/aida-burndown", "acceptEdits", false)
            .unwrap();
    crate::session::assert_headless_posture(&uncontained_args, false, "verbose args");
    assert_eq!(uncontained_args.last().unwrap(), "/aida-burndown");

    let contained_args =
        burndown_verbose_claude_args_with_containment("/aida-burndown", "dontAsk", true).unwrap();
    crate::session::assert_headless_posture(&contained_args, true, "contained verbose args");
    assert_eq!(contained_args.last().unwrap(), "/aida-burndown");
}

#[test]
fn child_grant_revocation_helper() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let id = crate::gate::seat_authority::test_support::mint_grant_for(
        root,
        "test-subject",
        "advisor",
        &["implementer"],
    );

    // Revocation via session helper revokes the grant
    crate::session::revoke_headless_child_grant(&id);

    // Second revoke should return false because it's already revoked
    let second = crate::gate::seat_authority::revoke_by_id(&id).unwrap();
    assert!(!second, "grant must already be revoked");
}
