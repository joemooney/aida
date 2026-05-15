//! The bottom 1-row status strip — always-visible chrome that never
//! interrupts the hosted Claude's screen (plan Fork 4).
//!
//! Shows the tab list (`[1·EPIC-26][2 BUG-9]`, the focused tab marked
//! with `·`), an optional notification badge, and a one-key hint. The
//! strip is the last terminal row; the hosted PTY owns rows `0..H-1`
//! (see [`crate::term::pty_rows`]).
//!
//! trace:STORY-132 | ai:claude

use crossterm::{
    cursor,
    style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{Clear, ClearType},
    QueueableCommand,
};
use std::io::{self, Write};
use unicode_width_shim as uw;

/// Minimal display-width shim — the strip is plain ASCII chrome, so a
/// 1-column-per-`char` count is exact and avoids a `unicode-width`
/// dependency. (Kept in its own module so the intent is explicit.)
mod unicode_width_shim {
    pub fn width(s: &str) -> usize {
        s.chars().count()
    }
}

/// Compose the strip text for a `width`-column row. Pure (no I/O) so it
/// is trivially testable; [`render`] adds the colour + cursor moves.
///
/// Layout: tab chips left-aligned, badge + hint right-aligned, padded
/// with spaces between. Over-long content is truncated to `width`.
pub fn strip_line(
    chips: &[String],
    focused: usize,
    badge: Option<&str>,
    hint: &str,
    width: u16,
) -> String {
    let width = width as usize;
    if width == 0 {
        return String::new();
    }

    let left = if chips.is_empty() {
        "(no sessions)".to_string()
    } else {
        chips
            .iter()
            .enumerate()
            .map(|(i, label)| {
                let marker = if i == focused { '·' } else { ' ' };
                format!("[{}{}{}]", i + 1, marker, label)
            })
            .collect::<Vec<_>>()
            .join("")
    };

    let mut right = String::new();
    if let Some(b) = badge {
        right.push_str(&format!("⚑ {}  ", b));
    }
    right.push_str(hint);

    let lw = uw::width(&left);
    let rw = uw::width(&right);

    let line = if lw + rw + 1 > width {
        // Not enough room for both — the tab list wins; truncate it.
        truncate(&left, width)
    } else {
        let gap = width - lw - rw;
        format!("{}{}{}", left, " ".repeat(gap), right)
    };
    line
}

/// Truncate `s` to at most `max` display columns.
fn truncate(s: &str, max: usize) -> String {
    if uw::width(s) <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

/// Paint the status strip at terminal row `row`. The focused-tab marker
/// is textual (`·`), so the whole strip renders with one uniform
/// background — no per-chip styling, no flicker.
pub fn render(
    out: &mut impl Write,
    row: u16,
    width: u16,
    chips: &[String],
    focused: usize,
    badge: Option<&str>,
    hint: &str,
) -> io::Result<()> {
    let line = strip_line(chips, focused, badge, hint, width);
    out.queue(cursor::MoveTo(0, row))?;
    out.queue(Clear(ClearType::CurrentLine))?;
    out.queue(SetBackgroundColor(Color::DarkGrey))?;
    out.queue(SetForegroundColor(Color::White))?;
    out.queue(Print(line))?;
    out.queue(ResetColor)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_line_marks_focused_tab() {
        let chips = vec!["EPIC-26".to_string(), "BUG-9".to_string()];
        let line = strip_line(&chips, 0, None, "^a cmd", 80);
        assert!(line.starts_with("[1·EPIC-26][2 BUG-9]"));
        // Focus on the second tab moves the `·` marker.
        let line2 = strip_line(&chips, 1, None, "^a cmd", 80);
        assert!(line2.starts_with("[1 EPIC-26][2·BUG-9]"));
    }

    #[test]
    fn strip_line_right_aligns_hint_and_badge() {
        let chips = vec!["EPIC-26".to_string()];
        let line = strip_line(&chips, 0, Some("CI green"), "^a cmd", 80);
        assert_eq!(line.chars().count(), 80);
        assert!(line.trim_end().ends_with("^a cmd"));
        assert!(line.contains("⚑ CI green"));
    }

    #[test]
    fn strip_line_handles_empty_and_narrow() {
        assert!(strip_line(&[], 0, None, "^a cmd", 80).starts_with("(no sessions)"));
        // A too-narrow strip truncates rather than panicking.
        let chips = vec!["VERY-LONG-SCOPE-NAME".to_string()];
        let line = strip_line(&chips, 0, None, "^a cmd", 10);
        assert_eq!(line.chars().count(), 10);
        assert!(strip_line(&chips, 0, None, "^a cmd", 0).is_empty());
    }
}
