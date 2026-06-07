//! Launcher left-nav panel — section selector (STORY-244).
//!
//! Renders the vertical list on the left of the launcher dashboard:
//! Queue / Backlog / History / PRs / Sessions, then an action verb block
//! (Drain queue / New session / Switch role) separated by a horizontal
//! rule. Pure state + render; the parent dashboard owns the data each
//! section drives. trace:STORY-244 | ai:claude

use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

/// One left-nav row. The first five drive the middle list; the last three
/// are action verbs — selecting one emits an [`crate::intent::Intent`]
/// directly without changing the middle list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavSection {
    Queue,
    Backlog,
    History,
    Prs,
    Sessions,
    /// Action: start an autonomous drain via `/aida-drain-queue`.
    ActionDrain,
    /// Action: open the new-session picker (a fresh `aida queue work` on
    /// the head of the queue).
    ActionNewSession,
    /// Action: cycle the role tab (also exposed as the `r` direct key).
    ActionSwitchRole,
}

impl NavSection {
    /// Label rendered in the left-nav list.
    pub fn label(self) -> &'static str {
        match self {
            NavSection::Queue => "Queue",
            NavSection::Backlog => "Backlog",
            NavSection::History => "History",
            NavSection::Prs => "PRs",
            NavSection::Sessions => "Sessions",
            NavSection::ActionDrain => "/aida-drain",
            NavSection::ActionNewSession => "New session",
            NavSection::ActionSwitchRole => "Switch role",
        }
    }

    /// True for the five list sections (which populate the middle list).
    /// False for the action verbs (which emit an Intent directly).
    pub fn is_list_section(self) -> bool {
        matches!(
            self,
            NavSection::Queue
                | NavSection::Backlog
                | NavSection::History
                | NavSection::Prs
                | NavSection::Sessions
        )
    }

    /// Default ordered nav list.
    pub fn all() -> Vec<NavSection> {
        vec![
            NavSection::Queue,
            NavSection::Backlog,
            NavSection::History,
            NavSection::Prs,
            NavSection::Sessions,
            NavSection::ActionDrain,
            NavSection::ActionNewSession,
            NavSection::ActionSwitchRole,
        ]
    }
}

/// Selection state for the left nav.
#[derive(Debug, Clone)]
pub struct NavState {
    pub sections: Vec<NavSection>,
    pub selected: usize,
}

impl Default for NavState {
    fn default() -> Self {
        NavState {
            sections: NavSection::all(),
            selected: 0,
        }
    }
}

impl NavState {
    /// Currently-highlighted section.
    pub fn current(&self) -> NavSection {
        self.sections[self.selected.min(self.sections.len() - 1)]
    }

    /// Move to the next section, wrapping. The dashboard treats wrapping
    /// as cheap — there are only 8 entries.
    #[allow(dead_code)]
    pub fn select_next(&mut self) {
        if self.sections.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.sections.len();
    }

    /// Move to the previous section, wrapping.
    #[allow(dead_code)]
    pub fn select_prev(&mut self) {
        if self.sections.is_empty() {
            return;
        }
        let n = self.sections.len();
        self.selected = (self.selected + n - 1) % n;
    }

    /// Jump to a specific section (e.g. the `q`/`b`/`h`/`p`/`s` direct
    /// keys); no-op when the section isn't in the list.
    pub fn select(&mut self, section: NavSection) {
        if let Some(idx) = self.sections.iter().position(|s| *s == section) {
            self.selected = idx;
        }
    }
}

/// Draw the left-nav panel into `area`. A divider row separates list
/// sections from action verbs. Colors resolve through `theme`.
/// trace:TASK-256 | ai:claude
pub fn render(frame: &mut Frame, area: Rect, state: &NavState, theme: &Theme) {
    let block = Block::bordered()
        .border_style(Style::default().fg(theme.border))
        .title(" Nav ");
    let inner_w = area.width.saturating_sub(2) as usize;

    let mut lines: Vec<Line> = Vec::new();
    let mut saw_action = false;
    for (i, section) in state.sections.iter().enumerate() {
        // Insert a rule between the last list section and the first
        // action verb.
        if !section.is_list_section() && !saw_action {
            lines.push(Line::from(Span::styled(
                "─".repeat(inner_w),
                Style::default().fg(theme.dim),
            )));
            saw_action = true;
        }
        let marker = if i == state.selected { "▸ " } else { "  " };
        let style = if i == state.selected {
            Style::default()
                .fg(theme.on_accent)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else if section.is_list_section() {
            Style::default().fg(theme.fg)
        } else {
            // Action verbs are dimmer until selected.
            Style::default().fg(theme.dim)
        };
        let text = format!("{marker}{}", section.label());
        let clipped: String = text.chars().take(inner_w.max(4)).collect();
        lines.push(Line::from(Span::styled(clipped, style)));
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nav_default_starts_on_queue() {
        let nav = NavState::default();
        assert_eq!(nav.current(), NavSection::Queue);
        assert!(nav.current().is_list_section());
    }

    #[test]
    fn nav_wraps_both_ways() {
        let mut nav = NavState::default();
        nav.select_prev(); // wrap to the end (Switch role)
        assert_eq!(nav.current(), NavSection::ActionSwitchRole);
        nav.select_next(); // wrap back to start
        assert_eq!(nav.current(), NavSection::Queue);
    }

    #[test]
    fn nav_select_jumps_to_section() {
        let mut nav = NavState::default();
        nav.select(NavSection::Prs);
        assert_eq!(nav.current(), NavSection::Prs);
        nav.select(NavSection::ActionDrain);
        assert_eq!(nav.current(), NavSection::ActionDrain);
        assert!(!nav.current().is_list_section());
    }

    #[test]
    fn is_list_section_classifies_correctly() {
        assert!(NavSection::Queue.is_list_section());
        assert!(NavSection::Backlog.is_list_section());
        assert!(NavSection::Sessions.is_list_section());
        assert!(!NavSection::ActionDrain.is_list_section());
        assert!(!NavSection::ActionNewSession.is_list_section());
        assert!(!NavSection::ActionSwitchRole.is_list_section());
    }

    #[test]
    fn labels_are_present_for_each_section() {
        for s in NavSection::all() {
            assert!(!s.label().is_empty());
        }
    }
}
