//! `aida config menu` — a scrollable, navigable TUI view of the project's
//! configurable items (STORY-661).
//!
//! AIDA's config surface is spread across `.aida/config.toml`, the global
//! `~/.aida/{config,agents}.toml`, and the `AIDA_*` environment knobs
//! documented in `docs/environment-variables.md`. `aida config show` prints
//! the resolved values, but a flat print of a couple dozen rows is hard to
//! scan and gives no per-knob explanation. This module renders the same
//! resolved surface as a one-screen navigable list: per row the knob name,
//! its current value, the built-in default, where the value was set (scope),
//! and a one-line explanation.
//!
//! The smallest-valuable-slice is **read + navigate** — the caller
//! (`aida-cli`) assembles the rows by reusing the existing config resolvers
//! (the same registry `aida config show` walks), so this module owns only the
//! presentation + navigation, never a parallel re-derivation of config values.
//! Inline editing of individual knobs is a deliberate follow-up.
//!
//! The terminal lifecycle reuses the crate's [`crate::term`] infra
//! (`TermGuard`, the panic hook, the SIGTERM/SIGINT restore) so a crash never
//! strands the user's terminal — the same guarantee `aida tui` ships.
//!
//! trace:STORY-661 | ai:claude

use crate::term::{install_panic_hook, install_signal_handler, TermGuard};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table,
    TableState, Wrap,
};
use ratatui::{Frame, Terminal};
use std::time::Duration;

/// One configurable item rendered in the menu. Plain data — the caller
/// (`aida-cli`) resolves these from the live config registry; this crate only
/// presents them. trace:STORY-661 | ai:claude
#[derive(Clone, Debug)]
pub struct ConfigMenuItem {
    /// The section header this knob belongs to (e.g. `[contained]`), used to
    /// group rows. trace:STORY-661 | ai:claude
    pub section: String,
    /// The knob's bare key (e.g. `os_wrap`).
    pub name: String,
    /// The current effective value, ANSI-stripped for clean TUI rendering.
    pub value: String,
    /// The built-in default (what you get with no config + no env override).
    pub default: String,
    /// Where the effective value was set: `default` / `.aida/config.toml` /
    /// `~/.aida/agents.toml` / `~/.aida/config.toml` / `<VAR> (env)`.
    pub scope: String,
    /// A one-line explanation of what the knob does.
    pub explanation: String,
}

/// A flattened, navigable row: either a section header or a knob row that
/// indexes back into the source `items`. trace:STORY-661 | ai:claude
enum DisplayRow {
    Header(String),
    Item(usize),
}

/// Run the `aida config menu` TUI over `items`. Read + navigate only:
/// up/down (or j/k) move the cursor, PgUp/PgDn page, g/G jump to top/bottom,
/// q/Esc quit. Returns once the user quits.
///
/// The caller is responsible for the no-TTY check — this enters raw mode and
/// the alternate screen via [`TermGuard`], which fails outside a real
/// terminal. trace:STORY-661 | ai:claude
pub fn run(items: Vec<ConfigMenuItem>) -> Result<()> {
    install_panic_hook();
    // Best-effort: a missing signal handler must not block the menu.
    let _ = install_signal_handler();
    let _guard = TermGuard::enter()?;

    let mut term = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;

    // Flatten items into display rows grouped by section header. The selectable
    // cursor only ever lands on Item rows; headers are skipped on navigation.
    let rows = build_display_rows(&items);
    let selectable: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, r)| matches!(r, DisplayRow::Item(_)).then_some(i))
        .collect();

    let mut cursor: usize = 0; // index into `selectable`
    loop {
        let table_state_selected = selectable.get(cursor).copied();
        term.draw(|f| draw(f, &items, &rows, table_state_selected))?;

        // Poll so a resize repaints without a key press; 200ms is plenty
        // responsive for a static read-only view.
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        if let Event::Key(key) = event::read()? {
            // crossterm on Windows fires both Press and Release; act on Press
            // only so a single keystroke doesn't move twice.
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let last = selectable.len().saturating_sub(1);
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                KeyCode::Down | KeyCode::Char('j') => {
                    if cursor < last {
                        cursor += 1;
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    cursor = cursor.saturating_sub(1);
                }
                KeyCode::PageDown => cursor = (cursor + 10).min(last),
                KeyCode::PageUp => cursor = cursor.saturating_sub(10),
                KeyCode::Home | KeyCode::Char('g') => cursor = 0,
                KeyCode::End | KeyCode::Char('G') => cursor = last,
                _ => {}
            }
        }
    }
    Ok(())
}

/// Flatten `items` into header + item display rows, one header per section in
/// first-seen order. trace:STORY-661 | ai:claude
fn build_display_rows(items: &[ConfigMenuItem]) -> Vec<DisplayRow> {
    let mut rows = Vec::new();
    let mut current: Option<&str> = None;
    for (i, item) in items.iter().enumerate() {
        if current != Some(item.section.as_str()) {
            rows.push(DisplayRow::Header(item.section.clone()));
            current = Some(item.section.as_str());
        }
        rows.push(DisplayRow::Item(i));
    }
    rows
}

