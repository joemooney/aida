//! Shared statusline contract (TASK-1479).
//!
//! `aida statusline` renders a stable, AIDA-only segment (role/session,
//! active spec, queue depth, worktree divergence, cache freshness, and the
//! rest of `statusline_cmd::handle_statusline_command`'s output) with no
//! network and no full store load — it is a hot path, invoked on every
//! prompt render / footer refresh. That segment is unchanged by this module
//! and stays the default: plain `aida statusline` never reads stdin.
//!
//! Some clients (Claude Code, Antigravity/Agy) additionally pipe a live
//! agent-state JSON payload to the statusLine command on stdin — model
//! name, context-window usage, current activity, VCS branch/dirty state.
//! Those fields are CLIENT-OWNED: they vary per client, arrive per-render,
//! and are never durable AIDA state. This module defines the shared shape
//! those live fields are normalized into ([`ClientLiveFields`]) and the one
//! formatter every client adapter calls to combine them with the AIDA
//! segment ([`format_combined`]). Per-client stdin parsing lives in the thin
//! adapters `statusline_claude_adapter` and `statusline_agy_adapter` — they
//! only produce a `ClientLiveFields`; they never re-render the AIDA segment
//! or duplicate its logic.
//!
//! ## Contract
//!
//! | Category | Fields | Source | Cadence |
//! |---|---|---|---|
//! | Stable AIDA fields | role/session, active spec (`@SPEC`), queue depth, worktree, cache freshness | `.aida/cache.db`, orphan-store queue YAML, session lease dir | Local, cache-only; unchanged by this module |
//! | Client live fields | model, context usage, activity, VCS branch/dirty | Client's own stdin JSON payload | Per-render, client-supplied, best-effort |
//!
//! Client live fields are all `Option` — a client that omits a field (or an
//! older client version that doesn't send it yet) degrades silently; a
//! client that sends fields this contract doesn't know about is ignored,
//! never an error. No field here is ever treated as durable AIDA state: it
//! is redrawn from the next payload on the next render and never written to
//! `.aida/` or the store.
//!
//! ## Privacy
//!
//! Adapters parse their client's stdin JSON directly into
//! [`ClientLiveFields`] and never retain, log, or echo the raw payload — a
//! client payload can carry `cwd`, transcript paths, or other
//! session-identifying data that has no business in a status line or a log
//! file. A parse failure degrades to `ClientLiveFields::default()` (i.e. no
//! live segment), never an error message that echoes the input.
// trace:TASK-1479 | ai:claude

use std::io::IsTerminal;

/// Client-supplied live fields, normalized to one shape regardless of which
/// client's JSON produced them. Every field is best-effort and optional —
/// see the module contract table above.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClientLiveFields {
    /// Short model label (e.g. `sonnet-4.5`, `gpt-5-codex`). Adapters prefer
    /// a display name over a raw model id when the client sends both.
    pub model: Option<String>,
    /// Percent of the context window REMAINING (0-100, clamped), not used —
    /// chosen so a healthy session reads as a big reassuring number and a
    /// nearly-full context reads as a small alarming one, matching how
    /// Claude Code's own UI frames context budget.
    pub context_remaining_pct: Option<u8>,
    /// Free-form current-activity label the client reports (e.g.
    /// `editing`, `thinking`, `running`). Truncated at render time; not
    /// validated against a fixed enum since clients differ.
    pub activity: Option<String>,
    /// VCS branch name, when the client reports one directly (most clients
    /// don't — AIDA's own segment already reflects the session's git
    /// state via the worktree/lease fields, so this is only populated when
    /// the client payload is the more current source, e.g. Agy running
    /// outside an AIDA session lease).
    pub vcs_branch: Option<String>,
    /// Whether the client reports a dirty working tree.
    pub vcs_dirty: Option<bool>,
}

impl ClientLiveFields {
    /// True when every field is `None` — nothing to render.
    pub fn is_empty(&self) -> bool {
        self.model.is_none()
            && self.context_remaining_pct.is_none()
            && self.activity.is_none()
            && self.vcs_branch.is_none()
            && self.vcs_dirty.is_none()
    }
}

/// Below this terminal width, the live segment sheds fields (lowest
/// priority first) before the combined line gets truncated outright.
pub const NARROW_TERMINAL_COLUMNS: usize = 60;

/// Terminal width for statusline layout. Honors `$COLUMNS` (exported by
/// interactive shells and some statusline hosts), clamps to a sane floor,
/// and falls back to 80 when unknown — mirrors `picker_terminal_width`'s
/// convention (`lib.rs`) so the whole CLI agrees on one env-var contract.
// trace:TASK-1479 | ai:claude
pub(crate) fn statusline_terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.trim().parse::<usize>().ok())
        .filter(|w| *w >= 20)
        .unwrap_or(80)
}

