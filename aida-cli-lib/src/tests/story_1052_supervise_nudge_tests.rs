use super::*;
use crate::events::{Event, EventKind};
use chrono::{TimeZone, Utc};
use std::collections::HashMap;

fn event_line(spec: &str, phase: &str, kind: &str, ts_ms: i64) -> String {
    serde_json::to_string(&Event {
        ts: Utc.timestamp_millis_opt(ts_ms).single().unwrap(),
        spec: Some(spec.to_string()),
        run_uuid: "run-1".to_string(),
        kind: EventKind::SpecShelved {
            phase: phase.to_string(),
            kind: kind.to_string(),
        },
    })
    .unwrap()
}

#[test]
fn nudge_classifier_names_two_transient_parks() {
    let mut titles = HashMap::new();
    titles.insert("TASK-1".to_string(), "first retry park".to_string());
    titles.insert("TASK-2".to_string(), "second retry park".to_string());
    let body = [
        event_line("TASK-1", "ci", "watchdog-timeout", 1_000),
        event_line("TASK-2", "reviewer", "network-transient", 2_000),
        event_line("TASK-3", "merge", "merge-conflict", 3_000),
    ]
    .join("\n");

    let stuck = stuck_items_from_events(&body, &titles);
    assert_eq!(stuck.len(), 2);
    assert_eq!(stuck[0].spec, "TASK-1");
    assert_eq!(stuck[1].spec, "TASK-2");

    let msg = format_nudge_body(&stuck);
    assert!(msg.contains("TASK-1: first retry park"), "{msg}");
    assert!(msg.contains("TASK-2: second retry park"), "{msg}");
    assert!(
        msg.contains("move: aida queue work TASK-1 --resume"),
        "{msg}"
    );
    assert!(!msg.contains("TASK-3"), "{msg}");
}

#[test]
fn nudge_dedupe_suppresses_same_fingerprint_within_window() {
    let item = StuckItem {
        spec: "TASK-1".to_string(),
        title: "retry park".to_string(),
        phase: "ci".to_string(),
        kind: "watchdog".to_string(),
        fingerprint: "TASK-1:1000:ci:watchdog".to_string(),
    };
    let mut state = NudgeState::default();
    record_nudged(&mut state, std::slice::from_ref(&item), 10_000);

    assert!(due_stuck_items(&[item.clone()], &state, 11_000, 30_000).is_empty());
    assert_eq!(
        due_stuck_items(&[item.clone()], &state, 41_000, 30_000),
        vec![item]
    );
}

#[test]
fn nudge_dedupe_allows_changed_state_immediately() {
    let old = StuckItem {
        spec: "TASK-1".to_string(),
        title: "retry park".to_string(),
        phase: "ci".to_string(),
        kind: "watchdog".to_string(),
        fingerprint: "TASK-1:1000:ci:watchdog".to_string(),
    };
    let changed = StuckItem {
        fingerprint: "TASK-1:2000:reviewer:watchdog".to_string(),
        phase: "reviewer".to_string(),
        ..old.clone()
    };
    let mut state = NudgeState::default();
    record_nudged(&mut state, &[old], 10_000);

    assert_eq!(
        due_stuck_items(&[changed.clone()], &state, 11_000, 30_000),
        vec![changed]
    );
}
