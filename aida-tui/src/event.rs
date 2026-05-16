//! The single event type the supervisor loop selects over.
//!
//! The TUI is sync + thread-per-source (plan decision: "Sync + threads,
//! not tokio"). One `mpsc` channel carries every source so the event
//! loop is a plain `for event in rx` — no `select!`, no runtime.
//!
//! trace:STORY-132 | ai:claude

/// An event delivered to the supervisor loop. Producers:
///   * the input thread — one [`TuiEvent::Input`] per crossterm event;
///   * each PTY host's reader thread — [`TuiEvent::PtyOutput`] chunks and
///     a final [`TuiEvent::PtyExited`] when the child closes the PTY;
///   * the overlay's background refresh thread — one [`TuiEvent::Overlay
///     Refresh`] when the `gh`-backed `aida status --json` lands;
///   * the dashboard's background refresh thread — one [`TuiEvent::Dash
///     boardRefresh`] when a mission-control snapshot lands (STORY-241);
///   * the timer ticker — one [`TuiEvent::Tick`] every ~2s, driving the
///     status-strip hint rotation (BUG-109) and the dashboard's
///     cache-staleness poll (STORY-241).
pub enum TuiEvent {
    /// A keystroke / resize / paste from the real terminal.
    Input(crossterm::event::Event),
    /// Raw bytes a hosted child wrote to its PTY. `tab` is the owning
    /// tab's id (see `TabManager`); the loop blits it to stdout only
    /// when that tab is focused.
    PtyOutput { tab: usize, bytes: Vec<u8> },
    /// A hosted child closed its PTY (process exited or detached).
    PtyExited { tab: usize },
    /// The overlay's background `aida status --json` (CI-inclusive)
    /// refresh completed — repaint the overlay if it is still open
    /// (STORY-133). Boxed to keep the enum small. trace:STORY-133
    OverlayRefresh(Box<crate::overlay::OverlayModel>),
    /// A background mission-control refresh landed — repaint the
    /// dashboard if it is still showing and the snapshot's role still
    /// matches the active one. Boxed to keep the enum small.
    /// trace:STORY-241 | ai:claude
    DashboardRefresh(Box<crate::dashboard::DashboardModel>),
    /// A ~2s timer tick — advances the status strip's rotating
    /// discovery hint (BUG-109) and triggers the dashboard's
    /// cache-staleness poll (STORY-241). trace:BUG-109 STORY-241
    Tick,
}
