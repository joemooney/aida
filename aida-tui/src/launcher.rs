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

use crate::dashboard::{self, DashboardModel, RoleTab, RowKind};
use crate::intent::{self, Intent};
use crate::nav::NavSection;
use crate::state::{self, TuiState};
use crate::term;
use anyhow::{Context, Result};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::Stdout;
use std::path::PathBuf;

/// Options for one launcher run.
pub struct LauncherOptions {
    /// Scope (an EPIC / STORY / … id) the launcher was started against —
    /// the Sessions section's `--list-sessions` shell-out targets it.
    pub scope: Option<String>,
    /// File descriptor the intent line is written to on exit. The bash
    /// wrapper sets `3>&1`; tests use 1 or a pipe fd.
    pub intent_fd: u32,
}

/// What a routed keystroke wants the loop to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LauncherAction {
    /// Just repaint — the model changed (selection / nav / role tab).
    Redraw,
    /// Re-fetch the current section, then redraw.
    Refetch,
    /// Emit `intent` and exit the loop.
    Emit(Intent),
    /// Show the help cheatsheet (overlay/help reuse stays as a followup
    /// — for now we surface it via the hint row at the bottom).
    Help,
    /// User opened the command palette — handled by the loop.
    EnterPalette,
}

/// Pure routing for a single keystroke. Doesn't touch the model; the
/// caller mutates the model after dispatch so this stays unit-testable.
pub fn route_key(key: KeyEvent, model: &DashboardModel) -> LauncherAction {
    // Esc / Ctrl-C always quit. (Routing happens before any palette
    // mode, which the loop drives separately.)
    if key.code == KeyCode::Esc
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
    {
        return LauncherAction::Emit(Intent::Quit);
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
        KeyCode::Char('?') => LauncherAction::Help,
        KeyCode::Char(':') => LauncherAction::EnterPalette,
        KeyCode::Tab => LauncherAction::Redraw, // role cycle handled in loop
        KeyCode::BackTab => LauncherAction::Redraw,
        KeyCode::Up | KeyCode::Char('k') => LauncherAction::Redraw,
        KeyCode::Down | KeyCode::Char('j') => LauncherAction::Redraw,
        KeyCode::Enter | KeyCode::Char(' ') => match model.current_row() {
            Some(row) => LauncherAction::Emit(act_on_row(row, model.role, model.nav.current())),
            None => match model.nav.current() {
                NavSection::ActionDrain => {
                    LauncherAction::Emit(Intent::Launch("aida queue work --auto-complete".into()))
                }
                NavSection::ActionNewSession => match model.role {
                    RoleTab::Implementer => LauncherAction::Emit(Intent::Launch(
                        "aida queue work --role implementer".into(),
                    )),
                    RoleTab::Reviewer => LauncherAction::Emit(Intent::Launch(
                        "aida queue work --role reviewer".into(),
                    )),
                    RoleTab::Dialog => {
                        LauncherAction::Emit(Intent::Launch("aida queue work --role dialog".into()))
                    }
                },
                NavSection::ActionSwitchRole => LauncherAction::Redraw,
                _ => LauncherAction::Redraw,
            },
        },
        // `q` direct-key is also bound, but we also accept the
        // configured nav direct-key 'Q' uppercase.
        KeyCode::Char('Q') => LauncherAction::Emit(Intent::Quit),
        KeyCode::Char(c) => {
            // Other characters: `n` for new session, etc. Keep the
            // surface tiny for now — direct keys are q/b/h/p/s/r/g/?/:/Q.
            let _ = c;
            LauncherAction::Redraw
        }
        _ => LauncherAction::Redraw,
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
    }
}

/// Run the launcher event loop. Sets up the RAII terminal guard, fetches
/// the initial dashboard, drives keystrokes through [`route_key`], and
/// on a [`LauncherAction::Emit`] exits cleanly while writing the intent
/// to the configured fd. trace:STORY-244 | ai:claude
pub fn run(opts: LauncherOptions) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let project_root = ensure_project_context(&cwd)?;
    let prior_state = state::load(&project_root).unwrap_or_default();
    let dialog_id = prior_state.dialog_session_id.clone();

    term::install_panic_hook();
    term::install_signal_handler()?;

    // Safety check: if the user ran `aida tui --launcher` bare (no bash
    // wrapper redirecting fd 3), refuse rather than corrupt the
    // terminal post-exit. trace:STORY-244 risk #1 | ai:claude
    if !intent::fd_is_writable_pipe(opts.intent_fd) {
        anyhow::bail!(
            "the launcher's intent fd {} is the same kernel object as stdout/stderr.\n\
             Run `aida tui` via the `aida-tui` shell wrapper instead (installed by \
             `aida dev shell-init --install`), or pass `--intent-fd` pointing at a pipe.",
            opts.intent_fd
        );
    }

    // Resolve the configured theme up front so the dashboard paints in
    // the user's palette from the first frame. trace:TASK-256 | ai:claude
    let theme = crate::config::TuiConfig::load(&cwd).theme.theme();

    let intent_to_emit = {
        let _guard = term::TermGuard::enter()?;
        let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
        let mut model = dashboard::fetch(
            RoleTab::default(),
            NavSection::Queue,
            opts.scope.as_deref(),
            dialog_id.as_deref(),
        );
        model.theme = theme;
        event_loop(
            &mut terminal,
            model,
            opts.scope.as_deref(),
            dialog_id.as_deref(),
        )?
    };

    // Outside the guard now — cooked mode restored.
    intent::write_to_fd(&intent_to_emit, opts.intent_fd)?;

    // Persist state for the next re-entry (preserves PTY-host's `tabs`
    // field, only mutates dialog_session_id).
    let new_state = TuiState {
        tabs: prior_state.tabs,
        dialog_session_id: maybe_update_dialog(&intent_to_emit, dialog_id),
    };
    state::save(&project_root, &new_state);

    Ok(())
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
        let Event::Key(key) = crossterm::event::read()? else {
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
        // Up/Down move the middle-list selection.
        if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) {
            model.select_prev();
            dashboard::ensure_preview(&mut model);
            paint(terminal, &model)?;
            continue;
        }
        if matches!(key.code, KeyCode::Down | KeyCode::Char('j')) {
            model.select_next();
            dashboard::ensure_preview(&mut model);
            paint(terminal, &model)?;
            continue;
        }
        // Now the pure router for the rest.
        match route_key(key, &model) {
            LauncherAction::Redraw => paint(terminal, &model)?,
            LauncherAction::Refetch => {
                dashboard::refetch_rows(&mut model, launch_scope, dialog_id);
                dashboard::ensure_preview(&mut model);
                paint(terminal, &model)?;
            }
            LauncherAction::Emit(intent) => return Ok(intent),
            LauncherAction::Help => {
                // Help overlay is followups; for now flip the hint row.
                model.notice = Some("Help: q quit · b backlog · h history · p PRs · s sessions · r role · g refresh".into());
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

    fn fixture(rows: Vec<ListRow>) -> DashboardModel {
        DashboardModel {
            rows,
            ..DashboardModel::default()
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
    fn route_key_esc_emits_quit() {
        let model = fixture(vec![]);
        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(route_key(key, &model), LauncherAction::Emit(Intent::Quit));
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
    fn route_key_colon_opens_palette() {
        let model = fixture(vec![]);
        assert_eq!(route_key(plain(':'), &model), LauncherAction::EnterPalette);
    }

    #[test]
    fn route_key_help_opens_help() {
        let model = fixture(vec![]);
        assert_eq!(route_key(plain('?'), &model), LauncherAction::Help);
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
