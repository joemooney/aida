//! `aida history opened|completed|rejected|approved|closed`: list-shaped
//! views of what changed in a time window. Each row is an `aida list` row
//! (the spec's CURRENT status, type, priority and title) with a leading
//! `when` column, the time of the matching event.
//!
//! The events come from the same index-or-walk path as `aida history
//! events`, so no event is decoded twice. The index query is not capped,
//! so a window it serves is complete. The git-walk fallback is capped at
//! `--max-commits`; when that cap cuts the window short, every output
//! format says so (`window truncated at <date>` / `truncated: true`).
// trace:STORY-1664 | ai:claude

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Result;
use colored::Colorize;

use crate::history::{self, Event, EventKind, HistoryOpts, HistorySource};

/// Which event a view selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryView {
    /// Spec creation: `when` is the time it was filed.
    Opened,
    /// Transitions into Completed.
    Completed,
    /// Transitions into Rejected.
    Rejected,
    /// Transitions into Approved.
    Approved,
    /// Transitions into any terminal status: Completed, Rejected or
    /// Superseded. Archived is housekeeping, not an outcome, so it is not
    /// one of them.
    Closed,
}

/// The statuses `closed` counts as an outcome.
const CLOSED_STATUSES: &[&str] = &["Completed", "Rejected", "Superseded"];

/// The window when `--since` is not given.
pub(crate) const VIEW_DEFAULT_SINCE: &str = "7d";

/// How many store commits the git-walk fallback reads by default. The walk
/// costs a few git calls per commit, and an active store makes thousands of
/// commits a week, so it stays bounded; the truncation notice names the
/// flag that widens it.
pub(crate) const VIEW_WALK_DEFAULT_MAX_COMMITS: usize = 250;

/// The index query's cap when `--max-commits` is not given: effectively
/// none. The index reads candidate commits from SQLite, so a whole window
/// costs one query.
const VIEW_INDEX_UNCAPPED: usize = 1 << 40;

impl HistoryView {
    pub(crate) fn name(self) -> &'static str {
        match self {
            HistoryView::Opened => "opened",
            HistoryView::Completed => "completed",
            HistoryView::Rejected => "rejected",
            HistoryView::Approved => "approved",
            HistoryView::Closed => "closed",
        }
    }

    /// The status a single-target view selects transitions into.
    fn target_status(self) -> Option<&'static str> {
        match self {
            HistoryView::Completed => Some("Completed"),
            HistoryView::Rejected => Some("Rejected"),
            HistoryView::Approved => Some("Approved"),
            HistoryView::Opened | HistoryView::Closed => None,
        }
    }

    /// Whether `kind` is an event this view selects. A transition must
    /// change the status: a respelling such as `In Progress → InProgress`
    /// is not one.
    fn matches(self, kind: &EventKind) -> bool {
        match (self, kind) {
            (HistoryView::Opened, EventKind::Added { .. }) => true,
            (HistoryView::Opened, _) => false,
            (view, EventKind::StatusChange { from, to }) => {
                let to_key = history::status_key(to);
                if history::status_key(from) == to_key {
                    return false;
                }
                match view.target_status() {
                    Some(target) => history::status_key(target) == to_key,
                    None => CLOSED_STATUSES
                        .iter()
                        .any(|s| history::status_key(s) == to_key),
                }
            }
            _ => false,
        }
    }
}

/// Filters on the spec's CURRENT fields, as `aida list` applies them.
#[derive(Debug, Clone, Default)]
pub(crate) struct ViewFilters {
    pub(crate) req_type: Option<String>,
    pub(crate) priority: Option<String>,
    /// Every listed tag must be present.
    pub(crate) tags: Vec<String>,
    /// A canonical status (from `resolve_status_filter`).
    pub(crate) status: Option<String>,
    pub(crate) archive: aida_core::ArchiveFilter,
    pub(crate) defer: aida_core::DeferFilter,
    pub(crate) include_meta: bool,
}

