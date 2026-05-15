//! Terminal lifecycle: raw mode, alternate screen, and a panic-safe
//! teardown so a crash never strands the user in a broken terminal.
//!
//! trace:STORY-132 | ai:claude

use anyhow::{Context, Result};
use crossterm::{
    cursor,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use std::io::{stdout, Write};

/// RAII guard for the real terminal. Construction enters raw mode + the
/// alternate screen and hides the cursor; `Drop` restores cooked mode,
/// leaves the alternate screen, and shows the cursor again.
///
/// The TUI is a process supervisor — a panic anywhere (ours or a hosted
/// child's drop) must not leave the user staring at a terminal with no
/// echo and no cursor. The guard pairs with [`install_panic_hook`] so
/// both the normal-return and the panic paths run the same teardown.
pub struct TermGuard {
    /// `false` once teardown has run, so a double-drop is a no-op.
    active: bool,
}

impl TermGuard {
    /// Enter raw mode + alternate screen. Fails if the process is not
    /// attached to a real terminal (e.g. output redirected to a file).
    ///
    /// The guard is constructed the instant raw mode is on — *before* the
    /// alternate-screen switch — so a failure there returns `Err` through
    /// `Drop` and `restore_terminal()` still disables raw mode. Otherwise
    /// the early return would strand the terminal in raw mode with no
    /// guard to tear it down. trace:TASK-248 | ai:claude
    pub fn enter() -> Result<Self> {
        terminal::enable_raw_mode().context("failed to enter terminal raw mode")?;
        // Raw mode is on; own teardown now. Any `?` past this point drops
        // `guard` on the way out and runs the full restore.
        let guard = Self { active: true };
        let mut out = stdout();
        out.execute(EnterAlternateScreen)
            .context("failed to switch to the alternate screen")?;
        out.execute(cursor::Hide).ok();
        out.flush().ok();
        Ok(guard)
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        if self.active {
            self.active = false;
            restore_terminal();
        }
    }
}

/// Best-effort terminal restore. Safe to call more than once and from a
/// panic hook — every step swallows its error so teardown always runs to
/// completion even if one step fails.
pub fn restore_terminal() {
    let mut out = stdout();
    let _ = out.execute(LeaveAlternateScreen);
    let _ = out.execute(cursor::Show);
    let _ = terminal::disable_raw_mode();
    let _ = out.flush();
}

/// Chain a panic hook that restores the terminal before the default hook
/// prints the panic message — otherwise the backtrace scrolls past in raw
/// mode with no newlines and the shell is left uncooked.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous(info);
    }));
}

/// Rows available to a hosted PTY child: the full terminal height minus
/// the one row reserved for the always-visible status strip. Never
/// returns 0 — a degenerate 1-row terminal still gets a 1-row child.
pub fn pty_rows(term_rows: u16) -> u16 {
    term_rows.saturating_sub(1).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pty_rows_reserves_status_row() {
        // The status strip always claims exactly one row.
        assert_eq!(pty_rows(40), 39);
        assert_eq!(pty_rows(2), 1);
        // Degenerate terminals: never hand a child 0 rows.
        assert_eq!(pty_rows(1), 1);
        assert_eq!(pty_rows(0), 1);
    }
}
