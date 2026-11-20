// trace:TASK-3428 | ai:codex
use crate::agent_config::{
    load_agent_default_flags, load_agents_bypass, load_agents_contained, load_agents_mcp,
};
#[cfg(test)]
use crate::agent_config::{write_global_agents_permission_posture, AgentPermissionPosture};
use crate::agent_launch::{
    refuse_unallowlisted_contained_passthrough, split_posture_flags, tool_bypass_flags,
    tool_contained_flags, AgentDefaultFlagOptions, AgentLaunchConfig,
};
use crate::kernel::{authority_stdin_is_terminal, authority_stdout_is_terminal};
use crate::{bypass_confirm, session};
use anyhow::Result;
use colored::Colorize;
// ----------------------------------------------------------------------------
// EPIC-31 Phase 2 — supervised agent launchers.
// trace:STORY-432 | ai:codex
// ----------------------------------------------------------------------------

/// Runtime vendor refusal shared by headless preflight and the agent-tool guard.
/// Native is accepted only by explicit project policy; it grants no AIDA containment.
// trace:TASK-2631 | ai:codex
pub(crate) fn refuse_uncontainable_agent_tool(agent_type: &str) -> Result<()> {
    if let Some(vendor) = session::HeadlessVendor::parse(agent_type) {
        session::containment_posture(vendor, true)?;
        return Ok(());
    }
    anyhow::bail!("`{agent_type}` cannot run an AIDA-contained session — refusing to launch it uncontained; use --vendor claude")
}

// trace:TASK-2631 | ai:codex
fn refuse_uncontainable_agent_tool_at(
    agent_type: &str,
    root: &std::path::Path,
) -> Result<session::ContainmentPosture> {
    session::containment_posture_in(root, agent_type, true)
}