impl ViewFilters {
    fn keep(&self, s: &aida_core::RequirementSummary) -> bool {
        use aida_core::{ArchiveFilter, DeferFilter};
        if let Some(t) = &self.req_type {
            if !s.req_type.eq_ignore_ascii_case(t) {
                return false;
            }
        } else if !self.include_meta && s.req_type.eq_ignore_ascii_case("meta") {
            return false;
        }
        if let Some(p) = &self.priority {
            if !s.priority.eq_ignore_ascii_case(p) {
                return false;
            }
        }
        if let Some(st) = &self.status {
            if history::status_key(&s.status) != history::status_key(st) {
                return false;
            }
        }
        if !self
            .tags
            .iter()
            .all(|t| s.tags.iter().any(|have| have.eq_ignore_ascii_case(t)))
        {
            return false;
        }
        let archive_ok = match self.archive {
            ArchiveFilter::NonArchivedOnly => !s.archived,
            ArchiveFilter::ArchivedOnly => s.archived,
            ArchiveFilter::Both => true,
        };
        let defer_ok = match self.defer {
            DeferFilter::NonDeferredOnly => !s.deferred,
            DeferFilter::DeferredOnly => s.deferred,
            DeferFilter::Both => true,
        };
        archive_ok && defer_ok
    }
}

/// Everything a view query needs beyond the view itself.
#[derive(Debug, Clone, Default)]
pub(crate) struct ViewQuery {
    pub(crate) since: Option<String>,
    pub(crate) until: Option<String>,
    pub(crate) author: Option<String>,
    /// `--max-commits`: caps the index query and the git walk alike.
    pub(crate) max_commits: Option<usize>,
    pub(crate) limit: Option<usize>,
    pub(crate) all_events: bool,
    pub(crate) oldest_first: bool,
    pub(crate) filters: ViewFilters,
}

/// One output row: the event time plus the spec's current summary.
#[derive(Debug, Clone)]
pub(crate) struct ViewRow {
    pub(crate) when: String,
    pub(crate) summary: aida_core::RequirementSummary,
}

/// The matching events and how complete they are.
#[derive(Debug)]
pub(crate) struct CollectedEvents {
    /// Newest first.
    pub(crate) events: Vec<Event>,
    /// The commit cap cut the window short: older matching events may
    /// exist that were not read.
    pub(crate) truncated: bool,
    /// The time of the oldest commit read, when `truncated`.
    pub(crate) truncated_at: Option<String>,
    pub(crate) source: HistorySource,
    pub(crate) since_at: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) until_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// The event selection a view hands to the shared collector. The kind
/// selectors narrow what the index and the walk decode; [`HistoryView::matches`]
/// re-checks each event exactly.
fn event_opts(view: HistoryView, q: &ViewQuery, max_commits: usize) -> Result<HistoryOpts> {
    let to_status = view
        .target_status()
        .map(|s| history::resolve_status_filter("--to", s))
        .transpose()?;
    Ok(HistoryOpts {
        // Every matching event in the window: only the commit cap may cut
        // it, so a cut is always visible as `truncated`.
        limit: usize::MAX,
        max_commits,
        max_commits_explicit: q.max_commits.is_some(),
        events_mode: true,
        id_filter: None,
        // Type is filtered on the CURRENT summary, not the event.
        type_filter: None,
        author_filter: q.author.clone(),
        since: Some(
            q.since
                .clone()
                .unwrap_or_else(|| VIEW_DEFAULT_SINCE.to_string()),
        ),
        until: q.until.clone(),
        status_changes_only: view == HistoryView::Closed,
        shipped_only: false,
        to_status,
        from_status: None,
        opened_only: view == HistoryView::Opened,
        comments_only: false,
        oneline: false,
        archived_specs: HashSet::new(),
        archived_only_specs: None,
        deferred_specs: HashSet::new(),
        deferred_only_specs: None,
        exclude_meta: false,
    })
}

