//! Launcher event loop — the STORY-244 pivot.
//!
//! Replaces the PTY-host shell as the default `aida tui` behavior: the
//! launcher renders a four-region dashboard
//! ([`crate::dashboard`]), routes user keystrokes, and on a user action
//! exits cleanly while writing one [`crate::intent::Intent`] line to fd 3
//! (or whatever the caller passed via `--intent-fd`). A small bash wrapper
//! (`aida-tui`, in [`crate::SHELL_HELPERS_NOTE`]) dispatches the intent
//! and re-launches the launcher when the dispatched command exits.
//!
//! Two TUIs no longer share the terminal — the launcher owns it, exits,
//! then Claude (or whatever the intent dispatches) owns it. No PTY
//! contention, no overlap chrome.
//!
//! trace:STORY-244 | ai:claude

use crate::dashboard::{self, DashboardModel, Pane, RoleTab, RowKind};
use crate::dispatch;
use crate::intent::{self, Intent};
use crate::nav::NavSection;
use crate::state::{self, TuiState};
use crate::term;
use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::Stdout;
use std::path::PathBuf;
use std::time::Duration;

/// How long the event loop blocks waiting for a keystroke before waking to
/// drain the async PR fetch. Short enough that a landed `gh pr list` paints
/// near-instantly, long enough to keep the loop near-idle while waiting on
/// the user. trace:BUG-619 | ai:claude
const PR_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Options for one launcher run.
pub struct LauncherOptions {
    /// Scope (an EPIC / STORY / … id) the launcher was started against —
    /// the Sessions section's `--list-sessions` shell-out targets it.
    pub scope: Option<String>,
    /// Optional intent file descriptor — the **power-user / legacy** fd-3
    /// hook (STORY-244). When `Some(fd)` and that fd is a real pipe (not
    /// stdout/stderr), the launcher runs once, writes the intent line to
    /// the fd, and exits — letting an external dispatcher (the old
    /// `aida-tui` bash wrapper, or a script) handle it.
    ///
    /// When `None` (the default for bare `aida tui`), the launcher
    /// dispatches the intent **in-process** and re-enters in a loop, so
    /// `aida tui` is self-sufficient with no fd-3 pipe and no shell wrapper
    /// (STORY-681).
    //
    // trace:STORY-681 | ai:claude
    pub intent_fd: Option<u32>,
}

/// What a routed keystroke wants the loop to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LauncherAction {
    /// Just repaint — the model changed (selection / nav / role tab).
    Redraw,
    /// Re-fetch the current section, then redraw.
    Refetch,
    /// Kick the on-demand heavyweight `aida intake` fence fetch (the `i` key),
    /// then redraw. The fetch runs off the UI thread; its proposals merge into
    /// the needs-approval group when they land.
    // trace:TASK-904 | ai:claude
    FetchIntake,
    /// Emit `intent` and exit the loop.
    Emit(Intent),
    /// Show the help cheatsheet (overlay/help reuse stays as a followup
    /// — for now we surface it via the hint row at the bottom).
    Help,
    /// User opened the command palette — handled by the loop.
    EnterPalette,
    /// Move the *focused pane*'s selection one step toward the top.
    /// Nav-focused → previous section (+ refetch its rows); List-focused →
    /// previous row. trace:STORY-685 | ai:claude
    SelectPrev,
    /// Move the focused pane's selection one step toward the bottom.
    /// trace:STORY-685 | ai:claude
    SelectNext,
    /// Move keyboard focus from the Nav pane into the list pane (Enter /
    /// Left). trace:STORY-685 | ai:claude
    FocusList,
    /// Move keyboard focus from the list pane back to the Nav pane (Right /
    /// Esc). trace:STORY-685 | ai:claude
    FocusNav,
}

/// Pure routing for a single keystroke. Doesn't touch the model; the
/// caller mutates the model after dispatch so this stays unit-testable.
///
/// Two-pane focus model (STORY-685): Up/Down act on whichever pane holds
/// focus (`model.focus`) — the Nav sections when [`Pane::Nav`], the list
/// rows when [`Pane::List`]. Enter/Right move focus Nav→List (Enter on a
/// *row* while already in the list still launches it); Left/Esc move
/// focus List→Nav. Esc from the Nav pane quits, so a quit path is always
/// reachable (alongside `q` / `Q` / Ctrl-C). Tab/BackTab keep their role
/// cycle. `b`/`h`/`p`/`s` stay as additive direct section jumps.
/// trace:STORY-685 | ai:claude
pub fn route_key(key: KeyEvent, model: &DashboardModel) -> LauncherAction {
    // Ctrl-C always quits, regardless of pane.
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return LauncherAction::Emit(Intent::Quit);
    }

    // Esc: in the list pane it returns focus to Nav; in the Nav pane it
    // quits (the always-reachable quit path the operator's "Right/Esc
    // returns to Nav" model implies). trace:STORY-685 | ai:claude
    if key.code == KeyCode::Esc {
        return match model.focus {
            Pane::List => LauncherAction::FocusNav,
            Pane::Nav => LauncherAction::Emit(Intent::Quit),
        };
    }

    // Ctrl-A <key> chord — alternate path for muscle memory (the spec
    // requires both direct and chorded keys). We treat the chord as
    // identical to the direct key, so Ctrl-A Q == Q == quit, etc.
    let prefix_armed = key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('a' | 'A'));
    if prefix_armed {
        // The next keystroke handling lives in the loop's palette/state
        // machine; from the pure router we just signal a redraw and let
        // the loop's "chord armed" flag pick up the follow-up key.
        return LauncherAction::Redraw;
    }

    match key.code {
        KeyCode::Char('q') => LauncherAction::Emit(Intent::Quit),
        KeyCode::Char('b') => switch_section(NavSection::Backlog),
        KeyCode::Char('h') => switch_section(NavSection::History),
        KeyCode::Char('p') => switch_section(NavSection::Prs),
        KeyCode::Char('s') => switch_section(NavSection::Sessions),
        KeyCode::Char('r') => LauncherAction::Redraw, // role cycle handled in loop
        KeyCode::Char('g') => LauncherAction::Refetch,
        KeyCode::Char('i') => LauncherAction::FetchIntake,
        KeyCode::Char('?') => LauncherAction::Help,
        KeyCode::Char(':') => LauncherAction::EnterPalette,
        KeyCode::Tab => LauncherAction::Redraw, // role cycle handled in loop
        KeyCode::BackTab => LauncherAction::Redraw,
        // Up/Down (and k/j) move the focused pane's selection.
        KeyCode::Up | KeyCode::Char('k') => LauncherAction::SelectPrev,
        KeyCode::Down | KeyCode::Char('j') => LauncherAction::SelectNext,
        // Right enters the list from Nav; inside the list it's a no-op.
        KeyCode::Right => match model.focus {
            Pane::Nav => LauncherAction::FocusList,
            Pane::List => LauncherAction::Redraw,
        },
        // Left returns focus to the Nav pane from the list; no-op in Nav.
        KeyCode::Left => match model.focus {
            Pane::List => LauncherAction::FocusNav,
            Pane::Nav => LauncherAction::Redraw,
        },
        KeyCode::Enter | KeyCode::Char(' ') => route_enter(model),
        // `q` direct-key is also bound, but we also accept the
        // configured nav direct-key 'Q' uppercase.
        KeyCode::Char('Q') => LauncherAction::Emit(Intent::Quit),
        KeyCode::Char(c) => {
            // Other characters: keep the surface tiny — direct keys are
            // q/b/h/p/s/r/g/i/?/:/Q.
            let _ = c;
            LauncherAction::Redraw
        }
        _ => LauncherAction::Redraw,
    }
}

