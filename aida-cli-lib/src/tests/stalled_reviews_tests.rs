// trace:STORY-3095 | ai:codex
use super::*;

#[test]
fn story_3095_stalled_review_cases() {
    use VerdictKind::{Approved, RequestChanges};
    let classify = |kind, head, queued, green, train, late| {
        stall(kind, Some("abc0123456"), head, queued, green, train, late)
    };
    assert!(classify(RequestChanges, "abc0123456", false, false, false, false).is_none());
    assert!(classify(RequestChanges, "abc0123456", true, false, false, true).is_none());
    assert!(classify(RequestChanges, "abc0123456ff", false, false, false, true).is_none());
    assert!(classify(RequestChanges, "def0123456", false, false, false, true).is_none());
    assert!(classify(Approved, "abc0123456", false, true, false, true).is_some());
    assert!(classify(Approved, "abc0123456", false, true, true, true).is_none());
    assert!(classify(Approved, "abc0123456", false, false, false, true).is_none());
    assert!(classify(Approved, "abc0123456", false, true, false, false).is_none());
    assert!(classify(RequestChanges, "abc0123456", false, false, false, true).is_some());
}

#[test]
fn story_3095_read_only_scan() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let state = root.join(".aida");
    std::fs::create_dir(&state).unwrap();
    let store = root.join(".aida-store");
    std::fs::create_dir_all(store.join("objects")).unwrap();
    std::fs::write(store.join("metadata.yaml"), "{}").unwrap();
    use crate::gate::ledger::{ledger_dir, store::ledger_file, EventBody, GateEvent};
    let head = "a".repeat(40);
    let change = ChangeKey::pr(1);
    let ledger = ledger_file(root, change);
    std::fs::create_dir(ledger_dir(root)).unwrap();
    crate::gate::ledger::bindings::bootstrap(root).unwrap();
    let state_entries = std::fs::read_dir(&state).unwrap().count();
    let ledger_entries = std::fs::read_dir(ledger_dir(root)).unwrap().count();
    let scan_pr =
        |pr| scan_rows(root, Utc::now(), TimeDelta::minutes(60), vec![pr], false).unwrap();
    let scan = || {
        scan_pr(serde_json::json!({
            "number":1, "headRefOid":head, "headRefName":"fix", "title":"Fix review"
        }))
    };
    for (age, expected) in [(59, 0), (61, 1)] {
        let record = serde_json::json!({"verdict":"request-changes", "reviewed_sha":head});
        let mut event = GateEvent::new(
            change,
            EventBody::Verdict {
                record: record.as_object().unwrap().clone(),
            },
        );
        event.at = (Utc::now() - TimeDelta::minutes(age)).to_rfc3339();
        event.seq = 1;
        let contents = format!("{}\n", serde_json::to_string(&event).unwrap());
        std::fs::write(&ledger, &contents).unwrap();
        assert_eq!(scan().len(), expected);
        assert_eq!(std::fs::read_to_string(&ledger).unwrap(), contents);
    }
    assert_eq!(std::fs::read_dir(&state).unwrap().count(), state_entries);
    assert_eq!(
        std::fs::read_dir(ledger_dir(root)).unwrap().count(),
        ledger_entries + 1
    );
    assert_eq!(std::fs::read_dir(store).unwrap().count(), 2);
    let conflicting = || {
        scan_pr(serde_json::json!({
            "number":7, "headRefOid":head, "baseRefOid":"b".repeat(40), "mergeable":"CONFLICTING",
            "headRefName":"fix", "title":"Fix"
        }))
    };
    assert!(conflicting()[0].summary.contains("conflicting"));
    let sessions = crate::leases_dir(root);
    std::fs::create_dir(&sessions).unwrap();
    let lease = format!(
        r#"id = "running"
scope = "STORY-1"
slug = "fix"
owner = "tester"
branch = "fix"
worktree_path = "."
started_at = "{}"
hostname = "localhost"
role = "implementer"
active_pid = {}
"#,
        Utc::now().to_rfc3339(),
        std::process::id()
    );
    std::fs::write(sessions.join("running.toml"), &lease).unwrap();
    assert!(scan().is_empty());
    let blocked = "blocked: reservation repair needed";
    let active = format!("active_pid = {}", std::process::id());
    let dead = lease.replace(&active, "active_pid = 2147483647");
    std::fs::write(sessions.join("running.toml"), dead).unwrap();
    assert!(conflicting()[0].summary.contains(blocked));
    std::fs::remove_file(sessions.join("running.toml")).unwrap();
    let reservation = serde_json::json!({"version":1,"entries":[{
        "key":{"repository":root,"pr":7,"original_head":head}, "dispatched_by":"host",
        "reserved_at":"now","lane_id":"retained","tree_path":root,"branch":"fix",
        "base":"b".repeat(40),"spawn":"unknown","original_conflict_paths":[]
    }]});
    std::fs::write(state.join("repair-journal.json"), reservation.to_string()).unwrap();
    assert!(conflicting()[0].summary.contains(blocked));
}

#[test]
fn story_3095_green_since_last_completion() {
    let first = "2026-11-24T09:00:00Z";
    let last = "2026-11-24T10:00:00Z";
    let mut rows = serde_json::json!([
        {"context":"legacy","state":"SUCCESS","createdAt": last},
        {"name":"build","conclusion":"SUCCESS","completedAt": first}
    ]);
    assert_eq!(
        crate::land::green_rollup_at(&rows),
        Some(DateTime::parse_from_rfc3339(last).unwrap())
    );
    rows[1]["completedAt"] = serde_json::Value::Null;
    assert!(crate::land::green_rollup_at(&rows).is_none());
}

// trace:TASK-3878 | ai:codex
#[test]
fn conflict_repair_queue_and_delivery_receipts_deduplicate() {
    assert!(should_queue(false, false, false).unwrap());
    assert!(!should_queue(true, false, true).unwrap());
    assert!(!should_queue(false, true, true).unwrap());
    assert!(should_queue(false, false, true).is_err());
}
