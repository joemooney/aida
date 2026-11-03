//! `aida own`: ownership records (STORY-2325).
// trace:STORY-2325 | ai:claude

#[path = "ownership_cmd_args.rs"]
pub(crate) mod ownership_cmd_args;
pub(crate) use ownership_cmd_args::*;

/// Ownership records: one per piece of work (a spec, a pull request, a
/// review, a merge train), naming who holds it and the evidence that the
/// holder is still running.
// trace:STORY-2325 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "own")]
pub(crate) struct OwnArgs {
    #[clap(subcommand)]
    pub inner: OwnCommand,
}

// `aida own` reads and writes `.aida/ownership/` on the main worktree and
// self-loads the requirement store; no shared storage handle.
// trace:STORY-2325 | ai:claude
fn run_own(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    let args = <OwnArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let start = crate::find_project_root()?;
    let main = crate::seat_occupancy::store::main_worktree_root(&start)?;
    match &args.inner {
        OwnCommand::Show { key, compare, json } => {
            crate::ownership::compare::run(&main, key.as_deref(), *compare, *json)
        }
        OwnCommand::Migrate { execute, json, .. } => {
            crate::ownership::migrate::run(&main, *execute, *json)
        }
    }
}

// trace:STORY-2325 | ai:claude
pub(crate) static OWN: crate::command_registry::CommandReg = crate::command_registry::CommandReg {
    name: "own",
    help_after: "orchestrator",
    command: <OwnArgs as clap::CommandFactory>::command,
    policy: |_| crate::command_registry::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::Write,
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::command_registry::Handler::Early(run_own),
};
