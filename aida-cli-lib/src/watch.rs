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
//! # Where follow mode starts
//!
//! Follow mode seeks to **end-of-file** on start, exactly like `tail -f`: a
//! supervisor arming this command wakes only on events appended *after* it
//! armed. `--once` is the drain-the-backlog mode (classify what is there and
//! exit); `--backlog` is the explicit opt-in to replay history *and then*
//! follow. trace:TASK-146 | ai:claude
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
//! # The verbose debugging feed
//!
//! `--verbose` is the *human* end of the same stream: it implies `--all` (every
//! event, not just the actionable ones) and stamps each line with the event's
//! local wall-clock time and the run correlation id, then appends the raw
//! payload — the fields the one-line hint elides (per-candidate bake-off scores,
//! a punt's spec, a shelve's phase/kind). That is what makes a live drain
//! debuggable: *when* did it happen, *which run* emitted it, and *exactly* what
//! the drain wrote. The wake-lines-only default stays the machine surface.
//! trace:TASK-994 | ai:claude
//!
//! trace:TASK-990 trace:STORY-712 | ai:claude

use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use chrono::Local;

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
    /// Verbose live-debugging feed: implies [`all`](Self::all), timestamps and
    /// run-tags every line, and appends each event's raw payload.
    // trace:TASK-994 | ai:claude
    pub verbose: bool,
    /// Drain the current backlog, classify it, and exit (cron / test mode).
    /// Without it, the command follows the stream like `tail -f`.
    pub once: bool,
    /// Follow mode only: replay the whole historical backlog before following
    /// instead of starting at end-of-file. Off by default — a fresh follower
    /// wants only what happens from now on.
    // trace:TASK-146 | ai:claude
    pub backlog: bool,
}

/// The two rendering levers, resolved once and threaded through the follow
/// loop so the debug feed and the default wake feed share one code path.
/// `Copy` (two flags) so it costs nothing to pass by value per line.
// trace:TASK-994 | ai:claude
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Feed {
    /// Surface the benign (non-actionable) events too.
    all: bool,
    /// Stamp each line with local time + run tag and append the raw payload.
    verbose: bool,
}

impl Feed {
    /// Resolve the caller's flags. `--verbose` **implies** `--all`: a debugging
    /// feed that still swallowed the benign majority would hide exactly the
    /// phase churn you turned it on to watch.
    // trace:TASK-994 | ai:claude
    fn from_opts(opts: &WatchOpts) -> Self {
        Self {
            all: opts.all || opts.verbose,
            verbose: opts.verbose,
        }
    }
}

/// Top-level entry point — invoked from the early command dispatch in `main.rs`
/// (read-only; needs no storage handle).
// trace:TASK-990 | ai:claude
pub fn handle_watch(project_root: &Path, opts: &WatchOpts) -> Result<()> {
    let path = events::events_path(project_root);
    let feed = Feed::from_opts(opts);
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    // The verbose header goes to STDERR, never stdout: stdout is the wake
    // stream a `Monitor`/`grep` consumer parses, and a banner there would read
    // as an event. On stderr it is visible to the human running the feed and
    // invisible to a pipe. trace:TASK-994 | ai:claude
    if feed.verbose {
        eprintln!(
            "feed: {} — {}",
            path.display(),
            if opts.once {
                "classifying the existing backlog, then exiting"
            } else if opts.backlog {
                "replaying the backlog, then following"
            } else {
                "following from end of file"
            }
        );
        eprintln!("cols: time · run · WAKE|benign · verb · spec — hint · payload");
    }

    // `--once` drains the current backlog and exits — that is the documented
    // catch-up mode. Follow mode instead starts at end-of-file like `tail -f`
    // so a fresh supervisor wakes only on events appended *after* it armed;
    // replaying a months-old backlog burned a wake plus tokens on stale noise
    // (and risked the Monitor being auto-stopped for volume). `--backlog`
    // opts back into the historical replay-then-follow behavior.
    // trace:TASK-146 | ai:claude
    let mut pos: u64 = 0;
    if opts.once || opts.backlog {
        drain_new_lines(&path, &mut pos, feed, &mut out)?;
    } else {
        pos = end_of_file(&path);
    }
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
        drain_new_lines(&path, &mut pos, feed, &mut out)?;
        std::thread::sleep(FOLLOW_POLL);
    }
}

