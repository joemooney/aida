//! Launcher left-nav panel — section selector (STORY-244).
//!
//! Renders the vertical list on the left of the launcher dashboard:
//! Queue / Backlog / History / PRs / Sessions, then an action verb block
//! (Drain queue / New session / Switch role) separated by a horizontal
//! rule. Pure state + render; the parent dashboard owns the data each
//! section drives. trace:STORY-244 | ai:claude

use crate::dashboard::Pane;
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

/// One left-nav row. The blocked-board (STORY-686) reason-groups lead the
/// Nav as the cockpit home; the original perspectives (Queue / Backlog /
/// History / PRs / Sessions) follow them; the last three are action verbs —
/// selecting one emits an [`crate::intent::Intent`] directly without
/// changing the middle list. Reason and perspective sections both drive the
/// middle list. trace:STORY-686 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavSection {
    /// A blocked-board reason-group (the flow-cockpit home). Carries the
    /// [`crate::board::Reason`] it surfaces. trace:STORY-686 | ai:claude
    Reason(crate::board::Reason),
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
    /// Label rendered in the left-nav list. Reason rows carry only the bare
    /// reason label here; the dashboard appends the live `(count) · owner`
    /// suffix at render time. trace:STORY-686 | ai:claude
    pub fn label(self) -> &'static str {
        match self {
            NavSection::Reason(r) => r.label(),
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

    /// The board reason this section surfaces, if it is a reason-group.
    pub fn reason(self) -> Option<crate::board::Reason> {
        match self {
            NavSection::Reason(r) => Some(r),
            _ => None,
        }
    }

    /// True for the list sections (reasons + the five perspectives) that
    /// populate the middle list. False for the action verbs (which emit an
    /// Intent directly). trace:STORY-686 | ai:claude
    pub fn is_list_section(self) -> bool {
        matches!(
            self,
            NavSection::Reason(_)
                | NavSection::Queue
                | NavSection::Backlog
                | NavSection::History
                | NavSection::Prs
                | NavSection::Sessions
        )
    }

    /// Default ordered nav list: the seven reason-groups (the cockpit home),
    /// then the perspectives, then the action verbs. trace:STORY-686
    pub fn all() -> Vec<NavSection> {
        let mut sections: Vec<NavSection> = crate::board::Reason::all()
            .into_iter()
            .map(NavSection::Reason)
            .collect();
        sections.extend([
            NavSection::Queue,
            NavSection::Backlog,
            NavSection::History,
            NavSection::Prs,
            NavSection::Sessions,
            NavSection::ActionDrain,
            NavSection::ActionNewSession,
            NavSection::ActionSwitchRole,
        ]);
        sections
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
    pub fn select_next(&mut self) {
        if self.sections.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.sections.len();
    }

    /// Move to the previous section, wrapping.
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

/// Build the nav-row label for a section. Reason rows get the live
/// `(count) · owner` suffix from `reason_counts`; everything else uses the
/// bare label. Kept as a pure helper so the suffix formatting is testable.
/// trace:STORY-686 | ai:claude
pub fn nav_row_label(
    section: NavSection,
    reason_counts: &std::collections::HashMap<&'static str, usize>,
) -> String {
    match section.reason() {
        Some(r) => {
            let count = reason_counts.get(r.label()).copied().unwrap_or(0);
            format!("{} ({}) · {}", r.label(), count, r.owner())
        }
        None => section.label().to_string(),
    }
}

/// Draw the left-nav panel into `area`. Reason-groups lead, then a
/// "perspectives" divider, then Queue/Backlog/…/Sessions, then a second
/// divider before the action verbs. Reason rows carry a live
/// `(count) · owner` suffix; an empty reason is dimmed but never hidden, so
/// the cockpit's seven-reason shape is always legible. Colors resolve
/// through `theme`. When `focus` is [`Pane::List`] the selected section
/// dims instead of taking the full accent fill, so the focused pane is the
/// only one wearing the bright highlight.
/// trace:TASK-256 trace:STORY-685 trace:STORY-686 | ai:claude
pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &NavState,
    theme: &Theme,
    focus: Pane,
    reason_counts: &std::collections::HashMap<&'static str, usize>,
) {
    let block = Block::bordered()
        .border_style(Style::default().fg(theme.border))
        .title(" Nav ");
    let inner_w = area.width.saturating_sub(2) as usize;

    let push_rule = |lines: &mut Vec<Line>| {
        lines.push(Line::from(Span::styled(
            "─".repeat(inner_w),
            Style::default().fg(theme.dim),
        )));
    };

    let mut lines: Vec<Line> = Vec::new();
    let mut saw_perspective = false;
    let mut saw_action = false;
    for (i, section) in state.sections.iter().enumerate() {
        // Divider between the reason-groups and the first perspective.
        if section.reason().is_none() && section.is_list_section() && !saw_perspective {
            push_rule(&mut lines);
            saw_perspective = true;
        }
        // Divider between the last list section and the first action verb.
        if !section.is_list_section() && !saw_action {
            push_rule(&mut lines);
            saw_action = true;
        }
        let marker = if i == state.selected { "▸ " } else { "  " };
        // An empty reason-group is rendered dimmed (count 0) so the seven
        // reasons stay visible as a stable map without competing for the eye.
        let empty_reason = section
            .reason()
            .map(|r| reason_counts.get(r.label()).copied().unwrap_or(0) == 0)
            .unwrap_or(false);
        let style = if i == state.selected {
            if focus == Pane::Nav {
                // Nav owns focus: full accent fill, bold.
                Style::default()
                    .fg(theme.on_accent)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                // Focus moved into the list — dim the Nav selection so it's
                // clearly the inactive pane. trace:STORY-685 | ai:claude
                Style::default().fg(theme.dim).add_modifier(Modifier::DIM)
            }
        } else if empty_reason {
            // Empty reason-group: present but quiet.
            Style::default().fg(theme.dim)
        } else if section.is_list_section() {
            Style::default().fg(theme.fg)
        } else {
            // Action verbs are dimmer until selected.
            Style::default().fg(theme.dim)
        };
        let text = format!("{marker}{}", nav_row_label(*section, reason_counts));
        let clipped: String = text.chars().take(inner_w.max(4)).collect();
        lines.push(Line::from(Span::styled(clipped, style)));
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::board::Reason;

    #[test]
    fn nav_default_starts_on_the_board() {
        // STORY-686: the board (first reason-group) is the default home view.
        let nav = NavState::default();
        assert_eq!(nav.current(), NavSection::Reason(Reason::InFlight));
        assert!(nav.current().is_list_section());
        assert_eq!(nav.current().reason(), Some(Reason::InFlight));
    }

    #[test]
    fn nav_wraps_both_ways() {
        let mut nav = NavState::default();
        nav.select_prev(); // wrap to the end (Switch role)
        assert_eq!(nav.current(), NavSection::ActionSwitchRole);
        nav.select_next(); // wrap back to start (the first reason-group)
        assert_eq!(nav.current(), NavSection::Reason(Reason::InFlight));
    }

    #[test]
    fn nav_select_jumps_to_section() {
        let mut nav = NavState::default();
        nav.select(NavSection::Prs);
        assert_eq!(nav.current(), NavSection::Prs);
        nav.select(NavSection::ActionDrain);
        assert_eq!(nav.current(), NavSection::ActionDrain);
        assert!(!nav.current().is_list_section());
        nav.select(NavSection::Reason(Reason::Deferred));
        assert_eq!(nav.current(), NavSection::Reason(Reason::Deferred));
        assert!(nav.current().is_list_section());
    }

    #[test]
    fn is_list_section_classifies_correctly() {
        assert!(NavSection::Reason(Reason::NeedsApproval).is_list_section());
        assert!(NavSection::Queue.is_list_section());
        assert!(NavSection::Backlog.is_list_section());
        assert!(NavSection::Sessions.is_list_section());
        assert!(!NavSection::ActionDrain.is_list_section());
        assert!(!NavSection::ActionNewSession.is_list_section());
        assert!(!NavSection::ActionSwitchRole.is_list_section());
    }

    #[test]
    fn all_includes_every_reason_then_perspectives() {
        let all = NavSection::all();
        // The leading entries are every `Reason` (the precedence-ordered
        // spec-classification groups, PLUS the mail group — STORY-701), in
        // `Reason::all()`'s order.
        for (got, want) in all.iter().zip(Reason::all().iter()) {
            assert_eq!(*got, NavSection::Reason(*want));
        }
        assert!(all.contains(&NavSection::Reason(Reason::Mail)));
        assert!(all.contains(&NavSection::Queue));
        assert!(all.contains(&NavSection::ActionSwitchRole));
    }

    #[test]
    fn nav_row_label_appends_count_and_owner_for_reasons() {
        let mut counts = std::collections::HashMap::new();
        counts.insert(Reason::NeedsApproval.label(), 3usize);
        let label = nav_row_label(NavSection::Reason(Reason::NeedsApproval), &counts);
        assert_eq!(label, "needs approval (3) · you");
        // A reason with no count entry renders (0).
        let zero = nav_row_label(NavSection::Reason(Reason::Blocked), &counts);
        assert_eq!(zero, "blocked by dep (0) · wait");
        // Non-reason sections keep their bare label.
        assert_eq!(nav_row_label(NavSection::Queue, &counts), "Queue");
    }

    #[test]
    fn labels_are_present_for_each_section() {
        for s in NavSection::all() {
            assert!(!s.label().is_empty());
        }
    }
}
