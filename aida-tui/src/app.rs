//! Supervisor state + the event loop.
//!
//! The TUI is a synchronous thread-per-source supervisor. Two background
//! threads feed one `mpsc` channel — an input thread (real-terminal
//! keystrokes) and each PTY host's reader thread — and the main thread
//! runs a plain `for event in rx` loop over [`TuiEvent`].
//!
//! Input routing is a small state machine: `Focused` passes keystrokes
//! straight to the hosted child; the prefix key (`Ctrl-a`) toggles
//! `Command` mode for one keystroke; `prefix q` / `prefix d` exit, a
//! double-prefix sends one literal prefix byte through.
//!
//! trace:STORY-132 | ai:claude

use crate::actions::{self, ActivityEntry, QuickAction};
use crate::config::TuiConfig;
use crate::event::TuiEvent;
use crate::overlay::{self, OverlayModel};
use crate::picker::{self, PickerState};
use crate::pty::PtyHost;
use crate::statusbar;
use crate::tab::{SessionTab, TabManager};
use crate::term::pty_rows;
use anyhow::{Context, Result};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::{cursor, terminal, QueueableCommand};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{Stdout, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

/// Cap on retained activity-log entries — old quick-action output is
/// dropped once the log grows past this (the overlay only shows a tail).
const ACTIVITY_LOG_CAP: usize = 200;

/// Input-routing mode — the Focused → Command → {Overlay, Picker} state
/// machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Keystrokes pass straight through to the focused child.
    Focused,
    /// The prefix key was pressed; the next keystroke is a command.
    Command,
    /// The `prefix o` status overlay is open — keystrokes drive the
    /// overlay (selection / actions / close), never the hosted child,
    /// and the child's PTY output is buffered (not blitted) until the
    /// overlay closes. trace:STORY-133 | ai:claude
    Overlay,
    /// The `prefix n` new-session picker is open — keystrokes drive the
    /// picker (select / open / cancel). Like `Overlay`, the focused
    /// child's PTY output is buffered until the picker closes.
    /// trace:STORY-134 | ai:claude
    Picker,
}

/// The outcome of routing one keystroke through [`App::route_key`].
#[derive(Debug)]
pub enum Routing {
    /// Forward these bytes verbatim to the focused child.
    Passthrough(Vec<u8>),
    /// The prefix key toggled `Command` mode; nothing else to do.
    EnteredCommand,
    /// A `Command`-mode key with no binding — returned to `Focused`.
    Unbound,
    /// Double-prefix — send one literal prefix byte to the focused child.
    LiteralPrefix,
    /// `prefix q` — quit the TUI.
    Quit,
    /// `prefix d` — detach (quit; Claude conversations persist on disk).
    Detach,
    /// `prefix <n>` — focus absolute tab index `n` (0-based).
    SwitchTab(usize),
    /// `prefix ]` — focus the next tab.
    NextTab,
    /// `prefix [` — focus the previous tab.
    PrevTab,
    /// `prefix o` — open the status overlay (STORY-133).
    OpenOverlay,
    /// `esc` / `q` from the overlay — close it, repaint the focused tab.
    CloseOverlay,
    /// An overlay keystroke that only changed overlay state (selection
    /// moved, confirm armed / cancelled) — repaint the overlay.
    OverlayRedraw,
    /// `enter` on a quick action with no pending confirm, or `y` once a
    /// confirm is armed — run the action as a captured subprocess.
    RunAction(QuickAction),
    /// `prefix n` — open the new-session picker (STORY-134).
    OpenPicker,
    /// `esc` / `q` from the picker — close it, repaint the focused tab.
    ClosePicker,
    /// A picker keystroke that only moved the selection — repaint it.
    PickerRedraw,
    /// `enter` in the picker — spawn the selected session in a new tab.
    SpawnSelected,
}

/// How a new tab's hosted `aida queue work` child should launch.
/// trace:STORY-134 | ai:claude
enum TabLaunch {
    /// Cold launch with a fresh, caller-minted `--session-id` UUID — the
    /// TUI tracks the conversation deterministically.
    Fresh,
    /// Resume a recorded conversation via `--resume <session-id>`.
    Resume(String),
}

