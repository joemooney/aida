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
use crate::config::{TabVendor, TuiConfig};
use crate::event::TuiEvent;
use crate::help;
use crate::overlay::{self, OverlayModel};
use crate::palette::{self, Dispatched, PaletteState};
use crate::picker::{self, PickerState};
use crate::pty::PtyHost;
use crate::state::{self, TabRecord, TuiState};
use crate::statusbar;
use crate::tab::{SessionTab, TabManager};
use crate::term::pty_rows;
use crate::welcome;
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
    /// overlay closes.
    //
    // trace:STORY-133 | ai:claude
    Overlay,
    /// The `prefix n` new-session picker is open — keystrokes drive the
    /// picker (select / open / cancel). Like `Overlay`, the focused
    /// child's PTY output is buffered until the picker closes.
    // trace:STORY-134 | ai:claude
    Picker,
    /// The `prefix ?` keybinding cheatsheet is open — keystrokes either
    /// close it (Esc / `q` / `?`) or are ignored, and like the other
    /// modals the focused child's PTY output is buffered until it
    /// closes.
    //
    // trace:BUG-109 | ai:claude
    Help,
    /// The `prefix p` pause surface is up — the focused child's process
    /// group is `SIGSTOP`ped (STORY-678) and its PTY output is buffered.
    /// While paused, a **deterministic AIDA action palette** owns the
    /// screen (STORY-679, EPIC-51 slice 2): keystrokes type/filter/select
    /// curated actions that run `aida … --json` subprocesses inline — zero
    /// LLM round-trip — never reaching the frozen child. `Esc` drives the
    /// resume (`SIGCONT` + repaint). Semantically a modal like `Overlay`.
    // trace:STORY-678 trace:STORY-679 | ai:claude
    Paused,
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
    /// `prefix ?` (or a bare `?` in the empty shell) — open the
    /// keybinding cheatsheet (BUG-109).
    OpenHelp,
    /// `esc` / `q` / `?` from the cheatsheet — close it, repaint beneath.
    CloseHelp,
    /// A cheatsheet keystroke with no binding — an idempotent redraw.
    HelpRedraw,
    /// `prefix p` — pause the focused child (`SIGSTOP` its process group)
    /// and open the deterministic action palette.
    //
    // trace:STORY-678 STORY-679
    Pause,
    /// `Esc` while paused — resume the focused child (`SIGCONT`) and
    /// repaint it from its snapshot.
    //
    // trace:STORY-678
    Resume,
    /// `Ctrl-Y` while paused with a result in hand — resume the focused
    /// child (`SIGCONT`) AND type the last palette result into its PTY
    /// stdin as a quoted context block, so the conversation continues with
    /// that result in view (the immediate-response slice's payoff). Routed
    /// as a plain `Resume` when no result has been produced yet.
    //
    // trace:STORY-680
    ResumeWithInject,
    /// A palette keystroke that only changed palette state (a typed/erased
    /// character, a moved selection) — repaint the palette.
    //
    // trace:STORY-679
    PaletteRedraw,
    /// `Enter` in the palette — run the highlighted (or typed `spec`/`run`)
    /// deterministic action as a captured subprocess.
    //
    // trace:STORY-679
    RunPaletteAction,
}

