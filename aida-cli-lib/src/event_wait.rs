//! Shared event-stream wait primitive (TASK-1036).
//!
//! This module is the common substrate for **event-driven** supervision loops:
//! a loop blocks on [`wait_for_actionable`] until a real, focus-relevant drain
//! event lands in `.aida/events.jsonl`, an idle backstop elapses, or a live
//! drain it was following stops. It replaces the blind timer-poll that the
//! integrator `--watch` loop used.
//!
//! The two low-level readers — [`scan_new_actionable_event`] (the offset-tracking
//! "did any new actionable line appear?" scan) and [`event_stream_is_live`] (is a
//! live drain streaming events right now?) — were LIFTED verbatim out of
//! `advisor_watch.rs` so both the advisor watch loop (STORY-712) and the
//! integrator watch loop (TASK-1036) share one implementation rather than two
//! copies drifting apart. `advisor_watch` now calls the lifted versions; its
//! behavior is unchanged.
//!
//! The wake taxonomy is exactly [`crate::events::EventKind::is_actionable`] — the
//! same classifier `aida watch` and the advisor loop apply, so benign phase churn
//! (`PhaseEntered`, `RunStarted`) is absorbed here and never wakes the loop. On
//! launch a caller seeds the byte offset at the stream's CURRENT end so a stale
//! backlog from a prior drain never re-fires an old wake (the advisor_watch
//! precedent).
//!
//! trace:TASK-1036 trace:STORY-712 | ai:claude

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::events::{Event, EventKind};

/// How often [`wait_for_actionable`] re-checks the event stream while blocking.
/// Small enough to stay responsive (an event wakes the loop within a couple of
/// seconds) without busy-spinning; the idle backstop, not this, governs the
/// worst-case rescan cadence.
// trace:TASK-1036 | ai:claude
const WAIT_POLL_SECS: u64 = 2;

/// Why [`wait_for_actionable`] returned — so the caller can log the wake reason
/// and decide whether the rescan is event-driven, an idle backstop, or a
/// follow-the-drain that crashed.
// trace:TASK-1036 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WakeReason {
    /// A focus-relevant actionable event appeared — the payload is the first
    /// such event kind consumed this wait.
    Event(EventKind),
    /// The idle backstop elapsed with no actionable event (the documented timer
    /// fallback, exactly like `advisor_watch::plan_watch_tick`'s cadence path).
    IdleBackstop,
    /// A live drain we had been following stopped streaming events (crashed or
    /// exited) — surface it so the caller rescans rather than blocking on a dead
    /// stream (a crashed producer may have shelved a spec to triage).
    DrainCrashed,
}

/// Is a live drain actively streaming events into `.aida/events.jsonl`?
///
/// "Live" = the event file exists AND [`crate::drain_state::probe`] reports an
/// [`Active`](crate::drain_state::DrainStatus::Active) drain. A `Stale` (crashed
/// orchestrator) or `None` verdict means no one is writing events. Best-effort
/// and read-only.
///
/// LIFTED verbatim from `advisor_watch.rs` (TASK-991) into this shared module so
/// the advisor and integrator watch loops share one implementation.
// trace:TASK-1036 trace:TASK-991 | ai:claude
pub(crate) fn event_stream_is_live(project_root: &Path) -> bool {
    crate::events::events_path(project_root).exists()
        && matches!(
            crate::drain_state::probe(project_root),
            crate::drain_state::DrainStatus::Active(_)
        )
}

/// Scan `.aida/events.jsonl` for any NEW **actionable** event appended since byte
/// offset `*pos`, advancing `*pos` past every complete line consumed (so a given
/// line triggers at most one wake). Reuses
/// [`crate::events::EventKind::is_actionable`] — the exact classifier `aida watch`
/// applies; benign churn (`PhaseEntered`, `RunStarted`) is absorbed here and
/// never wakes the loop.
///
/// Best-effort and tolerant, mirroring `watch::drain_new_lines`: a missing or
/// unreadable file yields `false` and leaves `*pos` untouched (so a dead event
/// stream cleanly falls back to the cadence timer), a shrunk file re-reads from
/// the top, a partial trailing line is left for the next tick, and a malformed
/// line is skipped rather than erroring.
///
/// LIFTED verbatim from `advisor_watch.rs` (TASK-991) into this shared module.
// trace:TASK-1036 trace:TASK-991 | ai:claude
pub(crate) fn scan_new_actionable_event(path: &Path, pos: &mut u64) -> bool {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(*pos);
    if len < *pos {
        // File was truncated/rotated under us — re-read from the top.
        *pos = 0;
    }
    if file.seek(SeekFrom::Start(*pos)).is_err() {
        return false;
    }
    let mut reader = BufReader::new(file);
    let mut buf = String::new();
    let mut actionable = false;
    loop {
        buf.clear();
        let read = match reader.read_line(&mut buf) {
            Ok(n) => n,
            Err(_) => break,
        };
        if read == 0 {
            break;
        }
        if !buf.ends_with('\n') {
            // Partial line — leave `pos` so the next tick re-reads it whole.
            break;
        }
        *pos += read as u64;
        if let Ok(ev) = serde_json::from_str::<Event>(buf.trim_end_matches('\n')) {
            if ev.kind.is_actionable() {
                actionable = true;
            }
        }
    }
    actionable
}