/// Collect the view's events: from the history index when it can answer,
/// else by walking the store's git log up to the commit cap.
pub(crate) fn collect_events(
    store_path: &Path,
    view: HistoryView,
    q: &ViewQuery,
) -> Result<CollectedEvents> {
    let index_cap = q.max_commits.unwrap_or(VIEW_INDEX_UNCAPPED);
    let opts = event_opts(view, q, index_cap)?;
    let (resolved, since_at, until_at) =
        history::resolve_history_window(&opts, chrono::Utc::now(), &chrono::Local)?;
    let enabled = crate::history_cache::cache_enabled();
    let served = if enabled {
        crate::history_cache::serve(store_path, &resolved)
    } else {
        None
    };
    let (events, truncated, source, cap) = match served {
        Some(answer) => (
            answer.events,
            answer.window_exhausted,
            HistorySource::Index { tip: answer.tip },
            index_cap,
        ),
        None => {
            let walk_cap = q.max_commits.unwrap_or(VIEW_WALK_DEFAULT_MAX_COMMITS);
            let walk_opts = HistoryOpts {
                max_commits: walk_cap,
                ..resolved.clone()
            };
            let (events, _, exhausted) =
                history::collect_filtered_events_git(store_path, &walk_opts)?;
            (
                events,
                exhausted,
                HistorySource::GitWalk { fallback: enabled },
                walk_cap,
            )
        }
    };
    let events: Vec<Event> = events
        .into_iter()
        .filter(|e| view.matches(&e.kind))
        .collect();
    let truncated_at = if truncated {
        oldest_commit_read(store_path, &resolved, cap)
    } else {
        None
    };
    Ok(CollectedEvents {
        events,
        truncated,
        truncated_at,
        source,
        since_at,
        until_at,
    })
}

/// The author time of the oldest commit a capped read covered: the
/// `cap`-th commit of the same `git log` window the walk reads.
fn oldest_commit_read(store_path: &Path, opts: &HistoryOpts, cap: usize) -> Option<String> {
    let mut args: Vec<String> = vec![
        "log".into(),
        "-n1".into(),
        format!("--skip={}", cap.saturating_sub(1)),
        "--format=%aI".into(),
    ];
    if let Some(s) = &opts.since {
        args.push(format!("--since={}", history::format_git_log_bound(s)));
    }
    if let Some(u) = &opts.until {
        args.push(format!("--until={}", history::format_git_log_bound(u)));
    }
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(store_path)
        .args(&args)
        .output()
        .ok()?;
    let iso = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !iso.is_empty()).then(|| history::human_timestamp(&iso))
}

/// Join events (newest first) to current summaries, apply the current-field
/// filters, keep one row per spec unless `all_events`, then order and limit.
/// A spec with no current summary (deleted since) has no current status to
/// show, so it has no row.
pub(crate) fn select_rows(
    events: &[Event],
    summaries: &[aida_core::RequirementSummary],
    q: &ViewQuery,
) -> Vec<ViewRow> {
    let mut by_id: HashMap<&str, &aida_core::RequirementSummary> = HashMap::new();
    for s in summaries {
        for id in [s.spec_id.as_deref(), s.agreed_id.as_deref()]
            .into_iter()
            .flatten()
        {
            by_id.entry(id).or_insert(s);
        }
    }
    let mut seen: HashSet<uuid::Uuid> = HashSet::new();
    let mut rows: Vec<ViewRow> = Vec::new();
    for e in events {
        let Some(summary) = by_id.get(e.spec_id.as_str()) else {
            continue;
        };
        if !q.filters.keep(summary) {
            continue;
        }
        if !q.all_events && !seen.insert(summary.id) {
            continue;
        }
        rows.push(ViewRow {
            when: e.timestamp.clone(),
            summary: (*summary).clone(),
        });
    }
    if q.oldest_first {
        rows.reverse();
    }
    if let Some(n) = q.limit {
        rows.truncate(n);
    }
    rows
}

/// The columns after `when`, in `aida list --fields` vocabulary.
const VIEW_FIELDS: &[&str] = &["id", "type", "status", "priority", "title"];

fn no_routing(_: &aida_core::RequirementSummary) -> (bool, bool, bool) {
    (false, false, false)
}

/// The human notice for a cut window; `None` when the window is whole.
pub(crate) fn truncation_notice(c: &CollectedEvents) -> Option<String> {
    if !c.truncated {
        return None;
    }
    let at = c.truncated_at.as_deref().unwrap_or("an unknown date");
    Some(format!(
        "window truncated at {at}: older events in the window were not read. \
         Pass a larger --max-commits to read further back."
    ))
}

/// The JSON document: list-shaped rows plus `when`, under the window and
/// completeness fields.
pub(crate) fn render_json(view: HistoryView, c: &CollectedEvents, rows: &[ViewRow]) -> String {
    let specs: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            let s = &r.summary;
            serde_json::json!({
                "when": r.when,
                "spec_id": s.agreed_id.as_deref().or(s.spec_id.as_deref()).unwrap_or(""),
                "title": s.title,
                "req_type": s.req_type,
                "type": s.req_type,
                "status": s.status,
                "priority": s.priority,
                "tags": s.tags,
            })
        })
        .collect();
    let doc = serde_json::json!({
        "view": view.name(),
        "since": c.since_at.map(|d| d.to_rfc3339()),
        "until": c.until_at.map(|d| d.to_rfc3339()),
        "source": c.source.as_str(),
        "truncated": c.truncated,
        "truncated_at": c.truncated_at,
        "count": rows.len(),
        "specs": specs,
    });
    serde_json::to_string_pretty(&doc).unwrap_or_default()
}

