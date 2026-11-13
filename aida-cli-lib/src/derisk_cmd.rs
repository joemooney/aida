//! `aida derisk` — de-risk a supervised spec down the autonomy ladder.
// trace:STORY-1728 | ai:claude

use anyhow::Result;

/// Launch an advisor session to de-risk a supervised requirement. This is
/// a thin vendor-aware front door: the advisor surfaces forks, records
/// decisions, and only changes routing with explicit operator confirmation.
// trace:TASK-1235 | ai:codex
#[derive(clap::Parser, Debug)]
#[clap(name = "derisk")]
pub(crate) struct DeriskArgs {
    /// Requirement identifier to de-risk.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-015 | ai:claude
    #[clap(value_name = "ID", value_parser = crate::spec_arg::spec_id)]
    pub spec: String,
}

// `aida derisk` is a thin advisor-session launcher; it uses the shared
// agent-new machinery and does not need the shared storage handle.
// trace:TASK-1235 | ai:codex
fn run_derisk(matches: &clap::ArgMatches, _ctx: &crate::cmdkit::DispatchCtx) -> anyhow::Result<()> {
    // trace:TASK-3428 | ai:codex
    use crate::handle_derisk_command;
    let args = <DeriskArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let DeriskArgs { spec } = &args;
    return handle_derisk_command(spec);
}

// trace:STORY-1728 | ai:claude
pub(crate) static DERISK: crate::cmdkit::CommandReg = crate::cmdkit::CommandReg {
    name: "derisk",
    help_after: "do",
    command: <DeriskArgs as clap::CommandFactory>::command,
    policy: |_| crate::cmdkit::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::Write,
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::cmdkit::Handler::Early(run_derisk),
};
