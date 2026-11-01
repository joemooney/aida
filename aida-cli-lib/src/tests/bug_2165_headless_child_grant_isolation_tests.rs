//! BUG-2165: Headless CI-fix and quiet-burndown children must run under
//! routed seat authority and scrub launcher session environment (PURPOSE).
//! SessionLease must preserve child_grant_id across lease updates.
// trace:BUG-2165 | ai:antigravity

use crate::burndown_run::{build_burndown_quiet_command, build_burndown_verbose_command};
use crate::gate::seat_authority::GRANT_ENV;
use crate::leases::SessionLease;
use crate::orchestrator_leases::{mark_lease_escalated_to_human, update_lease_branch};
use crate::phase_driver::build_phase_driver_advisor_command;
use crate::phase_driver_trait::build_phase_driver_ci_fix_command;
use crate::session::HeadlessVendor;
use std::path::Path;

fn get_cmd_env<'a>(cmd: &'a std::process::Command, key: &str) -> Option<Option<&'a str>> {
    cmd.get_envs()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.and_then(|s| s.to_str()))
}

#[test]
fn burndown_quiet_command_applies_implementer_grant_and_scrubs_purpose() {
    // Pin the vendor and project: these builders now resolve real launch config.
    // trace:BUG-2389 | ai:codex
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".git")).unwrap();
    let _ambient = crate::test_env::AmbientGuard::hermetic_and(
        root.path(),
        None,
        &[
            ("AIDA_HEADLESS_VENDOR", Some("claude")),
            ("AIDA_AGENT_CMD", Some("/bin/true")),
            ("AIDA_OS_WRAP", Some("0")),
            ("AIDA_CONTAINED", None),
        ],
    );
    let cmd = build_burndown_quiet_command(
        "test prompt",
        "bypassPermissions",
        "child-grant-quiet-123",
        "CEILING_KEY",
        "100",
        false,
    )
    .unwrap();

    assert_eq!(
        get_cmd_env(&cmd, "AIDA_SESSION_ROLE"),
        Some(Some("implementer")),
        "quiet burndown child must run as implementer"
    );
    assert_eq!(
        get_cmd_env(&cmd, GRANT_ENV),
        Some(Some("child-grant-quiet-123")),
        "quiet burndown child must receive child grant"
    );
    assert_eq!(
        get_cmd_env(&cmd, "AIDA_SESSION_PURPOSE"),
        Some(None),
        "quiet burndown child must have AIDA_SESSION_PURPOSE scrubbed"
    );
    assert_eq!(
        get_cmd_env(&cmd, "AIDA_BURNDOWN_LOCK_HELD"),
        Some(Some("1"))
    );
}

#[test]
fn burndown_quiet_command_supports_baseline_child() {
    // Pin the vendor and project: these builders now resolve real launch config.
    // trace:BUG-2389 | ai:codex
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".git")).unwrap();
    let _ambient = crate::test_env::AmbientGuard::hermetic_and(
        root.path(),
        None,
        &[
            ("AIDA_HEADLESS_VENDOR", Some("claude")),
            ("AIDA_AGENT_CMD", Some("/bin/true")),
            ("AIDA_OS_WRAP", Some("0")),
            ("AIDA_CONTAINED", None),
        ],
    );
    let cmd = build_burndown_quiet_command(
        "test prompt",
        "bypassPermissions",
        "",
        "CEILING_KEY",
        "100",
        false,
    )
    .unwrap();

    assert_eq!(
        get_cmd_env(&cmd, "AIDA_SESSION_ROLE"),
        Some(Some("implementer")),
        "quiet burndown child must run as implementer"
    );
    assert_eq!(
        get_cmd_env(&cmd, GRANT_ENV),
        Some(Some("")),
        "baseline child must receive empty grant"
    );
    assert_eq!(
        get_cmd_env(&cmd, "AIDA_SESSION_PURPOSE"),
        Some(None),
        "baseline child must have AIDA_SESSION_PURPOSE scrubbed"
    );
}

#[test]
fn burndown_verbose_command_applies_implementer_grant_and_scrubs_purpose() {
    // Pin the vendor and project: these builders now resolve real launch config.
    // trace:BUG-2389 | ai:codex
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".git")).unwrap();
    let _ambient = crate::test_env::AmbientGuard::hermetic_and(
        root.path(),
        None,
        &[
            ("AIDA_HEADLESS_VENDOR", Some("claude")),
            ("AIDA_AGENT_CMD", Some("/bin/true")),
            ("AIDA_OS_WRAP", Some("0")),
            ("AIDA_CONTAINED", None),
        ],
    );
    let cmd = build_burndown_verbose_command(
        "test prompt",
        "bypassPermissions",
        "child-grant-verbose-456",
        "CEILING_KEY",
        "100",
        false,
    )
    .unwrap();

    assert_eq!(
        get_cmd_env(&cmd, "AIDA_SESSION_ROLE"),
        Some(Some("implementer")),
        "verbose burndown child must run as implementer"
    );
    assert_eq!(
        get_cmd_env(&cmd, GRANT_ENV),
        Some(Some("child-grant-verbose-456")),
        "verbose burndown child must receive child grant"
    );
    assert_eq!(
        get_cmd_env(&cmd, "AIDA_SESSION_PURPOSE"),
        Some(None),
        "verbose burndown child must have AIDA_SESSION_PURPOSE scrubbed"
    );
    assert_eq!(
        get_cmd_env(&cmd, "AIDA_BURNDOWN_LOCK_HELD"),
        Some(Some("1"))
    );
}

