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
/// Blocked/waiting board — the TUI flow-cockpit home view (STORY-686). The
/// reason taxonomy, the precedence classifier, and the cache-fast source
/// fetchers that compose it.
//
// trace:STORY-686 | ai:claude
mod board;
/// Pure fuzzy command-palette core (STORY-682): clap-surface enumeration plus a
/// dependency-free subsequence fuzzy matcher. No TUI wiring lives here — the
/// rendering/launcher integration is a separate slice.
pub mod cmd_palette;
mod config;
mod config_menu;
mod dashboard;
/// In-process intent dispatch: turns the launcher's exit Intent into a
/// spawned child the `aida tui` process runs and waits on, then re-enters —
/// so `aida tui` is self-sufficient with no fd-3 pipe and no `aida-tui`
/// shell wrapper.
// trace:STORY-681 | ai:claude
mod dispatch;
mod event;
mod help;
mod intent;
mod launcher;
mod nav;
mod overlay;
/// Deterministic AIDA action palette shown while the hosted chat is
/// suspended (EPIC-51 slice 2). A fuzzy-filtered list of curated actions that
/// each run a fixed `aida …` subprocess and render the result inline —
/// zero LLM round-trip.
//
// trace:STORY-679 | ai:claude
mod palette;
mod picker;
mod pty;
/// The action→target command-palette redesign (EPIC-54). Now the DEFAULT
/// (TASK-1051): it renders unless `AIDA_TUI_REDESIGN` is an explicit opt-OUT
/// (`0`/`false`/`no`/`off`), which selects the legacy TUI below.
// trace:STORY-690 trace:TASK-1051 | ai:claude
mod redesign;
mod state;
mod statusbar;
mod tab;
mod term;
mod theme;
mod welcome;

pub use app::{App, ExitKind};
pub use cmd_palette::{enumerate, fuzzy_score, rank, CommandEntry, Scored, COMMON_ACTIONS};
pub use config::{TuiConfig, TuiMode};
pub use config_menu::{run as run_config_menu, ConfigMenuItem, EditKind, EditOutcome};
pub use launcher::LauncherOptions;
/// Is the EPIC-54 action→target redesign selected for this `aida tui`?
/// Default-on (TASK-1051); `AIDA_TUI_REDESIGN=0`/`false`/`no`/`off` opts out to
/// the legacy TUI. The CLI launcher gate calls this so its launcher-bypass
/// decision can never drift from `aida_tui::run`'s dispatch.
// trace:TASK-1051 | ai:claude
pub use redesign::enabled as redesign_enabled;
pub use theme::{Theme, ThemeName};

/// Test-only re-export of the launcher's internal Intent + writer so
/// integration tests under `aida-tui/tests/` can exercise the wire
/// format without depending on the private module. Not part of the
/// public API — gated on `cfg(any(test, feature = "test-internals"))`
/// would be cleaner once the feature lands; for now we expose it
/// always, named with a `__` prefix to signal "internal use only".
// trace:STORY-244 | ai:claude
#[doc(hidden)]
pub mod __test_only {
    // STORY-681: the in-process dispatch planner + child runner, so
    // integration tests can assert the Intent → spawned-command mapping
    // (the Rust equivalent of the old bash wrapper's `case` dispatch)
    // without standing up a real terminal. trace:STORY-681 | ai:claude
    pub use crate::dispatch::{plan as dispatch_plan, run_child as dispatch_run_child, Dispatch};
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

    // EPIC-54: the action→target redesign is now the DEFAULT (TASK-1051). It
    // renders unless `AIDA_TUI_REDESIGN` is an explicit opt-OUT
    // (`0`/`false`/`no`/`off`), which falls through to the legacy PTY-host
    // shell below. When selected, it owns the terminal and that shell is never
    // reached. trace:STORY-690 trace:TASK-1051
    if redesign::enabled() {
        return redesign::run(config.theme.theme(), &project_root);
    }
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
        // TASK-895: a Codex tab records an empty session id (Codex's
        // interactive CLI has no caller-minted/TUI-addressable session id), so
        // there is no `--resume` hint to print for it.
        if session_id.is_empty() {
            continue;
        }
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
