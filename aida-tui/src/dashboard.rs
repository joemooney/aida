//! Mission control — the interactive dashboard for the empty `aida tui`
//! shell (STORY-241).
//!
//! BUG-109 gave the empty shell a static welcome card so first-time users
//! weren't staring at a black screen. This supersedes that card with a
//! live dashboard: the active role's queue, the project's session leases,
//! open pull requests, and state-aware suggested actions. It drives the
//! implementer → reviewer → merge loop without leaving the TUI —
//! `[Enter]` launches the selected queue item as a hosted session,
//! `Ctrl-A R` switches role, `Ctrl-A M` merges a PR.
//!
//! Like the `prefix o` overlay it is a full-screen `ratatui` view (it
//! owns the screen; there is no hosted child behind it to keep visible).
//! Unlike the overlay it is the *base layer* of the empty shell, not a
//! modal — `App` sits in [`crate::app::Mode::Dashboard`] whenever no tab
//! is hosted, and the dashboard moves into the `prefix o` overlay once a
//! session is hosted (STORY-133 already routes the same data there).
//!
//! Every command the dashboard issues is something the user could have
//! typed: `aida queue work`, `gh pr merge`, `aida pull`. Nothing here is
//! a new code path — the dashboard is a faster way to reach them.
//!
//! trace:STORY-241 | ai:claude

use crate::overlay::{self, BranchInfo, CacheInfo, QueueItem};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;
use serde::Deserialize;
use std::process::Command;

/// Roles the dashboard cycles through with `Ctrl-A R` and lists in the
/// `Ctrl-A Shift-R` picker — the implementer → reviewer → dialog loop.
pub const KNOWN_ROLES: [&str; 3] = ["implementer", "reviewer", "dialog"];

/// How many queued items the queue panel shows — the rest fold into the
/// `· N total` count in the panel title.
const QUEUE_DISPLAY: usize = 5;

// ───────────────────────────── model ──────────────────────────────────

/// One mission-control snapshot. Built by [`fetch`] on a background
/// thread; a missing section degrades to an empty panel, never an error.
#[derive(Debug, Clone, Default)]
pub struct DashboardModel {
    /// The role this snapshot's queue was fetched for.
    pub role: String,
    /// Current branch + upstream divergence (role-independent).
    pub branch: Option<BranchInfo>,
    /// Orphan-store cache freshness (role-independent).
    pub cache: Option<CacheInfo>,
    /// Top queued items for `role`, capped to [`QUEUE_DISPLAY`].
    pub queue: Vec<QueueItem>,
    /// Total queued count for `role` — may exceed `queue.len()`.
    pub queue_total: u64,
    /// Live session leases across the project.
    pub leases: Vec<LeaseRow>,
    /// Open pull requests.
    pub prs: Vec<PrRow>,
    /// Whether this snapshot ran the slow `gh pr list` — the 2s poll
    /// skips it, so [`crate::app`] carries the prior `prs` forward.
    pub prs_fetched: bool,
}

/// One live session lease, parsed from `aida session leases`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LeaseRow {
    pub id: String,
    pub scope: String,
    pub branch: String,
    pub role: String,
    pub worktree: String,
}

/// One open pull request, parsed from `gh pr list --json`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PrRow {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    /// `MERGEABLE` / `CONFLICTING` / `UNKNOWN`.
    #[serde(default)]
    pub mergeable: String,
    /// `APPROVED` / `CHANGES_REQUESTED` / `REVIEW_REQUIRED` / empty.
    #[serde(default, rename = "reviewDecision")]
    pub review_decision: String,
}

impl PrRow {
    /// Whether `gh` reports this PR cleanly mergeable — the hard gate on
    /// the one-keystroke merge (STORY-241 acceptance).
    pub fn is_mergeable(&self) -> bool {
        self.mergeable.eq_ignore_ascii_case("MERGEABLE")
    }

    /// A short, colour-coded label for the review state — `None` when no
    /// decision is recorded yet (a draft, or none required). Surfaces
    /// "awaiting review" on the reviewer's mission control.
    fn review_label(&self) -> Option<(&'static str, Color)> {
        match self.review_decision.to_ascii_uppercase().as_str() {
            "APPROVED" => Some(("approved", Color::Green)),
            "CHANGES_REQUESTED" => Some(("changes", Color::Red)),
            "REVIEW_REQUIRED" => Some(("review", Color::Yellow)),
            _ => None,
        }
    }
}

