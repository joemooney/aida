//! Published per-spec timing records (STORY-1480, ADR-64 Option B).
//!
//! The live event feed (`.aida/events.jsonl`) is per-clone and gitignored, so
//! phase evidence recorded on one machine is invisible everywhere else. When
//! a spec reaches a terminal state (Done via `aida queue done`, Completed via
//! any `completion.rs` caller including the merge-trailer auto-bump), the
//! terminal clone publishes the spec's lifecycle events into the store
//! worktree at `timings/<TYPE>/<shard>/<ID>.json` — sharded exactly like
//! `objects/`, never UNDER `objects/`, whose scanners load every `*.yaml`
//! there as a requirement. `aida db sync`'s `git add -A .` carries the record
//! onto the `aida-store` orphan branch, so any clone can rebuild the spec's
//! timeline after `aida pull`.
//!
//! Publishes are union-merges: the auto-bump often completes a spec on a
//! different machine than the one that did the work (any clone's `aida pull`
//! flips Done → Completed), so the Done-time publish from the work machine
//! carries the events and a later publish only adds. Publication is
//! best-effort like every event-stream write — a failure never blocks a
//! terminal transition, and an existing record this binary cannot read is
//! never clobbered.
// trace:STORY-1480 | ai:claude

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::events::{Event, EventKind};

/// Bumped only when a reader of the published record must change.
pub(crate) const SCHEMA_VERSION: u32 = 1;

/// One terminal-state publish that touched the record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PublishStamp {
    /// When this publish happened.
    pub(crate) at: DateTime<Utc>,
    /// The terminal path that triggered it, e.g. `done`, `queue-done`,
    /// `auto-bump`.
    pub(crate) closed_by: String,
    /// Publisher's node id from `.aida/node.toml`, when registered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) node_id: Option<String>,
    /// Publisher's hostname from `.aida/node.toml`, when registered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) hostname: Option<String>,
}

/// The on-store record: the spec's lifecycle events as the publishing
/// clone(s) witnessed them, plus the publish trail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TimingRecord {
    pub(crate) schema_version: u32,
    pub(crate) spec: String,
    pub(crate) publishes: Vec<PublishStamp>,
    pub(crate) events: Vec<Event>,
}

/// Whether an event kind belongs in a published timing record. Lifecycle
/// evidence only — inbox, shift and cron noise stays local (STORY-1480's
/// explicit boundary). Exhaustive on purpose: a future kind forces a
/// publish-or-not decision here instead of silently leaking or vanishing.
// trace:STORY-1480 | ai:claude
pub(crate) fn lifecycle_publishable(kind: &EventKind) -> bool {
    use EventKind::*;
    match kind {
        RunStarted
        | PhaseEntered { .. }
        | CiTerminal { .. }
        | PhaseDonePr { .. }
        | SpecShelved { .. }
        | SpecSkipped { .. }
        | SpecRetried { .. }
        | SpecReDriven { .. }
        | SpecRequeued { .. }
        | ReclassifiedNeedsHuman { .. }
        | PuntFiled { .. }
        | AdvisorEscalated { .. }
        | PrMerged { .. }
        | MergeHoldChanged { .. }
        | ReviewVerdictRecorded { .. }
        | DispositionChanged { .. }
        | ExecutionModeChanged { .. }
        | SpecCompleted { .. }
        | RunCompleted { .. }
        | QueueDrained { .. }
        | UnshippedWorkDetected { .. }
        | PlanRecorded { .. }
        | GateHeld { .. } => true,
        UnreadMail
        | CompeteOutcome { .. }
        | CronJobFired { .. }
        | CronJobFailed { .. }
        | MailReceived { .. }
        | ShiftTick { .. }
        | Unknown => false,
    }
}

/// Publish (or extend) the spec's timing record in the default store
/// worktree, `<project_root>/.aida-store`. Best-effort: `None` means nothing
/// was written, never an error the caller must handle.
// trace:STORY-1480 | ai:claude
pub(crate) fn publish(project_root: &Path, spec_id: &str, closed_by: &str) -> Option<PathBuf> {
    let store_root = project_root.join(".aida-store");
    if !store_root.join("objects").is_dir() {
        return None;
    }
    publish_to_store(&store_root, project_root, spec_id, closed_by)
}

