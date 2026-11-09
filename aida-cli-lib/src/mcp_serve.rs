// trace:TASK-3429 | ai:codex
/// Start MCP (Model Context Protocol) server over stdio
///
/// Exposes AIDA requirements as MCP tools for Claude Code integration.
/// Reads JSON-RPC 2.0 requests from stdin, writes responses to stdout.
#[derive(clap::Parser, Debug)]
#[clap(name = "mcp-serve")]
#[clap(hide = true)]
pub struct McpServeArgs {}

fn run_mcp_serve(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    crate::command_registry::run_mcp_handler(matches, ctx)
}

// trace:STORY-1728 | ai:claude
pub(crate) static MCP_SERVE: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "mcp-serve",
        help_after: "jira",
        command: <McpServeArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Read,
            per_write_auto_push: false,
            skip_role_restore_prompt: true,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Store(run_mcp_serve),
    };