/// Enter / Space routing, focus-aware. In the Nav pane, Enter drops focus
/// into the list (so the operator can then arrow over rows) — unless the
/// selected nav row is an *action verb* (Drain / New session / Switch
/// role), which has no list and fires its intent directly. In the list
/// pane, Enter launches the highlighted row. trace:STORY-685 | ai:claude
fn route_enter(model: &DashboardModel) -> LauncherAction {
    match model.focus {
        Pane::Nav => match model.nav.current() {
            NavSection::ActionDrain => {
                LauncherAction::Emit(Intent::Launch("aida queue work --auto-complete".into()))
            }
            NavSection::ActionNewSession => match model.role {
                RoleTab::Implementer => LauncherAction::Emit(Intent::Launch(
                    "aida queue work --role implementer".into(),
                )),
                RoleTab::Reviewer => {
                    LauncherAction::Emit(Intent::Launch("aida queue work --role reviewer".into()))
                }
                RoleTab::Dialog => {
                    LauncherAction::Emit(Intent::Launch("aida queue work --role dialog".into()))
                }
            },
            NavSection::ActionSwitchRole => LauncherAction::Redraw,
            // A list section: Enter drops focus into the list pane.
            _ => LauncherAction::FocusList,
        },
        Pane::List => match model.current_row() {
            Some(row) => LauncherAction::Emit(act_on_row(row, model.role, model.nav.current())),
            None => LauncherAction::Redraw,
        },
    }
}

fn switch_section(section: NavSection) -> LauncherAction {
    // The router returns Redraw + the loop applies the section change;
    // we encode the section-target through a small side helper — but to
    // keep the pure router pure, we route it as a Refetch and let the
    // loop pick up which section to set from `route_key_section`.
    let _ = section;
    LauncherAction::Refetch
}

/// Companion to [`route_key`] for keys that target a specific nav
/// section. Returns `Some(section)` when the keystroke is a direct-key
/// nav switch, `None` otherwise. Kept separate so [`route_key`] stays
/// classifier-only.
pub fn key_to_section(key: KeyEvent) -> Option<NavSection> {
    if key.modifiers.intersects(KeyModifiers::ALT) {
        return None;
    }
    match key.code {
        KeyCode::Char('Q' | 'q') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            // q (lowercase) is also queue — but we only return Queue
            // for an unambiguous `Q` press to avoid stomping the
            // primary `q = quit` direct key. The dashboard's hint row
            // surfaces `q queue · b backlog · …`; for now we treat the
            // queue-nav key as the explicit `Q` (uppercase) or via the
            // command palette.
            None
        }
        KeyCode::Char('b') => Some(NavSection::Backlog),
        KeyCode::Char('h') => Some(NavSection::History),
        KeyCode::Char('p') => Some(NavSection::Prs),
        KeyCode::Char('s') => Some(NavSection::Sessions),
        _ => None,
    }
}

