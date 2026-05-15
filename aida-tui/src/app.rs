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

use crate::config::TuiConfig;
use crate::event::TuiEvent;
use crate::pty::PtyHost;
use crate::statusbar;
use crate::tab::{SessionTab, TabManager};
use crate::term::pty_rows;
use anyhow::{Context, Result};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::{cursor, terminal, QueueableCommand};
use std::io::{Stdout, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

/// Input-routing mode. `Overlay` is intentionally absent — the status
/// overlay lands in STORY-133, which adds the variant and its handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Keystrokes pass straight through to the focused child.
    Focused,
    /// The prefix key was pressed; the next keystroke is a command.
    Command,
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
        }
    }

    /// `(scope, claude_session_id)` for every session that was still
    /// hosted when the TUI exited — the caller turns each into a
    /// `aida queue work <scope> --resume <id>` hint.
    pub fn exit_sessions(&self) -> &[(String, String)] {
        &self.exit_sessions
    }

    /// Route one keystroke. Pure state machine over `self.mode` — touches
    /// no tabs and performs no I/O, so it is exhaustively unit-testable.
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
                // Every command-mode key is a single shot back to Focused.
                self.mode = Mode::Focused;
                if key_matches(key, prefix) {
                    return Routing::LiteralPrefix;
                }
                match key.code {
                    KeyCode::Char('q') => Routing::Quit,
                    KeyCode::Char('d') => Routing::Detach,
                    KeyCode::Char('[') => Routing::PrevTab,
                    KeyCode::Char(']') => Routing::NextTab,
                    KeyCode::Char(c @ '1'..='9') => Routing::SwitchTab(c as usize - '1' as usize),
                    _ => Routing::Unbound,
                }
            }
        }
    }

    /// Run the supervisor loop until the user quits or detaches. Hosts
    /// the optional `scope` in the first tab; with `None`, opens an empty
    /// shell the user can exit cleanly (STORY-3 adds the new-tab picker).
    pub fn run(&mut self, scope: Option<String>) -> Result<ExitKind> {
        self.term_size = terminal::size().unwrap_or((80, 24));
        let (tx, rx) = mpsc::channel::<TuiEvent>();

        if let Some(scope) = scope {
            self.spawn_tab(&scope, tx.clone())
                .with_context(|| format!("failed to host session for `{}`", scope))?;
        }

        spawn_input_thread(tx);

        let mut out = std::io::stdout();
        self.full_repaint(&mut out)?;

        let exit = self.event_loop(&rx, &mut out)?;

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

    /// The blocking `recv` loop. Returns how the TUI exited.
    fn event_loop(&mut self, rx: &Receiver<TuiEvent>, out: &mut Stdout) -> Result<ExitKind> {
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
                    if let Some(exit) = self.handle_routing(key, out)? {
                        return Ok(exit);
                    }
                }
                TuiEvent::Input(Event::Resize(cols, rows)) => {
                    self.term_size = (cols, rows);
                    if let Some(tab) = self.tabs.focused() {
                        let _ = tab.pty.resize(pty_rows(rows), cols);
                    }
                    self.full_repaint(out)?;
                }
                TuiEvent::Input(_) => {}
                TuiEvent::PtyOutput { tab, bytes } => {
                    if self.is_focused_tab(tab) {
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
                    self.full_repaint(out)?;
                }
            }
        }
        Ok(ExitKind::Quit)
    }

    /// Apply a routed keystroke. Returns `Some(ExitKind)` to break the
    /// loop, `None` to continue.
    fn handle_routing(&mut self, key: KeyEvent, out: &mut Stdout) -> Result<Option<ExitKind>> {
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
        }
        Ok(None)
    }

    /// Spawn a hosted session for `scope` in a fresh tab. The child is
    /// `aida queue work <scope> --session-id <uuid>` (never `claude`
    /// directly — all lease / worktree / manifest logic is inherited).
    fn spawn_tab(&mut self, scope: &str, tx: Sender<TuiEvent>) -> Result<()> {
        let session_id = uuid::Uuid::now_v7().to_string();
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;

        let (cols, rows) = self.term_size;
        let argv = vec![
            aida_exe().to_string_lossy().into_owned(),
            "queue".to_string(),
            "work".to_string(),
            scope.to_string(),
            "--session-id".to_string(),
            session_id.clone(),
        ];
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
            Mode::Command => "command: q quit · d detach · [ ] tab".to_string(),
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

/// The `aida` binary to host — the currently-running executable, so a
/// dev build hosts the same dev build.
fn aida_exe() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("aida"))
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
