//! `aida history` — project event timeline decoded from the orphan branch.
//!
//! Walks `git log` on the `aida-store` orphan branch via shell-out,
//! parses each commit's YAML diff with `serde_yaml::Value`, and emits one
//! logical Event per change. Output is reverse-chronological so the most
//! recent activity is at the top, matching `git log` defaults.
//!
//! trace:FR-1-037 | ai:claude

use anyhow::{Context, Result};
use colored::Colorize;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use serde_yaml::Value;
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command as ProcessCommand;

/// Options threaded down from the CLI handler. Keeps the public function
/// signature stable when new filters get added.
#[derive(Debug, Clone)]
pub struct HistoryOpts {
    pub limit: usize,
    pub max_commits: usize,
    /// Whether `max_commits` came from an explicit `--max-commits` (vs the
    /// `(limit*5).max(50)` events-mode default computed when the flag was
    /// omitted). Gates the BUG-1617 "window ran out" notice: a caller who
    /// pinned the window on purpose already knows it's narrow, so the hint
    /// on how to widen it would be noise.
    // trace:BUG-1617 | ai:claude
    pub max_commits_explicit: bool,
    pub events_mode: bool,
    pub id_filter: Option<String>,
    pub type_filter: Option<String>,
    pub author_filter: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub status_changes_only: bool,
    /// TASK-507: `--shipped` — only transitions into Completed (merged to the
    /// default branch), from any prior status: the "did my ship register?"
    /// view, vs `--all`'s recency-blind archive dump. Implies events mode.
    // trace:TASK-507 | ai:claude
    // trace:BUG-1636 | ai:claude
    pub shipped_only: bool,
    /// `--to <status>`: keep status transitions whose new status is this
    /// one (canonical form from [`resolve_status_filter`]). Implies events
    /// mode. See [`event_kind_allowed`] for how it combines with the other
    /// event selectors.
    // trace:TASK-1512 | ai:claude
    pub to_status: Option<String>,
    /// `--from <status>`: keep status transitions leaving this status.
    /// Combined with `--to`, both ends must match.
    // trace:TASK-1512 | ai:claude
    pub from_status: Option<String>,
    /// `--opened` / `--created`: keep spec-creation (`added`) events, the
    /// specs filed in the window, whatever status they were filed at.
    // trace:TASK-1512 | ai:claude
    pub opened_only: bool,
    pub comments_only: bool,
    pub oneline: bool,
    /// Spec-IDs currently archived. The default `aida history` view hides
    /// rows whose spec_id is in this set. Empty when `--all` or `--archived`
    /// was passed (no hiding needed). trace:STORY-441 | ai:claude
    pub archived_specs: std::collections::HashSet<String>,
    /// When `Some`, only rows whose spec_id is in this set are shown.
    /// Used by `--archived` to narrow the view to the archive itself.
    /// trace:STORY-441 | ai:claude
    pub archived_only_specs: Option<std::collections::HashSet<String>>,
    /// Spec-IDs currently deferred (flag set OR `deferred:*`-tagged). The
    /// default `aida history` view hides rows whose spec_id is in this set.
    /// Empty when `--all` or `--deferred` was passed. trace:STORY-584 | ai:claude
    pub deferred_specs: std::collections::HashSet<String>,
    /// When `Some`, only rows whose spec_id is in this set are shown.
    /// Used by `--deferred` to narrow to the primed shelf. trace:STORY-584 | ai:claude
    pub deferred_only_specs: Option<std::collections::HashSet<String>>,
    /// STORY-737 (delight #4): hide stateless internal META rows (the 6
    /// AI-prompt templates `aida init` seeds) from the default activity view
    /// so a fresh project's one real spec isn't drowned — matching `aida list`
    /// and `aida status`, which already exclude them. The caller leaves this
    /// `false` when `--include-meta` or an explicit `--type meta` was passed.
    // trace:STORY-737 | ai:claude
    pub exclude_meta: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct CommitMeta {
    pub(crate) sha: String,
    pub(crate) iso_timestamp: String,
    /// Author email from `git log %ae`. We prefer the YAML's
    /// `last_modified_by` field at event-rendering time, but the git
    /// author is the fallback when the YAML doesn't have one.
    pub(crate) git_author: String,
}

/// TASK-507: is this event a ship — a status transition into Completed
/// (merged to the default branch)? The `--shipped` view keeps only these.
///
/// BUG-1636: any prior status counts, not just Done. The merge path now
/// writes `InProgress → Completed` directly, and older ships went through
/// `Done → Completed`; both are ships. A `Completed → Completed` no-op and a
/// reopen away from Completed are not. A reopen followed by a re-complete
/// does count: the second completion is a separate merge of reworked
/// content, so it is its own ship event. `--shipped` is therefore the same
/// as `--to completed`.
// trace:TASK-507 | ai:claude
// trace:BUG-1636 | ai:claude
pub(crate) fn is_ship_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::StatusChange { from, to }
            if to.eq_ignore_ascii_case("Completed") && !from.eq_ignore_ascii_case("Completed")
    )
}

/// `--status-changes` and `--comments` each narrow to "just this kind of
/// event." Applied as two independent hard (AND) filters, passing both at
/// once used to silently produce an always-empty result — no event is ever
/// both a status change AND a comment — an incoherence TASK-1480's "keep
/// --status-changes, --comments ... coherent" acceptance bar calls out.
/// Combine them with OR instead: with both set, either kind passes; with
/// neither set, everything passes (unchanged from before).
///
/// TASK-1512 adds two more selectors to the same union: `--opened` selects
/// spec-creation events, and `--to`/`--from` select status transitions.
/// `--to`/`--from` also refine every status transition the query keeps,
/// so `--status-changes --to approved` is just the approvals, while
/// `--opened --to approved` is the specs filed plus the approvals.
// trace:TASK-1480 | ai:claude
// trace:TASK-1512 | ai:claude
fn event_kind_allowed(kind: &EventKind, opts: &HistoryOpts) -> bool {
    let transition = has_transition_filter(opts);
    if !opts.status_changes_only && !opts.comments_only && !opts.opened_only && !transition {
        return true;
    }
    match kind {
        EventKind::StatusChange { from, to } => {
            (opts.status_changes_only || transition) && transition_matches(from, to, opts)
        }
        EventKind::CommentsAdded { .. } => opts.comments_only,
        EventKind::Added { .. } => opts.opened_only,
        _ => false,
    }
}

/// Whether `--to` or `--from` was given.
// trace:TASK-1512 | ai:claude
pub(crate) fn has_transition_filter(opts: &HistoryOpts) -> bool {
    opts.to_status.is_some() || opts.from_status.is_some()
}

/// A status spelling reduced to its letters, lowercased, so the stored
/// `In Progress` / `InProgress` / `in_progress` forms all compare equal.
// trace:TASK-1512 | ai:claude
fn status_key(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, ' ' | '-' | '_'))
        .flat_map(char::to_lowercase)
        .collect()
}

/// Does the transition `from → to` pass `--to`/`--from`? A transition must
/// change the status (a respelling such as `In Progress → InProgress` is
/// not one), so `--to completed` is exactly `--shipped`'s "a transition
/// into Completed from any other status".
// trace:TASK-1512 | ai:claude
fn transition_matches(from: &str, to: &str, opts: &HistoryOpts) -> bool {
    if !has_transition_filter(opts) {
        return true;
    }
    let (from_key, to_key) = (status_key(from), status_key(to));
    if from_key == to_key {
        return false;
    }
    let to_ok = opts
        .to_status
        .as_deref()
        .is_none_or(|want| status_key(want) == to_key);
    let from_ok = opts
        .from_status
        .as_deref()
        .is_none_or(|want| status_key(want) == from_key);
    to_ok && from_ok
}

/// Refuse `--from X --to X`: a transition always changes the status, so
/// the pair could never match anything. `from`/`to` are canonical names
/// from [`resolve_status_filter`]; the flag names are how the caller
/// spells them (`--from`/`--to` on the CLI, `from`/`to` over MCP). The
/// message starts with `invalid combination:` so MCP reports it as an
/// invalid argument.
// trace:TASK-1512 | ai:claude
pub(crate) fn validate_transition_pair(
    from: Option<&str>,
    to: Option<&str>,
    from_flag: &str,
    to_flag: &str,
) -> Result<()> {
    if let (Some(f), Some(t)) = (from, to) {
        if status_key(f) == status_key(t) {
            anyhow::bail!(
                "invalid combination: {from_flag} and {to_flag} are both `{t}`; a transition \
                 always changes the status, so nothing can match. Use {to_flag} alone for \
                 transitions into it, or {from_flag} alone for transitions out of it"
            );
        }
    }
    Ok(())
}

/// Resolve a `--to`/`--from` value to its canonical status name. Accepts
/// the spellings `aida edit --status` accepts (`in-progress`,
/// `in_progress`, `InProgress`, any case), plus `accepted` for Approved
/// (the ADR spelling). An unknown value is an error naming the valid set.
// trace:TASK-1512 | ai:claude
pub(crate) fn resolve_status_filter(flag: &str, raw: &str) -> Result<String> {
    if raw.trim().eq_ignore_ascii_case("accepted") {
        return Ok("Approved".to_string());
    }
    crate::validate_status_input(raw.trim())
        .map(str::to_string)
        .map_err(|_| {
            anyhow::anyhow!(
                "invalid status `{raw}` for {flag}: expected one of: {} (or `accepted`, \
                 which means approved)",
                crate::VALID_STATUS_INPUTS
            )
        })
}

/// Serializable so the history index can store each decoded event and
/// render it byte-identically to a fresh git walk. Any change to this
/// shape must bump `history_cache::HISTORY_DECODER_VERSION`.
// trace:TASK-1507 | ai:claude
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) enum EventKind {
    Added {
        title: String,
        req_type: String,
        priority: String,
    },
    Deleted {
        title: String,
    },
    StatusChange {
        from: String,
        to: String,
    },
    PriorityChange {
        from: String,
        to: String,
    },
    TitleChange {
        from: String,
        to: String,
    },
    DescriptionEdited,
    OwnerChange {
        from: String,
        to: String,
    },
    FeatureChange {
        from: String,
        to: String,
    },
    TypeChange {
        from: String,
        to: String,
    },
    TagsChange {
        added: Vec<String>,
        removed: Vec<String>,
    },
    CommentsAdded {
        count: usize,
        author: Option<String>,
    },
    RelationshipsChange {
        added: usize,
        removed: usize,
        /// BUG-1631: the edges that changed, so the feed can show
        /// `STORY-1 → child TASK-2` instead of only a count. Empty on an
        /// event decoded before edges were recorded.
        // trace:BUG-1631 | ai:claude
        #[serde(default)]
        edges: Vec<RelEdge>,
    },
}

/// One relationship edge added to or removed from a spec, as stored on the
/// spec's YAML (`rel_type` + the target's UUID). `target` is the target's
/// spec ID, filled in after the events are collected (see
/// [`resolve_edge_targets`]); the history index stores it unresolved.
// trace:BUG-1631 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct RelEdge {
    pub(crate) added: bool,
    pub(crate) rel_type: String,
    pub(crate) target_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) target: Option<String>,
}

impl RelEdge {
    /// The target as a reader should see it: its spec ID when known, else
    /// the first 8 characters of its UUID (a since-deleted target).
    // trace:BUG-1631 | ai:claude
    pub(crate) fn target_label(&self) -> String {
        match &self.target {
            Some(t) => t.clone(),
            None => self.target_id.chars().take(8).collect(),
        }
    }

    /// The edge from `source`'s side, e.g. `STORY-1 → child TASK-2`.
    /// Stored `rel_type` names the source's role toward the target
    /// (`Parent` on the parent), so `Parent`/`Child` read as the target's
    /// role: STORY-1 `Parent` TASK-2 means TASK-2 is STORY-1's child.
    // trace:BUG-1631 | ai:claude
    pub(crate) fn describe(&self, source: &str) -> String {
        format!(
            "{source} \u{2192} {} {}",
            rel_label(&self.rel_type),
            self.target_label()
        )
    }
}