/// Supervisor state for one `aida tui` run.
pub struct App {
    tabs: TabManager<SessionTab>,
    mode: Mode,
    config: TuiConfig,
    /// Latest notification badge for the status strip.
    badge: Option<String>,
    /// Terminal size as `(cols, rows)`.
    term_size: (u16, u16),
    /// True once a tab has existed — distinguishes the no-scope empty
    /// shell (stay open) from a hosted session that exited (quit).
    had_tabs: bool,
    /// Monotonic source of stable tab ids.
    next_tab_id: usize,
    /// Armed by `prefix q` while children are live — the next keystroke
    /// confirms (`y`) or cancels the quit.
    quit_armed: bool,
    /// `(scope, claude_session_id)` of every session still hosted at
    /// exit — captured before children are killed so the caller can
    /// print resume hints once cooked mode is restored.
    exit_sessions: Vec<(String, String)>,
    /// State for the `prefix o` status overlay (STORY-133).
    overlay: OverlayState,
    /// Quick-action results, oldest first — the overlay's activity log.
    /// Survives overlay open/close within one `aida tui` run.
    activity: Vec<ActivityEntry>,
    /// State for the `prefix n` new-session picker (STORY-134).
    picker: PickerState,
    /// The scope the TUI was launched with, if any — the picker offers
    /// resumable conversations for it. trace:STORY-134 | ai:claude
    launch_scope: Option<String>,
    /// `ratatui` terminal, created lazily on the first modal open and
    /// reused for every subsequent draw of the overlay or picker. The
    /// supervisor's passthrough rendering writes raw bytes; this is only
    /// ever used while a modal owns the screen. trace:STORY-133 STORY-134
    ratatui_term: Option<Terminal<CrosstermBackend<Stdout>>>,
}

/// State for the status overlay — what `App` carries while [`Mode::Over
/// lay`] is active.
struct OverlayState {
    /// The most recent `aida status --json` projection driving the panels.
    model: OverlayModel,
    /// Index into [`QuickAction::ALL`] of the highlighted action.
    selected: usize,
    /// A quick action awaiting `y`/cancel confirmation, if any.
    confirm: Option<QuickAction>,
    /// True between an overlay open and the background CI refresh
    /// landing — drives the header's "fetching PR/CI…" hint.
    refreshing: bool,
}

impl OverlayState {
    fn new() -> Self {
        OverlayState {
            model: OverlayModel::default(),
            selected: 0,
            confirm: None,
            refreshing: false,
        }
    }

    /// Highlight the next action, wrapping past the end.
    fn select_next(&mut self) {
        self.selected = (self.selected + 1) % QuickAction::ALL.len();
    }

    /// Highlight the previous action, wrapping past the start.
    fn select_prev(&mut self) {
        let n = QuickAction::ALL.len();
        self.selected = (self.selected + n - 1) % n;
    }

    /// The currently highlighted action.
    fn selected_action(&self) -> QuickAction {
        QuickAction::ALL[self.selected]
    }
}

impl App {
    /// Build an empty supervisor. The event loop is [`App::run`].
    pub fn new(config: TuiConfig) -> Self {
        let max_tabs = config.max_tabs;
        App {
            tabs: TabManager::new(max_tabs),
            mode: Mode::Focused,
            config,
            badge: None,
            term_size: (80, 24),
            had_tabs: false,
            next_tab_id: 0,
            quit_armed: false,
            exit_sessions: Vec::new(),
            overlay: OverlayState::new(),
            activity: Vec::new(),
            picker: PickerState::empty(),
            launch_scope: None,
            ratatui_term: None,
        }
    }

    /// `(scope, claude_session_id)` for every session that was still
    /// hosted when the TUI exited — the caller turns each into a
    /// `aida queue work <scope> --resume <id>` hint.
    pub fn exit_sessions(&self) -> &[(String, String)] {
        &self.exit_sessions
    }

    /// Route one keystroke. Pure state machine over `self.mode` +
    /// `self.overlay` — touches no tabs and performs no I/O, so it is
    /// exhaustively unit-testable. I/O actions are deferred: the routing
    /// it returns names them and [`App::handle_routing`] performs them.
    pub fn route_key(&mut self, key: KeyEvent) -> Routing {
        let prefix = self.config.prefix_key;
        match self.mode {
            Mode::Focused => {
                if key_matches(key, prefix) {
                    self.mode = Mode::Command;
                    Routing::EnteredCommand
                } else {
                    Routing::Passthrough(encode_key(key))
                }
            }
            Mode::Command => {
                // Every command-mode key is a single shot out of Command.
                self.mode = Mode::Focused;
                if key_matches(key, prefix) {
                    return Routing::LiteralPrefix;
                }
                match key.code {
                    KeyCode::Char('q') => Routing::Quit,
                    KeyCode::Char('d') => Routing::Detach,
                    KeyCode::Char('o') => {
                        self.mode = Mode::Overlay;
                        Routing::OpenOverlay
                    }
                    KeyCode::Char('n') => {
                        self.mode = Mode::Picker;
                        Routing::OpenPicker
                    }
                    KeyCode::Char('[') => Routing::PrevTab,
                    KeyCode::Char(']') => Routing::NextTab,
                    KeyCode::Char(c @ '1'..='9') => Routing::SwitchTab(c as usize - '1' as usize),
                    _ => Routing::Unbound,
                }
            }
            Mode::Overlay => self.route_overlay_key(key),
            Mode::Picker => self.route_picker_key(key),
        }
    }

