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
    #[clap(value_name = "ID")]
    pub spec: String,
}

// `aida derisk` is a thin advisor-session launcher; it uses the shared
// agent-new machinery and does not need the shared storage handle.
// trace:TASK-1235 | ai:codex
fn run_derisk(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    use crate::*;
    let args = <DeriskArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let DeriskArgs { spec } = &args;
    return handle_derisk_command(spec);
}

// trace:STORY-1728 | ai:claude
pub(crate) static DERISK: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "derisk",
        help_after: "do",
        command: <DeriskArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Early(run_derisk),
    };
