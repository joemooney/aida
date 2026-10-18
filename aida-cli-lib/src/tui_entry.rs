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

/// Launch the AIDA TUI — a terminal-UI shell that PTY-hosts Claude
/// Code sessions.
///
/// The TUI becomes the outer shell: it owns the terminal, reserves a
/// bottom status strip, and hosts a Claude session as a PTY child
/// (`aida queue work <scope>`). A prefix key (Ctrl-a, configurable
/// via `[tui] prefix_key`) toggles command mode; `prefix q` quits and
/// `prefix d` detaches (conversations persist on disk).
///
// trace:STORY-132 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "tui")]
pub(crate) struct TuiArgs {
    /// Scope (an EPIC / STORY / … id) to host in the first tab
    /// (PTY-host mode) or to scope the Sessions section against
    /// (launcher mode). Omit to open the launcher dashboard / an
    /// empty PTY-host shell.
    pub scope: Option<String>,
    /// Skip crash-recovery re-attach of orphaned sessions on launch
    /// and discard any stale `.aida/tui-state.json`. PTY-host-mode
    /// only — the launcher does not own PTY children.
    // trace:STORY-135 | ai:claude
    // trace:TASK-487 | ai:claude
    #[clap(long)]
    pub no_recover: bool,
    // trace:STORY-244 | ai:claude
    /// Force launcher mode: the TUI renders the board, and on a user
    /// action dispatches the chosen command in-process before
    /// re-entering. Defaults to whatever `[tui] mode` resolves to
    /// (launcher unless overridden).
    // trace:STORY-244 STORY-681 | ai:claude
    #[clap(long)]
    pub launcher: bool,
    /// Power-user / legacy hook: emit one intent line to this file
    /// descriptor and exit (the legacy single-shot fd-3 protocol an
    /// external dispatcher consumes) instead of dispatching the intent
    /// in-process. Omit it — bare `aida tui` is self-sufficient and
    /// needs no fd-3 pipe or `aida-tui` shell wrapper.
    // trace:STORY-244 STORY-681 | ai:claude
    #[clap(long, value_name = "FD", hide = true)]
    pub intent_fd: Option<u32>,
}

// `aida tui` is the EPIC-26 TUI shell — a process supervisor that
// holds no storage handle (it shells out to `aida` subcommands), so
// it dispatches before storage init like Goal / Statusline.
// trace:STORY-132 | ai:claude
fn run_tui(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    let args = <TuiArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let TuiArgs {
        scope,
        no_recover,
        launcher,
        intent_fd,
    } = &args;
    return handle_tui_command(scope.clone(), *no_recover, *launcher, *intent_fd);
}

// trace:STORY-1728 | ai:claude
pub(crate) static TUI: crate::command_registry::CommandReg = crate::command_registry::CommandReg {
    name: "tui",
    help_after: "mcp",
    command: <TuiArgs as clap::CommandFactory>::command,
    phase: crate::command_registry::Phase::PreStore,
    policy: |_| crate::command_registry::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::Write,
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
    },
    run: run_tui,
};