/// How a new tab's hosted `aida queue work` child should launch.
// trace:STORY-134 | ai:claude
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
    /// Index into the status strip's rotating discovery hints, advanced
    /// by [`TuiEvent::Tick`] every ~3s.
    //
    // trace:BUG-109 | ai:claude
    hint_index: usize,
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
    /// State for the suspended-chat deterministic action palette
    /// (STORY-679, EPIC-51 slice 2) — the typed query + highlighted
    /// candidate. Live only while [`Mode::Paused`].
    //
    // trace:STORY-679
    paused_palette: PaletteState,
    /// The scope the TUI was launched with, if any — the picker offers
    /// resumable conversations for it.
    //
    // trace:STORY-134 | ai:claude
    launch_scope: Option<String>,
    /// Project root (holds `.aida/`) — where the crash-recovery state
    /// file lives.
    //
    // trace:STORY-135 | ai:claude
    project_root: PathBuf,
    /// `ratatui` terminal, created lazily on the first modal open and
    /// reused for every subsequent draw of the overlay or picker. The
    /// supervisor's passthrough rendering writes raw bytes; this is only
    /// ever used while a modal owns the screen.
    //
    // trace:STORY-133 STORY-134
    ratatui_term: Option<Terminal<CrosstermBackend<Stdout>>>,
    /// Cached mission-control snapshot (queue head + open-PR count) for the
    /// empty-state welcome panel. Fetched lazily on the first empty-shell
    /// repaint and invalidated to `None` whenever a session ends (so re-
    /// entering the empty shell re-reads the now-changed queue). Keeping it
    /// cached avoids the `gh` shell-out firing on every repaint (resize,
    /// modal close).
    //
    // trace:TASK-255 | ai:claude
    mission: Option<welcome::MissionData>,
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
            hint_index: 0,
            next_tab_id: 0,
            quit_armed: false,
            exit_sessions: Vec::new(),
            overlay: OverlayState::new(),
            activity: Vec::new(),
            picker: PickerState::empty(),
            paused_palette: PaletteState::new(),
            launch_scope: None,
            project_root: PathBuf::new(),
            ratatui_term: None,
            mission: None,
        }
    }

    /// `(scope, claude_session_id)` for every session that was still
    /// hosted when the TUI exited — the caller turns each into a
    /// `aida queue work <scope> --resume <id>` hint.
    pub fn exit_sessions(&self) -> &[(String, String)] {
        &self.exit_sessions
    }

    /// Route one keystroke. Near-pure state machine over `self.mode` +
    /// `self.overlay` (plus a read of the tab count, for the empty-shell
    /// `?` shortcut) — performs no I/O, so it is exhaustively
    /// unit-testable. I/O actions are deferred: the routing it returns
    /// names them and [`App::handle_routing`] performs them.
    pub fn route_key(&mut self, key: KeyEvent) -> Routing {
        let prefix = self.config.prefix_key;
        match self.mode {
            Mode::Focused => {
                if key_matches(key, prefix) {
                    self.mode = Mode::Command;
                    Routing::EnteredCommand
                } else if self.tabs.is_empty() && is_help_key(key) {
                    // Empty shell — no hosted child to receive the `?`,
                    // so a bare `?` opens the cheatsheet (BUG-109). With
                    // a session focused, `?` belongs to the child and
                    // only `prefix ?` opens help.
                    self.mode = Mode::Help;
                    Routing::OpenHelp
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
                    KeyCode::Char('?') => {
                        self.mode = Mode::Help;
                        Routing::OpenHelp
                    }
                    KeyCode::Char('p') => {
                        // `prefix p` — pause the focused child. The mode
                        // flip + SIGSTOP + overlay paint happen in
                        // `handle_routing`. trace:STORY-678
                        self.mode = Mode::Paused;
                        Routing::Pause
                    }
                    KeyCode::Char('[') => Routing::PrevTab,
                    KeyCode::Char(']') => Routing::NextTab,
                    KeyCode::Char(c @ '1'..='9') => Routing::SwitchTab(c as usize - '1' as usize),
                    _ => Routing::Unbound,
                }
            }
            Mode::Overlay => self.route_overlay_key(key),
            Mode::Picker => self.route_picker_key(key),
            Mode::Help => self.route_help_key(key),
            // Paused: the deterministic action palette owns the screen.
            // Keystrokes drive it; none reach the frozen child. Only `Esc`
            // resumes the conversation. trace:STORY-678 trace:STORY-679
            Mode::Paused => self.route_palette_key(key),
        }
    }

    /// Route a keystroke while the suspended-chat action palette is open.
    /// `Esc` resumes the frozen conversation; `Ctrl-Y` resumes AND injects
    /// the last result into the chat; `Enter` runs the highlighted (or typed
    /// `spec`/`run`) deterministic action; printable characters / Backspace
    /// edit the filter query; arrows / Tab move the selection. Every path is
    /// deterministic — no keystroke ever reaches the `SIGSTOP`ped child
    /// unmediated, and nothing here spawns an LLM.
    //
    // trace:STORY-679 trace:STORY-680 | ai:claude
    fn route_palette_key(&mut self, key: KeyEvent) -> Routing {
        // Ctrl-Y — "yank" the last palette result into the resumed chat as a
        // context block (EPIC-51 slice 3). Intercepted before the generic
        // `Char` arm so it never lands as filter text. With no result yet,
        // it degrades to a plain resume. trace:STORY-680
        if matches!(key.code, KeyCode::Char('y')) && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.mode = Mode::Focused;
            if self.activity.last().is_some() {
                return Routing::ResumeWithInject;
            }
            return Routing::Resume;
        }
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Focused;
                Routing::Resume
            }
            KeyCode::Enter => Routing::RunPaletteAction,
            KeyCode::Up => {
                self.paused_palette.select_prev();
                Routing::PaletteRedraw
            }
            KeyCode::Down | KeyCode::Tab => {
                self.paused_palette.select_next();
                Routing::PaletteRedraw
            }
            KeyCode::Backspace => {
                self.paused_palette.backspace();
                Routing::PaletteRedraw
            }
            KeyCode::Char(c) => {
                self.paused_palette.push_char(c);
                Routing::PaletteRedraw
            }
            // Any other key is a no-op; an idempotent redraw is harmless.
            _ => Routing::PaletteRedraw,
        }
    }

    /// Route a keystroke while the `prefix ?` keybinding cheatsheet is
    /// open: Esc / `q` / `?` close it, every other key is an idempotent
    /// redraw.
    //
    // trace:BUG-109 | ai:claude
    fn route_help_key(&mut self, key: KeyEvent) -> Routing {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
                self.mode = Mode::Focused;
                Routing::CloseHelp
            }
            // Any other key is a no-op; an idempotent redraw is harmless.
            _ => Routing::HelpRedraw,
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
    // trace:STORY-134 | ai:claude
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

    /// Run the supervisor loop until the user quits or detaches.
    ///
    /// On launch (STORY-135) any sessions a prior crashed-or-detached TUI
    /// left in `.aida/tui-state.json` are re-attached first (unless
    /// `no_recover`), then the freshly-requested `scope` is hosted. With
    /// neither, an empty shell opens — `prefix n` populates it.
    pub fn run(
        &mut self,
        project_root: PathBuf,
        scope: Option<String>,
        no_recover: bool,
    ) -> Result<ExitKind> {
        self.project_root = project_root;
        self.term_size = terminal::size().unwrap_or((80, 24));
        self.launch_scope = scope.clone();
        let (tx, rx) = mpsc::channel::<TuiEvent>();

        // STORY-135: re-attach orphaned sessions before hosting the
        // requested scope. `--no-recover` discards the stale state so a
        // later launch doesn't pick it up either.
        let mut recovered = 0usize;
        if no_recover {
            state::clear(&self.project_root);
        } else if let Some(prior) = state::load(&self.project_root) {
            for rec in prior.tabs {
                if self.tabs.len() >= self.config.max_tabs {
                    break;
                }
                if self
                    .spawn_tab(&rec.scope, TabLaunch::Resume(rec.session_id), tx.clone())
                    .is_ok()
                {
                    recovered += 1;
                }
            }
        }

        if let Some(scope) = scope {
            // Don't double-host a scope a recovered tab already covers.
            let already_hosted = self.tabs.iter().any(|t| t.scope == scope);
            if !already_hosted && self.tabs.len() < self.config.max_tabs {
                self.spawn_tab(&scope, TabLaunch::Fresh, tx.clone())
                    .with_context(|| format!("failed to host session for `{}`", scope))?;
            }
        }
        if recovered > 0 {
            self.badge = Some(format!(
                "recovered {} session(s) from a prior TUI",
                recovered
            ));
        }

        spawn_input_thread(tx.clone());
        spawn_tick_thread(tx.clone());

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

        // STORY-135: a clean `prefix q` quit has nothing to recover —
        // drop the state file. A `prefix d` detach leaves it in place so
        // the next launch re-attaches the conversations.
        match exit {
            ExitKind::Quit => state::clear(&self.project_root),
            ExitKind::Detached => {}
        }

        // Tear children down explicitly (PtyHost::Drop also kills, but an
        // explicit pass keeps the intent visible). The conversations are
        // durable `.jsonl` files — a detach re-attaches them via the
        // state file, regardless of the child process's fate.
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
                        // STORY-135: the recoverable tab set just shrank.
                        self.persist_state();
                    }
                    // TASK-255: a session ending likely changed the queue /
                    // PR picture — drop the cached mission snapshot so the
                    // empty-shell repaint below re-reads it fresh.
                    self.mission = None;
                    // BUG-109: a hosted session ending no longer takes
                    // the TUI with it — the supervisor drops back to the
                    // welcome shell, a persistent home the user leaves
                    // with `prefix q`. `repaint` renders the welcome
                    // panel once `tabs` is empty.
                    self.repaint(out)?;
                }
                TuiEvent::Tick => {
                    // Rotate the status strip's discovery hint (~3s
                    // cadence, BUG-109). Paused whenever the strip is
                    // showing something else — a modal owns the screen,
                    // the prefix is armed (`Command`), or a quit confirm
                    // is pending.
                    if self.mode == Mode::Focused && !self.quit_armed {
                        self.hint_index = self.hint_index.wrapping_add(1);
                        self.paint_strip(out)?;
                    }
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
            Routing::RunAction(action) => match action.injection() {
                // Drain actions type a slash command into the focused
                // Claude session (STORY-136); the rest run as captured
                // subprocesses.
                Some(text) => self.inject_to_focused(text, out)?,
                None => self.run_quick_action(action)?,
            },
            Routing::OpenPicker => self.open_picker()?,
            Routing::ClosePicker => self.full_repaint(out)?,
            Routing::PickerRedraw => self.draw_picker()?,
            Routing::SpawnSelected => self.spawn_from_picker(tx.clone(), out)?,
            Routing::OpenHelp => self.open_help()?,
            // Repaint whatever was beneath the cheatsheet — the focused
            // child, or the welcome panel in the empty shell.
            Routing::CloseHelp => self.full_repaint(out)?,
            Routing::HelpRedraw => self.draw_help()?,
            Routing::Pause => self.open_paused()?,
            Routing::Resume => {
                // Mirror the `CloseOverlay` close path exactly: SIGCONT the
                // child first, then repaint it from its `vt100` snapshot +
                // restore the real cursor + repaint the status strip (all
                // inside `full_repaint` / `paint_strip`). trace:STORY-678
                if let Some(tab) = self.tabs.focused() {
                    tab.pty.resume();
                }
                self.full_repaint(out)?;
            }
            Routing::ResumeWithInject => self.resume_with_inject(out)?,
            Routing::PaletteRedraw => self.draw_paused()?,
            Routing::RunPaletteAction => self.run_palette_action()?,
        }
        Ok(None)
    }

    /// Whether a full-screen modal (overlay, picker or help) currently
    /// owns the screen — when true, hosted children's PTY output is
    /// buffered.
    fn is_modal(&self) -> bool {
        matches!(
            self.mode,
            Mode::Overlay | Mode::Picker | Mode::Help | Mode::Paused
        )
    }

    /// Repaint whatever owns the screen for the current mode — the active
    /// modal, or (in `Focused` / `Command`) the focused tab or, when no
    /// tab is hosted, the welcome panel.
    fn repaint(&mut self, out: &mut Stdout) -> Result<()> {
        match self.mode {
            Mode::Overlay => self.draw_overlay(),
            Mode::Picker => self.draw_picker(),
            Mode::Help => self.draw_help(),
            Mode::Paused => self.draw_paused(),
            Mode::Focused | Mode::Command => self.full_repaint(out),
        }
    }

    /// Open the new-session picker: gather queued specs + resumable
    /// conversations for the launch scope, then draw it.
    //
    // trace:STORY-134
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
    /// instead — the user must free a slot first.
    //
    // trace:STORY-134
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

    /// Open the `prefix ?` keybinding cheatsheet (BUG-109). Static
    /// content — no model to fetch, unlike the status overlay; `clear()`
    /// forces a full redraw over whatever was on screen.
    fn open_help(&mut self) -> Result<()> {
        self.ensure_ratatui_term()?;
        if let Some(term) = self.ratatui_term.as_mut() {
            term.clear()?;
            term.hide_cursor()?;
        }
        self.draw_help()
    }

    /// Draw the keybinding cheatsheet into the `ratatui` terminal. The
    /// prefix label is resolved before the terminal is borrowed mutably,
    /// so the draw closure and the terminal don't alias.
    fn draw_help(&mut self) -> Result<()> {
        self.ensure_ratatui_term()?;
        let prefix = describe_key_long(self.config.prefix_key);
        let term = self
            .ratatui_term
            .as_mut()
            .expect("ratatui terminal initialized above");
        term.draw(|frame| help::render(frame, &prefix))?;
        Ok(())
    }

    /// Open the `prefix p` pause surface: `SIGSTOP` the focused child's
    /// process group (STORY-678), reset the deterministic action palette to
    /// a fresh empty query (STORY-679), then paint it. `clear()` forces a
    /// full redraw over the frozen child's screen. The pause still works
    /// with no session focused — the palette is usable standalone (its
    /// actions are independent `aida` subprocesses), which is also how a
    /// future `Ctrl-D`-suspend trigger will reach it.
    //
    // trace:STORY-678 trace:STORY-679 | ai:claude
    fn open_paused(&mut self) -> Result<()> {
        if let Some(tab) = self.tabs.focused() {
            tab.pty.suspend();
        }
        self.paused_palette = PaletteState::new();
        self.ensure_ratatui_term()?;
        if let Some(term) = self.ratatui_term.as_mut() {
            term.clear()?;
            term.hide_cursor()?;
        }
        self.draw_paused()
    }

    /// Draw the suspended-chat deterministic action palette (STORY-679)
    /// into the `ratatui` terminal: the `:` query line, the fuzzy-ranked
    /// candidate list, the last result, and the key hints. Disjoint-field
    /// borrows — palette state + the last activity entry are borrowed before
    /// `ratatui_term` is borrowed mutably.
    //
    // trace:STORY-679 | ai:claude
    fn draw_paused(&mut self) -> Result<()> {
        self.ensure_ratatui_term()?;
        let state = &self.paused_palette;
        let last = self.activity.last();
        let term = self
            .ratatui_term
            .as_mut()
            .expect("ratatui terminal initialized above");
        term.draw(|frame| palette::render(frame, state, last))?;
        Ok(())
    }

    /// Run the palette's current selection as a captured `aida … --json`
    /// subprocess (STORY-679) and land the result in the activity log, then
    /// repaint the palette so the output shows inline. The palette stays
    /// open — the frozen chat is resumed only by `Esc`. Deterministic: the
    /// argv comes from [`crate::palette::PaletteState::dispatch`], never an
    /// LLM. A refused line (empty / unsafe `spec`/`run`) becomes a note.
    //
    // trace:STORY-679 | ai:claude
    fn run_palette_action(&mut self) -> Result<()> {
        let exe = aida_exe().to_string_lossy().into_owned();
        let entry = match self.paused_palette.dispatch(&exe) {
            Dispatched::Run { label, argv } => actions::run_argv(&label, &argv),
            Dispatched::Refused(note) => ActivityEntry::note("palette", &note),
        };
        self.push_activity(entry);
        self.draw_paused()
    }

    /// Resume the suspended chat AND inject the last palette result into it
    /// as a quoted context block — the immediate-response slice's payoff.
    ///
    /// Sequence (order matters):
    ///
    /// 1. `SIGCONT` the focused child so its line discipline is live again
    ///    before we feed it bytes (writing to a `SIGSTOP`ped child's PTY
    ///    would queue input the frozen reader can't drain — it must be
    ///    running to consume the typed block + submit byte).
    /// 2. Format the last activity entry as a context block
    ///    ([`crate::palette::format_injection`]) and type it into the
    ///    focused PTY's stdin via the same write path the drain injection
    ///    uses ([`App::write_to_focused_pty`] — bytes + `\r`).
    /// 3. Full-repaint so the conversation is back on screen with the result
    ///    typed in, ready for the user to send / continue.
    ///
    /// With no focused child the injection is skipped with a note; with no
    /// result this path is never reached (`route_palette_key` degrades to a
    /// plain `Resume`). The mode is already `Focused` by the time we get
    /// here — set in `route_palette_key` — so this never types into the
    /// palette.
    //
    // trace:STORY-680 | ai:claude
    fn resume_with_inject(&mut self, out: &mut Stdout) -> Result<()> {
        // 1. SIGCONT first — the child must be running to consume the input.
        if let Some(tab) = self.tabs.focused() {
            tab.pty.resume();
        }
        // 2. Format the latest result and type it into the focused PTY. Own
        //    the block so the `&self.activity` borrow ends before the
        //    `&mut self` write call.
        let block = self.activity.last().map(palette::format_injection);
        match block {
            Some(text) => {
                if self.write_to_focused_pty(&text) {
                    self.push_activity(ActivityEntry::note(
                        "palette",
                        "injected the result into the resumed conversation",
                    ));
                } else {
                    self.push_activity(ActivityEntry::note(
                        "palette",
                        "no focused session to inject into — resumed only",
                    ));
                }
            }
            // Defensive: routing guarantees a result, but never panic if the
            // log was somehow cleared between routing and handling.
            None => {}
        }
        self.full_repaint(out)
    }

    /// Build the `ratatui` terminal on first use (overlay, picker, help).
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

    /// Type `text` into the focused child's PTY stdin, followed by a single
    /// `\r` submit byte. Returns `true` if a child was focused (bytes
    /// written), `false` if there was no session to type into. The shared
    /// low-level write path behind both the drain injection and the
    /// palette-result injection — neither hand-rolls the PTY write, so the
    /// "bytes + submit byte" contract lives in one place.
    //
    // trace:STORY-136 trace:STORY-680 | ai:claude
    fn write_to_focused_pty(&mut self, text: &str) -> bool {
        if self.tabs.focused().is_none() {
            return false;
        }
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(b'\r');
        if let Some(tab) = self.tabs.focused_mut() {
            let _ = tab.pty.write_input(&bytes);
        }
        true
    }

    /// Type a slash command into the focused Claude session, then close
    /// the overlay so Claude — now focused — receives and runs it. The
    /// autonomous-drain buttons use this (STORY-136): the drain runs
    /// *inside* the hosted conversation, not as a TUI subprocess, and
    /// the drain text comes from `/aida-drain-queue`, never hand-written
    /// `/goal`.
    //
    // trace:STORY-136 | ai:claude
    fn inject_to_focused(&mut self, text: &str, out: &mut Stdout) -> Result<()> {
        self.overlay.confirm = None;
        self.mode = Mode::Focused;
        if self.write_to_focused_pty(text) {
            self.push_activity(ActivityEntry::note(
                "drain",
                &format!("started — typed `{text}` into the focused session"),
            ));
        } else {
            self.push_activity(ActivityEntry::note(
                "drain",
                "no focused session — open one with `prefix n` before starting a drain",
            ));
        }
        self.full_repaint(out)
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
    /// always `aida queue work` (never the vendor CLI directly — all lease /
    /// worktree / manifest logic is inherited); `launch` selects between
    /// a fresh conversation and a `--resume` of a recorded one. The vendor
    /// (Claude default, or Codex when `[tui] vendor = "codex"`) selects which
    /// CLI `aida queue work` hosts and how the session is threaded.
    // trace:STORY-132 STORY-134 trace:TASK-895 | ai:claude
    fn spawn_tab(&mut self, scope: &str, launch: TabLaunch, tx: Sender<TuiEvent>) -> Result<()> {
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;

        let exe = aida_exe().to_string_lossy().into_owned();
        let (argv, session_id) = queue_work_argv(&exe, scope, launch, self.config.vendor);

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
        // STORY-135: the recoverable tab set just changed.
        self.persist_state();
        Ok(())
    }

    /// Write the live tab set to `.aida/tui-state.json` so a crash or
    /// `prefix d` detach can re-attach the sessions on the next launch.
    /// Preserves `dialog_session_id` if a prior launcher run wrote one —
    /// the launcher and PTY-host paths share the file.
    //
    // trace:STORY-135
    /// STORY-244 | ai:claude
    fn persist_state(&self) {
        let tabs = self
            .tabs
            .iter()
            .map(|t| TabRecord {
                session_id: t.session_id.clone(),
                scope: t.scope.clone(),
            })
            .collect();
        // Preserve dialog_session_id across PTY-host saves.
        let dialog_session_id = state::load(&self.project_root).and_then(|s| s.dialog_session_id);
        state::save(
            &self.project_root,
            &TuiState {
                tabs,
                dialog_session_id,
            },
        );
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

    /// Clear the screen, blit the focused child's snapshot (or the
    /// welcome panel when no session is hosted), paint the strip. Used on
    /// launch, resize, focus change, and when a modal closes.
    fn full_repaint(&mut self, out: &mut Stdout) -> Result<()> {
        out.queue(terminal::Clear(terminal::ClearType::All))?;
        out.queue(cursor::MoveTo(0, 0))?;
        if let Some(tab) = self.tabs.focused() {
            let snapshot = tab.pty.snapshot();
            out.write_all(&snapshot)?;
        } else {
            // Empty shell — no hosted child to blit. Render the welcome
            // panel so a first-time user sees the key vocabulary instead
            // of a blank black screen. trace:BUG-109 | ai:claude
            //
            // TASK-255: enrich it into a thin mission-control view — the
            // role queue head + open-PR count beneath the keys. Fetched
            // lazily once and cached on `self.mission` so the `gh` leg
            // doesn't fire on every repaint (resize, focus change, modal
            // close); invalidated when a session ends.
            if self.mission.is_none() {
                self.mission = Some(welcome::fetch_mission());
            }
            let (cols, rows) = self.term_size;
            let prefix = describe_key_long(self.config.prefix_key);
            welcome::render(
                out,
                cols,
                rows.saturating_sub(1),
                &prefix,
                self.mission.as_ref(),
            )?;
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
            // A rotating discovery hint (BUG-109) — advanced by `Tick`.
            Mode::Focused => rotating_hint(self.config.prefix_key, self.hint_index),
            Mode::Command => {
                "command: n new · o overlay · p pause · ? help · d detach · q quit · [ ] tab"
                    .to_string()
            }
            // The overlay, picker, help and paused modals paint their own
            // full-screen chrome; the strip is only ever drawn in
            // Focused / Command, but the arms are needed for the match
            // to stay exhaustive.
            Mode::Overlay => "overlay open".to_string(),
            Mode::Picker => "picker open".to_string(),
            Mode::Help => "help open".to_string(),
            Mode::Paused => "paused".to_string(),
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
///
/// Since BUG-109 the TUI is a persistent shell: a hosted session ending
/// drops back to the welcome panel rather than exiting, so the only ways
/// out are `prefix q` and `prefix d`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitKind {
    /// `prefix q` — quitting the welcome shell or ending hosted sessions.
    Quit,
    /// `prefix d` — conversations persist on disk.
    Detached,
}

impl ExitKind {
    /// One-line summary printed once cooked mode is restored.
    pub fn notice(self) -> &'static str {
        match self {
            ExitKind::Quit => "aida tui: exited",
            ExitKind::Detached => {
                "aida tui: detached — Claude conversations persist (resume with `aida queue work --resume`)"
            }
        }
    }
}

/// The `aida` binary to host / shell out to — the currently-running
/// executable, so a dev build hosts (and queries) the same dev build.
pub(crate) fn aida_exe() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("aida"))
}

