//! Launcher dashboard — four-region layout (STORY-244).
//!
//! Top tabs (role switcher), left nav (Queue/Backlog/.../action verbs),
//! middle list (items for the selected nav section), right preview
//! (`aida show <ID>` output for the highlighted row). The bottom status
//! strip is owned by [`crate::launcher`] which renders this dashboard.
//!
//! The dashboard is pure data + a render fn — fetch builds the model,
//! launcher feeds keystrokes that mutate it and call render again.
//!
//! trace:STORY-244 | ai:claude

use crate::nav::{self, NavSection, NavState};
use crate::theme::Theme;
use anyhow::Result;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// Which role the dashboard is filtering for. Cycled by `r` or the Tab
/// key; rendered as a row of pill-style chips at the top of the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RoleTab {
    #[default]
    Implementer,
    Reviewer,
    Dialog,
}

impl RoleTab {
    pub fn as_str(self) -> &'static str {
        match self {
            RoleTab::Implementer => "implementer",
            RoleTab::Reviewer => "reviewer",
            RoleTab::Dialog => "dialog",
        }
    }

    // User-facing tab label. Distinct from `as_str()` (the role IDENTIFIER passed
    // to `--role`, where the deprecated `dialog` alias must keep working): the
    // canonical display token is `advisor` per TASK-586, so the Dialog tab renders
    // as "advisor" everywhere it is shown to a human.
    // trace:BUG-620
    pub fn label(self) -> &'static str {
        match self {
            RoleTab::Dialog => "advisor",
            other => other.as_str(),
        }
    }

    pub fn cycle_next(self) -> RoleTab {
        match self {
            RoleTab::Implementer => RoleTab::Reviewer,
            RoleTab::Reviewer => RoleTab::Dialog,
            RoleTab::Dialog => RoleTab::Implementer,
        }
    }

    pub fn cycle_prev(self) -> RoleTab {
        match self {
            RoleTab::Implementer => RoleTab::Dialog,
            RoleTab::Reviewer => RoleTab::Implementer,
            RoleTab::Dialog => RoleTab::Reviewer,
        }
    }
}

/// One row in the middle list. `id` is what the launcher's Enter handler
/// echoes back into the emitted Intent (spec id, PR number, session id);
/// `title` and `status` are display-only. `kind` lets the dashboard pick
/// the right Intent constructor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListRow {
    pub id: String,
    pub title: String,
    pub status: String,
    pub kind: RowKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// Queue head — Enter starts a fresh `aida queue work <id>` session.
    Queued,
    /// Backlog item (Approved / Planned) — Enter starts `aida queue work`.
    Backlog,
    /// Already-completed spec — Enter shows it, no work to spawn.
    History,
    /// PR row — Enter shells out to `gh pr view <number>`.
    Pr,
    /// Recorded Claude conversation — Enter resumes it.
    Session,
    /// Action verb selected via the left nav action block.
    #[allow(dead_code)]
    Action,
    // --- Blocked-board reason rows (STORY-686). Each carries the reason's
    // Enter-dispatch action; see `launcher::act_on_row`. trace:STORY-686
    /// In-flight spec — Enter is info-only (shows the spec).
    ReasonInFlight,
    /// Blocked-by-dependency spec — Enter shows the blocked spec.
    ReasonBlocked,
    /// NeedsAttention spec — Enter launches `aida findings` triage.
    ReasonNeedsAttention,
    /// Awaiting-review spec — Enter opens the PR / shows the spec.
    ReasonAwaitingReview,
    /// Needs-an-answer spec — Enter launches the `aida questions` flow.
    ReasonNeedsAnswer,
    /// Needs-approval (Draft) spec — Enter approves via `aida edit … --status approved`.
    ReasonNeedsApproval,
    /// Advisor-backlog spec — Approved-but-not-queued, the advisor's pending
    /// queue surfaced in the needs-approval group (TASK-901). Enter routes it
    /// to the work queue (`aida queue add <id>`) rather than approving it (it
    /// is already approved). trace:TASK-901 | ai:claude
    ReasonAdvisorBacklog,
    /// Live intake-proposal spec — a candidate in the headless `aida intake`
    /// fence, filled async / on demand (TASK-904). Enter fires `aida intake`
    /// scoped to that spec so the operator runs the actual cold-boot advisor
    /// disposition.
    // trace:TASK-904 | ai:claude
    ReasonIntakeProposal,
    /// Deferred spec — Enter undefers it (`aida undefer <id>`).
    ReasonDeferred,
}

/// Which of the two panes currently owns the keyboard. Up/Down act on the
/// focused pane (Nav: section selection; List: row selection); Enter/Left
/// move focus Nav→List, Right/Esc move it List→Nav. The renderer paints the
/// focused pane's selected row with the accent fill and dims the Nav
/// selection once focus has moved into the list, so which pane is active is
/// always visually obvious. trace:STORY-685 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pane {
    /// The left section selector (Backlog / History / PRs / Sessions / …).
    #[default]
    Nav,
    /// The middle row list for the selected section.
    List,
}

/// Read-only state for the bottom status strip.
#[derive(Debug, Clone, Default)]
pub struct AmbientState {
    pub role: String,
    pub queue_depth: usize,
    pub dialog_state: &'static str,
}

/// Per-launcher-run dashboard model.
///
/// Not `Clone`: it now owns an in-flight `gh pr list` channel
/// ([`Self::pr_rx`], a `Receiver` that is intentionally non-cloneable) so
/// the PRs panel can fill asynchronously without ever blocking the cursor.
/// The model is mutated in place by the launcher loop and never cloned, so
/// dropping the derive costs nothing.
// trace:BUG-619 | ai:claude
#[derive(Debug, Default)]
pub struct DashboardModel {
    pub role: RoleTab,
    pub nav: NavState,
    /// Which pane has keyboard focus. Defaults to [`Pane::Nav`]: Up/Down
    /// move the section selection until the user presses Enter/Left to
    /// drop into the list. trace:STORY-685 | ai:claude
    pub focus: Pane,
    pub rows: Vec<ListRow>,
    pub selected: usize,
    pub ambient: AmbientState,
    /// Cached preview body, keyed by row id. Filled on first paint per row
    /// and reused for subsequent moves. Spec rows cache the raw `aida show`
    /// body as [`PreviewBody::Markdown`] so the pane renders headings/bold/
    /// lists/code instead of verbatim CLI text (STORY-689 slice 1); PR,
    /// session, action, and error previews stay [`PreviewBody::Plain`].
    pub preview_cache: HashMap<String, PreviewBody>,
    /// Notice line above the middle list (e.g. "loading PRs…", "`gh`
    /// failed — empty PR list"). Cleared on a successful refetch.
    pub notice: Option<String>,
    /// Active palette — every styled span in the dashboard resolves its
    /// color through this rather than naming a literal. Defaults to the
    /// Catppuccin Mocha palette; the launcher overrides it from
    /// `[tui] theme`. trace:TASK-256 | ai:claude
    pub theme: Theme,
    /// The blocked-board's classified items — every open spec assigned to
    /// exactly one reason-group (STORY-686). Refreshed from the cache-fast
    /// sources on launch and on `g`; the reason-group nav sections read
    /// their rows out of this. trace:STORY-686 | ai:claude
    pub board: Vec<crate::board::ClassifiedItem>,
    /// Per-reason counts derived from [`Self::board`] — drives the Nav
    /// `(count) · owner` suffix and the empty-reason dim. trace:STORY-686
    pub reason_counts: HashMap<&'static str, usize>,
    /// Whether the cheap board inputs have been composed at least once. The
    /// first reason-section render triggers the load; subsequent moves reuse
    /// the cached classification until a `g` refresh. trace:STORY-686
    pub board_loaded: bool,
    /// Whether the lazy `gh pr list` awaiting-review fill has run for the
    /// current board snapshot. The cheap rows paint first; the PR rows merge
    /// in on the next refetch of the awaiting-review group. trace:STORY-686
    pub prs_filled: bool,
    /// Last-known open-PR rows, fetched off the UI thread. The PRs panel and
    /// the board's awaiting-review group both read from this cache so a
    /// single async `gh pr list` feeds both surfaces. Painted immediately on
    /// navigation; refreshed when the background fetch lands. trace:BUG-619
    pub pr_cache: Vec<ListRow>,
    /// In-flight `gh pr list` receiver. Present while a background fetch is
    /// running; the launcher polls [`Self::poll_prs`] to merge the result and
    /// clear it. `None` means no fetch is in flight. trace:BUG-619
    pub pr_rx: Option<std::sync::mpsc::Receiver<Vec<ListRow>>>,
    /// Whether the open-PR cache has been populated at least once this run.
    /// Drives the "loading…" vs "refreshing…" notice wording and lets the
    /// PRs panel avoid re-spawning a fetch that is already cached + idle.
    /// trace:BUG-619
    pub pr_loaded: bool,
    /// Last-known live `aida intake` candidate ids, fetched off the UI thread
    /// (TASK-904). The needs-approval group merges these in as intake-proposal
    /// rows. Populated by the on-demand `i` keystroke; empty until then.
    // trace:TASK-904
    pub intake_cache: Vec<String>,
    /// In-flight `aida intake --dry-run` receiver. Present while a background
    /// intake fence fetch is running; the launcher polls [`Self::poll_intake`]
    /// to merge the result and clear it. `None` means no fetch is in flight.
    // trace:TASK-904
    pub intake_rx: Option<std::sync::mpsc::Receiver<Vec<String>>>,
    /// Whether the intake candidate cache has been populated at least once this
    /// run — drives the "running…" vs "refreshing…" notice and the idempotent
    /// merge guard.
    // trace:TASK-904
    pub intake_loaded: bool,
    /// Whether the loaded intake candidates have been merged into the current
    /// board snapshot. Cleared on a board refresh / a fresh fetch so the
    /// proposals re-merge.
    // trace:TASK-904
    pub intake_filled: bool,
}