/// The byte offset of the current end of the events file — the follow-mode
/// start position, so only lines appended after start are ever emitted. A
/// missing (or unreadable) file is offset `0`: the drain simply has not emitted
/// yet, and everything it writes from now on is new. `.aida/events.jsonl` is
/// append-only, so a plain length read is a sound seek point; a truncation is
/// still caught by the shrink check in [`drain_new_lines`].
// trace:TASK-146 | ai:claude
fn end_of_file(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
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
    feed: Feed,
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
        if let Some(line) = render_one(buf.trim_end_matches('\n'), feed) {
            writeln!(out, "{}", line)?;
            out.flush()?;
        }
    }
    Ok(())
}

/// Parse one raw JSONL line into an [`Event`] and render its output line, if any.
/// A blank or malformed line is silently skipped (tolerant follow, like
/// `headless_tail`), never a hard error.
fn render_one(line: &str, feed: Feed) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let ev: Event = serde_json::from_str(trimmed).ok()?;
    render(&ev, feed)
}

/// Render the terminal line for one event. Returns `Some(line)` for an
/// actionable event (always — a wake), or for a benign event only when `all`;
/// `None` otherwise (the benign-absorption default).
///
/// Format: `WAKE <verb> <spec> — <hint>` for wakes (the `<spec>` is omitted for
/// drain-level events that carry none); benign `--all` lines use a blank
/// indent prefix instead of `WAKE` so the two are trivially distinguishable.
///
/// Under `verbose` the same line is bracketed by two debugging affixes —
/// `<local-time> [<run>] ` in front and the event's raw JSON payload behind —
/// while the `WAKE `/indent marker keeps its position between them, so a
/// `grep WAKE` over a verbose feed still selects exactly the actionable lines.
// trace:TASK-994 | ai:claude
fn render(ev: &Event, feed: Feed) -> Option<String> {
    let actionable = ev.kind.is_actionable();
    if !actionable && !feed.all {
        return None;
    }
    let (verb, hint) = describe(&ev.kind);
    let mut line = String::new();
    if feed.verbose {
        // Local wall-clock (the operator's own clock — the stream stores UTC)
        // with millisecond resolution, so two events inside the same second are
        // still ordered by eye. trace:TASK-994 | ai:claude
        line.push_str(
            &ev.ts
                .with_timezone(&Local)
                .format("%H:%M:%S%.3f")
                .to_string(),
        );
        line.push(' ');
        line.push_str(&run_tag(&ev.run_uuid));
        line.push(' ');
    }
    line.push_str(if actionable { "WAKE " } else { "     " });
    line.push_str(verb);
    if let Some(spec) = ev.spec.as_deref() {
        line.push(' ');
        line.push_str(spec);
    }
    line.push_str(" — ");
    line.push_str(&hint);
    if feed.verbose {
        // The hint is a summary; the payload is the ground truth (bake-off
        // scores, phase/kind, PR number) a debugging session actually needs.
        if let Ok(payload) = serde_json::to_string(&ev.kind) {
            line.push_str("  ");
            line.push_str(&payload);
        }
    }
    Some(line)
}

