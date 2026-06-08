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
use ratatui::text::{Line, Span};
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
}

/// Read-only state for the bottom status strip.
#[derive(Debug, Clone, Default)]
pub struct AmbientState {
    pub role: String,
    pub queue_depth: usize,
    pub dialog_state: &'static str,
}

/// Per-launcher-run dashboard model.
#[derive(Debug, Clone, Default)]
pub struct DashboardModel {
    pub role: RoleTab,
    pub nav: NavState,
    pub rows: Vec<ListRow>,
    pub selected: usize,
    pub ambient: AmbientState,
    /// Cached `aida show <id>` preview text, keyed by row id. Filled on
    /// first paint per row and reused for subsequent moves.
    pub preview_cache: HashMap<String, Vec<String>>,
    /// Notice line above the middle list (e.g. "loading PRs…", "`gh`
    /// failed — empty PR list"). Cleared on a successful refetch.
    pub notice: Option<String>,
    /// Active palette — every styled span in the dashboard resolves its
    /// color through this rather than naming a literal. Defaults to the
    /// Catppuccin Mocha palette; the launcher overrides it from
    /// `[tui] theme`. trace:TASK-256 | ai:claude
    pub theme: Theme,
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
    model.rows = match section {
        NavSection::Queue => fetch_queue(model),
        NavSection::Backlog => fetch_status(&["approved", "planned"]),
        NavSection::History => fetch_status(&["completed"]),
        NavSection::Prs => match fetch_prs() {
            Ok(rows) => rows,
            Err(e) => {
                model.notice = Some(format!("`gh` failed — {e}"));
                Vec::new()
            }
        },
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

/// Queue section rows: reuse the overlay's cache-only fetch (sub-ms) to
/// land the queue head fast on first paint.
fn fetch_queue(model: &mut DashboardModel) -> Vec<ListRow> {
    match crate::overlay::fetch(true) {
        Ok(om) => om
            .queue
            .map(|q| {
                q.head
                    .into_iter()
                    .map(|item| ListRow {
                        id: item.spec_id,
                        title: item.title,
                        status: item.status,
                        kind: RowKind::Queued,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        Err(e) => {
            model.notice = Some(format!("queue fetch failed: {e}"));
            Vec::new()
        }
    }
}

/// Backlog / History rows: shell out to `aida list --status <csv> --json`.
fn fetch_status(statuses: &[&str]) -> Vec<ListRow> {
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
    parse_list_json(&out.stdout)
        .into_iter()
        .map(|row| ListRow {
            id: row.spec_id,
            title: row.title,
            status: row.status,
            kind: if statuses.contains(&"completed") {
                RowKind::History
            } else {
                RowKind::Backlog
            },
        })
        .collect()
}

/// Compact list-JSON row — what `aida list --json` emits.
/// trace:STORY-244 | ai:claude
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
}

/// Parse `aida list --json` output. Tolerant: a JSON shape mismatch
/// returns an empty list rather than crashing the launcher.
pub fn parse_list_json(bytes: &[u8]) -> Vec<ListJsonRow> {
    serde_json::from_slice(bytes).unwrap_or_default()
}

/// PRs section rows: `gh pr list --state open --json …`. The launcher
/// time-limits the `gh` shell-out at 5s so an offline / unauth shell
/// doesn't stall the dashboard. trace:STORY-244 risk #7 | ai:claude
fn fetch_prs() -> Result<Vec<ListRow>> {
    let out = run_with_timeout(
        Command::new("gh").args([
            "pr",
            "list",
            "--state",
            "open",
            "--json",
            "number,title,headRefName,statusCheckRollup",
        ]),
        Duration::from_secs(5),
    )?;
    if !out.status.success() {
        anyhow::bail!(
            "gh exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(parse_pr_json(&out.stdout))
}

#[derive(Debug, Clone, Deserialize)]
struct PrJson {
    number: u64,
    title: String,
    #[serde(rename = "headRefName", default)]
    head_ref_name: String,
    #[serde(rename = "statusCheckRollup", default)]
    status_check_rollup: Vec<serde_json::Value>,
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
    let lines = match row.kind {
        RowKind::Queued | RowKind::Backlog | RowKind::History => preview_via_show(&row.id),
        RowKind::Pr => preview_via_gh_pr(&row.id),
        RowKind::Session => vec![
            format!("Session id: {}", row.id),
            String::new(),
            "Enter resumes this conversation via `claude --resume`.".into(),
        ],
        RowKind::Action => vec![row.title.clone()],
    };
    model.preview_cache.insert(row.id, lines);
}

fn preview_via_show(id: &str) -> Vec<String> {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(["show", id]);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    match cmd.output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|s| s.to_string())
            .collect(),
        Ok(o) => vec![
            format!("`aida show {id}` failed:"),
            String::from_utf8_lossy(&o.stderr).to_string(),
        ],
        Err(e) => vec![format!("could not run `aida show`: {e}")],
    }
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

    nav::render(frame, body[0], &model.nav, &model.theme);
    render_list(frame, body[1], model);
    render_preview(frame, body[2], model);
    render_hint_row(frame, rows[2], model);
}

fn render_tabs(frame: &mut Frame, area: Rect, model: &DashboardModel) {
    let theme = &model.theme;
    let mut spans: Vec<Span> = Vec::new();
    for r in [RoleTab::Implementer, RoleTab::Reviewer, RoleTab::Dialog] {
        let label = format!("  {}  ", r.as_str());
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
    let block = Block::bordered()
        .border_style(Style::default().fg(theme.border))
        .title(format!(" {} ", section_title(model.nav.current())));
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
        let style = if i == model.selected {
            Style::default()
                .fg(theme.on_accent)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD)
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
    let lines: Vec<Line> = match model
        .current_row()
        .and_then(|r| model.preview_cache.get(&r.id))
    {
        Some(buf) => buf.iter().map(|s| Line::from(s.clone())).collect(),
        None => vec![Line::from(Span::styled(
            "(no selection)",
            Style::default().fg(theme.dim),
        ))],
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_hint_row(frame: &mut Frame, area: Rect, model: &DashboardModel) {
    let text = format!(
        "role:{} · queue:{} · dialog:{}    enter act · q/b/h/p/s nav · r role · g refresh · : palette · ? help",
        model.ambient.role, model.ambient.queue_depth, model.ambient.dialog_state
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
}
