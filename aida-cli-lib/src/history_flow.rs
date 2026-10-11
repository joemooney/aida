//! The status flow: what bare `aida history` shows. One line per status
//! change, newest first, each with its time, SPEC-ID, old → new status and
//! title; a spec filed in the window shows as `(new) → <status>`.
//!
//! The flow is served only from the history index. When the index cannot
//! answer (switched off, behind the store, not built yet, ...), the command
//! prints the per-spec digest instead, with a one-line note on stderr
//! naming the cause and the fix. It never falls back to the per-commit git
//! walk: that walk takes seconds on a real store, and the default view has
//! to stay fast. Nor does it build or back-fill the index. It catches the
//! index up inline only when it is a few commits behind HEAD (at most
//! `history_cache::FLOW_CATCH_UP_CAP`), which costs about what the digest
//! does; a larger gap falls back with a note naming a cheap event-view
//! refresh. The event views and `aida cache rebuild --history` keep the
//! index current otherwise.
// trace:STORY-1650 | ai:claude

use crate::history::{self, Event, EventKind, HistoryOpts, HistoryOutput};
use crate::history_cache::{self, IndexMiss};
use anyhow::Result;
use colored::Colorize;
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::path::Path;

/// One line of the status flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FlowRow {
    /// Local time, `YYYY-MM-DD HH:MM`.
    pub(crate) when: String,
    pub(crate) id: String,
    /// The status before; `None` when the spec was filed in this commit.
    pub(crate) from: Option<String>,
    pub(crate) to: String,
    /// The spec's current title, or the title it was filed or deleted
    /// with when the spec no longer exists.
    pub(crate) title: String,
    /// `added` or `status_change`, the events feed's kind names.
    pub(crate) kind: &'static str,
    pub(crate) author: String,
    pub(crate) sha: String,
}

/// How the creation side of a transition reads.
pub(crate) const NEW_LABEL: &str = "(new)";

/// The index answer behind a served flow.
#[derive(Debug, Clone)]
pub(crate) struct FlowAnswer {
    pub(crate) rows: Vec<FlowRow>,
    pub(crate) window_exhausted: bool,
    pub(crate) tip: String,
}

/// The events-feed options the flow queries with. With none of `--to`,
/// `--from`, `--opened` the flow is every status change plus every
/// creation; with any of them, those selectors apply exactly as they do to
/// the events feed (so `--to completed` is just the transitions into
/// Completed, and `--opened` just the creations).
pub(crate) fn flow_opts(opts: &HistoryOpts) -> HistoryOpts {
    let mut o = opts.clone();
    o.events_mode = true;
    o.comments_only = false;
    if !o.opened_only && !history::has_transition_filter(&o) {
        o.status_changes_only = true;
        o.opened_only = true;
    }
    o
}

/// Answer the flow from the history index only. `Err` names why the index
/// could not; the caller then shows the digest. Never walks git history.
pub(crate) fn answer(
    store: &Path,
    opts: &HistoryOpts,
) -> std::result::Result<FlowAnswer, IndexMiss> {
    let answer = history_cache::serve_flow(store, &flow_opts(opts))?;
    Ok(FlowAnswer {
        rows: flow_rows(store, &answer.events),
        window_exhausted: answer.window_exhausted,
        tip: answer.tip,
    })
}

/// Turn index events into flow rows. Titles come from each spec's current
/// object file (one read per distinct spec), falling back to the title an
/// added or deleted event carries for a spec that no longer exists.
pub(crate) fn flow_rows(store: &Path, events: &[Event]) -> Vec<FlowRow> {
    let mut titles: HashMap<String, String> = HashMap::new();
    events
        .iter()
        .filter_map(|e| {
            let (from, to, kind, own_title) = match &e.kind {
                EventKind::StatusChange { from, to } => {
                    (Some(from.clone()), to.clone(), "status_change", None)
                }
                EventKind::Added { title, status, .. } => {
                    (None, status.clone(), "added", Some(title.as_str()))
                }
                _ => return None,
            };
            let current = titles
                .entry(e.spec_id.clone())
                .or_insert_with(|| current_title(store, &e.spec_id))
                .clone();
            let title = if current.is_empty() {
                own_title.unwrap_or_default().to_string()
            } else {
                current
            };
            Some(FlowRow {
                when: e.timestamp.clone(),
                id: e.spec_id.clone(),
                from,
                to,
                title,
                kind,
                author: e.author.clone(),
                sha: e.sha.clone(),
            })
        })
        .collect()
}

/// The spec's current title, or empty when its object file is gone.
fn current_title(store: &Path, spec_id: &str) -> String {
    let Ok(rel) = aida_core::object_store::relative_object_path(spec_id) else {
        return String::new();
    };
    let (status, title, _) = history::read_current(&store.join(rel));
    if status == "(deleted)" || status == "(parse-error)" {
        String::new()
    } else {
        title
    }
}

fn from_label(row: &FlowRow) -> &str {
    row.from.as_deref().unwrap_or(NEW_LABEL)
}

