//! `aida-tui` — the AIDA terminal-UI shell (EPIC-26).
//!
//! AIDA is CLI-only today: Claude Code is the outer shell, and acting on
//! AIDA state mid-conversation means exiting Claude, running `aida …`,
//! then cold-resuming. EPIC-26 flips that — a thin TUI becomes the outer
//! shell and hosts Claude Code sessions as PTY children. The user drops
//! out of a live session to a status overlay, takes one-keystroke
//! actions, drops back into the *same* conversation.
//!
//! STORY-132 (this crate's first slice) delivered the shell: a PTY host,
//! a bottom status strip, prefix-key routing, and a clean prefix-key
//! exit. STORY-133 added the `prefix o` status overlay; STORY-134 the
//! `prefix n` multi-tab picker; STORY-135 crash recovery via
//! `.aida/tui-state.json`. BUG-109 made the empty shell discoverable —
//! a welcome panel, a `prefix ?` keybinding cheatsheet, a rotating
//! status-strip hint — and turned the shell persistent (a hosted
//! session ending drops back to the welcome panel, not exit).
//!
//! trace:STORY-132 | ai:claude

mod actions;
mod app;
mod config;
mod dashboard;
mod event;
mod help;
mod intent;
mod launcher;
mod nav;
mod overlay;
mod picker;
mod pty;
mod state;
mod statusbar;
mod tab;
mod term;
mod theme;
mod welcome;

pub use app::{App, ExitKind};
pub use config::{TuiConfig, TuiMode};
pub use launcher::LauncherOptions;
pub use theme::{Theme, ThemeName};

/// Test-only re-export of the launcher's internal Intent + writer so
/// integration tests under `aida-tui/tests/` can exercise the wire
/// format without depending on the private module. Not part of the
/// public API — gated on `cfg(any(test, feature = "test-internals"))`
/// would be cleaner once the feature lands; for now we expose it
/// always, named with a `__` prefix to signal "internal use only".
/// trace:STORY-244 | ai:claude
#[doc(hidden)]
pub mod __test_only {
    pub use crate::intent::{write_to_fd as write_intent, Intent};
}

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

/// Options for one `aida tui` invocation.
pub struct TuiOptions {
    /// Optional scope (an EPIC / STORY / … id) to host in the first tab.
    /// `None` opens an empty shell — exit it cleanly, or populate it via
    /// the `prefix n` picker (STORY-134).
    pub scope: Option<String>,
    /// Skip crash-recovery re-attach on launch (STORY-135): start clean
    /// and discard any stale `.aida/tui-state.json`.
    pub no_recover: bool,
}

/// Launch the AIDA TUI in **launcher mode** (STORY-244). Renders a
/// full-screen navigator, exits cleanly on user action emitting one
/// intent line for the `aida-tui` bash wrapper to dispatch. Does not
/// PTY-host Claude — the wrapper does.
pub fn run_launcher(opts: LauncherOptions) -> Result<()> {
    launcher::run(opts)
}

/// Launch the AIDA TUI. Installs a panic hook and an RAII terminal guard
/// so neither a normal exit nor a panic ever strands the terminal in raw
/// mode.
pub fn run(opts: TuiOptions) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let project_root = ensure_project_context(&cwd)?;

    let config = TuiConfig::load(&cwd);
    term::install_panic_hook();
    // SIGTERM / SIGINT (Unix) and CTRL_C_EVENT / CTRL_BREAK_EVENT
    // (Windows) restore the terminal before the process dies. Without
    // this, `kill <pid>` from another terminal leaves the shell with
    // cursor hidden + raw mode on; recovery would require `reset`.
    // SIGKILL stays uncatchable. trace:BUG-110 | ai:claude
    term::install_signal_handler()?;

    let (exit, resume_hints) = {
        let _guard = term::TermGuard::enter()?;
        let mut app = App::new(config);
        let exit = app.run(project_root, opts.scope, opts.no_recover)?;
        (exit, app.exit_sessions().to_vec())
        // `_guard` drops here — cooked mode + main screen restored before
        // the notices below print.
    };

    println!("{}", exit.notice());
    for (scope, session_id) in resume_hints {
        println!(
            "  {}: aida queue work {} --resume {}",
            scope, scope, session_id
        );
    }
    Ok(())
}

/// Refuse to launch outside a git repository / AIDA project, returning
/// the resolved project root (the nearest ancestor holding `.git` or
/// `.aida/config.toml`) — crash-recovery state lives in its `.aida/`.
/// The TUI hosts `aida queue work`, which needs a project to operate on;
/// failing here gives a clear message instead of a confusing child error
/// inside a PTY the user can barely see.
fn ensure_project_context(cwd: &Path) -> Result<PathBuf> {
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        if d.join(".git").exists() || d.join(".aida").join("config.toml").is_file() {
            return Ok(d.to_path_buf());
        }
        dir = d.parent();
    }
    bail!(
        "not inside a git repository — run `aida tui` from an AIDA project \
         (`aida init` sets one up)"
    );
}