/// Translate a row + role + active nav section into the Intent to emit.
///
/// The blocked-board reason rows (STORY-686) each dispatch the *lightest
/// sensible* unblock action for their reason: approve a draft, open the
/// questions flow, triage a finding, view a PR, undefer, or just show the
/// spec (in-flight / blocked are informational — Enter shows the spec /
/// jumps to it). Synthetic awaiting-review PR rows carry a `pr:<n>` id and
/// open the PR. trace:STORY-686 | ai:claude
pub fn act_on_row(row: &dashboard::ListRow, role: RoleTab, section: NavSection) -> Intent {
    match row.kind {
        RowKind::Queued | RowKind::Backlog => Intent::Launch(format!("aida queue work {}", row.id)),
        RowKind::History => Intent::Launch(format!("aida show {}", row.id)),
        RowKind::Pr => Intent::Shell(format!("gh pr view {}", row.id)),
        RowKind::Session => Intent::Resume(row.id.clone()),
        RowKind::Action => match section {
            NavSection::ActionDrain => Intent::Launch("aida queue work --auto-complete".into()),
            NavSection::ActionNewSession => {
                Intent::Launch(format!("aida queue work --role {}", role.as_str()))
            }
            _ => Intent::Quit,
        },
        // --- Blocked-board reason dispatch. trace:STORY-686 | ai:claude
        // Needs approval → approve the draft (reject/clarify stay as the
        // existing commands the operator runs explicitly).
        RowKind::ReasonNeedsApproval => {
            Intent::Launch(format!("aida edit {} --status approved", row.id))
        }
        // Advisor backlog → the spec is already Approved; the unblock is to
        // route it to the work queue, not approve it again. trace:TASK-901
        RowKind::ReasonAdvisorBacklog => Intent::Launch(format!("aida queue add {}", row.id)),
        // Live intake proposal → show the candidate so the operator can read it
        // and decide; the actual disposition is the whole-pass `aida intake`
        // (no per-id scope flag exists), kicked from the board with `i` rather
        // than firing a multi-minute cold-boot advisor on a single Enter.
        // trace:TASK-904 | ai:claude
        RowKind::ReasonIntakeProposal => Intent::Launch(format!("aida show {}", row.id)),
        // Needs an answer → open the decision-inbox flow for this spec.
        RowKind::ReasonNeedsAnswer => Intent::Launch(format!("aida questions answer {}", row.id)),
        // Needs attention → show the parked spec (its punt reason / decision
        // request lives on the spec body, which `aida show` surfaces); the
        // operator triages from there (`aida findings list`, `aida questions`,
        // or resume). trace:STORY-686 | ai:claude
        RowKind::ReasonNeedsAttention => Intent::Launch(format!("aida show {}", row.id)),
        // Awaiting review → open the PR (synthetic `pr:<n>` rows) or show
        // the Done-on-branch spec.
        RowKind::ReasonAwaitingReview => match row.id.strip_prefix("pr:") {
            Some(num) => Intent::Shell(format!("gh pr view {num}")),
            None => Intent::Launch(format!("aida show {}", row.id)),
        },
        // Deferred → restore it to the active view.
        RowKind::ReasonDeferred => Intent::Launch(format!("aida undefer {}", row.id)),
        // In-flight / blocked are informational — Enter shows the spec
        // (in-flight: lease holder + body; blocked: the blocked spec, from
        // where the operator reads its BlockedBy chain).
        RowKind::ReasonInFlight | RowKind::ReasonBlocked => {
            Intent::Launch(format!("aida show {}", row.id))
        }
        // Mail → open the reader. A row's `id` is a message id (`aida show`
        // doesn't apply), and composing a reply/send needs free-text body
        // input the launcher's `Intent::Launch` payload gate can't carry
        // safely (`is_safe_payload` excludes quotes/punctuation) — that
        // compose gesture is registered separately as
        // [`crate::board::send_mail_argv`] for a shell with a text-input
        // surface to dispatch. trace:STORY-701 | ai:claude
        RowKind::ReasonMail => Intent::Launch("aida mailbox inbox".into()),
    }
}

/// Run the launcher. Sets up the panic hook + signal handler, then drives
/// the board.
///
/// Two dispatch substrates (STORY-681):
///
/// - **In-process loop (default).** When `opts.intent_fd` is `None`, the
///   launcher paints the board, and on a user action it drops the terminal
///   guard, dispatches the intent **in this process** (spawns the command
///   as a child that inherits the real terminal, waits for it), sanitizes
///   the terminal, and re-enters — looping until the user quits. No fd 3,
///   no `aida-tui` shell wrapper, no `aida dev shell-init` prerequisite:
///   `aida tui` is self-sufficient from any shell. This fixes the BUG-612
///   standalone-launch failure properly rather than papering over it with
///   the shell function.
///
/// - **fd-3 emit (legacy / power-user hook).** When `opts.intent_fd` is
///   `Some(fd)` (passed via `--intent-fd`), the launcher runs once, writes
///   one intent line to that fd, and exits — preserving the STORY-244 wire
///   protocol for the old `aida-tui` bash wrapper and any external
///   dispatcher script.
//
// trace:STORY-244 STORY-681 | ai:claude
pub fn run(opts: LauncherOptions) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let project_root = ensure_project_context(&cwd)?;

    term::install_panic_hook();
    term::install_signal_handler()?;

    // Resolve the configured theme up front so the dashboard paints in
    // the user's palette from the first frame. trace:TASK-256 | ai:claude
    let theme = crate::config::TuiConfig::load(&cwd).theme.theme();

    match opts.intent_fd {
        // Legacy / power-user fd-3 path: emit one intent and exit.
        // trace:STORY-244 | ai:claude
        Some(fd) => run_emit_to_fd(&project_root, opts.scope.as_deref(), theme, fd),
        // Default self-sufficient in-process loop. trace:STORY-681 | ai:claude
        None => run_in_process_loop(&project_root, opts.scope.as_deref(), theme),
    }
}

/// Render the board once, dispatch the resulting intent in-process, and
/// re-enter — looping until the user quits. Each iteration: enter the
/// terminal guard, run the event loop, drop the guard (restoring cooked
/// mode), then act on the intent (spawn + wait the child), sanitize the
/// terminal, and loop.
//
// trace:STORY-681 | ai:claude
fn run_in_process_loop(
    project_root: &std::path::Path,
    scope: Option<&str>,
    theme: crate::theme::Theme,
) -> Result<()> {
    loop {
        // Re-read state each iteration so a `resume:` intent from the prior
        // pass is remembered for this one's dialog-session discovery.
        let prior_state = state::load(project_root).unwrap_or_default();
        let dialog_id = prior_state.dialog_session_id.clone();

        let intent = run_board_once(scope, dialog_id.as_deref(), theme)?;

        // Outside the guard now — cooked mode + main screen restored, so
        // the dispatched child paints the real terminal cleanly.
        let new_state = TuiState {
            tabs: prior_state.tabs,
            dialog_session_id: maybe_update_dialog(&intent, dialog_id),
        };
        state::save(project_root, &new_state);

        match dispatch::plan(&intent)? {
            dispatch::Dispatch::Quit => return Ok(()),
            dispatch::Dispatch::Run { program, args } => {
                // Defense in depth: the in-process path bypasses the
                // serialize() wire-format gate, so re-check the payload
                // through the same allow-list before spawning. A malformed
                // Intent (none of the board's Intents are) surfaces as a
                // notice and re-enters rather than reaching Command::new.
                // trace:STORY-681 | ai:claude
                if !payload_is_dispatch_safe(&program, &args) {
                    eprintln!(
                        "aida tui: refusing to dispatch an intent with unsafe characters: \
                         {program} {}",
                        args.join(" ")
                    );
                    continue;
                }
                let status = dispatch::run_child(&program, &args);
                // Sanitize the terminal between the child exiting and the
                // next launcher entry — a crashed child can leave raw mode
                // or a hidden cursor on. The bash wrapper ran `tput reset`
                // on a non-zero exit; we always sanitize (cheap, idempotent).
                term::sanitize_after_child();
                if let Err(e) = status {
                    // A missing program (e.g. `claude` not on PATH) is a
                    // user-facing condition, not a loop-ending crash: report
                    // it and re-enter so the board stays usable.
                    eprintln!("aida tui: {e:#}");
                }
            }
        }
    }
}