/// The human view: a dim header, then one aligned line per transition.
/// Always newline-terminated.
pub(crate) fn render_human(rows: &[FlowRow]) -> String {
    let when_w = rows.iter().map(|r| r.when.len()).max().unwrap_or(16).max(4);
    let id_w = rows.iter().map(|r| r.id.len()).max().unwrap_or(8).max(2);
    let from_w = rows
        .iter()
        .map(|r| from_label(r).chars().count())
        .max()
        .unwrap_or(5);
    let to_w = rows.iter().map(|r| r.to.chars().count()).max().unwrap_or(5);
    let change_w = from_w + 3 + to_w;
    let mut out = String::new();
    let header = format!(
        "{:<when_w$}  {:<id_w$}  {:<change_w$}  {}",
        "WHEN", "ID", "CHANGE", "TITLE"
    );
    out.push_str(&format!("{}\n", header.dimmed()));
    for r in rows {
        // Pad the plain text first, then colour it: width counts the
        // escape codes otherwise.
        let from = from_label(r);
        let from_cell = format!("{from:>from_w$}");
        let from_cell = if r.from.is_none() {
            from_cell.dimmed().to_string()
        } else {
            crate::status_display::paint_status(&from_cell, from).to_string()
        };
        let to_cell = format!("{:<to_w$}", r.to);
        let to_cell = crate::status_display::paint_status(&to_cell, &r.to).to_string();
        let id_cell = format!("{:<id_w$}", r.id);
        let line = format!(
            "{:<when_w$}  {}  {} \u{2192} {}  {}",
            r.when,
            id_cell.bold(),
            from_cell,
            to_cell,
            history::shorten(&r.title, 70).dimmed(),
        );
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// The TOON view for agent and piped output.
pub(crate) fn render_toon(rows: &[FlowRow]) -> String {
    let table: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            vec![
                r.when.clone(),
                r.id.clone(),
                from_label(r).to_string(),
                r.to.clone(),
                r.title.clone(),
            ]
        })
        .collect();
    format!(
        "view: history-flow\ncount: {}\n{}\n",
        rows.len(),
        crate::toon::table_raw(
            "transitions",
            &["when", "id", "from", "to", "title"],
            &table
        )
    )
}

/// The JSON view. `from` is `null` for a creation.
pub(crate) fn flow_json(answer: &FlowAnswer) -> JsonValue {
    let rows: Vec<JsonValue> = answer
        .rows
        .iter()
        .map(|r| {
            json!({
                "when": r.when,
                "id": r.id,
                "from": r.from,
                "to": r.to,
                "title": r.title,
                "kind": r.kind,
                "author": r.author,
                "sha": r.sha,
            })
        })
        .collect();
    json!({
        "view": "history-flow",
        "count": rows.len(),
        "source": "history-cache",
        "index_tip": answer.tip,
        "window_exhausted": answer.window_exhausted,
        "transitions": rows,
    })
}

/// The one-line stderr note printed when the flow falls back to the
/// digest: what you are looking at, why, and how to get the flow back.
/// The cheap refresh the behind note names: an event view, which catches
/// the index up within the per-query indexing budget and always keeps at
/// least one whole chunk, so each run moves the tip forward.
// trace:STORY-1650 | ai:claude
pub(crate) const REFRESH: &str = "aida history events --limit 1";

pub(crate) fn fallback_note(miss: IndexMiss) -> String {
    const LEAD: &str = "note: showing the per-spec digest instead of status changes because";
    const FIX: &str = "run `aida cache rebuild --history` to see status changes";
    match miss {
        IndexMiss::Disabled => format!(
            "{LEAD} the history index is switched off (AIDA_HISTORY_CACHE=0); \
             unset it and {FIX}"
        ),
        IndexMiss::NotBuilt => format!("{LEAD} the history index has not been built yet; {FIX}"),
        // trace:STORY-1650 | ai:claude
        // An event view catches the index up incrementally under its
        // budget, a chunk or more per run; a rebuild would take minutes.
        IndexMiss::Behind => format!(
            "{LEAD} the history index is too far behind the store to catch up here; \
             run `{REFRESH}` (again if this note persists) to see status changes"
        ),
        IndexMiss::Stuck => format!(
            "{LEAD} the history index is behind the store and cannot catch up on its own; {FIX}"
        ),
        IndexMiss::Filling => {
            format!("{LEAD} the history index has not reached this far back yet; {FIX}")
        }
        IndexMiss::Busy => format!(
            "{LEAD} another aida process is updating the history index; \
             try again in a moment, or {FIX}"
        ),
        IndexMiss::Unreadable => format!("{LEAD} the history index could not be read; {FIX}"),
        IndexMiss::Unprovable => {
            format!("{LEAD} the history index cannot vouch for the order of this window; {FIX}")
        }
    }
}

/// Render a served flow in `output`. Returns `(stdout, stderr)`; JSON and
/// the line views are complete documents.
pub(crate) fn render(answer: &FlowAnswer, output: HistoryOutput) -> Result<(String, String)> {
    if output == HistoryOutput::Json {
        let doc = flow_json(answer);
        return Ok((
            format!("{}\n", crate::cache_output::json_pretty(&doc)?),
            String::new(),
        ));
    }
    if answer.rows.is_empty() {
        return Ok((
            String::new(),
            format!("{}\n", "(no status changes match the filter)".dimmed()),
        ));
    }
    Ok(match output {
        HistoryOutput::Toon => (render_toon(&answer.rows), String::new()),
        _ => (render_human(&answer.rows), String::new()),
    })
}

/// Bare `aida history`: the flow from the index, else the digest plus a
/// note naming the cause.
pub(crate) fn run(store: &Path, opts: &HistoryOpts, output: HistoryOutput) -> Result<()> {
    match answer(store, opts) {
        Ok(answer) => {
            let (out, err) = render(&answer, output)?;
            print!("{out}");
            eprint!("{err}");
            if output != HistoryOutput::Json {
                history::print_window_exhausted_notice(
                    opts,
                    answer.window_exhausted,
                    answer.rows.len(),
                );
            }
            Ok(())
        }
        Err(miss) => {
            eprintln!("{}", fallback_note(miss));
            history::run_digest_view(store, opts, output)
        }
    }
}
