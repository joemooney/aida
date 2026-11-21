//! `aida review draft`: inspect the launch receipts of reviewer draft
//! handoffs ([`crate::review_handoff`]). Registered from this module under
//! `aida review` through `command_registry::NESTED`.
// trace:BUG-2669 | ai:claude

use anyhow::Result;

#[path = "review_draft_cmd_args.rs"]
pub(crate) mod review_draft_cmd_args;
use review_draft_cmd_args::ReviewDraftCommand;

/// Inspect the reviewer draft handoffs launchers have registered.
#[derive(clap::Parser, Debug)]
#[clap(name = "draft")]
pub(crate) struct ReviewDraftArgs {
    #[clap(subcommand)]
    pub command: ReviewDraftCommand,
}

fn run_draft(matches: &clap::ArgMatches, _ctx: &crate::cmdkit::DispatchCtx) -> Result<()> {
    let args = <ReviewDraftArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match args.command {
        ReviewDraftCommand::List { all, json } => crate::review_handoff::handle_list(all, json),
    }
}

/// `help_after` names the sibling `aida review` subcommand this one follows.
pub(crate) static DRAFT: crate::cmdkit::CommandReg = crate::cmdkit::CommandReg {
    name: "draft",
    help_after: "blocking-tiers",
    command: <ReviewDraftArgs as clap::CommandFactory>::command,
    // Listing reads; anything else under this command is a write until it
    // says otherwise.
    policy: |matches| crate::cmdkit::CmdPolicy {
        stakeholder: match matches.subcommand_name() {
            Some("list") => crate::stakeholder_role::StakeholderAction::Read,
            _ => crate::stakeholder_role::StakeholderAction::Write,
        },
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::cmdkit::Handler::Early(run_draft),
};
