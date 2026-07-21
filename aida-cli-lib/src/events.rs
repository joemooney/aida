//! `aida` drain event stream — `.aida/events.jsonl` (STORY-712 slice 1).
//!
//! # Why this exists
//!
//! Today supervision burns tokens because the *supervising loop is the LLM*:
//! a watcher wakes on a timer and forks a full `claude -p` pass on bare
//! cadence — whether or not anything actually happened. The drain already
//! knows every state change (it calls [`crate::drain_state`] mutators and the
//! [`crate::punt`] ledger writers at exactly the right moments), but those are
//! in-place JSON snapshot mutations, so a watcher can only *poll* them.
//!
//! This module adds one **append-only event stream** the drain appends a
//! single structured line to at each state change. A future cheap classifier
//! (`aida watch`, a later slice) tails it, absorbs the benign majority in
//! code, and wakes the supervising LLM only on an *actionable* verb — so the
//! supervisor burns zero tokens while nothing actionable is happening.
//!
//! This slice ships ONLY the substrate: the [`EventKind`] taxonomy, its pure
//! [`EventKind::is_actionable`] classifier, the [`Event`] record, and a
//! best-effort append-only [`emit`]. It changes **no control flow** — emit is
//! co-located beside the existing state mutators and its result is discarded,
//! so a write failure can never stall a drain. There is no consumer yet; the
//! immediate payoff is a live `tail -f .aida/events.jsonl` feed for humans.
//!
//! # Contract
//!
//! [`emit`] mirrors [`crate::punt::append_to_ledger`]'s writer contract
//! exactly — single `write(2)` of one JSON line, creates the parent dir,
//! best-effort — except that emit **swallows its error internally** (returns
//! `()`) rather than returning a `Result`, so no caller can accidentally
//! propagate an emit failure onto the drain's hot path.
//!
//! Unlike the punt ledger, this stream is **not telemetry-gated**: it is
//! supervision substrate (a sibling of `.aida/drain-state.json`), local-only,
//! never phoned home — so it must keep working even when telemetry is opted
//! out. The privacy floor is preserved because the file never leaves the
//! machine.
//!
//! trace:TASK-987 trace:STORY-712 | ai:claude

use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single drain state-change verb. Internally tagged on the `event` field so
/// an [`Unknown`](EventKind::Unknown) catch-all can absorb a kind a *newer*
/// drain binary wrote that this (older) binary does not recognize — keeping
/// forward compatibility wake-safe rather than a hard parse error.
// trace:TASK-987 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum EventKind {
    /// An orchestration run started for a spec. Bookkeeping — the supervisor
    /// already knows it launched. **Silent.**
    RunStarted,
    /// The current member entered a pipeline phase. The benign majority —
    /// phase 1→2→3 churn. **Silent.**
    PhaseEntered {
        /// 1-based phase index.
        idx: i32,
        /// Phase machine name, e.g. `implementer`.
        slug: String,
    },
    /// CI reached a terminal verdict (green/red). A real decision point.
    /// **Actionable.**
    CiTerminal {
        /// Whether the terminal verdict was green.
        green: bool,
    },
    /// A spec finished a phase with a PR open — the supervisor may merge or
    /// advance. **Actionable.**
    PhaseDonePr {
        /// The PR number that shipped.
        pr: u32,
    },
    /// A spec was parked `NeedsAttention` on a shelvable phase failure
    /// (EPIC-28) — a triage candidate. **Actionable.**
    SpecShelved {
        /// Phase that failed, e.g. `ci`.
        phase: String,
        /// Failure kind, e.g. `ci-red`.
        kind: String,
    },
    /// A design-fork punt hit the cascade — the load-bearing case.
    /// **Actionable.**
    PuntFiled {
        /// Display id of the punted spec.
        spec: String,
    },
    /// A decision was escalated to a human — the human tier was reached.
    /// **Actionable.**
    AdvisorEscalated {
        /// Categorized reason a human is needed.
        reason: String,
    },
    /// A PR was merged — an integration milestone. **Actionable.**
    PrMerged {
        /// The merged PR number.
        pr: u32,
    },
    /// A drain finished — the terminal "agent is done" an overnight loop waits
    /// for. **Actionable.**
    QueueDrained {
        /// Specs that shipped.
        shipped: usize,
        /// Specs that shelved.
        shelved: usize,
    },
    /// The supervisor's mailbox has unread mail — preserves the one
    /// event-driven trigger that exists today (TASK-776). **Actionable.**
    UnreadMail,
    /// A `aida zen <spec> --compete` bake-off reached a verdict: the winning
    /// candidate merged, the loser discarded. This row IS the outcome record
    /// the spec ratified (winner vendor + per-candidate scores + spec-kind) —
    /// the labeled dispatch-policy data point for "which vendor wins which
    /// kind of spec". **Actionable.**
    // trace:STORY-722 | ai:claude
    CompeteOutcome {
        /// Vendor whose candidate won and was merged (e.g. `claude`).
        winner: String,
        /// Per-candidate scores — every candidate that produced a branch,
        /// including the eliminated one.
        scores: Vec<CompeteCandidateScore>,
        /// The spec's requirement type (e.g. `Story`) — the spec-kind axis
        /// for dispatch-policy learning.
        spec_kind: String,
    },
    /// Forward-compat catch-all: a kind a newer binary wrote that this one
    /// does not know. Never emitted by this binary; produced only by
    /// deserializing an unrecognized `event` tag. Classified **actionable**
    /// so an unknown future event is wake-safe (never silently dropped).
    #[serde(other)]
    Unknown,
}