impl DashboardModel {
    /// Currently-highlighted row, or `None` when the list is empty.
    pub fn current_row(&self) -> Option<&ListRow> {
        self.rows.get(self.selected)
    }

    pub fn select_next(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.rows.len();
    }

    pub fn select_prev(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        let n = self.rows.len();
        self.selected = (self.selected + n - 1) % n;
    }

    /// Snap selection to row 0 after a refetch — keeps the cursor sane
    /// when the row set changed underneath.
    pub fn reset_selection(&mut self) {
        self.selected = 0;
    }

    /// Compose the cheap (non-network) board inputs, classify them, and
    /// cache the result + per-reason counts on the model. Cheap: only
    /// cache-fast `aida list …` reads and one `aida questions list` parse —
    /// never `aida status`. The `gh pr list` awaiting-review fill is
    /// deferred to [`Self::lazy_fill_prs`]. trace:STORY-686 | ai:claude
    pub fn refresh_board(&mut self) {
        let inputs = crate::board::fetch_inputs();
        self.board = crate::board::classify(&inputs);
        self.reason_counts = crate::board::counts(&self.board)
            .into_iter()
            .collect::<HashMap<_, _>>();
        self.board_loaded = true;
        self.prs_filled = false;
        // A fresh board snapshot drops the prior intake-proposal merge; re-arm
        // it so the cached candidates re-merge on the next read. trace:TASK-904
        self.intake_filled = false;
    }

    /// Merge the async-loaded open PRs into the awaiting-review board group.
    /// Idempotent per board snapshot via `prs_filled`; reads the rows from
    /// the off-thread [`Self::pr_cache`] (never shells out itself, so the
    /// cursor is never blocked) and kicks the background fetch when the cache
    /// has not been populated yet. PR rows are appended to the classified set
    /// as awaiting-review items, then the counts are recomputed.
    /// trace:STORY-686 trace:BUG-619 | ai:claude
    pub fn lazy_fill_prs(&mut self) {
        // Make sure a fetch is running / has run; the cursor never waits on it.
        self.ensure_prs_loading();
        if self.prs_filled || self.pr_cache.is_empty() {
            return;
        }
        self.prs_filled = true;
        // A PR row's `id` is the PR number; the head-ref SPEC mention is not
        // available here, so we surface every open PR as an awaiting-review
        // item. Existing Done-on-branch rows stay; PRs are appended with a
        // `pr:<n>` synthetic id so Enter opens the PR.
        for pr in &self.pr_cache {
            self.board.push(crate::board::ClassifiedItem {
                spec_id: format!("pr:{}", pr.id),
                title: pr.title.clone(),
                status: pr.status.clone(),
                reason: crate::board::Reason::AwaitingReview,
                advisor_backlog: false,
                intake_proposal: false,
                // Awaiting-review work is handed off, not parked → no park reason.
                // trace:STORY-703
                park_reason: None,
            });
        }
        self.reason_counts = crate::board::counts(&self.board)
            .into_iter()
            .collect::<HashMap<_, _>>();
    }