/// Build the `aida queue work` argv (program + args) a hosted tab spawns, plus
/// the session id the TUI records for the tab. Pure — exhaustively
/// unit-testable without spawning a PTY.
///
/// The vendor selects which CLI `aida queue work` ultimately hosts and how the
/// session is threaded:
///   - `Claude` — byte-identical to the prior path: a fresh launch mints a
///     caller-side UUID and threads `--session-id <uuid>`; a resume threads
///     `--resume <session-id>`. The minted/resumed id is tracked so the TUI can
///     re-attach the conversation.
///   - `Codex` — adds `--vendor codex`. Codex's interactive CLI has no
///     caller-minted `--session-id` and no addressable resume from the TUI's
///     side, so a Codex tab always hosts a *fresh* session (a `Resume` launch
///     falls back to fresh) and threads no `--session-id`/`--resume`. The
///     returned session id is empty, which suppresses the (claude-only) resume
///     hint at exit. Codex resume-parity is a follow-up.
// trace:TASK-895 | ai:claude
fn queue_work_argv(
    exe: &str,
    scope: &str,
    launch: TabLaunch,
    vendor: TabVendor,
) -> (Vec<String>, String) {
    let mut argv = vec![
        exe.to_string(),
        "queue".to_string(),
        "work".to_string(),
        scope.to_string(),
    ];
    match vendor {
        TabVendor::Claude => {
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
            (argv, session_id)
        }
        TabVendor::Codex => {
            // Codex has no caller-minted session id and no TUI-addressable
            // resume, so always host a fresh session (drop any `Resume` id) and
            // route `aida queue work` to the codex interactive CLI.
            argv.push("--vendor".to_string());
            argv.push(TabVendor::Codex.as_str().to_string());
            (argv, String::new())
        }
    }
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

/// Spawn the strip-rotation ticker: one [`TuiEvent::Tick`] every ~3s,
/// driving the status strip's rotating discovery hint (BUG-109).
/// Detached and fire-and-forget — it ends once the supervisor's receiver
/// is dropped.
//
// trace:BUG-109 | ai:claude
fn spawn_tick_thread(tx: Sender<TuiEvent>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(3));
        if tx.send(TuiEvent::Tick).is_err() {
            break;
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

/// A long human label for a key (`Ctrl-A`, `Alt-B`) — for the welcome
/// panel and help cheatsheet, where `describe_key`'s terse `^a` reads
/// worse.
//
// trace:BUG-109 | ai:claude
pub(crate) fn describe_key_long(key: KeyEvent) -> String {
    let mut s = String::new();
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        s.push_str("Ctrl-");
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        s.push_str("Alt-");
    }
    match key.code {
        KeyCode::Char(c) => s.push(c.to_ascii_uppercase()),
        other => s.push_str(&format!("{:?}", other)),
    }
    s
}

/// Whether `key` is a bare `?` (Shift permitted; Ctrl / Alt not) — the
/// empty-shell help shortcut. With a session focused `?` belongs to the
/// hosted child, so only `prefix ?` opens help there.
//
// trace:BUG-109
fn is_help_key(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('?')
        && !key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT)
}