impl EventKind {
    /// Whether this event should WAKE the supervising LLM, or be absorbed
    /// silently by the cheap classifier.
    ///
    /// Pure and exhaustive (no `_` wildcard) so that adding a future variant
    /// is a *compile error* until it is consciously classified — and the
    /// documented convention is to default a new variant to **actionable**
    /// (wake-safe), the same bias the [`Unknown`](Self::Unknown) catch-all
    /// applies at the deserialization boundary. Over-waking is recoverable;
    /// a silently-dropped actionable event is not.
    // trace:TASK-987 trace:TASK-990 | ai:claude
    // The production consumer is the `aida watch` streaming classifier
    // (`watch.rs`, STORY-712 slice 2), which calls this per event line.
    pub fn is_actionable(&self) -> bool {
        match self {
            // Benign churn — absorbed by the watcher, never wakes the LLM.
            EventKind::RunStarted | EventKind::PhaseEntered { .. } => false,
            // Real decision points — wake the supervisor.
            EventKind::CiTerminal { .. }
            | EventKind::PhaseDonePr { .. }
            | EventKind::SpecShelved { .. }
            | EventKind::PuntFiled { .. }
            | EventKind::AdvisorEscalated { .. }
            | EventKind::PrMerged { .. }
            | EventKind::QueueDrained { .. }
            | EventKind::UnreadMail
            | EventKind::CompeteOutcome { .. }
            | EventKind::Unknown => true,
        }
    }
}

/// One candidate's line in a [`EventKind::CompeteOutcome`] record: which
/// vendor produced it, whether its CI gate passed (a failing candidate is
/// eliminated, no debate), and the blind reviewer's rubric total when the
/// reviewer scored it.
// trace:STORY-722 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompeteCandidateScore {
    /// Vendor that produced the candidate (e.g. `claude`, `codex`).
    pub vendor: String,
    /// Whether the candidate's CI gate passed.
    pub ci_passed: bool,
    /// Blind-reviewer rubric total (out of 20). `None` for a CI-eliminated
    /// candidate, or on a walkover (a single passer wins without review).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rubric_total: Option<u32>,
}

/// One append-only line in `.aida/events.jsonl` — a single drain state change.
// trace:TASK-987 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// When the event was emitted.
    pub ts: DateTime<Utc>,
    /// The spec the event is about, when one applies (a drain-level event such
    /// as [`EventKind::QueueDrained`] has none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<String>,
    /// The orchestrator's per-run UUID, for correlation. Empty when emitted
    /// outside a live drain (best-effort — no consumer depends on it yet).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_uuid: String,
    /// The event verb + its payload.
    pub kind: EventKind,
}

impl Event {
    /// Build an event stamped with the current time.
    pub fn new(spec: Option<String>, run_uuid: impl Into<String>, kind: EventKind) -> Self {
        Self {
            ts: Utc::now(),
            spec,
            run_uuid: run_uuid.into(),
            kind,
        }
    }
}

/// Path to the event stream for a project, given its root directory.
pub fn events_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("events.jsonl")
}