    /// Route a keystroke while the status overlay is open. A pending
    /// confirm gates everything — `y` runs the armed action, any other
    /// key cancels it; otherwise arrows / `h` / `l` / Tab move the
    /// selection, Enter runs (or arms a confirm for) the selected
    /// action, and Esc / `q` close the overlay.
    fn route_overlay_key(&mut self, key: KeyEvent) -> Routing {
        if let Some(pending) = self.overlay.confirm.take() {
            return if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                Routing::RunAction(pending)
            } else {
                Routing::OverlayRedraw
            };
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.mode = Mode::Focused;
                Routing::CloseOverlay
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.overlay.select_prev();
                Routing::OverlayRedraw
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
                self.overlay.select_next();
                Routing::OverlayRedraw
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let action = self.overlay.selected_action();
                if action.needs_confirm() {
                    self.overlay.confirm = Some(action);
                    Routing::OverlayRedraw
                } else {
                    Routing::RunAction(action)
                }
            }
            // Any other key is a no-op; an idempotent redraw is harmless.
            _ => Routing::OverlayRedraw,
        }
    }

    /// Route a keystroke while the new-session picker is open: arrows /
    /// `j` / `k` move the selection, Enter opens the highlighted session
    /// in a new tab, and Esc / `q` / `n` close the picker.
    /// trace:STORY-134 | ai:claude
    fn route_picker_key(&mut self, key: KeyEvent) -> Routing {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('n') => {
                self.mode = Mode::Focused;
                Routing::ClosePicker
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.picker.select_prev();
                Routing::PickerRedraw
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                self.picker.select_next();
                Routing::PickerRedraw
            }
            KeyCode::Enter | KeyCode::Char(' ') => Routing::SpawnSelected,
            // Any other key is a no-op; an idempotent redraw is harmless.
            _ => Routing::PickerRedraw,
        }
    }

    /// Run the supervisor loop until the user quits or detaches. Hosts
    /// the optional `scope` in the first tab; with `None`, opens an empty
    /// shell — `prefix n` then populates it from the picker (STORY-134).
    pub fn run(&mut self, scope: Option<String>) -> Result<ExitKind> {
        self.term_size = terminal::size().unwrap_or((80, 24));
        self.launch_scope = scope.clone();
        let (tx, rx) = mpsc::channel::<TuiEvent>();

        if let Some(scope) = scope {
            self.spawn_tab(&scope, TabLaunch::Fresh, tx.clone())
                .with_context(|| format!("failed to host session for `{}`", scope))?;
        }

        spawn_input_thread(tx.clone());

        let mut out = std::io::stdout();
        self.full_repaint(&mut out)?;

        let exit = self.event_loop(&rx, &tx, &mut out)?;

        // Snapshot the live sessions before teardown so the caller can
        // print `--resume` hints (the conversations persist on disk).
        self.exit_sessions = self
            .tabs
            .iter()
            .map(|t| (t.scope.clone(), t.session_id.clone()))
            .collect();

        // Tear children down explicitly (PtyHost::Drop also kills, but an
        // explicit pass keeps the intent visible). STORY-5 will make a
        // detach record the sessions for re-attach instead of killing.
        for tab in self.tabs.iter_mut() {
            tab.pty.kill();
        }
        self.cleanup_screen(&mut out, exit)?;
        Ok(exit)
    }

    /// The blocking `recv` loop. Returns how the TUI exited. `tx` is the
    /// shared supervisor channel — the overlay's background refresh
    /// thread needs a clone of it.
    fn event_loop(
        &mut self,
        rx: &Receiver<TuiEvent>,
        tx: &Sender<TuiEvent>,
        out: &mut Stdout,
    ) -> Result<ExitKind> {
        while let Ok(event) = rx.recv() {
            match event {
                TuiEvent::Input(Event::Key(key)) => {
                    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                        continue;
                    }
                    if self.quit_armed {
                        if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                            return Ok(ExitKind::Quit);
                        }
                        // Any other key cancels the pending quit.
                        self.quit_armed = false;
                        self.badge = None;
                        self.paint_strip(out)?;
                        continue;
                    }
                    if let Some(exit) = self.handle_routing(key, tx, out)? {
                        return Ok(exit);
                    }
                }
                TuiEvent::Input(Event::Resize(cols, rows)) => {
                    self.term_size = (cols, rows);
                    if let Some(tab) = self.tabs.focused() {
                        let _ = tab.pty.resize(pty_rows(rows), cols);
                    }
                    // `ratatui` autoresizes on its next draw; the focused
                    // tab is repainted from its `vt100` snapshot.
                    self.repaint(out)?;
                }
                TuiEvent::Input(_) => {}
                TuiEvent::PtyOutput { tab, bytes } => {
                    // A modal (overlay / picker) owns the screen — buffer
                    // the focused child's output (its `vt100` mirror is
                    // still fed in the reader thread) and blit it when the
                    // modal closes.
                    if !self.is_modal() && self.is_focused_tab(tab) {
                        out.write_all(&bytes)?;
                        out.flush()?;
                    }
                }
                TuiEvent::PtyExited { tab } => {
                    if let Some(idx) = self.tab_index(tab) {
                        self.tabs.remove(idx);
                    }
                    // A hosted session that has ended takes the TUI with
                    // it; the no-scope empty shell (never had a tab)
                    // stays open until the user exits.
                    if self.had_tabs && self.tabs.is_empty() {
                        return Ok(ExitKind::SessionEnded);
                    }
                    self.repaint(out)?;
                }
                TuiEvent::OverlayRefresh(model) => {
                    // The `gh`-backed refresh landed — repaint only if the
                    // overlay is still open (it may have been closed
                    // before the slow `gh` call returned).
                    if self.mode == Mode::Overlay {
                        self.overlay.model = *model;
                        self.overlay.refreshing = false;
                        self.draw_overlay()?;
                    }
                }
            }
        }
        Ok(ExitKind::Quit)
    }

    /// Apply a routed keystroke. Returns `Some(ExitKind)` to break the
    /// loop, `None` to continue. `tx` is handed to the overlay so its
    /// background refresh thread can post back into the event channel.
    fn handle_routing(
        &mut self,
        key: KeyEvent,
        tx: &Sender<TuiEvent>,
        out: &mut Stdout,
    ) -> Result<Option<ExitKind>> {
        match self.route_key(key) {
            Routing::Passthrough(bytes) => {
                if let Some(tab) = self.tabs.focused_mut() {
                    let _ = tab.pty.write_input(&bytes);
                }
            }
            Routing::LiteralPrefix => {
                let literal = encode_key(self.config.prefix_key);
                if let Some(tab) = self.tabs.focused_mut() {
                    let _ = tab.pty.write_input(&literal);
                }
            }
            Routing::EnteredCommand => self.paint_strip(out)?,
            Routing::Unbound => self.paint_strip(out)?,
            Routing::Quit => {
                if self.tabs.is_empty() {
                    return Ok(Some(ExitKind::Quit));
                }
                // Live children — confirm before killing them.
                self.quit_armed = true;
                self.badge = Some(format!(
                    "quit and end {} session(s)? y = confirm, any other key = cancel",
                    self.tabs.len()
                ));
                self.paint_strip(out)?;
            }
            Routing::Detach => return Ok(Some(ExitKind::Detached)),
            Routing::SwitchTab(idx) => {
                if self.tabs.switch_to(idx) {
                    self.focus_changed(out)?;
                }
            }
            Routing::NextTab => {
                self.tabs.next();
                self.focus_changed(out)?;
            }
            Routing::PrevTab => {
                self.tabs.prev();
                self.focus_changed(out)?;
            }
            Routing::OpenOverlay => self.open_overlay(tx)?,
            Routing::CloseOverlay => {
                self.overlay.confirm = None;
                // Repaint the focused child from its `vt100` snapshot —
                // focus returns to the same Claude conversation.
                self.full_repaint(out)?;
            }
            Routing::OverlayRedraw => self.draw_overlay()?,
            Routing::RunAction(action) => {
                self.run_quick_action(action)?;
            }
            Routing::OpenPicker => self.open_picker()?,
            Routing::ClosePicker => self.full_repaint(out)?,
            Routing::PickerRedraw => self.draw_picker()?,
            Routing::SpawnSelected => self.spawn_from_picker(tx.clone(), out)?,
        }
        Ok(None)
    }

    /// Whether a full-screen modal (overlay or picker) currently owns the
    /// screen — when true, hosted children's PTY output is buffered.
    fn is_modal(&self) -> bool {
        matches!(self.mode, Mode::Overlay | Mode::Picker)
    }

    /// Repaint whatever owns the screen for the current mode — the active
    /// modal, or the focused tab.
    fn repaint(&mut self, out: &mut Stdout) -> Result<()> {
        match self.mode {
            Mode::Overlay => self.draw_overlay(),
            Mode::Picker => self.draw_picker(),
            Mode::Focused | Mode::Command => self.full_repaint(out),
        }
    }

    /// Open the new-session picker: gather queued specs + resumable
    /// conversations for the launch scope, then draw it. trace:STORY-134
    fn open_picker(&mut self) -> Result<()> {
        let open_ids: Vec<String> = self.tabs.iter().map(|t| t.session_id.clone()).collect();
        self.picker = picker::fetch(self.launch_scope.as_deref(), &open_ids);
        self.ensure_ratatui_term()?;
        if let Some(term) = self.ratatui_term.as_mut() {
            term.clear()?;
            term.hide_cursor()?;
        }
        self.draw_picker()
    }

    /// Draw the picker into the `ratatui` terminal. Disjoint-field
    /// borrows, same as [`App::draw_overlay`].
    fn draw_picker(&mut self) -> Result<()> {
        self.ensure_ratatui_term()?;
        let state = &self.picker;
        let at_cap = self.tabs.len() >= self.config.max_tabs;
        let term = self
            .ratatui_term
            .as_mut()
            .expect("ratatui terminal initialized above");
        term.draw(|frame| picker::render(frame, state, at_cap))?;
        Ok(())
    }

    /// Spawn the picker's highlighted session in a new tab, then close
    /// the picker. A full tab manager keeps the picker open with a note
    /// instead — the user must free a slot first. trace:STORY-134
    fn spawn_from_picker(&mut self, tx: Sender<TuiEvent>, out: &mut Stdout) -> Result<()> {
        let Some(entry) = self.picker.selected_entry().cloned() else {
            // Empty picker — nothing to spawn; just close it.
            self.mode = Mode::Focused;
            return self.full_repaint(out);
        };
        if self.tabs.len() >= self.config.max_tabs {
            // Stay in the picker; the cap note is rendered by the picker.
            return self.draw_picker();
        }
        let launch = match &entry {
            picker::PickerEntry::Fresh { .. } => TabLaunch::Fresh,
            picker::PickerEntry::Resume { session_id, .. } => TabLaunch::Resume(session_id.clone()),
        };
        match self.spawn_tab(entry.scope(), launch, tx) {
            Ok(()) => {
                self.mode = Mode::Focused;
                self.full_repaint(out)
            }
            Err(e) => {
                // Surface the failure in the picker rather than crashing.
                self.picker.note = Some(format!("could not open session: {e}"));
                self.draw_picker()
            }
        }
    }

    /// Build the `ratatui` terminal on first use (overlay or picker).
    fn ensure_ratatui_term(&mut self) -> Result<()> {
        if self.ratatui_term.is_none() {
            self.ratatui_term = Some(Terminal::new(CrosstermBackend::new(std::io::stdout()))?);
        }
        Ok(())
    }

    /// Open the status overlay: paint immediately from a cache-only
    /// `aida status --json --no-ci` (sub-millisecond), then kick off a
    /// background `gh`-backed refresh that repaints when it lands.
    fn open_overlay(&mut self, tx: &Sender<TuiEvent>) -> Result<()> {
        self.overlay.selected = 0;
        self.overlay.confirm = None;
        match overlay::fetch(true) {
            Ok(model) => self.overlay.model = model,
            Err(e) => {
                // A status failure must not block the overlay — render an
                // empty model and log why the panels are bare.
                self.overlay.model = OverlayModel::default();
                self.push_activity(ActivityEntry::note("aida status", &e.to_string()));
            }
        }
        self.overlay.refreshing = true;
        spawn_overlay_refresh(tx.clone());

        // Lazily build the `ratatui` terminal; `clear()` forces a full
        // redraw over whatever the hosted child left on screen.
        self.ensure_ratatui_term()?;
        if let Some(term) = self.ratatui_term.as_mut() {
            term.clear()?;
            term.hide_cursor()?;
        }
        self.draw_overlay()
    }

    /// Draw the overlay into the `ratatui` terminal. Disjoint-field
    /// borrows: the panel inputs are borrowed before `ratatui_term` is
    /// borrowed mutably, so the draw closure and the terminal don't
    /// alias.
    fn draw_overlay(&mut self) -> Result<()> {
        self.ensure_ratatui_term()?;
        let model = &self.overlay.model;
        let activity = &self.activity;
        let selected = self.overlay.selected;
        let confirm = self.overlay.confirm;
        let refreshing = self.overlay.refreshing;
        let term = self
            .ratatui_term
            .as_mut()
            .expect("ratatui terminal initialized above");
        term.draw(|frame| overlay::render(frame, model, activity, selected, confirm, refreshing))?;
        Ok(())
    }

    /// Run a quick action as a captured subprocess, append the result to
    /// the activity log, and repaint the overlay. The overlay stays open
    /// so the user sees the output; focus returns to Claude on close.
    fn run_quick_action(&mut self, action: QuickAction) -> Result<()> {
        self.overlay.confirm = None;
        let exe = aida_exe().to_string_lossy().into_owned();
        let entry = actions::run(action, &exe);
        self.push_activity(entry);
        self.draw_overlay()
    }

    /// Append an activity-log entry, trimming the oldest once the log
    /// passes [`ACTIVITY_LOG_CAP`].
    fn push_activity(&mut self, entry: ActivityEntry) {
        self.activity.push(entry);
        if self.activity.len() > ACTIVITY_LOG_CAP {
            let drop = self.activity.len() - ACTIVITY_LOG_CAP;
            self.activity.drain(0..drop);
        }
    }

    /// Spawn a hosted session for `scope` in a fresh tab. The child is
    /// always `aida queue work` (never `claude` directly — all lease /
    /// worktree / manifest logic is inherited); `launch` selects between
    /// a fresh `--session-id` conversation and a `--resume` of a recorded
    /// one. trace:STORY-132 STORY-134 | ai:claude
    fn spawn_tab(&mut self, scope: &str, launch: TabLaunch, tx: Sender<TuiEvent>) -> Result<()> {
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;

        let mut argv = vec![
            aida_exe().to_string_lossy().into_owned(),
            "queue".to_string(),
            "work".to_string(),
            scope.to_string(),
        ];
        let session_id = match launch {
            TabLaunch::Fresh => {
                let sid = uuid::Uuid::now_v7().to_string();
                argv.push("--session-id".to_string());
                argv.push(sid.clone());
                sid
            }
            TabLaunch::Resume(sid) => {
                argv.push("--resume".to_string());
                argv.push(sid.clone());
                sid
            }
        };

        let (cols, rows) = self.term_size;
        let pty = PtyHost::spawn(&argv, pty_rows(rows), cols, tab_id, tx)?;

        let tab = SessionTab {
            id: tab_id,
            session_id,
            scope: scope.to_string(),
            pty,
            title: scope.to_string(),
        };
        self.tabs.add(tab)?;
        self.had_tabs = true;
        Ok(())
    }

    /// Whether stable tab id `id` is the currently focused tab.
    fn is_focused_tab(&self, id: usize) -> bool {
        self.tabs.focused().map(|t| t.id) == Some(id)
    }

    /// Positional index of the tab with stable id `id`.
    fn tab_index(&self, id: usize) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    /// Repaint after a focus change: resize the now-focused child to the
    /// live terminal, blit its screen snapshot, repaint the strip.
    fn focus_changed(&mut self, out: &mut Stdout) -> Result<()> {
        let (cols, rows) = self.term_size;
        if let Some(tab) = self.tabs.focused() {
            let _ = tab.pty.resize(pty_rows(rows), cols);
        }
        self.full_repaint(out)
    }

    /// Clear the screen, blit the focused child's snapshot, paint the
    /// strip. Used on launch, resize, focus change.
    fn full_repaint(&mut self, out: &mut Stdout) -> Result<()> {
        out.queue(terminal::Clear(terminal::ClearType::All))?;
        out.queue(cursor::MoveTo(0, 0))?;
        if let Some(tab) = self.tabs.focused() {
            let snapshot = tab.pty.snapshot();
            out.write_all(&snapshot)?;
        }
        self.paint_strip(out)
    }

    /// Repaint just the status strip and restore the cursor to where the
    /// focused child left it (or hide it when there is no child).
    fn paint_strip(&self, out: &mut Stdout) -> Result<()> {
        let (cols, rows) = self.term_size;
        let strip_row = rows.saturating_sub(1);
        let chips: Vec<String> = self.tabs.iter().map(|t| t.title.clone()).collect();
        let hint = match self.mode {
            Mode::Focused => format!("{} = command", describe_key(self.config.prefix_key)),
            Mode::Command => "command: o overlay · n new · q quit · d detach · [ ] tab".to_string(),
            // The overlay and picker paint their own full-screen chrome;
            // the strip is only ever drawn in Focused / Command, but the
            // arms are needed for the match to stay exhaustive.
            Mode::Overlay => "overlay open".to_string(),
            Mode::Picker => "picker open".to_string(),
        };
        statusbar::render(
            out,
            strip_row,
            cols,
            &chips,
            self.tabs.focused_index(),
            self.badge.as_deref(),
            &hint,
        )?;
        if let Some(tab) = self.tabs.focused() {
            let (crow, ccol) = tab.pty.cursor_position();
            out.queue(cursor::MoveTo(ccol, crow))?;
            out.queue(cursor::Show)?;
        } else {
            out.queue(cursor::MoveTo(0, 0))?;
            out.queue(cursor::Hide)?;
        }
        out.flush()?;
        Ok(())
    }

    /// Final screen cleanup before the terminal guard restores cooked
    /// mode — clear, home the cursor, print a one-line exit notice.
    fn cleanup_screen(&self, out: &mut Stdout, exit: ExitKind) -> Result<()> {
        out.queue(terminal::Clear(terminal::ClearType::All))?;
        out.queue(cursor::MoveTo(0, 0))?;
        out.queue(cursor::Show)?;
        out.flush()?;
        let _ = exit; // The notice prints after the guard restores cooked mode.
        Ok(())
    }
}

