//! Output format configurations (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// TASK-970: the agent-ergonomics output gate. Two AIDA surfaces lean toward
/// agent-friendly output when the caller is a non-interactive agent rather than
/// a human at a TTY: bare `aida` routes to the status snapshot (not the
/// getting-started menu), and bare `aida list` applies a default row cap.
///
/// AGENT MODE is true when EITHER `AIDA_AGENT_OUTPUT` is set to a truthy value
/// OR stdout is not a TTY (piped / headless / MCP). `AIDA_AGENT_OUTPUT=0`
/// (or `false`/`no`/`off`) force-selects the HUMAN path even when piped, which
/// also makes the human path testable without a real terminal. The human-at-a-
/// TTY path is left byte-identical; everything gated on this is agent-only.
// trace:TASK-970
pub(crate) fn agent_output_mode_quiet() -> bool {
    let pin = output_format_override();
    let env = std::env::var("AIDA_AGENT_OUTPUT").ok();
    let stdout_is_tty = std::io::stdout().is_terminal();
    resolve_agent_mode(pin, env.as_deref(), stdout_is_tty)
}

pub(crate) fn agent_output_mode() -> bool {
    // STORY-764: an explicit `--format` / `AIDA_OUTPUT_FORMAT` pin wins over the
    // `AIDA_AGENT_OUTPUT` env + TTY default. `human` selects the human path;
    // `toon` / `json` both select the agent (machine) path. This is the durable
    // escape hatch scripts use so piped/captured output no longer silently
    // switches format out from under them (BUG-707). trace:STORY-764 | ai:claude
    let pin = output_format_override();
    let env = std::env::var("AIDA_AGENT_OUTPUT").ok();
    let stdout_is_tty = std::io::stdout().is_terminal();
    let agent = resolve_agent_mode(pin, env.as_deref(), stdout_is_tty);
    // STORY-764: loud-but-once. When we auto-switched to the compact agent format
    // purely because stdout is not a terminal — no explicit pin, no explicit
    // `AIDA_AGENT_OUTPUT` — nudge the caller (on STDERR, so piped STDOUT stays
    // clean) toward `--format` the first time. trace:STORY-764 | ai:claude
    if agent && pin.is_none() && env.is_none() && !stdout_is_tty {
        maybe_emit_toon_switch_hint();
    }
    agent
}

/// STORY-764: process-global explicit output-format override, installed exactly
/// once from the global `--format` flag (or `AIDA_OUTPUT_FORMAT`) right after
/// argv parsing. Everything that renders consults [`agent_output_mode`] /
/// [`output_format_is_json`], which read this first — so the pin is honored
/// uniformly without threading a format param through every handler.
// trace:STORY-764 | ai:claude
pub(crate) static OUTPUT_FORMAT_OVERRIDE: std::sync::OnceLock<Option<OutputFormat>> =
    std::sync::OnceLock::new();

/// Resolve + install the [`OUTPUT_FORMAT_OVERRIDE`] once. Precedence: the
/// `--format` flag wins; else `AIDA_OUTPUT_FORMAT` (human|toon|json,
/// case-insensitive); else `None` (fall back to the TTY-based default).
/// Idempotent — a later call is a no-op (the first install sticks).
// trace:STORY-764 | ai:claude
pub(crate) fn set_output_format_override(flag: Option<OutputFormat>) {
    let resolved = flag.or_else(output_format_from_env);
    let _ = OUTPUT_FORMAT_OVERRIDE.set(resolved);
}

/// Parse `AIDA_OUTPUT_FORMAT` into an [`OutputFormat`]. Unset or unrecognized
/// values yield `None` (defer to the TTY-based default rather than erroring, so
/// a stray value never breaks a command).
// trace:STORY-764 | ai:claude
pub(crate) fn output_format_from_env() -> Option<OutputFormat> {
    parse_output_format(std::env::var("AIDA_OUTPUT_FORMAT").ok().as_deref())
}

/// Pure parse of an `AIDA_OUTPUT_FORMAT` value into an [`OutputFormat`]. Unset
/// (`None`) or an unrecognized value yields `None` (defer to the TTY-based
/// default rather than erroring). Case-insensitive; `table`/`agent` are
/// accepted spellings of `human`/`toon`.
// trace:STORY-764 | ai:claude
pub(crate) fn parse_output_format(raw: Option<&str>) -> Option<OutputFormat> {
    match raw?.trim().to_ascii_lowercase().as_str() {
        "human" | "table" => Some(OutputFormat::Human),
        "toon" | "agent" => Some(OutputFormat::Toon),
        "json" => Some(OutputFormat::Json),
        _ => None,
    }
}

/// The installed explicit override, if any. `None` until
/// [`set_output_format_override`] runs (or if no pin was given).
// trace:STORY-764 | ai:claude
pub(crate) fn output_format_override() -> Option<OutputFormat> {
    OUTPUT_FORMAT_OVERRIDE.get().copied().flatten()
}