/// Path to the single rotated archive of the previous run's events — the file
/// [`rotate_if_oversized`] renames the live stream to when it outgrows the cap.
/// One generation is kept (each rotation overwrites the prior archive), so
/// on-disk footprint is bounded at ~2× the cap.
// trace:TASK-993 | ai:claude
pub fn events_archive_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("events.jsonl.1")
}

/// Default size cap (bytes) above which the event stream is rotated at the next
/// run-started boundary. 5 MiB — a drain emits a handful of lines per member,
/// so even a very long overnight loop stays well under this; the cap exists to
/// stop the *cumulative* file from many drains growing without bound.
const DEFAULT_EVENTS_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// The active rotation size cap, honoring the `AIDA_EVENTS_MAX_BYTES` override
/// (used by tests to force rotation with a tiny file; also a tuning knob). An
/// unparseable or absent value falls back to [`DEFAULT_EVENTS_MAX_BYTES`].
// trace:TASK-993 | ai:claude
fn events_max_bytes() -> u64 {
    std::env::var("AIDA_EVENTS_MAX_BYTES")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_EVENTS_MAX_BYTES)
}

/// Rotate `.aida/events.jsonl` when it has grown past the size cap, so the drain
/// event stream can't grow unbounded across many drains (TASK-993).
///
/// **Rotate-at-boundary contract.** This is called ONLY from a `RunStarted`
/// emission (a drain/member boundary — see [`crate::drain_state::set_run`]),
/// never mid-phase. That is what keeps the offset-tracking consumers safe: the
/// stream shrinks only at a moment no phase is streaming into it, and every
/// consumer ([`crate::event_wait::scan_new_actionable_event`], the advisor /
/// integrator watch loops) reopens the file *by path* each tick and already
/// resets its byte offset to `0` when it sees the file has shrunk (`len < pos`).
/// So after a rotation a follower re-reads the fresh (small) stream from the top
/// — it re-processes only the just-started run's own events, and over-waking on
/// a re-read is explicitly recoverable (a silently-dropped actionable event is
/// the only unrecoverable failure, and rotation never drops one from the live
/// stream: the old lines are preserved in the archive).
///
/// The rotation itself is a single `rename` (atomic on one filesystem), so a
/// concurrent reader opening the path sees either the full old file or the fresh
/// empty one — never a partially-clobbered stream. The subsequent [`emit`]
/// recreates `events.jsonl` via its `create(true)` open.
///
/// **Best-effort**: any error (stat, rename) is swallowed — like [`emit`], this
/// sits at the head of the drain's hot path and must never stall it. A file
/// that does not exist yet, or is under the cap, is a no-op.
// trace:TASK-993 | ai:claude
pub fn rotate_if_oversized(project_root: &Path) {
    rotate_over_cap(project_root, events_max_bytes());
}

/// The cap-parameterized body of [`rotate_if_oversized`]; kept separate so tests
/// can force rotation with a tiny explicit cap without mutating the process-wide
/// `AIDA_EVENTS_MAX_BYTES` env var (which would race parallel tests).
// trace:TASK-993 | ai:claude
fn rotate_over_cap(project_root: &Path, max_bytes: u64) {
    let path = events_path(project_root);
    let len = match std::fs::metadata(&path) {
        Ok(m) => m.len(),
        // No stream yet (or unreadable) — nothing to rotate.
        Err(_) => return,
    };
    if len <= max_bytes {
        return;
    }
    // Archive the prior run's events as events.jsonl.1 (one generation kept),
    // replacing any older archive. Best-effort; emit recreates the live file.
    let archive = events_archive_path(project_root);
    let _ = std::fs::rename(&path, &archive);
}

/// Env var that turns the whole event stream OFF for the process: when set to
/// a truthy value, [`emit`] is a no-op and nothing is ever appended to any
/// project's `.aida/events.jsonl`.
///
/// The escape hatch exists for TEST HARNESSES (BUG-770). An in-process unit
/// test can be isolated by injecting a temp root at the call site, but an
/// *integration* test spawns the real binary in a child process, where no
/// injected seam reaches — and `cfg!(test)` is false in that child. This var
/// is the one lever that covers both: export it once around a test invocation
/// and no test, at any nesting depth, can append to a developer's real
/// supervision stream. It is deliberately NOT the primary fix — the primary
/// fix is that callers pass the root they mean rather than resolving it from
/// the process cwd.
// trace:BUG-770 | ai:claude
pub const EVENTS_DISABLE_ENV: &str = "AIDA_EVENTS_DISABLE";

