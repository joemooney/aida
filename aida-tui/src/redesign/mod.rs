//! The `aida tui` **action→target redesign** prototype — Slice 1.
//!
//! A throwaway-able keystone that validates the gesture grammar from
//! `docs/plans/2026-06-25-tui-action-target-redesign.md` on ONE scope
//! (Backlog) and ONE functional verb (groom). It is gated behind the
//! `AIDA_TUI_REDESIGN=1` env toggle (see [`enabled`]); the existing TUI is
//! completely unchanged without it.
//!
//! The protocol: **scope → action → targets → execute**. The top panel
//! holds the scopes, then a scope's verbs after a drill; the bottom panel
//! is the multi-selectable target set; Enter on a verb runs it on the
//! selection (or confirms "apply to all N?"); `p` previews an item in a
//! modal; Esc pops the navigation stack; the status line carries the
//! breadcrumb + role + counts.
//!
//! All pure logic lives in [`state`] (and is unit-tested there). This
//! module owns only the IO: the terminal guard, the backlog fetch, the
//! render, and the keystroke→transition wiring.
//!
//! trace:STORY-690 | ai:claude

mod state;

pub use state::{RedesignState, RunOutcome, Scope, TargetItem, Verb};

use crate::dashboard;
use crate::term;
use crate::theme::Theme;
use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use state::{Focus, Level};
use std::io::Stdout;
use std::process::Command;

/// Is the redesign prototype toggled on? Checked by `aida_tui::run` so the
/// existing TUI is the default and the prototype is strictly opt-in.
pub fn enabled() -> bool {
    matches!(
        std::env::var("AIDA_TUI_REDESIGN").ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

/// Launch the redesign prototype. Owns the terminal via the same RAII
/// guard the rest of the TUI uses, so a panic or a normal exit never
/// strands the terminal in raw mode.
pub fn run(theme: Theme) -> Result<()> {
    term::install_panic_hook();
    term::install_signal_handler()?;

    let items = fetch_backlog();
    let mut st = RedesignState::new(items, resolve_role());
    st.theme = theme;
    st.status = Some("Slice 1 prototype — Backlog › groom. ? exits.".to_string());

    let _guard = term::TermGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    terminal.clear()?;

    event_loop(&mut terminal, &mut st)?;
    Ok(())
}

/// The shell's role lens (ambient context in the status line). Mirrors the
/// CLI's role resolution loosely — Slice 1 only displays it.
fn resolve_role() -> String {
    std::env::var("AIDA_SESSION_ROLE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "advisor".to_string())
}

/// Load the Backlog target set via the same cache-fast data path the
/// dashboard uses (`aida list --status approved,planned --json`). We reuse
/// [`dashboard::parse_list_json`] rather than re-shelling `aida show` per
/// row (the §7 anti-pattern); the per-item body is fetched lazily, only
/// when its modal opens. trace:STORY-690 | ai:claude
fn fetch_backlog() -> Vec<TargetItem> {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(["list", "--status", "approved,planned", "--json"]);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    let Ok(out) = cmd.output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    dashboard::parse_list_json(&out.stdout)
        .into_iter()
        .map(|r| TargetItem {
            id: r.spec_id,
            title: r.title,
            status: r.status,
            // Body is fetched on demand when the modal opens; empty until
            // then so cursor movement never shells out.
            body: String::new(),
        })
        .collect()
}

/// Lazily fill an item's body for the modal (single `aida show <id>`,
/// cached into the item). Cheap because it only fires on modal-open, never
/// on cursor move. trace:STORY-690 | ai:claude
fn ensure_body(st: &mut RedesignState, idx: usize) {
    let Some(item) = st.items.get(idx) else {
        return;
    };
    if !item.body.is_empty() {
        return;
    }
    let id = item.id.clone();
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(["show", &id, "--no-git"]);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    let body = match cmd.output() {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).into_owned(),
        _ => format!("(could not load body for {id})"),
    };
    if let Some(item) = st.items.get_mut(idx) {
        item.body = body;
    }
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    st: &mut RedesignState,
) -> Result<()> {
    loop {
        terminal.draw(|f| render(f, st))?;
        let Event::Key(key) = crossterm::event::read()? else {
            continue;
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            continue;
        }
        if handle_key(terminal, st, key)? {
            break;
        }
    }
    Ok(())
}

/// Route one keystroke. Returns `Ok(true)` when the app should exit.
fn handle_key(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    st: &mut RedesignState,
    key: KeyEvent,
) -> Result<bool> {
    // Ctrl-C always quits.
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return Ok(true);
    }

    // A confirmation popup captures input until resolved.
    if st.confirm.is_some() {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                let outcome = st.resolve_confirm(true);
                apply_outcome(terminal, st, outcome)?;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                st.resolve_confirm(false);
                st.status = Some("cancelled".to_string());
            }
            _ => {}
        }
        return Ok(false);
    }

    // A modal captures Esc / q / p (close) and nothing else mutating.
    if st.modal.is_some() {
        if matches!(
            key.code,
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('p')
        ) {
            st.close_modal();
        }
        return Ok(false);
    }

    match key.code {
        // `?` exits the prototype (a bare `q` is reserved for the modal /
        // could be typed into a filter, so the exit key is unambiguous).
        KeyCode::Char('?') => return Ok(true),

        KeyCode::Up => st.move_up(),
        KeyCode::Down => st.move_down(),

        KeyCode::Tab => st.focus_bottom(),
        KeyCode::BackTab => st.focus_top(),

        KeyCode::Char(' ') => st.toggle_select(),
        KeyCode::Char('a') if st.focus == Focus::Bottom => st.select_all(),
        KeyCode::Char('A') if st.focus == Focus::Bottom => st.select_none(),

        KeyCode::Char('p') => {
            if st.focus == Focus::Bottom {
                open_modal_with_body(st);
            }
        }

        KeyCode::Esc => {
            if !st.pop() {
                // Esc at the top-of-stack scope level exits.
                return Ok(true);
            }
        }

        KeyCode::Enter => match (st.focus, st.level) {
            // Scope level: Enter drills into the highlighted scope.
            (Focus::Top, Level::Scopes) => {
                st.drill();
            }
            // Verb level, top focus: Enter runs the verb.
            (Focus::Top, Level::Verbs) => {
                let outcome = st.run_verb();
                apply_outcome(terminal, st, outcome)?;
            }
            // Bottom focus: Enter on an item opens its modal (the N=1
            // "preview this spec" case of the same protocol).
            (Focus::Bottom, _) => {
                open_modal_with_body(st);
            }
        },

        KeyCode::Backspace => st.pop_filter(),

        // Type-to-fuzzy-filter the focused list. Printable chars only; the
        // bottom-panel select-all/none shortcuts above already claimed
        // `a`/`A` when focused there, so they won't reach the filter.
        KeyCode::Char(c) if !c.is_control() => st.push_filter(c),

        _ => {}
    }
    Ok(false)
}