/// Is this event in the current focus scope? A `None` filter means no scoping —
/// every event is in scope. With a filter, an event whose `spec` is in the
/// subtree set (case-insensitive) is in scope; a drain-LEVEL event with no spec
/// (e.g. `QueueDrained`) is always in scope (it is a milestone, never
/// out-of-scope).
// trace:TASK-1036 | ai:claude
fn event_in_focus(ev: &Event, focus_filter: Option<&HashSet<String>>) -> bool {
    match focus_filter {
        None => true,
        Some(subtree) => match &ev.spec {
            None => true,
            Some(spec) => subtree.iter().any(|s| s.eq_ignore_ascii_case(spec)),
        },
    }
}

/// Like [`scan_new_actionable_event`] but returns the FIRST focus-relevant
/// actionable event's kind (so the caller can log what woke it) and short-circuits
/// on it, instead of a bare bool over the whole new tail. Advances `*pos` past
/// every line it consumes — including benign churn and out-of-focus events it
/// skips — so a consumed line never re-fires. Same best-effort tolerance as the
/// bool scan.
// trace:TASK-1036 | ai:claude
fn scan_new_focus_actionable(
    path: &Path,
    pos: &mut u64,
    focus_filter: Option<&HashSet<String>>,
) -> Option<EventKind> {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().map(|m| m.len()).unwrap_or(*pos);
    if len < *pos {
        *pos = 0;
    }
    if file.seek(SeekFrom::Start(*pos)).is_err() {
        return None;
    }
    let mut reader = BufReader::new(file);
    let mut buf = String::new();
    loop {
        buf.clear();
        let read = match reader.read_line(&mut buf) {
            Ok(n) => n,
            Err(_) => break,
        };
        if read == 0 {
            break;
        }
        if !buf.ends_with('\n') {
            // Partial line — leave `pos` so the next tick re-reads it whole.
            break;
        }
        *pos += read as u64;
        if let Ok(ev) = serde_json::from_str::<Event>(buf.trim_end_matches('\n')) {
            if ev.kind.is_actionable() && event_in_focus(&ev, focus_filter) {
                return Some(ev.kind);
            }
        }
    }
    None
}

