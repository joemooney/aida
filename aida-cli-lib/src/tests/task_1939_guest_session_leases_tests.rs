use super::*;

// trace:TASK-1939 | ai:claude
// `aida session leases --prune-stale --yes` releases stale leases, including
// other clones' claims. A guest or requester seat must not run it.
#[test]
fn guest_may_not_prune_stale_session_leases() {
    let command = crate::command_registry::parse_command([
        "aida",
        "session",
        "leases",
        "--prune-stale",
        "--yes",
    ]);
    let action = stakeholder_cli_action(&command);
    assert_eq!(action, StakeholderAction::Session);
    assert!(!stakeholder_action_allowed("guest", action));
    assert!(!stakeholder_action_allowed("requester", action));
}

// trace:TASK-1939 | ai:claude
// Listing leases is still a read, with or without --yes.
#[test]
fn guest_may_list_session_leases() {
    for argv in [
        &["aida", "session", "leases"][..],
        &["aida", "session", "leases", "--all", "--json"][..],
        &["aida", "session", "leases", "--yes"][..],
    ] {
        let command = crate::command_registry::parse_command(argv.iter().copied());
        let action = stakeholder_cli_action(&command);
        assert_eq!(action, StakeholderAction::Read, "{argv:?}");
        assert!(stakeholder_action_allowed("guest", action), "{argv:?}");
    }
}

// trace:TASK-1939 | ai:claude
// `aida status --cleanup` only renders the "Needs attention" report; the
// fixer is `aida doctor heal`. It stays a read.
#[test]
fn guest_may_run_status_cleanup() {
    let command = crate::command_registry::parse_command(["aida", "status", "--cleanup"]);
    let action = stakeholder_cli_action(&command);
    assert_eq!(action, StakeholderAction::Read);
    assert!(stakeholder_action_allowed("guest", action));
}