    /// Kick a background `gh pr list` fetch off the UI thread, unless one is
    /// already in flight or the cache has already been populated this run.
    /// The cursor never waits on the spawn — it returns immediately and the
    /// result lands later via [`Self::poll_prs`]. trace:BUG-619 | ai:claude
    pub fn ensure_prs_loading(&mut self) {
        if self.pr_rx.is_some() || self.pr_loaded {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // `fetch_open_pr_rows` already time-limits the `gh` shell-out, so
            // an offline / wedged `gh` yields an empty Vec rather than hanging
            // the worker. The receiver may have been dropped (re-entry); the
            // send error is benign.
            let _ = tx.send(crate::board::fetch_open_pr_rows());
        });
        self.pr_rx = Some(rx);
    }

    /// Force a fresh background `gh pr list` fetch — clears the cached rows,
    /// the "loaded" flag, and the awaiting-review fill marker so the next
    /// poll re-merges. Used by the launcher's explicit `g` refresh.
    /// trace:BUG-619 | ai:claude
    pub fn invalidate_prs(&mut self) {
        self.pr_loaded = false;
        self.prs_filled = false;
        self.pr_rx = None;
        self.ensure_prs_loading();
    }

    /// Non-blocking drain of the in-flight `gh pr list` fetch. Returns `true`
    /// when a result was just consumed (so the caller repaints): the rows are
    /// cached, the awaiting-review board fill is re-armed, and the PRs panel
    /// rows + notice are refreshed if that section is current. Returns `false`
    /// when no fetch is in flight or it has not finished yet — the cursor is
    /// never blocked on the `gh` round-trip. trace:BUG-619 | ai:claude
    pub fn poll_prs(&mut self) -> bool {
        let Some(rx) = self.pr_rx.as_ref() else {
            return false;
        };
        match rx.try_recv() {
            Ok(rows) => {
                self.pr_cache = rows;
                self.pr_loaded = true;
                self.pr_rx = None;
                // Re-arm the awaiting-review merge so the now-loaded PRs land
                // in the board group on its next read.
                self.prs_filled = false;
                // If the PRs panel is the live section, refresh its rows +
                // notice in place so the freshly-loaded PRs appear without a
                // navigation. trace:BUG-619 | ai:claude
                if self.nav.current() == NavSection::Prs {
                    self.rows = self.pr_cache.clone();
                    self.notice = if self.pr_cache.is_empty() {
                        Some("no open PRs (or `gh` unavailable)".into())
                    } else {
                        None
                    };
                }
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                // Worker died without sending (should not happen — the closure
                // always sends). Treat as an empty result so we stop polling.
                self.pr_loaded = true;
                self.pr_rx = None;
                false
            }
        }
    }

    // --- Live intake-proposal source (TASK-904). The heavyweight `aida intake`
    // candidate fence (`aida intake --dry-run`, ~1s store-load) is fetched off
    // the UI thread, mirroring the `gh pr list` awaiting-review lazy fill. The
    // cursor never waits on it. trace:TASK-904 | ai:claude

    /// Merge the async-loaded intake candidate ids into the needs-approval
    /// board group. Idempotent per board snapshot via `intake_filled`; reads
    /// the ids from the off-thread [`Self::intake_cache`] (never shells out
    /// itself). Recomputes the per-reason counts after merging.
    // trace:TASK-904 | ai:claude
    pub fn merge_intake(&mut self) {
        if self.intake_filled || self.intake_cache.is_empty() {
            return;
        }
        self.intake_filled = true;
        crate::board::merge_intake_proposals(&mut self.board, &self.intake_cache);
        self.reason_counts = crate::board::counts(&self.board)
            .into_iter()
            .collect::<HashMap<_, _>>();
    }

    /// Kick a background `aida intake --dry-run` fetch off the UI thread,
    /// unless one is already in flight. Unlike the PR fetch (armed on
    /// navigation), this is armed only by the explicit `i` keystroke so the
    /// heavyweight pass never fires on its own. The cursor returns immediately;
    /// the result lands via [`Self::poll_intake`].
    // trace:TASK-904 | ai:claude
    pub fn request_intake(&mut self) {
        if self.intake_rx.is_some() {
            return;
        }
        // A re-request re-arms the merge so refreshed candidates re-land.
        self.intake_filled = false;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // The receiver may have been dropped (re-entry); the send error is
            // benign.
            let _ = tx.send(crate::board::fetch_intake_proposal_ids());
        });
        self.intake_rx = Some(rx);
        self.notice = Some(if self.intake_loaded {
            "refreshing intake proposals… (heavyweight advisor fence)".into()
        } else {
            "running intake fence… (heavyweight advisor pass, ~1s)".into()
        });
    }

    /// Non-blocking drain of the in-flight `aida intake` fetch. Returns `true`
    /// when a result was just consumed (so the caller repaints): the candidate
    /// ids are cached, the needs-approval merge is re-armed and applied, and
    /// the notice is updated. Returns `false` when no fetch is in flight or it
    /// has not finished yet — the cursor is never blocked.
    // trace:TASK-904
    pub fn poll_intake(&mut self) -> bool {
        let Some(rx) = self.intake_rx.as_ref() else {
            return false;
        };
        match rx.try_recv() {
            Ok(ids) => {
                self.intake_cache = ids;
                self.intake_loaded = true;
                self.intake_rx = None;
                self.intake_filled = false;
                // Make sure the board exists, then merge in place so the
                // proposals appear without a navigation. trace:TASK-904
                if !self.board_loaded {
                    self.refresh_board();
                }
                self.merge_intake();
                self.notice = Some(if self.intake_cache.is_empty() {
                    "intake fence empty — nothing for the advisor to weigh".into()
                } else {
                    format!(
                        "{} live intake proposal(s) merged into needs-approval",
                        self.intake_cache.len()
                    )
                });
                // If the needs-approval group is the live section, re-read its
                // rows so the merged proposals show immediately. trace:TASK-904
                if self.nav.current() == NavSection::Reason(crate::board::Reason::NeedsApproval) {
                    self.rows =
                        crate::board::rows_for(&self.board, crate::board::Reason::NeedsApproval);
                }
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.intake_loaded = true;
                self.intake_rx = None;
                false
            }
        }
    }
}

/// Fetch the rows for `section` and assemble a fresh dashboard model. The
/// caller supplies the launch scope (for the Sessions section) and the
/// persistent dialog session id (so the dialog tab can surface a resume
/// row). trace:STORY-244 | ai:claude
pub fn fetch(
    role: RoleTab,
    section: NavSection,
    launch_scope: Option<&str>,
    dialog_session_id: Option<&str>,
) -> DashboardModel {
    let mut model = DashboardModel {
        role,
        ambient: AmbientState {
            role: role.as_str().to_string(),
            dialog_state: if dialog_session_id.is_some() {
                "ready"
            } else {
                "idle"
            },
            ..AmbientState::default()
        },
        ..DashboardModel::default()
    };
    model.nav.select(section);
    refetch_rows(&mut model, launch_scope, dialog_session_id);
    model
}

/// Reload the middle list for the currently-selected nav section, keeping
/// the rest of the model intact. Used by the launcher's `g` refresh and
/// by re-entry after a dispatched command exits.
pub fn refetch_rows(
    model: &mut DashboardModel,
    launch_scope: Option<&str>,
    dialog_session_id: Option<&str>,
) {
    model.notice = None;
    let section = model.nav.current();
    // PRs panel: paint immediately from the off-thread cache and kick a
    // background `gh pr list` if needed — never a synchronous shell-out, so
    // navigating into the panel never blocks the cursor. The rows fill in
    // when the worker lands (drained by the launcher's `poll_prs`). The notice
    // reflects loading vs refreshing vs ready. Handled before the match so the
    // (slow, network) path can never reach the UI thread. trace:BUG-619
    if section == NavSection::Prs {
        refetch_prs(model);
        return;
    }
    model.rows = match section {
        // Blocked-board reason-group: read from the cached classification,
        // loading it on first touch. The awaiting-review group triggers the
        // lazy `gh pr list` fill so its PR rows appear without stalling the
        // cheap reasons. trace:STORY-686 | ai:claude
        NavSection::Reason(reason) => {
            if !model.board_loaded {
                model.refresh_board();
            }
            if reason == crate::board::Reason::AwaitingReview {
                model.lazy_fill_prs();
            }
            // The needs-approval group folds in any already-loaded live intake
            // proposals (TASK-904); the heavyweight fetch itself is armed only
            // by the `i` keystroke, never here, so navigation stays cheap.
            if reason == crate::board::Reason::NeedsApproval {
                model.merge_intake();
            }
            crate::board::rows_for(&model.board, reason)
        }
        NavSection::Queue => fetch_queue(model),
        NavSection::Backlog => fetch_status(model, &["approved", "planned"]),
        NavSection::History => fetch_status(model, &["completed"]),
        NavSection::Sessions => fetch_sessions(launch_scope, dialog_session_id),
        _ => Vec::new(),
    };
    model.reset_selection();
    model.ambient.queue_depth = model
        .rows
        .iter()
        .filter(|r| r.kind == RowKind::Queued)
        .count();
}

/// Paint the PRs panel from the off-thread cache and arm a background fetch
/// when needed — the cursor-non-blocking replacement for the old synchronous
/// `fetch_prs()` shell-out. The notice distinguishes the first load
/// ("loading PRs…") from a re-fetch over stale rows ("refreshing PRs…") from
/// a settled-but-empty result ("no open PRs…"). The actual rows land later via
/// [`DashboardModel::poll_prs`]. trace:BUG-619 | ai:claude
fn refetch_prs(model: &mut DashboardModel) {
    model.ensure_prs_loading();
    model.notice = if model.pr_rx.is_some() {
        Some(if model.pr_loaded {
            "refreshing PRs…".into()
        } else {
            "loading PRs…".into()
        })
    } else if model.pr_cache.is_empty() {
        Some("no open PRs (or `gh` unavailable)".into())
    } else {
        None
    };
    model.rows = model.pr_cache.clone();
    model.reset_selection();
    model.ambient.queue_depth = 0;
}

/// Queue section rows: shell out to the cache-fast `aida queue list --json`
/// rather than `overlay::fetch`, which ran the full `aida status`
/// worktree/process scan (~3s) just for the queue head. `queue list --json`
/// reads straight from the cache (sub-100ms) and emits the same
/// {spec_id,title,status} shape `parse_list_json` already handles.
// trace:BUG-616 | ai:claude
fn fetch_queue(model: &mut DashboardModel) -> Vec<ListRow> {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(["queue", "list", "--json"]);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    let Ok(out) = cmd.output() else {
        model.notice = Some("queue fetch failed".into());
        return Vec::new();
    };
    if !out.status.success() {
        model.notice = Some("queue fetch failed".into());
        return Vec::new();
    }
    parse_list_json(&out.stdout)
        .into_iter()
        .map(|row| ListRow {
            id: row.spec_id,
            title: row.title,
            status: row.status,
            kind: RowKind::Queued,
        })
        .collect()
}