pub(crate) fn apply_agent_default_flags(
    config: &mut AgentLaunchConfig,
    project_root: &std::path::Path,
    flag_options: AgentDefaultFlagOptions,
    explicit_permission: bool,
) -> Result<()> {
    // trace:BUG-2467 | ai:codex — carry explicit narrowing through every rotation.
    config.delegate_seats = flag_options.delegate_seats.clone();
    // BUG-2158: under a contained parent every claude passthrough flag (extra
    // or agents.toml default) must be on the allowlist; checked before the
    // BUG-1699 posture split, so a widening default is refused, not dropped.
    // trace:BUG-2158 | ai:claude
    let contained_claude_parent = config.agent_type == "claude" && session::aida_contained_env()?;
    if contained_claude_parent {
        refuse_unallowlisted_contained_passthrough(&flag_options.extra_flags, "--extra-flag")?;
    }
    let mut per_tool = if flag_options.use_config_defaults {
        load_agent_default_flags(project_root, config.agent_type)?
    } else {
        Vec::new()
    };
    if contained_claude_parent {
        refuse_unallowlisted_contained_passthrough(
            &per_tool,
            "[agents.claude] default_flags in agents.toml (pass --no-default-flags to skip them)",
        )?;
    }
    // trace:TASK-2631 | ai:codex — generated explicit Claude containment stays AIDA-only.
    // trace:TASK-2708 | ai:codex — reject disqualifying flags before policy
    // lookup and boundary composition. Posture resolution intentionally leaves
    // settings pending; callers with an already-composed boundary keep it.
    if config.contained {
        refuse_uncontainable_agent_tool_at(config.agent_type, project_root)?;
        if config.agent_type == "claude"
            && explicit_permission
            && !config.default_args.iter().any(|a| a == "--settings")
        {
            // trace:BUG-2131 | ai:codex
            aida_core::git_ops::ensure_git_supports_contained()?;
            let flags =
                tool_contained_flags(crate::seat_clone::launcher_main().as_deref(), "claude")?;
            config.default_args.splice(0..0, flags);
        }
    }
    if flag_options.use_config_defaults {
        // trace:TASK-2631 | ai:codex
        let native = !explicit_permission
            && load_agents_contained(project_root)?
            && refuse_uncontainable_agent_tool_at(config.agent_type, project_root)?
                == session::ContainmentPosture::Native;
        if native {
            let (_, posture) =
                split_posture_flags(config.agent_type, flag_options.extra_flags.clone());
            if !posture.is_empty() {
                anyhow::bail!("native sandbox, not AIDA-contained: drop conflicting --extra-flag posture options: {}", posture.join(" "));
            }
            // TASK-2712: the native write set is AIDA's alone; a passthrough
            // `--add-dir` (agents.toml or `--extra-flag`) is refused, never
            // forwarded or silently dropped.
            // trace:TASK-2712 | ai:claude
            crate::codex_native_sandbox::refuse_user_add_dir(
                &flag_options.extra_flags,
                "--extra-flag",
            )?;
            crate::codex_native_sandbox::refuse_user_add_dir(
                &per_tool,
                "[agents.codex] default_flags in agents.toml",
            )?;
            if load_agents_bypass(project_root)? {
                anyhow::bail!(
                    "[agents] bypass and [agents] contained are mutually exclusive launch postures"
                );
            }
            per_tool = split_posture_flags(config.agent_type, per_tool).0;
            config.default_args =
                split_posture_flags(config.agent_type, std::mem::take(&mut config.default_args)).0;
            // A native posture is a vendor sandbox (Codex). No host main is
            // pinned here (`queue work` reaches this after its chdir), so
            // `None`: a seat clone is not recognised and the write set fails
            // closed; launchers that know the lane rescope with their pinned
            // main (TASK-2032).
            config
                .default_args
                .extend(tool_contained_flags(None, config.agent_type)?);
            config.contained = false;
        }
        // STORY-495: the uniform `[agents] bypass` knob injects this tool's
        // bypass flag UNLESS (a) the launcher already set an explicit posture
        // (`--permission-mode` / `--bypass-sandbox` / forced `--bg`), or
        // (b) the tool carries its own per-tool `default_flags`, which take
        // precedence over the uniform knob (TASK-557 raw passthrough). A
        // `--no-default-flags` launch skips this block entirely (agents.toml
        // is never read), so it lands on the faithful native default.
        // trace:TASK-2631 | ai:codex
        // The root-aware refusal above preserves BUG-2144 for absent keys.
        // Native Codex has already received its explicit sandbox posture.
        if !explicit_permission && per_tool.is_empty() && !native {
            let bypass = load_agents_bypass(project_root)?;
            let contained = load_agents_contained(project_root)?;
            if bypass && contained {
                anyhow::bail!(
                    "[agents] bypass and [agents] contained are mutually exclusive launch postures"
                );
            }
            if contained {
                // trace:BUG-2131 | ai:claude
                aida_core::git_ops::ensure_git_supports_contained()?;
                // TASK-2032: only Claude's flags read the host main, and a
                // Claude launch reaches this from `aida agent new` at a host
                // cwd. The Codex arm, which `queue work` reaches after its
                // chdir into a seat, never resolves one.
                // trace:TASK-2032 | ai:claude
                let host_main = if config.agent_type == "claude" {
                    crate::seat_clone::launcher_main()
                } else {
                    None
                };
                config.default_args.extend(tool_contained_flags(
                    host_main.as_deref(),
                    config.agent_type,
                )?);
                config.contained = config.agent_type == "claude";
            } else if bypass {
                config
                    .default_args
                    .extend(tool_bypass_flags(config.agent_type));
                config.contained = false;
            }
        }
        // BUG-1699: per-tool `default_flags` used to be appended AFTER an explicit posture
        // the launcher had already set, producing one command line carrying both — e.g.
        // `codex --dangerously-bypass-approvals-and-sandbox --sandbox workspace-write
        // --ask-for-approval never` — and leaving the real posture to an undocumented
        // precedence inside the vendor CLI while the preview asserted the opposite. When the
        // launcher set the posture, the conflicting per-tool flags lose and we say so.
        // trace:BUG-1699 | ai:claude
        let per_tool = if explicit_permission {
            let (kept, dropped) = split_posture_flags(config.agent_type, per_tool);
            if !dropped.is_empty() {
                eprintln!(
                    "  {} explicit launch posture wins — dropped from agents.toml: {}",
                    "note:".yellow(),
                    dropped.join(" ")
                );
            }
            kept
        } else {
            per_tool
        };
        config.default_args.extend(per_tool);

        // BUG-1698 / TASK-1558: give a claude seat the configured AIDA surface. The default
        // (`off`) loads no MCP servers at all — SPIKE-73 measured MCP at ~1.8-2x the CLI's cost
        // for identical or worse success — and it also closes the project-MCP trust modal that
        // used to block every claude launch. Inside `use_config_defaults` so
        // `--no-default-flags` remains the escape hatch to the untouched native launch.
        // trace:BUG-1698 | ai:claude
        // trace:TASK-1558 | ai:claude
        if config.agent_type == "claude" {
            let surface = load_agents_mcp(project_root)?;
            config
                .default_args
                .extend(session::claude_mcp_flags(surface));
        }
        // BUG-2394: Codex has no strict-MCP flag; hold it to the same surface by
        // disabling each server its loaded config files declare with exact `-c`
        // overrides. Unset means `aida` for Codex: only an explicit `off` drops it.
        // The seat's tree is not known yet, so trust is read for `project_root`
        // here; `rescope_codex_mcp_flags` re-keys it on the seat once planned.
        // trace:BUG-2394 | ai:claude
        if config.agent_type == "codex" {
            let (surface, flags) = session::codex_mcp_launch_flags(project_root, project_root)?;
            if surface != session::AgentMcpSurface::Native {
                eprintln!(
                    "  codex mcp: {} ({})",
                    surface.as_str(),
                    session::CODEX_MCP_PLUGIN_NOTE
                );
            }
            config.default_args.extend(flags);
        }
    }
    let resolved_model = flag_options
        .model_override
        .as_deref()
        .filter(|m| !m.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            flag_options
                .use_config_defaults
                .then(|| {
                    aida_core::agents_config::resolve_vendor_model(project_root, config.agent_type)
                })
                .flatten()
        });
    // trace:STORY-1003 | ai:codex
    if let Some(model) = resolved_model {
        config.default_args.extend(["--model".to_string(), model]);
    }
    config.default_args.extend(flag_options.extra_flags);
    // BUG-2007: the codex launcher injects `--no-daemon`; an operator who also
    // passes it (extra flag or config default) must not get a doubled flag.
    // trace:BUG-2007 | ai:claude
    crate::agent_launch::dedupe_codex_no_daemon(config.agent_type, &mut config.default_args);
    Ok(())
}

