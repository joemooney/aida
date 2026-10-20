//! `aida brief` command dispatch (write/list/ack/read agent pickup briefs
//! under `.aida/agent-briefs/`).
//!
//! The thin command entry point that routes the `aida brief` subcommands. The
//! brief store, rendering, list/collect/read/ack, and `.pending` sentinel
//! machinery is shared with the MCP brief tools, `aida doctor`, `aida compete`,
//! and the zen-finish review handoff, so it stays in `main.rs`; this module
//! holds only the command-exclusive dispatch plus its private deep-link and
//! note-reading helpers. Extracted verbatim from `main.rs` (SPIKE-78); no
//! behavior change.

use anyhow::{Context, Result};
use colored::Colorize;

// trace:STORY-1728 | ai:claude
#[path = "brief_cmd_args.rs"]
pub(crate) mod brief_cmd_args;
pub(crate) use brief_cmd_args::*;

use crate::*;

// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_brief_command(
    agent: Option<&str>,
    spec: Option<&str>,
    note: Option<&str>,
    depends_on: Option<&str>,
    as_deep_link: bool,
    notify: bool,
    authorized_by: Option<&str>,
    cmd: &Option<BriefCommand>,
    store: &RequirementsStore,
    project_root: &std::path::Path,
) -> Result<()> {
    match cmd {
        None => {
            let agent = agent.ok_or_else(|| {
                anyhow::anyhow!("usage: aida brief <agent> <SPEC-ID> [--note <STR>|--note -]")
            })?;
            let spec = spec.ok_or_else(|| {
                anyhow::anyhow!("usage: aida brief <agent> <SPEC-ID> [--note <STR>|--note -]")
            })?;
            let effective_spec = if let Some(pr) = parse_pr_arg(spec) {
                let resolved = resolve_pr_to_spec(project_root, pr, store)?;
                println!("briefing on {} (backs {})", resolved, spec);
                resolved
            } else {
                spec.to_string()
            };
            // STORY-528: brief-time GUARD — warn (do NOT refuse) when the
            // target agent is paused (budget/rate-limit), so the operator
            // knows the brief may sit unread until the agent resumes.
            if let Some(warning) = agent_registry::paused_warning_for_target(project_root, agent) {
                eprintln!("{}", warning.yellow());
            }
            let note = read_brief_note(note)?;
            let path = create_agent_brief(
                project_root,
                store,
                agent,
                &effective_spec,
                note.as_deref(),
                depends_on,
                authorized_by,
            )?;
            println!("{}", path.display());
            // TASK-502: --notify marks the brief urgent — write a `.pending`
            // sentinel so the receiving agent's `aida status` surfaces it
            // without a heartbeat. Idempotent. trace:TASK-502 | ai:claude
            if notify {
                add_pending_brief(project_root, agent, &path)?;
                eprintln!(
                    "📬 notified — {} will see this in `aida status`",
                    agent.cyan()
                );
            }
            // SPIKE-33: also print a claude-cli:// deep link the operator
            // can click → opens Claude Code in the spec's worktree (or
            // project root) with a short pickup prompt referencing the
            // brief file. Inert until Enter. trace:SPIKE-33 | ai:claude
            if as_deep_link {
                emit_brief_deep_link(project_root, agent, &effective_spec, &path)?;
            }
            Ok(())
        }
        Some(BriefCommand::List {
            for_agent,
            include_acked,
        }) => list_agent_briefs(project_root, for_agent.as_deref(), *include_acked),
        Some(BriefCommand::Ack { brief_file }) => ack_agent_brief(brief_file),
        Some(BriefCommand::Read { brief_file, latest }) => {
            read_agent_brief(project_root, brief_file, *latest)
        }
    }
}

/// SPIKE-33: render a `claude-cli://open` URL pointing at the agent's
/// expected worktree (active lease for the spec, else project_root) with
/// a short prompt that tells the receiving Claude session to read the
/// brief file and pick the work up. The brief body itself isn't inlined
/// in `q=` — even modest briefs blow past the 5000-char ceiling — and
/// AIDA's existing brief workflow already establishes that the agent
/// reads the file.
// trace:SPIKE-33 | ai:claude
fn emit_brief_deep_link(
    project_root: &std::path::Path,
    agent: &str,
    spec_id: &str,
    brief_path: &std::path::Path,
) -> Result<()> {
    let rel = brief_path
        .strip_prefix(project_root)
        .unwrap_or(brief_path)
        .display()
        .to_string();
    let worktree = list_leases(project_root)
        .into_iter()
        .find(|l| l.scope == spec_id && !l.worktree_path.as_os_str().is_empty())
        .map(|l| l.worktree_path)
        .unwrap_or_else(|| project_root.to_path_buf());
    let prompt = format!(
        "Pick up brief at {} ({} on {}). Acknowledge with `aida brief ack {}` when done.",
        rel, agent, spec_id, rel
    );
    let rendered = deep_link::DeepLink::new()
        .with_prompt(&prompt)
        .with_cwd(&worktree)
        .render();
    if rendered.exceeds {
        eprintln!(
            "{} brief deep-link prompt exceeded Claude Code's 5000-char URL ceiling — \
             link may fail to open",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        );
    }
    println!("{}", rendered.url);
    Ok(())
}

