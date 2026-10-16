//! BUG-2166: Contained parent escape via `aida queue work --plan-only --permission-mode bypassPermissions`.
//!
//! Monotonic containment ruling (BUG-2151): a contained parent's env
//! (`AIDA_CONTAINED`, parsed strictly) ALWAYS wins and no flag overrides it.
//! `--plan-only` must not disable containment, and `--permission-mode` must be
//! refused under a contained parent.
// trace:BUG-2166 | ai:antigravity
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
    for mode in ["bypassPermissions", "acceptEdits", "dontAsk", "auto"] {
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
fn contained_parent_plan_only_preserves_containment_and_dont_ask_argv_and_env() {
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
        Some("dontAsk"),
        "contained session uses dontAsk posture"
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
        Some("dontAsk"),
        "permission-mode must be dontAsk: {args:?}"
    );
    assert!(
        args.iter().any(|a| a == "--settings"),
        "child argv must include contained settings flags: {args:?}"
    );
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
    assert_eq!(sandbox_posture.permission_mode.as_deref(), Some("dontAsk"));

    // Contained knob with plan_only
    let knob_posture = resolve_queue_work_posture(
        None, /* sandbox */ false, /* contained_env */ false,
        /* contained_knob */ true, /* bypass_knob */ false, /* plan_only */ true,
        None, None,
    )
    .expect("contained knob posture resolution succeeds");
    assert!(knob_posture.contained);
    assert_eq!(knob_posture.permission_mode.as_deref(), Some("dontAsk"));
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
