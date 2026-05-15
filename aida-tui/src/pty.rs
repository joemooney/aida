//! PTY host — one hosted child process (an `aida queue work` invocation
//! that `exec()`s into Claude Code) behind a pseudo-terminal.
//!
//! Rendering uses the tmux/passthrough model: a reader thread (a) feeds a
//! `vt100::Parser` continuously so the screen can be repainted on
//! tab-switch / overlay-close, and (b) forwards every byte to the
//! supervisor loop, which blits it straight to stdout when the owning tab
//! is focused. The `vt100` parse is *only* for the off-screen snapshot —
//! a focused child renders natively, so there is zero parser cost on the
//! hot path.
//!
//! trace:STORY-132 | ai:claude

use crate::event::TuiEvent;
use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// Scrollback the off-screen `vt100` mirror retains. The snapshot only
/// ever re-renders the visible screen, so a small ring is plenty.
const VT100_SCROLLBACK: usize = 0;

/// A single hosted child behind its own PTY.
pub struct PtyHost {
    /// PTY master — kept for `resize`.
    master: Box<dyn portable_pty::MasterPty + Send>,
    /// The spawned child; `Some` until [`PtyHost::kill`] consumes it.
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Input side of the PTY — keystrokes routed to the child land here.
    writer: Box<dyn Write + Send>,
    /// Off-screen terminal mirror, fed by the reader thread; the source
    /// for [`PtyHost::snapshot`].
    parser: Arc<Mutex<vt100::Parser>>,
    /// Reader thread handle (detached on drop).
    _reader: JoinHandle<()>,
}

impl PtyHost {
    /// Open a PTY of `rows`×`cols`, spawn `argv` inside it, and start the
    /// reader thread. `tab` tags every [`TuiEvent`] this host emits so
    /// the supervisor can route output to the right tab. `tx` is the
    /// shared supervisor channel.
    pub fn spawn(
        argv: &[String],
        rows: u16,
        cols: u16,
        tab: usize,
        tx: Sender<TuiEvent>,
    ) -> Result<PtyHost> {
        let (program, args) = argv
            .split_first()
            .context("PtyHost::spawn requires a non-empty argv")?;

        let pty_system = native_pty_system();
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = pty_system
            .openpty(size)
            .context("failed to open a pseudo-terminal")?;

        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        if let Ok(cwd) = std::env::current_dir() {
            cmd.cwd(cwd);
        }
        // Propagate the parent environment explicitly — a bare
        // CommandBuilder is not guaranteed to inherit it, and the hosted
        // `aida` needs PATH / HOME / AIDA_* to behave identically to a
        // direct invocation.
        for (key, val) in std::env::vars() {
            cmd.env(key, val);
        }
        if std::env::var_os("TERM").is_none() {
            cmd.env("TERM", "xterm-256color");
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("failed to spawn `{}` in a PTY", program))?;
        // The slave handle is not needed past spawn; dropping it lets the
        // child own the only slave reference so EOF propagates on exit.
        drop(pair.slave);

        let writer = pair
            .master
            .take_writer()
            .context("failed to acquire the PTY input writer")?;
        let reader = pair
            .master
            .try_clone_reader()
            .context("failed to clone the PTY output reader")?;

        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, VT100_SCROLLBACK)));
        let reader_handle = spawn_reader_thread(reader, Arc::clone(&parser), tab, tx);

        Ok(PtyHost {
            master: pair.master,
            child,
            writer,
            parser,
            _reader: reader_handle,
        })
    }

    /// Forward raw input bytes to the hosted child.
    pub fn write_input(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    /// Resize the PTY (and the off-screen mirror) to `rows`×`cols`.
    /// `portable-pty` delivers SIGWINCH to the child as part of `resize`.
    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to resize the PTY")?;
        if let Ok(mut parser) = self.parser.lock() {
            parser.screen_mut().set_size(rows, cols);
        }
        Ok(())
    }

    /// ANSI re-render of the child's current screen — used to repaint
    /// after a tab-switch or overlay-close without disturbing the child.
    pub fn snapshot(&self) -> Vec<u8> {
        match self.parser.lock() {
            Ok(parser) => parser.screen().contents_formatted(),
            Err(_) => Vec::new(),
        }
    }

    /// The child's cursor position as `(row, col)`, so the real cursor
    /// can be restored after the supervisor repaints the status strip.
    pub fn cursor_position(&self) -> (u16, u16) {
        match self.parser.lock() {
            Ok(parser) => parser.screen().cursor_position(),
            Err(_) => (0, 0),
        }
    }

    /// Terminate the hosted child. Best-effort — a child that already
    /// exited reports an error which is intentionally ignored.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for PtyHost {
    fn drop(&mut self) {
        // A dropped host must not leak a child process. STORY-5 will make
        // a graceful `prefix d` detach skip this (recording the session
        // for re-attach instead); today every teardown path kills.
        self.kill();
    }
}

/// Spawn the reader thread: drain PTY output, feed the `vt100` mirror,
/// and forward every chunk to the supervisor as a [`TuiEvent`].
fn spawn_reader_thread(
    mut reader: Box<dyn Read + Send>,
    parser: Arc<Mutex<vt100::Parser>>,
    tab: usize,
    tx: Sender<TuiEvent>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break, // EOF — child closed the PTY.
                Ok(n) => {
                    let chunk = buf[..n].to_vec();
                    if let Ok(mut p) = parser.lock() {
                        p.process(&chunk);
                    }
                    if tx.send(TuiEvent::PtyOutput { tab, bytes: chunk }).is_err() {
                        break; // Supervisor gone — stop draining.
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(TuiEvent::PtyExited { tab });
    })
}