/// Legacy fd-3 single-shot path: render the board once and write the
/// resulting intent line to `fd` for an external dispatcher to handle.
/// Preserves the STORY-244 wire protocol + the bare-invocation safety
/// check. trace:STORY-244 | ai:claude
fn run_emit_to_fd(
    project_root: &std::path::Path,
    scope: Option<&str>,
    theme: crate::theme::Theme,
    fd: u32,
) -> Result<()> {
    // Safety check: the caller asked for fd-emit but didn't wire a real
    // pipe (e.g. `aida tui --launcher --intent-fd 3` with no `3>` redirect)
    // — refuse rather than spray the intent line into the restored
    // terminal. trace:STORY-244 risk #1 | ai:claude
    if !intent::fd_is_writable_pipe(fd) {
        anyhow::bail!(
            "the launcher's intent fd {fd} is the same kernel object as stdout/stderr.\n\
             Pass `--intent-fd` pointing at a real pipe, or drop it to use the default \
             self-sufficient in-process dispatch (no wrapper needed)."
        );
    }

    let prior_state = state::load(project_root).unwrap_or_default();
    let dialog_id = prior_state.dialog_session_id.clone();

    let intent_to_emit = run_board_once(scope, dialog_id.as_deref(), theme)?;

    // Outside the guard now — cooked mode restored.
    intent::write_to_fd(&intent_to_emit, fd)?;

    let new_state = TuiState {
        tabs: prior_state.tabs,
        dialog_session_id: maybe_update_dialog(&intent_to_emit, dialog_id),
    };
    state::save(project_root, &new_state);

    Ok(())
}

/// Enter the terminal guard, fetch the board, drive the event loop to an
/// Emit, drop the guard, and return the intent. The single render pass
/// shared by both the in-process loop and the fd-3 path.
//
// trace:STORY-681 | ai:claude
fn run_board_once(
    scope: Option<&str>,
    dialog_id: Option<&str>,
    theme: crate::theme::Theme,
) -> Result<Intent> {
    let _guard = term::TermGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    // STORY-686: the blocked/waiting board is the default home view — land
    // on the first reason-group rather than the Queue perspective.
    let mut model = dashboard::fetch(
        RoleTab::default(),
        NavSection::Reason(crate::board::Reason::all()[0]),
        scope,
        dialog_id,
    );
    model.theme = theme;
    event_loop(&mut terminal, model, scope, dialog_id)
    // `_guard` drops here — cooked mode + main screen restored.
}

/// Defense-in-depth re-check before [`dispatch::run_child`]: every token of
/// a to-be-spawned command must pass the same allow-list the wire format
/// enforces. Belt-and-braces for the in-process path, which no longer
/// routes through [`intent::serialize`].
//
// trace:STORY-681 | ai:claude
fn payload_is_dispatch_safe(program: &str, args: &[String]) -> bool {
    intent::is_safe_payload(program) && args.iter().all(|a| intent::is_safe_payload(a))
}

/// On a `Resume(<id>)` intent for the dialog role, remember the id. On a
/// `Launch(... --role dialog ...)` we'd record the freshly-minted id —
/// but the launcher doesn't mint Claude session ids (Claude does), so
/// dialog ids land on the next entry via `--list-sessions` discovery.
fn maybe_update_dialog(intent: &Intent, prior: Option<String>) -> Option<String> {
    match intent {
        Intent::Resume(id) => Some(id.clone()),
        _ => prior,
    }
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    mut model: DashboardModel,
    launch_scope: Option<&str>,
    dialog_id: Option<&str>,
) -> Result<Intent> {
    dashboard::ensure_preview(&mut model);
    terminal.clear()?;
    paint(terminal, &model)?;

    loop {
        // Wake periodically (rather than blocking on `read()`) so a landed
        // async `gh pr list` paints without a keystroke. The poll is cheap and
        // the cursor stays responsive: `poll_prs` is a non-blocking channel
        // drain. trace:BUG-619 | ai:claude
        if !event::poll(PR_POLL_INTERVAL)? {
            // A landed `gh pr list` OR a landed `aida intake` fence both repaint
            // without a keystroke; both polls are non-blocking channel drains.
            // trace:BUG-619 trace:TASK-904 | ai:claude
            let mut landed = model.poll_prs();
            landed |= model.poll_intake();
            if landed {
                dashboard::ensure_preview(&mut model);
                paint(terminal, &model)?;
            }
            continue;
        }
        // A real event is queued — drain any landed async results first so this
        // frame already reflects them, then handle the event.
        // trace:BUG-619 trace:TASK-904 | ai:claude
        let mut landed = model.poll_prs();
        landed |= model.poll_intake();
        if landed {
            dashboard::ensure_preview(&mut model);
        }
        let Event::Key(key) = event::read()? else {
            paint(terminal, &model)?;
            continue;
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            continue;
        }
        // Direct-key nav switches first — they need access to the
        // model to mutate `nav` + refetch.
        if let Some(section) = key_to_section(key) {
            model.nav.select(section);
            dashboard::refetch_rows(&mut model, launch_scope, dialog_id);
            dashboard::ensure_preview(&mut model);
            paint(terminal, &model)?;
            continue;
        }
        // `r` / Tab cycle the role tab.
        if key.code == KeyCode::Char('r')
            || (key.code == KeyCode::Tab && !key.modifiers.contains(KeyModifiers::SHIFT))
        {
            model.role = model.role.cycle_next();
            model.ambient.role = model.role.as_str().to_string();
            paint(terminal, &model)?;
            continue;
        }
        if key.code == KeyCode::BackTab {
            model.role = model.role.cycle_prev();
            model.ambient.role = model.role.as_str().to_string();
            paint(terminal, &model)?;
            continue;
        }
        // Now the pure router. Up/Down, focus moves, Enter, etc. all flow
        // through it so the focus state machine stays unit-testable.
        // trace:STORY-685 | ai:claude
        match route_key(key, &model) {
            LauncherAction::Redraw => paint(terminal, &model)?,
            LauncherAction::SelectPrev => {
                select_prev_focused(&mut model, launch_scope, dialog_id);
                dashboard::ensure_preview(&mut model);
                paint(terminal, &model)?;
            }
            LauncherAction::SelectNext => {
                select_next_focused(&mut model, launch_scope, dialog_id);
                dashboard::ensure_preview(&mut model);
                paint(terminal, &model)?;
            }
            LauncherAction::FocusList => {
                model.focus = Pane::List;
                dashboard::ensure_preview(&mut model);
                paint(terminal, &model)?;
            }
            LauncherAction::FocusNav => {
                model.focus = Pane::Nav;
                paint(terminal, &model)?;
            }
            LauncherAction::Refetch => {
                // `g` is an explicit refresh: invalidate the cached board so
                // the reason-groups recompose from fresh cache-fast reads, and
                // force a fresh off-thread `gh pr list` so the PRs panel /
                // awaiting-review group re-fill without blocking the cursor.
                // trace:STORY-686 trace:BUG-619 | ai:claude
                model.board_loaded = false;
                model.mail_loaded = false;
                model.invalidate_prs();
                dashboard::refetch_rows(&mut model, launch_scope, dialog_id);
                dashboard::ensure_preview(&mut model);
                paint(terminal, &model)?;
            }
            LauncherAction::FetchIntake => {
                // `i` arms the on-demand heavyweight `aida intake` fence off the
                // UI thread; its proposals merge into the needs-approval group
                // when the worker lands (drained by `poll_intake`). The cursor
                // never waits on the cold-boot pass. trace:TASK-904 | ai:claude
                model.request_intake();
                paint(terminal, &model)?;
            }
            LauncherAction::Emit(intent) => return Ok(intent),
            LauncherAction::Help => {
                // Help overlay is followups; for now flip the hint row.
                // trace:STORY-685 | ai:claude
                model.notice = Some("Help: ↑↓ nav sections · enter/← into list · ↑↓ rows · enter run · →/esc back to nav · tab role · g refresh · i intake · q quit".into());
                paint(terminal, &model)?;
            }
            LauncherAction::EnterPalette => {
                if let Some(intent) = run_palette(terminal, &mut model)? {
                    return Ok(intent);
                }
                paint(terminal, &model)?;
            }
        }
    }
}

