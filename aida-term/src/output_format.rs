//! Pure output-format rendering helpers.
// trace:TASK-3783 | ai:codex
// trace:TASK-3783 | ai:codex
pub use aida_core::output_format::{parse_output_format, OutputFormat};
use anyhow::Result;
use std::io::IsTerminal;

/// Pure core of [`agent_output_mode`] (testable without touching the real env
/// or a real terminal). `env` is the `AIDA_AGENT_OUTPUT` value (None = unset);
/// `stdout_is_tty` is whether stdout is a terminal.
// trace:TASK-970
pub fn agent_output_mode_from(env: Option<&str>, stdout_is_tty: bool) -> bool {
    match env {
        Some(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "no" | "off"
        ),
        None => !stdout_is_tty,
    }
}

/// The agent-mode `local_id` cell: the spec's local ID when it differs from
/// the agreed ID shown in `id`, `(agreed id pending)` for a local-only spec,
/// and empty for a spec with one ID.
// trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude
pub fn toon_local_id_cell(agreed_id: Option<&str>, spec_id: Option<&str>) -> String {
    if aida_core::node_alias::agreed_id_pending(agreed_id, spec_id) {
        return "(agreed id pending)".to_string();
    }
    let agreed = agreed_id.map(str::trim).filter(|a| !a.is_empty());
    let local = spec_id.map(str::trim).filter(|s| !s.is_empty());
    match (agreed, local) {
        (Some(a), Some(l)) if !aida_core::id_identity::same_id(a, l) => {
            aida_core::node_alias::display_spec_id(l)
        }
        _ => String::new(),
    }
}

/// Whether any row has a `local_id` cell, so the default agent list adds the
/// column right after `id` (a store with only one-ID specs keeps its shape).
// trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude
pub fn with_local_id_column(
    mut selected: Vec<String>,
    explicit_fields: bool,
    reqs: &[aida_core::RequirementSummary],
) -> Vec<String> {
    if explicit_fields || selected.iter().any(|f| f == "local_id") {
        return selected;
    }
    let any = reqs
        .iter()
        .any(|r| !toon_local_id_cell(r.agreed_id.as_deref(), r.spec_id.as_deref()).is_empty());
    if any {
        let at = selected.iter().position(|f| f == "id").map_or(0, |i| i + 1);
        selected.insert(at, "local_id".to_string());
    }
    selected
}

/// Where the once-marker lives: the project's `.aida/` when inside a project,
/// else the user's `~/.aida/` so a shell outside any project still remembers
/// the hint was shown, else `None` (nowhere to persist it).
// trace:BUG-1813 | ai:claude
pub fn toon_switch_hint_marker(
    project_root: Option<&std::path::Path>,
    home: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    project_root
        .or(home)
        .map(|dir| dir.join(".aida").join(TOON_SWITCH_HINT_MARKER))
}

/// Write [`TOON_SWITCH_HINT`] to `err` at most once per `marker`. The marker is
/// persisted FIRST and the hint is written only when that succeeded: a hint
/// that cannot be remembered would otherwise reappear on every invocation
/// (each script call is a fresh process), which is exactly the spam this guards
/// against. `None` (nowhere to persist), an existing marker, or an unwritable
/// marker all mean silence. Returns true when the hint was written.
// trace:BUG-1813 | ai:claude
pub fn emit_toon_switch_hint_once(
    marker: Option<&std::path::Path>,
    err: &mut dyn std::io::Write,
) -> bool {
    let Some(marker) = marker else {
        return false;
    };
    if marker.exists() {
        return false;
    }
    if marker
        .parent()
        .is_some_and(|dir| std::fs::create_dir_all(dir).is_err())
    {
        return false;
    }
    if std::fs::write(marker, b"").is_err() {
        return false;
    }
    let _ = writeln!(err, "{TOON_SWITCH_HINT}");
    true
}

