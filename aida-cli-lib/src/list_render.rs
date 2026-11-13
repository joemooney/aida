// trace:TASK-3428 | ai:codex
use crate::{
    status_display, toon_list_cell, Colorize, AGENT_LIST_DEFAULT_LIMIT, LIST_DEFAULT_SORT,
    LIST_SORT_VALUES,
};

/// TASK-972 (AXI #6): split a formatted anyhow error into the one-line summary
/// the agent error block shows plus an optional suggested next command. The
/// summary is the first line with any leading human `Error:` prefix stripped
/// (several `bail!` sites embed one, which would otherwise read `error: Error:
/// …` once nested under the TOON `error:` key). The suggestion reuses the rich
/// hints the not_found module and friends already embed — see
/// [`extract_aida_suggestion`] — so "where known" comes for free.
// trace:TASK-972
/// BUG-684: the soft-signpost line for a truly-EMPTY `aida list` (human path).
/// Returns `Some(line)` only when nothing is merely hidden behind a filter —
/// a fresh zero-spec repo — so the hint teaches the create move. When rows exist
/// but are filtered out (closed/archived/deferred), returns `None` because the
/// hidden-hints already point at `--all`. Pure, so the empty-state decision is
/// unit-testable without the surrounding render machinery.
// trace:BUG-684 | ai:claude
// BUG-781: the accepted-decision axis counts as "hidden behind a filter" too —
// a project whose only remaining specs are ratified ADRs is not a fresh repo,
// so it must not be told to file its first spec.
// trace:BUG-781 | ai:claude
pub(crate) fn empty_list_hint_line(
    closed_hidden: usize,
    archived_hidden: usize,
    deferred_hidden: usize,
    accepted_decisions_hidden: usize,
) -> Option<&'static str> {
    if closed_hidden == 0
        && archived_hidden == 0
        && deferred_hidden == 0
        && accepted_decisions_hidden == 0
    {
        Some("Nothing here yet — file your first spec: aida add --title \"...\"")
    } else {
        None
    }
}

/// BUG-783: build the `aida list` footer hint lines, in render order.
///
/// Two families live here and they are NOT the same thing:
///
/// * **Lens hints** — closed rows and accepted (terminal) decisions the default
///   OPEN lens drops. These explain why a spec the user KNOWS exists is absent
///   from the rows they just asked for, so they always print.
/// * **View-tier hints** — the archived and deferred tiers. On an explicit
///   open-work request these are noise: the operator asked for open work and
///   does not need reminding on every invocation that the other two tiers
///   exist. Suppressed unless `[list] show_hidden_hints = true` opts back in.
///
/// The tiers stay reachable either way — `--archived`, `--deferred` and `--all`
/// are unaffected (they clear the default lens entirely, so nothing is hidden
/// and no hint applies), and each prints its own row count.
///
/// Returns undecorated lines (2-space indented); the caller dims and prints.
/// Pure so the "default view is quiet" contract is unit-testable without a
/// cache DB or a terminal.
// trace:BUG-783 | ai:claude
pub(crate) fn list_hidden_hint_lines(
    show_view_tier_hints: bool,
    closed_hidden: usize,
    archived_hidden: usize,
    deferred_hidden: usize,
    accepted_decisions_hidden: usize,
) -> Vec<String> {
    let mut lines = Vec::new();
    // STORY-723: the open lens hides closed history — say so.
    if closed_hidden > 0 {
        lines.push(format!(
            "  ({closed_hidden} closed hidden — open lens; pass --all or `--status closed` to see them)"
        ));
    }
    // STORY-441 / STORY-584: the archived + deferred view tiers. Opt-in.
    if show_view_tier_hints {
        if archived_hidden > 0 {
            lines.push(format!(
                "  ({archived_hidden} archived hidden — pass --all or --archived to see them)"
            ));
        }
        if deferred_hidden > 0 {
            lines.push(format!(
                "  ({deferred_hidden} deferred hidden — pass --all or --deferred to see them)"
            ));
        }
    }
    // BUG-781: accepted (terminal) decisions are hidden by the open lens — say
    // so, and name the flag that brings them back.
    if accepted_decisions_hidden > 0 {
        lines.push(format!(
            "  ({accepted_decisions_hidden} accepted decision{} hidden — pass --all or --type decision to see them)",
            if accepted_decisions_hidden == 1 { "" } else { "s" }
        ));
    }
    lines
}