/// [`publish`] with the store worktree named explicitly (the testable form).
// trace:STORY-1480 | ai:claude
pub(crate) fn publish_to_store(
    store_root: &Path,
    project_root: &Path,
    spec_id: &str,
    closed_by: &str,
) -> Option<PathBuf> {
    // The record is derived from the local feed; the feed's kill switch
    // (test hermeticity, BUG-770) covers the derived store write too.
    if crate::events::events_disabled() {
        return None;
    }
    let rel = aida_core::object_store::relative_timing_record_path(spec_id).ok()?;
    let path = store_root.join(&rel);

    let mut fresh: Vec<Event> = Vec::new();
    for feed in [
        crate::events::events_archive_path(project_root),
        crate::events::events_path(project_root),
    ] {
        let Ok(body) = std::fs::read_to_string(&feed) else {
            continue;
        };
        for line in body.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(ev) = serde_json::from_str::<Event>(line) else {
                continue;
            };
            if ev
                .spec
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(spec_id))
                && lifecycle_publishable(&ev.kind)
            {
                fresh.push(ev);
            }
        }
    }

    let mut record = match std::fs::read_to_string(&path) {
        Ok(body) => match serde_json::from_str::<TimingRecord>(&body) {
            Ok(r) => r,
            // A record this binary cannot read (newer schema, corruption) is
            // evidence someone else owns; never overwrite it.
            Err(_) => return None,
        },
        Err(_) => TimingRecord {
            schema_version: SCHEMA_VERSION,
            spec: aida_core::object_store::canonical_spec_id(spec_id),
            publishes: Vec::new(),
            events: Vec::new(),
        },
    };

    let mut seen: BTreeSet<String> = record
        .events
        .iter()
        .filter_map(|e| serde_json::to_string(e).ok())
        .collect();
    let before = record.events.len();
    for ev in fresh {
        let Ok(key) = serde_json::to_string(&ev) else {
            continue;
        };
        if seen.insert(key) {
            record.events.push(ev);
        }
    }
    if record.events.is_empty() {
        // Nothing instrumented anywhere: an empty record documents nothing
        // and would only churn the store once per completed spec.
        return None;
    }
    if record.events.len() == before && path.exists() {
        // No new evidence beyond what is already published (e.g. auto-bump
        // on a clone that saw none of the work): skip the write.
        return Some(path);
    }
    record.events.sort_by(|a, b| a.ts.cmp(&b.ts));

    let (node_id, hostname) = node_identity(project_root);
    record.publishes.push(PublishStamp {
        at: Utc::now(),
        closed_by: closed_by.to_string(),
        node_id,
        hostname,
    });

    let body = serde_json::to_string_pretty(&record).ok()?;
    std::fs::create_dir_all(path.parent()?).ok()?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body.as_bytes()).ok()?;
    std::fs::rename(&tmp, &path).ok()?;
    Some(path)
}

/// The published events for one spec, plus reader notes. Missing record is
/// the common case and carries no note; an unreadable one is reported, since
/// cross-machine evidence it held is silently missing otherwise.
// trace:STORY-1480 | ai:claude
pub(crate) fn load_published(store_root: &Path, spec_id: &str) -> (Vec<Event>, Vec<String>) {
    let Ok(rel) = aida_core::object_store::relative_timing_record_path(spec_id) else {
        return (Vec::new(), Vec::new());
    };
    let path = store_root.join(rel);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return (Vec::new(), Vec::new());
    };
    match serde_json::from_str::<TimingRecord>(&body) {
        Ok(r) => (r.events, Vec::new()),
        Err(_) => (
            Vec::new(),
            vec![
                "a published timing record exists for this spec but could not be read; \
                 cross-machine evidence it held is missing"
                    .to_string(),
            ],
        ),
    }
}

fn node_identity(project_root: &Path) -> (Option<String>, Option<String>) {
    let path = project_root.join(".aida").join("node.toml");
    match aida_core::node::NodeConfig::load(&path) {
        Ok(c) => (Some(c.node_id), Some(c.hostname)),
        Err(_) => (None, None),
    }
}

