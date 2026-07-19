//! `aida watch` — the streaming event classifier (STORY-712 slice 2).
//!
//! # Why this exists
//!
//! Slice 1 ([`crate::events`]) gave the drain an append-only event stream at
//! `.aida/events.jsonl` — one structured line per state change. This slice adds
//! the **cheap consumer**: a `tail -f`-style follow over that stream that
//! classifies every line in code via [`crate::events::EventKind::is_actionable`]
//! and prints **one wake line only on an actionable verb**, staying silent on
//! the benign majority (phase churn, run-started bookkeeping).
//!
//! The intended consumer is the harness `Monitor` tool over
//! `aida watch --emit-wakes`: it blocks on this streaming command and turns each
//! emitted wake line into a session event with **zero token cost while silent**.
//! The supervising LLM session therefore burns nothing until something
//! actionable actually happens.
//!
//! This is a purely **additive, read-only** command — a new reader of slice-1's
//! events; it changes no control flow in the drain.
//!
//! # Liveness
//!
//! A crashed drain would otherwise leave a `Monitor` blocked forever on a stream
//! that never emits again. So on every follow tick we poll
//! [`crate::drain_state::probe`]; a [`Stale`](crate::drain_state::DrainStatus::Stale)
//! verdict (the recorded orchestrator PID is dead) emits one `WAKE drain-crashed`
//! line and exits, so a supervisor blocked on this command learns the drain died
//! instead of waiting indefinitely.
//!
//! trace:TASK-990 trace:STORY-712 | ai:claude

use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::Result;

use crate::drain_state::{self, DrainStatus};
use crate::events::{self, Event, EventKind};

/// Follow-poll interval — short enough to feel live, not so short it busy-spins.
/// Mirrors `headless_tail::FOLLOW_POLL`.
// trace:TASK-990 | ai:claude
const FOLLOW_POLL: Duration = Duration::from_millis(250);

/// Caller-supplied options for `aida watch`.
#[derive(Debug, Clone, Default)]
pub struct WatchOpts {
    /// Also print the benign (non-actionable) events as an indented debug feed.
    /// Default (`false`) absorbs them silently — the whole point of the lever.
    pub all: bool,
    /// Drain the current backlog, classify it, and exit (cron / test mode).
    /// Without it, the command follows the stream like `tail -f`.
    pub once: bool,
}

/// Top-level entry point — invoked from the early command dispatch in `main.rs`
/// (read-only; needs no storage handle).
// trace:TASK-990 | ai:claude
pub fn handle_watch(project_root: &Path, opts: &WatchOpts) -> Result<()> {
    let path = events::events_path(project_root);
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    // Drain whatever is already in the backlog first — this is the whole job in
    // `--once` mode, and the catch-up pass before following otherwise.
    let mut pos: u64 = 0;
    drain_new_lines(&path, &mut pos, opts.all, &mut out)?;
    if opts.once {
        out.flush()?;
        return Ok(());
    }

    // Follow loop: on each tick, first check drain liveness (so a crashed
    // orchestrator wakes the supervisor rather than blocking it forever), then
    // emit any newly-appended events.
    loop {
        if let Some(line) = stale_wake_line(&drain_state::probe(project_root)) {
            writeln!(out, "{}", line)?;
            out.flush()?;
            return Ok(());
        }
        drain_new_lines(&path, &mut pos, opts.all, &mut out)?;
        std::thread::sleep(FOLLOW_POLL);
    }
}

/// Read every *complete* line appended since byte offset `pos`, classify it, and
/// write any resulting output line. Advances `pos` past each consumed line.
///
/// Modeled on `headless_tail::stream_log_inner`'s reopen+seek pattern: a missing
/// file is a no-op (the drain may not have emitted yet), a shrunk file resets to
/// the top, and a partial trailing line is left for the next tick. Each emitted
/// line is flushed immediately so a `Monitor` consumer gets the wake without
/// buffering latency. `.aida/events.jsonl` is append-only, so plain follow is
/// sufficient.
// trace:TASK-990 | ai:claude
fn drain_new_lines(
    path: &Path,
    pos: &mut u64,
    all: bool,
    out: &mut impl Write,
) -> std::io::Result<()> {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        // No events file yet — nothing to classify. Not an error.
        Err(_) => return Ok(()),
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(*pos);
    if len < *pos {
        // File was truncated/rotated under us — re-read from the top.
        *pos = 0;
    }
    file.seek(SeekFrom::Start(*pos))?;
    let mut reader = BufReader::new(file);
    let mut buf = String::new();
    loop {
        buf.clear();
        let read = reader.read_line(&mut buf)?;
        if read == 0 {
            break;
        }
        if !buf.ends_with('\n') {
            // Partial line — leave `pos` so the next tick re-reads it whole.
            break;
        }
        *pos += read as u64;
        if let Some(line) = render_one(buf.trim_end_matches('\n'), all) {
            writeln!(out, "{}", line)?;
            out.flush()?;
        }
    }
    Ok(())
}