/// Render the client-live segment alone, e.g. `sonnet-4.5 · ctx:62% ·
/// editing · main*`. Returns `None` when every field is absent — a client
/// that sent nothing usable renders no segment rather than an empty one.
/// `narrow` sheds fields in priority order (activity, then VCS, then
/// context) so the highest-signal field (model) survives longest on a
/// narrow terminal.
// trace:TASK-1479 | ai:claude
pub fn format_live_segment(live: &ClientLiveFields, narrow: bool) -> Option<String> {
    if live.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(model) = &live.model {
        if !model.trim().is_empty() {
            parts.push(model.trim().to_string());
        }
    }
    if let Some(pct) = live.context_remaining_pct {
        parts.push(format!("ctx:{}%", pct.min(100)));
    }
    if !narrow {
        if let Some(activity) = &live.activity {
            if !activity.trim().is_empty() {
                parts.push(activity.trim().to_string());
            }
        }
    }
    if let Some(branch) = &live.vcs_branch {
        if !branch.trim().is_empty() {
            let dirty_mark = if live.vcs_dirty == Some(true) {
                "*"
            } else {
                ""
            };
            parts.push(format!("{}{}", branch.trim(), dirty_mark));
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

/// The one shared formatter every client adapter calls: combine the
/// (already-rendered) stable AIDA segment with an optional client-live
/// segment. `columns` is the terminal width the line must fit; pass
/// [`statusline_terminal_width`] for real renders and a fixed value in
/// tests. Readable fallbacks:
///   - `live` is `None`/empty → the AIDA segment alone (today's behavior,
///     unchanged).
///   - the AIDA segment is empty (nothing resolved, e.g. outside a project)
///     → the live segment alone, rather than a stray leading separator.
///   - `columns` is at or above [`NARROW_TERMINAL_COLUMNS`] (the common
///     case — 80-column terminals included) → the combined line is
///     returned in full, exactly like the existing AIDA-only segment
///     already does today (it performs no width truncation of its own
///     either). Width awareness is a NARROW-terminal concern, not a
///     routine 80-column cap — a live segment making an ordinary
///     terminal's line a bit longer than 80 chars is the whole point of
///     this feature, not a defect to truncate away.
///   - `columns` is below [`NARROW_TERMINAL_COLUMNS`] → the live segment
///     sheds fields first (`format_live_segment(.., narrow: true)`); if
///     the result still doesn't fit, falls back to the AIDA segment alone,
///     then hard-truncates as the last resort so a pathologically narrow
///     host never gets an unbounded line.
///
/// The AIDA segment may carry ANSI color escapes (the default installed
/// command runs with `--color=always`, since the client pipes it through a
/// non-TTY). A raw character-count truncation would happily slice through
/// the middle of an escape sequence and corrupt the terminal's color state
/// for the rest of the session, which is worse than an over-wide line — so
/// the hard-truncation fallback below (narrow terminals only) engages only
/// when NEITHER segment contains an ESC byte. The live segment itself is
/// always plain text (no color), so field-shedding is always safe
/// regardless.
// trace:TASK-1479 | ai:claude
pub fn format_combined(
    aida_segment: &str,
    live: Option<&ClientLiveFields>,
    columns: usize,
) -> String {
    let narrow = columns < NARROW_TERMINAL_COLUMNS;
    let live_segment = live.and_then(|l| format_live_segment(l, narrow));
    let combined = match (&live_segment, aida_segment.is_empty()) {
        (Some(l), false) => format!("{} · {}", l, aida_segment),
        (Some(l), true) => l.clone(),
        (None, _) => aida_segment.to_string(),
    };
    // Normal/wide terminals never truncate — see the doc comment above.
    // This is the common case (COLUMNS unset/unknown falls back to 80),
    // and it must stay the common case: routinely dropping the live
    // segment on an ordinary 80-column terminal would defeat this
    // formatter's whole purpose.
    if !narrow {
        return combined;
    }
    if combined.chars().count() <= columns || combined.contains('\u{1b}') {
        return combined;
    }
    // Still too wide even after shedding low-priority live fields (or no
    // live fields to shed): fall back to the AIDA segment alone, then hard
    // -truncate as the last resort so a pathologically narrow host never
    // gets an unbounded line. Only reached for ANSI-free (plain-text)
    // lines — see the safety note above.
    if !aida_segment.is_empty() && aida_segment.chars().count() <= columns {
        return aida_segment.to_string();
    }
    let mut out: String = combined
        .chars()
        .take(columns.saturating_sub(1).max(1))
        .collect();
    out.push('…');
    out
}

/// Hard cap on how much stdin `read_stdin_payload` will ever buffer.
/// Generous for any real client statusline payload (a few KB of JSON);
/// small enough to bound memory if something pipes far more than that.
/// An oversize payload degrades to no live segment, same as a timeout.
pub const MAX_STDIN_PAYLOAD_BYTES: usize = 256 * 1024;

/// How long `read_stdin_payload` waits for a client's stdin payload before
/// giving up. A command-backed statusline is a hot path on every prompt
/// render — this bounds the wait so an open pipe/FIFO whose write end
/// never sends EOF (verified repro: `mkfifo`, hold the write end open,
/// read with `--client claude`) cannot wedge it. 200ms is generous above
/// realistic payload-write latency (a client writing a few KB of JSON)
/// while staying well under human-perceptible prompt lag.
// trace:TASK-1479 | ai:claude
const STDIN_READ_DEADLINE: std::time::Duration = std::time::Duration::from_millis(200);

/// Read the client's live-state JSON from stdin, bounded in both time and
/// size. Returns `None` — degrading callers to the AIDA-only segment,
/// never a hang — in every one of these cases:
///   - stdin is a TTY (a human running the command directly has no
///     payload to pipe; skipped before starting a reader thread at all);
///   - the read doesn't finish within [`STDIN_READ_DEADLINE`] (an open
///     pipe/FIFO that never sends EOF — this is the actual guarantee: a
///     BOUNDED wait, not "never blocks");
///   - the payload exceeds [`MAX_STDIN_PAYLOAD_BYTES`];
///   - the underlying read errors, or the bytes aren't valid UTF-8;
///   - the payload is empty/whitespace-only.
///
/// Implementation: the blocking read runs on a background thread; the
/// caller waits on a channel with `recv_timeout`. On timeout that thread
/// is deliberately NOT joined — a writer that never closes its end would
/// otherwise keep it blocked forever, which is exactly the hang this
/// guards against. One thread outliving a short-lived CLI process (it dies
/// with the process either way) is the correct trade over wedging the
/// prompt. The raw payload is handed to the caller's parser and is never
/// logged or echoed here (privacy contract in the module docs above).
// trace:TASK-1479 | ai:claude
pub fn read_stdin_payload() -> Option<String> {
    if std::io::stdin().is_terminal() {
        return None;
    }
    read_bounded(
        std::io::stdin(),
        MAX_STDIN_PAYLOAD_BYTES,
        STDIN_READ_DEADLINE,
    )
}

/// Read up to `max_bytes` from `reader` on a background thread, waiting at
/// most `deadline` on the calling thread. `reader` is generic (rather than
/// hardcoded to `Stdin`) so the deadline/cap behavior itself is unit
/// -testable against a real blocking `Read` (an OS pipe with an open
/// write end) without needing to touch the test process's own stdin fd.
// trace:TASK-1479 | ai:claude
fn read_bounded<R: std::io::Read + Send + 'static>(
    reader: R,
    max_bytes: usize,
    deadline: std::time::Duration,
) -> Option<String> {
    use std::io::Read;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // Cap the read at `max_bytes + 1` so an oversize payload is
        // detected (buf.len() > max_bytes below) without ever buffering
        // an unbounded amount, regardless of how much more the writer
        // sends after that — we stop reading once we've seen enough to
        // know it's too big.
        let mut buf = Vec::new();
        let result = reader.take((max_bytes as u64) + 1).read_to_end(&mut buf);
        // The receiver may already have timed out and dropped its end;
        // a failed send just means nobody's listening anymore, which is
        // fine — this thread's job is done either way.
        let _ = tx.send(result.map(|_| buf));
    });
    match rx.recv_timeout(deadline) {
        Ok(Ok(buf)) if buf.len() <= max_bytes => {
            String::from_utf8(buf).ok().filter(|s| !s.trim().is_empty())
        }
        // Oversize, a read error, or a timeout all degrade the same way.
        Ok(Ok(_)) | Ok(Err(_)) | Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_live_fields_render_nothing() {
        assert!(format_live_segment(&ClientLiveFields::default(), false).is_none());
    }

    #[test]
    fn full_live_fields_render_in_priority_order() {
        let live = ClientLiveFields {
            model: Some("sonnet-4.5".into()),
            context_remaining_pct: Some(62),
            activity: Some("editing".into()),
            vcs_branch: Some("main".into()),
            vcs_dirty: Some(true),
        };
        assert_eq!(
            format_live_segment(&live, false).unwrap(),
            "sonnet-4.5 · ctx:62% · editing · main*"
        );
    }

    #[test]
    fn narrow_mode_sheds_activity_first() {
        let live = ClientLiveFields {
            model: Some("sonnet-4.5".into()),
            context_remaining_pct: Some(62),
            activity: Some("editing".into()),
            vcs_branch: None,
            vcs_dirty: None,
        };
        assert_eq!(
            format_live_segment(&live, true).unwrap(),
            "sonnet-4.5 · ctx:62%"
        );
    }

    #[test]
    fn context_pct_is_clamped_to_100() {
        let live = ClientLiveFields {
            context_remaining_pct: Some(255),
            ..Default::default()
        };
        assert_eq!(format_live_segment(&live, false).unwrap(), "ctx:100%");
    }

    #[test]
    fn clean_branch_has_no_dirty_marker() {
        let live = ClientLiveFields {
            vcs_branch: Some("main".into()),
            vcs_dirty: Some(false),
            ..Default::default()
        };
        assert_eq!(format_live_segment(&live, false).unwrap(), "main");
    }

    #[test]
    fn combined_with_no_live_fields_is_aida_segment_unchanged() {
        assert_eq!(
            format_combined("αιδα · aida · role:implementer", None, 80),
            "αιδα · aida · role:implementer"
        );
        let empty = ClientLiveFields::default();
        assert_eq!(
            format_combined("αιδα · aida · role:implementer", Some(&empty), 80),
            "αιδα · aida · role:implementer"
        );
    }

    #[test]
    fn combined_prefixes_live_segment_before_aida_segment() {
        let live = ClientLiveFields {
            model: Some("sonnet-4.5".into()),
            ..Default::default()
        };
        assert_eq!(
            format_combined("aida · role:implementer", Some(&live), 80),
            "sonnet-4.5 · aida · role:implementer"
        );
    }

    #[test]
    fn combined_with_empty_aida_segment_is_live_segment_alone() {
        let live = ClientLiveFields {
            model: Some("sonnet-4.5".into()),
            ..Default::default()
        };
        assert_eq!(format_combined("", Some(&live), 80), "sonnet-4.5");
    }

    #[test]
    fn narrow_terminal_sheds_fields_before_hard_truncating() {
        let live = ClientLiveFields {
            model: Some("sonnet-4.5".into()),
            context_remaining_pct: Some(62),
            activity: Some("editing".into()),
            vcs_branch: None,
            vcs_dirty: None,
        };
        // 50 columns: below NARROW_TERMINAL_COLUMNS, so activity sheds and
        // both segments still fit — no hard truncation needed.
        let out = format_combined("aida · role:implementer", Some(&live), 50);
        assert_eq!(out, "sonnet-4.5 · ctx:62% · aida · role:implementer");
        assert!(out.chars().count() <= 50 || out.ends_with('…'));
    }

    #[test]
    fn pathologically_narrow_width_hard_truncates_with_ellipsis() {
        let live = ClientLiveFields {
            model: Some("a-very-long-model-name-indeed".into()),
            ..Default::default()
        };
        let out = format_combined(
            "aida · project · role:implementer · @TASK-1479-very-long-scope-name",
            Some(&live),
            15,
        );
        assert!(out.chars().count() <= 15, "{out:?}");
        assert!(out.ends_with('…'), "{out:?}");
    }

    #[test]
    fn normal_80_column_terminal_never_drops_the_live_segment() {
        // Regression: an earlier version of this formatter truncated to
        // AIDA-only whenever the combined line exceeded `columns`, even on
        // an ordinary (non-narrow) terminal — so a real Claude Code
        // payload (model + context) on the default 80-column fallback
        // silently lost its live segment the moment the AIDA segment was
        // already reasonably long. 80 columns is NOT narrow
        // (NARROW_TERMINAL_COLUMNS is 60); the combined line must render
        // in full here regardless of length. trace:TASK-1479 | ai:claude
        let aida_segment = "αιδα · wt-task-1479 · role:implementer (default) · @TASK-1479";
        let live = ClientLiveFields {
            model: Some("Sonnet 4.5".into()),
            context_remaining_pct: Some(62),
            ..Default::default()
        };
        let out = format_combined(aida_segment, Some(&live), 80);
        assert_eq!(out, format!("Sonnet 4.5 · ctx:62% · {aida_segment}"));
        assert!(
            out.chars().count() > 80,
            "this is exactly the point: {out:?}"
        );
    }

    #[test]
    fn ansi_colored_aida_segment_is_never_hard_truncated() {
        // Simulates `--color=always` output: an ESC byte anywhere in either
        // segment must disable the hard-truncation fallback, even when the
        // line nominally exceeds `columns` — slicing mid-escape would
        // corrupt the terminal's color state, which is worse than a long
        // line. trace:TASK-1479 | ai:claude
        let colored_aida = "\u{1b}[32mproject\u{1b}[0m · \u{1b}[33mrole:implementer\u{1b}[0m";
        let out = format_combined(colored_aida, None, 10);
        assert_eq!(
            out, colored_aida,
            "colored segment must pass through unmodified"
        );

        let live = ClientLiveFields {
            model: Some("sonnet-4.5".into()),
            ..Default::default()
        };
        let out = format_combined(colored_aida, Some(&live), 10);
        assert!(out.starts_with("sonnet-4.5 · "), "{out:?}");
        assert!(out.contains(colored_aida), "{out:?}");
    }

    #[test]
    fn stdin_read_smoke_test_does_not_panic() {
        // `read_stdin_payload()` itself reads the real process stdin, which
        // a unit test can't redirect — this just pins "compiles, returns an
        // Option, doesn't panic" under `cargo test`'s (non-TTY) stdin. The
        // real bounded-read/deadline/cap guarantee is exercised directly
        // against `read_bounded` below, against a genuine blocking `Read`
        // (an OS pipe), which is what actually reproduces the hang bug this
        // was written to fix. trace:TASK-1479 | ai:claude
        let _ = read_stdin_payload();
    }

    /// Regression for the reported hang: an open pipe/FIFO whose write end
    /// never sends EOF must not block `read_stdin_payload` forever. Uses a
    /// real OS pipe (not a TTY, not a closed reader) with the write end
    /// kept alive and never written to — the exact shape of the reviewer's
    /// repro (`mkfifo`; hold the write end open; read with `--client
    /// claude`). Asserts the bounded read returns `None` within the
    /// deadline plus a generous margin, not "eventually" or "never".
    // trace:TASK-1479 | ai:claude
    #[cfg(unix)]
    #[test]
    fn read_bounded_times_out_on_an_open_pipe_with_no_data() {
        let (reader, writer) = std::io::pipe().expect("create pipe");
        // Keep the write end alive for the whole test (never closed, never
        // written to) — this is what makes the read block on a real OS
        // pipe instead of hitting EOF immediately.
        let _keep_writer_open = writer;

        let deadline = std::time::Duration::from_millis(100);
        let start = std::time::Instant::now();
        let result = read_bounded(reader, MAX_STDIN_PAYLOAD_BYTES, deadline);
        let elapsed = start.elapsed();

        assert_eq!(
            result, None,
            "an open, silent pipe must degrade to no live segment"
        );
        // Generous margin over the deadline for scheduler jitter — this
        // must be well under "never returns", which is the bug.
        assert!(
            elapsed < deadline * 5,
            "read_bounded took {elapsed:?}, expected to return near the {deadline:?} deadline"
        );
    }

    #[cfg(unix)]
    #[test]
    fn read_bounded_returns_data_promptly_when_the_writer_closes() {
        let (reader, mut writer) = std::io::pipe().expect("create pipe");
        std::io::Write::write_all(&mut writer, b"hello from a client").unwrap();
        drop(writer); // EOF
        let result = read_bounded(
            reader,
            MAX_STDIN_PAYLOAD_BYTES,
            std::time::Duration::from_millis(200),
        );
        assert_eq!(result.as_deref(), Some("hello from a client"));
    }

    #[cfg(unix)]
    #[test]
    fn read_bounded_degrades_on_oversize_payload() {
        let (reader, mut writer) = std::io::pipe().expect("create pipe");
        let max_bytes = 16;
        let oversized = "x".repeat(max_bytes + 1);
        std::io::Write::write_all(&mut writer, oversized.as_bytes()).unwrap();
        drop(writer);
        let result = read_bounded(reader, max_bytes, std::time::Duration::from_millis(200));
        assert_eq!(
            result, None,
            "a payload over the cap must degrade to no live segment"
        );
    }

    #[cfg(unix)]
    #[test]
    fn read_bounded_accepts_a_payload_exactly_at_the_cap() {
        let (reader, mut writer) = std::io::pipe().expect("create pipe");
        let max_bytes = 16;
        let exact = "x".repeat(max_bytes);
        std::io::Write::write_all(&mut writer, exact.as_bytes()).unwrap();
        drop(writer);
        let result = read_bounded(reader, max_bytes, std::time::Duration::from_millis(200));
        assert_eq!(result.as_deref(), Some(exact.as_str()));
    }
}