/// Most-recent rows shown per Backlog/History panel. `aida list --json`
/// returns recency-first, so taking the first N keeps a large History
/// (hundreds of Completed specs) light to build, render, and scroll. The
/// full set is always available via `aida list`. trace:TASK-897 | ai:claude
const PANEL_ROW_LIMIT: usize = 100;

/// Backlog / History rows: shell out to `aida list --status <csv> --json`,
/// capped to the most-recent [`PANEL_ROW_LIMIT`]. Sets a truncation notice
/// when the full set is larger so the cap is discoverable. trace:TASK-897
fn fetch_status(model: &mut DashboardModel, statuses: &[&str]) -> Vec<ListRow> {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(["list", "--status", &statuses.join(","), "--json"]);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    let Ok(out) = cmd.output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let kind = if statuses.contains(&"completed") {
        RowKind::History
    } else {
        RowKind::Backlog
    };
    let all = parse_list_json(&out.stdout);
    let total = all.len();
    if total > PANEL_ROW_LIMIT {
        model.notice = Some(format!(
            "showing {PANEL_ROW_LIMIT} most-recent of {total} — `aida list` for all"
        ));
    }
    all.into_iter()
        .take(PANEL_ROW_LIMIT)
        .map(|row| ListRow {
            id: row.spec_id,
            title: row.title,
            status: row.status,
            kind,
        })
        .collect()
}

/// Compact list-JSON row — what `aida list --json` emits. The
/// `queued`/`in_flight`/`blocked` routing flags are carried (TASK-670): the
/// blocked-board classifier (STORY-686) reads `in_flight`/`blocked` to group
/// each spec by why it's not moving. `blocked` is only populated when the
/// shell-out passes `--blocked`; the field defaults to `false` otherwise.
/// trace:STORY-244 trace:STORY-686 | ai:claude
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ListJsonRow {
    pub spec_id: String,
    pub title: String,
    #[serde(default)]
    pub req_type: String,
    pub status: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub queued: bool,
    #[serde(default)]
    pub in_flight: bool,
    #[serde(default)]
    pub blocked: bool,
    /// The revisit trigger of a deferred spec (`aida defer --until "<cond>"`),
    /// when present — the content the cockpit's advisor-backlog panel surfaces
    /// inline so each parked item shows WHY it is parked. `None` for any spec
    /// that is not on the deferred shelf.
    // trace:STORY-703 | ai:claude
    #[serde(default)]
    pub deferred_until: Option<String>,
}

/// Parse `aida list --json` output. Tolerant: a JSON shape mismatch
/// returns an empty list rather than crashing the launcher.
pub fn parse_list_json(bytes: &[u8]) -> Vec<ListJsonRow> {
    serde_json::from_slice(bytes).unwrap_or_default()
}

// The PRs panel no longer shells out to `gh pr list` on the UI thread. The
// fetch runs off-thread via [`crate::board::fetch_open_pr_rows`] (kicked by
// `DashboardModel::ensure_prs_loading`) and lands through `poll_prs`, so
// navigating into the panel never blocks the cursor. trace:BUG-619 | ai:claude

#[derive(Debug, Clone, Deserialize)]
struct PrJson {
    number: u64,
    title: String,
    #[serde(rename = "headRefName", default)]
    head_ref_name: String,
    #[serde(rename = "statusCheckRollup", default)]
    status_check_rollup: Vec<serde_json::Value>,
}

/// Public re-export of [`parse_pr_json`] so the blocked-board's lazy
/// awaiting-review fill ([`crate::board::fetch_open_pr_rows`]) reuses the
/// same `gh pr list` JSON → [`ListRow`] mapping rather than re-deriving it.
/// trace:STORY-686 | ai:claude
pub fn parse_pr_json_rows(bytes: &[u8]) -> Vec<ListRow> {
    parse_pr_json(bytes)
}

fn parse_pr_json(bytes: &[u8]) -> Vec<ListRow> {
    let parsed: Vec<PrJson> = serde_json::from_slice(bytes).unwrap_or_default();
    parsed
        .into_iter()
        .map(|p| ListRow {
            id: p.number.to_string(),
            title: format!("PR #{}  {}  ({})", p.number, p.title, p.head_ref_name),
            status: rollup_state(&p.status_check_rollup),
            kind: RowKind::Pr,
        })
        .collect()
}

fn rollup_state(rollup: &[serde_json::Value]) -> String {
    if rollup.is_empty() {
        return "—".into();
    }
    let conclusions: Vec<&str> = rollup
        .iter()
        .filter_map(|v| v.get("conclusion").and_then(|c| c.as_str()))
        .collect();
    if conclusions.contains(&"FAILURE") {
        "failure".into()
    } else if conclusions.iter().all(|c| *c == "SUCCESS") {
        "green".into()
    } else {
        "mixed".into()
    }
}

/// Run a command with a wall-clock timeout. Kills the child on expiry
/// so the launcher doesn't block on a wedged `gh`.
fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> Result<std::process::Output> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()?;
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            if let Some(mut s) = child.stdout.take() {
                let _ = s.read_to_end(&mut stdout);
            }
            if let Some(mut s) = child.stderr.take() {
                let _ = s.read_to_end(&mut stderr);
            }
            return Ok(std::process::Output {
                status,
                stdout,
                stderr,
            });
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            anyhow::bail!("timed out after {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Sessions section rows: recorded Claude conversations for the launch
/// scope (TASK-112 `--list-sessions`). When the dialog tab also has a
/// stored session id, prepend it as a row at the top.
fn fetch_sessions(launch_scope: Option<&str>, dialog_session_id: Option<&str>) -> Vec<ListRow> {
    let mut rows: Vec<ListRow> = Vec::new();
    if let Some(id) = dialog_session_id {
        rows.push(ListRow {
            id: id.to_string(),
            title: format!("dialog session  {}", short_id(id)),
            status: "resume".into(),
            kind: RowKind::Session,
        });
    }
    if let Some(scope) = launch_scope {
        let exe = crate::app::aida_exe();
        let mut cmd = Command::new(&exe);
        cmd.args(["queue", "work", scope, "--list-sessions"]);
        if let Ok(cwd) = std::env::current_dir() {
            cmd.current_dir(cwd);
        }
        if let Ok(out) = cmd.output() {
            if out.status.success() {
                for (sid, label) in
                    crate::picker::parse_list_sessions(&String::from_utf8_lossy(&out.stdout))
                {
                    rows.push(ListRow {
                        id: sid.clone(),
                        title: if label.is_empty() {
                            format!("{scope}  {}", short_id(&sid))
                        } else {
                            format!("{scope}  {}  ({label})", short_id(&sid))
                        },
                        status: "resume".into(),
                        kind: RowKind::Session,
                    });
                }
            }
        }
    }
    rows
}

fn short_id(s: &str) -> String {
    s.chars().take(8).collect()
}

/// A cached preview body. Spec rows hold their raw `aida show --no-git`
/// stdout as [`PreviewBody::Markdown`] so the pane renders it through the
/// markdown renderer; everything else (PR/session/action text, error
/// messages) stays [`PreviewBody::Plain`] and renders verbatim.
//
// The markdown variant deliberately stores the *raw* string, not a
// pre-built ratatui `Text`: `tui_markdown::from_str` returns a `Text`
// borrowed from its input, so the owning `String` has to live in the
// cache and the `Text` is rebuilt (cheaply) at paint time in
// `render_preview`. trace:STORY-689 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewBody {
    /// Raw markdown source (a spec body); rendered via `tui_markdown`.
    Markdown(String),
    /// A spec preview with its STRUCTURED fields kept separate from the body
    /// so the fields can be color-coded with semantic colors and the body
    /// still rendered through the markdown renderer.
    //
    // trace:STORY-691 | ai:claude
    Spec(SpecPreview),
    /// Pre-split plain-text lines, rendered verbatim.
    Plain(Vec<String>),
}