/// Whether [`EVENTS_DISABLE_ENV`] is set to a truthy value. Empty, `0`,
/// `false`, `no`, `off` (any case) all mean "not disabled", so an accidentally
/// exported empty var can't silently blind a real drain's supervision stream.
// trace:BUG-770 | ai:claude
fn events_disabled() -> bool {
    std::env::var(EVENTS_DISABLE_ENV)
        .map(|v| is_truthy(&v))
        .unwrap_or(false)
}

/// Shared truthiness spelling for [`events_disabled`].
// trace:BUG-770 | ai:claude
fn is_truthy(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "0" | "false" | "no" | "off"
    )
}

/// How many events in a window the cheap classifier **absorbed** (benign, cost
/// the supervising LLM nothing) versus **surfaced** (actionable, woke it).
///
/// This is the empirical proof of the STORY-712 lever: the ratio says how much
/// of the drain's state-change churn never reached a token-spending consumer.
/// The split is computed against [`EventKind::is_actionable`] — the *same*
/// predicate `aida watch` classifies each line with — so the numbers mean
/// exactly what the wake behavior does, not an approximation of it.
// trace:TASK-997 | ai:claude
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EventTally {
    /// Events absorbed silently by the classifier — zero supervision tokens.
    pub benign_absorbed: usize,
    /// Events the classifier surfaced as a wake.
    pub actionable: usize,
    /// The window could only be derived in part: the live stream was rotated
    /// (TASK-993) after the window opened, so events from before the rotation
    /// are in the archive and are NOT counted here. Reported rather than
    /// silently swallowed so the ratio is never over-claimed.
    pub partial_window: bool,
}

impl EventTally {
    /// Total events classified in the window.
    pub fn seen(&self) -> usize {
        self.benign_absorbed + self.actionable
    }

    /// Percentage of the window the classifier absorbed, rounded to the nearest
    /// whole percent. `0` when nothing was seen (no division by zero).
    pub fn absorbed_pct(&self) -> u32 {
        let seen = self.seen();
        if seen == 0 {
            return 0;
        }
        ((self.benign_absorbed as f64 / seen as f64) * 100.0).round() as u32
    }
}

/// Classify every event line in `body` that is stamped at or after `since`,
/// tallying benign-absorbed versus actionable.
///
/// Pure, so the tally logic is unit-testable without touching a real stream.
/// Tolerant in exactly the way the streaming classifier is: a blank or
/// malformed line is skipped rather than counted or errored on, and an
/// unrecognized `event` tag deserializes to [`EventKind::Unknown`] — which is
/// actionable, so a newer binary's verb is counted as a wake, never as absorbed
/// churn.
// trace:TASK-997 | ai:claude
pub fn classify_since(body: &str, since: DateTime<Utc>) -> EventTally {
    let mut tally = EventTally::default();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(ev) = serde_json::from_str::<Event>(trimmed) else {
            continue;
        };
        if ev.ts < since {
            continue;
        }
        if ev.kind.is_actionable() {
            tally.actionable += 1;
        } else {
            tally.benign_absorbed += 1;
        }
    }
    tally
}

/// Read `.aida/events.jsonl` and tally the classifier's benign-absorbed versus
/// actionable split over the window starting at `since` — the drain's own
/// window when called at drain exit.
///
/// Best-effort like the rest of this module: a missing or unreadable stream
/// yields an all-zero tally (the drain simply emitted nothing we can read),
/// never an error on the exit path. `partial_window` is set when the archive
/// (`events.jsonl.1`) was written after the window opened, i.e. a rotation
/// happened mid-window and the pre-rotation lines are no longer in the live
/// stream.
///
/// Caveat worth knowing when reading the number: the stream is per-project, so
/// the window covers every event the project emitted in that span. AIDA holds a
/// single-drain lock per repo, so in practice that is this drain's own churn.
// trace:TASK-997 | ai:claude
pub fn tally_window(project_root: &Path, since: std::time::SystemTime) -> EventTally {
    let since_utc: DateTime<Utc> = since.into();
    let body = std::fs::read_to_string(events_path(project_root)).unwrap_or_default();
    let mut tally = classify_since(&body, since_utc);
    tally.partial_window = rotated_since(project_root, since);
    tally
}

