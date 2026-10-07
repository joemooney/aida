//! `aida tui` entry: the cfg(feature = "tui") / stub pair (STORY-1488 slice 1).
// trace:STORY-1488 | ai:claude

use crate::*;

/// EPIC-26: launch the AIDA TUI shell. Gated behind the `tui` feature
/// (default-on as of STORY-137) — the `aida tui` subcommand stays visible
/// in `--help` either way, but a binary built without the feature errors
/// clearly instead of half-running.
///
/// STORY-244 added launcher mode. With `--launcher` (or
/// `[tui] mode = "launcher"` in `.aida/config.toml`, the default), the
/// TUI is a dashboard that exits emitting one intent line on the
/// configured fd; without it (and with `mode = "pty-host"`), the legacy
/// STORY-132 PTY-host shell runs.
/// trace:STORY-132 STORY-137 STORY-244 | ai:claude
#[cfg(feature = "tui")]
pub(crate) fn handle_tui_command(
    scope: Option<String>,
    no_recover: bool,
    launcher: bool,
    intent_fd: Option<u32>,
) -> Result<()> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let cfg = aida_tui::TuiConfig::load(&cwd);
    // STORY-690: the action->target redesign owns the terminal directly (like
    // the PTY-host `run()` path), so it must bypass the launcher
    // intent-fd/shell-wrapper handshake. When the redesign is selected, force
    // the direct `run()` path — which then dispatches to `redesign::run()`.
    // EPIC-54 is now the DEFAULT (TASK-1051): the redesign renders unless
    // `AIDA_TUI_REDESIGN` is an explicit opt-OUT (`0`/`false`/`no`/`off`).
    // Routes through the same `aida_tui` predicate `run()` uses so the two
    // gates can't drift. trace:STORY-690 trace:TASK-1051 | ai:claude
    let redesign = aida_tui::redesign_enabled();
    let use_launcher = !redesign && (launcher || cfg.mode == aida_tui::TuiMode::Launcher);
    if use_launcher {
        // STORY-681: bare `aida tui` dispatches intents IN-PROCESS and
        // re-enters in a loop — self-sufficient, no fd-3 pipe, no `aida-tui`
        // shell wrapper. The `--intent-fd` flag stays as an opt-in
        // power-user / legacy hook: when present it switches back to the
        // STORY-244 single-shot fd-emit protocol for an external dispatcher.
        // trace:STORY-681 | ai:claude
        aida_tui::run_launcher(aida_tui::LauncherOptions { scope, intent_fd })
    } else {
        aida_tui::run(aida_tui::TuiOptions { scope, no_recover })
    }
}

/// Stub for binaries built without the `tui` feature.
/// trace:STORY-132 | ai:claude
#[cfg(not(feature = "tui"))]
pub(crate) fn handle_tui_command(
    _scope: Option<String>,
    _no_recover: bool,
    _launcher: bool,
    _intent_fd: Option<u32>,
) -> Result<()> {
    anyhow::bail!(
        "the `aida tui` shell is not compiled into this binary — rebuild \
         aida-cli with the default features (the `tui` feature ships \
         default-on as of STORY-137; a --no-default-features build omits it)"
    )
}