/// True when the explicit pin selects JSON. Commands that carry a `--json`
/// flag OR this in, so `--format json` / `AIDA_OUTPUT_FORMAT=json` reaches them
/// uniformly.
// trace:STORY-764 | ai:claude
pub(crate) fn output_format_is_json() -> bool {
    matches!(output_format_override(), Some(OutputFormat::Json))
}

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

/// Pure core of [`agent_output_mode`] (testable without touching the real env
/// or a real terminal). `env` is the `AIDA_AGENT_OUTPUT` value (None = unset);
/// `stdout_is_tty` is whether stdout is a terminal.
// trace:TASK-970
pub(crate) fn agent_output_mode_from(env: Option<&str>, stdout_is_tty: bool) -> bool {
    match env {
        Some(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "no" | "off"
        ),
        None => !stdout_is_tty,
    }
}

/// Resolve the requested `--fields` selection for agent-mode `aida list` into a
/// validated, ordered field list. `None` (no `--fields`) yields the minimal
/// default. A `csv` string is split on commas, trimmed, lowercased, and checked
/// against [`TOON_LIST_KNOWN_FIELDS`]; an unknown name is an error naming the
/// valid set. Empty selection falls back to the default.
// trace:TASK-964
pub(crate) fn toon_list_fields(csv: Option<&str>) -> Result<Vec<String>> {
    let Some(csv) = csv else {
        return Ok(TOON_LIST_DEFAULT_FIELDS
            .iter()
            .map(|s| s.to_string())
            .collect());
    };
    let mut out = Vec::new();
    for raw in csv.split(',') {
        let name = raw.trim().to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }
        // `req_type` is a friendly alias for `type`.
        let name = if name == "req_type" {
            "type".to_string()
        } else {
            name
        };
        if !TOON_LIST_KNOWN_FIELDS.contains(&name.as_str()) {
            anyhow::bail!(
                "unknown --fields entry `{name}`; valid fields: {}",
                TOON_LIST_KNOWN_FIELDS.join(", ")
            );
        }
        out.push(name);
    }
    if out.is_empty() {
        return Ok(TOON_LIST_DEFAULT_FIELDS
            .iter()
            .map(|s| s.to_string())
            .collect());
    }
    Ok(out)
}

/// Project one requirement summary + its routing triple `(in_flight, blocked,
/// queued)` onto a single named list field, as the cell string for a TOON row.
// trace:TASK-964
pub(crate) fn toon_list_cell(
    r: &aida_core::RequirementSummary,
    routing: (bool, bool, bool),
    field: &str,
) -> String {
    let (in_flight, blocked, queued) = routing;
    match field {
        "id" => r
            .agreed_id
            .as_deref()
            .or(r.spec_id.as_deref())
            .unwrap_or("")
            .to_string(),
        "title" => r.title.clone(),
        // BUG-781: the agent surface reports the TERMINAL truth for an accepted
        // decision — `accepted`, the recognized ADR verb — not the `approved`
        // token that reads as "cleared to start". trace:BUG-781 | ai:claude
        "status" => toon_status_token(crate::status_display::display_status_for_type(
            &r.req_type,
            &r.status,
        )),
        "type" => r.req_type.to_ascii_lowercase(),
        "priority" => r.priority.to_ascii_lowercase(),
        "feature" => r.feature.clone(),
        "owner" => r.owner.clone(),
        "assignee" => r.assignee.clone().unwrap_or_default(),
        "tags" => r.tags.join(" "),
        "heft" => r.heft.to_string(),
        // trace:TASK-1215 | ai:codex
        "modified_at" => r.modified_at.clone(),
        // trace:FR-283 | ai:claude — empty cell = no weight set.
        "weight" => r.weight.map(format_weight).unwrap_or_default(),
        "queued" => queued.to_string(),
        "in_flight" => in_flight.to_string(),
        "blocked" => blocked.to_string(),
        // trace:STORY-776 | ai:claude — empty cell = ungroomed.
        "mode" => r.execution_mode.clone().unwrap_or_default(),
        // trace:STORY-634 | ai:claude — empty cell = single-repo.
        "origin" => r.origin.clone().unwrap_or_default(),
        "deferred_reason" => r.deferred_reason.clone().unwrap_or_default(),
        _ => String::new(),
    }
}

/// Print the human `aida list --fields <csv>` table. Thin wrapper over
/// [`build_list_fields_lines`].
// trace:STORY-734 | ai:claude — plain `//` keeps the marker out of any doc/help.
pub(crate) fn render_list_fields_table<F>(
    reqs: &[aida_core::RequirementSummary],
    fields: &[String],
    routing_of: F,
) where
    F: Fn(&aida_core::RequirementSummary) -> (bool, bool, bool),
{
    for line in build_list_fields_lines(reqs, fields, routing_of) {
        println!("{line}");
    }
}

#[cfg(test)]
#[path = "tests/task970_agent_output_tests.rs"]
mod task970_agent_output_tests;