/// Whether the single-generation archive was written at or after `since` — the
/// signal that [`rotate_if_oversized`] fired inside the window, so the live
/// stream no longer holds all of it.
// trace:TASK-997 | ai:claude
fn rotated_since(project_root: &Path, since: std::time::SystemTime) -> bool {
    std::fs::metadata(events_archive_path(project_root))
        .and_then(|m| m.modified())
        .map(|modified| modified >= since)
        .unwrap_or(false)
}

/// Append one event to `.aida/events.jsonl`, creating the file (and `.aida/`)
/// if needed. One JSON object per line.
///
/// **Best-effort and non-blocking**: any error (serialization, dir creation,
/// full disk, permission) is swallowed here and never propagated — emit sits
/// on the drain's hot path and must never stall it. The serialized line + `\n`
/// is written in a single `write_all` so POSIX `O_APPEND` atomicity holds
/// under concurrent writers (the [`crate::punt::append_to_ledger`] contract).
///
/// BUG-770: honors the [`EVENTS_DISABLE_ENV`] kill switch. Note that emit
/// writes wherever `project_root` points — it has no way to tell a real
/// project from a test fixture, so **the caller owns that decision**: pass the
/// root you mean, never one resolved from the ambient process cwd at the emit
/// site.
// trace:TASK-987 trace:BUG-770 | ai:claude
pub fn emit(project_root: &Path, ev: &Event) {
    if events_disabled() {
        return;
    }
    let _ = try_emit(project_root, ev);
}

