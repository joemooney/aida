//! `aida ultraplan` command cluster (TASK-113 / TASK-247 / TASK-304 /
//! TASK-514 / TASK-517).
//!
//! `handle_ultraplan_command` assembles a rich planner prompt from a spec's
//! context — description + `## Acceptance` + trace-graph helpers + reserved
//! namespaces + the 11-section plan structure — and delivers it via clipboard
//! (default), `--stdout`, or `--json`. The private helpers render the
//! machine-readable JSON value (`ultraplan_json_value`) and the clipboard
//! success message (`ultraplan_copy_success_message`). The heavier prompt
//! assembly (`assemble_ultraplan_prompt`, `build_reusable_helpers_section`,
//! `read_reserved_paths`) is shared with `aida compete` and stays in `main.rs`,
//! reached here via `crate::`. Extracted verbatim from `main.rs` (SPIKE-78); no
//! behavior change.

use anyhow::Result;
use colored::Colorize;

use crate::{
    assemble_ultraplan_prompt, build_reusable_helpers_section, copy_to_clipboard,
    find_project_root, load_store_for_lookup, read_reserved_paths, read_ultraplan_config,
    ReservedPath, UltraplanMode,
};

fn ultraplan_json_value(
    display: &str,
    title: &str,
    prompt: &str,
    token_estimate: usize,
    warnings: &[String],
    reservations: &[ReservedPath],
) -> serde_json::Value {
    serde_json::json!({
        "spec_id": display,
        "title": title,
        "prompt": prompt,
        "token_estimate": token_estimate,
        "warnings": warnings,
        "reservations": reservations,
    })
}

// trace:TASK-514 | ai:antigravity
// trace:BUG-1177 | ai:codex
pub(crate) fn ultraplan_copy_success_message(display: &str, token_estimate: usize) -> String {
    format!(
        "assembled planner prompt for {} (~{} tokens) — copied to clipboard (use --stdout to print, --json for machine consumption)",
        display,
        token_estimate
    )
}

