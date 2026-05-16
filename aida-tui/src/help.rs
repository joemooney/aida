//! `prefix ?` keybinding cheatsheet — the discoverability modal (BUG-109).
//!
//! The empty-state welcome panel ([`crate::welcome`]) shows the top five
//! keys; this is the full reference. It is a ratatui modal, opened by
//! `prefix ?` (and by a bare `?` in the empty shell, where there is no
//! hosted child to pass the keystroke to). Esc / `q` / `?` close it.
//!
//! The content is static — unlike the `prefix o` status overlay there is
//! no model to fetch, so there is no background refresh.
//!
//! trace:BUG-109 | ai:claude

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

/// One row of the cheatsheet: a key chord, a short action name, and a
/// one-line description.
pub struct Binding {
    pub keys: String,
    pub action: &'static str,
    pub desc: &'static str,
}

/// A titled group of [`Binding`]s.
pub struct Group {
    pub title: &'static str,
    pub bindings: Vec<Binding>,
}

/// The full cheatsheet, grouped by purpose. `prefix` is the human label
/// for the configured prefix key — every chord begins with it.
pub fn cheatsheet(prefix: &str) -> Vec<Group> {
    // A chord = the prefix, then a single follow-up key.
    let k = |suffix: &str| format!("{prefix} {suffix}");
    vec![
        Group {
            title: "Sessions",
            bindings: vec![Binding {
                keys: k("N"),
                action: "new session",
                desc: "open the new-session picker",
            }],
        },
        Group {
            title: "Tabs",
            bindings: vec![
                Binding {
                    keys: k("]"),
                    action: "next tab",
                    desc: "focus the next hosted session",
                },
                Binding {
                    keys: k("["),
                    action: "previous tab",
                    desc: "focus the previous hosted session",
                },
                Binding {
                    keys: k("1-9"),
                    action: "jump to tab",
                    desc: "focus hosted tab N directly",
                },
            ],
        },
        Group {
            title: "Overlays",
            bindings: vec![
                Binding {
                    keys: k("O"),
                    action: "status overlay",
                    desc: "queue, branch, PR/CI + quick actions",
                },
                Binding {
                    keys: k("?"),
                    action: "keybindings",
                    desc: "this screen",
                },
            ],
        },
        Group {
            title: "Lifecycle",
            bindings: vec![
                Binding {
                    keys: k("D"),
                    action: "detach",
                    desc: "leave the TUI; conversations persist on disk",
                },
                Binding {
                    keys: k("Q"),
                    action: "quit",
                    desc: "end every hosted session and exit",
                },
                Binding {
                    keys: format!("{prefix} {prefix}"),
                    action: "literal prefix",
                    desc: "send one prefix keystroke to the focused session",
                },
            ],
        },
    ]
}

/// Draw the cheatsheet — a full-screen modal (one tab is focused at a
/// time, so there is nothing to keep visible behind it).
pub fn render(frame: &mut Frame, prefix: &str) {
    let rows = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Min(3),    // grouped reference
        Constraint::Length(1), // footer
    ])
    .split(frame.area());

    let header = Line::from(vec![
        Span::styled(
            " keybindings ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  every command starts with the prefix {prefix}"),
            dim(),
        ),
    ]);
    frame.render_widget(Paragraph::new(header), rows[0]);

    render_reference(frame, rows[1], prefix);

    let footer = Line::from(vec![
        Span::styled(" esc / q / ? ", Style::default().fg(Color::Cyan)),
        Span::styled("close — back to where you were", dim()),
    ]);
    frame.render_widget(Paragraph::new(footer), rows[2]);
}

/// Render the grouped key reference into a bordered block. The key and
/// action columns are width-aligned across every group so the chords
/// and descriptions line up.
fn render_reference(frame: &mut Frame, area: Rect, prefix: &str) {
    let groups = cheatsheet(prefix);
    let col_w = |f: &dyn Fn(&Binding) -> usize| {
        groups
            .iter()
            .flat_map(|g| g.bindings.iter())
            .map(f)
            .max()
            .unwrap_or(0)
    };
    let keys_w = col_w(&|b| b.keys.chars().count());
    let action_w = col_w(&|b| b.action.chars().count());

    let mut lines: Vec<Line> = Vec::new();
    for (gi, group) in groups.iter().enumerate() {
        if gi > 0 {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            group.title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        for b in &group.bindings {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("{:<keys_w$}", b.keys),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("{:<action_w$}", b.action),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(b.desc, dim()),
            ]));
        }
    }
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" reference ")),
        area,
    );
}

/// Dimmed style for secondary text.
fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cheatsheet_is_grouped_by_purpose() {
        let titles: Vec<_> = cheatsheet("Ctrl-A").iter().map(|g| g.title).collect();
        assert_eq!(titles, ["Sessions", "Tabs", "Overlays", "Lifecycle"]);
    }

    #[test]
    fn cheatsheet_covers_every_command() {
        let all: String = cheatsheet("Ctrl-A")
            .iter()
            .flat_map(|g| g.bindings.iter())
            .map(|b| format!("{} | {} | {}", b.keys, b.action, b.desc))
            .collect::<Vec<_>>()
            .join("\n");
        for needle in [
            "Ctrl-A N",
            "Ctrl-A ]",
            "Ctrl-A [",
            "Ctrl-A 1-9",
            "Ctrl-A O",
            "Ctrl-A ?",
            "Ctrl-A D",
            "Ctrl-A Q",
            "Ctrl-A Ctrl-A",
        ] {
            assert!(all.contains(needle), "cheatsheet missing `{needle}`");
        }
    }

    #[test]
    fn cheatsheet_honours_a_reconfigured_prefix() {
        let groups = cheatsheet("Alt-X");
        assert_eq!(groups[0].bindings[0].keys, "Alt-X N");
        let lifecycle = groups.last().expect("lifecycle group");
        let literal = lifecycle.bindings.last().expect("literal-prefix row");
        assert_eq!(literal.keys, "Alt-X Alt-X");
    }
}