/// How an `aida tui` run ended — drives the post-exit notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitKind {
    /// `prefix q` (or quitting an empty shell).
    Quit,
    /// `prefix d` — conversations persist on disk.
    Detached,
    /// The last hosted session's child exited on its own.
    SessionEnded,
}

impl ExitKind {
    /// One-line summary printed once cooked mode is restored.
    pub fn notice(self) -> &'static str {
        match self {
            ExitKind::Quit => "aida tui: exited",
            ExitKind::Detached => {
                "aida tui: detached — Claude conversations persist (resume with `aida queue work --resume`)"
            }
            ExitKind::SessionEnded => "aida tui: hosted session ended",
        }
    }
}

/// The `aida` binary to host / shell out to — the currently-running
/// executable, so a dev build hosts (and queries) the same dev build.
pub(crate) fn aida_exe() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("aida"))
}

/// Spawn the overlay's background refresh: one CI-inclusive
/// `aida status --json` whose result posts back as [`TuiEvent::Overlay
/// Refresh`]. Detached and fire-and-forget — if the overlay closes
/// before `gh` returns, the event is simply dropped by the loop.
fn spawn_overlay_refresh(tx: Sender<TuiEvent>) {
    std::thread::spawn(move || {
        if let Ok(model) = overlay::fetch(false) {
            let _ = tx.send(TuiEvent::OverlayRefresh(Box::new(model)));
        }
    });
}

