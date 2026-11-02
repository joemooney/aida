//! `aida mcp` command cluster — `handle_mcp_command`, agent registration
//! and `mcp translate`, extracted from `lib.rs` (SPIKE-78 / STORY-771;
//! pure movement, no behavior change). The MCP server itself stays in
//! `mcp.rs`.
// trace:STORY-771 | ai:claude

use crate::*;

// trace:STORY-1728 | ai:claude
#[path = "mcp_cmd_args.rs"]
pub(crate) mod mcp_cmd_args;
pub(crate) use mcp_cmd_args::*;

/// STORY-361: management commands for the MCP coordination surface.
pub(crate) fn handle_mcp_command(cmd: &McpCommand) -> Result<()> {
    match cmd {
        McpCommand::RegisterAgent { name, print, force } => {
            register_mcp_agent(name, *print, *force)
        }
        McpCommand::Translate {
            project_root,
            force,
            dry_run,
        } => {
            let root = project_root
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            run_mcp_translate(&root, *force, *dry_run)
        }
        McpCommand::Skill(SkillCommand::Render { name }) => {
            let project_root = find_project_root()
                .or_else(|_| std::env::current_dir())
                .map_err(|e| {
                    anyhow::anyhow!(
                        "Error: could not find project root or current directory: {}",
                        e
                    )
                })?;
            // trace:BUG-2388 | ai:codex
            print!("{}", crate::skill_cmd::render_skill(&project_root, name)?);
            Ok(())
        }
        // `skill lint` is a read-only filesystem check; route it through the
        // same engine the CLI uses. trace:TASK-927 | ai:claude
        McpCommand::Skill(SkillCommand::Lint { skill, json, quiet }) => {
            lint_skills(skill.as_deref(), *json, *quiet)
        }
    }
}

/// STORY-361: write (or print) a `.mcp.json` entry that points an MCP-speaking
/// agent at this project's AIDA server.
///
/// The entry runs `aida mcp-serve` over stdio (Anthropic MCP spec — every
/// MCP-speaking agent supports stdio transport). The printed `serverUrl` is
/// `stdio://aida` for now; cross-machine transport (HTTP/SSE) is deferred to a
/// follow-up SPIKE per STORY-361's acceptance.
pub(crate) fn register_mcp_agent(name: &str, print_only: bool, force: bool) -> Result<()> {
    let aida_exe = resolve_aida_exe();
    let entry = serde_json::json!({
        "command": aida_exe.to_string_lossy(),
        "args": ["mcp-serve"],
        "description": "AIDA — spec graph + coordination surface (STORY-361)"
    });
    let server_url = "stdio://aida";

    if print_only {
        println!("# AIDA MCP server registration");
        println!();
        println!("Name:       {}", name);
        println!("Server URL: {}", server_url);
        println!();
        println!("`.mcp.json` entry (add under `\"mcpServers\"`):");
        println!();
        let pretty = serde_json::json!({ name: entry });
        println!("{}", serde_json::to_string_pretty(&pretty)?);
        println!();
        println!("Tools exposed:");
        let descriptors = mcp::tool_descriptors();
        if let Some(arr) = descriptors.as_array() {
            for tool in arr {
                let n = tool.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                let d = tool
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                println!("  - {:<22} {}", n, d);
            }
        }
        return Ok(());
    }

    // Write/update .mcp.json in the project root.
    let project_root = find_project_root()
        .with_context(|| "not inside an AIDA project (no .aida/config.toml found)")?;
    let path = project_root.join(".mcp.json");
    let mut root: serde_json::Value = if path.exists() {
        let body = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&body).with_context(|| format!("parsing {}", path.display()))?
    } else {
        serde_json::json!({})
    };

    let servers = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!(".mcp.json must be a JSON object"))?
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));

    let servers_obj = servers
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("mcpServers must be a JSON object"))?;

    if servers_obj.contains_key(name) && !force {
        anyhow::bail!(
            "entry '{}' already exists in {} — pass --force to overwrite",
            name,
            path.display()
        );
    }

    servers_obj.insert(name.to_string(), entry);
    let body = serde_json::to_string_pretty(&root)?;
    std::fs::write(&path, body)?;

    println!(
        "Registered MCP agent '{}' in {} ({})",
        name,
        path.display(),
        server_url
    );
    println!("MCP-speaking agents (Codex, Cursor, etc.) can now call AIDA's 24 tools — see `aida mcp register-agent --print` for the full surface.");
    Ok(())
}

/// `aida mcp translate`: derive `.codex/config.toml` and
/// `.gemini/settings.json` from this project's `.mcp.json` AIDA server
/// registration. TASK-1046.
pub(crate) fn run_mcp_translate(
    project_root: &std::path::Path,
    force: bool,
    dry_run: bool,
) -> Result<()> {
    use crate::mcp_translate::{TranslateAction, TranslateReport};

    let report = crate::mcp_translate::translate(project_root, force, dry_run)?;

    let TranslateReport::Translated {
        server_name,
        codex,
        gemini,
    } = (match report {
        TranslateReport::NothingToTranslate { mcp_json_path } => {
            println!(
                "Nothing to translate: no AIDA MCP server registered in {}",
                mcp_json_path.display()
            );
            if !mcp_json_path.exists() {
                println!("  (the file itself doesn't exist — `aida mcp register-agent` or `aida init` creates it)");
            }
            return Ok(());
        }
        other => other,
    })
    else {
        unreachable!("NothingToTranslate handled above");
    };

    let describe = |label: &str, outcome: &crate::mcp_translate::TargetOutcome| {
        let verb = match outcome.action {
            TranslateAction::Created if dry_run => "would create/update",
            TranslateAction::Created => "wrote",
            TranslateAction::Updated if dry_run => "would overwrite",
            TranslateAction::Updated => "overwrote",
            TranslateAction::UpToDate => "already up to date",
            TranslateAction::SkippedExisting => "skipped (differs — use --force to overwrite)",
        };
        println!("{}: {} — {}", label, outcome.path.display(), verb);
    };

    println!(
        "Translating '{}' MCP server registration from .mcp.json{}",
        server_name,
        if dry_run { " (dry run)" } else { "" }
    );
    describe("Codex", &codex);
    describe("Gemini", &gemini);

    Ok(())
}

/// MCP server management commands (register agents, list tool surface).
///
/// The AIDA MCP server's coordination tools are how non-Claude-Code
/// agents (Codex, Cursor, …) participate in AIDA drains.
/// `aida mcp register-agent` writes the AIDA server's entry into a
/// project's `.mcp.json` so an MCP-speaking agent can connect.
// trace:STORY-361 | ai:claude
// trace:TASK-487 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "mcp")]
#[clap(hide = true)]
pub(crate) struct McpArgs {
    #[clap(subcommand)]
    pub inner: crate::cli::McpCommand,
}

fn run_mcp(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <McpArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
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
                McpArgs { inner: mcp_cmd } => {
                    // trace:STORY-361 | ai:claude
                    return crate::command_registry::returned(handle_mcp_command(mcp_cmd));
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            match &args {
                McpArgs { inner: mcp_cmd } => {
                    // STORY-361: management commands for the MCP coordination surface.
                    handle_mcp_command(mcp_cmd)?;
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static MCP: crate::command_registry::CommandReg = crate::command_registry::CommandReg {
    name: "mcp",
    help_after: "mcp-serve",
    command: <McpArgs as clap::CommandFactory>::command,
    policy: |_| crate::command_registry::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::Write,
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::command_registry::Handler::Store(run_mcp),
};
