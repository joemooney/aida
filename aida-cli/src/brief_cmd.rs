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