/// The fallible body of [`emit`]; kept separate so the happy path reads as a
/// normal `?`-chain while [`emit`] discards the result.
fn try_emit(project_root: &Path, ev: &Event) -> std::io::Result<()> {
    let path = events_path(project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_string(ev)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    line.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_actionable_wakes_on_punt_and_shelve_and_done() {
        // The load-bearing WAKE rows of the taxonomy.
        assert!(EventKind::PuntFiled {
            spec: "STORY-712".into()
        }
        .is_actionable());
        assert!(EventKind::SpecShelved {
            phase: "ci".into(),
            kind: "ci-red".into(),
        }
        .is_actionable());
        assert!(EventKind::PhaseDonePr { pr: 1207 }.is_actionable());
        assert!(EventKind::CiTerminal { green: true }.is_actionable());
        assert!(EventKind::AdvisorEscalated {
            reason: "strategy".into()
        }
        .is_actionable());
        assert!(EventKind::PrMerged { pr: 1207 }.is_actionable());
        assert!(EventKind::QueueDrained {
            shipped: 8,
            shelved: 1,
        }
        .is_actionable());
        assert!(EventKind::UnreadMail.is_actionable());
    }

    // trace:STORY-722 | ai:claude
    #[test]
    fn compete_outcome_is_actionable_and_roundtrips() {
        let kind = EventKind::CompeteOutcome {
            winner: "claude".into(),
            scores: vec![
                CompeteCandidateScore {
                    vendor: "claude".into(),
                    ci_passed: true,
                    rubric_total: Some(17),
                },
                CompeteCandidateScore {
                    vendor: "codex".into(),
                    ci_passed: false,
                    rubric_total: None,
                },
            ],
            spec_kind: "Story".into(),
        };
        assert!(kind.is_actionable());
        let json = serde_json::to_string(&kind).unwrap();
        // Internally tagged on `event`, like every other kind.
        assert!(json.contains("\"event\":\"CompeteOutcome\""));
        // A CI-eliminated candidate's absent rubric total is omitted, not null.
        assert!(!json.contains("rubric_total\":null"));
        let back: EventKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, kind);
    }

    #[test]
    fn is_actionable_silent_on_phase_churn() {
        // The benign majority the watcher absorbs in cheap code.
        assert!(!EventKind::RunStarted.is_actionable());
        assert!(!EventKind::PhaseEntered {
            idx: 2,
            slug: "ci".into(),
        }
        .is_actionable());
    }

    #[test]
    fn is_actionable_unknown_variant_defaults_to_wake() {
        // A kind a newer binary wrote, deserialized by this (older) one, lands
        // in the Unknown catch-all rather than erroring — and wakes, because a
        // silently-dropped actionable event is worse than an extra wake.
        let parsed: EventKind =
            serde_json::from_str(r#"{"event":"SomeFutureKind","extra":42}"#).unwrap();
        assert_eq!(parsed, EventKind::Unknown);
        assert!(parsed.is_actionable());
        // And the explicit value classifies the same way.
        assert!(EventKind::Unknown.is_actionable());
    }

    #[test]
    fn emit_appends_single_jsonl_line_creates_dir() {
        let dir = tempfile::tempdir().unwrap();
        // The `.aida/` dir does not exist yet — emit must create it.
        assert!(!dir.path().join(".aida").exists());

        emit(
            dir.path(),
            &Event::new(
                Some("STORY-712".into()),
                "run-uuid-1",
                EventKind::PuntFiled {
                    spec: "STORY-712".into(),
                },
            ),
        );
        // A second emit appends rather than overwrites.
        emit(
            dir.path(),
            &Event::new(
                None,
                "",
                EventKind::QueueDrained {
                    shipped: 8,
                    shelved: 0,
                },
            ),
        );

        let body = std::fs::read_to_string(events_path(dir.path())).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2, "each emit is exactly one line");
        // Each line round-trips back to an Event.
        let first: Event = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first.spec.as_deref(), Some("STORY-712"));
        assert_eq!(first.run_uuid, "run-uuid-1");
        assert!(matches!(first.kind, EventKind::PuntFiled { .. }));
        // Internal `event` tag makes the stream human-greppable.
        assert!(lines[0].contains("\"event\":\"PuntFiled\""), "{}", lines[0]);
        let second: Event = serde_json::from_str(lines[1]).unwrap();
        assert!(second.spec.is_none());
        assert!(matches!(second.kind, EventKind::QueueDrained { .. }));
    }

    #[test]
    fn emit_is_noop_on_unwritable_path() {
        // Point the project root at a regular FILE, so `.aida/` can never be
        // created under it — emit must swallow the error and not panic.
        let dir = tempfile::tempdir().unwrap();
        let file_as_root = dir.path().join("not-a-dir");
        std::fs::write(&file_as_root, b"x").unwrap();

        // No panic, no propagated error — best-effort contract.
        emit(&file_as_root, &Event::new(None, "", EventKind::RunStarted));
        // And nothing was written (the path was unwritable).
        assert!(!events_path(&file_as_root).exists());
    }

    #[test]
    fn rotate_is_noop_when_absent_or_under_cap() {
        let dir = tempfile::tempdir().unwrap();
        // No stream yet — rotation is a no-op and creates nothing.
        rotate_if_oversized(dir.path());
        assert!(!events_path(dir.path()).exists());
        assert!(!events_archive_path(dir.path()).exists());

        // A small stream (well under the default 5 MiB cap) is left untouched.
        emit(
            dir.path(),
            &Event::new(Some("STORY-712".into()), "run-1", EventKind::RunStarted),
        );
        let before = std::fs::read_to_string(events_path(dir.path())).unwrap();
        rotate_if_oversized(dir.path());
        let after = std::fs::read_to_string(events_path(dir.path())).unwrap();
        assert_eq!(before, after, "under-cap stream must not rotate");
        assert!(
            !events_archive_path(dir.path()).exists(),
            "no archive created under the cap"
        );
    }

    #[test]
    fn rotate_archives_when_over_cap_and_emit_starts_fresh() {
        // A tiny explicit cap forces rotation on a small file — no multi-MiB
        // fixture, no env mutation (uses the cap-parameterized inner helper).
        let dir = tempfile::tempdir().unwrap();

        // Prime the stream past the 10-byte cap.
        emit(
            dir.path(),
            &Event::new(Some("STORY-712".into()), "old-run", EventKind::RunStarted),
        );
        let old_body = std::fs::read_to_string(events_path(dir.path())).unwrap();
        assert!(old_body.len() > 10, "fixture must exceed the tiny cap");

        // Rotate at the next run-started boundary: live file is archived...
        rotate_over_cap(dir.path(), 10);
        assert!(
            !events_path(dir.path()).exists(),
            "live stream is renamed away by rotation"
        );
        let archived = std::fs::read_to_string(events_archive_path(dir.path())).unwrap();
        assert_eq!(
            archived, old_body,
            "prior run's events preserved in archive"
        );

        // ...and the next emit recreates a fresh, small live stream holding only
        // the new run's events (the RunStarted emit that co-triggers rotation in
        // the real drain path).
        emit(
            dir.path(),
            &Event::new(Some("STORY-712".into()), "new-run", EventKind::RunStarted),
        );
        let fresh = std::fs::read_to_string(events_path(dir.path())).unwrap();
        assert_eq!(fresh.lines().count(), 1, "fresh stream holds only new run");
        let ev: Event = serde_json::from_str(fresh.trim_end()).unwrap();
        assert_eq!(ev.run_uuid, "new-run");
    }

    #[test]
    fn rotate_overwrites_prior_archive_bounding_footprint() {
        let dir = tempfile::tempdir().unwrap();

        // First over-cap generation → archived.
        emit(
            dir.path(),
            &Event::new(Some("STORY-1".into()), "gen-1", EventKind::RunStarted),
        );
        rotate_over_cap(dir.path(), 10);
        let gen1 = std::fs::read_to_string(events_archive_path(dir.path())).unwrap();
        assert!(gen1.contains("gen-1"));

        // Second over-cap generation → the archive is OVERWRITTEN, not grown
        // (only one generation kept, so on-disk footprint stays bounded).
        emit(
            dir.path(),
            &Event::new(Some("STORY-2".into()), "gen-2", EventKind::RunStarted),
        );
        rotate_over_cap(dir.path(), 10);
        let gen2 = std::fs::read_to_string(events_archive_path(dir.path())).unwrap();
        assert!(gen2.contains("gen-2"));
        assert!(!gen2.contains("gen-1"), "archive keeps only one generation");
    }

    /// BUG-770: the kill switch turns emit into a no-op for the whole process
    /// — the lever a test harness (or an integration test, where no in-process
    /// seam reaches) exports so nothing can append to a real supervision
    /// stream. Falsy and empty spellings must NOT disable it, or an
    /// accidentally-exported empty var would silently blind a live drain.
    // trace:BUG-770 | ai:claude
    #[test]
    fn emit_is_noop_when_disable_env_is_truthy() {
        let dir = tempfile::tempdir().unwrap();
        let ev = || Event::new(None, "", EventKind::RunStarted);

        // Truthy → nothing is written at all (not even the `.aida/` dir).
        let mut guard = crate::test_env::EnvVarGuard::set(EVENTS_DISABLE_ENV, "1");
        emit(dir.path(), &ev());
        assert!(
            !events_path(dir.path()).exists(),
            "the kill switch must suppress the write entirely"
        );

        // Falsy / empty spellings are NOT a disable — emit still works.
        for falsy in ["", "0", "false", "no", "off", "OFF"] {
            guard.reset(falsy);
            assert!(!events_disabled(), "{falsy:?} must not disable emit");
        }
        guard.reset_unset();
        assert!(!events_disabled(), "absent env is not a disable");
        emit(dir.path(), &ev());
        drop(guard);
        let body = std::fs::read_to_string(events_path(dir.path())).unwrap();
        assert_eq!(body.lines().count(), 1, "only the enabled emit landed");
    }

    /// TASK-997: the tally splits a window against the SAME `is_actionable`
    /// predicate the streaming classifier wakes on — benign churn counted as
    /// absorbed, decision points as actionable — and ignores anything stamped
    /// before the window opened.
    // trace:TASK-997 | ai:claude
    #[test]
    fn classify_since_splits_benign_from_actionable_within_the_window() {
        let window_open = Utc::now();
        let before = window_open - chrono::Duration::seconds(60);
        let after = window_open + chrono::Duration::seconds(1);

        let mut lines: Vec<String> = Vec::new();
        let mut push = |ts: DateTime<Utc>, kind: EventKind| {
            let ev = Event {
                ts,
                spec: Some("STORY-1".into()),
                run_uuid: "run-1".into(),
                kind,
            };
            lines.push(serde_json::to_string(&ev).unwrap());
        };
        // A previous drain's churn — outside the window, must not be counted.
        push(before, EventKind::RunStarted);
        push(before, EventKind::PrMerged { pr: 1 });
        // This window: benign churn…
        push(after, EventKind::RunStarted);
        for idx in 1..=3 {
            push(
                after,
                EventKind::PhaseEntered {
                    idx,
                    slug: "implementer".into(),
                },
            );
        }
        // …and the actionable decision points.
        push(after, EventKind::CiTerminal { green: true });
        push(
            after,
            EventKind::PuntFiled {
                spec: "STORY-1".into(),
            },
        );

        let body = format!("{}\n\n  \n{{not json\n", lines.join("\n"));
        let tally = classify_since(&body, window_open);

        assert_eq!(tally.benign_absorbed, 4, "run-started + 3 phase-entered");
        assert_eq!(tally.actionable, 2, "ci-terminal + punt-filed");
        assert_eq!(tally.seen(), 6);
        assert_eq!(tally.absorbed_pct(), 67, "4/6 rounds to 67%");
        // Pure classification says nothing about rotation.
        assert!(!tally.partial_window);
    }

    /// An empty window is all-zero (and divides by zero nowhere), and an
    /// unrecognized verb from a newer binary counts as ACTIONABLE — never as
    /// absorbed churn, so the lever can't be flattered by a forward-compat gap.
    // trace:TASK-997 | ai:claude
    #[test]
    fn classify_since_is_zero_on_empty_and_wake_safe_on_unknown() {
        let now = Utc::now();
        let empty = classify_since("", now);
        assert_eq!(empty.seen(), 0);
        assert_eq!(empty.absorbed_pct(), 0);

        let unknown = format!(
            r#"{{"ts":"{}","kind":{{"event":"some-future-verb"}}}}"#,
            now.to_rfc3339()
        );
        let tally = classify_since(&unknown, now);
        assert_eq!(tally.actionable, 1, "unknown verbs are wake-safe");
        assert_eq!(tally.benign_absorbed, 0);
    }

    /// The file-backed wrapper reads the live stream, honors the window, and
    /// flags a mid-window rotation as a PARTIAL window rather than reporting a
    /// ratio over the surviving fragment as if it covered the whole drain.
    // trace:TASK-997 | ai:claude
    #[test]
    fn tally_window_reads_the_stream_and_flags_rotation_as_partial() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Backdated a few seconds: a real drain's window opens well before its
        // first event, and the slack keeps the assertion off filesystem mtime
        // granularity (a coarse-mtime tmpdir can floor a just-written file's
        // timestamp below an instant taken microseconds earlier).
        let opened = std::time::SystemTime::now() - std::time::Duration::from_secs(5);

        // Written directly rather than via `emit` so the process-wide
        // AIDA_EVENTS_DISABLE kill switch (exercised by a sibling test) can
        // never race this fixture.
        let stream = [
            Event::new(Some("STORY-1".into()), "run-1", EventKind::RunStarted),
            Event::new(
                Some("STORY-1".into()),
                "run-1",
                EventKind::PhaseEntered {
                    idx: 1,
                    slug: "implementer".into(),
                },
            ),
            Event::new(
                Some("STORY-1".into()),
                "run-1",
                EventKind::PrMerged { pr: 9 },
            ),
        ];
        let path = events_path(root);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let body: String = stream
            .iter()
            .map(|e| serde_json::to_string(e).unwrap() + "\n")
            .collect();
        std::fs::write(&path, body).unwrap();

        let tally = tally_window(root, opened);
        assert_eq!(tally.seen(), 3);
        assert_eq!(tally.actionable, 1, "pr-merged is the only wake");
        assert_eq!(tally.benign_absorbed, 2);
        assert!(
            !tally.partial_window,
            "no archive was written — the window is whole"
        );

        // Force a rotation inside the window: the archive now postdates the
        // window opening, so the tally must own up to being partial.
        std::fs::write(events_archive_path(root), "{}\n").unwrap();
        let rotated = tally_window(root, opened);
        assert!(rotated.partial_window, "a mid-window rotation is partial");

        // An archive predating the window is a PRIOR drain's rotation — it says
        // nothing about this window, so the tally stays whole.
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        assert!(
            !tally_window(root, later).partial_window,
            "an archive from before the window opened is not this drain's rotation"
        );
    }

    /// A missing stream is an all-zero tally, never an error on the exit path.
    // trace:TASK-997 | ai:claude
    #[test]
    fn tally_window_is_zero_when_stream_absent() {
        let dir = tempfile::tempdir().unwrap();
        let tally = tally_window(dir.path(), std::time::SystemTime::now());
        assert_eq!(tally, EventTally::default());
    }

    #[test]
    fn events_max_bytes_honors_env_override_else_default() {
        // Default when unset (and un-parseable) is the 5 MiB constant.
        assert_eq!(DEFAULT_EVENTS_MAX_BYTES, 5 * 1024 * 1024);
        // The env parse tolerates surrounding whitespace.
        assert_eq!("  42  ".trim().parse::<u64>().unwrap(), 42);
    }
}