/// Resolve the requested `--fields` selection for agent-mode `aida list` into a
/// validated, ordered field list. `None` (no `--fields`) yields the minimal
/// default. A `csv` string is split on commas and/or whitespace (so a copied
/// TOON header's field list works; the human table's display headers, such as
/// `Modified`, are labels rather than field names and are rejected),
/// lowercased, alias-resolved via
/// [`TOON_LIST_FIELD_ALIASES`], and checked against [`TOON_LIST_KNOWN_FIELDS`];
/// an unknown name is an error naming the valid set. Empty segments are
/// ignored; an empty selection falls back to the default.
// trace:TASK-964
// trace:TASK-2205 | ai:claude
// trace:TASK-2238 | ai:claude
pub fn toon_list_fields(csv: Option<&str>) -> Result<Vec<String>> {
    let Some(csv) = csv else {
        return Ok(TOON_LIST_DEFAULT_FIELDS
            .iter()
            .map(|s| s.to_string())
            .collect());
    };
    let mut out = Vec::new();
    for raw in csv.split(|c: char| c == ',' || c.is_whitespace()) {
        let name = raw.to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }
        let name = TOON_LIST_FIELD_ALIASES
            .iter()
            .find(|(alias, _)| *alias == name)
            .map_or(name, |(_, field)| field.to_string());
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
pub fn toon_list_cell(
    r: &aida_core::RequirementSummary,
    routing: (bool, bool, bool),
    field: &str,
) -> String {
    let (in_flight, blocked, queued) = routing;
    match field {
        // trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
        "id" => aida_core::node_alias::display_spec_id(
            r.agreed_id
                .as_deref()
                .or(r.spec_id.as_deref())
                .unwrap_or(""),
        ),
        // The local ID beside the agreed one; a local-only spec says its
        // agreed ID is pending (its `id` cell already is the local ID).
        // trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude
        "local_id" => toon_local_id_cell(r.agreed_id.as_deref(), r.spec_id.as_deref()),
        "title" => r.title.clone(),
        // BUG-781: the agent surface reports the TERMINAL truth for an accepted
        // decision — `accepted`, the recognized ADR verb — not the `approved`
        // token that reads as "cleared to start". trace:BUG-781 | ai:claude
        "status" => toon_status_token(
            if aida_core::lifecycle::is_accepted_decision(&r.req_type, &r.status) {
                "Accepted"
            } else {
                &r.status
            },
        ),
        "type" => r.req_type.to_ascii_lowercase(),
        "priority" => r.priority.to_ascii_lowercase(),
        "feature" => r.feature.clone(),
        "owner" => r.owner.clone(),
        "assignee" => r.assignee.clone().unwrap_or_default(),
        "tags" => r.tags.join(" "),
        "heft" => r.heft.to_string(),
        // trace:TASK-1215 | ai:codex
        "modified_at" => r.modified_at.clone(),
        // trace:TASK-2205 | ai:claude — rendered like modified_at.
        "opened" => r.created_at.clone(),
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

/// STORY-764's one-time nudge, emitted on STDERR when output auto-switched to
/// TOON because stdout is piped. Kept as a constant so the test asserts the
/// exact text the user sees.
// trace:BUG-1813 | ai:claude
pub const TOON_SWITCH_HINT: &str =
    "note: stdout is not a terminal — aida switched to compact TOON output. \
     Pin a format with `--format human|toon|json` or AIDA_OUTPUT_FORMAT to keep scripts stable.";

/// File name of the once-marker that silences [`TOON_SWITCH_HINT`] after its
/// first appearance; lives under a project's `.aida/` or, outside a project,
/// under `~/.aida/`.
// trace:BUG-1813 | ai:claude
pub const TOON_SWITCH_HINT_MARKER: &str = ".format-hint-shown";

// ── TASK-964: TOON agent-output renderers (AXI #1 token-efficient output +
// #2 minimal default schemas). These run ONLY in agent mode; the human emoji /
// table path is left byte-identical. Each command declares a minimal default
// field set; `--fields` expands `aida list`. trace:TASK-964

/// The minimal default column schema for `aida list` in agent mode (AXI #2).
/// Five fields cover the agent's routine memory-lane need — id / title /
/// status / type / modified_at — instead of the human table's
/// id/origin/type/status/title/tags/glyph spread. `--fields` widens it.
// trace:TASK-964
// trace:TASK-1215 | ai:codex
pub const TOON_LIST_DEFAULT_FIELDS: &[&str] = &["id", "title", "status", "type", "modified_at"];

/// The list fields the agent-mode `--fields` selector understands, mapped onto
/// the cache summary plus the work-routing axis. Unknown names are rejected with
/// this list in the error so the agent can self-correct.
// trace:TASK-964
pub const TOON_LIST_KNOWN_FIELDS: &[&str] = &[
    "id",
    // trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude — local ID
    // beside the agreed one, or `(agreed id pending)`.
    "local_id",
    "title",
    "status",
    "type",
    "priority",
    "feature",
    "owner",
    "assignee",
    "tags",
    "heft",
    // trace:TASK-1215 | ai:codex — freshness signal for authored lane memory.
    "modified_at",
    // trace:TASK-2205 | ai:claude — creation time; see TOON_LIST_FIELD_ALIASES.
    "opened",
    // trace:FR-283 | ai:claude — the optional numeric weight/score.
    "weight",
    "queued",
    "in_flight",
    "blocked",
    // trace:STORY-776 | ai:claude — the advisor's dispatch classification.
    "mode",
    // trace:STORY-634 | ai:claude — the multi-repo repo/component dimension.
    "origin",
    "deferred_reason",
    // trace:STORY-1653 | ai:claude — derived driver of an in-progress spec.
    "driver",
    "driver_kind",
    // trace:TASK-1951 trace:TASK-2003 | ai:claude — NeedsAttention review lens.
    "review_lens",
];

/// Accepted alternate spellings for `--fields` entries, as `(alias, field)`.
/// [`toon_list_fields`] rewrites each alias to its canonical field, and
/// [`list_fields_help`] names them, so help and parser share one table.
// trace:TASK-2205 | ai:claude
pub const TOON_LIST_FIELD_ALIASES: &[(&str, &str)] = &[
    ("req_type", "type"),
    ("created_at", "opened"),
    ("created", "opened"),
];

/// Render a numeric weight/score for display: integral values drop the
/// fractional part (`42`, not `42.0`); everything else uses the shortest
/// round-trip float formatting (`0.75`).
// trace:FR-283 | ai:claude
pub fn format_weight(w: f64) -> String {
    if w.fract() == 0.0 && w.abs() < 1e15 {
        format!("{}", w as i64)
    } else {
        format!("{}", w)
    }
}

/// Normalize a cache status string (`InProgress`, `in-progress`, `NeedsAttention`,
/// …) to a stable lowercase-hyphen token for agent output, so the TOON value is
/// uniform regardless of how the cache spelled it.
// trace:TASK-964
pub fn toon_status_token(status: &str) -> String {
    let s = status.trim();
    match s.to_ascii_lowercase().replace([' ', '_'], "-").as_str() {
        "inprogress" | "in-progress" => "in-progress".to_string(),
        "needsattention" | "needs-attention" => "needs-attention".to_string(),
        other => other.to_string(),
    }
}

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
pub fn agent_output_mode_quiet() -> bool {
    let pin = output_format_override();
    let env = std::env::var("AIDA_AGENT_OUTPUT").ok();
    let stdout_is_tty = std::io::stdout().is_terminal();
    resolve_agent_mode(pin, env.as_deref(), stdout_is_tty)
}

pub fn agent_output_mode(
    hint_context: impl FnOnce() -> (Option<std::path::PathBuf>, Option<std::path::PathBuf>),
) -> bool {
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
    // BUG-1813: the decision is the pure [`toon_switch_hint_applies`] so it is
    // unit-tested, and it also requires stderr to be a terminal — when a script
    // captures `2>&1` there is no human to nudge, only output to pollute.
    // trace:BUG-1813 | ai:claude
    let stderr_is_tty = std::io::stderr().is_terminal();
    if toon_switch_hint_applies(pin, env.as_deref(), stdout_is_tty, stderr_is_tty) {
        maybe_emit_toon_switch_hint(hint_context);
    }
    agent
}

/// Resolve + install the [`OUTPUT_FORMAT_OVERRIDE`] once. Precedence: the
/// `--format` flag wins; else `AIDA_OUTPUT_FORMAT` (human|toon|json,
/// case-insensitive); else `None` (fall back to the TTY-based default).
/// Idempotent — a later call is a no-op (the first install sticks).
// trace:STORY-764 | ai:claude
pub fn set_output_format_override(flag: Option<OutputFormat>) {
    let resolved = flag.or_else(output_format_from_env);
    let _ = OUTPUT_FORMAT_OVERRIDE.set(resolved);
}

/// Parse `AIDA_OUTPUT_FORMAT` into an [`OutputFormat`]. Unset or unrecognized
/// values yield `None` (defer to the TTY-based default rather than erroring, so
/// a stray value never breaks a command).
// trace:STORY-764 | ai:claude
pub fn output_format_from_env() -> Option<OutputFormat> {
    parse_output_format(std::env::var("AIDA_OUTPUT_FORMAT").ok().as_deref())
}

/// The installed explicit override, if any. `None` until
/// [`set_output_format_override`] runs (or if no pin was given).
// trace:STORY-764 | ai:claude
pub fn output_format_override() -> Option<OutputFormat> {
    OUTPUT_FORMAT_OVERRIDE.get().copied().flatten()
}

/// True when the explicit pin selects JSON. Commands that carry a `--json`
/// flag OR this in, so `--format json` / `AIDA_OUTPUT_FORMAT=json` reaches them
/// uniformly.
// trace:STORY-764 | ai:claude
pub fn output_format_is_json() -> bool {
    matches!(output_format_override(), Some(OutputFormat::Json))
}

/// Pure decision: does the STORY-764 auto-switch hint apply? True only on the
/// REAL auto-switch path — no `--format` / `AIDA_OUTPUT_FORMAT` pin, no
/// `AIDA_AGENT_OUTPUT`, stdout not a terminal (with no pin and no env the
/// agent verdict is exactly `!stdout_is_tty`) — AND stderr is a terminal.
/// A pinned format was the caller's explicit choice, so there is nothing to
/// hint about; a non-terminal stderr means no human is watching (cron, CI,
/// MCP, a `2>&1` capture), so the hint would only pollute captured output.
/// That keeps the hint for the case STORY-764 built it for: a human at a
/// terminal piping `aida … | grep` for the first time.
// trace:BUG-1813 | ai:claude
pub fn toon_switch_hint_applies(
    pin: Option<OutputFormat>,
    agent_env: Option<&str>,
    stdout_is_tty: bool,
    stderr_is_tty: bool,
) -> bool {
    pin.is_none() && agent_env.is_none() && !stdout_is_tty && stderr_is_tty
}

/// STORY-764: emit the first-pipe format hint at most once. Guarded first by a
/// process-once `OnceLock`, then by the once-marker from
/// [`toon_switch_hint_marker`] (project `.aida/`, else `~/.aida/`). Real
/// filesystem + real stderr; the testable logic is [`emit_toon_switch_hint_once`].
// trace:STORY-764 | ai:claude
// trace:BUG-1813 | ai:claude
pub fn maybe_emit_toon_switch_hint(
    hint_context: impl FnOnce() -> (Option<std::path::PathBuf>, Option<std::path::PathBuf>),
) {
    static PROCESS_ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if PROCESS_ONCE.set(()).is_err() {
        return; // already fired in this process
    }
    let (root, home) = hint_context();
    let marker = toon_switch_hint_marker(root.as_deref(), home.as_deref());
    let _ = emit_toon_switch_hint_once(marker.as_deref(), &mut std::io::stderr());
}

/// STORY-764: process-global explicit output-format override, installed exactly
/// once from the global `--format` flag (or `AIDA_OUTPUT_FORMAT`) right after
/// argv parsing. Everything that renders consults [`agent_output_mode`] /
/// [`output_format_is_json`], which read this first — so the pin is honored
/// uniformly without threading a format param through every handler.
// trace:STORY-764 | ai:claude
pub static OUTPUT_FORMAT_OVERRIDE: std::sync::OnceLock<Option<OutputFormat>> =
    std::sync::OnceLock::new();

// trace:TASK-3783 | ai:codex
pub fn resolve_agent_mode(
    pin: Option<OutputFormat>,
    agent_env: Option<&str>,
    stdout_is_tty: bool,
) -> bool {
    pin.map(|format| format != OutputFormat::Human)
        .unwrap_or_else(|| agent_output_mode_from(agent_env, stdout_is_tty))
}
