//! Output format configurations (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

// trace:TASK-3428 | ai:codex
use crate::cli::OutputFormat;
// trace:TASK-3783 | ai:codex
use crate::kernel::find_project_root;
use crate::leases::list_leases;
use crate::worktree_gc::find_main_worktree_root;
use crate::{build_list_fields_lines_with, resolve_agent_mode};
use crate::{headless_tail, tail_cmd};
pub(crate) use aida_term::output_format::*;
use anyhow::Result;
use std::io::IsTerminal;

/// Build and dispatch the shared tail resolver from clap-parsed arguments.
/// Both `aida tail drain` and `aida drain tail` enter here, then flow through
/// `tail_cmd::handle_tail`.
// trace:TASK-1209 | ai:codex
pub(crate) fn handle_tail_cli(
    target: Option<String>,
    list: bool,
    json: bool,
    lines: Option<usize>,
    since: Option<&str>,
    no_follow: bool,
    with_tools: bool,
    no_timestamp: bool,
    annotate: bool,
) -> Result<()> {
    // trace:BUG-1289 | ai:claude
    let json = json || output_format_is_json();
    let project_root = find_main_worktree_root()
        .or_else(|_| find_project_root())
        .or_else(|_| std::env::current_dir())
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    let since_duration = match since {
        Some(s) => Some(headless_tail::parse_since(s)?),
        None => None,
    };
    let sessions: Vec<tail_cmd::SessionRef> = list_leases(&project_root)
        .into_iter()
        .map(|l| tail_cmd::SessionRef {
            id: l.id,
            scope: l.scope,
            branch: l.branch,
            role: l.role,
        })
        .collect();
    let opts = tail_cmd::TailOptions {
        target,
        list,
        json,
        lines,
        since: since_duration,
        no_follow,
        with_tools,
        color: std::io::IsTerminal::is_terminal(&std::io::stdout())
            && std::env::var_os("NO_COLOR").is_none(),
        no_timestamp,
        annotate,
    };
    tail_cmd::handle_tail(&project_root, sessions, &opts)
}

/// Print the human `aida list --fields <csv>` table. Thin wrapper over
/// [`build_list_fields_lines`].
// trace:STORY-734 | ai:claude — plain `//` keeps the marker out of any doc/help.
pub(crate) fn render_list_fields_table<F, C>(
    reqs: &[aida_core::RequirementSummary],
    fields: &[String],
    routing_of: F,
    cell_of: C,
) where
    F: Fn(&aida_core::RequirementSummary) -> (bool, bool, bool),
    C: Fn(&aida_core::RequirementSummary, &str) -> Option<String>,
{
    // trace:TASK-1951 trace:TASK-2003 | ai:claude
    for line in build_list_fields_lines_with(reqs, fields, routing_of, cell_of) {
        println!("{line}");
    }
}

#[cfg(test)]
#[path = "tests/task970_agent_output_tests.rs"]
mod task970_agent_output_tests;

// trace:TASK-2205 | ai:claude
#[cfg(test)]
#[path = "tests/task_2205_list_fields_tests.rs"]
mod task_2205_list_fields_tests;

// trace:BUG-1813 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1813_toon_hint_tests.rs"]
mod bug_1813_toon_hint_tests;

// trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
#[cfg(test)]
#[path = "tests/node_alias_display_ratchet_tests.rs"]
mod node_alias_display_ratchet_tests;

// trace:TASK-3783 | ai:codex
fn to_term(format: OutputFormat) -> aida_core::output_format::OutputFormat {
    match format {
        OutputFormat::Human => aida_core::output_format::OutputFormat::Human,
        OutputFormat::Toon => aida_core::output_format::OutputFormat::Toon,
        OutputFormat::Json => aida_core::output_format::OutputFormat::Json,
    }
}
fn from_term(format: aida_core::output_format::OutputFormat) -> OutputFormat {
    match format {
        aida_core::output_format::OutputFormat::Human => OutputFormat::Human,
        aida_core::output_format::OutputFormat::Toon => OutputFormat::Toon,
        aida_core::output_format::OutputFormat::Json => OutputFormat::Json,
    }
}
fn hint_context() -> (Option<std::path::PathBuf>, Option<std::path::PathBuf>) {
    (find_project_root().ok(), crate::home_dir())
}
pub(crate) fn agent_output_mode() -> bool {
    aida_term::output_format::agent_output_mode(hint_context)
}
pub(crate) fn set_output_format_override(flag: Option<OutputFormat>) {
    aida_term::output_format::set_output_format_override(flag.map(to_term));
}
pub(crate) fn output_format_from_env() -> Option<OutputFormat> {
    aida_term::output_format::output_format_from_env().map(from_term)
}
pub(crate) fn parse_output_format(raw: Option<&str>) -> Option<OutputFormat> {
    aida_term::output_format::parse_output_format(raw).map(from_term)
}
pub(crate) fn output_format_override() -> Option<OutputFormat> {
    aida_term::output_format::output_format_override().map(from_term)
}
pub(crate) fn toon_switch_hint_applies(
    pin: Option<OutputFormat>,
    env: Option<&str>,
    out_tty: bool,
    err_tty: bool,
) -> bool {
    aida_term::output_format::toon_switch_hint_applies(pin.map(to_term), env, out_tty, err_tty)
}
pub(crate) fn maybe_emit_toon_switch_hint() {
    aida_term::output_format::maybe_emit_toon_switch_hint(hint_context);
}