fn open_modal_with_body(st: &mut RedesignState) {
    let idxs = st.bottom_indices();
    if let Some(&real) = idxs.get(st.bottom_idx) {
        ensure_body(st, real);
    }
    st.open_modal();
}

/// Turn a [`RunOutcome`] into IO. Slice 1 STUBS the actual groom: it logs
/// the verb + target ids to the status line. Wiring the real groom (shell
/// out to `aida` / the grooming skill) is a later slice — the loop and the
/// selection are what Slice 1 validates. trace:STORY-690 | ai:claude
fn apply_outcome(
    _terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    st: &mut RedesignState,
    outcome: RunOutcome,
) -> Result<()> {
    match outcome {
        RunOutcome::Execute { verb, ids } => {
            // TODO(Slice 2+): replace this stub with the real verb wiring —
            // e.g. shell out to `aida` (groom = the backlog-groom skill /
            // `aida intake`) or emit an intent for the bash wrapper. Slice
            // 1 only proves the selection + gesture loop, so we log instead.
            let preview: Vec<&str> = ids.iter().take(5).map(|s| s.as_str()).collect();
            let more = if ids.len() > 5 {
                format!(" +{} more", ids.len() - 5)
            } else {
                String::new()
            };
            st.status = Some(format!(
                "[stub] {} {} item(s): {}{}  (TODO: wire real {})",
                verb.label(),
                ids.len(),
                preview.join(", "),
                more,
                verb.label(),
            ));
        }
        RunOutcome::NeedsConfirm(_) => { /* popup already raised by run_verb */ }
        RunOutcome::None => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

fn render(f: &mut Frame, st: &RedesignState) {
    let theme = &st.theme;

    let rows = Layout::vertical([
        Constraint::Length(1), // status / breadcrumb line
        Constraint::Min(0),    // top panel (list)
        Constraint::Min(0),    // bottom panel (targets)
        Constraint::Length(1), // key hint
    ])
    .split(f.area());

    render_status(f, rows[0], st, theme);
    render_top(f, rows[1], st, theme);
    render_bottom(f, rows[2], st, theme);
    render_hint(f, rows[3], st, theme);

    if let Some(idx) = st.modal {
        render_modal(f, f.area(), st, theme, idx);
    }
    if let Some(c) = st.confirm {
        render_confirm(f, f.area(), theme, c.verb, c.count);
    }
}

fn render_status(f: &mut Frame, area: Rect, st: &RedesignState, theme: &Theme) {
    let breadcrumb = st.breadcrumb();
    let sel = st.selected_count();
    let counts = format!(
        "role: {} · {} item(s) · {} selected",
        st.role,
        st.items.len(),
        sel
    );
    let spans = vec![
        Span::styled(
            format!(" {breadcrumb} "),
            Style::default()
                .fg(theme.on_accent)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(counts, Style::default().fg(theme.dim)),
    ];
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_top(f: &mut Frame, area: Rect, st: &RedesignState, theme: &Theme) {
    let focused = st.focus == Focus::Top;
    let title = match st.level {
        Level::Scopes => " Scopes ".to_string(),
        Level::Verbs => format!(" {} › verbs ", st.scope.map(|s| s.label()).unwrap_or("")),
    };
    let block = panel_block(title, focused, theme);
    let inner_h = area.height.saturating_sub(2) as usize;

    let idxs = st.top_indices();
    let mut lines: Vec<Line> = Vec::new();
    for (row, &real) in idxs.iter().enumerate() {
        let selected = row == st.top_idx;
        let (glyph, label, hint, drills) = match st.level {
            Level::Scopes => {
                let s = Scope::all()[real];
                // A scope is a noun with children → `›` (drill).
                (
                    if s.is_functional() { "›" } else { "·" },
                    s.label(),
                    s.hint(),
                    s.is_functional(),
                )
            }
            Level::Verbs => {
                let v = st.scope.map(|s| s.verbs()).unwrap_or_default()[real];
                // A verb is a leaf action → `↵` (run).
                ("↵", v.label(), v.hint(), false)
            }
        };
        let marker = if selected { "▸ " } else { "  " };
        let dim_label = !drills && st.level == Level::Scopes; // non-wired scopes are dimmed
        let style = row_style(theme, selected && focused, dim_label);
        let line = Line::from(vec![
            Span::styled(marker, style),
            Span::styled(format!("{glyph} "), style),
            Span::styled(format!("{label:<10}"), style.add_modifier(Modifier::BOLD)),
            Span::styled(format!("  {hint}"), Style::default().fg(theme.dim)),
        ]);
        lines.push(line);
    }
    if focused && !st.filter.is_empty() {
        lines.insert(
            0,
            Line::from(Span::styled(
                format!("> {}", st.filter),
                Style::default().fg(theme.info),
            )),
        );
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "(no matches)",
            Style::default().fg(theme.dim),
        )));
    }
    lines.truncate(inner_h.max(1));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_bottom(f: &mut Frame, area: Rect, st: &RedesignState, theme: &Theme) {
    let focused = st.focus == Focus::Bottom;
    let block = panel_block(" Targets ".to_string(), focused, theme);
    let inner_w = area.width.saturating_sub(2) as usize;
    let inner_h = area.height.saturating_sub(2) as usize;

    let idxs = st.bottom_indices();
    if st.items.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(
                "(no backlog items — file some with `aida add --status approved`)",
                Style::default().fg(theme.dim),
            ))
            .block(block),
            area,
        );
        return;
    }

    // Scroll so the cursor stays visible.
    let start = if inner_h > 0 && st.bottom_idx >= inner_h {
        st.bottom_idx - inner_h + 1
    } else {
        0
    };

    let mut lines: Vec<Line> = Vec::new();
    for (row, &real) in idxs.iter().enumerate().skip(start).take(inner_h) {
        let item = &st.items[real];
        let is_sel = st.selected[real];
        let cursor = row == st.bottom_idx;
        let checkbox = if is_sel { "[x]" } else { "[ ]" };
        let marker = if cursor { "▸" } else { " " };
        let text = format!(
            "{marker}{checkbox} {}  {}  [{}]",
            item.id, item.title, item.status
        );
        let clipped: String = text.chars().take(inner_w.max(4)).collect();
        let style = row_style(theme, cursor && focused, false);
        let style = if is_sel && !(cursor && focused) {
            style.fg(theme.info)
        } else {
            style
        };
        lines.push(Line::from(Span::styled(clipped, style)));
    }
    if focused && !st.filter.is_empty() {
        lines.insert(
            0,
            Line::from(Span::styled(
                format!("> {}", st.filter),
                Style::default().fg(theme.info),
            )),
        );
    }
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_hint(f: &mut Frame, area: Rect, st: &RedesignState, theme: &Theme) {
    let base = match (st.focus, st.level) {
        (Focus::Top, Level::Scopes) => "↵ drill · Tab items · ? quit",
        (Focus::Top, Level::Verbs) => "↵ run · Tab items · ⇧Tab scopes? Esc back · ? quit",
        (Focus::Bottom, _) => "Space select · a/A all/none · p preview · ⇧Tab back · Esc back",
    };
    let text = st.status.clone().unwrap_or_else(|| base.to_string());
    f.render_widget(
        Paragraph::new(Span::styled(text, Style::default().fg(theme.dim))),
        area,
    );
}

