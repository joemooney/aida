use super::*;

// trace:BUG-1486 | ai:claude
// `aida db block status` is a pure read (remaining-capacity report) and
// must be allowed under the requester stakeholder role, not refused as
// a "database write" the way the genuinely mutating db verbs are.
#[test]
fn requester_may_run_db_block_status() {
    let command = Command::Db(DbCommand::Block {
        subcommand: BlockCommand::Status,
    });
    let action = stakeholder_cli_action(&command);
    assert_eq!(action, StakeholderAction::Read);
    assert!(stakeholder_action_allowed("requester", action));
}

// trace:BUG-1486 | ai:claude
// `aida db block claim` genuinely writes (reserves a block and pushes to
// the shared store) and must stay gated for the requester role.
#[test]
fn requester_may_not_run_db_block_claim() {
    let command = Command::Db(DbCommand::Block {
        subcommand: BlockCommand::Claim {
            r#type: "FR".to_string(),
            size: 100,
        },
    });
    let action = stakeholder_cli_action(&command);
    assert_eq!(action, StakeholderAction::Database);
    assert!(!stakeholder_action_allowed("requester", action));
}