/// Reader-facing label for a stored `rel_type`: `Parent` → `child`,
/// `Child` → `parent`, other variants kebab-cased (`BlockedBy` →
/// `blocked-by`), custom names unchanged.
// trace:BUG-1631 | ai:claude
fn rel_label(rel_type: &str) -> String {
    match rel_type {
        "Parent" => "child".to_string(),
        "Child" => "parent".to_string(),
        other => {
            let mut out = String::new();
            for (i, ch) in other.chars().enumerate() {
                if ch.is_ascii_uppercase() {
                    if i > 0 {
                        out.push('-');
                    }
                    out.push(ch.to_ascii_lowercase());
                } else {
                    out.push(ch);
                }
            }
            out
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Event {
    pub(crate) sha: String,
    pub(crate) timestamp: String,
    /// Resolved author — YAML's `last_modified_by` if present, else git
    /// committer email's local-part.
    pub(crate) author: String,
    pub(crate) spec_id: String,
    pub(crate) req_type: String,
    pub(crate) kind: EventKind,
}

/// Structured event record for MCP and other programmatic consumers.
/// It is derived from the same orphan-branch decoder used by `aida history`
/// so CLI and MCP history views cannot drift.
/// trace:TASK-538 | ai:codex
#[derive(Debug, Clone, Serialize)]
pub struct HistoryEventRecord {
    pub sha: String,
    pub timestamp: String,
    pub author: String,
    pub spec_id: String,
    pub req_type: String,
    pub kind: String,
    pub summary: String,
    pub detail: JsonValue,
    /// The spec ID again, under the short per-event row name (`id`) the
    /// JSON projection documents; same value as `spec_id`.
    // trace:BUG-1635 | ai:claude
    pub id: String,
    /// The event time again, under the short row name `ts`; same value as
    /// `timestamp`.
    // trace:BUG-1635 | ai:claude
    pub ts: String,
    /// Old value for a field transition (status, priority, title, owner,
    /// feature, type); `null` for any other kind of event.
    // trace:BUG-1635 | ai:claude
    pub from: Option<String>,
    /// New value for a field transition; `null` for any other kind.
    // trace:BUG-1635 | ai:claude
    pub to: Option<String>,
}

/// Whether a single-spec `aida history` call (`--id` or the positional
/// SPEC-ID alias, both resolved into `opts.id_filter` before this point)
/// renders the status-progression view rather than the full event trail.
/// `--full`/`events` (an explicit request for the complete trail) and
/// `--shipped` set `events_mode`, which wins.
///
/// BUG-1635: this no longer depends on whether the caller is a human at a
/// terminal. Agent/piped callers used to fall back to a one-row digest,
/// so they never saw the transitions a human saw; now every output format
/// renders the same progression (human prose, TOON rows, JSON rows).
// trace:TASK-1480 | ai:claude
// trace:BUG-1635 | ai:claude
pub(crate) fn single_spec_uses_progress_view(opts: &HistoryOpts) -> bool {
    opts.id_filter.is_some() && !opts.events_mode
}

/// Which flags switch `aida history` from the per-spec digest to the
/// per-event feed. Returns the `events_mode` the CLI stores in
/// [`HistoryOpts`].
///
/// `--full`/`events` and the event selectors (`--shipped`, `--to`,
/// `--from`, `--opened`, passed together as `event_selector`) always do.
/// Without a SPEC-ID, so do
/// the flags that only mean something per event: `--status-changes`,
/// `--comments`, `--oneline` and `--json` (the digest has one row per
/// spec, so honoring them there is impossible and ignoring them silently
/// answered a different question). With a SPEC-ID they narrow or restyle
/// the status-progression view instead, so they leave `events_mode` off.
// trace:BUG-1635 | ai:claude
// trace:TASK-1512 | ai:claude
pub(crate) fn resolve_events_mode(
    explicit_events: bool,
    single_spec: bool,
    event_selector: bool,
    status_changes: bool,
    comments: bool,
    oneline: bool,
    json: bool,
) -> bool {
    explicit_events
        || event_selector
        || (!single_spec && (status_changes || comments || oneline || json))
}

/// The default `--max-commits` window when the caller did not pin one.
/// A bare `--full`/`events` feed, or a bare multi-spec `--json`, walks
/// 5x `--limit` (at least 50): it decodes every commit, so it scans
/// shallow. Everything else walks 250: the digest touches each commit
/// once, and a narrowing filter (`--shipped`, `--to`, `--from`,
/// `--opened`, `--status-changes`, `--comments`, `--oneline`; the first
/// four arrive together as `event_selector`) or a single SPEC-ID needs
/// depth to find
/// anything. The window follows the mode, never the output format:
/// adding `--json` to a filtered query does not change it.
// trace:BUG-1635 | ai:claude
// trace:TASK-1512 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn default_max_commits(
    limit: usize,
    explicit_events: bool,
    single_spec: bool,
    json: bool,
    event_selector: bool,
    status_changes: bool,
    comments: bool,
    oneline: bool,
) -> usize {
    let filtered = event_selector || status_changes || comments || oneline;
    let shallow = explicit_events || (json && !single_spec && !filtered);
    if shallow {
        (limit * 5).max(50)
    } else {
        250
    }
}

/// How one `aida history` answer is rendered. Precedence: `--json`, then
/// `--oneline`, then TOON for agent/piped callers, else the human view.
// trace:BUG-1635 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryOutput {
    Human,
    Toon,
    Json,
    Oneline,
}

impl HistoryOutput {
    pub(crate) fn select(json: bool, oneline: bool, agent_mode: bool) -> Self {
        if json {
            HistoryOutput::Json
        } else if oneline {
            HistoryOutput::Oneline
        } else if agent_mode {
            HistoryOutput::Toon
        } else {
            HistoryOutput::Human
        }
    }
}

// trace:TASK-1502 | ai:claude
type ResolvedWindow = (
    HistoryOpts,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
);

/// Resolve `opts.since`/`opts.until` into concrete UTC instants up front,
/// before any git shelling. Accepts every form `queue_cmd::parse_since_arg_at`
/// does: a compact relative duration (`5h`, `7d`, `30m`, `2w`), the phrase
/// form (`24 hours ago`), an RFC3339 timestamp (zone honored exactly), a
/// zone-less ISO datetime (local time), or a bare ISO date (local midnight).
///
/// Reuses the same grammar `aida archive --older-than` and `aida queue
/// progress --since` use rather than adding another parser. Bounds are
/// resolved here, not left to `git log --since=`, because git's approxidate
/// parser misreads compact units (`git log --since=30m` is read as a date
/// on the 30th and matches nothing) and silently ignores unparseable values.
/// Returns a clone of `opts` with `since`/`until` rewritten to unambiguous
/// RFC3339 strings, so every downstream `git log` call works unmodified,
/// plus the resolved instants for display. Rejects `--since` later than
/// `--until`, since that window can never match anything. `now` and `tz`
/// are injectable so tests get deterministic output regardless of the wall
/// clock or system timezone; production passes `chrono::Local`.
// trace:TASK-1502 | ai:claude
fn resolve_history_window<Tz>(
    opts: &HistoryOpts,
    now: chrono::DateTime<chrono::Utc>,
    tz: &Tz,
) -> Result<ResolvedWindow>
where
    Tz: chrono::TimeZone,
    Tz::Offset: std::fmt::Display,
{
    let since_at = opts
        .since
        .as_deref()
        .map(|raw| parse_history_bound(raw, "--since", now, tz))
        .transpose()?;
    let until_at = opts
        .until
        .as_deref()
        .map(|raw| parse_history_bound(raw, "--until", now, tz))
        .transpose()?;
    validate_window_order(since_at, until_at, tz)?;
    let mut resolved = opts.clone();
    resolved.since = since_at.map(|d| d.to_rfc3339());
    resolved.until = until_at.map(|d| d.to_rfc3339());
    Ok((resolved, since_at, until_at))
}

/// Shared by `resolve_history_window` and `history_kind_report`'s `--kind`
/// path so the reversed-order check (and its error wording) lives in one
/// place instead of being duplicated per `aida history` sub-mode.
// trace:TASK-1502 | ai:claude
pub(crate) fn validate_window_order<Tz>(
    since_at: Option<chrono::DateTime<chrono::Utc>>,
    until_at: Option<chrono::DateTime<chrono::Utc>>,
    tz: &Tz,
) -> Result<()>
where
    Tz: chrono::TimeZone,
    Tz::Offset: std::fmt::Display,
{
    if let (Some(s), Some(u)) = (since_at, until_at) {
        if s > u {
            anyhow::bail!(
                "--since resolves to {} which is later than --until's {} — \
                 that window can never match anything; swap the bounds or \
                 widen one",
                fmt_window_ts(s, tz),
                fmt_window_ts(u, tz),
            );
        }
    }
    Ok(())
}

/// One time bound for `aida history`, with an error message labeled for the
/// flag that actually produced it (`parse_since_arg_at`'s own message always
/// says `--since`, which would misname a bad `--until`). A DST gap/overlap
/// error is kept verbatim so the caller learns why the value was refused.
// trace:TASK-1502 | ai:claude
pub(crate) fn parse_history_bound<Tz: chrono::TimeZone>(
    raw: &str,
    flag: &str,
    now: chrono::DateTime<chrono::Utc>,
    tz: &Tz,
) -> Result<chrono::DateTime<chrono::Utc>> {
    // trace:TASK-1509 | ai:claude — the relabeling now lives beside the
    // shared parser so every time-bound flag reports errors the same way.
    crate::queue_cmd::parse_time_bound_at(raw, flag, now, tz)
}

/// Renders `d` in timezone `tz` with an explicit numeric zone (`%z`) taken
/// from that instant's own offset, so the `Window:` line (and any error
/// quoting a bound) states the offset actually in effect on that date,
/// including across a DST change.
// trace:TASK-1502 | ai:claude
fn fmt_window_ts<Tz>(d: chrono::DateTime<chrono::Utc>, tz: &Tz) -> String
where
    Tz: chrono::TimeZone,
    Tz::Offset: std::fmt::Display,
{
    d.with_timezone(tz).format("%Y-%m-%d %H:%M %z").to_string()
}

/// The human-output "here's what I actually queried" line, unambiguous
/// even when the caller typed a relative duration or a bare date. `None`
/// when neither bound was given (nothing to show).
// trace:TASK-1502 | ai:claude
fn format_resolved_window<Tz>(
    since_at: Option<chrono::DateTime<chrono::Utc>>,
    until_at: Option<chrono::DateTime<chrono::Utc>>,
    tz: &Tz,
) -> Option<String>
where
    Tz: chrono::TimeZone,
    Tz::Offset: std::fmt::Display,
{
    match (since_at, until_at) {
        (None, None) => None,
        (Some(s), None) => Some(format!(
            "Window: since {} (open-ended)",
            fmt_window_ts(s, tz)
        )),
        (None, Some(u)) => Some(format!(
            "Window: until {} (unbounded start)",
            fmt_window_ts(u, tz)
        )),
        (Some(s), Some(u)) => Some(format!(
            "Window: {} \u{2192} {}",
            fmt_window_ts(s, tz),
            fmt_window_ts(u, tz)
        )),
    }
}

/// `json` selects the JSON projection of the events feed (`--json` /
/// `--format json`); the caller also sets `opts.events_mode` for it.
// trace:BUG-1631 | ai:claude
pub fn run(store_path: &Path, opts: &HistoryOpts, json: bool) -> Result<()> {
    if !store_path.is_dir() {
        anyhow::bail!(
            "Not a git-canonical AIDA store: {}\n\
             aida history walks the orphan branch's git log; the legacy SQLite\n\
             backend has no per-edit history surface.",
            store_path.display()
        );
    }

    // trace:TASK-1502 | ai:claude
    let (resolved_opts, since_at, until_at) =
        resolve_history_window(opts, chrono::Utc::now(), &chrono::Local)?;
    let opts = &resolved_opts;
    // BUG-1631: `--json` stdout is pure JSON, even at a human terminal.
    // trace:BUG-1631 | ai:claude
    if show_window_line(json, crate::agent_output_mode()) {
        if let Some(line) = format_resolved_window(since_at, until_at, &chrono::Local) {
            println!("{}", line.dimmed());
        }
    }

    // BUG-1635: one output selection for every view, so the progression
    // and the events feed render the same rows whether a human or a script
    // is reading. trace:BUG-1635 | ai:claude
    let output = HistoryOutput::select(json, opts.oneline, crate::agent_output_mode());

    // TASK-1480: a single spec (`--id` / the positional SPEC-ID alias)
    // without `--full`/`events` gets the status-progression view, in
    // every output format (BUG-1635). trace:TASK-1480 | ai:claude
    if single_spec_uses_progress_view(opts) {
        return run_single_spec_progress(store_path, opts, output);
    }

    if !opts.events_mode {
        return run_digest(store_path, opts);
    }

    let (filtered, hidden_archived, window_exhausted, source) =
        collect_filtered_events_sourced(store_path, opts)?;

    // BUG-1631: the JSON projection mirrors the MCP history tool's shape;
    // every record carries its `spec_id`.
    if output == HistoryOutput::Json {
        let payload = events_json(&filtered, window_exhausted, &source);
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    if filtered.is_empty() {
        eprintln!("{}", "(no events match the filter)".dimmed());
        // BUG-1636: `hidden_archived` counts every archived spec, not the
        // events this filter hid, so a count here pointed at archiving as
        // the likely cause of an empty result when it usually is not. Say
        // neutrally that the view excludes archived specs.
        // trace:BUG-1636 | ai:claude
        if hidden_archived > 0 {
            eprintln!(
                "{}",
                "(this view excludes archived specs; --all includes them, --archived shows only them)"
                    .dimmed()
            );
        }
        print_window_exhausted_notice(opts, window_exhausted, filtered.len());
        print_fallback_footer(&source);
        return Ok(());
    }

    print!("{}", render_events_feed(&filtered, opts, output));

    print_window_exhausted_notice(opts, window_exhausted, filtered.len());
    print_fallback_footer(&source);

    Ok(())
}

/// The events feed as text for every non-JSON output: one line per event
/// for `--oneline`, a TOON table (BUG-1631) for agent callers, else the
/// human blocks. Always newline-terminated.
// trace:BUG-1635 | ai:claude
pub(crate) fn render_events_feed(
    events: &[Event],
    opts: &HistoryOpts,
    output: HistoryOutput,
) -> String {
    match output {
        HistoryOutput::Oneline => render_oneline(events),
        HistoryOutput::Toon => format!("{}\n", render_events_toon(events)),
        HistoryOutput::Human => render_events_human(events, opts.id_filter.is_none()),
        // `run` prints JSON through `events_json` before it ever renders
        // text, so this arm is never reached.
        HistoryOutput::Json => unreachable!("render_events_feed: JSON is rendered by events_json"),
    }
}

/// One `format_oneline` line per event.
// trace:BUG-1635 | ai:claude
fn render_oneline(events: &[Event]) -> String {
    events
        .iter()
        .map(|e| format!("{}\n", format_oneline(e)))
        .collect()
}

/// Whether `run` prints the human `Window: …` line on stdout: never for
/// `--json` (its stdout must parse as JSON) nor for agent output.
// trace:BUG-1631 | ai:claude
pub(crate) fn show_window_line(json: bool, agent_mode: bool) -> bool {
    !json && !agent_mode
}

/// Where a history answer came from. MCP reports it as `source` (plus
/// `index_tip`); a human terminal gets a footer only on a fallback.
// trace:TASK-1508 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistorySource {
    /// Served from the history index, as of this store commit.
    Index { tip: String },
    /// Read by walking the store's git log. `fallback` is true when the
    /// index was switched on but could not answer (a miss or an error),
    /// false when it was switched off with `AIDA_HISTORY_CACHE=0`.
    GitWalk { fallback: bool },
}

impl HistorySource {
    /// The machine-readable `source` value.
    pub fn as_str(&self) -> &'static str {
        match self {
            HistorySource::Index { .. } => "history-cache",
            HistorySource::GitWalk { .. } => "git-walk",
        }
    }

    /// The store commit an index-served answer reflects; `None` for a walk.
    pub fn index_tip(&self) -> Option<&str> {
        match self {
            HistorySource::Index { tip } => Some(tip),
            HistorySource::GitWalk { .. } => None,
        }
    }
}

/// Records for MCP and other programmatic consumers, with provenance.
// trace:TASK-1508 | ai:claude
#[derive(Debug, Clone)]
pub struct EventRecords {
    pub events: Vec<HistoryEventRecord>,
    pub window_exhausted: bool,
    pub source: HistorySource,
}

/// The dim footer a human sees when the fast path could not answer and the
/// slower full read did. `None` when the index answered, when it was
/// switched off on purpose (then the slow read is expected, not news), or
/// for agent/piped output (`agent_mode`), which stays script-clean like the
/// other `(...)` hints; MCP reports `source` instead.
// trace:TASK-1508 | ai:claude
pub(crate) fn fallback_footer_text(
    source: &HistorySource,
    agent_mode: bool,
) -> Option<&'static str> {
    if agent_mode {
        return None;
    }
    match source {
        HistorySource::GitWalk { fallback: true } => Some(
            "(answered from the full change history, not the faster saved copy — this can take longer)",
        ),
        _ => None,
    }
}

/// Print [`fallback_footer_text`] on stderr (human terminal only).
// trace:TASK-1508 | ai:claude
fn print_fallback_footer(source: &HistorySource) {
    if let Some(msg) = fallback_footer_text(source, crate::agent_output_mode()) {
        eprintln!("{}", msg.dimmed());
    }
}

/// BUG-1617: `aida history events` bounds its `git log` walk to
/// `opts.max_commits` commits (by default `(limit*5).max(50)`). When that
/// window is used up before `--limit` events were found, the command used
/// to just... stop, with no indication fewer results came back than asked
/// for. Tell a human caller how to widen the walk — but only when it's
/// actually the DEFAULT window that ran out (an explicit `--max-commits`
/// means the caller already knows they narrowed it) and only when a human
/// is reading (agent/piped output is script-parsed; `window_exhausted` in
/// the JSON/MCP surface is the machine-readable equivalent there, and
/// injecting a prose line into that stream would just be noise to strip).
/// Prints to stderr like the other `(...)` hints in this module, so it
/// never pollutes stdout for a caller piping the event lines themselves.
// trace:BUG-1617 | ai:claude
fn print_window_exhausted_notice(opts: &HistoryOpts, window_exhausted: bool, shown: usize) {
    // Agent/piped output is script-parsed; `window_exhausted` in the
    // JSON/MCP surface is the machine-readable equivalent there, and a
    // prose line would just be noise a script has to filter back out.
    if crate::agent_output_mode() {
        return;
    }
    if let Some(msg) = window_exhausted_notice_text(opts, window_exhausted, shown) {
        eprintln!("{}", msg.dimmed());
    }
}

/// The notice text (if any) for [`print_window_exhausted_notice`] — split
/// out as a pure function so the "when do we print" logic is testable
/// without capturing stdout/stderr. `None` when the limit was met, no
/// events were exhausted, or the caller explicitly pinned `--max-commits`
/// (they already know they narrowed the walk).
// trace:BUG-1617 | ai:claude
fn window_exhausted_notice_text(
    opts: &HistoryOpts,
    window_exhausted: bool,
    shown: usize,
) -> Option<String> {
    if !window_exhausted || opts.max_commits_explicit {
        return None;
    }
    Some(format!(
        "(only {shown} of the requested {limit} found — the default {max_commits}-commit history window ran out first; widen it with --max-commits <N>, or bound the walk with --since/--until)",
        shown = shown,
        limit = opts.limit,
        max_commits = opts.max_commits,
    ))
}

/// TASK-1480: the default view for a single spec (`--id` / positional
/// SPEC-ID) without `--full`/`events` — status transitions (or, with
/// `--comments`, a comment timeline; the CLI layer already folded
/// "neither flag given" into `status_changes_only`) in chronological
/// order, each with a timestamp and old→new status. Reuses the exact same
/// decoder and filters as the full events feed (`collect_filtered_events`)
/// so this view can never drift from it — it only narrows *which* event
/// kinds show and reads oldest-first (a progression reads forward in
/// time), the opposite of the full trail's git-log-style newest-first.
///
/// BUG-1635: every output format renders the same rows — the human view,
/// a TOON table (one row per transition) and a JSON document.
// trace:TASK-1480 | ai:claude
// trace:BUG-1635 | ai:claude
fn run_single_spec_progress(
    store_path: &Path,
    opts: &HistoryOpts,
    output: HistoryOutput,
) -> Result<()> {
    let id = opts
        .id_filter
        .as_deref()
        .expect("run_single_spec_progress requires opts.id_filter");

    let (mut filtered, _hidden_archived, window_exhausted, source) =
        collect_filtered_events_sourced(store_path, opts)?;

    if filtered.is_empty() {
        // Errors when the id never had any recorded history at all.
        ensure_spec_has_history(store_path, opts, id)?;
    }

    // Oldest-first: see the doc comment above.
    filtered.reverse();

    let (current_status, title) = current_snapshot(store_path, id);
    let view = Progression {
        id,
        title,
        current_status,
        events: &filtered,
    };
    match output {
        HistoryOutput::Json => {
            let payload = progression_json(&view, opts, window_exhausted, &source);
            println!("{}", serde_json::to_string_pretty(&payload)?);
            return Ok(());
        }
        _ if filtered.is_empty() => {
            if output == HistoryOutput::Toon {
                print!("{}", render_progression_toon(&view, opts));
            } else {
                report_empty_single_spec(id);
            }
        }
        HistoryOutput::Oneline => print!("{}", render_oneline(&filtered)),
        HistoryOutput::Toon => print!("{}", render_progression_toon(&view, opts)),
        HistoryOutput::Human => print!("{}", render_progression_human(&view, opts)),
    }
    print_fallback_footer(&source);
    Ok(())
}

/// One spec's progression, oldest-first, with the header facts every
/// rendering shows.
// trace:BUG-1635 | ai:claude
pub(crate) struct Progression<'a> {
    pub(crate) id: &'a str,
    /// Current title; empty when the spec is no longer in the store.
    pub(crate) title: String,
    /// Current status; `None` when the spec is no longer in the store.
    pub(crate) current_status: Option<String>,
    pub(crate) events: &'a [Event],
}

/// The progression's heading and its machine `view` name, by which event
/// kinds the caller asked for.
// trace:BUG-1635 | ai:claude
fn progression_label(opts: &HistoryOpts) -> (&'static str, &'static str) {
    match (opts.status_changes_only, opts.comments_only) {
        (true, true) => (
            "Status + comment timeline",
            "history-status-comment-timeline",
        ),
        (_, true) => ("Comment timeline", "history-comment-timeline"),
        _ => ("Status progression", "history-progression"),
    }
}

/// The human progression view: header, one line per event, count footer.
// trace:TASK-1480 | ai:claude
// trace:BUG-1635 | ai:claude
pub(crate) fn render_progression_human(view: &Progression<'_>, opts: &HistoryOpts) -> String {
    let header_title = if view.title.is_empty() {
        String::new()
    } else {
        format!(" — {}", view.title)
    };
    let header_status = match &view.current_status {
        Some(s) => format!("  [current: {}]", s.green()),
        None => "  [not currently in the store]".dimmed().to_string(),
    };
    let mut out = format!("{}{}{}\n", view.id.bold(), header_title, header_status);
    let (label, _) = progression_label(opts);
    out.push_str(&format!("{} (oldest → newest):\n\n", label.dimmed()));
    for e in view.events {
        out.push_str(&format!(
            "  {}  {}  {}\n",
            e.timestamp.dimmed(),
            format_event_body(e),
            format!("(by {})", e.author).dimmed()
        ));
    }
    out.push_str(&format!(
        "\n{} event(s) shown. Pass --full for the complete edit/comment trail{}.\n",
        view.events.len(),
        if opts.comments_only {
            ""
        } else {
            ", or --comments for comment history"
        }
    ));
    out
}

/// The TOON progression view for agent/piped callers: the same header
/// facts and one row per event as the human view.
// trace:BUG-1635 | ai:claude
pub(crate) fn render_progression_toon(view: &Progression<'_>, opts: &HistoryOpts) -> String {
    let (_, view_name) = progression_label(opts);
    let rows: Vec<Vec<String>> = view
        .events
        .iter()
        .map(|e| {
            let r = event_record(e);
            vec![
                r.sha.chars().take(8).collect(),
                r.ts,
                r.author,
                r.id,
                r.kind,
                r.from.unwrap_or_default(),
                r.to.unwrap_or_default(),
                r.summary,
            ]
        })
        .collect();
    let mut out = format!(
        "view: {view_name}\n{}\n{}\n{}\norder: oldest-first\ncount: {}\n",
        crate::toon::scalar("id", view.id),
        crate::toon::scalar("title", &view.title),
        crate::toon::scalar("current", view.current_status.as_deref().unwrap_or("")),
        rows.len()
    );
    out.push_str(&crate::toon::table_raw(
        "events",
        &[
            "sha", "when", "author", "id", "kind", "from", "to", "summary",
        ],
        &rows,
    ));
    out.push('\n');
    out
}

