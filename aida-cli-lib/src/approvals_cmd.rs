//! `aida approvals` — record and list human approval decisions.
// trace:STORY-1728 | ai:claude

use anyhow::Result;

// trace:STORY-1728 | ai:claude
#[path = "approvals_cmd_args.rs"]
pub(crate) mod approvals_cmd_args;
pub(crate) use approvals_cmd_args::*;

/// List or write proxy-approval ledger comments.
///
/// A proxy approval is stored as an ordinary spec comment carrying the
/// stable `[aida:proxy-approval]` marker plus structured fields, so the
/// spec comment remains the git-canonical audit source.
// trace:STORY-1173 | ai:codex
#[derive(clap::Parser, Debug)]
#[clap(name = "approvals")]
pub(crate) struct ApprovalsArgs {
    /// Only entries at or after this time. Accepts a relative window
    /// (`45m`, `12h`, `7d`, `2w`, or `24 hours ago`), an ISO date
    /// (`2026-05-01`, local midnight), a zone-less ISO datetime
    /// (`2026-05-01T10:00`, local time), or RFC3339.
    #[clap(long)]
    pub since: Option<String>,

    /// Only entries before this time. Same forms as `--since`.
    #[clap(long)]
    pub until: Option<String>,

    /// Emit ledger entries as JSON.
    #[clap(long)]
    pub json: bool,

    /// Optional writer command. With no subcommand, lists approvals.
    #[clap(subcommand)]
    pub command: Option<crate::cli::ApprovalCommand>,
}

fn run_approvals(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <ApprovalsArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match &ctx.store {
        None => Ok(crate::command_registry::Flow::Continue),
        Some(crate::command_registry::StoreCtx::Distributed { .. }) => {
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Git {
            store_path,
            backend,
        }) => {
            use crate::git_backend_cmd::*;
            let (store_path, backend) = (*store_path, *backend);
            match &args {
                ApprovalsArgs {
                    since,
                    until,
                    json,
                    command,
                } => {
                    // trace:STORY-1173 | ai:codex
                    match command {
                        Some(ApprovalCommand::Add {
                            spec,
                            decision,
                            verdict,
                            proxy,
                            session,
                            authorized_by,
                            independent,
                        }) => {
                            return crate::command_registry::returned(add_proxy_approval(
                                backend,
                                spec,
                                decision,
                                verdict,
                                proxy.as_deref(),
                                session.as_deref(),
                                authorized_by,
                                *independent,
                            ));
                        }
                        None => {
                            return crate::command_registry::returned(list_proxy_approvals(
                                backend,
                                since.as_deref(),
                                until.as_deref(),
                                *json || output_format_is_json(),
                            ));
                        }
                    }
                }
                #[allow(unreachable_patterns)]
                _ => crate::git_backend_cmd::exit_unsupported_on_git_backend(),
            }
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            match &args {
                ApprovalsArgs { .. } => {
                    // trace:STORY-1173 | ai:codex
                    anyhow::bail!(
                        "aida approvals requires the distributed git-canonical store \
                     (run `aida init` to migrate, or this project is on the \
                     deprecated --centralized backend)"
                    );
                }
                #[allow(unreachable_patterns)]
                _ => unreachable!("`aida approvals` finished before storage init"),
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static APPROVALS: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "approvals",
        help_after: "show",
        command: <ApprovalsArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Store(run_approvals),
    };