/// Render one frame: a title bar, the scrollable table, an explanation panel
/// for the selected row, and a key-hint footer. trace:STORY-661 | ai:claude
fn draw(
    f: &mut Frame,
    items: &[ConfigMenuItem],
    rows: &[DisplayRow],
    selected_display_idx: Option<usize>,
) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // title
        Constraint::Min(3),    // table
        Constraint::Length(4), // explanation panel
        Constraint::Length(1), // footer
    ])
    .split(f.area());

    draw_title(f, chunks[0]);
    draw_table(f, chunks[1], items, rows, selected_display_idx);
    draw_explanation(f, chunks[2], items, rows, selected_display_idx);
    draw_footer(f, chunks[3]);
}

fn draw_title(f: &mut Frame, area: Rect) {
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            " AIDA config ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  configurable items — current value, default, scope, explanation"),
    ]));
    f.render_widget(title, area);
}

fn draw_table(
    f: &mut Frame,
    area: Rect,
    items: &[ConfigMenuItem],
    rows: &[DisplayRow],
    selected_display_idx: Option<usize>,
) {
    let header = Row::new(vec![
        Cell::from("Name"),
        Cell::from("Value"),
        Cell::from("Default"),
        Cell::from("Scope"),
    ])
    .style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );

    let table_rows: Vec<Row> = rows
        .iter()
        .map(|r| match r {
            DisplayRow::Header(section) => Row::new(vec![Cell::from(Span::styled(
                format!("[{section}]"),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ))]),
            DisplayRow::Item(i) => {
                let item = &items[*i];
                Row::new(vec![
                    Cell::from(Span::styled(
                        format!("  {}", item.name),
                        Style::default().fg(Color::Cyan),
                    )),
                    Cell::from(item.value.clone()),
                    Cell::from(Span::styled(
                        item.default.clone(),
                        Style::default().fg(Color::DarkGray),
                    )),
                    Cell::from(Span::styled(
                        item.scope.clone(),
                        Style::default().fg(Color::DarkGray),
                    )),
                ])
            }
        })
        .collect();

    let widths = [
        Constraint::Length(22),
        Constraint::Percentage(34),
        Constraint::Percentage(20),
        Constraint::Percentage(26),
    ];

    let mut state = TableState::default();
    state.select(selected_display_idx);

    let table = Table::new(table_rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(" Items "))
        .row_highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(table, area, &mut state);

    // Scrollbar so the user knows there's more below the fold.
    let mut sb_state = ScrollbarState::new(rows.len()).position(selected_display_idx.unwrap_or(0));
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None);
    f.render_stateful_widget(
        scrollbar,
        area.inner(ratatui::layout::Margin {
            vertical: 1,
            horizontal: 0,
        }),
        &mut sb_state,
    );
}

fn draw_explanation(
    f: &mut Frame,
    area: Rect,
    items: &[ConfigMenuItem],
    rows: &[DisplayRow],
    selected_display_idx: Option<usize>,
) {
    let text = match selected_display_idx.and_then(|i| rows.get(i)) {
        Some(DisplayRow::Item(i)) => {
            let item = &items[*i];
            vec![
                Line::from(vec![
                    Span::styled(
                        format!("[{}] {}", item.section, item.name),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        format!("(set via: {})", item.scope),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]),
                Line::from(item.explanation.clone()),
            ]
        }
        _ => vec![Line::from(Span::styled(
            "Select an item to see its explanation.",
            Style::default().fg(Color::DarkGray),
        ))],
    };
    let para = Paragraph::new(text)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title(" About "));
    f.render_widget(para, area);
}

fn draw_footer(f: &mut Frame, area: Rect) {
    let footer = Paragraph::new(Line::from(vec![
        Span::styled("↑/↓ j/k", Style::default().fg(Color::Cyan)),
        Span::raw(" move  "),
        Span::styled("PgUp/PgDn", Style::default().fg(Color::Cyan)),
        Span::raw(" page  "),
        Span::styled("g/G", Style::default().fg(Color::Cyan)),
        Span::raw(" top/bottom  "),
        Span::styled("q/Esc", Style::default().fg(Color::Cyan)),
        Span::raw(" quit   "),
        Span::styled(
            "(read-only — edit with `aida config <set>` or your config.toml)",
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    f.render_widget(footer, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(section: &str, name: &str) -> ConfigMenuItem {
        ConfigMenuItem {
            section: section.to_string(),
            name: name.to_string(),
            value: "v".to_string(),
            default: "d".to_string(),
            scope: "default".to_string(),
            explanation: "x".to_string(),
        }
    }

    #[test]
    fn display_rows_insert_one_header_per_section() {
        let items = vec![
            item("agents", "bypass"),
            item("contained", "enable"),
            item("contained", "os_wrap"),
        ];
        let rows = build_display_rows(&items);
        // 2 headers + 3 items = 5 display rows.
        assert_eq!(rows.len(), 5);
        let headers: Vec<&String> = rows
            .iter()
            .filter_map(|r| match r {
                DisplayRow::Header(s) => Some(s),
                _ => None,
            })
            .collect();
        assert_eq!(headers, vec!["agents", "contained"]);
    }

    #[test]
    fn display_rows_index_back_into_items_in_order() {
        let items = vec![item("a", "one"), item("a", "two")];
        let rows = build_display_rows(&items);
        let item_idxs: Vec<usize> = rows
            .iter()
            .filter_map(|r| match r {
                DisplayRow::Item(i) => Some(*i),
                _ => None,
            })
            .collect();
        assert_eq!(item_idxs, vec![0, 1]);
    }

    #[test]
    fn empty_items_produce_no_rows() {
        let rows = build_display_rows(&[]);
        assert!(rows.is_empty());
    }
}