// ---------------------------------------------------------------------------
// Tests — tempdir fixtures only; no git, no real store.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    fn ev(ts: &str, spec: &str, kind: EventKind) -> Event {
        Event {
            ts: DateTime::parse_from_rfc3339(ts)
                .unwrap_or_else(|e| panic!("bad fixture timestamp {ts}: {e}"))
                .with_timezone(&Utc),
            spec: Some(spec.to_string()),
            run_uuid: String::new(),
            seat: None,
            kind,
        }
    }

    fn write_feed(project_root: &Path, events: &[Event]) {
        let dir = project_root.join(".aida");
        std::fs::create_dir_all(&dir).unwrap();
        let body: String = events
            .iter()
            .map(|e| serde_json::to_string(e).unwrap() + "\n")
            .collect();
        std::fs::write(dir.join("events.jsonl"), body).unwrap();
    }

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().to_path_buf();
        let store = project.join(".aida-store");
        std::fs::create_dir_all(store.join("objects")).unwrap();
        (dir, project, store)
    }

    // trace:STORY-1480 | ai:claude
    #[test]
    fn publish_writes_lifecycle_events_and_filters_noise() {
        let (_g, project, store) = fixture();
        write_feed(
            &project,
            &[
                ev(
                    "2026-10-01T10:00:00Z",
                    "STORY-9",
                    EventKind::PhaseEntered {
                        idx: 1,
                        slug: "implementer".into(),
                        vendor: None,
                        seat: None,
                        model: None,
                        effort: None,
                        attempt: 1,
                    },
                ),
                // Inbox noise for the same spec: never published.
                ev(
                    "2026-10-01T10:01:00Z",
                    "STORY-9",
                    EventKind::MailReceived {
                        to: "advisor".into(),
                    },
                ),
                // Another spec: not this record's business.
                ev(
                    "2026-10-01T10:02:00Z",
                    "STORY-8",
                    EventKind::PrMerged { pr: 5 },
                ),
                ev(
                    "2026-10-01T11:00:00Z",
                    "STORY-9",
                    EventKind::PrMerged { pr: 7 },
                ),
            ],
        );
        let path = publish_to_store(&store, &project, "STORY-9", "done").expect("published");
        assert_eq!(path, store.join("timings/STORY/000/STORY-9.json"));
        let rec: TimingRecord =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(rec.schema_version, SCHEMA_VERSION);
        assert_eq!(rec.spec, "STORY-9");
        assert_eq!(
            rec.events.len(),
            2,
            "lifecycle events only: {:?}",
            rec.events
        );
        assert!(matches!(rec.events[0].kind, EventKind::PhaseEntered { .. }));
        assert!(matches!(rec.events[1].kind, EventKind::PrMerged { pr: 7 }));
        assert_eq!(rec.publishes.len(), 1);
        assert_eq!(rec.publishes[0].closed_by, "done");
    }

    // trace:STORY-1480 | ai:claude
    #[test]
    fn plan_recorded_is_lifecycle_evidence_and_publishes() {
        assert!(lifecycle_publishable(&EventKind::PlanRecorded {
            verified: false
        }));
        let (_g, project, store) = fixture();
        write_feed(
            &project,
            &[ev(
                "2026-10-01T09:00:00Z",
                "TASK-11",
                EventKind::PlanRecorded { verified: true },
            )],
        );
        let path = publish_to_store(&store, &project, "TASK-11", "done").expect("published");
        let rec: TimingRecord =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(matches!(
            rec.events[0].kind,
            EventKind::PlanRecorded { verified: true }
        ));
    }

    // trace:STORY-1480 | ai:claude
    #[test]
    fn publish_is_a_union_merge_not_an_overwrite() {
        let (_g, project, store) = fixture();
        // First machine published one event.
        write_feed(
            &project,
            &[ev(
                "2026-10-01T10:00:00Z",
                "TASK-3",
                EventKind::CiTerminal { green: true },
            )],
        );
        publish_to_store(&store, &project, "TASK-3", "done").expect("first publish");
        // "Another machine": same event plus a later one.
        write_feed(
            &project,
            &[
                ev(
                    "2026-10-01T10:00:00Z",
                    "TASK-3",
                    EventKind::CiTerminal { green: true },
                ),
                ev(
                    "2026-10-01T12:00:00Z",
                    "TASK-3",
                    EventKind::PrMerged { pr: 9 },
                ),
            ],
        );
        let path =
            publish_to_store(&store, &project, "TASK-3", "auto-bump").expect("second publish");
        let rec: TimingRecord =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(rec.events.len(), 2, "union, deduped: {:?}", rec.events);
        assert_eq!(rec.publishes.len(), 2);
        assert_eq!(rec.publishes[1].closed_by, "auto-bump");
    }

    // trace:STORY-1480 | ai:claude
    #[test]
    fn publish_with_nothing_new_skips_the_write() {
        let (_g, project, store) = fixture();
        write_feed(
            &project,
            &[ev(
                "2026-10-01T10:00:00Z",
                "TASK-4",
                EventKind::PrMerged { pr: 2 },
            )],
        );
        let path = publish_to_store(&store, &project, "TASK-4", "done").unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        // Re-publish with an identical feed: no new stamp, no content churn.
        let again = publish_to_store(&store, &project, "TASK-4", "queue-done").unwrap();
        assert_eq!(again, path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
    }

    // trace:STORY-1480 | ai:claude
    #[test]
    fn publish_without_events_or_store_is_a_no_op() {
        let (_g, project, store) = fixture();
        // No feed at all.
        assert!(publish_to_store(&store, &project, "TASK-5", "done").is_none());
        assert!(!store.join("timings").exists());
        // Feed with only noise for the spec.
        write_feed(
            &project,
            &[ev(
                "2026-10-01T10:00:00Z",
                "TASK-5",
                EventKind::ShiftTick {
                    launched: None,
                    reaped: 0,
                    recovered_stale_pid: None,
                    refused: vec![],
                    breaker: None,
                    escalated: vec![],
                    redriven: vec![],
                    reclassified: vec![],
                    mail_escalated: vec![],
                    redrive_held: None,
                },
            )],
        );
        assert!(publish_to_store(&store, &project, "TASK-5", "done").is_none());
        // `publish` (not `publish_to_store`) against a root with no store
        // worktree: silent no-op.
        assert!(publish(&project.join("nowhere"), "TASK-5", "done").is_none());
    }

    // trace:STORY-1480 | ai:claude
    #[test]
    fn unreadable_existing_record_is_never_clobbered() {
        let (_g, project, store) = fixture();
        write_feed(
            &project,
            &[ev(
                "2026-10-01T10:00:00Z",
                "TASK-6",
                EventKind::PrMerged { pr: 1 },
            )],
        );
        let rel = aida_core::object_store::relative_timing_record_path("TASK-6").unwrap();
        let path = store.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        assert!(publish_to_store(&store, &project, "TASK-6", "done").is_none());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
        let (events, notes) = load_published(&store, "TASK-6");
        assert!(events.is_empty());
        assert_eq!(notes.len(), 1, "unreadable record is reported: {notes:?}");
    }

    // trace:STORY-1480 | ai:claude
    #[test]
    fn load_published_roundtrips_what_publish_wrote() {
        let (_g, project, store) = fixture();
        write_feed(
            &project,
            &[
                ev(
                    "2026-10-01T10:00:00Z",
                    "BUG-77",
                    EventKind::CiTerminal { green: false },
                ),
                ev(
                    "2026-10-01T10:30:00Z",
                    "BUG-77",
                    EventKind::CiTerminal { green: true },
                ),
            ],
        );
        publish_to_store(&store, &project, "bug-77", "done").unwrap();
        let (events, notes) = load_published(&store, "BUG-77");
        assert!(notes.is_empty());
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0].kind,
            EventKind::CiTerminal { green: false }
        ));
        // Missing record: empty, silent.
        let (none, no_notes) = load_published(&store, "BUG-78");
        assert!(none.is_empty() && no_notes.is_empty());
    }
}
