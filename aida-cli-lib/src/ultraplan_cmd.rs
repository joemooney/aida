//! `aida ultraplan` command cluster (TASK-113 / TASK-247 / TASK-304 /
//! TASK-514 / TASK-517).
//!
//! `handle_ultraplan_command` assembles the rich `/ultraplan` planning prompt
//! from a spec's context — description + `## Acceptance` + trace-graph helpers +
//! reserved namespaces + the 11-section plan structure — and delivers it via
//! clipboard (default), `--stdout`, or `--json`. The private helpers render the
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
pub(crate) fn ultraplan_copy_success_message(display: &str, token_estimate: usize) -> String {
    format!(
        "assembled /ultraplan prompt for {} (~{} tokens) — copied to clipboard (use --stdout to print, --json for machine consumption)",
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
            "paste it into a Claude Code session, prefixed with /ultraplan".dimmed()
        );
        // TASK-305: the web /ultraplan flow lands a PR directly without
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
    }
}
