//! `aida remote` — manage the store's git remotes and forge repos.
// trace:STORY-1728 | ai:claude

use anyhow::Result;

// trace:STORY-1728 | ai:claude
#[path = "remote_cmd_args.rs"]
pub(crate) mod remote_cmd_args;
pub(crate) use remote_cmd_args::*;

/// Set up a git `origin` for a project that has none — guided origin
/// bootstrap. `create` offers GitHub (via gh), personal-GitLab
/// push-to-create over SSH (no glab/token needed), or attach-existing;
/// `attach <url>` wires an existing repo. Non-interactive prints the
/// manual recipe and exits cleanly.
#[derive(clap::Parser, Debug)]
#[clap(name = "remote")]
pub(crate) struct RemoteArgs {
    #[clap(subcommand)]
    pub inner: crate::cli::RemoteCommand,
}

// `aida remote create`/`attach` bootstrap a git origin — they operate on a
// project that, by definition, may not have a working store/remote yet, so
// dispatch before storage init. trace:STORY-537 | ai:claude
fn run_remote(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    use crate::*;
    let args = <RemoteArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let remote_cmd = &args.inner;
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    return match remote_cmd {
        crate::cli::RemoteCommand::Create {
            attach,
            github,
            gitlab,
            public,
        } => remote_create::handle_remote_create(
            &project_root,
            attach.as_deref(),
            *github,
            gitlab.as_deref(),
            !*public,
        ),
        crate::cli::RemoteCommand::Attach { url } => {
            remote_create::handle_remote_attach(&project_root, url)
        }
        crate::cli::RemoteCommand::Status { json, no_fetch } => {
            remote_create::handle_remote_status(&project_root, *json, *no_fetch)
        }
        // trace:TASK-1097 | ai:claude
        crate::cli::RemoteCommand::Mirror { name, url } => {
            remote_create::handle_remote_mirror(&project_root, name, url.as_deref())
        }
        crate::cli::RemoteCommand::MirrorPush {
            pushed_remote,
            dry_run,
        } => remote_create::handle_remote_mirror_push(&project_root, pushed_remote, *dry_run),
        // trace:BUG-2015 | ai:claude
        crate::cli::RemoteCommand::MirrorGuardCheck {
            remote,
            install_guard,
            force,
        } => match remote.as_deref().filter(|_| !*install_guard) {
            Some(remote) => mirror_guard::handle_mirror_guard_check(&project_root, remote),
            None => mirror_guard::install_main_guard(&project_root, *force),
        },
        // trace:BUG-1676 | ai:claude
        crate::cli::RemoteCommand::MirrorSync { json } => {
            remote_create::handle_remote_mirror_sync(&project_root, *json)
        }
        crate::cli::RemoteCommand::Reconcile { execute, json, yes } => {
            remote_create::handle_remote_reconcile(&project_root, *execute, *json, *yes)
        }
    };
}

// trace:STORY-1728 | ai:claude
pub(crate) static REMOTE: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "remote",
        help_after: "digest",
        command: <RemoteArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Early(run_remote),
    };