/// Pure decision for the TASK-970 `aida list` default row cap. Returns the cap
/// to apply when AGENT MODE should bound a bare `aida list`, else `None`. The
/// cap is scoped to the default human-readable table render: an explicit
/// `--limit`/`--all`, the machine shapes (`--short`/`--json`), the grouped
/// `--tree` view, and the human TTY path all bypass it.
// trace:TASK-970
///
/// STORY-1644: `cap` is the resolved agent cap ([`resolve_agent_list_limit`]);
/// `0` means unbounded, so no cap applies.
// trace:STORY-1644 | ai:claude
pub(crate) fn agent_list_default_cap(
    limit: Option<usize>,
    all: bool,
    short: bool,
    json: bool,
    tree: bool,
    agent_mode: bool,
    cap: usize,
) -> Option<usize> {
    if limit.is_none() && !all && !short && !json && !tree && agent_mode && cap > 0 {
        Some(cap)
    } else {
        None
    }
}

/// STORY-1644: resolve the agent-mode `aida list` row cap from the env value
/// (`AIDA_LIST_LIMIT`) and the `[list] agent_limit` config value. Env beats
/// config beats [`AGENT_LIST_DEFAULT_LIMIT`]; `0` = unbounded. A value that
/// isn't a non-negative integer is ignored (falls through to the next
/// source). The `--limit` / `--all` flags beat all of these and are applied
/// by the caller.
// trace:STORY-1644 | ai:claude
pub(crate) fn resolve_agent_list_limit(env: Option<&str>, config: Option<i64>) -> usize {
    if let Some(n) = env.and_then(|v| v.trim().parse::<usize>().ok()) {
        return n;
    }
    match config {
        Some(n) if n >= 0 => n as usize,
        _ => AGENT_LIST_DEFAULT_LIMIT,
    }
}

/// STORY-1644: resolve the `aida list` sort key. An explicit `--sort` always
/// wins; else a recognised `[list] default_sort`; else
/// [`LIST_DEFAULT_SORT`]. An unrecognised config value is ignored with a
/// returned warning so the caller can print it.
// trace:STORY-1644 | ai:claude
pub(crate) fn resolve_list_sort(
    flag: Option<&str>,
    config: Option<&str>,
) -> (String, Option<String>) {
    if let Some(f) = flag {
        return (f.to_string(), None);
    }
    match config.map(|c| c.trim().to_ascii_lowercase()) {
        Some(c) if LIST_SORT_VALUES.contains(&c.as_str()) || c == "centrality" => (c, None),
        Some(c) => (
            LIST_DEFAULT_SORT.to_string(),
            Some(format!(
                "warning: unknown [list] default_sort '{c}' in .aida/config.toml (expected one of: {}); using '{LIST_DEFAULT_SORT}'",
                LIST_SORT_VALUES.join(", ")
            )),
        ),
        None => (LIST_DEFAULT_SORT.to_string(), None),
    }
}

/// STORY-1644: the one-line truncation notice for the agent-mode cap. Says
/// what was cut, in what order, and how to see the rest — never silent.
/// e.g. `count: 100 of 142 open — showing first 100 by id; newest hidden:
/// --sort modified, or --limit 0`.
// trace:STORY-1644 | ai:claude
pub(crate) fn agent_list_truncation_header(
    shown: usize,
    total: usize,
    denom_label: &str,
    sort: &str,
) -> String {
    let (hidden, widen) = match sort {
        "id" => ("newest", "--sort modified, or --limit 0"),
        "modified" => ("least recently modified", "--limit 0"),
        "created" => ("oldest", "--limit 0"),
        "completed" => ("least recently completed", "--limit 0"),
        "heft" | "centrality" => ("least connected", "--limit 0"),
        "weight" => ("lightest", "--limit 0"),
        _ => ("rest", "--limit 0"),
    };
    format!(
        "count: {shown} of {total} {denom_label} — showing first {shown} by {sort}; {hidden} hidden: {widen}"
    )
}

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
pub(crate) const TOON_LIST_DEFAULT_FIELDS: &[&str] =
    &["id", "title", "status", "type", "modified_at"];