/// Parse one raw JSONL line into an [`Event`] and render its output line, if any.
/// A blank or malformed line is silently skipped (tolerant follow, like
/// `headless_tail`), never a hard error.
fn render_one(line: &str, all: bool) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let ev: Event = serde_json::from_str(trimmed).ok()?;
    render(&ev, all)
}

/// Render the terminal line for one event. Returns `Some(line)` for an
/// actionable event (always — a wake), or for a benign event only when `all`;
/// `None` otherwise (the benign-absorption default).
///
/// Format: `WAKE <verb> <spec> — <hint>` for wakes (the `<spec>` is omitted for
/// drain-level events that carry none); benign `--all` lines use a blank
/// indent prefix instead of `WAKE` so the two are trivially distinguishable.
fn render(ev: &Event, all: bool) -> Option<String> {
    let actionable = ev.kind.is_actionable();
    if !actionable && !all {
        return None;
    }
    let (verb, hint) = describe(&ev.kind);
    let mut line = String::from(if actionable { "WAKE " } else { "     " });
    line.push_str(verb);
    if let Some(spec) = ev.spec.as_deref() {
        line.push(' ');
        line.push_str(spec);
    }
    line.push_str(" — ");
    line.push_str(&hint);
    Some(line)
}

/// Map an event kind to its kebab-case verb and a human hint. Pure — the same
/// exhaustive-match discipline as [`EventKind::is_actionable`], so a future
/// variant is a compile error here until it is given a label.
// trace:TASK-990 | ai:claude
fn describe(ek: &EventKind) -> (&'static str, String) {
    match ek {
        EventKind::RunStarted => ("run-started", "orchestration run started".to_string()),
        EventKind::PhaseEntered { idx, slug } => {
            ("phase-entered", format!("phase {} ({})", idx, slug))
        }
        EventKind::CiTerminal { green } => (
            "ci-terminal",
            if *green { "CI green" } else { "CI red" }.to_string(),
        ),
        EventKind::PhaseDonePr { pr } => ("phase-done-pr", format!("PR #{} open", pr)),
        EventKind::SpecShelved { phase, kind } => {
            ("spec-shelved", format!("shelved at {} ({})", phase, kind))
        }
        EventKind::PuntFiled { .. } => {
            ("punt-filed", "design-fork at .aida/punts.jsonl".to_string())
        }
        EventKind::AdvisorEscalated { reason } => (
            "advisor-escalated",
            format!("escalated to human: {}", reason),
        ),
        EventKind::PrMerged { pr } => ("pr-merged", format!("PR #{} merged", pr)),
        EventKind::QueueDrained { shipped, shelved } => (
            "queue-drained",
            format!("drain done — {} shipped, {} shelved", shipped, shelved),
        ),
        EventKind::UnreadMail => (
            "unread-mail",
            "supervisor mailbox has unread mail".to_string(),
        ),
        EventKind::Unknown => (
            "unknown",
            "unrecognized event (newer drain binary?)".to_string(),
        ),
    }
}