/// `aida ultraplan <SPEC>` — assemble + deliver the planning prompt.
pub(crate) fn handle_ultraplan_command(
    spec_arg: &str,
    stdout: bool,
    json: bool,
    no_comments: bool,
) -> Result<()> {
    let project_root = find_project_root()?;
    // TASK-304: `[ultraplan] mode = "never"` disables the integration for
    // this project — refuse with a message and write nothing to the
    // clipboard. Printed to stderr so `--json` / `--stdout` consumers see a
    // clean (empty) stdout. trace:TASK-304 | ai:claude
    if read_ultraplan_config(&project_root).mode == UltraplanMode::Never {
        eprintln!(
            "{} `aida ultraplan` is disabled for this project \
             (`[ultraplan] mode = \"never\"` in .aida/config.toml).",
            "Note:".yellow().bold()
        );
        eprintln!(
            "  {}",
            "Set mode = \"on-demand\" or \"suggested\" to re-enable.".dimmed()
        );
        return Ok(());
    }
    let store = load_store_for_lookup(&project_root)
        .ok_or_else(|| anyhow::anyhow!("could not load the AIDA requirements store"))?;
    let target = if let Ok(uuid) = uuid::Uuid::parse_str(spec_arg) {
        store.requirements.iter().find(|r| r.id == uuid)
    } else {
        store.get_requirement_by_spec_id(spec_arg)
    }
    .ok_or_else(|| anyhow::anyhow!("requirement `{spec_arg}` not found"))?;

    let helpers = build_reusable_helpers_section(&store, &project_root, target);
    let (reservations, reservation_warnings) = read_reserved_paths(&project_root);
    let (prompt, warnings) = assemble_ultraplan_prompt(
        &store,
        target,
        helpers.as_deref(),
        !no_comments,
        &reservations,
    );
    crate::init_cmd::ensure_plan_template_scaffold(
        &project_root.join("docs").join("plans"),
        false,
    )?;
    let mut warnings = warnings;
    warnings.extend(reservation_warnings);
    let token_estimate = prompt.chars().count() / 4;
    let display = target.display_id();

    if json {
        let out = ultraplan_json_value(
            &display,
            &target.title,
            &prompt,
            token_estimate,
            &warnings,
            &reservations,
        );
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    if stdout {
        print!("{prompt}");
        for w in &warnings {
            eprintln!("{} {}", "Warning:".yellow().bold(), w);
        }
        return Ok(());
    }

    // Default: copy to clipboard, falling back to stdout when no tool exists.
    // trace:TASK-514 | ai:antigravity
    if copy_to_clipboard(&prompt) {
        println!(
            "{} {}",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            ultraplan_copy_success_message(&display.bold().to_string(), token_estimate)
        );
        println!(
            "  {}",
            "next: run `/aida-plan <SPEC>` in Claude Code, or hand this prompt to any planner"
                .dimmed()
        );
        // TASK-305: external planner flows may land a PR directly without
        // writing a local plan file. Nudge the user to reconcile it back
        // into docs/plans/ once that PR lands.
        println!(
            "  {}",
            "web flow? after the PR lands, run `aida plan capture <PR>` to archive the plan"
                .dimmed()
        );
    } else {
        eprintln!(
            "{} no clipboard tool found (wl-copy/xclip/xsel/pbcopy/clip) — printing instead",
            "Note:".bold()
        );
        print!("{prompt}");
    }
    for w in &warnings {
        eprintln!("{} {}", "Warning:".yellow().bold(), w);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ultraplan_json_includes_reservations_array() {
        let reservations = vec![ReservedPath {
            path: "docs/aida/".into(),
            reason: "reserved by docs build".into(),
        }];
        let warnings = vec!["heads up".to_string()];

        let value = ultraplan_json_value(
            "TASK-517",
            "reserved paths",
            "prompt",
            123,
            &warnings,
            &reservations,
        );

        assert_eq!(value["spec_id"], "TASK-517");
        assert_eq!(value["reservations"][0]["path"], "docs/aida/");
        assert_eq!(value["reservations"][0]["reason"], "reserved by docs build");
    }

    /// TASK-514: test that success message text contains '--stdout' hint
    // trace:TASK-514 | ai:antigravity
    #[test]
    fn test_ultraplan_success_message_contains_stdout_hint() {
        let msg = ultraplan_copy_success_message("TASK-514", 2655);
        assert!(msg.contains("use --stdout to print"));
        assert!(msg.contains("TASK-514"));
        assert!(msg.contains("2655"));
        assert!(msg.contains("assembled planner prompt"));
        assert!(!msg.contains("slash-command prompt"));
    }
}

/// Assemble a rich, structured planning prompt for a SPEC. Pulls the
/// spec's description, acceptance criteria, related-spec context, the
/// spec's enrichment comments, the AIDA plan-template structure, and
/// the trace-graph reusable helpers into one prompt — turning a terse
/// ask into a fully-contextualised brief for `/aida-plan`, a Plan
/// agent, a multi-agent workflow, or a human reviewer. Copies to the
/// clipboard by default.
// trace:BUG-1177 | ai:codex
// trace:TASK-113 TASK-247 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "ultraplan")]
pub(crate) struct UltraplanArgs {
    /// SPEC-ID (or UUID) to assemble the planning prompt for.
    pub spec: String,

    /// Print the assembled prompt to stdout instead of copying it to
    /// the clipboard. Also the headless fallback when no clipboard
    /// tool is available.
    #[clap(long, conflicts_with = "json")]
    pub stdout: bool,

    /// Emit the prompt + metadata (warnings, token estimate) as JSON
    /// for scripting.
    #[clap(long)]
    pub json: bool,

    /// Copy the assembled prompt to the clipboard (default behavior).
    /// This flag is an explicit alias for the default, provided for
    /// command-line discoverability and ease of use.
    // trace:TASK-514 | ai:antigravity
    #[clap(long, conflicts_with = "stdout", conflicts_with = "json")]
    pub copy: bool,

    /// Omit the `## Comments` section — the spec's enrichment
    /// comments are pulled in by default; pass this for the
    /// leaner comment-free prompt.
    // trace:TASK-247 | ai:claude
    // trace:TASK-487 | ai:claude
    #[clap(long)]
    pub no_comments: bool,
}

// `aida ultraplan` also self-loads the store via `load_store_for_lookup`.
// trace:TASK-113 | ai:claude
fn run_ultraplan(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    use crate::*;
    let args = <UltraplanArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let UltraplanArgs {
        spec,
        stdout,
        json,
        copy: _,
        no_comments,
    } = &args;
    return self::handle_ultraplan_command(spec, *stdout, *json, *no_comments);
}

// trace:STORY-1728 | ai:claude
pub(crate) static ULTRAPLAN: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "ultraplan",
        help_after: "manual",
        command: <UltraplanArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Read,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
        },
        run: crate::command_registry::Handler::Early(run_ultraplan),
    };