/// The structured fields + markdown body of a spec, sourced from
/// `aida show <id> --json --no-git` (typed fields, not a re-parse of the
/// human `aida show` stdout). `--no-git` preserves the BUG-616 preview-perf
/// floor.
//
// trace:STORY-691 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SpecPreview {
    pub spec_id: String,
    pub title: String,
    pub status: String,
    #[serde(default)]
    pub priority: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// The markdown body (spec description), rendered below the header fields.
    #[serde(default)]
    pub description: String,
}

/// Run `aida show <id>` for the currently-highlighted row and cache the
/// stdout into the preview pane. Called once per cursor move; cached
/// for the lifetime of the dashboard model.
pub fn ensure_preview(model: &mut DashboardModel) {
    let Some(row) = model.current_row().cloned() else {
        return;
    };
    if model.preview_cache.contains_key(&row.id) {
        return;
    }
    let body = match row.kind {
        RowKind::Queued | RowKind::Backlog | RowKind::History => preview_via_show(&row.id),
        RowKind::Pr => PreviewBody::Plain(preview_via_gh_pr(&row.id)),
        RowKind::Session => PreviewBody::Plain(vec![
            format!("Session id: {}", row.id),
            String::new(),
            "Enter resumes this conversation via `claude --resume`.".into(),
        ]),
        RowKind::Action => PreviewBody::Plain(vec![row.title.clone()]),
        // Blocked-board reason rows preview the spec body, except the
        // synthetic PR rows (`pr:<n>`) which preview the PR. trace:STORY-686
        RowKind::ReasonInFlight
        | RowKind::ReasonBlocked
        | RowKind::ReasonNeedsAttention
        | RowKind::ReasonAwaitingReview
        | RowKind::ReasonNeedsAnswer
        | RowKind::ReasonNeedsApproval
        | RowKind::ReasonAdvisorBacklog
        | RowKind::ReasonIntakeProposal
        | RowKind::ReasonDeferred => {
            if let Some(num) = row.id.strip_prefix("pr:") {
                PreviewBody::Plain(preview_via_gh_pr(num))
            } else {
                preview_via_show(&row.id)
            }
        }
    };
    model.preview_cache.insert(row.id, body);
}

fn preview_via_show(id: &str) -> PreviewBody {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    // BUG-613-adjacent: the row preview only needs the cached spec body, not the
    // per-spec git-linkage walk (commits/files/branch/PR) that makes `aida show`
    // ~1-2s per uncached row. `--no-git` drops it to sub-200ms. trace:BUG-616 | ai:claude
    //
    // STORY-691: fetch the TYPED fields via `--json` (not a re-parse of the
    // human `aida show` stdout) so the structured header (status / priority /
    // tags) can be color-coded with semantic, themeable colors above the
    // markdown body. trace:STORY-691 | ai:claude
    cmd.args(["show", id, "--no-git", "--json"]);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    match cmd.output() {
        Ok(o) if o.status.success() => parse_spec_preview(&o.stdout).map_or_else(
            // A JSON shape mismatch degrades to the raw stdout as markdown
            // rather than dropping the preview entirely.
            || PreviewBody::Markdown(String::from_utf8_lossy(&o.stdout).into_owned()),
            PreviewBody::Spec,
        ),
        Ok(o) => PreviewBody::Plain(vec![
            format!("`aida show {id}` failed:"),
            String::from_utf8_lossy(&o.stderr).to_string(),
        ]),
        Err(e) => PreviewBody::Plain(vec![format!("could not run `aida show`: {e}")]),
    }
}

/// Parse `aida show <id> --json` output into a [`SpecPreview`]. Tolerant: a
/// JSON shape mismatch returns `None` so the caller can degrade gracefully
/// rather than crash the launcher.
//
// trace:STORY-691 | ai:claude
fn parse_spec_preview(bytes: &[u8]) -> Option<SpecPreview> {
    serde_json::from_slice(bytes).ok()
}

fn preview_via_gh_pr(number: &str) -> Vec<String> {
    let mut cmd = Command::new("gh");
    cmd.args(["pr", "view", number]);
    match run_with_timeout(&mut cmd, Duration::from_secs(5)) {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|s| s.to_string())
            .collect(),
        _ => vec![format!("PR #{number}  (gh pr view failed or unauthorized)")],
    }
}

/// Render the dashboard into `frame`. The bottom status strip is added
/// by the launcher's outer paint, not here.
pub fn render(frame: &mut Frame, model: &DashboardModel) {
    let rows = Layout::vertical([
        Constraint::Length(1), // top tab bar
        Constraint::Min(0),    // body
        Constraint::Length(1), // hint/help row above the bottom strip
    ])
    .split(frame.area());

    render_tabs(frame, rows[0], model);

    let body = Layout::horizontal([
        Constraint::Length(20),     // left nav
        Constraint::Percentage(40), // middle list
        Constraint::Percentage(60), // right preview
    ])
    .split(rows[1]);

    nav::render(
        frame,
        body[0],
        &model.nav,
        &model.theme,
        model.focus,
        &model.reason_counts,
    );
    render_list(frame, body[1], model);
    render_preview(frame, body[2], model);
    render_hint_row(frame, rows[2], model);
}