/// The fixed-width run-correlation tag for the verbose feed: the first 8 chars
/// of the emitting orchestrator's per-run uuid, so interleaved runs are
/// separable by eye. `run_uuid` is best-effort (empty when the event was
/// emitted outside a live drain), which renders as a same-width placeholder so
/// the columns never jitter.
// trace:TASK-994 | ai:claude
fn run_tag(run_uuid: &str) -> String {
    let id: String = run_uuid.chars().take(8).collect();
    if id.is_empty() {
        "[--------]".to_string()
    } else {
        format!("[{:<8}]", id)
    }
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
        // trace:STORY-722 | ai:claude
        EventKind::CompeteOutcome {
            winner, spec_kind, ..
        } => (
            "compete-outcome",
            format!("bake-off won by {} (spec-kind {})", winner, spec_kind),
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

    /// The default machine feed: wake lines only.
    const WAKES: Feed = Feed {
        all: false,
        verbose: false,
    };
    /// `--all`: benign events surface too, in the terse one-line form.
    const ALL: Feed = Feed {
        all: true,
        verbose: false,
    };
    /// `--verbose`: the live-debugging feed (implies `--all`).
    // trace:TASK-994 | ai:claude
    const VERBOSE: Feed = Feed {
        all: true,
        verbose: true,
    };

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
        drain_new_lines(&path, &mut pos, WAKES, &mut out).unwrap();
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
        drain_new_lines(&path, &mut pos, WAKES, &mut out2).unwrap();
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
        drain_new_lines(&path, &mut pos, ALL, &mut out).unwrap();
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

    /// The TASK-146 regression: follow mode must start at end-of-file, so a
    /// pre-populated backlog (however old, however actionable) emits nothing —
    /// only a line appended *after* the follower armed wakes it.
    // trace:TASK-146 | ai:claude
    #[test]
    fn follow_starts_at_eof_and_emits_only_new_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        // A historical backlog full of actionable verbs — exactly the flood the
        // supervisor used to receive on arming the Monitor.
        fixture(
            &path,
            &[
                Event::new(
                    Some("STORY-306".into()),
                    "",
                    EventKind::AdvisorEscalated {
                        reason: "old escalation".into(),
                    },
                ),
                Event::new(Some("BUG-241".into()), "", EventKind::PrMerged { pr: 7 }),
            ],
        );

        // Arming the follower: `pos` starts at EOF, not 0.
        let mut pos = end_of_file(&path);
        assert!(pos > 0, "the fixture backlog is non-empty");

        let mut out: Vec<u8> = Vec::new();
        drain_new_lines(&path, &mut pos, WAKES, &mut out).unwrap();
        assert!(
            out.is_empty(),
            "no historical backlog replay in follow mode: {:?}",
            String::from_utf8_lossy(&out)
        );

        // A newly-appended actionable event does wake.
        {
            use std::io::Write as _;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            let ev = Event::new(
                Some("STORY-999".into()),
                "",
                EventKind::PuntFiled {
                    spec: "STORY-999".into(),
                },
            );
            writeln!(f, "{}", serde_json::to_string(&ev).unwrap()).unwrap();
        }
        let mut out2: Vec<u8> = Vec::new();
        drain_new_lines(&path, &mut pos, WAKES, &mut out2).unwrap();
        let s = String::from_utf8(out2).unwrap();
        assert_eq!(s.lines().count(), 1, "exactly the new event wakes: {s:?}");
        assert!(s.contains("punt-filed STORY-999"), "{s:?}");
        assert!(!s.contains("STORY-306"), "no stale replay: {s:?}");

        // `--once` / `--backlog` keep the drain-the-backlog behavior.
        let mut pos_backlog = 0u64;
        let mut out3: Vec<u8> = Vec::new();
        drain_new_lines(&path, &mut pos_backlog, WAKES, &mut out3).unwrap();
        let s3 = String::from_utf8(out3).unwrap();
        assert_eq!(
            s3.lines().count(),
            3,
            "backlog mode still classifies the whole file: {s3:?}"
        );
    }

    /// A follower armed before the drain ever emitted starts at offset 0 and
    /// still sees the first event — an absent file is not a missed wake.
    // trace:TASK-146 | ai:claude
    #[test]
    fn follow_start_offset_is_zero_when_file_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl"); // never created
        assert_eq!(end_of_file(&path), 0);
    }

    #[test]
    fn render_one_skips_blank_and_malformed_lines() {
        assert!(render_one("", WAKES).is_none());
        assert!(render_one("   ", WAKES).is_none());
        assert!(render_one("{not json", WAKES).is_none());
    }

    /// `--verbose` implies `--all`: asking for the debugging feed must not
    /// leave the benign majority absorbed, since phase churn is most of what a
    /// live drain debugging session is looking at.
    // trace:TASK-994 | ai:claude
    #[test]
    fn verbose_implies_all() {
        let opts = WatchOpts {
            verbose: true,
            ..Default::default()
        };
        assert_eq!(Feed::from_opts(&opts), VERBOSE);

        // ...and the terse feeds are unchanged by it.
        assert_eq!(Feed::from_opts(&WatchOpts::default()), WAKES);
        assert_eq!(
            Feed::from_opts(&WatchOpts {
                all: true,
                ..Default::default()
            }),
            ALL
        );
    }

    /// The verbose line carries the three debugging affixes the terse feed
    /// elides — local wall-clock time, the run correlation tag, and the raw
    /// payload — while keeping the `WAKE ` marker in place so `grep WAKE` over
    /// a verbose feed still selects exactly the actionable lines.
    // trace:TASK-994 | ai:claude
    #[test]
    fn verbose_feed_stamps_time_run_and_payload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        fixture(
            &path,
            &[
                Event::new(
                    Some("STORY-1".into()),
                    "abcdef0123456789",
                    EventKind::PhaseEntered {
                        idx: 2,
                        slug: "ci".into(),
                    },
                ), // benign
                Event::new(
                    Some("STORY-1".into()),
                    "abcdef0123456789",
                    EventKind::SpecShelved {
                        phase: "ci".into(),
                        kind: "ci-red".into(),
                    },
                ), // WAKE
            ],
        );

        let mut out: Vec<u8> = Vec::new();
        let mut pos = 0u64;
        drain_new_lines(&path, &mut pos, VERBOSE, &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), 2, "verbose surfaces benign events too: {s:?}");

        // Every line: `HH:MM:SS.mmm [run-tag] ...`.
        for l in &lines {
            let (ts, rest) = l.split_once(' ').expect("a timestamp prefix");
            assert_eq!(ts.len(), 12, "HH:MM:SS.mmm local timestamp: {l:?}");
            assert!(
                ts.chars()
                    .all(|c| c.is_ascii_digit() || c == ':' || c == '.'),
                "{l:?}"
            );
            assert!(rest.starts_with("[abcdef01]"), "run correlation tag: {l:?}");
        }

        // The WAKE marker survives the prefix, so `grep WAKE` still works.
        let wakes: Vec<&&str> = lines.iter().filter(|l| l.contains("WAKE ")).collect();
        assert_eq!(wakes.len(), 1, "only the shelve wakes: {s:?}");
        assert!(wakes[0].contains("spec-shelved STORY-1"), "{s:?}");
        assert!(!lines[0].contains("WAKE"), "benign stays un-marked: {s:?}");

        // The raw payload rides along — the fields the hint elides.
        assert!(
            lines[0].contains(r#"{"event":"PhaseEntered","idx":2,"slug":"ci"}"#),
            "benign payload appended: {s:?}"
        );
        assert!(
            lines[1].contains(r#""kind":"ci-red""#),
            "wake payload appended: {s:?}"
        );
    }

    /// An event emitted outside a live drain has no run uuid; the tag must stay
    /// the same width so the verbose columns never jitter.
    // trace:TASK-994 | ai:claude
    #[test]
    fn run_tag_is_fixed_width_even_when_absent() {
        assert_eq!(run_tag(""), "[--------]");
        assert_eq!(run_tag("abcdef0123456789"), "[abcdef01]");
        assert_eq!(run_tag("ab"), "[ab      ]");
        assert_eq!(run_tag("").len(), run_tag("abcdef0123456789").len());
    }

    /// The default and `--all` feeds are untouched by the verbose addition —
    /// no timestamp, no run tag, no payload noise on the machine surface.
    // trace:TASK-994 | ai:claude
    #[test]
    fn terse_feeds_carry_no_verbose_affixes() {
        let ev = Event::new(
            Some("STORY-1".into()),
            "abcdef0123456789",
            EventKind::PrMerged { pr: 42 },
        );
        let line = render(&ev, WAKES).expect("actionable");
        assert_eq!(line, "WAKE pr-merged STORY-1 — PR #42 merged");
        assert_eq!(render(&ev, ALL).as_deref(), Some(line.as_str()));
    }

    #[test]
    fn drain_new_lines_noop_when_file_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl"); // never created
        let mut out: Vec<u8> = Vec::new();
        let mut pos = 0u64;
        drain_new_lines(&path, &mut pos, ALL, &mut out).unwrap();
        assert!(out.is_empty());
        assert_eq!(pos, 0);
    }
}