/// The JSON progression document: the events-feed shape (per-event rows
/// with `id`, `ts`, `author`, `kind`, `from`, `to`, `summary`) plus the
/// header facts and `order: "oldest-first"`.
// trace:BUG-1635 | ai:claude
pub(crate) fn progression_json(
    view: &Progression<'_>,
    opts: &HistoryOpts,
    window_exhausted: bool,
    source: &HistorySource,
) -> JsonValue {
    let (_, view_name) = progression_label(opts);
    let mut doc = events_json(view.events, window_exhausted, source);
    if let Some(obj) = doc.as_object_mut() {
        obj.insert("view".into(), json!(view_name));
        obj.insert("id".into(), json!(view.id));
        obj.insert(
            "title".into(),
            if view.title.is_empty() {
                JsonValue::Null
            } else {
                json!(view.title)
            },
        );
        obj.insert("current_status".into(), json!(view.current_status));
        obj.insert("order".into(), json!("oldest-first"));
    }
    doc
}

/// TASK-1480: an id that never had any recorded history at all is an
/// "invalid ID gets a clear error" case, not a quiet empty view. The
/// re-check lifts every kind/date narrowing but keeps the id's pathspec
/// scope, so it's still a cheap, bounded git-log walk — not a full-store
/// scan.
// trace:TASK-1480 | ai:claude
fn ensure_spec_has_history(store_path: &Path, opts: &HistoryOpts, id: &str) -> Result<()> {
    let mut probe = opts.clone();
    probe.status_changes_only = false;
    probe.comments_only = false;
    probe.shipped_only = false;
    // trace:TASK-1512 | ai:claude
    probe.to_status = None;
    probe.from_status = None;
    probe.opened_only = false;
    probe.since = None;
    probe.until = None;
    let (any, _, _) = collect_filtered_events(store_path, &probe)?;
    if any.is_empty() {
        return Err(crate::not_found::requirement_not_found_in_loaded_store(id));
    }
    Ok(())
}

/// TASK-1480: the human "nothing here" hint for a real spec that is simply
/// quiet in this view.
// trace:TASK-1480 | ai:claude
fn report_empty_single_spec(id: &str) {
    eprintln!(
        "{}",
        format!("(no matching history for {id} in this view)").dimmed()
    );
    eprintln!(
        "{}",
        "(this spec has other recorded history — try --full for the complete trail)".dimmed()
    );
}

/// Best-effort "what does this spec look like right now" read for the
/// progression view's header — current status + title, straight from the
/// live YAML. `None`/empty when the spec has since been deleted (its
/// object file no longer exists); the progression view still renders fine
/// without it.
// trace:TASK-1480 | ai:claude
fn current_snapshot(store_path: &Path, spec_id: &str) -> (Option<String>, String) {
    let objects_root = store_path.join("objects");
    let Ok(yaml_path) = aida_core::object_store::object_path(&objects_root, spec_id) else {
        return (None, String::new());
    };
    let (status, title, _modified_at) = read_current(&yaml_path);
    if status == "(deleted)" {
        (None, String::new())
    } else {
        (Some(status), title)
    }
}

/// Collect structured event records using the same filters as
/// `aida history events`. Intended for MCP and other non-TTY consumers.
/// Returns the records, `window_exhausted` (see [`collect_filtered_events`];
/// MCP surfaces it as a top-level JSON field so a caller can tell "fewer
/// results" from "that's everything") and where the answer came from.
// trace:TASK-538 | ai:codex
// trace:BUG-1617 | ai:claude
// trace:TASK-1508 | ai:claude
pub fn collect_event_records(store_path: &Path, opts: &HistoryOpts) -> Result<EventRecords> {
    if !store_path.is_dir() {
        anyhow::bail!(
            "Not a git-canonical AIDA store: {}\n\
             aida history walks the orphan branch's git log; the legacy SQLite\n\
             backend has no per-edit history surface.",
            store_path.display()
        );
    }

    // TASK-1502: MCP's history tool shares this path, so it gets the same
    // relative-duration acceptance and since/until validation as the CLI.
    let (resolved_opts, _since_at, _until_at) =
        resolve_history_window(opts, chrono::Utc::now(), &chrono::Local)?;
    let (events, _, window_exhausted, source) =
        collect_filtered_events_sourced(store_path, &resolved_opts)?;
    Ok(EventRecords {
        events: events.iter().map(event_record).collect(),
        window_exhausted,
        source,
    })
}

/// Returns `(events, archived_hidden_count, window_exhausted)`.
/// `window_exhausted` is true when the commit walk found at least one more
/// commit beyond `max_commits` (real history continues past the cap — see
/// the over-fetch-by-one comment on the `git log` call below) AND fewer
/// than `opts.limit` matching events were found. History that is exactly
/// `max_commits` commits long, or shorter, is NOT exhaustion — there was
/// nothing more to find regardless of window size.
// trace:BUG-1617 | ai:claude
// trace:TASK-1507 | ai:claude
fn collect_filtered_events(
    store_path: &Path,
    opts: &HistoryOpts,
) -> Result<(Vec<Event>, usize, bool)> {
    let (events, hidden, exhausted, _source) = collect_filtered_events_sourced(store_path, opts)?;
    Ok((events, hidden, exhausted))
}

/// [`collect_filtered_events`] plus where the answer came from, with
/// relationship edge targets resolved to spec IDs (BUG-1631).
// trace:TASK-1508 | ai:claude
// trace:BUG-1631 | ai:claude
fn collect_filtered_events_sourced(
    store_path: &Path,
    opts: &HistoryOpts,
) -> Result<(Vec<Event>, usize, bool, HistorySource)> {
    let (mut events, hidden, exhausted, source) =
        collect_filtered_events_unresolved(store_path, opts)?;
    resolve_edge_targets(store_path, &mut events);
    Ok((events, hidden, exhausted, source))
}

/// Fill in each relationship edge's `target` spec ID. Runs after
/// filtering, and only when an edge needs it. The requirements cache's
/// uuid→spec_id rows answer first (one indexed lookup per target), but
/// only while the cache reflects the store's current HEAD: a spec's ID
/// can change for the same UUID (duplicate repair, renumbering, agreed-id
/// promotion), and a long-lived caller such as the MCP server can hold a
/// cache that has not caught up. With no readable cache, or a stale one,
/// the object files are scanned instead. A target the source cannot name (a
/// since-deleted spec) keeps `target: None` and renders as its short UUID.
// trace:BUG-1631 | ai:claude
pub(crate) fn resolve_edge_targets(store_path: &Path, events: &mut [Event]) {
    use std::collections::{HashMap, HashSet};

    let mut wanted: HashSet<String> = HashSet::new();
    for e in events.iter() {
        if let EventKind::RelationshipsChange { edges, .. } = &e.kind {
            for edge in edges {
                if edge.target.is_none() {
                    wanted.insert(edge.target_id.clone());
                }
            }
        }
    }
    if wanted.is_empty() {
        return;
    }
    let found: HashMap<String, String> = edge_targets_from_cache(store_path, &wanted)
        .unwrap_or_else(|| edge_targets_from_objects(store_path, &wanted));
    for e in events.iter_mut() {
        if let EventKind::RelationshipsChange { edges, .. } = &mut e.kind {
            for edge in edges.iter_mut() {
                if edge.target.is_none() {
                    edge.target = found.get(&edge.target_id).cloned();
                }
            }
        }
    }
}

/// uuid→spec_id from the project's requirements cache, opened read-only.
/// `None` when there is no cache, it cannot be read, or its recorded
/// `source_head_sha` is not the store's current HEAD (stale).
// trace:BUG-1631 | ai:claude
fn edge_targets_from_cache(
    store_path: &Path,
    wanted: &std::collections::HashSet<String>,
) -> Option<std::collections::HashMap<String, String>> {
    use rusqlite::{Connection, OpenFlags, OptionalExtension};
    let path = aida_core::CachedGitBackend::default_cache_path(store_path);
    if !path.is_file() {
        return None;
    }
    let conn = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let recorded: String = conn
        .query_row(
            "SELECT value FROM cache_meta WHERE key = 'source_head_sha'",
            [],
            |row| row.get(0),
        )
        .ok()?;
    let head = aida_core::git_ops::head_sha(store_path).ok()?;
    if recorded.trim().is_empty() || recorded.trim() != head.trim() {
        return None;
    }
    let mut stmt = conn
        .prepare("SELECT spec_id FROM requirements_cache WHERE id = ?1")
        .ok()?;
    let mut found = std::collections::HashMap::new();
    for uuid in wanted {
        let row: Option<Option<String>> = stmt
            .query_row([uuid], |row| row.get::<_, Option<String>>(0))
            .optional()
            .ok()?;
        if let Some(Some(spec_id)) = row {
            found.insert(uuid.clone(), spec_id);
        }
    }
    Some(found)
}

/// uuid→spec_id by reading the `id:` line near the top of each object
/// file: the fallback when no cache is readable.
// trace:BUG-1631 | ai:claude
fn edge_targets_from_objects(
    store_path: &Path,
    wanted: &std::collections::HashSet<String>,
) -> std::collections::HashMap<String, String> {
    use std::io::BufRead;
    let mut found = std::collections::HashMap::new();
    let Ok(objects) = aida_core::object_store::list_objects(&store_path.join("objects")) else {
        return found;
    };
    for (spec_id, path) in objects {
        if found.len() == wanted.len() {
            break;
        }
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        for line in std::io::BufReader::new(file).lines().take(8) {
            let Ok(line) = line else { break };
            if let Some(uuid) = line.strip_prefix("id:") {
                let uuid = uuid.trim().trim_matches(|c| c == '\'' || c == '"');
                if wanted.contains(uuid) {
                    found.insert(uuid.to_string(), spec_id.clone());
                }
                break;
            }
        }
    }
    found
}

/// The index-or-walk answer before edge targets are resolved.
// trace:TASK-1508 | ai:claude
fn collect_filtered_events_unresolved(
    store_path: &Path,
    opts: &HistoryOpts,
) -> Result<(Vec<Event>, usize, bool, HistorySource)> {
    // Switched off (`AIDA_HISTORY_CACHE=0`): the walk is the chosen path,
    // not a fallback, so no footer is owed.
    if !crate::history_cache::cache_enabled() {
        let (events, hidden, exhausted) = collect_filtered_events_git(store_path, opts)?;
        return Ok((
            events,
            hidden,
            exhausted,
            HistorySource::GitWalk { fallback: false },
        ));
    }
    // The history index answers only when it provably holds the whole
    // answer; on a miss or an error it returns `None` and the git walk
    // below (the canonical source) answers instead.
    if let Some(answer) = crate::history_cache::serve(store_path, opts) {
        return Ok((
            answer.events,
            answer.hidden_archived,
            answer.window_exhausted,
            HistorySource::Index { tip: answer.tip },
        ));
    }
    let (events, hidden, exhausted) = collect_filtered_events_git(store_path, opts)?;
    Ok((
        events,
        hidden,
        exhausted,
        HistorySource::GitWalk { fallback: true },
    ))
}

/// The git-walk implementation of [`collect_filtered_events`]: the
/// canonical answer, used whenever the history index cannot serve a query,
/// and the oracle the index's parity tests compare against.
// trace:BUG-1617 | ai:claude
// trace:TASK-1507 | ai:claude
pub(crate) fn collect_filtered_events_git(
    store_path: &Path,
    opts: &HistoryOpts,
) -> Result<(Vec<Event>, usize, bool)> {
    // Build a `git log` command bounded by --since / --until / --max_commits.
    // BUG-1617 review fix: over-fetch by one commit. `-n<max_commits>` alone
    // can't distinguish "history is exactly max_commits commits long" (not
    // exhausted — that's everything) from "there's more beyond the cap"
    // (exhausted) — both return exactly `max_commits` lines. Asking for one
    // extra and only ever DECODING the first `max_commits` (truncated right
    // after the log call, below) gives an unambiguous signal at the cost of
    // one extra `git log` line, not one extra `git show`.
    // trace:BUG-1617 | ai:claude
    let mut log_args: Vec<String> = vec![
        "log".into(),
        "--pretty=format:%H%x09%aI%x09%ae".into(),
        format!("-n{}", opts.max_commits.saturating_add(1)),
    ];
    if let Some(s) = &opts.since {
        log_args.push(format!("--since={}", s));
    }
    if let Some(u) = &opts.until {
        log_args.push(format!("--until={}", u));
    }

    // Path-scope the log walk to the one spec when `--id` is set. Without
    // this, git walks the entire orphan-branch history (~15.7k commits on the
    // AIDA store) and the spec filter happens in-memory below; the pathspec
    // makes git emit only the handful of commits that touched this spec's
    // YAML, so a targeted `--id` query is ~as fast as the spec's edit count
    // rather than the whole-store walk. The in-memory id filter below still
    // runs (it also resolves UUIDs/agreed-ids to the canonical spec_id), so
    // correctness is unchanged — this just shrinks the candidate set.
    // trace:TASK-1055
    let id_pathspec: Option<String> = opts
        .id_filter
        .as_deref()
        .and_then(|id| aida_core::object_store::relative_object_path(id).ok());
    if let Some(ref path) = id_pathspec {
        // `--full-history`: git's default path simplification follows only
        // one TREESAME parent of a merge, so it drops a merged side branch
        // whose changes did not survive the merge (an `-s ours` merge, for
        // one). Unfiltered `history events` shows those side commits, so
        // `--id` must too. With it, git walks every commit the unfiltered
        // walk does, in the same order, and prints the ones whose path
        // differs from at least one parent: a subsequence of the
        // unfiltered walk. A merge that kept one side's version is listed
        // but decodes to no events (its combined diff is empty), as in the
        // unfiltered walk.
        // trace:BUG-1620 | ai:claude
        log_args.push("--full-history".into());
        log_args.push("--".into());
        log_args.push(path.clone());
    }

    let log_output = run_git(store_path, &log_args)?;
    let mut commits: Vec<CommitMeta> = log_output.lines().filter_map(parse_log_line).collect();

    // More than `max_commits` came back only because we asked for one extra
    // as a probe — that extra commit means real history continues past the
    // cap. Exactly `max_commits` (or fewer) means the cap either wasn't hit
    // or landed exactly on the true end of history; either way, nothing is
    // being hidden. Truncate back down to `max_commits` before doing any of
    // the expensive per-commit `git show` work below — the probe commit
    // itself is never decoded into events. trace:BUG-1617 | ai:claude
    let commit_walk_capped = commits.len() > opts.max_commits;
    commits.truncate(opts.max_commits);

    let mut events: Vec<Event> = Vec::new();

    for commit in &commits {
        // Only commits that actually touch object YAML files contribute
        // events. The auto-commit "chore: update requirements store" still
        // gets walked because each individual file change inside it is
        // its own event.
        // `--no-renames`: git's default rename detection pairs a delete plus
        // a similar add into one `R<score>\told\tnew` line, which the
        // single-path parse below cannot read, so both events were dropped.
        // With renames off, git reports the separate D and A lines.
        // trace:BUG-1616 | ai:claude
        let mut show_args: Vec<String> = vec![
            "show".into(),
            "--name-status".into(),
            "--no-renames".into(),
            "--format=".into(),
            commit.sha.clone(),
        ];
        // When path-scoped (`--id`), restrict the file listing to the spec's
        // YAML so a bulk "chore: update requirements store" commit that
        // happens to touch this spec doesn't also decode every other file it
        // changed. trace:TASK-1055
        if let Some(ref path) = id_pathspec {
            show_args.push("--".into());
            show_args.push(path.clone());
        }
        let changed = run_git(store_path, &show_args)?;
        for line in changed.lines() {
            // Lines look like:  M\tobjects/FR/000/FR-1-011.yaml
            //                   A\tobjects/TASK/000/TASK-1-021.yaml
            //                   D\tobjects/EPIC/000/EPIC-9999.yaml
            let mut parts = line.splitn(2, '\t');
            let status = parts.next().unwrap_or("").trim();
            let path = parts.next().unwrap_or("").trim();
            if path.is_empty() || !path.starts_with("objects/") || !path.ends_with(".yaml") {
                continue;
            }

            // Pull before/after content via `git show <sha>^:path` and
            // `git show <sha>:path`. The `^` form fails on the first commit
            // — that's fine; we treat absence as "added".
            let after = git_show_blob(store_path, &commit.sha, path).ok();
            let before = git_show_blob(store_path, &format!("{}^", commit.sha), path).ok();

            decode_into_events(
                commit,
                status,
                path,
                before.as_deref(),
                after.as_deref(),
                &mut events,
            );
        }
    }

    // Apply filters not handled by `git log` itself.
    // STORY-441: archive filtering replaces the older terminal-status hide.
    // `archived_specs` (non-empty when default `--non-archived`) hides those
    // spec events; `archived_only_specs` (Some(...) when `--archived`)
    // narrows to only those. trace:STORY-441 | ai:claude
    let mut filtered: Vec<Event> = events
        .into_iter()
        .filter(|e| event_passes_filters(e, opts))
        .collect();

    // BUG-1617: `commit_walk_capped` (computed above, right after the log
    // call) already tells us whether real history continues past the cap.
    // Combine with "did we still fall short of --limit" — a limit-satisfying
    // result is never "exhausted" even if the walk was also capped.
    // trace:BUG-1617 | ai:claude
    let window_exhausted = commit_walk_capped && filtered.len() < opts.limit;

    filtered.truncate(opts.limit);

    Ok((filtered, opts.archived_specs.len(), window_exhausted))
}

/// Every in-memory filter `aida history events` applies after decoding,
/// shared by the git walk and the history index so the two paths cannot
/// drift. `--since`/`--until`/`--max-commits` are commit-level bounds and
/// are applied before decoding, not here.
// trace:TASK-1507 | ai:claude
pub(crate) fn event_passes_filters(e: &Event, opts: &HistoryOpts) -> bool {
    if let Some(id) = &opts.id_filter {
        if !e.spec_id.eq_ignore_ascii_case(id) {
            return false;
        }
    }
    if let Some(t) = &opts.type_filter {
        if !e.req_type.eq_ignore_ascii_case(t) {
            return false;
        }
    }
    // STORY-737 (delight #4): drop stateless META prompt-template rows from
    // the default view. trace:STORY-737 | ai:claude
    if opts.exclude_meta && e.req_type.eq_ignore_ascii_case("meta") {
        return false;
    }
    // A case-sensitive substring match, deliberately (see the history index,
    // which must match it exactly).
    if let Some(a) = &opts.author_filter {
        if !e.author.contains(a.as_str()) {
            return false;
        }
    }
    if !event_kind_allowed(&e.kind, opts) {
        return false;
    }
    // TASK-507: `--shipped` keeps only transitions into Completed — the
    // merge-to-default ship event, whatever the prior status (BUG-1636).
    // trace:TASK-507 | ai:claude
    if opts.shipped_only && !is_ship_event(&e.kind) {
        return false;
    }
    // STORY-441: archive filtering. `archived_specs` (non-empty for the
    // default non-archived view) hides those specs; `archived_only_specs`
    // (Some for `--archived`) narrows to them. trace:STORY-441 | ai:claude
    if opts.archived_specs.contains(&e.spec_id) {
        return false;
    }
    if let Some(only) = &opts.archived_only_specs {
        if !only.contains(&e.spec_id) {
            return false;
        }
    }
    // STORY-584: same shape on the defer axis. trace:STORY-584 | ai:claude
    if opts.deferred_specs.contains(&e.spec_id) {
        return false;
    }
    if let Some(only) = &opts.deferred_only_specs {
        if !only.contains(&e.spec_id) {
            return false;
        }
    }
    true
}