/// Which panel the dashboard cursor is in — arrows move within it, Tab
/// switches between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Queue,
    Prs,
}

/// A merge about to happen — what the `Ctrl-A M` confirmation modal
/// shows. Only ever built for an already-mergeable PR.
#[derive(Debug, Clone)]
pub struct MergePlan {
    pub number: u64,
    pub title: String,
    /// Specs the merge will auto-bump Done → Completed, derived from the
    /// PR's commit messages. Empty when `gh pr view` was unavailable.
    pub specs: Vec<String>,
}

/// The `Ctrl-A Shift-R` role picker modal.
#[derive(Debug, Clone)]
pub struct RolePicker {
    pub roles: Vec<String>,
    pub selected: usize,
}

impl RolePicker {
    /// A picker over [`KNOWN_ROLES`], pre-selecting `current`.
    pub fn new(current: &str) -> Self {
        let roles: Vec<String> = KNOWN_ROLES.iter().map(|r| r.to_string()).collect();
        let selected = roles.iter().position(|r| r == current).unwrap_or(0);
        RolePicker { roles, selected }
    }

    pub fn next(&mut self) {
        self.selected = (self.selected + 1) % self.roles.len();
    }

    pub fn prev(&mut self) {
        let n = self.roles.len();
        self.selected = (self.selected + n - 1) % n;
    }

    pub fn current(&self) -> &str {
        &self.roles[self.selected]
    }
}

/// Everything `App` carries while in [`crate::app::Mode::Dashboard`].
pub struct DashboardState {
    pub model: DashboardModel,
    /// The active (possibly role-switched) role the dashboard is viewing.
    pub role: String,
    /// Which panel the cursor is in.
    pub focus: Panel,
    pub queue_sel: usize,
    pub pr_sel: usize,
    /// A merge awaiting `y`/cancel confirmation — drives the modal.
    pub merge_confirm: Option<MergePlan>,
    /// The role picker, open while `Some`.
    pub role_picker: Option<RolePicker>,
    /// A transient one-line status (merge result, an error) shown in the
    /// footer — cleared by the next navigation key or refresh.
    pub note: Option<String>,
    /// True between a refresh kick-off and its snapshot landing.
    pub refreshing: bool,
}

impl DashboardState {
    /// An empty dashboard for `role` — panels fill in on the first
    /// [`DashboardModel`] from a background refresh.
    pub fn new(role: &str) -> Self {
        DashboardState {
            model: DashboardModel {
                role: role.to_string(),
                ..DashboardModel::default()
            },
            role: role.to_string(),
            focus: Panel::Queue,
            queue_sel: 0,
            pr_sel: 0,
            merge_confirm: None,
            role_picker: None,
            note: None,
            refreshing: false,
        }
    }

    /// Apply a freshly-fetched snapshot. A 2s poll snapshot (`prs_fetched`
    /// false) keeps the prior PR list rather than blanking the panel.
    /// Selections are clamped — a shrunk queue must not strand the cursor.
    pub fn apply(&mut self, mut model: DashboardModel) {
        if !model.prs_fetched {
            model.prs = std::mem::take(&mut self.model.prs);
            model.prs_fetched = self.model.prs_fetched;
        }
        self.model = model;
        self.refreshing = false;
        self.clamp_selections();
    }

    /// Keep `queue_sel` / `pr_sel` inside their (possibly shrunk) lists.
    pub fn clamp_selections(&mut self) {
        let qn = self.model.queue.len();
        self.queue_sel = if qn == 0 {
            0
        } else {
            self.queue_sel.min(qn - 1)
        };
        let pn = self.model.prs.len();
        self.pr_sel = if pn == 0 { 0 } else { self.pr_sel.min(pn - 1) };
    }

    /// Move the cursor down within the focused panel, wrapping.
    pub fn move_down(&mut self) {
        match self.focus {
            Panel::Queue => {
                let n = self.model.queue.len();
                if n > 0 {
                    self.queue_sel = (self.queue_sel + 1) % n;
                }
            }
            Panel::Prs => {
                let n = self.model.prs.len();
                if n > 0 {
                    self.pr_sel = (self.pr_sel + 1) % n;
                }
            }
        }
    }

    /// Move the cursor up within the focused panel, wrapping.
    pub fn move_up(&mut self) {
        match self.focus {
            Panel::Queue => {
                let n = self.model.queue.len();
                if n > 0 {
                    self.queue_sel = (self.queue_sel + n - 1) % n;
                }
            }
            Panel::Prs => {
                let n = self.model.prs.len();
                if n > 0 {
                    self.pr_sel = (self.pr_sel + n - 1) % n;
                }
            }
        }
    }