/// The verbs the status strip's right-side hint rotates through — a
/// small set so a first-time user discovers the key vocabulary without
/// a busy, static strip.
//
// trace:BUG-109 | ai:claude
const HINT_VERBS: [&str; 4] = [
    "N new session",
    "O status overlay",
    "? keybindings",
    "Q quit",
];

/// The status strip's right-side discovery hint for `Focused` mode —
/// the prefix label plus one of [`HINT_VERBS`], selected by `index`
/// (advanced ~3s by [`TuiEvent::Tick`]).
//
// trace:BUG-109 | ai:claude
fn rotating_hint(prefix: KeyEvent, index: usize) -> String {
    format!(
        "{} {}",
        describe_key(prefix),
        HINT_VERBS[index % HINT_VERBS.len()]
    )
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

    #[test]
    fn prefix_p_opens_the_action_palette() {
        // STORY-678 suspends the chat; STORY-679 fills the paused surface
        // with the deterministic action palette. `prefix p` enters Paused
        // mode and routes a Pause (which SIGSTOPs + opens the palette).
        let mut app = App::new(TuiConfig::default());
        let prefix = config::default_prefix_key();

        app.route_key(prefix);
        assert!(matches!(app.route_key(plain('p')), Routing::Pause));
        assert_eq!(app.mode, Mode::Paused);
    }

    #[test]
    fn palette_typing_filters_and_stays_paused() {
        // While paused, printable keys drive the palette's filter — they do
        // NOT resume (and never reach the frozen child). trace:STORY-679
        let mut app = App::new(TuiConfig::default());
        let prefix = config::default_prefix_key();
        app.route_key(prefix);
        app.route_key(plain('p'));

        // Type into the palette: each keystroke is a redraw, not a resume.
        for c in "find".chars() {
            assert!(matches!(app.route_key(plain(c)), Routing::PaletteRedraw));
            assert_eq!(app.mode, Mode::Paused);
        }
        assert_eq!(app.paused_palette.query, "find");
        // The filter narrowed to the single matching action.
        assert_eq!(
            app.paused_palette
                .ranked()
                .iter()
                .map(|r| r.action.keyword())
                .collect::<Vec<_>>(),
            vec!["findings"]
        );
    }

    #[test]
    fn palette_arrows_move_selection_enter_runs() {
        let mut app = App::new(TuiConfig::default());
        let prefix = config::default_prefix_key();
        app.route_key(prefix);
        app.route_key(plain('p'));

        // Down moves the selection; the palette stays open.
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        assert!(matches!(app.route_key(down), Routing::PaletteRedraw));
        assert_eq!(app.paused_palette.selected, 1);
        assert_eq!(app.mode, Mode::Paused);

        // Enter runs the highlighted deterministic action (no LLM).
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(app.route_key(enter), Routing::RunPaletteAction));
        // Still paused — only Esc resumes the conversation.
        assert_eq!(app.mode, Mode::Paused);
    }

    #[test]
    fn palette_esc_resumes_the_conversation() {
        let mut app = App::new(TuiConfig::default());
        let prefix = config::default_prefix_key();
        app.route_key(prefix);
        app.route_key(plain('p'));
        assert_eq!(app.mode, Mode::Paused);

        // Only Esc resumes (SIGCONT + repaint) — nothing reaches the frozen
        // child. trace:STORY-678 trace:STORY-679
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(app.route_key(esc), Routing::Resume));
        assert_eq!(app.mode, Mode::Focused);
    }

    #[test]
    fn palette_ctrl_y_without_a_result_just_resumes() {
        // STORY-680: Ctrl-Y resumes + injects, but with no result produced
        // yet there is nothing to inject — it degrades to a plain resume,
        // and the `y` is NOT typed into the filter.
        let mut app = App::new(TuiConfig::default());
        let prefix = config::default_prefix_key();
        app.route_key(prefix);
        app.route_key(plain('p'));
        assert_eq!(app.mode, Mode::Paused);
        assert!(app.activity.is_empty());

        let ctrl_y = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL);
        assert!(matches!(app.route_key(ctrl_y), Routing::Resume));
        assert_eq!(app.mode, Mode::Focused);
        // The Ctrl-Y never reached the palette query.
        assert!(app.paused_palette.query.is_empty());
    }

    #[test]
    fn palette_ctrl_y_with_a_result_resumes_and_injects() {
        // STORY-680: once an action has produced a result, Ctrl-Y routes a
        // ResumeWithInject (resume the chat AND type the result into it).
        let mut app = App::new(TuiConfig::default());
        let prefix = config::default_prefix_key();
        app.route_key(prefix);
        app.route_key(plain('p'));
        assert_eq!(app.mode, Mode::Paused);

        // Simulate a run having landed a result in the activity log.
        app.push_activity(ActivityEntry::note("queue", "3 items queued"));

        let ctrl_y = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL);
        assert!(matches!(app.route_key(ctrl_y), Routing::ResumeWithInject));
        assert_eq!(app.mode, Mode::Focused);
        assert!(app.paused_palette.query.is_empty());
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
    fn overlay_drain_action_confirms_then_routes_run_action() {
        let mut app = App::new(TuiConfig::default());
        let prefix = config::default_prefix_key();
        app.route_key(prefix);
        app.route_key(plain('o'));

        // QuickAction::ALL = [QueueNext, SessionEnd, PrView, DrainToReview,
        // DrainToMerge] — step right to "Drain → review" (index 3).
        for _ in 0..3 {
            app.route_key(plain('l'));
        }
        assert_eq!(app.overlay.selected_action(), QuickAction::DrainToReview);

        // A drain is autonomous → it arms a confirm, runs nothing yet.
        assert!(matches!(app.route_key(enter()), Routing::OverlayRedraw));
        assert_eq!(app.overlay.confirm, Some(QuickAction::DrainToReview));

        // `y` confirms → RunAction; handle_routing injects the
        // `/aida-drain-queue` slash command into the focused session.
        match app.route_key(plain('y')) {
            Routing::RunAction(QuickAction::DrainToReview) => {}
            other => panic!("expected RunAction(DrainToReview), got {:?}", other),
        }
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
    fn bare_question_opens_help_in_the_empty_shell() {
        // No tabs hosted — a bare `?` has no child to receive it, so it
        // opens the keybinding cheatsheet (BUG-109).
        let mut app = App::new(TuiConfig::default());
        assert!(app.tabs.is_empty());
        match app.route_key(plain('?')) {
            Routing::OpenHelp => {}
            other => panic!("expected OpenHelp, got {:?}", other),
        }
        assert_eq!(app.mode, Mode::Help);

        // Esc closes the cheatsheet back to Focused.
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(app.route_key(esc), Routing::CloseHelp));
        assert_eq!(app.mode, Mode::Focused);
    }

    #[test]
    fn prefix_question_opens_help_and_question_closes_it() {
        let mut app = App::new(TuiConfig::default());
        let prefix = config::default_prefix_key();

        // `prefix ?` enters Help mode.
        app.route_key(prefix);
        assert!(matches!(app.route_key(plain('?')), Routing::OpenHelp));
        assert_eq!(app.mode, Mode::Help);

        // A no-op key keeps the cheatsheet open and just redraws.
        assert!(matches!(app.route_key(plain('z')), Routing::HelpRedraw));
        assert_eq!(app.mode, Mode::Help);

        // `?` again closes it back to Focused.
        assert!(matches!(app.route_key(plain('?')), Routing::CloseHelp));
        assert_eq!(app.mode, Mode::Focused);
    }

    #[test]
    fn rotating_hint_cycles_and_wraps() {
        let prefix = config::default_prefix_key();
        // Consecutive indices give different hints.
        assert_ne!(rotating_hint(prefix, 0), rotating_hint(prefix, 1));
        // The index wraps modulo the verb count.
        assert_eq!(
            rotating_hint(prefix, 0),
            rotating_hint(prefix, HINT_VERBS.len())
        );
        // Each hint carries the short prefix label.
        assert!(rotating_hint(prefix, 0).starts_with("^a "));
    }

    #[test]
    fn describe_key_long_is_human_readable() {
        assert_eq!(describe_key_long(config::default_prefix_key()), "Ctrl-A");
        assert_eq!(
            describe_key_long(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT)),
            "Alt-B"
        );
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

    // TASK-895: the hosted-tab argv must be vendor-correct — Claude threads
    // `--session-id`/`--resume` (byte-identical to before), Codex routes
    // `aida queue work --vendor codex` with no session threading.
    #[test]
    fn queue_work_argv_claude_fresh_threads_session_id() {
        let (argv, sid) = queue_work_argv("aida", "TASK-1", TabLaunch::Fresh, TabVendor::Claude);
        assert_eq!(argv[0..4], ["aida", "queue", "work", "TASK-1"]);
        // `--session-id <uuid>` is threaded, and the returned id matches it.
        let pos = argv.iter().position(|a| a == "--session-id").unwrap();
        assert_eq!(argv[pos + 1], sid);
        assert!(!sid.is_empty());
        assert!(!argv.iter().any(|a| a == "--vendor"));
        assert!(!argv.iter().any(|a| a == "--resume"));
    }

    #[test]
    fn queue_work_argv_claude_resume_threads_resume_id() {
        let (argv, sid) = queue_work_argv(
            "aida",
            "TASK-1",
            TabLaunch::Resume("abc-123".to_string()),
            TabVendor::Claude,
        );
        assert_eq!(sid, "abc-123");
        let pos = argv.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(argv[pos + 1], "abc-123");
        assert!(!argv.iter().any(|a| a == "--vendor"));
        assert!(!argv.iter().any(|a| a == "--session-id"));
    }

    #[test]
    fn queue_work_argv_codex_routes_vendor_and_no_session_threading() {
        let (argv, sid) = queue_work_argv("aida", "TASK-1", TabLaunch::Fresh, TabVendor::Codex);
        assert_eq!(argv[0..4], ["aida", "queue", "work", "TASK-1"]);
        let pos = argv.iter().position(|a| a == "--vendor").unwrap();
        assert_eq!(argv[pos + 1], "codex");
        // Codex has no caller-minted session id / TUI-addressable resume.
        assert!(sid.is_empty());
        assert!(!argv.iter().any(|a| a == "--session-id"));
        assert!(!argv.iter().any(|a| a == "--resume"));
    }

    #[test]
    fn queue_work_argv_codex_resume_falls_back_to_fresh() {
        // A `Resume` launch on Codex must drop the id (Codex can't resume from
        // the TUI) and still host a fresh session with no resume threading.
        let (argv, sid) = queue_work_argv(
            "aida",
            "TASK-1",
            TabLaunch::Resume("ignored".to_string()),
            TabVendor::Codex,
        );
        assert!(sid.is_empty());
        assert!(!argv.iter().any(|a| a == "--resume"));
        assert!(argv.iter().any(|a| a == "--vendor"));
    }
}
