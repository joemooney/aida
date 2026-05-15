//! New-session picker — the `prefix n` modal that adds a tab (STORY-134).
//!
//! STORY-132 built the [`crate::tab::TabManager`] (add / remove / switch /
//! soft cap) and the prefix-key tab switching; STORY-133 the overlay.
//! This is the missing verb — opening another Claude session without
//! leaving the TUI. The picker offers two kinds of entry:
//!
//!   * **start** a queued spec fresh (`aida queue work <spec>
//!     --session-id <uuid>`), or
//!   * **resume** a recorded Claude conversation for the launch scope
//!     (`aida queue work <scope> --resume <id>`, TASK-112).
//!
//! Either way the hosted child is `aida queue work`, never `claude`
//! directly — all lease / worktree / manifest logic is inherited.
//!
//! trace:STORY-134 | ai:claude

use crate::overlay;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use std::process::Command;

/// One selectable row in the new-session picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerEntry {
    /// Start a queued spec in a fresh, TUI-tracked conversation.
    Fresh {
        spec_id: String,
        title: String,
        status: String,
    },
    /// Resume a recorded Claude conversation for `scope`.
    Resume {
        scope: String,
        session_id: String,
        label: String,
    },
}

impl PickerEntry {
    /// The scope string recorded on the resulting [`crate::tab::Session
    /// Tab`] — the spec id for a fresh start, the original scope for a
    /// resume.
    pub fn scope(&self) -> &str {
        match self {
            PickerEntry::Fresh { spec_id, .. } => spec_id,
            PickerEntry::Resume { scope, .. } => scope,
        }
    }

    /// One-line label for the picker list.
    pub fn display(&self) -> String {
        match self {
            PickerEntry::Fresh {
                spec_id,
                title,
                status,
            } => format!("start   {}  {}  [{}]", spec_id, title, status),
            PickerEntry::Resume {
                scope,
                session_id,
                label,
            } => {
                let short = &session_id[..session_id.len().min(8)];
                if label.is_empty() {
                    format!("resume  {}  session {}", scope, short)
                } else {
                    format!("resume  {}  {}  ({})", scope, short, label)
                }
            }
        }
    }
}

/// State for the `prefix n` picker — what `App` carries while
/// [`crate::app::Mode::Picker`] is active.
pub struct PickerState {
    /// Candidate sessions, queued-fresh entries first then resumables.
    pub entries: Vec<PickerEntry>,
    /// Index of the highlighted entry.
    pub selected: usize,
    /// Transient note (e.g. the tab cap was hit) shown under the list.
    pub note: Option<String>,
}

impl PickerState {
    pub fn new(entries: Vec<PickerEntry>) -> Self {
        PickerState {
            entries,
            selected: 0,
            note: None,
        }
    }

    /// An empty picker — nothing queued and nothing to resume.
    pub fn empty() -> Self {
        PickerState::new(Vec::new())
    }

    /// Highlight the next entry, wrapping past the end.
    pub fn select_next(&mut self) {
        if !self.entries.is_empty() {
            self.selected = (self.selected + 1) % self.entries.len();
        }
    }

    /// Highlight the previous entry, wrapping past the start.
    pub fn select_prev(&mut self) {
        if !self.entries.is_empty() {
            let n = self.entries.len();
            self.selected = (self.selected + n - 1) % n;
        }
    }

    /// The highlighted entry, if the picker is non-empty.
    pub fn selected_entry(&self) -> Option<&PickerEntry> {
        self.entries.get(self.selected)
    }
}

/// Build the picker's candidate list. `scope` is the TUI's launch scope
/// (resume entries are offered for it); `open_session_ids` are the full
/// ids of already-hosted tabs, filtered out of the resume list so the
/// picker never offers a conversation that is already on screen.
pub fn fetch(scope: Option<&str>, open_session_ids: &[String]) -> PickerState {
    let mut entries: Vec<PickerEntry> = Vec::new();

    // Queued specs → start fresh. Reuse the overlay's cache-only status
    // fetch (sub-millisecond) for the queue head.
    if let Ok(model) = overlay::fetch(true) {
        if let Some(queue) = model.queue {
            for item in queue.head {
                entries.push(PickerEntry::Fresh {
                    spec_id: item.spec_id,
                    title: item.title,
                    status: item.status,
                });
            }
        }
    }

    // Recorded conversations for the launch scope → resume. Drop any
    // already hosted in a tab (matched by id prefix — `--list-sessions`
    // prints short ids, the tab carries the full UUID).
    if let Some(scope) = scope {
        for (sid, label) in list_scope_sessions(scope) {
            let already_open = open_session_ids
                .iter()
                .any(|open| open.starts_with(&sid) || sid.starts_with(open.as_str()));
            if already_open {
                continue;
            }
            entries.push(PickerEntry::Resume {
                scope: scope.to_string(),
                session_id: sid,
                label,
            });
        }
    }

    PickerState::new(entries)
}

/// Recorded Claude sessions for `scope`, via `aida queue work <scope>
/// --list-sessions` (TASK-112). Returns `(session_id, label)` pairs.
fn list_scope_sessions(scope: &str) -> Vec<(String, String)> {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(["queue", "work", scope, "--list-sessions"]);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    match cmd.output() {
        Ok(o) if o.status.success() => parse_list_sessions(&String::from_utf8_lossy(&o.stdout)),
        _ => Vec::new(),
    }
}