/// Digest mode (default): one row per recently-touched requirement, sorted
/// by last-touch time. Aimed at "what was I up to last session?" — answers
/// in a glance without decoding every individual diff.
///
/// Source of truth for "when did this actually change" is each YAML's own
/// `modified_at` field, NOT the git commit timestamp. Reason: the legacy
/// Storage façade still calls `GitBackend::save(&store)` for some paths,
/// which rewrites every YAML and produces "chore: update requirements
/// store" bulk-commits that touch dozens of files at once but don't bump
/// modified_at on any of them. Sorting by git ts would cluster every spec
/// into the most recent bulk commit's timestamp; sorting by modified_at
/// reflects real edits.
///
/// "C" column counts only commits whose subject explicitly mentions this
/// spec_id (`update X`, `add X`, `delete X`) so chore bulk-saves don't
/// inflate it.
///
/// Speed: one `git log --name-status` call + one filesystem read per
/// distinct requirement that surfaces. Sub-second on the AIDA store.
/// trace:FR-1-037 | ai:claude
fn run_digest(store_path: &Path, opts: &HistoryOpts) -> Result<()> {
    let (entries, archived_hidden, deferred_hidden) = build_digest_rows(store_path, opts)?;

    if entries.is_empty() {
        eprintln!("{}", "(no recent activity)".dimmed());
        if archived_hidden > 0 {
            eprintln!(
                "{}",
                format!(
                    "({archived_hidden} archived hidden — pass --all or --archived to see them)"
                )
                .dimmed()
            );
        }
        if deferred_hidden > 0 {
            eprintln!(
                "{}",
                format!(
                    "({deferred_hidden} deferred hidden — pass --all or --deferred to see them)"
                )
                .dimmed()
            );
        }
        return Ok(());
    }

    // Agent mode: token-efficient TOON, mirroring `aida ps` / `aida integrate`
    // so `aida history` (the default digest) gives agents flat scalars + a
    // uniform table instead of the human ID/TYPE/STATUS/WHEN column table.
    // `aida history events --json` remains the structured event-stream path.
    // trace:STORY-753 | ai:claude
    if crate::agent_output_mode() {
        println!("view: history");
        println!("count: {}", entries.len());
        let table: Vec<Vec<String>> = entries
            .iter()
            .map(|e| {
                let change = if e.had_delete {
                    "del"
                } else if e.had_add {
                    "add"
                } else {
                    "edit"
                };
                vec![
                    e.spec_id.clone(),
                    display_type_name(&e.req_type).to_string(),
                    e.status.clone(),
                    short_clock(&e.last_ts_iso),
                    change.to_string(),
                    e.title.clone(),
                ]
            })
            .collect();
        println!(
            "{}",
            crate::toon::table_raw(
                "history",
                &["id", "type", "status", "when", "change", "title"],
                &table
            )
        );
        return Ok(());
    }

    // Width-align spec_id column so the rest reads as a table.
    let id_w = entries.iter().map(|e| e.spec_id.len()).max().unwrap_or(8);
    let status_w = entries.iter().map(|e| e.status.len()).max().unwrap_or(10);
    // Time column needs to fit the widest rendered value (today's HH:MM
    // collapses to 5 chars; older "MM-DD HH:MM" is 11). Compute once.
    let time_w = entries
        .iter()
        .map(|e| short_clock(&e.last_ts_iso).len())
        .max()
        .unwrap_or(5);

    // Header: same column widths as rows. dimmed() applies after padding
    // so the color codes don't break alignment of the row data below.
    let header = format!(
        "{:<2} {:<id_w$}  {:<8}  {:<status_w$}  {:<time_w$}  {}",
        "",
        "ID",
        "TYPE",
        "STATUS",
        "WHEN",
        "TITLE",
        id_w = id_w,
        status_w = status_w,
        time_w = time_w,
    );
    println!("{}", header.dimmed());

    for e in &entries {
        // Inline marker per row: "+" for added in window, "−" for deleted,
        // "·" for edited. Keeps the C-column noise out and makes the
        // "what's new" answer immediately visible.
        let marker = if e.had_delete {
            "−".red().to_string()
        } else if e.had_add {
            "+".green().bold().to_string()
        } else {
            "·".dimmed().to_string()
        };

        let time = short_clock(&e.last_ts_iso);
        let title = shorten(&e.title, 70);

        // Pad PLAIN text first, THEN apply color — Rust's `{:<width$}`
        // counts bytes (including ANSI escape codes) so colorizing before
        // padding produces visibly misaligned columns.
        let id_padded = format!("{:<id_w$}", e.spec_id, id_w = id_w);
        let type_padded = format!("{:<8}", display_type_name(&e.req_type));
        let status_padded = format!("{:<status_w$}", e.status, status_w = status_w);
        let time_padded = format!("{:<time_w$}", time, time_w = time_w);

        println!(
            "{:<2} {}  {}  {}  {}  {}",
            marker,
            id_padded.bold(),
            type_padded,
            colorize_status(&status_padded),
            time_padded,
            title.dimmed(),
        );
    }

    if archived_hidden > 0 {
        println!(
            "{}",
            format!("  ({archived_hidden} archived hidden — pass --all or --archived to see them)")
                .dimmed()
        );
    }
    if deferred_hidden > 0 {
        println!(
            "{}",
            format!("  ({deferred_hidden} deferred hidden — pass --all or --deferred to see them)")
                .dimmed()
        );
    }

    Ok(())
}

/// Builds the sorted, filtered digest rows for `run_digest` (extracted so the
/// shard-resolution fix — BUG-1596 — can be exercised directly by tests
/// without capturing stdout). Returns (rows, archived_hidden_count,
/// deferred_hidden_count).
// trace:BUG-1596 | ai:claude
fn build_digest_rows(
    store_path: &Path,
    opts: &HistoryOpts,
) -> Result<(Vec<DigestRow>, usize, usize)> {
    let mut log_args: Vec<String> = vec![
        "log".into(),
        "--name-status".into(),
        // Report a delete plus a similar add as separate D and A lines, not
        // one `R<score>` line, so the digest's add/delete markers stay
        // accurate. trace:BUG-1616 | ai:claude
        "--no-renames".into(),
        "--pretty=format:%H%x09%aI%x09%ae%x09%s".into(),
        format!("-n{}", opts.max_commits),
    ];
    if let Some(s) = &opts.since {
        log_args.push(format!("--since={}", s));
    }
    if let Some(u) = &opts.until {
        log_args.push(format!("--until={}", u));
    }

    let log_output = run_git(store_path, &log_args)?;

    // Walk the streamed output line-by-line. Commit-metadata lines have
    // exactly three tabs (sha\tts\tauthor\tsubject); --name-status lines
    // have one (M\tpath / A\tpath / D\tpath). Blank lines separate commits.
    use std::collections::BTreeMap;
    let mut summaries: BTreeMap<String, DigestEntry> = BTreeMap::new();
    let mut current: Option<CommitInfo> = None;

    for line in log_output.lines() {
        if line.is_empty() {
            continue;
        }
        let tabs = line.bytes().filter(|b| *b == b'\t').count();
        if tabs >= 3 {
            // commit metadata
            let mut parts = line.splitn(4, '\t');
            let sha = parts.next().unwrap_or("").to_string();
            let ts = parts.next().unwrap_or("").to_string();
            let author = parts.next().unwrap_or("").to_string();
            let subject = parts.next().unwrap_or("").to_string();
            current = Some(CommitInfo {
                sha,
                ts,
                author,
                subject_spec: targeted_spec_id_from_subject(&subject),
            });
            continue;
        }

        // Otherwise treat as a name-status line. Skip non-object paths (the
        // orphan branch carries oplog.yaml and a few control files we don't
        // want surfacing here).
        let mut parts = line.split('\t');
        let status_letter = parts.next().unwrap_or("").to_string();
        let path = parts.next().unwrap_or("").to_string();
        if !path.starts_with("objects/") || !path.ends_with(".yaml") {
            continue;
        }
        let Some(commit) = current.as_ref() else {
            continue;
        };

        let spec_id = spec_id_from_path(&path);
        let req_type = req_type_from_path(&path);

        let entry = summaries
            .entry(spec_id.clone())
            .or_insert_with(|| DigestEntry {
                spec_id: spec_id.clone(),
                req_type,
                last_git_ts: commit.ts.clone(),
                last_author_email: commit.author.clone(),
                had_add: false,
                had_delete: false,
            });
        // Track whether this YAML was added or deleted somewhere in the
        // window so we can show "+ / − / ·" markers. Status letter is the
        // one from `git log --name-status`.
        match status_letter.chars().next() {
            Some('A') => entry.had_add = true,
            Some('D') => entry.had_delete = true,
            _ => {}
        }
    }

    // Read the current state for each surfaced spec_id from the worktree.
    // YAML's `modified_at` is the canonical timestamp — git commit ts is
    // only the fallback for deleted requirements (no YAML to read).
    let mut entries: Vec<DigestRow> = summaries
        .into_values()
        .map(|e| {
            // BUG-1596: resolve the canonical object path the same way the
            // store itself does (shard derived from the id's sequence
            // number), instead of hard-coding shard 000 — an id whose
            // sequence lives in shard 001+ was read from a path that never
            // existed and fell through to `read_current`'s "(deleted)"
            // fallback even though the YAML was present under its real
            // shard. trace:BUG-1596 | ai:claude
            let objects_root = store_path.join("objects");
            let yaml_path = aida_core::object_store::object_path(&objects_root, &e.spec_id)
                .unwrap_or_else(|_| {
                    objects_root
                        .join(&e.req_type)
                        .join("000")
                        .join(format!("{}.yaml", e.spec_id))
                });
            let (status, title, modified_at) = read_current(&yaml_path);
            // Prefer the YAML's modified_at (canonical), fall back to git ts.
            let last_ts_iso = modified_at.unwrap_or_else(|| e.last_git_ts.clone());
            DigestRow {
                spec_id: e.spec_id,
                req_type: e.req_type,
                status,
                title,
                last_ts_iso,
                last_author: pick_author_email(&e.last_author_email),
                had_add: e.had_add,
                had_delete: e.had_delete,
            }
        })
        .collect();

    // Apply filters that depend on the resolved row.
    let id_filter = opts.id_filter.clone();
    let type_filter = opts.type_filter.clone();
    let author_filter = opts.author_filter.clone();
    entries.retain(|e| {
        if let Some(ref id) = id_filter {
            if !e.spec_id.eq_ignore_ascii_case(id) {
                return false;
            }
        }
        if let Some(ref t) = type_filter {
            // Allow either the path-prefix form ("FR") or the human form
            // ("functional"). Path prefix is the canonical hit.
            let want = t.to_uppercase();
            let path_prefix = e.req_type.to_uppercase();
            let display = display_type_name(&e.req_type).to_uppercase();
            if path_prefix != want && display != want {
                return false;
            }
        }
        if let Some(ref a) = author_filter {
            if !e.last_author.contains(a) {
                return false;
            }
        }
        // STORY-737 (delight #4): hide stateless META prompt-template rows from
        // the default digest. `e.req_type` is the path-prefix form ("META"), so
        // a case-insensitive compare catches it. trace:STORY-737 | ai:claude
        if opts.exclude_meta && e.req_type.eq_ignore_ascii_case("meta") {
            return false;
        }
        true
    });

    // STORY-441: hide archived rows in the default view; count drops so we
    // can print a "(N archived hidden — pass --all …)" hint that mirrors
    // `aida list`. With `--archived`, narrow to the archive itself.
    // trace:STORY-441 | ai:claude
    let archived_hidden = {
        let before = entries.len();
        entries.retain(|e| !opts.archived_specs.contains(&e.spec_id));
        before - entries.len()
    };
    if let Some(only) = &opts.archived_only_specs {
        entries.retain(|e| only.contains(&e.spec_id));
    }

    // STORY-584: same shape on the defer axis. trace:STORY-584 | ai:claude
    let deferred_hidden = {
        let before = entries.len();
        entries.retain(|e| !opts.deferred_specs.contains(&e.spec_id));
        before - entries.len()
    };
    if let Some(only) = &opts.deferred_only_specs {
        entries.retain(|e| only.contains(&e.spec_id));
    }

    // Sort newest-first by ISO timestamp (string compare works for ISO 8601).
    entries.sort_by(|a, b| b.last_ts_iso.cmp(&a.last_ts_iso));
    entries.truncate(opts.limit);

    Ok((entries, archived_hidden, deferred_hidden))
}

/// In-flight commit metadata while parsing `git log --name-status`.
#[derive(Debug)]
struct CommitInfo {
    #[allow(dead_code)] // kept for symmetry / future per-row "last sha" column
    sha: String,
    ts: String,
    author: String,
    /// SPEC-ID explicitly named in the commit subject (e.g. "update FR-1-037").
    /// Currently unused after the C-column simplification but kept for a
    /// future "targeted edits only" filter — the parsing is essentially
    /// free and removing/restoring it churns the format string.
    #[allow(dead_code)]
    subject_spec: Option<String>,
}

#[derive(Debug)]
struct DigestEntry {
    spec_id: String,
    req_type: String,
    /// Newest git commit ts that touched this YAML (chore commits included).
    /// Used only as a fallback when the YAML's `modified_at` can't be read
    /// (e.g. the file was deleted).
    last_git_ts: String,
    last_author_email: String,
    had_add: bool,
    had_delete: bool,
}

#[derive(Debug)]
struct DigestRow {
    spec_id: String,
    req_type: String,
    status: String,
    title: String,
    last_ts_iso: String,
    last_author: String,
    had_add: bool,
    had_delete: bool,
}

/// Returns (status, title, modified_at). `modified_at` is None for
/// deleted requirements or when the field is missing — the caller then
/// falls back to the git commit timestamp.
fn read_current(yaml_path: &Path) -> (String, String, Option<String>) {
    let Ok(text) = std::fs::read_to_string(yaml_path) else {
        return ("(deleted)".to_string(), String::new(), None);
    };
    let v: Value = match serde_yaml::from_str(&text) {
        Ok(v) => v,
        Err(_) => return ("(parse-error)".to_string(), String::new(), None),
    };
    let status = effective(&v, "status", "custom_status").unwrap_or_default();
    let title = yaml_string(&v, "title").unwrap_or_default();
    let modified_at = yaml_string(&v, "modified_at");
    (status, title, modified_at)
}

/// Extract the SPEC-ID a commit's subject line explicitly targets, if any.
/// Subjects emitted by GitBackend look like "update FR-1-037" / "add
/// FR-1-037 — title" / "delete FR-1-037". The bulk-save path emits
/// "chore: update requirements store" — None for those, so they don't
/// inflate the per-spec commit count.
/// trace:FR-1-037 | ai:claude
fn targeted_spec_id_from_subject(subject: &str) -> Option<String> {
    let s = subject.trim();
    for prefix in ["update ", "add ", "delete "] {
        if let Some(rest) = s.strip_prefix(prefix) {
            // Take the first whitespace- or em-dash-separated token.
            let token: String = rest
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != '—')
                .collect();
            if !token.is_empty() && looks_like_spec_id(&token) {
                return Some(token);
            }
        }
    }
    None
}

fn looks_like_spec_id(s: &str) -> bool {
    // SPEC-IDs are letters then dash-and-digits, optionally with a node
    // segment (`FR-1-037`). Reject pure numbers and chore-style words.
    let mut chars = s.chars();
    if !chars
        .next()
        .map(|c| c.is_ascii_alphabetic())
        .unwrap_or(false)
    {
        return false;
    }
    s.contains('-') && s.chars().any(|c| c.is_ascii_digit())
}

fn pick_author_email(email: &str) -> String {
    email.split('@').next().unwrap_or(email).to_string()
}

/// HH:MM if today (in the user's local tz), else "MM-DD HH:MM". Always
/// converts the input ISO timestamp from UTC (or whatever offset it
/// carries) into the user's local time first — YAML modified_at fields
/// are stored as UTC (`Z` suffix), so showing the raw HH:MM made
/// timestamps look up to 12 hours in the future on west-of-UTC
/// machines.
/// trace:FR-1-037 | ai:claude
fn short_clock(iso: &str) -> String {
    use chrono::{DateTime, FixedOffset, Local};
    let Ok(dt_offset) = iso.parse::<DateTime<FixedOffset>>() else {
        return iso.to_string();
    };
    let dt_local = dt_offset.with_timezone(&Local);
    let today = Local::now().date_naive();
    if dt_local.date_naive() == today {
        dt_local.format("%H:%M").to_string()
    } else {
        dt_local.format("%m-%d %H:%M").to_string()
    }
}

fn display_type_name(s: &str) -> String {
    // Path prefixes are uppercase "FR", "EPIC", "TASK", "BUG", "STORY",
    // etc. Map to short display tokens that fit an 8-char column.
    match s {
        "FR" => "Func".into(),
        "NFR" => "NonFn".into(),
        "BUG" => "Bug".into(),
        "EPIC" => "Epic".into(),
        "STORY" => "Story".into(),
        "TASK" => "Task".into(),
        "SPIKE" => "Spike".into(),
        "SPRINT" => "Sprint".into(),
        "FOLDER" => "Folder".into(),
        "META" => "Meta".into(),
        "UR" => "User".into(),
        "SR" => "System".into(),
        "CR" => "ChgReq".into(),
        other => other.to_string(),
    }
}

/// Colourise an already-padded status cell for the `aida history` table.
/// TASK-269 unified the per-command palettes — this delegates to the shared
/// `status_display` module. `status` arrives column-padded; `paint_status`
/// normalises the trailing spaces away when picking the colour. No glyph is
/// added here: a 2-char glyph prefix would break the fixed-width column.
/// trace:TASK-269 | ai:claude
fn colorize_status(status: &str) -> String {
    crate::status_display::paint_status(status, status).to_string()
}

fn parse_log_line(line: &str) -> Option<CommitMeta> {
    let mut parts = line.split('\t');
    let sha = parts.next()?.to_string();
    let iso_timestamp = parts.next()?.to_string();
    let git_author = parts.next()?.to_string();
    Some(CommitMeta {
        sha,
        iso_timestamp,
        git_author,
    })
}