/// Move the focused pane's selection one step up. Nav-focused: select the
/// previous section and refetch its rows so the list pane tracks it.
/// List-focused: move the row cursor. trace:STORY-685 | ai:claude
fn select_prev_focused(
    model: &mut DashboardModel,
    launch_scope: Option<&str>,
    dialog_id: Option<&str>,
) {
    match model.focus {
        Pane::Nav => {
            model.nav.select_prev();
            dashboard::refetch_rows(model, launch_scope, dialog_id);
        }
        Pane::List => model.select_prev(),
    }
}

/// Move the focused pane's selection one step down (companion to
/// [`select_prev_focused`]). trace:STORY-685 | ai:claude
fn select_next_focused(
    model: &mut DashboardModel,
    launch_scope: Option<&str>,
    dialog_id: Option<&str>,
) {
    match model.focus {
        Pane::Nav => {
            model.nav.select_next();
            dashboard::refetch_rows(model, launch_scope, dialog_id);
        }
        Pane::List => model.select_next(),
    }
}

fn paint(terminal: &mut Terminal<CrosstermBackend<Stdout>>, model: &DashboardModel) -> Result<()> {
    terminal.draw(|frame| dashboard::render(frame, model))?;
    Ok(())
}

/// A tiny `:`-prompt — collects keystrokes until Enter / Esc and tries
/// to dispatch the typed command to an Intent. Recognises `q`, `quit`,
/// `role implementer|reviewer|dialog`, `resume <id>`. Anything else
/// becomes a notice line.
fn run_palette(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    model: &mut DashboardModel,
) -> Result<Option<Intent>> {
    let mut buf = String::new();
    loop {
        model.notice = Some(format!(":{buf}"));
        paint(terminal, model)?;
        let Event::Key(key) = crossterm::event::read()? else {
            continue;
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            continue;
        }
        match key.code {
            KeyCode::Esc => {
                model.notice = None;
                return Ok(None);
            }
            KeyCode::Backspace => {
                buf.pop();
            }
            KeyCode::Enter => {
                model.notice = None;
                return Ok(dispatch_palette(buf.trim()));
            }
            KeyCode::Char(c) => buf.push(c),
            _ => {}
        }
    }
}

/// Parse a palette command. Returns `Some(Intent)` if the command
/// emits an intent (typically `q`/`quit`/`resume`), `None` for unknown.
pub fn dispatch_palette(cmd: &str) -> Option<Intent> {
    match cmd {
        "q" | "quit" => return Some(Intent::Quit),
        _ => {}
    }
    if let Some(rest) = cmd.strip_prefix("resume ") {
        let id = rest.trim();
        if !id.is_empty() {
            return Some(Intent::Resume(id.to_string()));
        }
    }
    if let Some(rest) = cmd.strip_prefix("launch ") {
        let cmd = rest.trim();
        if !cmd.is_empty() {
            return Some(Intent::Launch(cmd.to_string()));
        }
    }
    None
}