/// Block until a focus-relevant actionable event appears in `.aida/events.jsonl`,
/// the idle backstop elapses, or a live drain we were following stops.
///
/// `event_offset` is the caller-owned byte cursor into the event stream — seed it
/// at the stream's current END on launch so a stale backlog never re-fires an old
/// wake. It is advanced past every line consumed across calls.
///
/// `idle_backstop_secs` is the worst-case rescan cadence: with NO live event
/// stream this degenerates to a plain timer (the documented fallback, exactly
/// like `advisor_watch::plan_watch_tick`'s cadence path), so a project with no
/// event-emitting drain regresses to nothing-worse-than-the-old-timer behavior.
///
/// `focus_filter`, when `Some`, scopes the wake to events about specs in that
/// subtree (plus drain-level events with no spec); out-of-scope events are
/// consumed (the offset advances) but never wake the loop.
// trace:TASK-1036 | ai:claude
pub(crate) fn wait_for_actionable(
    project_root: &Path,
    event_offset: &mut u64,
    idle_backstop_secs: u64,
    focus_filter: Option<&HashSet<String>>,
) -> WakeReason {
    let events_path = crate::events::events_path(project_root);
    let deadline = Instant::now() + Duration::from_secs(idle_backstop_secs);
    // Track whether we have observed a live drain streaming events: only then is
    // a subsequent "not live" a CRASH worth surfacing. A loop that starts with no
    // live drain (the common integrator case — implementers are separate
    // processes) just rides the idle backstop, never reporting DrainCrashed.
    let mut saw_live = false;
    loop {
        if let Some(kind) = scan_new_focus_actionable(&events_path, event_offset, focus_filter) {
            return WakeReason::Event(kind);
        }
        if event_stream_is_live(project_root) {
            saw_live = true;
        } else if saw_live {
            return WakeReason::DrainCrashed;
        }
        if Instant::now() >= deadline {
            return WakeReason::IdleBackstop;
        }
        std::thread::sleep(Duration::from_secs(WAIT_POLL_SECS));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{Event, EventKind};

    fn write_lines(path: &Path, events: &[Event]) {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for e in events {
            writeln!(f, "{}", serde_json::to_string(e).unwrap()).unwrap();
        }
    }

    // ── scan_new_actionable_event (lifted from advisor_watch) ────────────────

    #[test]
    fn scan_detects_only_new_actionable_lines_and_advances_offset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        // Benign-only backlog: no wake.
        write_lines(
            &path,
            &[
                Event::new(Some("STORY-1".into()), "", EventKind::RunStarted),
                Event::new(
                    Some("STORY-1".into()),
                    "",
                    EventKind::PhaseEntered {
                        idx: 1,
                        slug: "implementer".into(),
                    },
                ),
            ],
        );
        let mut pos = 0u64;
        assert!(
            !scan_new_actionable_event(&path, &mut pos),
            "benign churn must not wake"
        );
        let after_benign = pos;
        assert!(after_benign > 0, "offset advanced past the benign lines");

        // Append one actionable event — the next scan wakes exactly once.
        write_lines(
            &path,
            &[Event::new(
                Some("STORY-1".into()),
                "",
                EventKind::PuntFiled {
                    spec: "STORY-1".into(),
                },
            )],
        );
        assert!(
            scan_new_actionable_event(&path, &mut pos),
            "a new actionable event wakes"
        );
        assert!(
            pos > after_benign,
            "offset advanced past the actionable line"
        );

        // Already consumed — no re-fire on the same line.
        assert!(
            !scan_new_actionable_event(&path, &mut pos),
            "consumed lines never re-fire"
        );
    }

    #[test]
    fn scan_is_false_when_no_event_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl"); // never created
        let mut pos = 0u64;
        assert!(!scan_new_actionable_event(&path, &mut pos));
        assert_eq!(pos, 0, "a missing file leaves the offset untouched");
    }

    #[test]
    fn event_stream_not_live_without_drain_state() {
        // No drain-state file + no events file → not live, so the timer governs.
        let dir = tempfile::tempdir().unwrap();
        assert!(!event_stream_is_live(dir.path()));
    }

    // ── wait_for_actionable (TASK-1036) ──────────────────────────────────────

    // The event stream lives under <root>/.aida/events.jsonl; a unit fixture
    // seeds that path directly so no real drain / network is involved.
    fn seed_events(root: &Path, events: &[Event]) -> std::path::PathBuf {
        let path = crate::events::events_path(root);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_lines(&path, events);
        path
    }

    #[test]
    fn wait_for_actionable_wakes_on_phase_done_pr_not_phase_churn() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Benign churn only → with a 0s backstop the wait returns IdleBackstop,
        // proving phase churn does NOT wake (no live stream is running either).
        seed_events(
            root,
            &[
                Event::new(Some("STORY-1".into()), "", EventKind::RunStarted),
                Event::new(
                    Some("STORY-1".into()),
                    "",
                    EventKind::PhaseEntered {
                        idx: 2,
                        slug: "ci".into(),
                    },
                ),
            ],
        );
        let mut offset = 0u64;
        assert_eq!(
            wait_for_actionable(root, &mut offset, 0, None),
            WakeReason::IdleBackstop,
            "benign phase churn must not wake the loop"
        );
        // Now an actionable PhaseDonePr lands → the next wait wakes on it.
        seed_events(
            root,
            &[Event::new(
                Some("STORY-1".into()),
                "",
                EventKind::PhaseDonePr { pr: 1207 },
            )],
        );
        assert_eq!(
            wait_for_actionable(root, &mut offset, 30, None),
            WakeReason::Event(EventKind::PhaseDonePr { pr: 1207 }),
            "an open-PR event wakes the loop"
        );
    }

    #[test]
    fn wait_for_actionable_falls_back_to_idle_timer_without_live_stream() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // No events file at all, no live drain → the idle backstop governs.
        let mut offset = 0u64;
        assert_eq!(
            wait_for_actionable(root, &mut offset, 0, None),
            WakeReason::IdleBackstop,
            "with no live event stream the wait degenerates to the idle timer"
        );
    }

    #[test]
    fn wait_for_actionable_focus_filter_ignores_out_of_scope_events() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let focus: HashSet<String> = ["STORY-1".to_string()].into_iter().collect();
        // An actionable event about an OUT-OF-SCOPE spec must not wake a focused
        // loop — the 0s backstop returns IdleBackstop instead.
        seed_events(
            root,
            &[Event::new(
                Some("STORY-99".into()),
                "",
                EventKind::PhaseDonePr { pr: 9 },
            )],
        );
        let mut offset = 0u64;
        assert_eq!(
            wait_for_actionable(root, &mut offset, 0, Some(&focus)),
            WakeReason::IdleBackstop,
            "an out-of-scope event must not wake a focused loop"
        );
        // An IN-SCOPE actionable event does wake it.
        seed_events(
            root,
            &[Event::new(
                Some("STORY-1".into()),
                "",
                EventKind::CiTerminal { green: true },
            )],
        );
        assert_eq!(
            wait_for_actionable(root, &mut offset, 30, Some(&focus)),
            WakeReason::Event(EventKind::CiTerminal { green: true }),
            "an in-scope event wakes the focused loop"
        );
    }
}