    /// Toggle the focused panel — Queue ⇄ Prs.
    pub fn toggle_panel(&mut self) {
        self.focus = match self.focus {
            Panel::Queue => Panel::Prs,
            Panel::Prs => Panel::Queue,
        };
    }

    /// The highlighted queue item, if any.
    pub fn selected_queue_item(&self) -> Option<&QueueItem> {
        self.model.queue.get(self.queue_sel)
    }

    /// The highlighted PR, if any.
    pub fn selected_pr(&self) -> Option<&PrRow> {
        self.model.prs.get(self.pr_sel)
    }
}

/// The role one `Ctrl-A R` cycle past `current` — implementer → reviewer
/// → dialog → implementer. An unknown current role restarts the cycle.
pub fn next_role(current: &str) -> String {
    let idx = KNOWN_ROLES.iter().position(|r| *r == current);
    let next = match idx {
        Some(i) => (i + 1) % KNOWN_ROLES.len(),
        None => 0,
    };
    KNOWN_ROLES[next].to_string()
}

/// State-aware suggested actions for the dashboard's `Suggested` panel,
/// as `(key-chord, description)` pairs — at most three, most useful
/// first given what's queued / open. `prefix` is the configured
/// prefix-key label so a reconfigured prefix stays accurate.
pub fn suggested_actions(state: &DashboardState, prefix: &str) -> Vec<(String, String)> {
    let m = &state.model;
    let mut out: Vec<(String, String)> = Vec::new();
    if let Some(item) = state.selected_queue_item().or_else(|| m.queue.first()) {
        out.push((
            "Enter".to_string(),
            format!("start {} in a session", item.spec_id),
        ));
    }
    if let Some(pr) = m.prs.iter().find(|p| p.is_mergeable()) {
        out.push((format!("{prefix} M"), format!("merge PR #{}", pr.number)));
    }
    out.push((format!("{prefix} R"), "switch role".to_string()));
    if out.len() < 3 {
        out.push((format!("{prefix} N"), "open a session".to_string()));
    }
    if out.len() < 3 {
        out.push((format!("{prefix} G"), "refresh".to_string()));
    }
    out.truncate(3);
    out
}

// ──────────────────────────── fetch ───────────────────────────────────

/// Fetch a mission-control snapshot for `role`. `with_prs` runs the slow
/// `gh pr list`; the 2s cache-staleness poll passes `false` and lets
/// [`DashboardState::apply`] carry the prior PR list forward.
pub fn fetch(role: &str, with_prs: bool) -> DashboardModel {
    // Branch + cache from the cache-only status path (sub-millisecond).
    let status = overlay::fetch(true).ok();
    let (branch, cache) = match status {
        Some(s) => (s.branch, s.cache),
        None => (None, None),
    };
    let (queue, queue_total) = fetch_queue(role);
    let leases = fetch_leases();
    let prs = if with_prs { fetch_prs() } else { Vec::new() };
    DashboardModel {
        role: role.to_string(),
        branch,
        cache,
        queue,
        queue_total,
        leases,
        prs,
        prs_fetched: with_prs,
    }
}

/// The top queued items for `role` via `aida queue list --for <role>`.
/// `--no-in-flight` keeps the "Done — awaiting merge" tail out of the
/// parse. Returns the display-capped items + the true total.
fn fetch_queue(role: &str) -> (Vec<QueueItem>, u64) {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(["queue", "list", "--for", role, "--no-in-flight"]);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    let text = match cmd.output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).into_owned(),
        Err(_) => return (Vec::new(), 0),
    };
    let (mut items, total) = parse_queue_list(&text);
    items.truncate(QUEUE_DISPLAY);
    (items, total)
}

/// Live session leases via `aida session leases`.
fn fetch_leases() -> Vec<LeaseRow> {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(["session", "leases"]);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    match cmd.output() {
        Ok(o) => parse_leases(&String::from_utf8_lossy(&o.stdout)),
        Err(_) => Vec::new(),
    }
}