#[test]
fn phase_driver_ci_fix_command_applies_implementer_grant_and_scrubs_purpose() {
    let tmp = tempfile::tempdir().unwrap();
    let cmd = build_phase_driver_ci_fix_command(
        tmp.path(),
        "claude",
        &["-p".into(), "fix".into()],
        "CEILING_KEY",
        "100",
        HeadlessVendor::Claude,
        false,
        "child-grant-cifix-789",
    );

    assert_eq!(
        get_cmd_env(&cmd, "AIDA_SESSION_ROLE"),
        Some(Some("implementer")),
        "CI-fix child must run as implementer"
    );
    assert_eq!(
        get_cmd_env(&cmd, GRANT_ENV),
        Some(Some("child-grant-cifix-789")),
        "CI-fix child must receive child grant"
    );
    assert_eq!(
        get_cmd_env(&cmd, "AIDA_SESSION_PURPOSE"),
        Some(None),
        "CI-fix child must have AIDA_SESSION_PURPOSE scrubbed"
    );
    assert_eq!(get_cmd_env(&cmd, "AIDA_HEADLESS"), Some(Some("1")));
    // trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
    assert_eq!(
        get_cmd_env(&cmd, crate::session::DISABLE_BACKGROUND_TASKS_ENV),
        Some(Some("1")),
        "CI-fix child must run with background tasks disabled"
    );
}

#[test]
fn phase_driver_advisor_command_scrubs_purpose() {
    let tmp = tempfile::tempdir().unwrap();
    let req = tmp.path().join("req.json");
    let resp = tmp.path().join("resp.json");
    let cmd = build_phase_driver_advisor_command(
        tmp.path(),
        "claude",
        &["-p".into(), "advisor".into()],
        &req,
        &resp,
        "CEILING_KEY",
        "100",
        HeadlessVendor::Claude,
        false,
    );

    assert_eq!(
        get_cmd_env(&cmd, "AIDA_SESSION_ROLE"),
        Some(Some("advisor")),
        "advisor consult child must run as advisor"
    );
    assert_eq!(
        get_cmd_env(&cmd, "AIDA_SESSION_PURPOSE"),
        Some(None),
        "advisor consult child must have AIDA_SESSION_PURPOSE scrubbed"
    );
    assert_eq!(get_cmd_env(&cmd, "AIDA_HEADLESS"), Some(Some("1")));
    // trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
    assert_eq!(
        get_cmd_env(&cmd, crate::session::DISABLE_BACKGROUND_TASKS_ENV),
        Some(Some("1")),
        "advisor consult child must run with background tasks disabled"
    );
    assert_eq!(get_cmd_env(&cmd, "CEILING_KEY"), Some(Some("100")));
}

#[test]
fn session_lease_preserves_child_grant_id_across_updates() {
    let tmp = tempfile::tempdir().unwrap();
    let sessions_dir = tmp.path().join(".aida").join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();

    let lease_id = "test-lease-child-grant";
    let lease_file = sessions_dir.join(format!("{lease_id}.toml"));

    let lease = SessionLease {
        id: lease_id.to_string(),
        scope: "BUG-2165".to_string(),
        slug: "bug-2165".to_string(),
        owner: "tester".into(),
        worktree_path: tmp.path().to_path_buf(),
        branch: "feat/initial-branch".to_string(),
        started_at: chrono::Utc::now(),
        hostname: "h".into(),
        role: Some("implementer".into()),
        creator_pid: Some(4242),
        creator_pid_start_time: None,
        active_pid: None,
        active_pid_start_time: None,
        cargo_target_dir: None,
        parent_project_root: None,
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: false,
        claim_verb: false,
        manual_enter_at: None,
        interrupted_at: None,
        interrupted_reason: None,
        ending_retained_at: None,
        ending_retained_reason: None,
        driver: None,
        tree_kind: Default::default(),
        child_grant_id: Some("01948abc-def0-7000-8000-000000000001".into()),
    };
    std::fs::write(&lease_file, toml::to_string_pretty(&lease).unwrap()).unwrap();

    // 1. update_lease_branch
    update_lease_branch(tmp.path(), lease_id, "feat/new-branch").unwrap();

    let body_after_branch = std::fs::read_to_string(&lease_file).unwrap();
    let lease_after_branch: SessionLease = toml::from_str(&body_after_branch).unwrap();
    assert_eq!(lease_after_branch.branch, "feat/new-branch");
    assert_eq!(
        lease_after_branch.child_grant_id.as_deref(),
        Some("01948abc-def0-7000-8000-000000000001"),
        "update_lease_branch must not drop child_grant_id"
    );

    // 2. mark_lease_escalated_to_human
    mark_lease_escalated_to_human(tmp.path(), lease_id).unwrap();

    let body_after_esc = std::fs::read_to_string(&lease_file).unwrap();
    let lease_after_esc: SessionLease = toml::from_str(&body_after_esc).unwrap();
    assert!(lease_after_esc.escalated_to_human.is_some());
    assert_eq!(
        lease_after_esc.child_grant_id.as_deref(),
        Some("01948abc-def0-7000-8000-000000000001"),
        "mark_lease_escalated_to_human must not drop child_grant_id"
    );
}