/// Parse `aida queue work --list-sessions` output into `(id, label)`
/// pairs. Session rows start (after indentation) with a `●`/`○` bullet
/// then the id; header and footer lines have no bullet and are skipped.
/// Split out so it is testable without spawning `aida`.
pub fn parse_list_sessions(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let body = line.trim_start();
        let Some(rest) = body.strip_prefix('●').or_else(|| body.strip_prefix('○')) else {
            continue;
        };
        let rest = rest.trim();
        let Some(id) = rest.split_whitespace().next() else {
            continue;
        };
        // A session id is a hex run (full UUID or short prefix).
        if id.len() < 6 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        let label = rest
            .strip_prefix(id)
            .unwrap_or("")
            .trim()
            .trim_matches(|c| c == '(' || c == ')')
            .trim()
            .to_string();
        out.push((id.to_string(), label));
    }
    out
}

/// Draw the picker — a full-screen modal (one tab is focused at a time,
/// so there is nothing to keep visible behind it).
pub fn render(frame: &mut Frame, state: &PickerState, at_cap: bool) {
    let rows = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Min(3),    // entry list
        Constraint::Length(2), // note + help
    ])
    .split(frame.area());

    let header = Line::from(vec![
        Span::styled(
            " new session ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "  pick a queued spec to start, or a session to resume",
            dim(),
        ),
    ]);
    frame.render_widget(Paragraph::new(header), rows[0]);

    render_list(frame, rows[1], state);

    let mut footer: Vec<Line> = Vec::new();
    if at_cap {
        footer.push(Line::from(Span::styled(
            "tab cap reached — close a tab before opening another",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
    } else if let Some(note) = &state.note {
        footer.push(Line::from(Span::styled(
            note.clone(),
            Style::default().fg(Color::Yellow),
        )));
    } else {
        footer.push(Line::from(Span::raw("")));
    }
    footer.push(Line::from(vec![
        Span::styled(" ↑/↓ ", Style::default().fg(Color::Cyan)),
        Span::styled("select   ", dim()),
        Span::styled("enter ", Style::default().fg(Color::Cyan)),
        Span::styled("open in new tab   ", dim()),
        Span::styled("esc ", Style::default().fg(Color::Cyan)),
        Span::styled("cancel", dim()),
    ]));
    frame.render_widget(Paragraph::new(footer), rows[2]);
}

fn render_list(frame: &mut Frame, area: Rect, state: &PickerState) {
    let block = Block::bordered().title(" Sessions ");
    let inner_w = area.width.saturating_sub(2) as usize;
    let inner_h = area.height.saturating_sub(2) as usize;

    if state.entries.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "nothing queued and no recorded sessions to resume — \
             queue work with `aida queue add`, then reopen this picker",
            dim(),
        )))
        .block(block);
        frame.render_widget(empty, area);
        return;
    }

    // Window the list around the selection so a long resume list scrolls.
    let start = if inner_h > 0 && state.selected >= inner_h {
        state.selected - inner_h + 1
    } else {
        0
    };
    let mut lines: Vec<Line> = Vec::new();
    for (i, entry) in state.entries.iter().enumerate().skip(start).take(inner_h) {
        let marker = if i == state.selected { "▸ " } else { "  " };
        let text = format!("{}{}", marker, entry.display());
        let clipped: String = text.chars().take(inner_w.max(4)).collect();
        let style = if i == state.selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(clipped, style)));
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// Dimmed style for secondary text.
fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_list_sessions_keeps_only_bulleted_rows() {
        let text = "\
3 claude session(s) for EPIC-26 (most recent first):
  ● 019e2d4f    0s  implementer  (TUI overlay)
  ○ 019e2c1a    2h  reviewer     (untitled)
  ● 019e2b07   1d  -            (status work)

  resume one:  aida queue work EPIC-26 --resume <session-id>
";
        let got = parse_list_sessions(text);
        assert_eq!(got.len(), 3, "header + footer + blank line dropped");
        assert_eq!(got[0].0, "019e2d4f");
        assert!(got[0].1.contains("implementer"));
        assert_eq!(got[2].0, "019e2b07");
    }

    #[test]
    fn parse_list_sessions_empty_when_no_rows() {
        assert!(parse_list_sessions("0 claude session(s) for X").is_empty());
        assert!(parse_list_sessions("").is_empty());
    }

    #[test]
    fn picker_selection_wraps_both_ways() {
        let mut s = PickerState::new(vec![
            PickerEntry::Fresh {
                spec_id: "STORY-1".into(),
                title: "a".into(),
                status: "Approved".into(),
            },
            PickerEntry::Resume {
                scope: "EPIC-26".into(),
                session_id: "019e2d4f1111".into(),
                label: "x".into(),
            },
        ]);
        assert_eq!(s.selected, 0);
        s.select_prev(); // wrap to the end
        assert_eq!(s.selected, 1);
        s.select_next(); // wrap back to the start
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn empty_picker_select_is_a_no_op() {
        let mut s = PickerState::empty();
        s.select_next();
        s.select_prev();
        assert_eq!(s.selected, 0);
        assert!(s.selected_entry().is_none());
    }

    #[test]
    fn entry_scope_and_display() {
        let fresh = PickerEntry::Fresh {
            spec_id: "STORY-9".into(),
            title: "thing".into(),
            status: "Approved".into(),
        };
        assert_eq!(fresh.scope(), "STORY-9");
        assert!(fresh.display().starts_with("start   STORY-9"));

        let resume = PickerEntry::Resume {
            scope: "EPIC-26".into(),
            session_id: "019e2d4f9999".into(),
            label: String::new(),
        };
        assert_eq!(resume.scope(), "EPIC-26");
        assert!(resume.display().contains("019e2d4f"));
    }
}
