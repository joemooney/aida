//! `aida history` clap types, owned by `history.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum HistoryCommand {
    /// Switch to per-event chronological mode.
    // trace:STORY-1028 | ai:codex
    Events {
        /// Output structured event JSON.
        // trace:STORY-1477 | ai:codex — JSON belongs to event views, not management.
        #[clap(long, conflicts_with = "template")]
        json: bool,
    },
    /// List scoped templates and shadowing; builtins full/oneline are legacy mode aliases.
    // trace:STORY-1477 | ai:codex
    Templates {
        #[clap(subcommand)]
        cmd: Option<HistoryTemplatesCommand>,
    },
    /// Specs filed in the window, one row each: when it was filed, then
    /// its `aida list` row with its CURRENT status.
    ///
    /// Example: `aida history opened --since 2w --type bug`
    // trace:STORY-1664 | ai:claude
    Opened(HistoryViewArgs),
    /// Specs that entered Completed in the window, one row each: when,
    /// then its `aida list` row with its CURRENT status (a spec reopened
    /// since shows its status now).
    ///
    /// Example: `aida history completed --since 2w --type bug`
    // trace:STORY-1664 | ai:claude
    Completed(HistoryViewArgs),
    /// Specs that entered Rejected in the window, one row each, with
    /// their CURRENT status.
    ///
    /// Example: `aida history rejected --since 30d`
    // trace:STORY-1664 | ai:claude
    Rejected(HistoryViewArgs),
    /// Specs that entered Approved in the window, one row each, with
    /// their CURRENT status.
    ///
    /// Example: `aida history approved --since 3d --priority high`
    // trace:STORY-1664 | ai:claude
    Approved(HistoryViewArgs),
    /// Specs that entered a terminal status in the window: Completed,
    /// Rejected or Superseded. Archiving is housekeeping, not an outcome,
    /// so it does not count. One row each, with their CURRENT status.
    ///
    /// Example: `aida history closed --since 7d --tags cli`
    // trace:STORY-1664 | ai:claude
    Closed(HistoryViewArgs),
}

/// Options shared by the `aida history opened|completed|rejected|approved|closed`
/// views. The window is `--since` (default 7d) to `--until` (default now),
/// in the forms `aida history` accepts (`7d`, `2w`, an ISO date, RFC3339).
/// `--type`, `--limit`, `--max-commits`, `--author`, `--all`, `--archived`
/// and `--deferred` work as they do on `aida history`; `--type` matches the
/// spec's current type. Rows are newest first.
// trace:STORY-1664 | ai:claude
#[derive(clap::Args, Debug, Clone, Default)]
pub struct HistoryViewArgs {
    /// Only specs whose CURRENT priority is this.
    #[clap(long)]
    pub priority: Option<String>,
    /// Only specs carrying every one of these tags (comma-separated).
    #[clap(long, value_delimiter = ',')]
    pub tags: Vec<String>,
    /// Only specs whose CURRENT status is this. The status column is
    /// always the current status, not the status at the event.
    #[clap(long, value_name = "STATUS")]
    pub status: Option<String>,
    /// One row per matching event instead of one per spec: a spec
    /// completed, reopened and completed again appears twice.
    #[clap(long)]
    pub all_events: bool,
    /// Oldest first instead of newest first.
    #[clap(long)]
    pub oldest_first: bool,
    /// Output JSON (same as `--format json`).
    #[clap(long)]
    pub json: bool,
}

// trace:STORY-1664 | ai:claude
impl HistoryCommand {
    /// The list view this subcommand selects, with its options; `None` for
    /// `events` and `templates`.
    pub(crate) fn view(&self) -> Option<(crate::history_views::HistoryView, &HistoryViewArgs)> {
        use crate::history_views::HistoryView;
        match self {
            HistoryCommand::Opened(a) => Some((HistoryView::Opened, a)),
            HistoryCommand::Completed(a) => Some((HistoryView::Completed, a)),
            HistoryCommand::Rejected(a) => Some((HistoryView::Rejected, a)),
            HistoryCommand::Approved(a) => Some((HistoryView::Approved, a)),
            HistoryCommand::Closed(a) => Some((HistoryView::Closed, a)),
            HistoryCommand::Events { .. } | HistoryCommand::Templates { .. } => None,
        }
    }
}

// trace:STORY-1477 | ai:codex
#[derive(Subcommand, Debug)]
pub enum HistoryTemplatesCommand {
    /// Remove exactly user:NAME or project:NAME (explicit scope required).
    Rm { name: String },
}
