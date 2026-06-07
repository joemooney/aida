//! Status overlay — a read-only `ratatui` view over `aida status --json`.
//!
//! `prefix o` opens it. It paints panels for the session lease, the
//! branch / PR / CI rollup, the role queue, and an activity log, plus a
//! three-button quick-action row (STORY-133). The first paint is
//! cache-only (`aida status --json --no-ci`, sub-millisecond); a
//! background `gh`-backed refresh repaints when it lands (plan Fork 3 +
//! risk #10 — `gh` must never stall the first paint).
//!
//! The overlay never writes AIDA state itself — its only side effects are
//! the [`crate::actions`] subprocesses, and each one is a command the
//! user could have typed.
//!
//! trace:STORY-133 | ai:claude

use crate::actions::{ActivityEntry, QuickAction};
use anyhow::{Context, Result};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;
use serde::Deserialize;
use std::process::Command;

/// Parsed projection of `aida status --json`. Every field is optional —
/// `--no-ci` / `--queue-only` runs omit sections, `session` / `branch`
/// arrive as JSON `null` outside a session, and an older binary may drop
/// a field entirely. A missing panel renders a placeholder, never an
/// error: a malformed overlay must not block the keystroke that opened it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OverlayModel {
    #[serde(default)]
    pub session: Option<SessionInfo>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub branch: Option<BranchInfo>,
    #[serde(default)]
    pub pr: Option<PrInfo>,
    #[serde(default)]
    pub queue: Option<QueueInfo>,
    #[serde(default)]
    pub cache: Option<CacheInfo>,
}

/// The active session lease (`status.session`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SessionInfo {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub worktree: String,
    #[serde(default)]
    pub role: Option<String>,
}

/// Branch / upstream divergence (`status.branch`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BranchInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub dirty: bool,
    #[serde(default)]
    pub ahead_main: i64,
    #[serde(default)]
    pub behind_main: i64,
    #[serde(default)]
    pub ahead_upstream: i64,
    #[serde(default)]
    pub behind_upstream: i64,
    #[serde(default)]
    pub has_upstream: bool,
}

/// PR / CI rollup (`status.pr`). The same key carries four shapes —
/// `{skipped}`, `{error,reason}`, `{state:"none"}`, or the full PR — so
/// every field is optional and [`pr_lines`] dispatches on which are set.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PrInfo {
    #[serde(default)]
    pub number: Option<u64>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub ci_rollup: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub skipped: Option<bool>,
}

/// The active role's queue head (`status.queue`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct QueueInfo {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub head: Vec<QueueItem>,
}

/// One queued requirement in [`QueueInfo::head`].
#[derive(Debug, Clone, Default, Deserialize)]
pub struct QueueItem {
    #[serde(default)]
    pub spec_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub status: String,
}

/// Cache freshness (`status.cache`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CacheInfo {
    #[serde(default)]
    pub fresh: bool,
    #[serde(default)]
    pub rows: u64,
}

/// Run `aida status --json` and parse it into an [`OverlayModel`]. With
/// `no_ci`, passes `--no-ci` so the PR/CI shell-out is skipped — the
/// sub-millisecond first paint (plan risk #10).
pub fn fetch(no_ci: bool) -> Result<OverlayModel> {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.arg("status").arg("--json");
    if no_ci {
        cmd.arg("--no-ci");
    }
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    let out = cmd
        .output()
        .with_context(|| format!("could not run `{} status --json`", exe.display()))?;
    if !out.status.success() {
        anyhow::bail!(
            "`aida status --json` exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    parse(&out.stdout)
}

/// Parse `aida status --json` output. Split from [`fetch`] so the panel
/// model is testable without spawning a subprocess.
pub fn parse(json: &[u8]) -> Result<OverlayModel> {
    serde_json::from_slice(json)
        .context("`aida status --json` returned JSON the overlay can't read")
}

/// Draw the whole overlay into `frame`. Full-screen — one focused tab is
/// hosted at a time, so the overlay owns the screen while open and the
/// focused child is repainted from its `vt100` snapshot on close.
pub fn render(
    frame: &mut Frame,
    model: &OverlayModel,
    activity: &[ActivityEntry],
    selected: usize,
    confirm: Option<QuickAction>,
    refreshing: bool,
) {
    let rows = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Length(6), // session + branch/pr
        Constraint::Min(4),    // queue
        Constraint::Length(9), // activity log
        Constraint::Length(4), // actions + confirm
        Constraint::Length(1), // help
    ])
    .split(frame.area());

    render_header(frame, rows[0], model, refreshing);

    let top =
        Layout::horizontal([Constraint::Percentage(46), Constraint::Percentage(54)]).split(rows[1]);
    render_session(frame, top[0], model);
    render_branch(frame, top[1], model);

    render_queue(frame, rows[2], model);
    render_activity(frame, rows[3], activity);
    render_actions(frame, rows[4], selected, confirm);
    render_help(frame, rows[5]);
}