/// The list fields the agent-mode `--fields` selector understands, mapped onto
/// the cache summary plus the work-routing axis. Unknown names are rejected with
/// this list in the error so the agent can self-correct.
// trace:TASK-964
pub(crate) const TOON_LIST_KNOWN_FIELDS: &[&str] = &[
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
pub(crate) const TOON_LIST_FIELD_ALIASES: &[(&str, &str)] = &[
    ("req_type", "type"),
    ("created_at", "opened"),
    ("created", "opened"),
];

/// The `aida list --fields` help text, generated from
/// [`TOON_LIST_KNOWN_FIELDS`], [`TOON_LIST_FIELD_ALIASES`] and
/// [`TOON_LIST_DEFAULT_FIELDS`] so the documented set cannot drift from the
/// set the parser accepts.
// trace:TASK-2205 | ai:claude
pub(crate) fn list_fields_help() -> String {
    let aliases: Vec<String> = TOON_LIST_FIELD_ALIASES
        .iter()
        .map(|(alias, field)| format!("{alias} = {field}"))
        .collect();
    format!(
        "Select AND order the displayed columns, e.g. `--fields id,title,status,priority,tags` \
         or `--fields 'id title opened'`; entries are separated by commas and/or whitespace. \
         Renders exactly those columns in that order — on the human table AND the \
         token-efficient agent output (a real token win for agents). Valid fields: {}. \
         `opened` is the creation time. Aliases: {}. Default (no `--fields`) keeps the \
         standard columns; agent mode defaults to the minimal {} set",
        TOON_LIST_KNOWN_FIELDS.join(", "),
        aliases.join(", "),
        TOON_LIST_DEFAULT_FIELDS.join(",")
    )
}

/// The `aida search --fields` help text. Search parses `--fields` through the
/// same [`toon_list_fields`] as `aida list`, so its help is generated from the
/// same tables as [`list_fields_help`].
// trace:TASK-2238 | ai:claude
pub(crate) fn search_fields_help() -> String {
    let aliases: Vec<String> = TOON_LIST_FIELD_ALIASES
        .iter()
        .map(|(alias, field)| format!("{alias} = {field}"))
        .collect();
    format!(
        "Select AND order the displayed columns, e.g. `--fields id,title,status,priority,tags` \
         or `--fields 'id title opened'`; entries are separated by commas and/or whitespace. \
         Renders exactly those columns in that order. Valid fields: {}. `opened` is the \
         creation time. Aliases: {}. Search rows carry no work-routing state, so queued, \
         in_flight and blocked render false. Default keeps the standard layout. Mirrors \
         `aida list --fields`",
        TOON_LIST_KNOWN_FIELDS.join(", "),
        aliases.join(", ")
    )
}

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
pub(crate) fn toon_status_token(status: &str) -> String {
    let s = status.trim();
    match s.to_ascii_lowercase().replace([' ', '_'], "-").as_str() {
        "inprogress" | "in-progress" => "in-progress".to_string(),
        "needsattention" | "needs-attention" => "needs-attention".to_string(),
        other => other.to_string(),
    }
}

/// The column header label for a `--fields` field name. The same field
/// vocabulary [`toon_list_cell`] / [`toon_list_fields`] understand, mapped to a
/// human-friendly capitalized header for the `aida list --fields` table.
// trace:STORY-734 | ai:claude
pub(crate) fn list_field_header(field: &str) -> String {
    match field {
        "id" => "ID",
        "title" => "Title",
        "status" => "Status",
        "type" => "Type",
        "priority" => "Priority",
        "feature" => "Feature",
        "owner" => "Owner",
        "assignee" => "Assignee",
        "tags" => "Tags",
        "heft" => "Heft",
        // trace:TASK-1215 | ai:codex
        "modified_at" => "Modified",
        // trace:TASK-2205 | ai:claude
        "opened" => "Opened",
        // trace:FR-283 | ai:claude
        "weight" => "Weight",
        "queued" => "Queued",
        "in_flight" => "In-Flight",
        "blocked" => "Blocked",
        // trace:TASK-1951 trace:TASK-2003 | ai:claude
        "review_lens" => "Review",
        // Validated against the known set upstream, so this is unreachable in
        // practice; fall back to the raw name for robustness.
        other => return other.to_string(),
    }
    .to_string()
}

/// Build the lines of the human `aida list --fields <csv>` table: exactly the
/// requested columns, in the requested order, each left-aligned and sized to its
/// widest cell (header included). Cell values reuse [`toon_list_cell`] so the
/// human and agent surfaces share one field vocabulary; the `status` and `id`
/// columns are coloured AFTER padding so ANSI escapes don't break alignment, and
/// the trailing column is not padded. Returned as a `Vec<String>` (header,
/// divider, then one line per row) so the layout is unit-testable; the colour
/// helpers no-op when stdout isn't a TTY.
// trace:STORY-734 | ai:claude — plain `//` keeps the marker out of any doc/help.
pub(crate) fn build_list_fields_lines<F>(
    reqs: &[aida_core::RequirementSummary],
    fields: &[String],
    routing_of: F,
) -> Vec<String>
where
    F: Fn(&aida_core::RequirementSummary) -> (bool, bool, bool),
{
    build_list_fields_lines_with(reqs, fields, routing_of, |_, _| None)
}

/// [`build_list_fields_lines`] with `cell_of` answering the columns that need
/// state beyond the summary (the TASK-2003 `review_lens`); `None` falls back
/// to [`toon_list_cell`].
// trace:TASK-1951 trace:TASK-2003 | ai:claude
pub(crate) fn build_list_fields_lines_with<F, C>(
    reqs: &[aida_core::RequirementSummary],
    fields: &[String],
    routing_of: F,
    cell_of: C,
) -> Vec<String>
where
    F: Fn(&aida_core::RequirementSummary) -> (bool, bool, bool),
    C: Fn(&aida_core::RequirementSummary, &str) -> Option<String>,
{
    // Plain (uncoloured) cell text per row per column — width math runs on this.
    let plain: Vec<Vec<String>> = reqs
        .iter()
        .map(|r| {
            let routing = routing_of(r);
            fields
                .iter()
                .map(|f| cell_of(r, f).unwrap_or_else(|| toon_list_cell(r, routing, f)))
                .collect()
        })
        .collect();

    // Column width = widest of (header, every cell). The last column is left
    // unpadded so a long Title never trails whitespace.
    let widths: Vec<usize> = fields
        .iter()
        .enumerate()
        .map(|(c, f)| {
            let header = list_field_header(f).len();
            plain
                .iter()
                .map(|row| row[c].len())
                .max()
                .unwrap_or(0)
                .max(header)
        })
        .collect();
    let last = fields.len().saturating_sub(1);

    let mut lines = Vec::with_capacity(reqs.len() + 2);

    // Header row + divider.
    let mut header = String::new();
    for (c, f) in fields.iter().enumerate() {
        if c > 0 {
            header.push(' ');
        }
        let label = list_field_header(f);
        if c == last {
            header.push_str(&label);
        } else {
            header.push_str(&format!("{:<width$}", label, width = widths[c]));
        }
    }
    lines.push(header.bold().to_string());
    let rule: usize = widths.iter().sum::<usize>() + widths.len().saturating_sub(1);
    lines.push("─".repeat(rule));

    // Data rows: pad the plain cell to its column width, THEN colour status / id
    // so the ANSI escapes don't inflate the `{:<}` byte count. trace:STORY-734
    for (row, req) in plain.iter().zip(reqs.iter()) {
        let mut line = String::new();
        for (c, f) in fields.iter().enumerate() {
            if c > 0 {
                line.push(' ');
            }
            let padded = if c == last {
                row[c].clone()
            } else {
                format!("{:<width$}", row[c], width = widths[c])
            };
            let cell = match f.as_str() {
                // BUG-781: paint on the DISPLAY label, so an accepted decision
                // gets the terminal green rather than the un-started cyan.
                // trace:BUG-781 | ai:claude
                "status" => status_display::paint_status(
                    &padded,
                    status_display::display_status_for_type(&req.req_type, &req.status),
                )
                .to_string(),
                "id" => padded.bold().to_string(),
                _ => padded,
            };
            line.push_str(&cell);
        }
        lines.push(line);
    }
    lines
}

/// Render the body of `aida search --fields <csv>` for a non-empty result set,
/// branching on agent vs human surface the same way `aida list --fields` does.
/// In agent mode (`agent == true`) it's the lean TOON `specs[N]{...}` projection
/// — the ~2x token win the agent surface exists for, instead of the verbose
/// box-table an agent would otherwise pay for. On a human TTY it's the aligned
/// box-table shared with `aida list` via [`build_list_fields_lines`]. Both
/// surfaces resolve cells through [`toon_list_cell`] over the already-validated
/// `selected` field set, so the two surfaces never drift in field vocabulary.
/// Search rows carry no work-routing axis, so queued/in_flight/blocked render
/// false; `review_lens` answers the `review_lens` column as in `aida list`.
/// Returned as a `String` so the branch is unit-testable.
// trace:STORY-734 trace:BUG-668 | ai:claude — plain `//` keeps the marker out of any doc/help.
// trace:TASK-1951 trace:TASK-2003 | ai:claude
pub(crate) fn render_search_fields(
    results: &[aida_core::RequirementSummary],
    selected: &[String],
    agent: bool,
    review_lens: &std::collections::HashMap<uuid::Uuid, String>,
) -> String {
    let cell = |r: &aida_core::RequirementSummary, f: &str| {
        (f == "review_lens").then(|| review_lens.get(&r.id).cloned().unwrap_or_default())
    };
    if agent {
        let field_refs: Vec<&str> = selected.iter().map(String::as_str).collect();
        let rows: Vec<Vec<String>> = results
            .iter()
            .map(|r| {
                selected
                    .iter()
                    .map(|f| {
                        cell(r, f).unwrap_or_else(|| toon_list_cell(r, (false, false, false), f))
                    })
                    .collect()
            })
            .collect();
        let mut out = format!("count: {} results\n", results.len());
        out.push_str(&crate::toon::table_raw("specs", &field_refs, &rows));
        out
    } else {
        let mut lines =
            build_list_fields_lines_with(results, selected, |_| (false, false, false), cell);
        lines.push(format!("\n{} results", results.len()));
        lines.join("\n")
    }
}