/// Spawn the input thread: one [`TuiEvent::Input`] per crossterm event.
/// Detached — the thread is blocked in `read()` at exit and the process
/// reaps it.
fn spawn_input_thread(tx: Sender<TuiEvent>) {
    std::thread::spawn(move || {
        // Ends on a read error (terminal closed) or a send error
        // (supervisor gone) — either way the process is exiting.
        while let Ok(ev) = crossterm::event::read() {
            if tx.send(TuiEvent::Input(ev)).is_err() {
                break;
            }
        }
    });
}

/// Compare two key events by code + modifiers only — `KeyEvent`'s derived
/// equality also covers `kind`/`state`, which would make a configured
/// prefix (always `Press`) miss a `Repeat`.
fn key_matches(a: KeyEvent, b: KeyEvent) -> bool {
    a.code == b.code && a.modifiers == b.modifiers
}

/// A short human label for a key, for the status-strip hint (`^a`).
fn describe_key(key: KeyEvent) -> String {
    let mut s = String::new();
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        s.push('^');
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        s.push_str("M-");
    }
    match key.code {
        KeyCode::Char(c) => s.push(c),
        other => s.push_str(&format!("{:?}", other)),
    }
    s
}

/// Encode a key event into the byte sequence a PTY child expects. Covers
/// the common cases (printable chars, control combos, the named editing
/// keys, arrows); exotic sequences are dropped rather than mis-encoded.
pub fn encode_key(key: KeyEvent) -> Vec<u8> {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let mut bytes: Vec<u8> = match key.code {
        KeyCode::Char(c) => {
            if ctrl && c.is_ascii() {
                let b = c as u8;
                let is_ctrlable = b.is_ascii_alphabetic() || b"@[\\]^_ ".contains(&b);
                if is_ctrlable {
                    vec![b.to_ascii_uppercase() & 0x1f]
                } else {
                    vec![b]
                }
            } else {
                c.to_string().into_bytes()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => vec![0x1b, b'[', b'Z'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => vec![0x1b, b'[', b'A'],
        KeyCode::Down => vec![0x1b, b'[', b'B'],
        KeyCode::Right => vec![0x1b, b'[', b'C'],
        KeyCode::Left => vec![0x1b, b'[', b'D'],
        KeyCode::Home => vec![0x1b, b'[', b'H'],
        KeyCode::End => vec![0x1b, b'[', b'F'],
        KeyCode::PageUp => vec![0x1b, b'[', b'5', b'~'],
        KeyCode::PageDown => vec![0x1b, b'[', b'6', b'~'],
        KeyCode::Delete => vec![0x1b, b'[', b'3', b'~'],
        KeyCode::Insert => vec![0x1b, b'[', b'2', b'~'],
        _ => Vec::new(),
    };
    // ALT is transmitted as an ESC prefix on the sequence.
    if alt && !bytes.is_empty() {
        bytes.insert(0, 0x1b);
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    fn plain(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn prefix_key_routes_command_then_passthrough() {
        let mut app = App::new(TuiConfig::default());
        let prefix = config::default_prefix_key();

        // Focused: the prefix key toggles into Command mode.
        assert!(matches!(app.route_key(prefix), Routing::EnteredCommand));
        assert_eq!(app.mode, Mode::Command);

        // Command: an unbound key returns to Focused with no action.
        assert!(matches!(app.route_key(plain('z')), Routing::Unbound));
        assert_eq!(app.mode, Mode::Focused);

        // Focused again: an ordinary key passes straight through.
        match app.route_key(plain('h')) {
            Routing::Passthrough(bytes) => assert_eq!(bytes, b"h"),
            other => panic!("expected passthrough, got {:?}", other),
        }
        assert_eq!(app.mode, Mode::Focused);
    }

    #[test]
    fn double_prefix_sends_literal_prefix() {
        let mut app = App::new(TuiConfig::default());
        let prefix = config::default_prefix_key();

        assert!(matches!(app.route_key(prefix), Routing::EnteredCommand));
        // Second prefix press: emit one literal prefix byte, back to Focused.
        assert!(matches!(app.route_key(prefix), Routing::LiteralPrefix));
        assert_eq!(app.mode, Mode::Focused);
        // Ctrl-a encodes to exactly one byte: 0x01.
        assert_eq!(encode_key(prefix), vec![0x01]);
    }

    #[test]
    fn command_mode_bindings_route_to_exit_and_tabs() {
        let mut app = App::new(TuiConfig::default());
        let prefix = config::default_prefix_key();

        app.route_key(prefix);
        assert!(matches!(app.route_key(plain('q')), Routing::Quit));

        app.route_key(prefix);
        assert!(matches!(app.route_key(plain('d')), Routing::Detach));

        app.route_key(prefix);
        assert!(matches!(app.route_key(plain('2')), Routing::SwitchTab(1)));

        app.route_key(prefix);
        assert!(matches!(app.route_key(plain(']')), Routing::NextTab));

        app.route_key(prefix);
        assert!(matches!(app.route_key(plain('[')), Routing::PrevTab));
    }

    fn enter() -> KeyEvent {
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
    }

    #[test]
    fn prefix_o_opens_overlay_and_esc_closes_it() {
        let mut app = App::new(TuiConfig::default());
        let prefix = config::default_prefix_key();

        // `prefix o` enters Overlay mode.
        app.route_key(prefix);
        assert!(matches!(app.route_key(plain('o')), Routing::OpenOverlay));
        assert_eq!(app.mode, Mode::Overlay);

        // A navigation key keeps the overlay open and just redraws.
        assert!(matches!(app.route_key(plain('l')), Routing::OverlayRedraw));
        assert_eq!(app.mode, Mode::Overlay);

        // `esc` closes back to Focused.
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(app.route_key(esc), Routing::CloseOverlay));
        assert_eq!(app.mode, Mode::Focused);
    }

    #[test]
    fn overlay_enter_runs_read_only_action_without_confirm() {
        let mut app = App::new(TuiConfig::default());
        let prefix = config::default_prefix_key();
        app.route_key(prefix);
        app.route_key(plain('o'));

        // Selection starts on QueueNext — read-only, so Enter runs it
        // straight away and the overlay stays open for the output.
        assert_eq!(app.overlay.selected_action(), QuickAction::QueueNext);
        match app.route_key(enter()) {
            Routing::RunAction(QuickAction::QueueNext) => {}
            other => panic!("expected RunAction(QueueNext), got {:?}", other),
        }
        assert_eq!(app.mode, Mode::Overlay);
    }

    #[test]
    fn overlay_session_end_requires_y_confirm() {
        let mut app = App::new(TuiConfig::default());
        let prefix = config::default_prefix_key();
        app.route_key(prefix);
        app.route_key(plain('o'));

        // Move to "End session" — Enter arms a confirm, runs nothing.
        app.route_key(plain('l'));
        assert_eq!(app.overlay.selected_action(), QuickAction::SessionEnd);
        assert!(matches!(app.route_key(enter()), Routing::OverlayRedraw));
        assert_eq!(app.overlay.confirm, Some(QuickAction::SessionEnd));

        // `y` confirms → the action runs and the confirm clears.
        match app.route_key(plain('y')) {
            Routing::RunAction(QuickAction::SessionEnd) => {}
            other => panic!("expected RunAction(SessionEnd), got {:?}", other),
        }
        assert!(app.overlay.confirm.is_none());
    }

    #[test]
    fn overlay_confirm_is_cancelled_by_any_other_key() {
        let mut app = App::new(TuiConfig::default());
        let prefix = config::default_prefix_key();
        app.route_key(prefix);
        app.route_key(plain('o'));
        app.route_key(plain('l')); // select "End session"
        app.route_key(enter()); // arm the confirm
        assert_eq!(app.overlay.confirm, Some(QuickAction::SessionEnd));

        // Any non-`y` key cancels the confirm; the overlay stays open.
        assert!(matches!(app.route_key(plain('n')), Routing::OverlayRedraw));
        assert!(app.overlay.confirm.is_none());
        assert_eq!(app.mode, Mode::Overlay);
    }

    #[test]
    fn prefix_n_opens_picker_and_esc_closes_it() {
        let mut app = App::new(TuiConfig::default());
        let prefix = config::default_prefix_key();

        // `prefix n` enters Picker mode.
        app.route_key(prefix);
        assert!(matches!(app.route_key(plain('n')), Routing::OpenPicker));
        assert_eq!(app.mode, Mode::Picker);

        // `esc` closes back to Focused.
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(app.route_key(esc), Routing::ClosePicker));
        assert_eq!(app.mode, Mode::Focused);
    }

    #[test]
    fn picker_navigates_and_enter_spawns_selection() {
        use crate::picker::{PickerEntry, PickerState};
        let mut app = App::new(TuiConfig::default());
        let prefix = config::default_prefix_key();
        app.route_key(prefix);
        app.route_key(plain('n'));

        // Populate two candidates so navigation is observable.
        app.picker = PickerState::new(vec![
            PickerEntry::Fresh {
                spec_id: "STORY-1".into(),
                title: "first".into(),
                status: "Approved".into(),
            },
            PickerEntry::Fresh {
                spec_id: "STORY-2".into(),
                title: "second".into(),
                status: "Approved".into(),
            },
        ]);
        assert_eq!(app.picker.selected, 0);

        // `j` / Down moves the selection; the picker stays open.
        assert!(matches!(app.route_key(plain('j')), Routing::PickerRedraw));
        assert_eq!(app.picker.selected, 1);
        assert_eq!(app.mode, Mode::Picker);

        // Enter routes a spawn of the highlighted entry.
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(app.route_key(enter), Routing::SpawnSelected));
    }

    #[test]
    fn encode_key_covers_named_keys() {
        assert_eq!(encode_key(plain('A')), b"A");
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            b"\r"
        );
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            b"\x1b"
        );
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            b"\x1b[A"
        );
        // Ctrl-c → 0x03 (ETX); Alt-x → ESC-prefixed.
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            vec![0x03]
        );
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT)),
            vec![0x1b, b'x']
        );
    }
}