/// Dimmed style for secondary text.
fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// Clip `s` to at most `max` display columns, with an ellipsis when cut.
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

/// TASK-252: char-safe first-12 truncation of a session id for the lease
/// overlay. The previous `&id[..12]` byte-slice panics the whole TUI render if
/// a multi-byte UTF-8 char straddles byte 12. Session ids are normally ASCII
/// hex, but we never assume that of an arbitrary &str. trace:TASK-252 | ai:claude
fn short_session_id(id: &str) -> String {
    id.chars().take(12).collect()
}

fn render_header(frame: &mut Frame, area: Rect, model: &OverlayModel, refreshing: bool) {
    let branch = model
        .branch
        .as_ref()
        .map(|b| b.name.clone())
        .unwrap_or_else(|| "—".to_string());
    let (cache_txt, cache_style) = match &model.cache {
        Some(c) if c.fresh => ("fresh", Style::default().fg(Color::Green)),
        Some(_) => ("stale", Style::default().fg(Color::Yellow)),
        None => ("?", dim()),
    };
    let mut spans = vec![
        Span::styled(
            " aida status ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("  branch {}   cache ", branch)),
        Span::styled(cache_txt.to_string(), cache_style),
    ];
    if let Some(c) = &model.cache {
        spans.push(Span::styled(format!(" · {} reqs", c.rows), dim()));
    }
    if refreshing {
        spans.push(Span::styled("   · fetching PR/CI…", dim()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_session(frame: &mut Frame, area: Rect, model: &OverlayModel) {
    let block = Block::bordered().title(" Session lease ");
    let inner_w = area.width.saturating_sub(2) as usize;
    let lines: Vec<Line> = match &model.session {
        Some(s) => {
            let id = short_session_id(&s.id);
            let role = s
                .role
                .clone()
                .or_else(|| model.role.clone())
                .unwrap_or_else(|| "—".to_string());
            vec![
                Line::from(vec![
                    Span::styled("scope ", dim()),
                    Span::styled(
                        s.scope.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("   role ", dim()),
                    Span::raw(role),
                ]),
                Line::from(format!("id {}   ·   branch {}", id, s.branch)),
                Line::from(clip(&format!("worktree {}", s.worktree), inner_w)),
            ]
        }
        None => vec![
            Line::from("no active session lease"),
            Line::from(Span::styled(
                "the TUI shell isn't inside a session worktree",
                dim(),
            )),
        ],
    };
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_branch(frame: &mut Frame, area: Rect, model: &OverlayModel) {
    let block = Block::bordered().title(" Branch · PR · CI ");
    let mut lines: Vec<Line> = Vec::new();
    match &model.branch {
        Some(b) => {
            let dirty = if b.dirty {
                Span::styled("dirty", Style::default().fg(Color::Yellow))
            } else {
                Span::styled("clean", Style::default().fg(Color::Green))
            };
            let mut spans = vec![
                Span::styled(
                    b.name.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw("   "),
                dirty,
            ];
            if b.ahead_main != 0 || b.behind_main != 0 {
                spans.push(Span::styled(
                    format!("   ↑{} ↓{} main", b.ahead_main, b.behind_main),
                    dim(),
                ));
            }
            if b.has_upstream && (b.ahead_upstream != 0 || b.behind_upstream != 0) {
                spans.push(Span::styled(
                    format!("   ↑{} ↓{} origin", b.ahead_upstream, b.behind_upstream),
                    dim(),
                ));
            }
            lines.push(Line::from(spans));
        }
        None => lines.push(Line::from(Span::styled("branch status unavailable", dim()))),
    }
    lines.extend(pr_lines(model.pr.as_ref()));
    frame.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

/// Render `status.pr` — one or two [`Line`]s describing the PR + CI. The
/// JSON key is overloaded (skipped / error / none / full), so dispatch on
/// which fields are populated.
fn pr_lines(pr: Option<&PrInfo>) -> Vec<Line<'static>> {
    let Some(pr) = pr else {
        return vec![Line::from(Span::styled("PR   no data", dim()))];
    };
    if pr.skipped == Some(true) {
        return vec![Line::from(Span::styled(
            "PR   CI skipped — fetching…",
            dim(),
        ))];
    }
    if let Some(err) = &pr.error {
        let msg = match err.as_str() {
            "gh-missing" => "PR   gh not on PATH".to_string(),
            "gh-failed" => format!("PR   gh error: {}", pr.reason.clone().unwrap_or_default()),
            other => format!("PR   {}", other),
        };
        return vec![Line::from(Span::styled(
            msg,
            Style::default().fg(Color::Yellow),
        ))];
    }
    if pr.state.as_deref() == Some("none") {
        return vec![Line::from(Span::styled("PR   none for this branch", dim()))];
    }
    let Some(num) = pr.number else {
        return vec![Line::from(Span::styled("PR   (no number reported)", dim()))];
    };
    let mut spans = vec![
        Span::styled(
            format!("PR #{}", num),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("   {}", pr.state.clone().unwrap_or_default())),
    ];
    if let Some(t) = &pr.title {
        spans.push(Span::styled(format!("   {}", t), dim()));
    }
    let ci = pr.ci_rollup.clone().unwrap_or_else(|| "—".to_string());
    let ci_style = ci_style(&ci);
    vec![
        Line::from(spans),
        Line::from(vec![Span::raw("CI   "), Span::styled(ci, ci_style)]),
    ]
}

/// Colour a CI rollup string by keyword — green pass / red fail / yellow
/// in-flight. The exact rollup wording varies, so match on substrings.
fn ci_style(rollup: &str) -> Style {
    let r = rollup.to_ascii_lowercase();
    if r.contains("pass") || r.contains("green") || r.contains("success") {
        Style::default().fg(Color::Green)
    } else if r.contains("fail") || r.contains("red") || r.contains("error") {
        Style::default().fg(Color::Red)
    } else if r.contains("pend") || r.contains("run") || r.contains("progress") {
        Style::default().fg(Color::Yellow)
    } else {
        dim()
    }
}

fn render_queue(frame: &mut Frame, area: Rect, model: &OverlayModel) {
    let q = model.queue.clone().unwrap_or_default();
    let role = q
        .role
        .clone()
        .or_else(|| model.role.clone())
        .unwrap_or_else(|| "—".to_string());
    let block = Block::bordered().title(format!(" Queue — role {} · {} total ", role, q.total));
    let inner_w = area.width.saturating_sub(2) as usize;
    let mut lines: Vec<Line> = Vec::new();
    if q.head.is_empty() {
        lines.push(Line::from(Span::styled("queue empty for this role", dim())));
    } else {
        for (i, item) in q.head.iter().enumerate() {
            let head = format!("{}. {}  ", i + 1, item.spec_id);
            let tail = format!("[{}]", item.status);
            let budget = inner_w
                .saturating_sub(head.chars().count() + tail.chars().count() + 2)
                .max(4);
            lines.push(Line::from(vec![
                Span::styled(head, Style::default().fg(Color::Cyan)),
                Span::raw(clip(&item.title, budget)),
                Span::raw("  "),
                Span::styled(tail, dim()),
            ]));
        }
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_activity(frame: &mut Frame, area: Rect, activity: &[ActivityEntry]) {
    let block = Block::bordered().title(" Activity log ");
    let inner_w = area.width.saturating_sub(2) as usize;
    let inner_h = area.height.saturating_sub(2) as usize;
    let mut lines: Vec<Line> = Vec::new();
    if activity.is_empty() {
        lines.push(Line::from(Span::styled(
            "no actions run yet — pick one below and press enter",
            dim(),
        )));
    } else {
        for entry in activity {
            let (glyph, gstyle) = if entry.ok {
                ("✓", Style::default().fg(Color::Green))
            } else {
                ("✗", Style::default().fg(Color::Red))
            };
            let mut header = vec![
                Span::styled(format!("{} ", entry.when), dim()),
                Span::styled(format!("{} ", glyph), gstyle),
                Span::styled(
                    entry.label.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
            ];
            if !entry.command.is_empty() {
                header.push(Span::styled(format!("   $ {}", entry.command), dim()));
            }
            lines.push(Line::from(header));
            for out_line in &entry.lines {
                lines.push(Line::from(format!(
                    "  {}",
                    clip(out_line, inner_w.saturating_sub(2))
                )));
            }
        }
    }
    // Show the tail — the newest output stays visible as the log grows.
    if inner_h > 0 && lines.len() > inner_h {
        lines = lines.split_off(lines.len() - inner_h);
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_actions(frame: &mut Frame, area: Rect, selected: usize, confirm: Option<QuickAction>) {
    let block = Block::bordered().title(" Actions ");
    let mut buttons: Vec<Span> = Vec::new();
    for (i, action) in QuickAction::ALL.iter().enumerate() {
        let style = if i == selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        buttons.push(Span::styled(format!(" {} ", action.label()), style));
        buttons.push(Span::raw("  "));
    }
    let mut lines = vec![Line::from(buttons)];
    if let Some(action) = confirm {
        lines.push(Line::from(Span::styled(
            format!(
                "{}? — press y to confirm, any other key cancels",
                action.label()
            ),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_help(frame: &mut Frame, area: Rect) {
    let help = Line::from(vec![
        Span::styled(" ←/→ ", Style::default().fg(Color::Cyan)),
        Span::styled("select   ", dim()),
        Span::styled("enter ", Style::default().fg(Color::Cyan)),
        Span::styled("run   ", dim()),
        Span::styled("esc/q ", Style::default().fg(Color::Cyan)),
        Span::styled("close overlay (back to Claude)", dim()),
    ]);
    frame.render_widget(Paragraph::new(help), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Joined text of a [`Line`] — `Line::spans` and `Span::content` are
    /// both public, so this avoids depending on a `Display` impl.
    fn line_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// TASK-252: `short_session_id` truncates by char (not byte) and never
    /// panics on multi-byte UTF-8 — the old `&id[..12]` byte-slice would.
    #[test]
    fn short_session_id_is_char_safe() {
        // Normal ASCII hex id → first 12 chars.
        assert_eq!(short_session_id("019e2d4fd55baaaa"), "019e2d4fd55b");
        // Shorter than 12 → unchanged.
        assert_eq!(short_session_id("short"), "short");
        // All multi-byte: 14 emoji → exactly 12 chars, no panic.
        let emoji = "🎯".repeat(14);
        assert_eq!(short_session_id(&emoji).chars().count(), 12);
        // A char straddling byte boundary 12 (©=2 bytes) must not panic.
        let mixed = "ab©de©fg©hi©jklmnop";
        let _ = short_session_id(mixed);
    }

    const FIXTURE: &str = r#"{
      "branch": {"ahead_main":1,"behind_main":0,"ahead_upstream":0,"behind_upstream":0,"dirty":true,"has_upstream":true,"name":"epic-26-3"},
      "cache": {"fresh":true,"rows":721},
      "pr": {"skipped":true},
      "queue": {"head":[{"for_role":"implementer","spec_id":"STORY-133","status":"Approved","title":"AIDA TUI status overlay"}],"role":"implementer","total":9},
      "role":"implementer",
      "session": {"branch":"epic-26-3","id":"019e2d4fd55baaaa","role":"implementer","scope":"EPIC-26","started_at":"2026-05-15T20:24:27+00:00","worktree":"/home/joe/ai/aida-epic-26"}
    }"#;

    #[test]
    fn overlay_model_parses_status_json() {
        let m = parse(FIXTURE.as_bytes()).expect("fixture parses");
        assert_eq!(m.role.as_deref(), Some("implementer"));

        let s = m.session.expect("session present");
        assert_eq!(s.scope, "EPIC-26");
        assert_eq!(s.branch, "epic-26-3");

        let b = m.branch.expect("branch present");
        assert_eq!(b.name, "epic-26-3");
        assert!(b.dirty);
        assert_eq!(b.ahead_main, 1);

        let q = m.queue.expect("queue present");
        assert_eq!(q.total, 9);
        assert_eq!(q.head.len(), 1);
        assert_eq!(q.head[0].spec_id, "STORY-133");

        assert!(m.cache.expect("cache present").fresh);
        assert_eq!(m.pr.expect("pr present").skipped, Some(true));
    }

    #[test]
    fn parses_null_sections_without_error() {
        // `aida status` run outside a session: session + branch are null.
        let json = r#"{"session":null,"branch":null,"role":null,
          "queue":{"head":[],"role":"implementer","total":0}}"#;
        let m = parse(json.as_bytes()).expect("null sections parse");
        assert!(m.session.is_none());
        assert!(m.branch.is_none());
        assert_eq!(m.queue.expect("queue present").total, 0);
    }

    #[test]
    fn parses_minimal_queue_only_json() {
        // `--queue-only` drops every other section; the model still loads.
        let m = parse(br#"{"queue":{"head":[],"role":"reviewer","total":3}}"#)
            .expect("queue-only parses");
        assert!(m.session.is_none());
        assert!(m.cache.is_none());
        assert_eq!(
            m.queue.expect("queue present").role.as_deref(),
            Some("reviewer")
        );
    }

    #[test]
    fn pr_lines_cover_each_status_shape() {
        // Absent → placeholder.
        assert!(line_text(&pr_lines(None)[0]).contains("no data"));
        // gh missing.
        let missing = PrInfo {
            error: Some("gh-missing".to_string()),
            ..Default::default()
        };
        assert!(line_text(&pr_lines(Some(&missing))[0]).contains("gh not on PATH"));
        // No PR for the branch.
        let none = PrInfo {
            state: Some("none".to_string()),
            ..Default::default()
        };
        assert!(line_text(&pr_lines(Some(&none))[0]).contains("none for this branch"));
        // Full PR → number line + CI line.
        let open = PrInfo {
            number: Some(42),
            state: Some("OPEN".to_string()),
            ci_rollup: Some("passing".to_string()),
            ..Default::default()
        };
        let lines = pr_lines(Some(&open));
        assert_eq!(lines.len(), 2);
        assert!(line_text(&lines[0]).contains("#42"));
        assert!(line_text(&lines[1]).contains("passing"));
    }

    #[test]
    fn ci_style_keys_off_keywords() {
        assert_eq!(ci_style("passing").fg, Some(Color::Green));
        assert_eq!(ci_style("1 failing").fg, Some(Color::Red));
        assert_eq!(ci_style("in progress").fg, Some(Color::Yellow));
    }

    #[test]
    fn clip_adds_ellipsis_only_when_cut() {
        assert_eq!(clip("short", 20), "short");
        assert_eq!(clip("0123456789", 5), "0123…");
    }
}