fn read_brief_note(note: Option<&str>) -> Result<Option<String>> {
    match note {
        Some("-") => {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
                .context("failed to read --note - from stdin")?;
            Ok(Some(buf.trim_end_matches(['\r', '\n']).to_string()))
        }
        Some(text) => Ok(Some(text.to_string())),
        None => Ok(None),
    }
}

/// Generate, list, and acknowledge per-agent pickup briefs.
// trace:TASK-492 | ai:codex
#[derive(clap::Parser, Debug)]
#[clap(name = "brief")]
pub(crate) struct BriefArgs {
    /// Agent name for brief creation, e.g. codex or antigravity.
    pub agent: Option<String>,

    /// Requirement id for brief creation.
    pub spec: Option<String>,

    /// Optional operator context. Use '-' to read a multi-line note from stdin.
    // trace:BUG-1294 | ai:claude
    #[clap(long, allow_hyphen_values = true)]
    pub note: Option<String>,

    /// SPEC-ID this brief must be picked up after.
    #[clap(long = "depends-on")]
    pub depends_on: Option<String>,

    /// Print a `claude-cli://open` deep link after writing the brief.
    /// Click → Claude Code opens in the spec's worktree with a
    /// short pickup prompt (inert until Enter). Eliminates the
    /// paste step. Requires Claude Code v2.1.91+ for the URL scheme.
    // trace:SPIKE-33 | ai:claude
    #[clap(long = "as-deep-link")]
    pub as_deep_link: bool,

    // trace:TASK-502 | ai:claude
    /// Mark the brief urgent: write a `.pending` sentinel so an idle
    /// agent's `aida status` (and statusline) surfaces it without a
    /// heartbeat. Omit for FYI-only briefs that shouldn't interrupt.
    #[clap(long)]
    pub notify: bool,

    /// The advisor id authorizing this brief's target worktree. Recorded
    /// on the brief and, if the receiving agent is launched with `aida
    /// agent new ... --spec <SPEC>` after this brief exists, carried into
    /// that session's role-context snapshot — the token the automatic
    /// advisor-lock gate reads at commit time under `[locking] posture =
    /// "warn"`/`"enforce"`.
    // trace:TASK-1140 | ai:claude
    #[clap(long = "authorized-by")]
    pub authorized_by: Option<String>,

    #[clap(subcommand)]
    pub cmd: Option<crate::cli::BriefCommand>,
}

fn run_brief(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <BriefArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
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
                BriefArgs {
                    agent,
                    spec,
                    note,
                    depends_on,
                    as_deep_link,
                    notify,
                    authorized_by,
                    cmd,
                } => {
                    let store = backend.load()?;
                    let project_root = store_path.parent().ok_or_else(|| {
                        anyhow::anyhow!("cannot derive project root from store path")
                    })?;
                    return crate::command_registry::returned(self::handle_brief_command(
                        agent.as_deref(),
                        spec.as_deref(),
                        note.as_deref(),
                        depends_on.as_deref(),
                        *as_deep_link,
                        *notify,
                        authorized_by.as_deref(),
                        cmd,
                        &store,
                        project_root,
                    ));
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
                BriefArgs {
                    agent,
                    spec,
                    note,
                    depends_on,
                    as_deep_link,
                    notify,
                    authorized_by,
                    cmd,
                } => {
                    let store = storage.load()?;
                    let project_root = find_project_root()
                        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| ".".into()));
                    self::handle_brief_command(
                        agent.as_deref(),
                        spec.as_deref(),
                        note.as_deref(),
                        depends_on.as_deref(),
                        *as_deep_link,
                        *notify,
                        authorized_by.as_deref(),
                        cmd,
                        &store,
                        &project_root,
                    )?;
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static BRIEF: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "brief",
        help_after: "worker",
        command: <BriefArgs as clap::CommandFactory>::command,
        policy: brief_cli_policy,
        run: crate::command_registry::Handler::Store(run_brief),
    };

// Per-verb policy, carried over from the legacy stakeholder and per-write
// auto-push tables. An unparseable invocation gets the fail-closed answer.
// trace:STORY-1728 | ai:claude
fn brief_cli_policy(matches: &clap::ArgMatches) -> crate::command_registry::CmdPolicy {
    use crate::cli::BriefCommand;
    use crate::stakeholder_role::StakeholderAction as SA;
    let args = <BriefArgs as clap::FromArgMatches>::from_arg_matches(matches).ok();
    let cmd = args.as_ref().and_then(|args| args.cmd.as_ref());
    crate::command_registry::CmdPolicy {
        stakeholder: if matches!(
            cmd,
            Some(BriefCommand::List { .. } | BriefCommand::Read { .. })
        ) {
            SA::Read
        } else {
            SA::Write
        },
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: true,
    }
}
