// trace:TASK-3428 | ai:codex
use crate::{
    agent_output_mode, agent_output_mode_from, output_format_is_json, IsTerminal, OutputFormat,
    Result,
};

// SPIKE-70: `--sequential` names + guards the ordered, per-member-PR SHAPE of the
// batch drain (`auto_complete::drain_batch*`) rather than introducing a parallel
// knob. TASK-185: it does NOT pin concurrency — STORY-1091 made the batch drain
// honour `[drain] pipeline_depth`, so the one-member-at-a-time property comes from
// `drain_state::default_pipeline_depth()` (1), not from this flag. The old
// `SEQUENTIAL_DRAIN_CONCURRENCY` const asserted the pinned-to-1 invariant and was
// removed with the claim; read the default depth instead.
// trace:TASK-1005 trace:TASK-185 | ai:claude

/// STORY-764: pure core of the agent-mode decision with the explicit `--format`
/// pin layered on top of the historical `AIDA_AGENT_OUTPUT`/TTY logic. An
/// explicit pin wins outright (`human` → human path, `toon`/`json` → agent
/// path); with no pin the behavior is byte-identical to [`agent_output_mode_from`].
/// Testable without touching the real env or a real terminal.
// trace:STORY-764 | ai:claude
pub(crate) fn resolve_agent_mode(
    pin: Option<OutputFormat>,
    agent_env: Option<&str>,
    stdout_is_tty: bool,
) -> bool {
    if let Some(fmt) = pin {
        return !matches!(fmt, OutputFormat::Human);
    }
    agent_output_mode_from(agent_env, stdout_is_tty)
}

/// Resolve the selected clap leaf and reject a JSON format pin unless that
/// leaf advertises a dedicated JSON projection. `--format` is global, so clap
/// otherwise accepts it for every command and lets unsupported handlers emit
/// successful human/TOON text. A local `--json` argument is the executable
/// declaration that the leaf owns a machine contract; the renderer must then
/// make both spellings identical.
// trace:BUG-1502 | ai:codex
pub(crate) fn enforce_json_format_capability(argv: &mut Vec<String>) -> Result<()> {
    if !output_format_is_json() {
        return Ok(());
    }
    json_capability_gate(argv)
}

/// [`enforce_json_format_capability`] once JSON has been requested.
pub(crate) fn json_capability_gate(argv: &mut Vec<String>) -> Result<()> {
    let root = crate::CLI_SUPPORT.built_command();
    let matches = root.clone().try_get_matches_from(argv.clone())?;
    let mut command = &root;
    let mut selected = &matches;
    let mut path = vec!["aida".to_string()];
    // BUG-1631: a `global = true` `--json` on an ancestor (e.g. `aida
    // history`) also covers its subcommands (`aida history events`).
    // trace:BUG-1631 | ai:claude
    let mut inherited_json = false;
    while let Some((name, submatches)) = selected.subcommand() {
        inherited_json |= command
            .get_arguments()
            .any(|argument| argument.get_id().as_str() == "json" && argument.is_global_set());
        let Some(subcommand) = command
            .get_subcommands()
            .find(|candidate| candidate.get_name() == name)
        else {
            anyhow::bail!(
                "could not resolve JSON capability for command `{}`",
                path.join(" ")
            );
        };
        path.push(name.to_string());
        command = subcommand;
        selected = submatches;
    }

    let supports_json = inherited_json
        || command
            .get_arguments()
            .any(|argument| argument.get_id().as_str() == "json");
    if !supports_json {
        // The old spelling of a renamed local format option (`export
        // --format json`) names that option, not just `--format human|toon`.
        // trace:BUG-2189 | ai:claude
        if selected.value_source("format") == Some(clap::parser::ValueSource::CommandLine) {
            if let Some(error) = crate::cmdkit::renamed_format_error(&path[1..].join(" "), "json") {
                return Err(error);
            }
        }
        anyhow::bail!(
            "`{} --format json` is unsupported: this command has no JSON projection; use `--format human` or `--format toon`",
            path.join(" ")
        );
    }

    // Materialize the global spelling as the selected leaf's dedicated flag.
    // This is the load-bearing dispatch guarantee: every existing handler sees
    // exactly the same parsed boolean for `--format json` as for `--json`, so
    // capability cannot drift from renderer wiring (for example, fasttrack
    // status previously declared `--json` but only inspected that local bool).
    // trace:BUG-1502 | ai:codex
    if !selected.get_flag("json") {
        argv.push("--json".to_string());
    }
    Ok(())
}

/// True when there is no human at the keyboard to answer a `Type 'y' to
/// confirm:` prompt — agent output mode is forced, or stdin is not a TTY. In
/// this state reading the prompt only hits EOF: the answer is never 'y', so the
/// historical `if !yes { read_line }` gate CANCELLED the write while an agent
/// capturing stdout (the "Cancelled" notice went to stderr) believed it
/// succeeded — a silent no-op that derails the implement -> done -> merge chain.
/// Callers must branch on this up front: AUTO-CONFIRM a reversible write (the
/// agent explicitly invoked the verb), or FAIL LOUDLY on a destructive one.
/// Never silently cancel a non-TTY write.
// trace:BUG-671
pub(crate) fn non_interactive_confirm() -> bool {
    non_interactive_confirm_from(agent_output_mode(), std::io::stdin().is_terminal())
}

/// Pure core of [`non_interactive_confirm`] (testable without a real terminal).
/// `agent_mode` is [`agent_output_mode`]'s verdict; `stdin_is_tty` is whether
/// stdin is a terminal.
// trace:BUG-671
pub(crate) fn non_interactive_confirm_from(agent_mode: bool, stdin_is_tty: bool) -> bool {
    agent_mode || !stdin_is_tty
}

/// BUG-721: gate for the interactive `aida review <spec>` verb's reviewer
/// launch. The verb spawns a `claude -p` reviewer subprocess against the live
/// repo and then drives a follow-up menu — it needs a human at the terminal to
/// watch that reviewer and answer. Returns true (may launch) only when BOTH
/// stdin and stdout are TTYs. When it returns false the verb must NOT spawn a
/// reviewer: a non-interactive `aida review` that silently launched a blind
/// headless reviewer the caller could neither see nor interrupt is exactly the
/// bug. This gate covers ONLY the interactive verb — the orchestrator's drain
/// review phase never reaches this code (it shells out to `aida queue work
/// PR-N` with `AIDA_AUTO_COMPLETE=1` and launches its headless reviewer there
/// on purpose, `run_reviewer`), so the autonomous drain is untouched.
// trace:BUG-721
pub(crate) fn review_may_launch_reviewer(stdin_is_tty: bool, stdout_is_tty: bool) -> bool {
    stdin_is_tty && stdout_is_tty
}

/// Apply the user's `--color=auto|always|never` choice to the colored
/// crate's global override. `auto` is the colored crate's default
/// behavior — it uses `isatty(stdout)` and respects `NO_COLOR`.
/// trace:FR-1-041 | ai:claude
pub(crate) fn apply_color_mode(mode: &str) {
    match mode {
        "always" => colored::control::set_override(true),
        "never" => colored::control::set_override(false),
        _ => {} // "auto" — let colored crate decide via tty detection
    }
}