// Apply the consent decision to the final argv, after every configured and
// explicit flag has been assembled. Preview callers only report this result.
// trace:TASK-1500 | ai:codex
pub(crate) fn gate_agent_bypass(
    root: &std::path::Path,
    agent: &str,
    args: &mut Vec<String>,
    background: bool,
    explicit: bool,
) -> Result<()> {
    use bypass_confirm::{BypassSource, Decision};
    let has_bypass = bypass_confirm::args_have_bypass(args);
    let source = if background {
        BypassSource::Background
    } else if explicit {
        BypassSource::Explicit
    } else {
        BypassSource::Configured
    };
    let setting = bypass_confirm::load(root);
    let tty = authority_stdin_is_terminal() && authority_stdout_is_terminal();
    match bypass_confirm::decide(has_bypass, source, setting.on, tty) {
        Decision::Proceed => Ok(()),
        Decision::Prompt if bypass_confirm::prompt(agent) => {
            eprintln!("  bypass confirmed at prompt");
            Ok(())
        }
        Decision::Prompt if matches!(source, BypassSource::Background) => {
            anyhow::bail!("background bypass needs affirmative terminal confirmation; pass --permission-mode <mode> for an explicit foreground launch")
        }
        Decision::Prompt => { bypass_confirm::strip_bypass_flags(args); eprintln!("  bypass declined; continuing with native permissions"); Ok(()) }
        Decision::DowngradeNative => { bypass_confirm::strip_bypass_flags(args); eprintln!("  bypass confirmation needs a terminal; continuing with native permissions"); Ok(()) }
        Decision::Refuse => anyhow::bail!("bypass needs confirmation at a terminal; run from an interactive shell or set [agents] confirm_bypass = false in your user config"),
    }
}

/// Write the global `~/.aida/agents.toml` recording the chosen permission
/// posture. Creates `~/.aida/` if needed. trace:TASK-698 | ai:claude
#[cfg(test)]
pub(crate) fn write_global_agents_posture(path: &std::path::Path, bypass: bool) -> Result<()> {
    write_global_agents_permission_posture(
        path,
        if bypass {
            AgentPermissionPosture::Bypass
        } else {
            AgentPermissionPosture::Native
        },
    )
}

/// STORY-495: resolve the effective `--permission-mode` for the
/// `session new` / `session start --launch` interactive launchers. `None`
/// means honor Claude's native posture (the faithful default). Precedence:
/// explicit flag > `[agents] bypass` knob > native. (Unlike `queue work`,
/// these launchers do not layer the `AIDA_PERMISSION_MODE` env or
/// `[behavior] permission_mode` config — that resolution is queue-work
/// specific and predates this change.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedClaudeLaunch {
    pub(crate) mode: Option<String>,
    pub(crate) contained: bool,
}