/// Walk up from `start` for the project root (`.git` or
/// `.aida/config.toml`). Mirrors [`crate::ensure_project_context`] but
/// re-declared so the launcher module is self-contained.
fn ensure_project_context(cwd: &std::path::Path) -> Result<PathBuf> {
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        if d.join(".git").exists() || d.join(".aida").join("config.toml").is_file() {
            return Ok(d.to_path_buf());
        }
        dir = d.parent();
    }
    Err(anyhow::anyhow!(
        "not inside a git repository — run `aida tui` from an AIDA project \
         (`aida init` sets one up)"
    ))
    .context("launcher requires project context")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboard::{ListRow, RowKind};

    fn plain(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn code(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn fixture(rows: Vec<ListRow>) -> DashboardModel {
        DashboardModel {
            rows,
            ..DashboardModel::default()
        }
    }

    /// A model with focus forced into a given pane — the focus state
    /// machine routes Up/Down/Enter/Esc differently per pane.
    fn fixture_focus(rows: Vec<ListRow>, focus: Pane) -> DashboardModel {
        DashboardModel {
            rows,
            focus,
            ..DashboardModel::default()
        }
    }

    fn queued_row(id: &str) -> ListRow {
        ListRow {
            id: id.into(),
            title: "row".into(),
            status: "Approved".into(),
            kind: RowKind::Queued,
        }
    }

    #[test]
    fn route_key_q_emits_quit() {
        let model = fixture(vec![]);
        assert_eq!(
            route_key(plain('q'), &model),
            LauncherAction::Emit(Intent::Quit)
        );
    }

    #[test]
    fn route_key_esc_from_nav_emits_quit() {
        // Esc in the Nav pane (the default focus) is the quit path.
        let model = fixture_focus(vec![], Pane::Nav);
        assert_eq!(
            route_key(code(KeyCode::Esc), &model),
            LauncherAction::Emit(Intent::Quit)
        );
    }

    #[test]
    fn route_key_esc_from_list_returns_to_nav() {
        // Esc in the List pane returns focus to Nav (does NOT quit).
        let model = fixture_focus(vec![queued_row("STORY-1")], Pane::List);
        assert_eq!(
            route_key(code(KeyCode::Esc), &model),
            LauncherAction::FocusNav
        );
    }

    #[test]
    fn route_key_ctrl_c_emits_quit() {
        let model = fixture(vec![]);
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(route_key(key, &model), LauncherAction::Emit(Intent::Quit));
    }

    #[test]
    fn route_key_b_h_p_s_refetch() {
        let model = fixture(vec![]);
        for c in ['b', 'h', 'p', 's'] {
            assert_eq!(route_key(plain(c), &model), LauncherAction::Refetch);
        }
    }

    #[test]
    fn route_key_g_refetches() {
        let model = fixture(vec![]);
        assert_eq!(route_key(plain('g'), &model), LauncherAction::Refetch);
    }

    #[test]
    fn route_key_i_fetches_intake() {
        // `i` arms the on-demand heavyweight intake fence. trace:TASK-904
        let model = fixture(vec![]);
        assert_eq!(route_key(plain('i'), &model), LauncherAction::FetchIntake);
    }

    #[test]
    fn intake_proposal_row_dispatches_to_show() {
        // A live intake-proposal row's Enter shows the candidate (the actual
        // disposition is the whole-pass `aida intake`, kicked with `i`).
        // trace:TASK-904
        let row = ListRow {
            id: "STORY-42".into(),
            title: "row".into(),
            status: "intake · Draft".into(),
            kind: RowKind::ReasonIntakeProposal,
        };
        assert_eq!(
            act_on_row(
                &row,
                RoleTab::Implementer,
                NavSection::Reason(crate::board::Reason::NeedsApproval)
            ),
            Intent::Launch("aida show STORY-42".into())
        );
    }

    #[test]
    fn route_key_colon_opens_palette() {
        let model = fixture(vec![]);
        assert_eq!(route_key(plain(':'), &model), LauncherAction::EnterPalette);
    }

    #[test]
    fn route_key_help_opens_help() {
        let model = fixture(vec![]);
        assert_eq!(route_key(plain('?'), &model), LauncherAction::Help);
    }

    // --- STORY-685 two-pane focus state machine ---------------------------

    #[test]
    fn focus_defaults_to_nav() {
        assert_eq!(DashboardModel::default().focus, Pane::Nav);
        assert_eq!(Pane::default(), Pane::Nav);
    }

    #[test]
    fn up_down_select_in_either_pane() {
        // Up/Down route to SelectPrev/SelectNext regardless of pane — the
        // loop applies them to whichever pane is focused.
        for focus in [Pane::Nav, Pane::List] {
            let model = fixture_focus(vec![queued_row("STORY-1")], focus);
            assert_eq!(
                route_key(code(KeyCode::Up), &model),
                LauncherAction::SelectPrev,
                "Up in {focus:?}"
            );
            assert_eq!(
                route_key(code(KeyCode::Down), &model),
                LauncherAction::SelectNext,
                "Down in {focus:?}"
            );
            // k/j mirror Up/Down.
            assert_eq!(route_key(plain('k'), &model), LauncherAction::SelectPrev);
            assert_eq!(route_key(plain('j'), &model), LauncherAction::SelectNext);
        }
    }

    #[test]
    fn enter_from_nav_on_list_section_focuses_list() {
        // Default nav section is Queue (a list section), so Enter drops
        // focus into the list rather than launching anything.
        let model = fixture_focus(vec![queued_row("STORY-1")], Pane::Nav);
        assert_eq!(
            route_key(code(KeyCode::Enter), &model),
            LauncherAction::FocusList
        );
        assert_eq!(route_key(plain(' '), &model), LauncherAction::FocusList);
    }

    #[test]
    fn right_from_nav_focuses_list() {
        // BUG-617: Right enters the list from Nav (was Left).
        let model = fixture_focus(vec![queued_row("STORY-1")], Pane::Nav);
        assert_eq!(
            route_key(code(KeyCode::Right), &model),
            LauncherAction::FocusList
        );
    }

    #[test]
    fn enter_from_list_launches_the_row() {
        // Once focus is in the list, Enter on a row emits its intent.
        let model = fixture_focus(vec![queued_row("STORY-1")], Pane::List);
        assert_eq!(
            route_key(code(KeyCode::Enter), &model),
            LauncherAction::Emit(Intent::Launch("aida queue work STORY-1".into()))
        );
    }

    #[test]
    fn left_returns_focus_to_nav_from_list() {
        // BUG-617: Left returns focus to Nav from the list (was Right).
        let model = fixture_focus(vec![queued_row("STORY-1")], Pane::List);
        assert_eq!(
            route_key(code(KeyCode::Left), &model),
            LauncherAction::FocusNav
        );
    }

    #[test]
    fn left_from_nav_is_noop_redraw() {
        // BUG-617: Left in the Nav pane is inert (Right enters the list).
        let model = fixture_focus(vec![], Pane::Nav);
        assert_eq!(
            route_key(code(KeyCode::Left), &model),
            LauncherAction::Redraw
        );
    }

    #[test]
    fn right_in_list_is_noop_redraw() {
        // BUG-617: Right only enters the list from Nav; inside the list it's inert.
        let model = fixture_focus(vec![queued_row("STORY-1")], Pane::List);
        assert_eq!(
            route_key(code(KeyCode::Right), &model),
            LauncherAction::Redraw
        );
    }

    #[test]
    fn tab_and_backtab_unchanged_under_focus() {
        // Tab/BackTab stay role-cycle redraws in the pure router in both
        // panes (the loop applies the cycle). They are NOT repurposed.
        for focus in [Pane::Nav, Pane::List] {
            let model = fixture_focus(vec![queued_row("STORY-1")], focus);
            assert_eq!(
                route_key(code(KeyCode::Tab), &model),
                LauncherAction::Redraw
            );
            assert_eq!(
                route_key(code(KeyCode::BackTab), &model),
                LauncherAction::Redraw
            );
        }
    }

    #[test]
    fn enter_from_nav_on_action_drain_emits_intent() {
        // Action verbs have no list — Enter fires their intent directly
        // even though focus is in Nav.
        let mut model = fixture_focus(vec![], Pane::Nav);
        model.nav.select(NavSection::ActionDrain);
        assert_eq!(
            route_key(code(KeyCode::Enter), &model),
            LauncherAction::Emit(Intent::Launch("aida queue work --auto-complete".into()))
        );
    }

    #[test]
    fn select_helpers_move_list_cursor_when_list_focused() {
        // List-focused: the row cursor moves (pure — no store shell-out),
        // the section stays put.
        let mut m = fixture_focus(vec![queued_row("A"), queued_row("B")], Pane::List);
        let start_section = m.nav.current();
        assert_eq!(m.selected, 0);
        select_next_focused(&mut m, None, None);
        assert_eq!(m.selected, 1);
        select_prev_focused(&mut m, None, None);
        assert_eq!(m.selected, 0);
        // The Nav selection is untouched while focus is in the list.
        assert_eq!(m.nav.current(), start_section);
    }

    #[test]
    fn nav_section_move_is_pure_in_navstate() {
        // The Nav-pane move delegates to NavState::select_{next,prev}; we
        // assert that pure step here (the loop pairs it with a row
        // refetch). Keeping the assertion off `select_*_focused` avoids a
        // live `aida` shell-out inside the unit test.
        let mut nav = crate::nav::NavState::default();
        let start = nav.current();
        nav.select_next();
        assert_ne!(nav.current(), start);
        nav.select_prev();
        assert_eq!(nav.current(), start);
    }

    #[test]
    fn key_to_section_maps_direct_keys() {
        assert_eq!(key_to_section(plain('b')), Some(NavSection::Backlog));
        assert_eq!(key_to_section(plain('h')), Some(NavSection::History));
        assert_eq!(key_to_section(plain('p')), Some(NavSection::Prs));
        assert_eq!(key_to_section(plain('s')), Some(NavSection::Sessions));
        assert_eq!(key_to_section(plain('x')), None);
    }

    #[test]
    fn enter_on_queued_row_emits_launch_intent() {
        let row = ListRow {
            id: "STORY-244".into(),
            title: "TUI pivot".into(),
            status: "Approved".into(),
            kind: RowKind::Queued,
        };
        let intent = act_on_row(&row, RoleTab::Implementer, NavSection::Queue);
        assert_eq!(intent, Intent::Launch("aida queue work STORY-244".into()));
    }

    #[test]
    fn enter_on_session_row_emits_resume_intent() {
        let row = ListRow {
            id: "019e2d4f-7777-7abc".into(),
            title: "dialog session".into(),
            status: "resume".into(),
            kind: RowKind::Session,
        };
        let intent = act_on_row(&row, RoleTab::Dialog, NavSection::Sessions);
        assert_eq!(intent, Intent::Resume("019e2d4f-7777-7abc".into()));
    }

    #[test]
    fn enter_on_pr_row_emits_shell_gh_pr_view() {
        let row = ListRow {
            id: "42".into(),
            title: "PR #42".into(),
            status: "green".into(),
            kind: RowKind::Pr,
        };
        let intent = act_on_row(&row, RoleTab::Reviewer, NavSection::Prs);
        assert_eq!(intent, Intent::Shell("gh pr view 42".into()));
    }

    #[test]
    fn enter_on_action_drain_emits_auto_complete() {
        let row = ListRow {
            id: "drain".into(),
            title: "/aida-drain".into(),
            status: "".into(),
            kind: RowKind::Action,
        };
        let intent = act_on_row(&row, RoleTab::Implementer, NavSection::ActionDrain);
        assert_eq!(
            intent,
            Intent::Launch("aida queue work --auto-complete".into())
        );
    }

    #[test]
    fn enter_on_history_row_emits_show() {
        let row = ListRow {
            id: "STORY-1".into(),
            title: "done".into(),
            status: "Completed".into(),
            kind: RowKind::History,
        };
        let intent = act_on_row(&row, RoleTab::Implementer, NavSection::History);
        assert_eq!(intent, Intent::Launch("aida show STORY-1".into()));
    }

    // --- STORY-686 blocked-board reason dispatch -------------------------

    /// Build a reason row of a given kind for the dispatch-Intent tests.
    fn reason_row(id: &str, kind: RowKind) -> ListRow {
        ListRow {
            id: id.into(),
            title: "row".into(),
            status: "—".into(),
            kind,
        }
    }

    #[test]
    fn enter_on_reason_rows_dispatches_the_unblock_action() {
        use crate::board::Reason;
        let sect = |r: Reason| NavSection::Reason(r);

        // needs approval → approve the draft.
        assert_eq!(
            act_on_row(
                &reason_row("STORY-3", RowKind::ReasonNeedsApproval),
                RoleTab::Implementer,
                sect(Reason::NeedsApproval),
            ),
            Intent::Launch("aida edit STORY-3 --status approved".into())
        );
        // needs an answer → the decision-inbox flow.
        assert_eq!(
            act_on_row(
                &reason_row("BUG-9", RowKind::ReasonNeedsAnswer),
                RoleTab::Implementer,
                sect(Reason::NeedsAnswer),
            ),
            Intent::Launch("aida questions answer BUG-9".into())
        );
        // needs attention → show the parked spec.
        assert_eq!(
            act_on_row(
                &reason_row("STORY-4", RowKind::ReasonNeedsAttention),
                RoleTab::Implementer,
                sect(Reason::NeedsAttention),
            ),
            Intent::Launch("aida show STORY-4".into())
        );
        // deferred → undefer.
        assert_eq!(
            act_on_row(
                &reason_row("STORY-6", RowKind::ReasonDeferred),
                RoleTab::Implementer,
                sect(Reason::Deferred),
            ),
            Intent::Launch("aida undefer STORY-6".into())
        );
        // in-flight / blocked → info (show the spec).
        assert_eq!(
            act_on_row(
                &reason_row("STORY-1", RowKind::ReasonInFlight),
                RoleTab::Implementer,
                sect(Reason::InFlight),
            ),
            Intent::Launch("aida show STORY-1".into())
        );
        assert_eq!(
            act_on_row(
                &reason_row("STORY-2", RowKind::ReasonBlocked),
                RoleTab::Implementer,
                sect(Reason::Blocked),
            ),
            Intent::Launch("aida show STORY-2".into())
        );
    }

    #[test]
    fn enter_on_mail_row_opens_the_inbox() {
        // STORY-701: a mail row's Enter opens the reader — its `id` is a
        // message id, not a spec id, so `aida show` doesn't apply, and
        // composing a reply needs free-text the launcher's Intent payload
        // gate can't carry (see `act_on_row`'s ReasonMail arm).
        use crate::board::Reason;
        let intent = act_on_row(
            &reason_row("019e2d4f-abcd", RowKind::ReasonMail),
            RoleTab::Implementer,
            NavSection::Reason(Reason::Mail),
        );
        assert_eq!(intent, Intent::Launch("aida mailbox inbox".into()));
    }

    #[test]
    fn enter_on_awaiting_review_opens_pr_or_shows_spec() {
        use crate::board::Reason;
        let sect = NavSection::Reason(Reason::AwaitingReview);
        // A synthetic PR row (`pr:<n>`) opens the PR.
        assert_eq!(
            act_on_row(
                &reason_row("pr:42", RowKind::ReasonAwaitingReview),
                RoleTab::Reviewer,
                sect,
            ),
            Intent::Shell("gh pr view 42".into())
        );
        // A Done-on-branch spec without a PR row shows the spec.
        assert_eq!(
            act_on_row(
                &reason_row("STORY-5", RowKind::ReasonAwaitingReview),
                RoleTab::Reviewer,
                sect,
            ),
            Intent::Launch("aida show STORY-5".into())
        );
    }

    #[test]
    fn reason_dispatch_intents_are_shell_safe() {
        // Every reason-dispatch Intent must pass the wire-format safety
        // gate (the wrapper eval's it). trace:STORY-686 | ai:claude
        use crate::board::Reason;
        for (id, kind, reason) in [
            (
                "STORY-3",
                RowKind::ReasonNeedsApproval,
                Reason::NeedsApproval,
            ),
            (
                "STORY-8",
                RowKind::ReasonAdvisorBacklog,
                Reason::NeedsApproval,
            ),
            ("BUG-9", RowKind::ReasonNeedsAnswer, Reason::NeedsAnswer),
            (
                "STORY-4",
                RowKind::ReasonNeedsAttention,
                Reason::NeedsAttention,
            ),
            ("STORY-6", RowKind::ReasonDeferred, Reason::Deferred),
            ("STORY-1", RowKind::ReasonInFlight, Reason::InFlight),
            ("STORY-2", RowKind::ReasonBlocked, Reason::Blocked),
            (
                "pr:42",
                RowKind::ReasonAwaitingReview,
                Reason::AwaitingReview,
            ),
            ("019e2d4f-abcd", RowKind::ReasonMail, Reason::Mail),
        ] {
            let intent = act_on_row(
                &reason_row(id, kind),
                RoleTab::Implementer,
                NavSection::Reason(reason),
            );
            assert!(
                crate::intent::serialize(&intent).is_ok(),
                "intent for {id:?} must serialize safely: {intent:?}"
            );
        }
    }

    #[test]
    fn advisor_backlog_dispatches_to_queue_add() {
        // TASK-901: an advisor-backlog row (Approved-but-not-queued) is already
        // approved, so its unblock routes it to the queue rather than
        // re-approving it. A draft row still approves. trace:TASK-901
        let backlog = act_on_row(
            &reason_row("STORY-8", RowKind::ReasonAdvisorBacklog),
            RoleTab::Implementer,
            NavSection::Reason(crate::board::Reason::NeedsApproval),
        );
        assert_eq!(backlog, Intent::Launch("aida queue add STORY-8".into()));
        let draft = act_on_row(
            &reason_row("STORY-3", RowKind::ReasonNeedsApproval),
            RoleTab::Implementer,
            NavSection::Reason(crate::board::Reason::NeedsApproval),
        );
        assert_eq!(
            draft,
            Intent::Launch("aida edit STORY-3 --status approved".into())
        );
    }

    #[test]
    fn dispatch_palette_resolves_known_commands() {
        assert_eq!(dispatch_palette("q"), Some(Intent::Quit));
        assert_eq!(dispatch_palette("quit"), Some(Intent::Quit));
        assert_eq!(
            dispatch_palette("resume 019e2d4f"),
            Some(Intent::Resume("019e2d4f".into()))
        );
        assert_eq!(
            dispatch_palette("launch aida queue work STORY-1"),
            Some(Intent::Launch("aida queue work STORY-1".into()))
        );
        assert_eq!(dispatch_palette("unknown"), None);
        assert_eq!(dispatch_palette("resume "), None);
    }

    #[test]
    fn route_key_ctrl_a_chord_is_redraw_in_pure_router() {
        // The loop wires the chord follow-up key; the pure router just
        // signals "armed, repaint". This keeps the alternate path
        // available without complicating the unit-testable surface.
        let model = fixture(vec![]);
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert_eq!(route_key(key, &model), LauncherAction::Redraw);
    }
}
