use super::human_subcommand_needs_no_storage;
use crate::cli::HumanCommand;

#[test]
fn presence_unblock_verbs_need_no_storage() {
    assert!(human_subcommand_needs_no_storage(&HumanCommand::Away));
    assert!(human_subcommand_needs_no_storage(&HumanCommand::Home));
    assert!(human_subcommand_needs_no_storage(&HumanCommand::Presence));
    assert!(human_subcommand_needs_no_storage(&HumanCommand::Status));
    assert!(human_subcommand_needs_no_storage(&HumanCommand::Unblock {
        copy: false,
        stdout: false,
        json: false,
        interactive: false,
        then_drain: false,
    }));
}

#[test]
fn action_aliases_need_storage_so_fall_through_to_main_dispatch() {
    // The `answer`/`decide`/`review` aliases delegate to backend-needing
    // canonical verbs — they must NOT be handled in the pre-storage early
    // return. trace:STORY-611
    assert!(!human_subcommand_needs_no_storage(&HumanCommand::Answer {
        spec: "STORY-1".into(),
        choice: "1".into(),
        note: None,
    }));
    assert!(!human_subcommand_needs_no_storage(&HumanCommand::Decide {
        spec: "STORY-1".into(),
        choice: "1".into(),
        note: None,
    }));
    assert!(!human_subcommand_needs_no_storage(&HumanCommand::Review {
        spec: "STORY-1".into(),
        no_agent: false,
        allow_stale_base: false,
    }));
}