fn render_modal(f: &mut Frame, area: Rect, st: &RedesignState, theme: &Theme, idx: usize) {
    let Some(item) = st.items.get(idx) else {
        return;
    };
    let popup = centered(area, 80, 80);
    f.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_style(Style::default().fg(theme.accent))
        .title(format!(" {} — {} ", item.id, item.status));
    // Slice 1: a plain Paragraph. STORY-689 makes this markdown + field
    // color-coding (Slice 4). trace:STORY-690 | ai:claude
    let body = if item.body.is_empty() {
        format!("{}\n\n(loading…)", item.title)
    } else {
        item.body.clone()
    };
    let para = Paragraph::new(body)
        .block(block)
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(theme.fg));
    f.render_widget(para, popup);
}

fn render_confirm(f: &mut Frame, area: Rect, theme: &Theme, verb: Verb, count: usize) {
    let popup = centered(area, 50, 20);
    f.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_style(Style::default().fg(theme.warn))
        .title(" confirm ");
    let lines = vec![
        Line::from(Span::styled(
            format!("Nothing selected. {} all {count} item(s)?", verb.label()),
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "y / Enter = yes   ·   n / Esc = no",
            Style::default().fg(theme.dim),
        )),
    ];
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .alignment(Alignment::Center),
        popup,
    );
}