fn run_git(cwd: &Path, args: &[String]) -> Result<String> {
    let out = ProcessCommand::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git in {}", cwd.display()))?;
    if !out.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn git_show_blob(cwd: &Path, rev: &str, path: &str) -> Result<String> {
    run_git(cwd, &["show".into(), format!("{}:{}", rev, path)])
}

/// Decode one (commit, path) tuple into zero or more events and append
/// them to `out`. `status` is the git `--name-status` letter (A/M/D/...).
pub(crate) fn decode_into_events(
    commit: &CommitMeta,
    status: &str,
    path: &str,
    before: Option<&str>,
    after: Option<&str>,
    out: &mut Vec<Event>,
) {
    let req_type = req_type_from_path(path);

    let after_yaml = after.and_then(|s| serde_yaml::from_str::<Value>(s).ok());
    let before_yaml = before.and_then(|s| serde_yaml::from_str::<Value>(s).ok());

    let spec_id = after_yaml
        .as_ref()
        .or(before_yaml.as_ref())
        .and_then(|v| v.get("spec_id").and_then(Value::as_str).map(String::from))
        .unwrap_or_else(|| spec_id_from_path(path));

    let author = pick_author(&before_yaml, &after_yaml, commit);
    let mk = |kind: EventKind| Event {
        sha: commit.sha.clone(),
        timestamp: human_timestamp(&commit.iso_timestamp),
        author: author.clone(),
        spec_id: spec_id.clone(),
        req_type: req_type.clone(),
        kind,
    };

    match status.chars().next() {
        Some('A') => {
            if let Some(v) = &after_yaml {
                out.push(mk(EventKind::Added {
                    title: yaml_string(v, "title").unwrap_or_default(),
                    req_type: yaml_string(v, "req_type")
                        .or_else(|| yaml_string(v, "type"))
                        .unwrap_or_else(|| req_type.clone()),
                    priority: effective(v, "priority", "custom_priority").unwrap_or_default(),
                }));
            }
        }
        Some('D') => {
            if let Some(v) = &before_yaml {
                out.push(mk(EventKind::Deleted {
                    title: yaml_string(v, "title").unwrap_or_default(),
                }));
            }
        }
        _ => {
            // Modified — diff before vs after.
            if let (Some(a), Some(b)) = (&before_yaml, &after_yaml) {
                diff_modified(a, b, &mk, out);
            }
        }
    }
}

/// Compare scalar fields and emit a tagged event for each that changed.
/// Uses the `mk` closure so every emitted event inherits the commit
/// metadata without us re-cloning it inline.
fn diff_modified(
    before: &Value,
    after: &Value,
    mk: &dyn Fn(EventKind) -> Event,
    out: &mut Vec<Event>,
) {
    // Status — read effective (custom_status overrides) so we report the
    // user-visible value, not the underlying enum that might lag behind.
    let s_before = effective(before, "status", "custom_status");
    let s_after = effective(after, "status", "custom_status");
    if s_before != s_after {
        out.push(mk(EventKind::StatusChange {
            from: s_before.clone().unwrap_or_else(|| "—".into()),
            to: s_after.clone().unwrap_or_else(|| "—".into()),
        }));
    }

    let p_before = effective(before, "priority", "custom_priority");
    let p_after = effective(after, "priority", "custom_priority");
    if p_before != p_after {
        out.push(mk(EventKind::PriorityChange {
            from: p_before.unwrap_or_else(|| "—".into()),
            to: p_after.unwrap_or_else(|| "—".into()),
        }));
    }

    let t_before = yaml_string(before, "title");
    let t_after = yaml_string(after, "title");
    if t_before != t_after {
        out.push(mk(EventKind::TitleChange {
            from: t_before.unwrap_or_default(),
            to: t_after.unwrap_or_default(),
        }));
    }

    if yaml_string(before, "description") != yaml_string(after, "description") {
        out.push(mk(EventKind::DescriptionEdited));
    }

    if yaml_string(before, "owner") != yaml_string(after, "owner") {
        out.push(mk(EventKind::OwnerChange {
            from: yaml_string(before, "owner").unwrap_or_default(),
            to: yaml_string(after, "owner").unwrap_or_default(),
        }));
    }
    if yaml_string(before, "feature") != yaml_string(after, "feature") {
        out.push(mk(EventKind::FeatureChange {
            from: yaml_string(before, "feature").unwrap_or_default(),
            to: yaml_string(after, "feature").unwrap_or_default(),
        }));
    }

    let rt_before = yaml_string(before, "req_type").or_else(|| yaml_string(before, "type"));
    let rt_after = yaml_string(after, "req_type").or_else(|| yaml_string(after, "type"));
    if rt_before != rt_after {
        out.push(mk(EventKind::TypeChange {
            from: rt_before.unwrap_or_default(),
            to: rt_after.unwrap_or_default(),
        }));
    }

    // Tags — set diff so {a,b} → {b,c} reports +c, -a.
    let tags_before = yaml_string_set(before, "tags");
    let tags_after = yaml_string_set(after, "tags");
    if tags_before != tags_after {
        let added: Vec<String> = tags_after.difference(&tags_before).cloned().collect();
        let removed: Vec<String> = tags_before.difference(&tags_after).cloned().collect();
        out.push(mk(EventKind::TagsChange { added, removed }));
    }

    // Comments — count delta on the array. If the array grew, the new
    // tail entry's `author` field is surfaced.
    let cb = yaml_array_len(before, "comments");
    let ca = yaml_array_len(after, "comments");
    if ca > cb {
        let last_author = after
            .get("comments")
            .and_then(Value::as_sequence)
            .and_then(|s| s.last())
            .and_then(|v| v.get("author"))
            .and_then(Value::as_str)
            .map(String::from);
        out.push(mk(EventKind::CommentsAdded {
            count: ca - cb,
            author: last_author,
        }));
    }

    // BUG-1631: a multiset diff on (rel_type, target) so the event names
    // the edges that changed, not just how many. Every entry counts, a
    // duplicate once per copy and an unreadable one as `unknown`, so
    // [X, X] → [X, Y] is +Y and -X, and the totals always match the
    // entries that really changed.
    let edges_before = yaml_edge_counts(before);
    let edges_after = yaml_edge_counts(after);
    let mut edges: Vec<RelEdge> = Vec::new();
    for ((rel_type, target_id), n_after) in &edges_after {
        let n_before = edges_before
            .get(&(rel_type.clone(), target_id.clone()))
            .copied()
            .unwrap_or(0);
        for _ in n_before..*n_after {
            edges.push(RelEdge {
                added: true,
                rel_type: rel_type.clone(),
                target_id: target_id.clone(),
                target: None,
            });
        }
    }
    for ((rel_type, target_id), n_before) in &edges_before {
        let n_after = edges_after
            .get(&(rel_type.clone(), target_id.clone()))
            .copied()
            .unwrap_or(0);
        for _ in n_after..*n_before {
            edges.push(RelEdge {
                added: false,
                rel_type: rel_type.clone(),
                target_id: target_id.clone(),
                target: None,
            });
        }
    }
    if !edges.is_empty() {
        let added = edges.iter().filter(|e| e.added).count();
        let removed = edges.len() - added;
        out.push(mk(EventKind::RelationshipsChange {
            added,
            removed,
            edges,
        }));
    }
}

/// How many times each `(rel_type, target_id)` pair appears in a spec's
/// `relationships` array. Every entry counts: an unreadable `rel_type` keys
/// as `unknown`, a missing `target_id` as `?`.
// trace:BUG-1631 | ai:claude
fn yaml_edge_counts(v: &Value) -> std::collections::BTreeMap<(String, String), usize> {
    let mut counts = std::collections::BTreeMap::new();
    let Some(seq) = v.get("relationships").and_then(Value::as_sequence) else {
        return counts;
    };
    for key in seq.iter().map(|rel| {
        let rel_type = rel
            .get("rel_type")
            .map(rel_type_key)
            .unwrap_or_else(|| UNKNOWN_REL.to_string());
        let target = rel
            .get("target_id")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string();
        (rel_type, target)
    }) {
        *counts.entry(key).or_insert(0) += 1;
    }
    counts
}

/// Label for an edge whose `rel_type` cannot be read at all.
// trace:BUG-1631 | ai:claude
const UNKNOWN_REL: &str = "unknown";

/// One canonical key per relationship type, whatever its stored form:
/// a bare string (`Parent`, or a custom `implemented-by`), the legacy
/// tagged `!Custom name`, or the current mapping `{custom: name}`. Parsed
/// through `RelationshipType`'s own deserializer, so a store-wide rewrite
/// from one form to another yields the same key and no false edge events.
/// Standard types key as their stored variant name (`Parent`), custom ones
/// as their name.
// trace:BUG-1631 | ai:claude
fn rel_type_key(v: &Value) -> String {
    use aida_core::models::RelationshipType;
    match serde_yaml::from_value::<RelationshipType>(v.clone()) {
        Ok(RelationshipType::Custom(name)) => name,
        Ok(rt) => serde_json::to_value(&rt)
            .ok()
            .and_then(|j| j.as_str().map(str::to_string))
            .unwrap_or_else(|| rt.to_string()),
        Err(_) => UNKNOWN_REL.to_string(),
    }
}

fn pick_author(before: &Option<Value>, after: &Option<Value>, commit: &CommitMeta) -> String {
    // Prefer the YAML's last_modified_by if the field exists and is set.
    // Falls back to the git committer's local-part.
    let from_yaml = after
        .as_ref()
        .or(before.as_ref())
        .and_then(|v| yaml_string(v, "last_modified_by"));
    if let Some(name) = from_yaml {
        if !name.is_empty() {
            return name;
        }
    }
    commit
        .git_author
        .split('@')
        .next()
        .unwrap_or(&commit.git_author)
        .to_string()
}

/// "objects/FR/000/FR-1-011.yaml" → "FR-1-011"
fn spec_id_from_path(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("?")
        .to_string()
}

/// "objects/FR/000/FR-1-011.yaml" → "FR"
fn req_type_from_path(path: &str) -> String {
    path.strip_prefix("objects/")
        .and_then(|s| s.split('/').next())
        .unwrap_or("?")
        .to_string()
}

fn yaml_string(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(String::from)
}

/// Read `key` first; if absent or empty, fall back to `custom_key`. This
/// mirrors `effective_status()` / `effective_priority()` semantics on the
/// model side.
fn effective(v: &Value, key: &str, custom_key: &str) -> Option<String> {
    if let Some(custom) = yaml_string(v, custom_key) {
        if !custom.is_empty() {
            return Some(custom);
        }
    }
    yaml_string(v, key)
}

fn yaml_string_set(v: &Value, key: &str) -> BTreeSet<String> {
    v.get(key)
        .and_then(Value::as_sequence)
        .map(|seq| {
            seq.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn yaml_array_len(v: &Value, key: &str) -> usize {
    v.get(key)
        .and_then(Value::as_sequence)
        .map(|s| s.len())
        .unwrap_or(0)
}

/// "YYYY-MM-DD HH:MM" in the user's LOCAL timezone. Full RFC3339 is too
/// noisy for a feed view, but date+time without timezone is enough to
/// scan. Always converts to local — input ISO strings are usually UTC
/// (`Z` suffix on YAML modified_at) and showing them raw made timestamps
/// look up to 12 hours in the future on west-of-UTC machines.
/// trace:FR-1-037 | ai:claude
pub(crate) fn human_timestamp(iso: &str) -> String {
    use chrono::{DateTime, FixedOffset, Local};
    let Ok(dt_offset) = iso.parse::<DateTime<FixedOffset>>() else {
        return iso.to_string();
    };
    dt_offset
        .with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

/// Human block rendering of the events feed. Events are grouped under a
/// `commit <sha>` header. In a multi-spec feed (`multi_spec`) the header
/// also names the spec and a new block starts for each spec a commit
/// touched, so every event reads with its spec ID (BUG-1631). The
/// single-spec view keeps the ID implicit.
// trace:BUG-1631 | ai:claude
pub(crate) fn render_events_human(events: &[Event], multi_spec: bool) -> String {
    let mut out = String::new();
    let mut last: Option<(&str, &str)> = None;
    for e in events {
        let key = (
            e.sha.as_str(),
            if multi_spec { e.spec_id.as_str() } else { "" },
        );
        if last != Some(key) {
            if last.is_some() {
                out.push('\n');
            }
            let meta = format!("({}, by {})", e.timestamp, e.author).dimmed();
            let sha = &e.sha[..e.sha.len().min(8)];
            if multi_spec {
                out.push_str(&format!(
                    "{} {}  {}  {}\n",
                    "commit".yellow(),
                    sha.yellow(),
                    e.spec_id.bold(),
                    meta
                ));
            } else {
                out.push_str(&format!(
                    "{} {}  {}\n",
                    "commit".yellow(),
                    sha.yellow(),
                    meta
                ));
            }
            last = Some(key);
        }
        out.push_str(&format!("  {}\n", format_event_body(e)));
    }
    out
}

/// TOON rendering of the events feed for agent callers: one row per
/// event, each with its spec ID.
// trace:BUG-1631 | ai:claude
pub(crate) fn render_events_toon(events: &[Event]) -> String {
    let rows: Vec<Vec<String>> = events
        .iter()
        .map(|e| {
            let r = event_record(e);
            vec![
                r.sha.chars().take(8).collect(),
                r.timestamp,
                r.author,
                r.spec_id,
                r.kind,
                r.summary,
            ]
        })
        .collect();
    format!(
        "view: history-events\ncount: {}\n{}",
        rows.len(),
        crate::toon::table_raw(
            "events",
            &["sha", "when", "author", "id", "kind", "summary"],
            &rows
        )
    )
}

/// JSON projection of the events feed, the same shape the MCP history
/// tool returns.
// trace:BUG-1631 | ai:claude
pub(crate) fn events_json(
    events: &[Event],
    window_exhausted: bool,
    source: &HistorySource,
) -> JsonValue {
    let records: Vec<HistoryEventRecord> = events.iter().map(event_record).collect();
    records_json(&records, window_exhausted, source)
}

/// The history JSON document for already-built records; shared by the CLI
/// (`--json`) and the MCP history tool so the two cannot drift.
// trace:BUG-1631 | ai:claude
pub(crate) fn records_json(
    records: &[HistoryEventRecord],
    window_exhausted: bool,
    source: &HistorySource,
) -> JsonValue {
    json!({
        "count": records.len(),
        "events": records,
        "window_exhausted": window_exhausted,
        "source": source.as_str(),
        "index_tip": source.index_tip(),
    })
}

fn format_oneline(e: &Event) -> String {
    let head = format!(
        "{} {} {}",
        e.timestamp.dimmed(),
        e.author.cyan(),
        e.spec_id.bold()
    );
    format!("{}  {}", head, format_event_body(e))
}

pub(crate) fn event_record(e: &Event) -> HistoryEventRecord {
    let (kind, summary, detail) = event_kind_record(&e.kind, &e.spec_id);
    let (from, to) = kind_from_to(&e.kind);
    HistoryEventRecord {
        sha: e.sha.clone(),
        timestamp: e.timestamp.clone(),
        author: e.author.clone(),
        spec_id: e.spec_id.clone(),
        req_type: e.req_type.clone(),
        kind,
        summary,
        detail,
        id: e.spec_id.clone(),
        ts: e.timestamp.clone(),
        from,
        to,
    }
}

/// The old and new value of a field transition, for the per-event row's
/// `from`/`to`; `(None, None)` for events that are not a transition.
// trace:BUG-1635 | ai:claude
fn kind_from_to(kind: &EventKind) -> (Option<String>, Option<String>) {
    match kind {
        EventKind::StatusChange { from, to }
        | EventKind::PriorityChange { from, to }
        | EventKind::TitleChange { from, to }
        | EventKind::OwnerChange { from, to }
        | EventKind::FeatureChange { from, to }
        | EventKind::TypeChange { from, to } => (Some(from.clone()), Some(to.clone())),
        _ => (None, None),
    }
}

fn event_kind_record(kind: &EventKind, spec_id: &str) -> (String, String, JsonValue) {
    match kind {
        EventKind::Added {
            title,
            req_type,
            priority,
        } => (
            "added".to_string(),
            format!("added ({req_type}, {priority}) {}", shorten(title, 80)),
            json!({
                "title": title,
                "req_type": req_type,
                "priority": priority,
            }),
        ),
        EventKind::Deleted { title } => (
            "deleted".to_string(),
            format!("deleted {}", shorten(title, 80)),
            json!({ "title": title }),
        ),
        EventKind::StatusChange { from, to } => (
            "status_change".to_string(),
            format!("status: {from} -> {to}"),
            json!({ "from": from, "to": to }),
        ),
        EventKind::PriorityChange { from, to } => (
            "priority_change".to_string(),
            format!("priority: {from} -> {to}"),
            json!({ "from": from, "to": to }),
        ),
        EventKind::TitleChange { from, to } => (
            "title_change".to_string(),
            format!("title: {} -> {}", shorten(from, 40), shorten(to, 40)),
            json!({ "from": from, "to": to }),
        ),
        EventKind::DescriptionEdited => (
            "description_edited".to_string(),
            "description edited".to_string(),
            json!({}),
        ),
        EventKind::OwnerChange { from, to } => (
            "owner_change".to_string(),
            format!("owner: {} -> {}", maybe_dash(from), maybe_dash(to)),
            json!({ "from": from, "to": to }),
        ),
        EventKind::FeatureChange { from, to } => (
            "feature_change".to_string(),
            format!("feature: {} -> {}", maybe_dash(from), maybe_dash(to)),
            json!({ "from": from, "to": to }),
        ),
        EventKind::TypeChange { from, to } => (
            "type_change".to_string(),
            format!("type: {from} -> {to}"),
            json!({ "from": from, "to": to }),
        ),
        EventKind::TagsChange { added, removed } => {
            let mut parts: Vec<String> = added.iter().map(|t| format!("+{t}")).collect();
            parts.extend(removed.iter().map(|t| format!("-{t}")));
            (
                "tags_change".to_string(),
                format!("tags: {}", parts.join(" ")),
                json!({ "added": added, "removed": removed }),
            )
        }
        EventKind::CommentsAdded { count, author } => {
            let noun = if *count == 1 {
                "comment added"
            } else {
                "comments added"
            };
            let summary = match author {
                Some(a) => format!("{noun} {count} by {a}"),
                None => format!("{noun} {count}"),
            };
            (
                "comments_added".to_string(),
                summary,
                json!({ "count": count, "author": author }),
            )
        }
        // trace:BUG-1631 | ai:claude
        EventKind::RelationshipsChange {
            added,
            removed,
            edges,
        } => {
            let summary = if edges.is_empty() {
                format!("relationships: +{added} added, -{removed} removed")
            } else {
                let mut parts: Vec<String> = edges
                    .iter()
                    .map(|edge| {
                        let sign = if edge.added { '+' } else { '-' };
                        format!("{sign}{}", edge.describe(spec_id))
                    })
                    .collect();
                let (more_added, more_removed) = unnamed_edge_counts(*added, *removed, edges);
                if more_added > 0 {
                    parts.push(format!("+{more_added} more"));
                }
                if more_removed > 0 {
                    parts.push(format!("-{more_removed} more"));
                }
                format!("relationships: {}", parts.join(", "))
            };
            let edge_detail: Vec<JsonValue> = edges
                .iter()
                .map(|edge| {
                    json!({
                        "op": if edge.added { "added" } else { "removed" },
                        "from": spec_id,
                        "rel_type": edge.rel_type,
                        "label": rel_label(&edge.rel_type),
                        "to": edge.target,
                        "target_id": edge.target_id,
                    })
                })
                .collect();
            (
                "relationships_change".to_string(),
                summary,
                json!({ "added": added, "removed": removed, "edges": edge_detail }),
            )
        }
    }
}

fn format_event_body(e: &Event) -> String {
    match &e.kind {
        EventKind::Added {
            title,
            req_type,
            priority,
        } => format!(
            "{} ({}, {}) {}",
            "added".green(),
            req_type,
            priority,
            shorten(title, 80).dimmed()
        ),
        EventKind::Deleted { title } => {
            format!("{} {}", "deleted".red(), shorten(title, 80).dimmed())
        }
        EventKind::StatusChange { from, to } => {
            format!("{}: {} → {}", "status".bold(), from.yellow(), to.green())
        }
        EventKind::PriorityChange { from, to } => {
            format!("{}: {} → {}", "priority".bold(), from, to)
        }
        EventKind::TitleChange { from, to } => format!(
            "{}: {} → {}",
            "title".bold(),
            shorten(from, 40).dimmed(),
            shorten(to, 40)
        ),
        EventKind::DescriptionEdited => format!("{}", "description edited".bold()),
        EventKind::OwnerChange { from, to } => {
            format!(
                "{}: {} → {}",
                "owner".bold(),
                maybe_dash(from),
                maybe_dash(to)
            )
        }
        EventKind::FeatureChange { from, to } => format!(
            "{}: {} → {}",
            "feature".bold(),
            maybe_dash(from),
            maybe_dash(to)
        ),
        EventKind::TypeChange { from, to } => {
            format!("{}: {} → {}", "type".bold(), from, to)
        }
        EventKind::TagsChange { added, removed } => {
            let mut parts: Vec<String> = Vec::new();
            for t in added {
                parts.push(format!("+{}", t).green().to_string());
            }
            for t in removed {
                parts.push(format!("-{}", t).red().to_string());
            }
            format!("{}: {}", "tags".bold(), parts.join(" "))
        }
        EventKind::CommentsAdded { count, author } => {
            let by = match author {
                Some(a) => format!(" by {}", a.cyan()),
                None => String::new(),
            };
            if *count == 1 {
                format!("{}{}", "comment added".bold(), by)
            } else {
                format!("{} {}{}", "comments added".bold(), count, by)
            }
        }
        // BUG-1631: name each changed edge when it is known.
        // trace:BUG-1631 | ai:claude
        EventKind::RelationshipsChange {
            edges,
            added,
            removed,
        } if !edges.is_empty() => {
            let mut parts: Vec<String> = edges
                .iter()
                .map(|edge| {
                    let text = edge.describe(&e.spec_id);
                    if edge.added {
                        format!("+{text}").green().to_string()
                    } else {
                        format!("-{text}").red().to_string()
                    }
                })
                .collect();
            let (more_added, more_removed) = unnamed_edge_counts(*added, *removed, edges);
            if more_added > 0 {
                parts.push(format!("+{more_added} more").green().to_string());
            }
            if more_removed > 0 {
                parts.push(format!("-{more_removed} more").red().to_string());
            }
            format!("{}: {}", "relationships".bold(), parts.join(", "))
        }
        EventKind::RelationshipsChange { added, removed, .. } => {
            let mut parts = Vec::new();
            if *added > 0 {
                parts.push(format!("+{} added", added).green().to_string());
            }
            if *removed > 0 {
                parts.push(format!("-{} removed", removed).red().to_string());
            }
            format!("{}: {}", "relationships".bold(), parts.join(", "))
        }
    }
}

/// Changed edges counted in `added`/`removed` but not named in `edges`.
// trace:BUG-1631 | ai:claude
fn unnamed_edge_counts(added: usize, removed: usize, edges: &[RelEdge]) -> (usize, usize) {
    let named_added = edges.iter().filter(|e| e.added).count();
    let named_removed = edges.len() - named_added;
    (
        added.saturating_sub(named_added),
        removed.saturating_sub(named_removed),
    )
}

fn maybe_dash(s: &str) -> String {
    if s.is_empty() {
        "—".into()
    } else {
        s.to_string()
    }
}

// Char-boundary-safe truncation: titles carry emoji/unicode, so slicing by raw
// byte index panics mid-codepoint (BUG-424). Truncate by chars. trace:BUG-424
fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TASK-1055: a path-scoped `--id` walk only visits the spec's own commits,
    /// not the whole orphan-branch history. Builds a tiny store with three
    /// commits — one touching only TASK-1, one touching only STORY-1, and a
    /// bulk commit touching BOTH — then asserts (a) git's pathspec sees just
    /// the two commits that touched TASK-1's YAML (not all three), and (b) the
    /// filtered events come back as TASK-1's only.
    // trace:TASK-1055
    #[test]
    fn history_id_filter_path_scopes_log_walk() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let git = |args: &[&str]| {
            let out = ProcessCommand::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);

        let write = |rel: &str, body: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, body).unwrap();
        };
        let task_path = "objects/TASK/000/TASK-1.yaml";
        let story_path = "objects/STORY/000/STORY-1.yaml";

        // Commit 1: TASK-1 only.
        write(task_path, "spec_id: TASK-1\ntitle: t\nstatus: Draft\n");
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "add TASK-1"]);

        // Commit 2: STORY-1 only (a different spec — must NOT be walked).
        write(story_path, "spec_id: STORY-1\ntitle: s\nstatus: Draft\n");
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "add STORY-1"]);

        // Commit 3: a bulk commit touching BOTH specs.
        write(task_path, "spec_id: TASK-1\ntitle: t\nstatus: Approved\n");
        write(story_path, "spec_id: STORY-1\ntitle: s\nstatus: Approved\n");
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "chore: update requirements store"]);

        // (a) git's pathspec sees only the two TASK-1 commits, not all three.
        let scoped = run_git(
            root,
            &[
                "log".into(),
                "--oneline".into(),
                "--".into(),
                task_path.into(),
            ],
        )
        .unwrap();
        let full = run_git(root, &["log".into(), "--oneline".into()]).unwrap();
        assert_eq!(
            scoped.lines().count(),
            2,
            "pathspec must scope the walk to TASK-1's 2 commits"
        );
        assert_eq!(
            full.lines().count(),
            3,
            "the full history has all 3 commits — proving the pathspec narrows it"
        );

        // (b) the filtered events are TASK-1's only.
        let opts = HistoryOpts {
            id_filter: Some("TASK-1".to_string()),
            ..base_opts()
        };
        let (events, _, _) = collect_filtered_events(root, &opts).unwrap();
        assert!(!events.is_empty(), "expected at least one TASK-1 event");
        assert!(
            events.iter().all(|e| e.spec_id == "TASK-1"),
            "every event must be for the path-scoped spec, got: {:?}",
            events.iter().map(|e| &e.spec_id).collect::<Vec<_>>()
        );
    }

    /// STORY-737 (delight #4): the default `aida history` view hides the
    /// stateless internal META prompt-template rows (`exclude_meta`), but they
    /// stay reachable. This drives the real orphan-store git log and asserts the
    /// META spec's events drop out by default and return when `exclude_meta` is
    /// off (the `--include-meta` / `--type meta` path).
    // trace:STORY-737
    #[test]
    fn history_excludes_meta_rows_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let git = |args: &[&str]| {
            let out = ProcessCommand::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);

        let write = |rel: &str, body: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, body).unwrap();
        };
        let meta_path = "objects/META/000/META-1.yaml";
        let task_path = "objects/TASK/000/TASK-1.yaml";

        // Commit 1: seed both specs as Draft.
        write(meta_path, "spec_id: META-1\ntitle: m\nstatus: Draft\n");
        write(task_path, "spec_id: TASK-1\ntitle: t\nstatus: Draft\n");
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "seed"]);

        // Commit 2: flip both to Approved — one status-change event each.
        write(meta_path, "spec_id: META-1\ntitle: m\nstatus: Approved\n");
        write(task_path, "spec_id: TASK-1\ntitle: t\nstatus: Approved\n");
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "approve both"]);

        // Default view (exclude_meta = true): META-1 is hidden, TASK-1 shows.
        let hidden = HistoryOpts {
            exclude_meta: true,
            ..base_opts()
        };
        let (events, _, _) = collect_filtered_events(root, &hidden).unwrap();
        assert!(
            events.iter().any(|e| e.spec_id == "TASK-1"),
            "the real spec's events must still show, got: {:?}",
            events.iter().map(|e| &e.spec_id).collect::<Vec<_>>()
        );
        assert!(
            !events.iter().any(|e| e.spec_id == "META-1"),
            "META rows must be hidden by default, got: {:?}",
            events.iter().map(|e| &e.spec_id).collect::<Vec<_>>()
        );

        // `--include-meta` / `--type meta` (exclude_meta = false): META returns.
        let shown = HistoryOpts {
            exclude_meta: false,
            ..base_opts()
        };
        let (events, _, _) = collect_filtered_events(root, &shown).unwrap();
        assert!(
            events.iter().any(|e| e.spec_id == "META-1"),
            "META rows must be visible when not excluded, got: {:?}",
            events.iter().map(|e| &e.spec_id).collect::<Vec<_>>()
        );
    }

    /// BUG-1596: the digest must resolve a requirement's *current* YAML
    /// under its real shard, not a hard-coded `000`. Builds a store with one
    /// spec in shard 000 (BUG-1) and one whose sequence number lands in
    /// shard 001 (BUG-1001, sequence 1001 — the first id past the 1000-per-
    /// shard boundary), commits both, and asserts the digest reports each
    /// one's real status/title rather than "(deleted)". A genuinely deleted
    /// spec (never written) must still render "(deleted)".
    // trace:BUG-1596 | ai:claude
    #[test]
    fn digest_resolves_current_yaml_across_shards() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let git = |args: &[&str]| {
            let out = ProcessCommand::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);

        let write = |rel: &str, body: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, body).unwrap();
        };

        // Shard 000: sequence 1.
        let shard0_path = "objects/BUG/000/BUG-1.yaml";
        write(
            shard0_path,
            "spec_id: BUG-1\ntitle: shard zero bug\nstatus: Completed\nmodified_at: \"2026-01-01T00:00:00Z\"\n",
        );
        // Shard 001: sequence 1001 (first id past the 1000-per-shard
        // boundary — object_path()/shard_number() puts seq 1001 in shard
        // 001). This is the path the hard-coded "000" join used to miss.
        let shard1_path = "objects/BUG/001/BUG-1001.yaml";
        write(
            shard1_path,
            "spec_id: BUG-1001\ntitle: shard one bug\nstatus: Done\nmodified_at: \"2026-01-02T00:00:00Z\"\n",
        );
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "add BUG-1 and BUG-1001"]);

        let opts = HistoryOpts {
            events_mode: false,
            ..base_opts()
        };
        let (rows, _, _) = build_digest_rows(root, &opts).unwrap();

        let by_id = |id: &str| rows.iter().find(|r| r.spec_id == id);

        let shard0 = by_id("BUG-1").expect("BUG-1 (shard 000) must appear");
        assert_eq!(shard0.status, "Completed");
        assert_eq!(shard0.title, "shard zero bug");

        let shard1 = by_id("BUG-1001").expect("BUG-1001 (shard 001) must appear");
        assert_eq!(
            shard1.status, "Done",
            "shard-001 spec must resolve its real status, not fall through to (deleted)"
        );
        assert_eq!(shard1.title, "shard one bug");

        // A genuinely deleted / never-written id still renders "(deleted)".
        let (status, title, modified_at) = read_current(&root.join("objects/BUG/000/BUG-999.yaml"));
        assert_eq!(status, "(deleted)");
        assert_eq!(title, "");
        assert_eq!(modified_at, None);
    }

    // TASK-1502: `resolve_history_window` — every unit, mixed relative +
    // absolute bounds, bad units, reversed order, and unchanged absolute
    // forms. All deterministic via an injected `now`. trace:TASK-1502 | ai:claude

    fn fixed_now() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-09-25T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    /// A fixed, non-UTC offset (UTC-7) so `resolve_history_window` tests are
    /// deterministic regardless of the test machine's real timezone, and so
    /// the local-midnight-vs-UTC-midnight distinction is actually exercised
    /// (on UTC+0 the two coincide and the bug wouldn't show up).
    // trace:TASK-1502 | ai:claude
    fn fixed_local_offset() -> chrono::FixedOffset {
        chrono::FixedOffset::west_opt(7 * 3600).unwrap()
    }

    #[test]
    fn resolve_history_window_every_relative_unit() {
        let now = fixed_now();
        let offset = fixed_local_offset();
        let cases: &[(&str, chrono::Duration)] = &[
            ("30m", chrono::Duration::minutes(30)),
            ("5h", chrono::Duration::hours(5)),
            ("7d", chrono::Duration::days(7)),
            ("2w", chrono::Duration::weeks(2)),
        ];
        for (raw, delta) in cases {
            let opts = HistoryOpts {
                since: Some((*raw).to_string()),
                ..base_opts()
            };
            let (resolved, since_at, until_at) =
                resolve_history_window(&opts, now, &offset).unwrap();
            assert_eq!(since_at, Some(now - *delta), "unit `{raw}`");
            assert_eq!(until_at, None);
            // The rewritten opts carry an unambiguous RFC3339 string, not
            // the original compact form, so every downstream `git log`
            // call gets a value git's approxidate parser won't mangle.
            assert_eq!(resolved.since, Some((now - *delta).to_rfc3339()));
        }
    }

    #[test]
    fn resolve_history_window_mixed_relative_and_absolute_bounds() {
        // --since as a compact relative duration, --until as an absolute
        // RFC3339 timestamp — both forms must compose.
        let now = fixed_now();
        let offset = fixed_local_offset();
        let opts = HistoryOpts {
            since: Some("7d".to_string()),
            until: Some("2026-09-24T00:00:00Z".to_string()),
            ..base_opts()
        };
        let (_, since_at, until_at) = resolve_history_window(&opts, now, &offset).unwrap();
        assert_eq!(since_at, Some(now - chrono::Duration::days(7)));
        assert_eq!(
            until_at,
            Some(
                chrono::DateTime::parse_from_rfc3339("2026-09-24T00:00:00Z")
                    .unwrap()
                    .with_timezone(&chrono::Utc)
            )
        );

        // And the reverse pairing: --since absolute (bare date, LOCAL
        // midnight at UTC-7 → 07:00 UTC), --until relative.
        let opts = HistoryOpts {
            since: Some("2026-09-01".to_string()),
            until: Some("5h".to_string()),
            ..base_opts()
        };
        let (_, since_at, until_at) = resolve_history_window(&opts, now, &offset).unwrap();
        assert_eq!(since_at.unwrap().to_rfc3339(), "2026-09-01T07:00:00+00:00");
        assert_eq!(until_at, Some(now - chrono::Duration::hours(5)));
    }

    #[test]
    fn resolve_history_window_unchanged_absolute_forms_still_work() {
        // RFC3339 — the pre-TASK-1502 documented form — must keep resolving
        // exactly as before, zone honored exactly regardless of the
        // machine's local offset.
        let now = fixed_now();
        let offset = fixed_local_offset();
        let opts = HistoryOpts {
            since: Some("2026-05-01T00:00:00Z".to_string()),
            until: Some("2026-06-01T00:00:00+02:00".to_string()),
            ..base_opts()
        };
        let (_, since_at, until_at) = resolve_history_window(&opts, now, &offset).unwrap();
        assert_eq!(
            since_at.unwrap().format("%Y-%m-%d").to_string(),
            "2026-05-01"
        );
        assert_eq!(until_at.unwrap().to_rfc3339(), "2026-05-31T22:00:00+00:00");
    }

    #[test]
    fn resolve_history_window_bare_date_uses_local_offset_not_utc() {
        // A bare ISO date resolves to LOCAL midnight, not UTC midnight; the
        // two differ by exactly `offset` away from UTC+0.
        let now = fixed_now();
        let offset = fixed_local_offset(); // UTC-7
        let opts = HistoryOpts {
            since: Some("2026-05-01".to_string()),
            ..base_opts()
        };
        let (resolved, since_at, _) = resolve_history_window(&opts, now, &offset).unwrap();
        assert_eq!(
            since_at.unwrap().to_rfc3339(),
            "2026-05-01T07:00:00+00:00",
            "bare date must resolve to local midnight (UTC-7 → 07:00 UTC), not UTC midnight"
        );
        assert_eq!(
            resolved.since,
            Some("2026-05-01T07:00:00+00:00".to_string())
        );
    }

    #[test]
    fn resolve_history_window_rejects_bad_unit() {
        let now = fixed_now();
        let offset = fixed_local_offset();
        let opts = HistoryOpts {
            since: Some("5y".to_string()),
            ..base_opts()
        };
        let err = resolve_history_window(&opts, now, &offset)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("--since") && err.contains("5y"),
            "error should name the flag and the bad value, got: {err}"
        );

        let opts = HistoryOpts {
            until: Some("3q".to_string()),
            ..base_opts()
        };
        let err = resolve_history_window(&opts, now, &offset)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("--until") && !err.starts_with("invalid --since"),
            "a bad --until must be labeled --until, not --since, got: {err}"
        );
    }

    #[test]
    fn resolve_history_window_rejects_since_later_than_until() {
        let now = fixed_now();
        let offset = fixed_local_offset();
        // --since 5h (5 hours ago) is LATER than --until 7d (7 days ago) —
        // the window is empty and must be refused with a clear error.
        let opts = HistoryOpts {
            since: Some("5h".to_string()),
            until: Some("7d".to_string()),
            ..base_opts()
        };
        let err = resolve_history_window(&opts, now, &offset)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("--since") && err.contains("--until"),
            "reversed-order error should name both flags, got: {err}"
        );
    }

    #[test]
    fn format_resolved_window_variants() {
        let now = fixed_now();
        let offset = fixed_local_offset();
        assert_eq!(format_resolved_window(None, None, &offset), None);
        assert!(format_resolved_window(Some(now), None, &offset)
            .unwrap()
            .contains("since"));
        assert!(format_resolved_window(None, Some(now), &offset)
            .unwrap()
            .contains("until"));
        let both =
            format_resolved_window(Some(now - chrono::Duration::days(1)), Some(now), &offset)
                .unwrap();
        // Rendered in the injected LOCAL offset (UTC-7): `now`
        // (2026-09-25T12:00:00Z) is 2026-09-25 05:00 -0700 locally, and
        // `now - 1 day` is 2026-09-24 05:00 -0700 — same calendar dates as
        // UTC here (no midnight boundary crossed at this hour), so this
        // also proves the explicit `-0700` zone marker is present.
        // trace:TASK-1502 | ai:claude
        assert!(
            both.contains("-0700"),
            "Window line must state the timezone, got: {both}"
        );
        assert!(both.contains("2026-09-24") && both.contains("2026-09-25"));
    }

    /// A test-only zone that observes US-Pacific-style DST in 2026: UTC-8
    /// outside, UTC-7 between 2026-03-08T10:00Z (02:00 PST) and
    /// 2026-11-01T09:00Z (02:00 PDT). Stands in for a real DST zone so the
    /// per-date offset lookup is exercised without adding chrono-tz.
    // trace:TASK-1502 | ai:claude
    #[derive(Clone, Copy, Debug)]
    struct TestPacific;

    impl TestPacific {
        fn std() -> chrono::FixedOffset {
            chrono::FixedOffset::west_opt(8 * 3600).unwrap()
        }
        fn dst() -> chrono::FixedOffset {
            chrono::FixedOffset::west_opt(7 * 3600).unwrap()
        }
    }

    impl chrono::TimeZone for TestPacific {
        type Offset = chrono::FixedOffset;

        fn from_offset(_: &chrono::FixedOffset) -> Self {
            TestPacific
        }

        fn offset_from_local_date(
            &self,
            local: &chrono::NaiveDate,
        ) -> chrono::MappedLocalTime<chrono::FixedOffset> {
            self.offset_from_local_datetime(&local.and_time(chrono::NaiveTime::MIN))
        }

        fn offset_from_local_datetime(
            &self,
            local: &chrono::NaiveDateTime,
        ) -> chrono::MappedLocalTime<chrono::FixedOffset> {
            // Every offset whose implied UTC instant maps back to itself.
            let valid: Vec<chrono::FixedOffset> = [Self::std(), Self::dst()]
                .into_iter()
                .filter(|off| {
                    let utc = *local - chrono::Duration::seconds(off.local_minus_utc().into());
                    self.offset_from_utc_datetime(&utc) == *off
                })
                .collect();
            match valid.as_slice() {
                [] => chrono::MappedLocalTime::None,
                [one] => chrono::MappedLocalTime::Single(*one),
                [a, b] => chrono::MappedLocalTime::Ambiguous(*a, *b),
                _ => unreachable!(),
            }
        }

        fn offset_from_utc_date(&self, utc: &chrono::NaiveDate) -> chrono::FixedOffset {
            self.offset_from_utc_datetime(&utc.and_time(chrono::NaiveTime::MIN))
        }

        fn offset_from_utc_datetime(&self, utc: &chrono::NaiveDateTime) -> chrono::FixedOffset {
            let start = chrono::NaiveDate::from_ymd_opt(2026, 3, 8)
                .unwrap()
                .and_hms_opt(10, 0, 0)
                .unwrap();
            let end = chrono::NaiveDate::from_ymd_opt(2026, 11, 1)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap();
            if *utc >= start && *utc < end {
                Self::dst()
            } else {
                Self::std()
            }
        }
    }

    #[test]
    fn resolve_history_window_bare_dates_use_that_dates_dst_offset() {
        // `now` is in September (PDT, UTC-7), but a January date must still
        // resolve against January's offset (PST, UTC-8), not today's.
        let now = fixed_now();
        let opts = HistoryOpts {
            since: Some("2026-01-15".to_string()),
            until: Some("2026-07-15".to_string()),
            ..base_opts()
        };
        let (_, since_at, until_at) = resolve_history_window(&opts, now, &TestPacific).unwrap();
        assert_eq!(since_at.unwrap().to_rfc3339(), "2026-01-15T08:00:00+00:00");
        assert_eq!(until_at.unwrap().to_rfc3339(), "2026-07-15T07:00:00+00:00");
        // And the Window line shows each instant's own offset.
        let line = format_resolved_window(since_at, until_at, &TestPacific).unwrap();
        assert!(
            line.contains("2026-01-15 00:00 -0800") && line.contains("2026-07-15 00:00 -0700"),
            "each bound must render with its own offset, got: {line}"
        );
    }

    #[test]
    fn resolve_history_window_rejects_dst_gap_and_overlap() {
        let now = fixed_now();
        // 02:30 on the spring-forward day never happens locally.
        let opts = HistoryOpts {
            since: Some("2026-03-08T02:30".to_string()),
            ..base_opts()
        };
        let err = resolve_history_window(&opts, now, &TestPacific)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("--since") && err.contains("ambiguous or doesn't exist"),
            "a DST-gap time must be refused with the DST reason, got: {err}"
        );
        // 01:30 on the fall-back day happens twice.
        let opts = HistoryOpts {
            until: Some("2026-11-01 01:30".to_string()),
            ..base_opts()
        };
        let err = resolve_history_window(&opts, now, &TestPacific)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("--until") && err.contains("ambiguous or doesn't exist"),
            "a DST-overlap time must be refused with the DST reason, got: {err}"
        );
    }

    #[test]
    fn resolve_history_window_accepts_legacy_forms() {
        // Zone-less ISO datetimes (local time) and git-style `N units ago`
        // phrases were accepted before and must keep working.
        let now = fixed_now();
        let offset = fixed_local_offset(); // UTC-7
        for raw in [
            "2026-05-01T10:00",
            "2026-05-01 10:00",
            "2026-05-01T10:00:00",
        ] {
            let opts = HistoryOpts {
                since: Some(raw.to_string()),
                ..base_opts()
            };
            let (_, since_at, _) = resolve_history_window(&opts, now, &offset).unwrap();
            assert_eq!(
                since_at.unwrap().to_rfc3339(),
                "2026-05-01T17:00:00+00:00",
                "`{raw}` must be read as local time"
            );
        }
        let cases: &[(&str, chrono::Duration)] = &[
            ("24 hours ago", chrono::Duration::hours(24)),
            ("1 hour ago", chrono::Duration::hours(1)),
            ("90 minutes ago", chrono::Duration::minutes(90)),
            ("3 days ago", chrono::Duration::days(3)),
            ("1 week ago", chrono::Duration::weeks(1)),
            ("2 Weeks Ago", chrono::Duration::weeks(2)),
        ];
        for (raw, delta) in cases {
            let opts = HistoryOpts {
                since: Some((*raw).to_string()),
                ..base_opts()
            };
            let (_, since_at, _) = resolve_history_window(&opts, now, &offset).unwrap();
            assert_eq!(since_at, Some(now - *delta), "phrase `{raw}`");
        }
        for bad in ["3 fortnights ago", "3 days", "ago 3 days"] {
            let opts = HistoryOpts {
                since: Some(bad.to_string()),
                ..base_opts()
            };
            assert!(
                resolve_history_window(&opts, now, &offset).is_err(),
                "`{bad}` must be rejected"
            );
        }
    }

    /// TASK-1502: end-to-end through `collect_filtered_events` — a compact
    /// relative duration like `1h` must bound the actual `git log --since=`
    /// walk correctly, not just parse cleanly. This is the regression the
    /// feature exists to fix: git's own approxidate parser does not
    /// understand compact `30m`/`1h` forms (`git log --since=30m` is read as
    /// a date on the 30th and matches nothing, with no error), so history
    /// resolves the bound itself and hands git an unambiguous RFC3339
    /// string instead.
    #[test]
    fn relative_since_bounds_the_actual_git_log_walk() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let git = |args: &[&str], env: &[(&str, &str)]| {
            let mut cmd = ProcessCommand::new("git");
            cmd.arg("-C").arg(root).args(args);
            for (k, v) in env {
                cmd.env(k, v);
            }
            let out = cmd.output().unwrap();
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q"], &[]);
        git(&["config", "user.email", "t@example.com"], &[]);
        git(&["config", "user.name", "t"], &[]);

        let write = |rel: &str, body: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, body).unwrap();
        };

        let now = fixed_now();
        let old_ts = (now - chrono::Duration::hours(2)).to_rfc3339();
        let recent_ts = (now - chrono::Duration::minutes(10)).to_rfc3339();

        // Commit A: 2 hours before `now` — outside a 1h window.
        write(
            "objects/TASK/000/TASK-1.yaml",
            "spec_id: TASK-1\ntitle: old\nstatus: Draft\n",
        );
        git(&["add", "-A"], &[]);
        git(
            &["commit", "-q", "-m", "old commit"],
            &[
                ("GIT_AUTHOR_DATE", old_ts.as_str()),
                ("GIT_COMMITTER_DATE", old_ts.as_str()),
            ],
        );

        // Commit B: 10 minutes before `now` — inside a 1h window.
        write(
            "objects/TASK/000/TASK-2.yaml",
            "spec_id: TASK-2\ntitle: recent\nstatus: Draft\n",
        );
        git(&["add", "-A"], &[]);
        git(
            &["commit", "-q", "-m", "recent commit"],
            &[
                ("GIT_AUTHOR_DATE", recent_ts.as_str()),
                ("GIT_COMMITTER_DATE", recent_ts.as_str()),
            ],
        );

        let opts = HistoryOpts {
            since: Some("1h".to_string()),
            ..base_opts()
        };
        let (resolved, _, _) = resolve_history_window(&opts, now, &fixed_local_offset()).unwrap();
        let (events, _, _) = collect_filtered_events(root, &resolved).unwrap();
        let ids: std::collections::BTreeSet<&str> =
            events.iter().map(|e| e.spec_id.as_str()).collect();
        assert!(
            ids.contains("TASK-2"),
            "recent commit must be in a 1h window, got {ids:?}"
        );
        assert!(
            !ids.contains("TASK-1"),
            "commit from 2h ago must be excluded from a 1h window, got {ids:?}"
        );
    }

    /// BUG-1616: a commit that deletes one spec and adds another with
    /// near-identical content is paired by git's default rename detection
    /// into a single `R<score>` line. The event walk and the digest must
    /// still report the delete and the add as separate events.
    // trace:BUG-1616 | ai:claude
    #[test]
    fn history_reports_delete_and_add_that_git_pairs_as_rename() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let git = |args: &[&str]| -> String {
            let out = ProcessCommand::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).to_string()
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);

        let a_path = "objects/BUG/000/BUG-389.yaml";
        let b_path = "objects/BUG/000/BUG-390.yaml";
        let body = |id: &str| {
            format!(
                "spec_id: {id}\ntitle: a long shared title so git scores the pair as a rename\n\
                 status: Draft\npriority: high\nreq_type: bug\n\
                 description: the same long description body in both files so the \
                 similarity index stays well above the rename threshold\n"
            )
        };

        std::fs::create_dir_all(root.join("objects/BUG/000")).unwrap();
        std::fs::write(root.join(a_path), body("BUG-389")).unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "add BUG-389"]);

        // One commit: delete BUG-389, add BUG-390 with near-identical content.
        std::fs::remove_file(root.join(a_path)).unwrap();
        std::fs::write(root.join(b_path), body("BUG-390")).unwrap();
        git(&["add", "-A"]);
        git(&[
            "commit",
            "-q",
            "-m",
            "chore: update 1 requirements, delete 1",
        ]);

        // Precondition: git's default detection does pair these as a rename,
        // so the fixture really exercises the bug.
        let default_status = git(&["show", "--name-status", "--format=", "-M", "HEAD"]);
        assert!(
            default_status.lines().any(|l| l.starts_with('R')),
            "fixture must be a git rename pair, got: {default_status}"
        );
        let head = git(&["rev-parse", "HEAD"]).trim().to_string();

        // Event walk: both the delete and the add from the rename commit.
        let (events, _, _) = collect_filtered_events(root, &base_opts()).unwrap();
        let in_head: Vec<&Event> = events.iter().filter(|e| e.sha == head).collect();
        assert!(
            in_head
                .iter()
                .any(|e| e.spec_id == "BUG-389" && matches!(e.kind, EventKind::Deleted { .. })),
            "missing Deleted BUG-389, got: {:?}",
            in_head
                .iter()
                .map(|e| (&e.spec_id, &e.kind))
                .collect::<Vec<_>>()
        );
        assert!(
            in_head
                .iter()
                .any(|e| e.spec_id == "BUG-390" && matches!(e.kind, EventKind::Added { .. })),
            "missing Added BUG-390, got: {:?}",
            in_head
                .iter()
                .map(|e| (&e.spec_id, &e.kind))
                .collect::<Vec<_>>()
        );

        // Digest: BUG-389 marked deleted, BUG-390 marked added.
        let digest_opts = HistoryOpts {
            events_mode: false,
            ..base_opts()
        };
        let (rows, _, _) = build_digest_rows(root, &digest_opts).unwrap();
        let a = rows
            .iter()
            .find(|r| r.spec_id == "BUG-389")
            .expect("BUG-389 row");
        let b = rows
            .iter()
            .find(|r| r.spec_id == "BUG-390")
            .expect("BUG-390 row");
        assert!(a.had_delete, "digest must mark BUG-389 as deleted");
        assert!(b.had_add, "digest must mark BUG-390 as added");
    }

    /// A `HistoryOpts` with every filter off — tests override the one field
    /// they exercise.
    // trace:TASK-1055
    fn base_opts() -> HistoryOpts {
        HistoryOpts {
            limit: 1000,
            max_commits: 1000,
            max_commits_explicit: false,
            events_mode: true,
            id_filter: None,
            type_filter: None,
            author_filter: None,
            since: None,
            until: None,
            status_changes_only: false,
            shipped_only: false,
            to_status: None,
            from_status: None,
            opened_only: false,
            comments_only: false,
            oneline: false,
            archived_specs: std::collections::HashSet::new(),
            archived_only_specs: None,
            deferred_specs: std::collections::HashSet::new(),
            deferred_only_specs: None,
            exclude_meta: false,
        }
    }

    /// BUG-1617: builds a git-canonical fixture with one spec whose status
    /// flips back and forth `commit_count` times after a seed commit — each
    /// flip is exactly one `StatusChange` event, so a test can dial in a
    /// precise commit/event count without needing hundreds of real commits
    /// to exercise a small `max_commits` window.
    // trace:BUG-1617 | ai:claude
    fn write_status_flip_fixture(root: &Path, commit_count: usize) {
        let git = |args: &[&str]| {
            let out = ProcessCommand::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);

        let rel = "objects/BUG/000/BUG-1.yaml";
        let full = root.join(rel);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();

        std::fs::write(&full, "spec_id: BUG-1\ntitle: t\nstatus: Draft\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "seed"]);

        for i in 0..commit_count {
            let status = if i % 2 == 0 { "Approved" } else { "Draft" };
            std::fs::write(
                &full,
                format!("spec_id: BUG-1\ntitle: t\nstatus: {status}\n"),
            )
            .unwrap();
            git(&["add", "-A"]);
            git(&["commit", "-q", "-m", &format!("flip {i}")]);
        }
    }

    /// BUG-1617: the commit walk gets capped at 3 commits, but 8 events were
    /// requested and 10 exist — the DEFAULT window ran out first, so
    /// `window_exhausted` must be true.
    // trace:BUG-1617 | ai:claude
    #[test]
    fn collect_filtered_events_reports_window_exhausted_when_default_window_runs_out() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_status_flip_fixture(root, 10);

        let opts = HistoryOpts {
            limit: 8,
            max_commits: 3,
            max_commits_explicit: false,
            ..base_opts()
        };
        let (events, _, window_exhausted) = collect_filtered_events(root, &opts).unwrap();
        assert!(
            events.len() < 8,
            "expected fewer than the requested limit, got {}",
            events.len()
        );
        assert!(
            window_exhausted,
            "the 3-commit cap was hit before the 8-event limit — expected window_exhausted=true"
        );
    }

    /// BUG-1617: same fixture, but the window and the limit line up exactly
    /// (3 commits, 3 requested) — the limit was MET, so even though the
    /// commit walk was also capped at the same point, this is not
    /// "exhausted": nothing was silently left out of the requested count.
    // trace:BUG-1617 | ai:claude
    #[test]
    fn collect_filtered_events_no_window_exhausted_when_limit_met() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_status_flip_fixture(root, 10);

        let opts = HistoryOpts {
            limit: 3,
            max_commits: 3,
            max_commits_explicit: false,
            ..base_opts()
        };
        let (events, _, window_exhausted) = collect_filtered_events(root, &opts).unwrap();
        assert_eq!(events.len(), 3);
        assert!(
            !window_exhausted,
            "the limit was met exactly at the window edge — must not be flagged as exhausted"
        );
    }

    /// BUG-1617: the window is far larger than the real history (4 commits
    /// total vs. a 100-commit cap) — the walk legitimately ran out of
    /// commits to look at, not window capacity. Must not be flagged as
    /// exhausted even though fewer than `limit` events came back.
    // trace:BUG-1617 | ai:claude
    #[test]
    fn collect_filtered_events_not_exhausted_when_real_history_ends_first() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_status_flip_fixture(root, 3);

        let opts = HistoryOpts {
            limit: 10,
            max_commits: 100,
            max_commits_explicit: false,
            ..base_opts()
        };
        let (events, _, window_exhausted) = collect_filtered_events(root, &opts).unwrap();
        // 3 flip commits (StatusChange events) plus the seed commit itself
        // (an Added event — the default opts don't filter kinds) = 4.
        assert_eq!(events.len(), 4);
        assert!(
            !window_exhausted,
            "real history ran out, not the window — must not be flagged as exhausted"
        );
    }

    /// BUG-1617 review fix: the exact-boundary case the naive
    /// `commits.len() >= max_commits` check got wrong — history that is
    /// *exactly* `max_commits` commits long is a coincidence, not
    /// exhaustion. 4 flips + 1 seed = 5 commits total, `max_commits: 5`, and
    /// a `--limit` nowhere near met (10). Before the over-fetch-by-one fix
    /// this reported `window_exhausted: true`; it must report `false` —
    /// there is nothing beyond the cap to widen toward.
    // trace:BUG-1617 | ai:claude
    #[test]
    fn collect_filtered_events_not_exhausted_when_history_is_exactly_max_commits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_status_flip_fixture(root, 4);

        let opts = HistoryOpts {
            limit: 10,
            max_commits: 5,
            max_commits_explicit: false,
            ..base_opts()
        };
        let (events, _, window_exhausted) = collect_filtered_events(root, &opts).unwrap();
        // 4 status-change flips + 1 Added (the seed) = 5, all of history.
        assert_eq!(events.len(), 5);
        assert!(
            !window_exhausted,
            "history is exactly max_commits long — that IS everything, not exhaustion"
        );
    }

    /// BUG-1617 review fix: the sibling of the exactly-at-cap case above —
    /// history is one commit LONGER than `max_commits` (5 flips + 1 seed = 6
    /// commits, `max_commits: 5`), so there genuinely is more beyond the
    /// window. Only the newest `max_commits` commits should be decoded
    /// (the oldest — the seed's Added event — must NOT appear), and
    /// `window_exhausted` must be true.
    // trace:BUG-1617 | ai:claude
    #[test]
    fn collect_filtered_events_exhausted_when_history_is_one_more_than_max_commits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_status_flip_fixture(root, 5);

        let opts = HistoryOpts {
            limit: 10,
            max_commits: 5,
            max_commits_explicit: false,
            ..base_opts()
        };
        let (events, _, window_exhausted) = collect_filtered_events(root, &opts).unwrap();
        // Only the 5 newest commits are decoded — all 5 are StatusChange
        // flips; the oldest (seed/Added) commit falls outside the window.
        assert_eq!(events.len(), 5);
        assert!(
            events
                .iter()
                .all(|e| matches!(e.kind, EventKind::StatusChange { .. })),
            "the oldest (seed/Added) commit must fall outside the window"
        );
        assert!(
            window_exhausted,
            "history continues one commit past the cap — expected window_exhausted=true"
        );
    }

    /// BUG-1617: the notice text a human sees — present, and mentions both
    /// ways to widen the walk (--max-commits and a time bound), exactly when
    /// the window was exhausted and the caller did NOT pin --max-commits
    /// themselves.
    // trace:BUG-1617 | ai:claude
    #[test]
    fn window_exhausted_notice_text_present_for_default_window() {
        let opts = HistoryOpts {
            limit: 8,
            max_commits: 3,
            max_commits_explicit: false,
            ..base_opts()
        };
        let msg = window_exhausted_notice_text(&opts, true, 3).expect("expected a notice");
        assert!(
            msg.contains("--max-commits"),
            "notice should mention --max-commits, got: {msg}"
        );
        assert!(
            msg.contains("--since") || msg.contains("--until"),
            "notice should mention a time-bound escape hatch, got: {msg}"
        );
    }

    /// BUG-1617 acceptance: no notice when the limit was met.
    #[test]
    fn window_exhausted_notice_text_absent_when_limit_met() {
        let opts = HistoryOpts {
            limit: 3,
            max_commits: 3,
            max_commits_explicit: false,
            ..base_opts()
        };
        assert!(window_exhausted_notice_text(&opts, false, 3).is_none());
    }

    /// BUG-1617 acceptance: no notice when the caller passed an explicit
    /// --max-commits — they already know they narrowed the walk.
    #[test]
    fn window_exhausted_notice_text_absent_when_max_commits_explicit() {
        let opts = HistoryOpts {
            limit: 8,
            max_commits: 3,
            max_commits_explicit: true,
            ..base_opts()
        };
        assert!(window_exhausted_notice_text(&opts, true, 3).is_none());
    }

    /// BUG-424: a multibyte char straddling the truncation point must not panic
    /// (raw byte-slicing did). Titles carry emoji/unicode glyphs.
    #[test]
    fn shorten_is_char_boundary_safe_on_emoji_title() {
        let s = "Review PR-75: apply ⇒/⏸ glyph set to skill templates (BUG-116)";
        // byte index ~59 lands inside the ⏸ codepoint — the old panic point.
        for max in [40usize, 58, 59, 60, 1] {
            let out = shorten(s, max); // must not panic at any boundary
            assert!(out.chars().count() <= max.max(1));
        }
        assert!(shorten(s, 30).ends_with('…'));
        // No truncation when it fits (by char count).
        assert_eq!(shorten("short ⏸ title", 100), "short ⏸ title");
    }

    #[test]
    fn spec_id_from_path_extracts_stem() {
        assert_eq!(
            spec_id_from_path("objects/FR/000/FR-1-011.yaml"),
            "FR-1-011"
        );
        assert_eq!(
            spec_id_from_path("objects/EPIC/000/EPIC-1-005.yaml"),
            "EPIC-1-005"
        );
    }

    #[test]
    fn req_type_from_path_pulls_first_segment() {
        assert_eq!(req_type_from_path("objects/FR/000/FR-1-011.yaml"), "FR");
        assert_eq!(req_type_from_path("objects/TASK/000/TASK-1.yaml"), "TASK");
    }

    #[test]
    fn human_timestamp_converts_utc_to_local() {
        // The input is UTC; the output is in the user's local zone. We
        // can't assert a specific value here without controlling TZ, but
        // we CAN assert the parse path round-trips: parsing then
        // formatting produces the same instant regardless of zone.
        use chrono::{DateTime, FixedOffset, Local};
        let iso = "2026-05-04T18:02:38.123456Z";
        let formatted = human_timestamp(iso);
        // Re-parse our formatted local string + assert it matches the
        // expected local representation of the original UTC instant.
        let utc: DateTime<FixedOffset> = iso.parse().unwrap();
        let expected = utc
            .with_timezone(&Local)
            .format("%Y-%m-%d %H:%M")
            .to_string();
        assert_eq!(formatted, expected);
        // Sanity: the bug we're guarding against is "shows raw UTC HH:MM
        // regardless of local zone" — so on west-of-UTC machines the
        // output should NOT contain "18:02" verbatim.
        if Local::now().offset().local_minus_utc() < 0 {
            assert!(
                !formatted.ends_with("18:02"),
                "output is still in UTC: {}",
                formatted
            );
        }
    }

    #[test]
    fn effective_falls_back_to_custom() {
        let v: Value =
            serde_yaml::from_str("status: Draft\ncustom_status: Awaiting Review").unwrap();
        assert_eq!(
            effective(&v, "status", "custom_status").as_deref(),
            Some("Awaiting Review")
        );
    }

    #[test]
    fn diff_status_emits_one_event() {
        let before: Value = serde_yaml::from_str("status: Draft\nspec_id: FR-1\ntitle: t").unwrap();
        let after: Value =
            serde_yaml::from_str("status: Approved\nspec_id: FR-1\ntitle: t").unwrap();
        let commit = CommitMeta {
            sha: "abc".into(),
            iso_timestamp: "2026-05-04T12:00:00Z".into(),
            git_author: "joe@example.com".into(),
        };
        let mut out = Vec::new();
        let mk = |kind: EventKind| Event {
            sha: commit.sha.clone(),
            timestamp: human_timestamp(&commit.iso_timestamp),
            author: "joe".into(),
            spec_id: "FR-1".into(),
            req_type: "FR".into(),
            kind,
        };
        diff_modified(&before, &after, &mk, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0].kind, EventKind::StatusChange { .. }));
    }

    /// TASK-507: `--shipped` keeps only transitions into Completed, not other
    /// status flips. BUG-1636: from any prior status, not only Done.
    // trace:BUG-1636 | ai:claude
    #[test]
    fn is_ship_event_only_transitions_into_completed() {
        assert!(is_ship_event(&EventKind::StatusChange {
            from: "InProgress".into(),
            to: "Completed".into(),
        }));
        assert!(!is_ship_event(&EventKind::StatusChange {
            from: "Completed".into(),
            to: "Completed".into(),
        }));
        let ship = EventKind::StatusChange {
            from: "Done".into(),
            to: "Completed".into(),
        };
        assert!(is_ship_event(&ship));
        // case-insensitive
        assert!(is_ship_event(&EventKind::StatusChange {
            from: "done".into(),
            to: "completed".into(),
        }));
        // other transitions are not ships
        assert!(!is_ship_event(&EventKind::StatusChange {
            from: "Approved".into(),
            to: "Done".into(),
        }));
        assert!(!is_ship_event(&EventKind::StatusChange {
            from: "InProgress".into(),
            to: "Done".into(),
        }));
        assert!(!is_ship_event(&EventKind::PriorityChange {
            from: "Low".into(),
            to: "High".into(),
        }));
    }

    #[test]
    fn event_record_exposes_structured_status_change() {
        let event = Event {
            sha: "abcdef123456".into(),
            timestamp: "2026-05-24 10:00".into(),
            author: "codex".into(),
            spec_id: "TASK-538".into(),
            req_type: "TASK".into(),
            kind: EventKind::StatusChange {
                from: "Approved".into(),
                to: "Completed".into(),
            },
        };

        let record = event_record(&event);
        assert_eq!(record.kind, "status_change");
        assert_eq!(record.spec_id, "TASK-538");
        assert_eq!(record.detail["from"], "Approved");
        assert_eq!(record.detail["to"], "Completed");
        assert!(record.summary.contains("Approved -> Completed"));
    }

    #[test]
    fn diff_tags_set_semantics() {
        let before: Value = serde_yaml::from_str("tags: [a, b]").unwrap();
        let after: Value = serde_yaml::from_str("tags: [b, c]").unwrap();
        let mut out = Vec::new();
        let commit = CommitMeta {
            sha: "abc".into(),
            iso_timestamp: "2026-05-04T12:00:00Z".into(),
            git_author: "joe@example.com".into(),
        };
        let mk = |kind: EventKind| Event {
            sha: commit.sha.clone(),
            timestamp: "x".into(),
            author: "joe".into(),
            spec_id: "FR-1".into(),
            req_type: "FR".into(),
            kind,
        };
        diff_modified(&before, &after, &mk, &mut out);
        assert_eq!(out.len(), 1);
        match &out[0].kind {
            EventKind::TagsChange { added, removed } => {
                assert_eq!(added, &vec!["c".to_string()]);
                assert_eq!(removed, &vec!["a".to_string()]);
            }
            _ => panic!("expected TagsChange"),
        }
    }

    #[test]
    fn comment_added_uses_last_author() {
        let before: Value = serde_yaml::from_str("comments: []").unwrap();
        let after: Value =
            serde_yaml::from_str("comments:\n  - author: alice\n    content: hi").unwrap();
        let mut out = Vec::new();
        let commit = CommitMeta {
            sha: "abc".into(),
            iso_timestamp: "2026-05-04T12:00:00Z".into(),
            git_author: "joe@example.com".into(),
        };
        let mk = |kind: EventKind| Event {
            sha: commit.sha.clone(),
            timestamp: "x".into(),
            author: "joe".into(),
            spec_id: "FR-1".into(),
            req_type: "FR".into(),
            kind,
        };
        diff_modified(&before, &after, &mk, &mut out);
        assert_eq!(out.len(), 1);
        match &out[0].kind {
            EventKind::CommentsAdded { count, author } => {
                assert_eq!(*count, 1);
                assert_eq!(author.as_deref(), Some("alice"));
            }
            _ => panic!("expected CommentsAdded"),
        }
    }

    /// `--status-changes` and `--comments` each narrow to one event kind;
    /// applied together they used to AND (impossible — always empty).
    /// TASK-1480 combines them with OR: either kind passes.
    // trace:TASK-1480 | ai:claude
    #[test]
    fn event_kind_allowed_combines_status_and_comments_with_or() {
        let status = EventKind::StatusChange {
            from: "Draft".into(),
            to: "Approved".into(),
        };
        let comment = EventKind::CommentsAdded {
            count: 1,
            author: Some("joe".into()),
        };
        let other = EventKind::TagsChange {
            added: vec!["x".into()],
            removed: vec![],
        };

        let neither = HistoryOpts { ..base_opts() };
        assert!(event_kind_allowed(&status, &neither));
        assert!(event_kind_allowed(&comment, &neither));
        assert!(event_kind_allowed(&other, &neither));

        let status_only = HistoryOpts {
            status_changes_only: true,
            ..base_opts()
        };
        assert!(event_kind_allowed(&status, &status_only));
        assert!(!event_kind_allowed(&comment, &status_only));
        assert!(!event_kind_allowed(&other, &status_only));

        let comments_only = HistoryOpts {
            comments_only: true,
            ..base_opts()
        };
        assert!(!event_kind_allowed(&status, &comments_only));
        assert!(event_kind_allowed(&comment, &comments_only));

        // The regression case: both set at once must OR, not AND.
        let both = HistoryOpts {
            status_changes_only: true,
            comments_only: true,
            ..base_opts()
        };
        assert!(event_kind_allowed(&status, &both));
        assert!(event_kind_allowed(&comment, &both));
        assert!(!event_kind_allowed(&other, &both));
    }

    /// A single spec (`--id` or the positional SPEC-ID alias) renders the
    /// status-progression view unless `--full`/`events` asked for the
    /// complete trail. BUG-1635: the choice no longer depends on whether a
    /// human or a script is reading; agent/piped callers used to get a
    /// one-row digest instead of the transitions.
    // trace:TASK-1480 | ai:claude
    // trace:BUG-1635 | ai:claude
    #[test]
    fn single_spec_view_selection_ignores_the_reader() {
        let opts = HistoryOpts {
            id_filter: Some("TASK-1".to_string()),
            events_mode: false,
            ..base_opts()
        };
        assert!(single_spec_uses_progress_view(&opts));

        // `--full` (or the `events` subcommand) always wins.
        let full = HistoryOpts {
            events_mode: true,
            ..opts.clone()
        };
        assert!(!single_spec_uses_progress_view(&full));

        // No id at all never takes the single-spec branch.
        let no_id = HistoryOpts {
            id_filter: None,
            ..opts
        };
        assert!(!single_spec_uses_progress_view(&no_id));
    }

    /// TASK-1480: build a small history for one spec — several status
    /// changes plus a comment on the same commit as one of the transitions
    /// — and drive it through `collect_filtered_events` the same way
    /// `aida history <SPEC-ID>` (status-progression, the default single-spec
    /// view) does: `status_changes_only = true`. Asserts the status changes
    /// come back (and only them — the comment is excluded), and that
    /// reversing the git-log order (as `run_single_spec_progress` does)
    /// produces the true chronological (oldest → newest) progression:
    /// Draft → Approved → In Progress → Done.
    #[test]
    fn status_progression_filters_to_status_changes_in_chronological_order() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let git = |args: &[&str]| {
            let out = ProcessCommand::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {:?} failed", args);
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        let write = |body: &str| {
            let path = root.join("objects/TASK/000/TASK-1.yaml");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, body).unwrap();
        };

        write("spec_id: TASK-1\ntitle: t\nstatus: Draft\ncomments: []\n");
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "add TASK-1"]);

        write("spec_id: TASK-1\ntitle: t\nstatus: Approved\ncomments: []\n");
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "approve TASK-1"]);

        // Same commit carries a status change AND a comment.
        write(
            "spec_id: TASK-1\ntitle: t\nstatus: In Progress\ncomments:\n  - author: joe\n    content: starting\n",
        );
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "start TASK-1"]);

        write(
            "spec_id: TASK-1\ntitle: t\nstatus: Done\ncomments:\n  - author: joe\n    content: starting\n",
        );
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "finish TASK-1"]);

        let opts = HistoryOpts {
            id_filter: Some("TASK-1".to_string()),
            status_changes_only: true,
            ..base_opts()
        };
        let (mut filtered, _, _) = collect_filtered_events(root, &opts).unwrap();
        assert_eq!(
            filtered.len(),
            3,
            "expected exactly the 3 status changes, got: {:?}",
            filtered.iter().map(|e| &e.kind).collect::<Vec<_>>()
        );
        assert!(
            filtered
                .iter()
                .all(|e| matches!(e.kind, EventKind::StatusChange { .. })),
            "the comment event must be filtered out"
        );

        // git-log order is newest-first; the progression view reverses it.
        filtered.reverse();
        let transitions: Vec<(String, String)> = filtered
            .iter()
            .map(|e| match &e.kind {
                EventKind::StatusChange { from, to } => (from.clone(), to.clone()),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            transitions,
            vec![
                ("Draft".to_string(), "Approved".to_string()),
                ("Approved".to_string(), "In Progress".to_string()),
                ("In Progress".to_string(), "Done".to_string()),
            ]
        );

        // The comment-only view picks up exactly the one comment event
        // instead.
        let comments_opts = HistoryOpts {
            id_filter: Some("TASK-1".to_string()),
            comments_only: true,
            ..base_opts()
        };
        let (comment_events, _, _) = collect_filtered_events(root, &comments_opts).unwrap();
        assert_eq!(comment_events.len(), 1);
        assert!(matches!(
            comment_events[0].kind,
            EventKind::CommentsAdded { .. }
        ));

        // Both flags together (the OR-fix): status changes AND the comment.
        let both_opts = HistoryOpts {
            id_filter: Some("TASK-1".to_string()),
            status_changes_only: true,
            comments_only: true,
            ..base_opts()
        };
        let (both_events, _, _) = collect_filtered_events(root, &both_opts).unwrap();
        assert_eq!(both_events.len(), 4);
    }

    /// TASK-1480: `aida history <SPEC-ID>` on a real spec that simply
    /// hasn't changed status yet must NOT error — it's a friendly empty
    /// view, not an invalid id. Calls `run_single_spec_progress` directly
    /// with an explicit output rather than the public `run()` dispatcher,
    /// whose output choice depends on the ambient `agent_output_mode()`
    /// (real TTY/env state).
    // trace:TASK-1480 | ai:claude
    #[test]
    fn run_single_spec_with_no_status_changes_yet_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let git = |args: &[&str]| {
            let out = ProcessCommand::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {:?} failed", args);
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        let path = root.join("objects/TASK/000/TASK-2.yaml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "spec_id: TASK-2\ntitle: quiet spec\nstatus: Draft\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "add TASK-2"]);

        let opts = HistoryOpts {
            id_filter: Some("TASK-2".to_string()),
            events_mode: false,
            status_changes_only: false,
            ..base_opts()
        };
        assert!(
            run_single_spec_progress(root, &opts, HistoryOutput::Human).is_ok(),
            "a real, quiet spec must not error"
        );
    }

    /// TASK-1480: an id that never appears anywhere in the orphan branch's
    /// history (well-formed shape, but nothing was ever committed under it)
    /// gets a clear error from the single-spec view, not a silent
    /// "(no recent activity)." Calls `run_single_spec_progress` directly —
    /// see the doc comment on the previous test for why.
    // trace:TASK-1480 | ai:claude
    #[test]
    fn run_single_spec_with_no_history_at_all_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let git = |args: &[&str]| {
            let out = ProcessCommand::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {:?} failed", args);
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        // Some unrelated history must exist so the branch isn't itself
        // empty (an empty orphan branch is a different, store-level case).
        let path = root.join("objects/TASK/000/TASK-1.yaml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "spec_id: TASK-1\ntitle: t\nstatus: Draft\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "add TASK-1"]);

        let opts = HistoryOpts {
            id_filter: Some("TASK-999999".to_string()),
            events_mode: false,
            ..base_opts()
        };
        let err = run_single_spec_progress(root, &opts, HistoryOutput::Human).unwrap_err();
        assert!(
            format!("{err:#}").to_lowercase().contains("not found"),
            "expected a not-found error, got: {err:#}"
        );
    }
}