/// Open pull requests via `gh pr list --json`. An absent / failing `gh`
/// just yields an empty panel.
fn fetch_prs() -> Vec<PrRow> {
    let mut cmd = Command::new("gh");
    cmd.args([
        "pr",
        "list",
        "--state",
        "open",
        "--limit",
        "10",
        "--json",
        "number,title,mergeable,reviewDecision",
    ]);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    match cmd.output() {
        Ok(o) if o.status.success() => serde_json::from_slice(&o.stdout).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Build the [`MergePlan`] for a mergeable PR — `gh pr view` supplies the
/// commit messages, from which the auto-bump specs are derived. A failing
/// `gh` still yields a plan (PR number + title), just with no spec list.
pub fn merge_plan(pr: &PrRow) -> MergePlan {
    let mut plan = MergePlan {
        number: pr.number,
        title: pr.title.clone(),
        specs: Vec::new(),
    };
    let mut cmd = Command::new("gh");
    cmd.args(["pr", "view", &pr.number.to_string(), "--json", "commits"]);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    if let Ok(o) = cmd.output() {
        if o.status.success() {
            plan.specs = parse_commit_specs(&o.stdout);
        }
    }
    plan
}

// ──────────────────────────── parsers ─────────────────────────────────

/// Whether `s` is a spec id — an uppercase prefix, `-`, then digits
/// (`STORY-241`, `BUG-9`).
fn is_spec_id(s: &str) -> bool {
    let Some((prefix, num)) = s.split_once('-') else {
        return false;
    };
    !prefix.is_empty()
        && prefix.chars().all(|c| c.is_ascii_uppercase())
        && !num.is_empty()
        && num.chars().all(|c| c.is_ascii_digit())
}

/// Strip a leading `N. ` ordinal from a queue-list row, returning the
/// remainder. `None` when the trimmed line isn't an `N. …` item row.
fn strip_ordinal(trimmed: &str) -> Option<&str> {
    let digits_end = trimmed.find(|c: char| !c.is_ascii_digit())?;
    if digits_end == 0 {
        return None;
    }
    trimmed[digits_end..]
        .strip_prefix(". ")
        .map(|r| r.trim_start())
}

/// Pull an `(N items)` count out of a queue-list header line.
fn header_count(line: &str) -> Option<u64> {
    let open = line.rfind('(')?;
    let inner = &line[open + 1..];
    let close = inner.find(')')?;
    let inner = &inner[..close];
    if !inner.contains("item") {
        return None;
    }
    let digits: String = inner.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Parse `aida queue list --for <role>` text into queue items + the true
/// total. Item rows look like `  1. STORY-241 <title>  [Status]  [tags]`;
/// the title runs up to the first `  [` bracket group, the status is that
/// group's content. The `(N items)` header supplies the total.
pub fn parse_queue_list(text: &str) -> (Vec<QueueItem>, u64) {
    let mut items: Vec<QueueItem> = Vec::new();
    let mut total: u64 = 0;
    for line in text.lines() {
        let trimmed = line.trim();
        let Some(rest) = strip_ordinal(trimmed) else {
            // Not an item row — a header may carry the total.
            if let Some(n) = header_count(trimmed) {
                total = n;
            }
            continue;
        };
        let mut parts = rest.splitn(2, char::is_whitespace);
        let Some(spec) = parts.next() else {
            continue;
        };
        if !is_spec_id(spec) {
            continue;
        }
        let body = parts.next().unwrap_or("").trim();
        let (title, status) = match body.find("  [") {
            Some(i) => {
                let title = body[..i].trim().to_string();
                let status = body[i + 3..]
                    .split(']')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                (title, status)
            }
            None => (body.to_string(), String::new()),
        };
        items.push(QueueItem {
            spec_id: spec.to_string(),
            title,
            status,
        });
    }
    if total < items.len() as u64 {
        total = items.len() as u64;
    }
    (items, total)
}

/// Parse `aida session leases` text into [`LeaseRow`]s. Data rows start
/// with a hex session id and carry five-plus whitespace-separated fields;
/// the header (`id  scope  …`), the `────` rule and the footer hint have
/// no hex-id first column and are skipped.
pub fn parse_leases(text: &str) -> Vec<LeaseRow> {
    let mut out = Vec::new();
    for line in text.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.len() < 5 {
            continue;
        }
        let id = toks[0];
        if id.len() < 6 || id.len() > 24 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        out.push(LeaseRow {
            id: id.to_string(),
            scope: toks[1].to_string(),
            branch: toks[2].to_string(),
            role: toks[3].to_string(),
            worktree: toks[4..].join(" "),
        });
    }
    out
}

/// Extract spec ids referenced anywhere in `text` (a commit message), in
/// first-seen order, deduped. Tokens are split on anything that isn't a
/// spec-id character, so `(STORY-241)` and `BUG-9` both surface.
pub fn extract_specs(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tok in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
        if is_spec_id(tok) && !out.iter().any(|s| s == tok) {
            out.push(tok.to_string());
        }
    }
    out
}

/// Parse `gh pr view --json commits` output, harvesting spec ids from
/// every commit's headline + body.
fn parse_commit_specs(json: &[u8]) -> Vec<String> {
    #[derive(Deserialize)]
    struct ViewJson {
        #[serde(default)]
        commits: Vec<CommitJson>,
    }
    #[derive(Deserialize)]
    struct CommitJson {
        #[serde(default, rename = "messageHeadline")]
        headline: String,
        #[serde(default, rename = "messageBody")]
        body: String,
    }
    let Ok(view) = serde_json::from_slice::<ViewJson>(json) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for commit in view.commits {
        for spec in extract_specs(&commit.headline) {
            if !out.contains(&spec) {
                out.push(spec);
            }
        }
        for spec in extract_specs(&commit.body) {
            if !out.contains(&spec) {
                out.push(spec);
            }
        }
    }
    out
}

// ──────────────────────────── render ──────────────────────────────────

/// Dimmed style for secondary text.
fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// Clip `s` to at most `max` display columns, ellipsising when cut.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max <= 1 {
        return s.chars().take(max).collect();
    }
    let mut t: String = s.chars().take(max - 1).collect();
    t.push('…');
    t
}

