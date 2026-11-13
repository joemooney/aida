//! `aida server` command cluster — the CLI-side launcher/client for the
//! aida-server REST/gRPC service (port 8080). Extracted verbatim from
//! `main.rs` (SPIKE-78); no behavior change. The actual server lives in the
//! `aida-server` crate; this handler just dispatches remote-client calls
//! (reached via `crate::client`).
// trace:TASK-3428 | ai:codex

use crate::cli::ServerCommand;

// trace:STORY-1728 | ai:claude
#[path = "server_cmd_args.rs"]
pub(crate) mod server_cmd_args;
#[cfg(feature = "remote")]
use crate::client;
use anyhow::Result;
pub(crate) use server_cmd_args::*;

pub(crate) fn handle_server_command(cmd: &ServerCommand, server_addr: Option<&str>) -> Result<()> {
    let server_addr = server_addr.ok_or_else(|| {
        anyhow::anyhow!(
            "Server address required. Use --server flag or set AIDA_SERVER environment variable."
        )
    })?;

    #[cfg(feature = "remote")]
    {
        // Create a tokio runtime for async operations
        let rt = tokio::runtime::Runtime::new()?;

        match cmd {
            ServerCommand::Status => {
                rt.block_on(client::get_server_status(server_addr))?;
            }
            ServerCommand::List {
                status,
                feature,
                limit,
            } => {
                rt.block_on(client::list_requirements(
                    server_addr,
                    status.as_deref(),
                    feature.as_deref(),
                    *limit,
                ))?;
            }
            ServerCommand::Get { id } => {
                rt.block_on(client::get_requirement(server_addr, id))?;
            }
            ServerCommand::Ping => {
                rt.block_on(client::ping_server(server_addr))?;
            }
        }
        Ok(())
    }

    #[cfg(not(feature = "remote"))]
    {
        let _ = cmd; // suppress unused warning
        let _ = server_addr;
        anyhow::bail!(
            "Remote server support is not enabled. \
            Build with: cargo build -p aida-cli --features remote"
        )
    }
}

/// Server management commands (requires --server or AIDA_SERVER)
#[derive(clap::Parser, Debug)]
#[clap(name = "server")]
#[clap(hide = true)]
pub(crate) struct ServerArgs {
    #[clap(subcommand)]
    pub inner: crate::cli::ServerCommand,
}

fn run_server(
    matches: &clap::ArgMatches,
    ctx: &crate::cmdkit::DispatchCtx,
) -> anyhow::Result<crate::cmdkit::Flow> {
    let args = <ServerArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match &ctx.store {
        None => Ok(crate::cmdkit::Flow::Continue),
        Some(crate::cmdkit::StoreCtx::Distributed { .. }) => Ok(crate::cmdkit::Flow::Continue),
        Some(crate::cmdkit::StoreCtx::Git {
            store_path,
            backend,
        }) => {
            use crate::git_backend_cmd::*;
            let (store_path, backend) = (*store_path, *backend);
            let _ = (store_path, backend, &args);
            crate::git_backend_cmd::exit_unsupported_on_git_backend()
        }
        Some(crate::cmdkit::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            match &args {
                ServerArgs { inner: server_cmd } => {
                    self::handle_server_command(
                        server_cmd,
                        ctx.globals.and_then(|globals| globals.server.as_deref()),
                    )?;
                }
            }
            Ok(crate::cmdkit::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static SERVER: crate::cmdkit::CommandReg = crate::cmdkit::CommandReg {
    name: "server",
    help_after: "user-guide",
    command: <ServerArgs as clap::CommandFactory>::command,
    policy: |_| crate::cmdkit::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::Write,
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::cmdkit::Handler::Store(run_server),
};