// --- small render helpers --------------------------------------------------

fn panel_block(title: String, focused: bool, theme: &Theme) -> Block<'static> {
    let border = if focused { theme.accent } else { theme.border };
    Block::bordered()
        .border_style(Style::default().fg(border))
        .title(title)
}

fn row_style(theme: &Theme, active: bool, dim: bool) -> Style {
    if active {
        Style::default()
            .fg(theme.on_accent)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else if dim {
        Style::default().fg(theme.dim)
    } else {
        Style::default().fg(theme.fg)
    }
}

/// A centered popup `pct_w` × `pct_h` percent of `area`.
fn centered(area: Rect, pct_w: u16, pct_h: u16) -> Rect {
    let vert = Layout::vertical([
        Constraint::Percentage((100 - pct_h) / 2),
        Constraint::Percentage(pct_h),
        Constraint::Percentage((100 - pct_h) / 2),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - pct_w) / 2),
        Constraint::Percentage(pct_w),
        Constraint::Percentage((100 - pct_w) / 2),
    ])
    .split(vert[1])[1]
}

#[cfg(test)]
mod render_tests {
    //! Render smoke tests — drive the IO-side `render` over a headless
    //! [`TestBackend`] to prove the two-panel layout and the modal /
    //! confirm overlays paint without panicking at a realistic and a tiny
    //! terminal size. (The interaction logic itself is covered by the pure
    //! tests in `state`.) trace:STORY-690 | ai:claude
    use super::*;
    use ratatui::backend::TestBackend;

    fn sample(n: usize) -> RedesignState {
        let items = (0..n)
            .map(|i| TargetItem {
                id: format!("STORY-{i}"),
                title: format!("a sample backlog item {i}"),
                status: "Approved".into(),
                body: format!("# STORY-{i}\n\nbody text here"),
            })
            .collect();
        RedesignState::new(items, "advisor")
    }

    fn draw(st: &RedesignState, w: u16, h: u16) {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test backend");
        terminal.draw(|f| render(f, st)).expect("render");
    }

    #[test]
    fn renders_scope_level() {
        draw(&sample(5), 100, 30);
    }

    #[test]
    fn renders_verb_level_with_selection() {
        let mut st = sample(5);
        st.drill();
        st.focus_bottom();
        st.toggle_select();
        st.focus_top();
        draw(&st, 100, 30);
    }

    #[test]
    fn renders_item_modal() {
        let mut st = sample(5);
        st.drill();
        st.focus_bottom();
        st.open_modal();
        draw(&st, 100, 30);
    }

    #[test]
    fn renders_confirm_popup() {
        let mut st = sample(5);
        st.drill();
        st.run_verb(); // raises the confirm-all popup
        draw(&st, 100, 30);
    }

    #[test]
    fn renders_into_a_tiny_terminal_without_panicking() {
        draw(&sample(5), 20, 6);
        let mut st = sample(0); // empty backlog
        st.drill();
        draw(&st, 20, 6);
    }
}