/// A centred `width × height` sub-rect of `area`, clamped to fit.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

/// Draw the whole dashboard into `frame`, plus the merge / role-picker
/// modal on top when one is open. `prefix` is the configured prefix-key
/// label (`Ctrl-A` by default) — woven into every key hint.
pub fn render(frame: &mut Frame, state: &DashboardState, prefix: &str) {
    let rows = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Min(6),    // panels
        Constraint::Length(5), // suggested actions
        Constraint::Length(1), // footer
    ])
    .split(frame.area());

    render_header(frame, rows[0], state);

    let cols =
        Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)]).split(rows[1]);
    render_queue(frame, cols[0], state, prefix);

    let right = Layout::vertical([Constraint::Length(7), Constraint::Min(4)]).split(cols[1]);
    render_sessions(frame, right[0], state);
    render_prs(frame, right[1], state);

    render_suggested(frame, rows[2], state, prefix);
    render_footer(frame, rows[3], state, prefix);

    if let Some(plan) = &state.merge_confirm {
        render_merge_modal(frame, plan);
    } else if let Some(picker) = &state.role_picker {
        render_role_modal(frame, picker);
    }
}

fn render_header(frame: &mut Frame, area: Rect, state: &DashboardState) {
    let m = &state.model;
    let branch = m
        .branch
        .as_ref()
        .map(|b| b.name.clone())
        .unwrap_or_else(|| "—".to_string());
    let (cache_txt, cache_style) = match &m.cache {
        Some(c) if c.fresh => ("fresh", Style::default().fg(Color::Green)),
        Some(_) => ("stale", Style::default().fg(Color::Yellow)),
        None => ("?", dim()),
    };
    let mut spans = vec![
        Span::styled(
            " mission control ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  role ", dim()),
        Span::styled(
            state.role.clone(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("   branch {}   cache ", branch), dim()),
        Span::styled(cache_txt.to_string(), cache_style),
    ];
    if state.refreshing {
        spans.push(Span::styled("   · refreshing…", dim()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_queue(frame: &mut Frame, area: Rect, state: &DashboardState, prefix: &str) {
    let m = &state.model;
    let block = Block::bordered().title(format!(
        " Queue — role {} · {} total ",
        m.role, m.queue_total
    ));
    let inner_w = area.width.saturating_sub(2) as usize;
    let focused = state.focus == Panel::Queue;

    let mut lines: Vec<Line> = Vec::new();
    if m.queue.is_empty() {
        // "All clear + next steps" empty state (STORY-241 design Q4).
        lines.push(Line::from(Span::styled(
            "✓ All clear — queue empty",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("Next:", dim())));
        for hint in [
            format!("{prefix} R   switch role (try reviewer)"),
            format!("{prefix} N   open a session"),
            "aida queue add <id> --for <role>".to_string(),
        ] {
            lines.push(Line::from(format!("  {}", hint)));
        }
    } else {
        for (i, item) in m.queue.iter().enumerate() {
            let selected = focused && i == state.queue_sel;
            let head = format!("{}. {}  ", i + 1, item.spec_id);
            let tail = format!("[{}]", item.status);
            let budget = inner_w
                .saturating_sub(head.chars().count() + tail.chars().count() + 3)
                .max(4);
            let row_style = if selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            lines.push(Line::from(vec![
                Span::styled(
                    head,
                    row_style.fg(if selected { Color::Black } else { Color::Cyan }),
                ),
                Span::styled(clip(&item.title, budget), row_style),
                Span::raw(" "),
                Span::styled(tail, if selected { row_style } else { dim() }),
            ]));
        }
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_sessions(frame: &mut Frame, area: Rect, state: &DashboardState) {
    let leases = &state.model.leases;
    let block = Block::bordered().title(format!(" Sessions · {} ", leases.len()));
    let inner_w = area.width.saturating_sub(2) as usize;
    let mut lines: Vec<Line> = Vec::new();
    if leases.is_empty() {
        lines.push(Line::from(Span::styled("no live leases", dim())));
    } else {
        for lease in leases {
            let text = format!("● {}  {}  {}", lease.id, lease.scope, lease.role);
            lines.push(Line::from(Span::styled(
                clip(&text, inner_w),
                Style::default().fg(Color::Green),
            )));
        }
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_prs(frame: &mut Frame, area: Rect, state: &DashboardState) {
    let prs = &state.model.prs;
    let block = Block::bordered().title(format!(" Open PRs · {} ", prs.len()));
    let inner_w = area.width.saturating_sub(2) as usize;
    let focused = state.focus == Panel::Prs;
    let mut lines: Vec<Line> = Vec::new();
    if prs.is_empty() {
        lines.push(Line::from(Span::styled("none open", dim())));
    } else {
        for (i, pr) in prs.iter().enumerate() {
            let selected = focused && i == state.pr_sel;
            let (tag, tag_color) = if pr.is_mergeable() {
                ("MERGEABLE", Color::Green)
            } else if pr.mergeable.eq_ignore_ascii_case("CONFLICTING") {
                ("CONFLICTING", Color::Red)
            } else {
                ("checking", Color::DarkGray)
            };
            let review = pr.review_label();
            let head = format!("#{}  ", pr.number);
            let review_w = review.map(|(t, _)| t.len() + 1).unwrap_or(0);
            let budget = inner_w
                .saturating_sub(head.chars().count() + tag.len() + 2 + review_w)
                .max(4);
            let base = if selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let mut spans = vec![
                Span::styled(
                    head,
                    base.fg(if selected { Color::Black } else { Color::Cyan }),
                ),
                Span::styled(
                    format!("{} ", tag),
                    if selected {
                        base
                    } else {
                        Style::default().fg(tag_color)
                    },
                ),
            ];
            if let Some((rtext, rcolor)) = review {
                spans.push(Span::styled(
                    format!("{} ", rtext),
                    if selected {
                        base
                    } else {
                        Style::default().fg(rcolor)
                    },
                ));
            }
            spans.push(Span::styled(clip(&pr.title, budget), base));
            lines.push(Line::from(spans));
        }
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_suggested(frame: &mut Frame, area: Rect, state: &DashboardState, prefix: &str) {
    let block = Block::bordered().title(" Suggested ");
    let actions = suggested_actions(state, prefix);
    let key_w = actions
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0);
    let lines: Vec<Line> = actions
        .iter()
        .map(|(key, desc)| {
            Line::from(vec![
                Span::styled(
                    format!("  {:<width$}  ", key, width = key_w),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(desc.clone()),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_footer(frame: &mut Frame, area: Rect, state: &DashboardState, prefix: &str) {
    if let Some(note) = &state.note {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {}", note),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ))),
            area,
        );
        return;
    }
    let footer = Line::from(vec![
        Span::styled(format!(" {} ", prefix), Style::default().fg(Color::Cyan)),
        Span::styled(
            "N new · R role · M merge · G refresh · O overlay · Q quit",
            dim(),
        ),
        Span::styled("    ↑↓ ", Style::default().fg(Color::Cyan)),
        Span::styled("select", dim()),
        Span::styled("  Tab ", Style::default().fg(Color::Cyan)),
        Span::styled("panel", dim()),
        Span::styled("  Enter ", Style::default().fg(Color::Cyan)),
        Span::styled("start", dim()),
    ]);
    frame.render_widget(Paragraph::new(footer), area);
}

fn render_merge_modal(frame: &mut Frame, plan: &MergePlan) {
    let area = centered_rect(54, 11, frame.area());
    frame.render_widget(Clear, area);
    let block = Block::bordered().title(" Confirm merge ");
    let mut lines: Vec<Line> = vec![
        Line::from(vec![Span::styled(
            format!("PR #{}", plan.number),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(clip(&plan.title, area.width.saturating_sub(4) as usize)),
        Line::from(""),
    ];
    if plan.specs.is_empty() {
        lines.push(Line::from(Span::styled(
            "No specs detected in the PR's commits.",
            dim(),
        )));
    } else {
        lines.push(Line::from(Span::styled("Auto-bumps to Completed:", dim())));
        lines.push(Line::from(Span::styled(
            format!("  {}", plan.specs.join(", ")),
            Style::default().fg(Color::Green),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "gh pr merge --squash --delete-branch, then aida pull",
        dim(),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            "  [ y ] merge  ",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("   [ any other key ] cancel", dim()),
    ]));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_role_modal(frame: &mut Frame, picker: &RolePicker) {
    let height = picker.roles.len() as u16 + 4;
    let area = centered_rect(34, height, frame.area());
    frame.render_widget(Clear, area);
    let block = Block::bordered().title(" Switch role ");
    let mut lines: Vec<Line> = Vec::new();
    for (i, role) in picker.roles.iter().enumerate() {
        let selected = i == picker.selected;
        let marker = if selected { "▸ " } else { "  " };
        let style = if selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(
            format!("{}{}", marker, role),
            style,
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(" ↑/↓ ", Style::default().fg(Color::Cyan)),
        Span::styled("select  ", dim()),
        Span::styled("enter ", Style::default().fg(Color::Cyan)),
        Span::styled("switch  ", dim()),
        Span::styled("esc ", Style::default().fg(Color::Cyan)),
        Span::styled("cancel", dim()),
    ]));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_populated_queue_list() {
        let text = "\
My Queue · role:implementer (7 items)
────────────────────────────────────────
  1. STORY-241 TUI workflow loop: mission control  [In Progress]  [@EPIC-26*]  [tag1, tag2]
  2. BUG-110 install SIGTERM handler (raw mode + cursor)  [Approved]  [cleanup]
  3. TASK-254 revert default-flip  [Approved]  [revert]
";
        let (items, total) = parse_queue_list(text);
        assert_eq!(total, 7, "header (N items) wins over the parsed count");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].spec_id, "STORY-241");
        assert_eq!(items[0].status, "In Progress");
        // A title with parentheses must not be mistaken for the status.
        assert_eq!(items[1].spec_id, "BUG-110");
        assert_eq!(
            items[1].title,
            "install SIGTERM handler (raw mode + cursor)"
        );
        assert_eq!(items[1].status, "Approved");
    }

    #[test]
    fn parses_an_empty_queue_list() {
        let text = "Your queue (no items routed to role reviewer; pass --all)\n";
        let (items, total) = parse_queue_list(text);
        assert!(items.is_empty());
        assert_eq!(total, 0);
    }

    #[test]
    fn parses_session_leases_skipping_chrome() {
        let text = "\
Active session leases

id             scope                branch             role           worktree
────────────────────────────────────────────────────────────────────────────────
019e2ef5       EPIC-26              epic-26-4          implementer    /home/joe/ai/aida-epic-26
019e2d4f       BUG-9                bug-9              -              /home/joe/ai/aida

End one with: aida session end <id>
";
        let leases = parse_leases(text);
        assert_eq!(leases.len(), 2, "header, rule and footer are skipped");
        assert_eq!(leases[0].id, "019e2ef5");
        assert_eq!(leases[0].scope, "EPIC-26");
        assert_eq!(leases[0].role, "implementer");
        assert_eq!(leases[1].role, "-");
    }

    #[test]
    fn extract_specs_finds_ids_in_commit_text() {
        let specs = extract_specs("[AI:claude] feat(tui): mission control (STORY-241)");
        assert_eq!(specs, vec!["STORY-241"]);
        // Deduped, first-seen order; lowercase branch-like tokens ignored.
        let specs = extract_specs("fixes BUG-9 and STORY-241; see BUG-9 again — epic-26-4");
        assert_eq!(specs, vec!["BUG-9", "STORY-241"]);
    }

    #[test]
    fn next_role_cycles_the_known_roles() {
        assert_eq!(next_role("implementer"), "reviewer");
        assert_eq!(next_role("reviewer"), "dialog");
        assert_eq!(next_role("dialog"), "implementer");
        // An unknown role restarts the cycle at the head.
        assert_eq!(next_role("captain"), "implementer");
    }

    /// A dashboard state with `n` queued items and `p` PRs, for nav tests.
    fn state_with(queue: usize, prs: usize) -> DashboardState {
        let mut s = DashboardState::new("implementer");
        s.model.queue = (0..queue)
            .map(|i| QueueItem {
                spec_id: format!("STORY-{}", i),
                title: format!("item {}", i),
                status: "Approved".into(),
            })
            .collect();
        s.model.prs = (0..prs)
            .map(|i| PrRow {
                number: i as u64 + 40,
                mergeable: "MERGEABLE".into(),
                ..PrRow::default()
            })
            .collect();
        s
    }

    #[test]
    fn navigation_wraps_within_the_focused_panel() {
        let mut s = state_with(3, 2);
        assert_eq!(s.queue_sel, 0);
        s.move_up(); // wrap to the end
        assert_eq!(s.queue_sel, 2);
        s.move_down(); // wrap back to the start
        assert_eq!(s.queue_sel, 0);

        // Tab moves focus to the PR panel; nav now drives `pr_sel`.
        s.toggle_panel();
        assert_eq!(s.focus, Panel::Prs);
        s.move_down();
        assert_eq!(s.pr_sel, 1);
        assert_eq!(s.queue_sel, 0, "queue selection untouched while on PRs");
    }

    #[test]
    fn apply_clamps_a_stranded_selection_and_keeps_prior_prs() {
        let mut s = state_with(5, 3);
        s.queue_sel = 4;
        s.pr_sel = 2;
        // A poll snapshot (prs_fetched = false) with a shorter queue.
        let mut poll = DashboardModel {
            role: "implementer".into(),
            prs_fetched: false,
            ..DashboardModel::default()
        };
        poll.queue = vec![QueueItem {
            spec_id: "STORY-0".into(),
            title: "only".into(),
            status: "Approved".into(),
        }];
        s.apply(poll);
        assert_eq!(s.queue_sel, 0, "stranded queue selection clamped");
        assert_eq!(s.pr_sel, 2, "PR selection still valid — 3 carried PRs");
        assert_eq!(s.model.prs.len(), 3, "poll snapshot keeps the prior PRs");
    }

    #[test]
    fn suggested_actions_track_state() {
        // Empty everything → still three hints, role-switch always offered.
        let empty = DashboardState::new("implementer");
        let hints = suggested_actions(&empty, "Ctrl-A");
        assert_eq!(hints.len(), 3);
        assert!(hints.iter().any(|(_, d)| d == "switch role"));

        // Queued work + a mergeable PR → both surface, merge included.
        let s = state_with(2, 1);
        let hints = suggested_actions(&s, "Ctrl-A");
        assert!(hints[0].1.contains("start STORY-0"));
        assert!(hints.iter().any(|(_, d)| d == "merge PR #40"));
        // The prefix label is woven into the chord, not hard-coded.
        let custom = suggested_actions(&s, "Ctrl-B");
        assert!(custom.iter().any(|(k, _)| k == "Ctrl-B M"));
    }

    #[test]
    fn pr_mergeable_gate_is_case_insensitive_and_strict() {
        let mergeable = PrRow {
            mergeable: "MERGEABLE".into(),
            ..PrRow::default()
        };
        assert!(mergeable.is_mergeable());
        let conflicting = PrRow {
            mergeable: "CONFLICTING".into(),
            ..PrRow::default()
        };
        assert!(!conflicting.is_mergeable());
        let unknown = PrRow {
            mergeable: "UNKNOWN".into(),
            ..PrRow::default()
        };
        assert!(!unknown.is_mergeable());
    }

    #[test]
    fn role_picker_preselects_current_and_wraps() {
        let mut p = RolePicker::new("reviewer");
        assert_eq!(p.current(), "reviewer");
        p.next();
        assert_eq!(p.current(), "dialog");
        p.next();
        assert_eq!(p.current(), "implementer");
        p.prev();
        assert_eq!(p.current(), "dialog");
    }

    #[test]
    fn parse_commit_specs_reads_gh_pr_view_json() {
        let json = br#"{"commits":[
          {"messageHeadline":"feat(tui): mission control (STORY-241)","messageBody":"composes with BUG-110"},
          {"messageHeadline":"chore: tidy","messageBody":""}
        ]}"#;
        let specs = parse_commit_specs(json);
        assert_eq!(specs, vec!["STORY-241", "BUG-110"]);
    }
}