/// The TOON rendering: scalars, then the `aida list` row table with `when`
/// leading.
pub(crate) fn render_toon(view: HistoryView, c: &CollectedEvents, rows: &[ViewRow]) -> String {
    let mut out = format!("view: {}\n", view.name());
    if let Some(s) = c.since_at {
        out.push_str(&format!("since: {}\n", s.to_rfc3339()));
    }
    if let Some(u) = c.until_at {
        out.push_str(&format!("until: {}\n", u.to_rfc3339()));
    }
    out.push_str(&format!("truncated: {}\n", c.truncated));
    if let Some(at) = &c.truncated_at {
        out.push_str(&format!("truncated_at: {at}\n"));
    }
    out.push_str(&format!("count: {}\n", rows.len()));
    let mut fields: Vec<&str> = vec!["when"];
    fields.extend_from_slice(VIEW_FIELDS);
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            std::iter::once(r.when.clone())
                .chain(VIEW_FIELDS.iter().map(|f| {
                    crate::output_format::toon_list_cell(&r.summary, (false, false, false), f)
                }))
                .collect()
        })
        .collect();
    out.push_str(&crate::toon::table_raw("specs", &fields, &cells));
    out.push('\n');
    out
}

/// The human table: the `aida list --fields` table with a `When` column
/// in front.
pub(crate) fn render_human_table(rows: &[ViewRow]) -> Vec<String> {
    let summaries: Vec<aida_core::RequirementSummary> =
        rows.iter().map(|r| r.summary.clone()).collect();
    let fields: Vec<String> = VIEW_FIELDS.iter().map(|s| s.to_string()).collect();
    let lines = crate::build_list_fields_lines(&summaries, &fields, no_routing);
    let width = rows
        .iter()
        .map(|r| r.when.chars().count())
        .max()
        .unwrap_or(0)
        .max("When".len());
    lines
        .into_iter()
        .enumerate()
        .map(|(i, line)| match i {
            0 => format!("{} {line}", format!("{:<width$}", "When").bold()),
            1 => format!("{} {line}", "─".repeat(width)),
            n => format!("{:<width$} {line}", rows[n - 2].when),
        })
        .collect()
}

/// Run a view end to end and print it in the selected format.
pub(crate) fn run(
    backend: &aida_core::CachedGitBackend,
    store_path: &Path,
    view: HistoryView,
    q: &ViewQuery,
    json: bool,
) -> Result<()> {
    if !store_path.is_dir() {
        anyhow::bail!(
            "Not a git-canonical AIDA store: {}\n\
             aida history reads the orphan branch's git log; the legacy SQLite\n\
             backend has no per-edit history surface.",
            store_path.display()
        );
    }
    let collected = collect_events(store_path, view, q)?;
    let summaries = backend.list_summaries(&aida_core::ListFilter {
        archive: aida_core::ArchiveFilter::Both,
        defer: aida_core::DeferFilter::Both,
        ..Default::default()
    })?;
    let rows = select_rows(&collected.events, &summaries, q);
    if json {
        println!("{}", render_json(view, &collected, &rows));
        return Ok(());
    }
    let agent = crate::agent_output_mode();
    if agent {
        print!("{}", render_toon(view, &collected, &rows));
        return Ok(());
    }
    if let Some(line) =
        history::format_resolved_window(collected.since_at, collected.until_at, &chrono::Local)
    {
        println!("{}", line.dimmed());
    }
    if rows.is_empty() {
        println!("(no specs {} in this window)", view.name());
    } else {
        for line in render_human_table(&rows) {
            println!("{line}");
        }
    }
    if let Some(notice) = truncation_notice(&collected) {
        println!("{}", notice.yellow());
    }
    if let Some(footer) = history::fallback_footer_text(&collected.source, agent) {
        eprintln!("{}", footer.dimmed());
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/story_1664_history_views_tests.rs"]
mod tests;
