//! BUG-2166: Contained parent escape via `aida queue work --plan-only --permission-mode bypassPermissions`.
//!
//! Monotonic containment ruling (BUG-2151): a contained parent's env
//! (`AIDA_CONTAINED`, parsed strictly) ALWAYS wins and no flag overrides it.
//! `--plan-only` must not disable containment, and `--permission-mode` must be
//! refused under a contained parent.
// trace:BUG-2166 | ai:antigravity
// trace:TASK-1794 | ai:codex
use crate::queue_cmd::{
    build_queue_work_claude_command, honour_auto_complete_sandbox, resolve_queue_work_posture,
};
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
fn contained_parent_plan_only_refuses_permission_mode_posture() {
    let err = resolve_queue_work_posture(
        Some("bypassPermissions"),
        /* sandbox */ false,
        /* contained_env */ true,
        /* contained_knob */ false,
        /* bypass_knob */ false,
        /* plan_only */ true,
        None,
        None,
    )
    .expect_err("contained parent must refuse --permission-mode with --plan-only");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("[contained] this session is contained (AIDA_CONTAINED)"),
        "{msg}"
    );
    assert!(
        msg.contains("refusing `--permission-mode bypassPermissions`"),
        "{msg}"
    );
}

#[test]
fn contained_parent_refuses_any_permission_mode_even_without_plan_only() {
    for mode in [
        "bypassPermissions",
        "acceptEdits",
        "dontAsk",
        "auto",
        "plan",
    ] {
        let err = resolve_queue_work_posture(
            Some(mode),
            /* sandbox */ false,
            /* contained_env */ true,
            /* contained_knob */ false,
            /* bypass_knob */ false,
            /* plan_only */ false,
            None,
            None,
        )
        .expect_err("contained parent must refuse any --permission-mode");
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
fn contained_parent_plan_only_preserves_containment_and_plan_argv_and_env() {
    let posture = resolve_queue_work_posture(
        None, /* sandbox */ false, /* contained_env */ true,
        /* contained_knob */ false, /* bypass_knob */ false, /* plan_only */ true,
        None, None,
    )
    .expect("contained parent without --permission-mode succeeds");

    assert!(
        posture.contained,
        "plan_only under contained_env must be contained"
    );
    assert_eq!(
        posture.permission_mode.as_deref(),
        Some("plan"),
        "contained plan-only session uses plan posture"
    );

    let session_id = "01a14325-157c-7dd1-97b6-82f8cf2c7a5a";
    let cmd = build_queue_work_claude_command(
        &posture,
        Some("test-plan-session"),
        "/aida-plan",
        session_id,
        None,
        None,
    );

    assert_eq!(
        env_of(&cmd, "AIDA_CONTAINED"),
        Some(Some("1".to_string())),
        "child command must keep AIDA_CONTAINED=1 in env"
    );

    let args = args_of(&cmd);
    assert!(
        !args.iter().any(|a| a == "bypassPermissions"),
        "child argv must not have bypassPermissions: {args:?}"
    );
    assert!(
        args.iter().any(|a| a == "--permission-mode"),
        "child argv must specify permission-mode: {args:?}"
    );
    let perm_idx = args.iter().position(|a| a == "--permission-mode").unwrap();
    assert_eq!(
        args.get(perm_idx + 1).map(String::as_str),
        Some("plan"),
        "permission-mode must be plan: {args:?}"
    );
    assert!(
        args.iter().any(|a| a == "--settings"),
        "child argv must include contained settings flags: {args:?}"
    );
    // Preserve the actual containment settings alongside plan, rather than
    // merely passing a --settings flag with an empty or relaxed body.
    // trace:TASK-1794 | ai:codex
    let settings_index = args.iter().position(|a| a == "--settings").unwrap();
    let settings: serde_json::Value = serde_json::from_str(&args[settings_index + 1]).unwrap();
    assert_eq!(settings["sandbox"]["enabled"], true);
    assert_eq!(settings["sandbox"]["failIfUnavailable"], true);
    assert_eq!(settings["sandbox"]["allowUnsandboxedCommands"], false);
    assert!(settings["permissions"]["deny"].as_array().unwrap().len() > 1);
    let sources_index = args.iter().position(|a| a == "--setting-sources").unwrap();
    assert_eq!(args[sources_index + 1], "project");
    assert!(
        args.iter().any(|a| a == "/aida-plan"),
        "child argv must include the plan prompt: {args:?}"
    );
}

#[test]
fn sandbox_and_contained_knob_with_plan_only_preserve_containment() {
    // Sandbox with plan_only
    let sandbox_posture = resolve_queue_work_posture(
        None, /* sandbox */ true, /* contained_env */ false,
        /* contained_knob */ false, /* bypass_knob */ false, /* plan_only */ true,
        None, None,
    )
    .expect("sandbox posture resolution succeeds");
    assert!(sandbox_posture.contained);
    assert_eq!(sandbox_posture.permission_mode.as_deref(), Some("plan"));

    // Contained knob with plan_only
    let knob_posture = resolve_queue_work_posture(
        None, /* sandbox */ false, /* contained_env */ false,
        /* contained_knob */ true, /* bypass_knob */ false, /* plan_only */ true,
        None, None,
    )
    .expect("contained knob posture resolution succeeds");
    assert!(knob_posture.contained);
    assert_eq!(knob_posture.permission_mode.as_deref(), Some("plan"));
}

#[test]
fn uncontained_plan_only_defaults_to_plan_permission_mode() {
    let posture = resolve_queue_work_posture(
        None, /* sandbox */ false, /* contained_env */ false,
        /* contained_knob */ false, /* bypass_knob */ false, /* plan_only */ true,
        None, None,
    )
    .expect("uncontained plan_only succeeds");

    assert!(!posture.contained);
    assert_eq!(posture.permission_mode.as_deref(), Some("plan"));

    let cmd = build_queue_work_claude_command(
        &posture,
        None,
        "/aida-plan",
        "01a14325-157c-7dd1-97b6-82f8cf2c7a5a",
        None,
        None,
    );
    let args = args_of(&cmd);
    let perm_idx = args.iter().position(|a| a == "--permission-mode").unwrap();
    assert_eq!(
        args.get(perm_idx + 1).map(String::as_str),
        Some("plan"),
        "uncontained plan_only defaults to plan permission-mode: {args:?}"
    );
    assert!(
        !args.iter().any(|a| a == "--settings"),
        "uncontained launch must not include contained settings: {args:?}"
    );
    assert_ne!(
        env_of(&cmd, "AIDA_CONTAINED"),
        Some(Some("1".to_string())),
        "uncontained launch must not set AIDA_CONTAINED=1"
    );
}

#[test]
fn auto_complete_sandbox_under_contained_parent_refuses_permission_mode() {
    let _env = EnvVarsGuard::apply(&[("AIDA_CONTAINED", Some("1"))]);

    for sandbox in [false, true] {
        let err = honour_auto_complete_sandbox(sandbox, Some("bypassPermissions"))
            .expect_err("contained parent must refuse --permission-mode in auto-complete");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("[contained] this session is contained (AIDA_CONTAINED)"),
            "{sandbox}: {msg}"
        );
        assert!(
            msg.contains("refusing `--permission-mode bypassPermissions`"),
            "{sandbox}: {msg}"
        );
    }

    // Without --permission-mode, auto_complete_sandbox succeeds under contained parent
    honour_auto_complete_sandbox(false, None).unwrap();
    honour_auto_complete_sandbox(true, None).unwrap();
}

// trace:TASK-1794 | ai:codex
#[test]
fn contained_non_plan_pickup_keeps_dont_ask_posture() {
    for (sandbox, parent, config) in [
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let posture =
            resolve_queue_work_posture(None, sandbox, parent, config, false, false, None, None)
                .unwrap();
        assert!(posture.contained);
        assert_eq!(posture.permission_mode.as_deref(), Some("dontAsk"));
    }
}

// trace:TASK-1794 | ai:codex
#[test]
fn contained_plan_only_ignores_inherited_permission_overrides_and_refuses_bypass() {
    for (sandbox, parent, config) in [
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let posture = resolve_queue_work_posture(
            None,
            sandbox,
            parent,
            config,
            false,
            true,
            Some("bypassPermissions"),
            Some("acceptEdits"),
        )
        .unwrap();
        assert!(posture.contained);
        assert_eq!(posture.permission_mode.as_deref(), Some("plan"));
        assert!(
            resolve_queue_work_posture(None, sandbox, parent, config, true, true, None, None)
                .is_err()
        );
    }
}

// trace:TASK-1794 | ai:codex
#[test]
fn plan_only_refuses_headless_launch_before_setup() {
    use clap::Parser;
    for headless in [
        "--no-human",
        "--headless",
        "--unattended",
        "--drain",
        "--auto-complete",
    ] {
        for sandbox in [false, true] {
            let mut args = vec!["aida", "queue", "work", "TASK-1", "--plan-only", headless];
            if sandbox {
                args.push("--sandbox");
            }
            let error = crate::command_registry::try_parse_from(args)
                .expect_err("headless/autonomous launch must not discard plan mode");
            assert_eq!(
                error.kind(),
                clap::error::ErrorKind::ArgumentConflict,
                "{headless}, sandbox={sandbox}: {error}"
            );
        }
    }
    assert!(crate::command_registry::try_parse_from([
        "aida",
        "queue",
        "work",
        "TASK-1",
        "--plan-only",
        "--sandbox",
    ])
    .is_ok());
    assert!(
        crate::command_registry::try_parse_from([
            "aida",
            "queue",
            "work",
            "TASK-1",
            "--sandbox",
            "--no-human",
        ])
        .is_ok(),
        "ordinary headless containment remains supported"
    );
}

// The `--with-plan` drain prelude's planning argv must parse for both an
// interactive and a headless implementer: the interactive-only `--plan-only`
// conflict must not break the headless prelude.
// trace:TASK-1794 | ai:claude
#[test]
fn plan_prelude_session_argv_parses_for_headless_and_interactive() {
    use clap::Parser;
    for headless in [false, true] {
        for step in crate::auto_complete::plan_prelude_steps("TASK-1", true, headless) {
            let crate::auto_complete::PlanPreludeStep::PlanSession { spec, headless } = step else {
                continue;
            };
            let args = crate::drain_gate::plan_session_args(&spec, headless);
            let argv = std::iter::once("aida".to_string()).chain(args.iter().cloned());
            let cli = crate::command_registry::try_parse_from(argv)
                .unwrap_or_else(|e| panic!("headless={headless}: {args:?} rejected: {e}"));
            let crate::queue_cmd::QueueArgs {
                inner:
                    crate::cli::QueueCommand::Work {
                        plan_only,
                        plan_phase,
                        no_human,
                        ..
                    },
            } = cli.command.registered::<crate::queue_cmd::QueueArgs>()
            else {
                panic!("headless={headless}: {args:?} did not parse as queue work");
            };
            assert_eq!(no_human.is_some(), headless, "{args:?}");
            assert_eq!(plan_phase, headless, "{args:?}");
            assert_eq!(plan_only, !headless, "{args:?}");
        }
    }
}

// The hidden headless plan lane cannot be used interactively, nor combined
// with the autonomous drain or the user-facing plan/guided lanes.
// trace:TASK-1794 | ai:claude
#[test]
fn plan_phase_requires_headless_and_conflicts_with_other_lanes() {
    use clap::Parser;
    let error = crate::command_registry::try_parse_from([
        "aida",
        "queue",
        "work",
        "TASK-1",
        "--plan-phase",
    ])
    .expect_err("--plan-phase without --no-human must refuse");
    assert_eq!(
        error.kind(),
        clap::error::ErrorKind::MissingRequiredArgument
    );
    for other in ["--plan-only", "--guided", "--auto-complete", "--drain"] {
        let error = crate::command_registry::try_parse_from([
            "aida",
            "queue",
            "work",
            "TASK-1",
            "--plan-phase",
            "--no-human",
            other,
        ])
        .expect_err("--plan-phase must conflict");
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::ArgumentConflict,
            "{other}: {error}"
        );
    }
}

// Operator-facing permission origin names the plan lane, not just containment.
// trace:TASK-1794 | ai:claude
#[test]
fn contained_permission_origin_names_plan_only() {
    let plan =
        resolve_queue_work_posture(None, true, false, false, false, true, None, None).unwrap();
    assert_eq!(
        plan.permission_mode_origin,
        "contained sandbox + --plan-only"
    );
    let pickup =
        resolve_queue_work_posture(None, true, false, false, false, false, None, None).unwrap();
    assert_eq!(pickup.permission_mode_origin, "contained sandbox");
}