fn render_tabs(frame: &mut Frame, area: Rect, model: &DashboardModel) {
    let theme = &model.theme;
    let mut spans: Vec<Span> = Vec::new();
    for r in [RoleTab::Implementer, RoleTab::Reviewer, RoleTab::Dialog] {
        let label = format!("  {}  ", r.label());
        if r == model.role {
            spans.push(Span::styled(
                format!("[{}]", label.trim()),
                Style::default()
                    .fg(theme.on_accent)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(label, Style::default().fg(theme.dim)));
        }
        spans.push(Span::raw("  "));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_list(frame: &mut Frame, area: Rect, model: &DashboardModel) {
    let theme = &model.theme;
    // STORY-703: on the advisor-backlog panel (the needs-approval reason, which
    // holds the drafts + advisor-backlog rows) annotate the title with the total
    // advisor-queue depth so the advisor's pending queue is a visible number, not
    // a black box. trace:STORY-703 | ai:claude
    let title = if model.nav.current() == NavSection::Reason(crate::board::Reason::NeedsApproval) {
        format!(
            " {} · advisor queue: {} ",
            section_title(model.nav.current()),
            crate::board::advisor_queue_depth(&model.board)
        )
    } else {
        format!(" {} ", section_title(model.nav.current()))
    };
    let block = Block::bordered()
        .border_style(Style::default().fg(theme.border))
        .title(title);
    let inner_w = area.width.saturating_sub(2) as usize;
    let inner_h = area.height.saturating_sub(2) as usize;

    if model.rows.is_empty() {
        let body = model
            .notice
            .clone()
            .unwrap_or_else(|| "(nothing here)".to_string());
        frame.render_widget(Paragraph::new(body).block(block), area);
        return;
    }

    let start = if inner_h > 0 && model.selected >= inner_h {
        model.selected - inner_h + 1
    } else {
        0
    };
    let mut lines: Vec<Line> = Vec::new();
    for (i, row) in model.rows.iter().enumerate().skip(start).take(inner_h) {
        let marker = if i == model.selected { "▸ " } else { "  " };
        let text = format!("{marker}{}  {}  [{}]", row.id, row.title, row.status);
        let clipped: String = text.chars().take(inner_w.max(4)).collect();
        // The active-pane row gets the full accent fill; when focus is in
        // the Nav pane the list's own selection shows as a quieter
        // accent-foreground underline so the cursor position is still
        // visible without competing with the focused Nav highlight.
        // trace:STORY-685 | ai:claude
        let style = if i == model.selected {
            if model.focus == Pane::List {
                Style::default()
                    .fg(theme.on_accent)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::UNDERLINED)
            }
        } else {
            Style::default().fg(theme.fg)
        };
        lines.push(Line::from(Span::styled(clipped, style)));
    }
    if let Some(notice) = &model.notice {
        lines.insert(
            0,
            Line::from(Span::styled(
                notice.clone(),
                Style::default().fg(theme.warn),
            )),
        );
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn section_title(s: NavSection) -> &'static str {
    match s {
        // The board reason-groups title the list with their reason label.
        // trace:STORY-686 | ai:claude
        NavSection::Reason(r) => r.label(),
        NavSection::Queue => "Queue",
        NavSection::Backlog => "Backlog",
        NavSection::History => "History",
        NavSection::Prs => "Pull Requests",
        NavSection::Sessions => "Sessions",
        _ => "Actions",
    }
}

fn render_preview(frame: &mut Frame, area: Rect, model: &DashboardModel) {
    let theme = &model.theme;
    let block = Block::bordered()
        .border_style(Style::default().fg(theme.border))
        .title(" Preview ");
    let text = match model
        .current_row()
        .and_then(|r| model.preview_cache.get(&r.id))
    {
        Some(body) => preview_text(body, theme),
        None => Text::from(Line::from(Span::styled(
            "(no selection)",
            Style::default().fg(theme.dim),
        ))),
    };
    frame.render_widget(
        Paragraph::new(text).block(block).wrap(Wrap { trim: false }),
        area,
    );
}

/// Convert a cached [`PreviewBody`] into the ratatui [`Text`] the preview
/// pane renders. Markdown bodies go through `tui_markdown` (headings, bold/
/// italic, lists, fenced code → styled spans); plain bodies render verbatim,
/// one [`Line`] per stored string. An empty markdown body degrades to a dim
/// placeholder rather than a blank pane.
// trace:STORY-689 | ai:claude
fn preview_text<'a>(body: &'a PreviewBody, theme: &Theme) -> Text<'a> {
    match body {
        PreviewBody::Markdown(src) => {
            if src.trim().is_empty() {
                Text::from(Line::from(Span::styled(
                    "(empty)",
                    Style::default().fg(theme.dim),
                )))
            } else {
                tui_markdown::from_str(src)
            }
        }
        PreviewBody::Spec(spec) => spec_preview_text(spec, theme),
        PreviewBody::Plain(lines) => Text::from(
            lines
                .iter()
                .map(|s| Line::from(s.clone()))
                .collect::<Vec<_>>(),
        ),
    }
}

/// Render a [`SpecPreview`] as the color-coded structured header (id + title,
/// then status / priority / tags painted with semantic THEMEABLE colors) above
/// the markdown-rendered body. The status and priority values resolve their
/// color through [`Theme::status_color`] / [`Theme::priority_color`], which
/// mirror the CLI's `paint_status` palette but route through the active theme
/// so a custom palette recolors them. Degrades gracefully on empty/missing
/// fields.
//
// trace:STORY-691 | ai:claude
fn spec_preview_text<'a>(spec: &'a SpecPreview, theme: &Theme) -> Text<'a> {
    let mut lines: Vec<Line<'a>> = Vec::new();

    // id + title header.
    lines.push(Line::from(vec![
        Span::styled(
            spec.spec_id.as_str(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(spec.title.as_str(), Style::default().fg(theme.fg)),
    ]));

    // Field label color — dim, so the colored VALUE carries the signal.
    let label = Style::default().fg(theme.dim);

    if !spec.status.trim().is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Status:   ", label),
            Span::styled(
                spec.status.as_str(),
                Style::default()
                    .fg(theme.status_color(&spec.status))
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    if !spec.priority.trim().is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Priority: ", label),
            Span::styled(
                spec.priority.as_str(),
                Style::default().fg(theme.priority_color(&spec.priority)),
            ),
        ]));
    }
    if !spec.tags.is_empty() {
        let mut spans: Vec<Span<'a>> = vec![Span::styled("Tags:     ", label)];
        for (i, tag) in spec.tags.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(tag.as_str(), Style::default().fg(theme.info)));
        }
        lines.push(Line::from(spans));
    }

    // Blank separator before the markdown body, then the body itself rendered
    // through the same markdown path as a plain Markdown preview.
    let body = spec.description.trim();
    if !body.is_empty() {
        lines.push(Line::default());
        let mut text = Text::from(lines);
        text.extend(tui_markdown::from_str(body));
        text
    } else {
        Text::from(lines)
    }
}

fn render_hint_row(frame: &mut Frame, area: Rect, model: &DashboardModel) {
    // trace:STORY-685 | ai:claude — surface the focus-aware nav map and the
    // current pane so the two-pane model is discoverable.
    let pane = match model.focus {
        Pane::Nav => "nav",
        Pane::List => "list",
    };
    let move_hint = match model.focus {
        Pane::Nav => "↑↓ section · enter/→ list",
        Pane::List => "↑↓ row · enter act · ←/esc nav",
    };
    let text = format!(
        "role:{} · queue:{} · dialog:{} · focus:{}    {} · tab role · g refresh · : palette · ? help · q quit",
        model.ambient.role, model.ambient.queue_depth, model.ambient.dialog_state, pane, move_hint
    );
    frame.render_widget(
        Paragraph::new(Span::styled(text, Style::default().fg(model.theme.dim))),
        area,
    );
}

