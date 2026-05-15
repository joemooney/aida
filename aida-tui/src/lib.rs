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
//! exit. STORY-133 adds the `prefix o` status overlay — a read-only
//! `ratatui` view over `aida status --json` with three quick actions.
//! Multi-tab management (STORY-3) and crash recovery (STORY-5) build on
//! these primitives.
//!
//! trace:STORY-132 | ai:claude

mod actions;
mod app;
mod config;
mod event;
mod overlay;
mod picker;
mod pty;
mod statusbar;
mod tab;
mod term;

pub use app::{App, ExitKind};
pub use config::TuiConfig;

use anyhow::{bail, Result};
use std::path::Path;

/// Options for one `aida tui` invocation.
pub struct TuiOptions {
    /// Optional scope (an EPIC / STORY / … id) to host in the first tab.
    /// `None` opens an empty shell — exit it cleanly, or (STORY-3)
    /// populate it via the new-tab picker.
    pub scope: Option<String>,
    /// Skip crash-recovery re-attach on launch. STORY-5 wires the
    /// behaviour; STORY-132 only carries the flag so the surface is
    /// stable for that story.
    pub no_recover: bool,
}

/// Launch the AIDA TUI. Installs a panic hook and an RAII terminal guard
/// so neither a normal exit nor a panic ever strands the terminal in raw
/// mode.
pub fn run(opts: TuiOptions) -> Result<()> {
    let cwd = std::env::current_dir()?;
    ensure_project_context(&cwd)?;

    // STORY-5 owns crash recovery; until then `--no-recover` is a no-op
    // beyond being accepted, so the flag's presence is stable.
    let _ = opts.no_recover;

    let config = TuiConfig::load(&cwd);
    term::install_panic_hook();

    let (exit, resume_hints) = {
        let _guard = term::TermGuard::enter()?;
        let mut app = App::new(config);
        let exit = app.run(opts.scope)?;
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

/// Refuse to launch outside a git repository / AIDA project. The TUI
/// hosts `aida queue work`, which needs a project to operate on — failing
/// here gives a clear message instead of a confusing child error inside
/// a PTY the user can barely see.
fn ensure_project_context(cwd: &Path) -> Result<()> {
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        if d.join(".git").exists() || d.join(".aida").join("config.toml").is_file() {
            return Ok(());
        }
        dir = d.parent();
    }
    bail!(
        "not inside a git repository — run `aida tui` from an AIDA project \
         (`aida init` sets one up)"
    );
}
