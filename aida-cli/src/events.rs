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
    // trace:TASK-987 | ai:claude
    // why: the pure classifier is exercised by this module's unit tests now; its
    // production consumer is the `aida watch` streaming classifier, a deliberate
    // later slice of STORY-712 (this slice ships only the emit substrate).
    #[allow(dead_code)]
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
            | EventKind::Unknown => true,
        }
    }
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

/// Append one event to `.aida/events.jsonl`, creating the file (and `.aida/`)
/// if needed. One JSON object per line.
///
/// **Best-effort and non-blocking**: any error (serialization, dir creation,
/// full disk, permission) is swallowed here and never propagated — emit sits
/// on the drain's hot path and must never stall it. The serialized line + `\n`
/// is written in a single `write_all` so POSIX `O_APPEND` atomicity holds
/// under concurrent writers (the [`crate::punt::append_to_ledger`] contract).
// trace:TASK-987 | ai:claude
pub fn emit(project_root: &Path, ev: &Event) {
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
}