/// Returns the project root directory of the cwd — used to resolve the
/// dashboard's preview cache scope. Kept simple; matches the launcher's
/// `ensure_project_context` resolution.
#[allow(dead_code)]
pub fn project_root_of(cwd: &std::path::Path) -> Option<PathBuf> {
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        if d.join(".git").exists() || d.join(".aida").join("config.toml").is_file() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_model(rows: Vec<ListRow>) -> DashboardModel {
        DashboardModel {
            role: RoleTab::Implementer,
            rows,
            ..DashboardModel::default()
        }
    }

    #[test]
    fn role_tab_cycles() {
        assert_eq!(RoleTab::Implementer.cycle_next(), RoleTab::Reviewer);
        assert_eq!(RoleTab::Reviewer.cycle_next(), RoleTab::Dialog);
        assert_eq!(RoleTab::Dialog.cycle_next(), RoleTab::Implementer);
        assert_eq!(RoleTab::Implementer.cycle_prev(), RoleTab::Dialog);
    }

    #[test]
    fn dialog_tab_displays_advisor_but_keeps_identifier() {
        // The user-facing tab label is the canonical "advisor" (TASK-586)...
        assert_eq!(RoleTab::Dialog.label(), "advisor");
        assert_eq!(RoleTab::Implementer.label(), "implementer");
        assert_eq!(RoleTab::Reviewer.label(), "reviewer");
        // ...while the role IDENTIFIER passed to `--role` stays the deprecated
        // alias so back-compat routing keeps working.
        // trace:BUG-620
        assert_eq!(RoleTab::Dialog.as_str(), "dialog");
    }

    #[test]
    fn selection_wraps_both_ways() {
        let mut m = fixture_model(vec![
            ListRow {
                id: "STORY-1".into(),
                title: "a".into(),
                status: "Approved".into(),
                kind: RowKind::Queued,
            },
            ListRow {
                id: "STORY-2".into(),
                title: "b".into(),
                status: "Approved".into(),
                kind: RowKind::Queued,
            },
        ]);
        assert_eq!(m.selected, 0);
        m.select_prev();
        assert_eq!(m.selected, 1);
        m.select_next();
        assert_eq!(m.selected, 0);
    }

    #[test]
    fn empty_selection_is_noop() {
        let mut m = fixture_model(vec![]);
        m.select_next();
        m.select_prev();
        assert_eq!(m.selected, 0);
        assert!(m.current_row().is_none());
    }

    #[test]
    fn parse_list_json_round_trips() {
        let json = br#"[
            {"spec_id":"STORY-244","title":"TUI pivot","req_type":"story","status":"approved","tags":[]},
            {"spec_id":"TASK-256","title":"theming","req_type":"task","status":"approved","tags":["epic-26"]}
        ]"#;
        let rows = parse_list_json(json);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].spec_id, "STORY-244");
        assert_eq!(rows[1].tags, vec!["epic-26".to_string()]);
    }

    #[test]
    fn parse_list_json_tolerates_garbage() {
        // A malformed payload becomes an empty list; the launcher must
        // not crash on a future-shape mismatch.
        assert!(parse_list_json(b"not json").is_empty());
        assert!(parse_list_json(b"").is_empty());
    }

    #[test]
    fn parse_pr_json_collapses_rollup() {
        let json = br#"[
            {"number":42,"title":"a fix","headRefName":"feat/x",
             "statusCheckRollup":[{"conclusion":"SUCCESS"},{"conclusion":"SUCCESS"}]},
            {"number":43,"title":"another","headRefName":"feat/y",
             "statusCheckRollup":[{"conclusion":"FAILURE"}]}
        ]"#;
        let rows = parse_pr_json(json);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "42");
        assert_eq!(rows[0].status, "green");
        assert_eq!(rows[1].status, "failure");
    }

    #[test]
    fn rollup_state_classifies() {
        let s = serde_json::json!({"conclusion":"SUCCESS"});
        let f = serde_json::json!({"conclusion":"FAILURE"});
        assert_eq!(rollup_state(&[]), "—");
        assert_eq!(rollup_state(&[s.clone(), s.clone()]), "green");
        assert_eq!(rollup_state(&[s.clone(), f.clone()]), "failure");
    }

    #[test]
    fn dashboard_role_tab_filters_rows() {
        // The role tab is purely a display chip today — the data
        // fetchers will eventually filter by it. For now we verify the
        // chip cycle moves the displayed role on a fixture model.
        let mut m = fixture_model(vec![]);
        assert_eq!(m.role, RoleTab::Implementer);
        m.role = m.role.cycle_next();
        assert_eq!(m.role, RoleTab::Reviewer);
        m.ambient.role = m.role.as_str().to_string();
        assert_eq!(m.ambient.role, "reviewer");
    }

    fn pr_row(n: u64) -> ListRow {
        ListRow {
            id: n.to_string(),
            title: format!("PR #{n}  thing  (feat/x)"),
            status: "green".into(),
            kind: RowKind::Pr,
        }
    }

    // BUG-619: navigating into the PRs panel must paint immediately from the
    // off-thread cache and never shell out on the UI thread. `refetch_prs`
    // pre-loaded (so `ensure_prs_loading` no-ops) paints the cached rows and
    // clears the notice without blocking. trace:BUG-619
    #[test]
    fn refetch_prs_paints_cache_without_blocking() {
        let mut m = fixture_model(vec![]);
        m.nav.select(NavSection::Prs);
        // Simulate a prior async fetch that already landed.
        m.pr_cache = vec![pr_row(7), pr_row(8)];
        m.pr_loaded = true;
        m.pr_rx = None;
        refetch_prs(&mut m);
        assert_eq!(m.rows, m.pr_cache);
        assert!(m.notice.is_none(), "ready cache shows no notice");
        assert_eq!(m.selected, 0);
    }

    // A loaded-but-empty result degrades to an "unavailable" notice, never a
    // hang. trace:BUG-619
    #[test]
    fn refetch_prs_empty_cache_shows_unavailable_notice() {
        let mut m = fixture_model(vec![]);
        m.nav.select(NavSection::Prs);
        m.pr_loaded = true; // ensure_prs_loading no-ops; no thread spawned
        m.pr_rx = None;
        refetch_prs(&mut m);
        assert!(m.rows.is_empty());
        assert_eq!(
            m.notice.as_deref(),
            Some("no open PRs (or `gh` unavailable)")
        );
    }

    // `poll_prs` is a non-blocking channel drain: a landed result is cached,
    // the awaiting-review fill is re-armed, and (when the PRs panel is live)
    // the rows + notice refresh in place. trace:BUG-619
    #[test]
    fn poll_prs_merges_landed_result_on_prs_panel() {
        let mut m = fixture_model(vec![]);
        m.nav.select(NavSection::Prs);
        let (tx, rx) = std::sync::mpsc::channel();
        m.pr_rx = Some(rx);
        m.prs_filled = true; // pretend a prior (empty) fill ran

        // Nothing sent yet → no consume, cursor not blocked.
        assert!(!m.poll_prs());
        assert!(m.pr_rx.is_some());

        // Worker lands → consumed, cached, panel refreshed, fill re-armed.
        tx.send(vec![pr_row(11)]).unwrap();
        assert!(m.poll_prs());
        assert!(m.pr_rx.is_none());
        assert!(m.pr_loaded);
        assert!(!m.prs_filled, "awaiting-review fill is re-armed");
        assert_eq!(m.pr_cache.len(), 1);
        assert_eq!(m.rows, m.pr_cache);
        assert!(m.notice.is_none());
    }

    // With no fetch in flight, polling is a cheap no-op. trace:BUG-619
    #[test]
    fn poll_prs_no_fetch_is_noop() {
        let mut m = fixture_model(vec![]);
        assert!(!m.poll_prs());
    }

    // The awaiting-review board group merges the async-loaded PR cache
    // (never shelling out on the read path) into the classified set as
    // `pr:<n>` awaiting-review items. trace:BUG-619 trace:STORY-686
    #[test]
    fn lazy_fill_prs_merges_cached_rows_into_board() {
        let mut m = fixture_model(vec![]);
        m.pr_loaded = true; // ensure_prs_loading no-ops; no gh thread
        m.pr_cache = vec![pr_row(21)];
        m.lazy_fill_prs();
        assert!(m.prs_filled);
        assert!(
            m.board
                .iter()
                .any(|c| c.spec_id == "pr:21" && c.reason == crate::board::Reason::AwaitingReview),
            "cached PR surfaces as an awaiting-review board item"
        );
        // Idempotent: a second fill does not duplicate.
        m.lazy_fill_prs();
        assert_eq!(m.board.iter().filter(|c| c.spec_id == "pr:21").count(), 1);
    }

    // --- Live intake-proposal source (TASK-904). ---

    // `request_intake` arms an off-thread fetch and shows a running notice;
    // re-requesting while in flight is a no-op (no second worker). trace:TASK-904
    #[test]
    fn request_intake_arms_once() {
        let mut m = fixture_model(vec![]);
        m.request_intake();
        assert!(m.intake_rx.is_some());
        assert!(m.notice.as_deref().unwrap().contains("intake"));
        // Re-request while in flight keeps the same receiver (no second spawn).
        let ptr_before = m.intake_rx.as_ref().map(|r| r as *const _);
        m.request_intake();
        let ptr_after = m.intake_rx.as_ref().map(|r| r as *const _);
        assert_eq!(ptr_before, ptr_after);
    }

    // `poll_intake` is a non-blocking channel drain: a landed candidate set is
    // cached, merged into the needs-approval group, and the notice updated.
    // trace:TASK-904
    #[test]
    fn poll_intake_merges_landed_candidates_into_needs_approval() {
        let mut m = fixture_model(vec![]);
        // A draft already on the board that the intake fence also weighs.
        m.board = vec![crate::board::ClassifiedItem {
            spec_id: "STORY-1".into(),
            title: "t".into(),
            status: "Draft".into(),
            reason: crate::board::Reason::NeedsApproval,
            advisor_backlog: false,
            intake_proposal: false,
            park_reason: None,
        }];
        m.board_loaded = true;
        let (tx, rx) = std::sync::mpsc::channel();
        m.intake_rx = Some(rx);

        // Nothing sent yet → no consume.
        assert!(!m.poll_intake());
        assert!(m.intake_rx.is_some());

        // Worker lands → consumed, cached, merged.
        tx.send(vec!["STORY-1".to_string(), "STORY-2".to_string()])
            .unwrap();
        assert!(m.poll_intake());
        assert!(m.intake_rx.is_none());
        assert!(m.intake_loaded);
        assert_eq!(m.intake_cache.len(), 2);
        // STORY-1 upgraded in place; STORY-2 appended as a fresh proposal.
        let upgraded = m.board.iter().find(|c| c.spec_id == "STORY-1").unwrap();
        assert!(upgraded.intake_proposal);
        assert!(m
            .board
            .iter()
            .any(|c| c.spec_id == "STORY-2" && c.intake_proposal));
        assert!(m.notice.as_deref().unwrap().contains("intake proposal"));
    }

    // An empty fence lands a clear notice and merges nothing. trace:TASK-904
    #[test]
    fn poll_intake_empty_fence_notice() {
        let mut m = fixture_model(vec![]);
        m.board_loaded = true;
        let (tx, rx) = std::sync::mpsc::channel();
        m.intake_rx = Some(rx);
        tx.send(vec![]).unwrap();
        assert!(m.poll_intake());
        assert!(m.intake_cache.is_empty());
        assert_eq!(
            m.notice.as_deref(),
            Some("intake fence empty — nothing for the advisor to weigh")
        );
    }

    // With no fetch in flight, polling is a cheap no-op. trace:TASK-904
    #[test]
    fn poll_intake_no_fetch_is_noop() {
        let mut m = fixture_model(vec![]);
        assert!(!m.poll_intake());
    }

    // --- STORY-689 slice 1: markdown preview rendering ---

    // Flatten a rendered `Text` back to one plain string per line so a test can
    // assert on the *content* the renderer kept, independent of styling.
    fn text_to_lines(text: &Text) -> Vec<String> {
        text.lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn markdown_preview_renders_structure() {
        // A markdown body is parsed into ratatui `Text`: the heading text and
        // list items survive, the inline emphasis delimiters (`**`) are
        // consumed (the word keeps a BOLD modifier instead), and a bullet
        // marker is emitted for the list. This proves the body went through
        // the markdown renderer rather than being shown verbatim.
        // trace:STORY-689
        let body =
            PreviewBody::Markdown("# Title\n\nSome **bold** prose.\n\n- first\n- second\n".into());
        let theme = Theme::default();
        let text = preview_text(&body, &theme);
        let lines = text_to_lines(&text);
        let joined = lines.join("\n");
        assert!(
            joined.contains("Title"),
            "heading text survives: {joined:?}"
        );
        assert!(
            joined.contains("first") && joined.contains("second"),
            "list items survive: {joined:?}"
        );
        // The bold delimiters are consumed — the literal `**bold**` is gone,
        // but the word remains, carrying a BOLD style modifier.
        assert!(
            !joined.contains("**bold**"),
            "bold markers consumed: {joined:?}"
        );
        let bold_span_exists = text.lines.iter().any(|line| {
            line.spans.iter().any(|s| {
                s.content.contains("bold") && s.style.add_modifier.contains(Modifier::BOLD)
            })
        });
        assert!(bold_span_exists, "emphasized word carries a BOLD modifier");
    }

    #[test]
    fn empty_markdown_preview_degrades_gracefully() {
        // An empty/whitespace body yields a dim placeholder rather than a
        // blank pane. trace:STORY-689
        let body = PreviewBody::Markdown("   \n  ".to_string());
        let theme = Theme::default();
        let lines = text_to_lines(&preview_text(&body, &theme));
        assert_eq!(lines, vec!["(empty)".to_string()]);
    }

    #[test]
    fn plain_preview_renders_verbatim() {
        // Non-spec previews (PR/session/error) keep their lines exactly,
        // including markdown-looking characters. trace:STORY-689
        let body = PreviewBody::Plain(vec!["PR #21".to_string(), "# not a heading".to_string()]);
        let theme = Theme::default();
        let lines = text_to_lines(&preview_text(&body, &theme));
        assert_eq!(
            lines,
            vec!["PR #21".to_string(), "# not a heading".to_string()]
        );
    }

    // --- STORY-691 slice 2: color-coded structured fields ---

    fn sample_spec() -> SpecPreview {
        SpecPreview {
            spec_id: "STORY-691".into(),
            title: "color the preview".into(),
            status: "In Progress".into(),
            priority: "High".into(),
            tags: vec!["tui".into(), "theme".into()],
            description: "# Body\n\nSome **bold** prose.\n".into(),
        }
    }

    // Find the painted style of the first span whose content contains `needle`.
    fn span_fg(text: &Text, needle: &str) -> Option<ratatui::style::Color> {
        text.lines.iter().find_map(|line| {
            line.spans
                .iter()
                .find(|s| s.content.contains(needle))
                .and_then(|s| s.style.fg)
        })
    }

    #[test]
    fn spec_preview_colors_fields_with_theme_semantics() {
        // The structured status / priority values resolve to the active
        // theme's semantic color (the same map the CLI uses), and the markdown
        // body still renders below. trace:STORY-691
        let theme = crate::theme::DARK;
        let body = PreviewBody::Spec(sample_spec());
        let text = preview_text(&body, &theme);

        // status value painted with the in-progress slot; priority with high.
        assert_eq!(
            span_fg(&text, "In Progress"),
            Some(theme.status_color("In Progress")),
            "status value carries the themed status color",
        );
        assert_eq!(
            span_fg(&text, "High"),
            Some(theme.priority_color("High")),
            "priority value carries the themed priority color",
        );

        // The markdown body rendered below: heading text + emphasized word
        // survive (delimiters consumed).
        let joined = text_to_lines(&text).join("\n");
        assert!(joined.contains("Body"), "body heading survives: {joined:?}");
        assert!(
            joined.contains("tui") && joined.contains("theme"),
            "tags shown"
        );
        assert!(!joined.contains("**bold**"), "bold markers consumed");
    }

    #[test]
    fn spec_preview_status_color_is_themeable() {
        // Switching themes recolors the same status — proving the color is not
        // hardcoded but resolved through the palette. trace:STORY-691
        let dark_body = PreviewBody::Spec(sample_spec());
        let mocha_body = PreviewBody::Spec(sample_spec());
        let dark = preview_text(&dark_body, &crate::theme::DARK);
        let mocha = preview_text(&mocha_body, &crate::theme::CATPPUCCIN_MOCHA);
        assert_ne!(
            span_fg(&dark, "In Progress"),
            span_fg(&mocha, "In Progress"),
            "the status color tracks the active theme",
        );
    }

    #[test]
    fn spec_preview_degrades_on_missing_fields() {
        // Empty priority/tags/body are simply omitted — no blank "Priority:"
        // line, no crash. trace:STORY-691
        let theme = crate::theme::DARK;
        let spec = SpecPreview {
            spec_id: "TASK-1".into(),
            title: "bare".into(),
            status: "Draft".into(),
            priority: String::new(),
            tags: vec![],
            description: String::new(),
        };
        let lines = text_to_lines(&preview_text(&PreviewBody::Spec(spec), &theme));
        let joined = lines.join("\n");
        assert!(joined.contains("Status:"), "status still shown: {joined:?}");
        assert!(!joined.contains("Priority:"), "no empty priority line");
        assert!(!joined.contains("Tags:"), "no empty tags line");
    }

    #[test]
    fn parse_spec_preview_reads_show_json() {
        // The `aida show --json` shape parses into the typed fields the
        // preview needs. trace:STORY-691
        let json = br#"{
            "spec_id": "STORY-691",
            "title": "t",
            "status": "Approved",
            "priority": "Medium",
            "tags": ["a", "b"],
            "description": "body"
        }"#;
        let spec = parse_spec_preview(json).expect("parses");
        assert_eq!(spec.spec_id, "STORY-691");
        assert_eq!(spec.status, "Approved");
        assert_eq!(spec.priority, "Medium");
        assert_eq!(spec.tags, vec!["a".to_string(), "b".to_string()]);
        // A shape mismatch returns None (caller degrades gracefully).
        assert!(parse_spec_preview(b"not json").is_none());
    }
}
