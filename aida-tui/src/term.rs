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
use std::sync::atomic::{AtomicBool, Ordering};

/// Single-writer gate for terminal restoration. Set to `true` the moment
/// raw mode is on, swapped to `false` by whichever path runs the restore
/// first — `TermGuard::drop`, the panic hook, or the SIGTERM/SIGINT
/// handler. Without this, the signal handler and Drop could race and the
/// terminal would receive two restore sequences (or two `disable_raw_mode`
/// calls when one already restored cooked mode). trace:BUG-110 | ai:claude
static TERMINAL_NEEDS_RESTORE: AtomicBool = AtomicBool::new(false);

/// RAII guard for the real terminal. Construction enters raw mode + the
/// alternate screen and hides the cursor; `Drop` restores cooked mode,
/// leaves the alternate screen, and shows the cursor again.
///
/// The TUI is a process supervisor — a panic anywhere (ours or a hosted
/// child's drop) must not leave the user staring at a terminal with no
/// echo and no cursor. The guard pairs with [`install_panic_hook`] so
/// both the normal-return and the panic paths run the same teardown, and
/// with [`install_signal_handler`] so a `kill <pid>` from another
/// terminal restores too (BUG-110).
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
        // Arm the cross-thread restore gate *before* enable_raw_mode so a
        // signal landing in the microsecond-wide window between the two
        // still finds the gate set. The inverse race (signal after the
        // store, before raw mode is on) is benign: restore_terminal's
        // disable_raw_mode / LeaveAlternateScreen / cursor::Show are all
        // no-ops when their state was never entered.
        // trace:BUG-110 | ai:claude trace:TASK-429 | ai:claude
        TERMINAL_NEEDS_RESTORE.store(true, Ordering::SeqCst);
        terminal::enable_raw_mode().context("failed to enter terminal raw mode")?;
        // Own teardown now. Any `?` past this point drops `guard` on the
        // way out and runs the full restore.
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
/// panic hook or signal handler — every step swallows its error so
/// teardown always runs to completion even if one step fails.
///
/// Gated on [`TERMINAL_NEEDS_RESTORE`] so a Drop / panic-hook / signal
/// race ends in exactly one restore: the first caller swaps the flag to
/// `false` and runs the sequence; the loser sees `false` and returns
/// without touching the terminal. trace:BUG-110 | ai:claude
pub fn restore_terminal() {
    if !TERMINAL_NEEDS_RESTORE.swap(false, Ordering::SeqCst) {
        return;
    }
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

/// Install a SIGTERM / SIGINT (Unix) and CTRL_C_EVENT / CTRL_BREAK_EVENT
/// (Windows) handler that restores the terminal before the process dies.
///
/// `TermGuard::drop` covers the normal-exit and panic paths, but Drop
/// never runs when a signal terminates the process abruptly. Without
/// this, `kill <pid>` from another terminal strands the user's shell
/// with the cursor hidden and raw mode on — recovery requires `reset`
/// or `stty sane && tput cnorm`. SIGKILL stays uncatchable by design.
///
/// Idempotent: a second call (e.g. across two `aida_tui::run`
/// invocations in one process) returns Ok rather than failing on the
/// underlying single-handler restriction. trace:BUG-110 | ai:claude
pub fn install_signal_handler() -> Result<()> {
    match ctrlc::try_set_handler(|| {
        restore_terminal();
        // Use SIGINT's conventional exit status (128 + 2) — `ctrlc`
        // collapses SIGTERM / SIGINT / CTRL_BREAK into one closure, so
        // we cannot recover the originating signum to pick 143 vs 130.
        // The shell still sees a non-zero exit and a sane terminal,
        // which is the contract this fix actually owes the user.
        std::process::exit(130);
    }) {
        Ok(()) => Ok(()),
        // A second install in the same process is a no-op, not a crash —
        // matters in tests that exercise `run()` more than once.
        Err(ctrlc::Error::MultipleHandlers) => Ok(()),
        Err(e) => Err(e).context("failed to install SIGTERM/SIGINT handler"),
    }
}

/// Best-effort terminal sanitize between an in-process-dispatched child
/// exiting and the launcher re-entering (STORY-681). The bash wrapper ran
/// `tput reset` when a dispatched command exited non-zero — a crashed
/// `claude`/`aida queue work` could leave raw mode or a hidden cursor on,
/// and the next launcher entry would paint over garbage. We do the
/// equivalent here: best-effort disable raw mode and show the cursor so
/// [`TermGuard::enter`] starts from a clean slate. Every step swallows its
/// error (the child may have left the terminal in any state).
//
// trace:STORY-681 | ai:claude
pub fn sanitize_after_child() {
    let mut out = stdout();
    let _ = terminal::disable_raw_mode();
    let _ = out.execute(cursor::Show);
    let _ = out.flush();
}

/// Run `f` with the TUI SUSPENDED: leave the alternate screen + raw mode and
/// show the cursor so a spawned INTERACTIVE child (e.g. `aida questions clarify`,
/// which itself hosts an interactive `claude`) owns the real terminal, then
/// re-enter raw mode + the alternate screen when it returns. Returns whatever
/// `f` returns.
///
/// The outer [`TermGuard`]'s restore flag ([`TERMINAL_NEEDS_RESTORE`]) is left
/// untouched, so a panic inside the child still tears the terminal down exactly
/// once on unwind (the flag is armed, the guard's Drop / panic-hook runs the
/// single restore). The re-enter is best-effort — every step swallows its error
/// the same way [`restore_terminal`] does — because the child may have left the
/// terminal in any state; the caller repaints from scratch afterwards.
// trace:STORY-744 | ai:claude
pub fn suspend_for_child<T>(f: impl FnOnce() -> T) -> T {
    let mut out = stdout();
    // Hand the terminal to the child: cooked mode, main screen, visible cursor.
    let _ = out.execute(LeaveAlternateScreen);
    let _ = out.execute(cursor::Show);
    let _ = terminal::disable_raw_mode();
    let _ = out.flush();

    let result = f();

    // Reclaim it: mirror TermGuard::enter's sequence (raw mode → alt screen →
    // hide cursor). Best-effort — the caller clears + redraws next frame.
    let _ = terminal::enable_raw_mode();
    let _ = out.execute(EnterAlternateScreen);
    let _ = out.execute(cursor::Hide);
    let _ = out.flush();
    result
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

    /// `restore_terminal` is the single composition point between
    /// `TermGuard::drop`, the panic hook, and the signal handler. The
    /// gate ensures that whichever runs first wins; the others are
    /// no-ops. Without this, a SIGTERM that arrives just as the guard
    /// is dropping would `disable_raw_mode` twice (harmless on Linux,
    /// but the alt-screen-leave sequence to a terminal already on the
    /// main screen *does* repaint, which we want to avoid).
    ///
    /// All three flag transitions live in one test because cargo runs
    /// `#[test]` functions in parallel and they share the process-global
    /// atomic — splitting them risks interleaved racing. We manipulate
    /// the flag directly rather than going through `TermGuard::enter()`
    /// because that calls `enable_raw_mode`, which is unavailable in
    /// the no-TTY CI environment. trace:BUG-110 | ai:claude
    #[test]
    fn restore_gate_is_single_writer() {
        // Unarmed: a stray `restore_terminal` call (e.g. a panic hook
        // firing before the guard ever entered raw mode) is a no-op and
        // leaves the flag false.
        TERMINAL_NEEDS_RESTORE.store(false, Ordering::SeqCst);
        restore_terminal();
        assert!(!TERMINAL_NEEDS_RESTORE.load(Ordering::SeqCst));

        // Armed: first caller wins, runs the restore, clears the flag.
        TERMINAL_NEEDS_RESTORE.store(true, Ordering::SeqCst);
        assert!(TERMINAL_NEEDS_RESTORE.load(Ordering::SeqCst));
        restore_terminal();
        assert!(!TERMINAL_NEEDS_RESTORE.load(Ordering::SeqCst));

        // Loser of the race (second restorer, whichever path it is) sees
        // the cleared flag and skips the crossterm sequences entirely.
        restore_terminal();
        assert!(!TERMINAL_NEEDS_RESTORE.load(Ordering::SeqCst));
    }
}
