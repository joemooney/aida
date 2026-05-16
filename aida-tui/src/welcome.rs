//! Empty-state welcome panel — the centered card shown when `aida tui`
//! has no hosted session (BUG-109).
//!
//! Before this, an empty shell (no scope on launch, or every hosted
//! session ended) painted nothing but the bottom status strip — a black
//! screen with no hint that `Ctrl-A` is the prefix key. First-time users
//! (the TUI is default-on since STORY-137) reflexively pressed q /
//! Ctrl-C / arrows, none of which did anything, and killed the process
//! from another terminal.
//!
//! The panel coexists with the strip: it owns rows `0..H-1`, the strip
//! owns the last row. It is plain crossterm chrome — a static base layer
//! like the strip and the blitted child — whereas the `prefix o`
//! overlay, `prefix n` picker and `prefix ?` help are ratatui modals.
//!
//! trace:BUG-109 | ai:claude

use crossterm::{
    cursor,
    style::Print,
    terminal::{Clear, ClearType},
    QueueableCommand,
};
use std::io::{self, Write};

/// The empty-state card's body lines, before the box is drawn around
/// them. `prefix` is the human label for the configured prefix key
/// (`Ctrl-A` by default) so a reconfigured prefix stays accurate.
fn body(prefix: &str) -> Vec<String> {
    vec![
        "AIDA hosts your Claude Code sessions.".to_string(),
        "Nothing is running yet.".to_string(),
        String::new(),
        format!("{prefix}  N    new session"),
        format!("{prefix}  O    status overlay"),
        format!("{prefix}  ?    all keybindings"),
        format!("{prefix}  D    detach"),
        format!("{prefix}  Q    quit"),
        String::new(),
        format!("Press {prefix} then N to pick one — or"),
        // Generic `<SCOPE>` placeholder, never a real spec id: a
        // first-user has no EPIC-26 (that's AIDA's own TUI epic), and an
        // internal id with no context is noise on a welcome screen.
        // trace:TASK-268 | ai:claude
        "relaunch as  aida tui <SCOPE>.".to_string(),
    ]
}

/// Build the welcome card: a bordered box around [`body`]. Every returned
/// line is the same display width, so [`render`] can centre the block
/// with one offset. Pure (no I/O) — unit-tested directly.
pub fn panel(prefix: &str) -> Vec<String> {
    let body = body(prefix);
    let title = " aida tui ";
    let text_w = body.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    // Inner width between the corner columns; never narrower than the
    // title bar.
    let inner = (text_w + 2).max(title.chars().count());

    let mut out = Vec::with_capacity(body.len() + 2);
    out.push(format!("╭{}╮", center_fill(title, inner)));
    for line in &body {
        out.push(format!("│ {:<width$} │", line, width = inner - 2));
    }
    out.push(format!("╰{}╯", "─".repeat(inner)));
    out
}

/// Place `s` in a field of `width` columns padded with `─`, centred.
fn center_fill(s: &str, width: usize) -> String {
    let sw = s.chars().count();
    if sw >= width {
        return s.chars().take(width).collect();
    }
    let left = (width - sw) / 2;
    let right = width - sw - left;
    format!("{}{}{}", "─".repeat(left), s, "─".repeat(right))
}

/// Paint the welcome card centred in the `width × height` region (the
/// screen above the status strip). The caller has already cleared the
/// screen; on a terminal too small for the bordered card the bare body
/// lines are printed top-left, clipped, so the keys are still legible.
pub fn render(out: &mut impl Write, width: u16, height: u16, prefix: &str) -> io::Result<()> {
    let card = panel(prefix);
    let card_h = card.len() as u16;
    let card_w = card.first().map(|l| l.chars().count()).unwrap_or(0) as u16;

    if width < card_w || height < card_h {
        // Too small for the bordered card — fall back to the bare body.
        for (i, line) in body(prefix).iter().enumerate() {
            if i as u16 >= height {
                break;
            }
            let clipped: String = line.chars().take(width as usize).collect();
            out.queue(cursor::MoveTo(0, i as u16))?;
            out.queue(Clear(ClearType::CurrentLine))?;
            out.queue(Print(clipped))?;
        }
        return Ok(());
    }

    let top = (height - card_h) / 2;
    let left = (width - card_w) / 2;
    for (i, line) in card.iter().enumerate() {
        out.queue(cursor::MoveTo(left, top + i as u16))?;
        out.queue(Print(line))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_lines_are_uniform_width() {
        let card = panel("Ctrl-A");
        let w = card[0].chars().count();
        assert!(w >= 30, "card should be wide enough to read");
        assert!(
            card.iter().all(|l| l.chars().count() == w),
            "every box row must be the same display width"
        );
    }

    #[test]
    fn panel_shows_the_keys_and_relaunch_banner() {
        let text = panel("Ctrl-A").join("\n");
        // Title bar.
        assert!(text.contains("aida tui"));
        // The five empty-state keybindings.
        assert!(text.contains("Ctrl-A  N    new session"));
        assert!(text.contains("Ctrl-A  O    status overlay"));
        assert!(text.contains("Ctrl-A  ?    all keybindings"));
        assert!(text.contains("Ctrl-A  D    detach"));
        assert!(text.contains("Ctrl-A  Q    quit"));
        // The relaunch-with-scope banner — a generic placeholder, not a
        // real (internal) spec id. trace:TASK-268
        assert!(text.contains("Press Ctrl-A then N to pick one"));
        assert!(text.contains("aida tui <SCOPE>"));
    }

    #[test]
    fn panel_honours_a_reconfigured_prefix() {
        let text = panel("Ctrl-B").join("\n");
        assert!(text.contains("Ctrl-B  N    new session"));
        assert!(text.contains("Press Ctrl-B then N"));
        assert!(!text.contains("Ctrl-A"));
    }

    #[test]
    fn render_into_a_small_terminal_falls_back_without_panicking() {
        let mut buf: Vec<u8> = Vec::new();
        // 20×4 is far too small for the card — must not panic.
        render(&mut buf, 20, 4, "Ctrl-A").expect("render succeeds");
        assert!(!buf.is_empty());
    }
}