/// The liveness decision core: a [`Stale`](DrainStatus::Stale) drain (recorded
/// orchestrator PID is dead) yields one `WAKE drain-crashed` line; an active or
/// absent drain yields nothing. Pure so it is unit-testable without a live
/// process, the same pattern as `ci_idle_timeout::ci_wait_verdict`.
// trace:TASK-990 | ai:claude
fn stale_wake_line(status: &DrainStatus) -> Option<String> {
    match status {
        DrainStatus::Stale(state) => Some(format!(
            "WAKE drain-crashed — orchestrator pid {} is no longer running",
            state.orchestrator_pid
        )),
        DrainStatus::Active(_) | DrainStatus::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(path: &Path, events: &[Event]) {
        let body: String = events
            .iter()
            .map(|e| serde_json::to_string(e).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn watch_once_emits_only_actionable_lines() {
        // A fixture mixing benign churn with actionable verbs. The once-backlog
        // path (`drain_new_lines`) must print one WAKE line per actionable event
        // and stay silent on the benign ones.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        fixture(
            &path,
            &[
                Event::new(Some("STORY-1".into()), "", EventKind::RunStarted), // benign
                Event::new(
                    Some("STORY-1".into()),
                    "",
                    EventKind::PhaseEntered {
                        idx: 2,
                        slug: "ci".into(),
                    },
                ), // benign
                Event::new(
                    Some("STORY-1".into()),
                    "",
                    EventKind::PuntFiled {
                        spec: "STORY-1".into(),
                    },
                ), // WAKE
                Event::new(
                    None,
                    "",
                    EventKind::QueueDrained {
                        shipped: 1,
                        shelved: 0,
                    },
                ), // WAKE
            ],
        );

        let mut out: Vec<u8> = Vec::new();
        let mut pos = 0u64;
        drain_new_lines(&path, &mut pos, false, &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = s.lines().collect();

        assert_eq!(lines.len(), 2, "only the two actionable events wake: {s:?}");
        assert!(
            lines.iter().all(|l| l.starts_with("WAKE ")),
            "every emitted line is a wake: {s:?}"
        );
        assert!(s.contains("punt-filed STORY-1"), "{s:?}");
        assert!(s.contains("queue-drained"), "{s:?}");
        // The benign verbs are absorbed silently in the default feed.
        assert!(!s.contains("phase-entered"), "{s:?}");
        assert!(!s.contains("run-started"), "{s:?}");

        // `pos` advanced to EOF, so a re-drain with no new bytes emits nothing.
        let mut out2: Vec<u8> = Vec::new();
        drain_new_lines(&path, &mut pos, false, &mut out2).unwrap();
        assert!(out2.is_empty(), "no re-emission of already-consumed lines");
    }

    #[test]
    fn watch_all_surfaces_benign_lines() {
        // `--all` is the debug feed: benign events render too, but without the
        // WAKE prefix so a consumer can still tell them apart.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        fixture(
            &path,
            &[
                Event::new(Some("STORY-1".into()), "", EventKind::RunStarted),
                Event::new(
                    Some("STORY-1".into()),
                    "",
                    EventKind::PuntFiled {
                        spec: "STORY-1".into(),
                    },
                ),
            ],
        );
        let mut out: Vec<u8> = Vec::new();
        let mut pos = 0u64;
        drain_new_lines(&path, &mut pos, true, &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert_eq!(
            s.lines().count(),
            2,
            "both lines surface under --all: {s:?}"
        );
        assert!(s.contains("run-started"), "{s:?}");
        assert!(
            s.lines().any(|l| l.starts_with("WAKE punt-filed")),
            "actionable still WAKEs under --all: {s:?}"
        );
        assert!(
            s.lines()
                .any(|l| !l.starts_with("WAKE") && l.contains("run-started")),
            "benign line is un-prefixed under --all: {s:?}"
        );
    }

    #[test]
    fn watch_emits_drain_crashed_on_stale_probe() {
        // A Stale verdict (dead orchestrator pid) must wake with `drain-crashed`
        // so a follower is never blocked forever on a crashed drain.
        let stale = DrainStatus::Stale(drain_state::DrainState::new_single(
            "STORY-1", "run-1", false,
        ));
        let line = stale_wake_line(&stale).expect("a stale drain must wake");
        assert!(line.starts_with("WAKE drain-crashed"), "{line}");

        // An active or absent drain does not wake — the follower keeps waiting.
        let active = DrainStatus::Active(drain_state::DrainState::new_single(
            "STORY-1", "run-1", false,
        ));
        assert!(stale_wake_line(&active).is_none());
        assert!(stale_wake_line(&DrainStatus::None).is_none());
    }

    #[test]
    fn render_one_skips_blank_and_malformed_lines() {
        assert!(render_one("", false).is_none());
        assert!(render_one("   ", false).is_none());
        assert!(render_one("{not json", false).is_none());
    }

    #[test]
    fn drain_new_lines_noop_when_file_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl"); // never created
        let mut out: Vec<u8> = Vec::new();
        let mut pos = 0u64;
        drain_new_lines(&path, &mut pos, true, &mut out).unwrap();
        assert!(out.is_empty());
        assert_eq!(pos, 0);
    }
}
