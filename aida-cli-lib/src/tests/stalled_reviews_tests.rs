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
    use aida_core::exec_context::{Environment, ExecContext, SystemClock, ToolPaths};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let state = root.join(".aida");
    std::fs::create_dir(&state).unwrap();
    let store = root.join(".aida-store");
    std::fs::create_dir_all(store.join("objects")).unwrap();
    std::fs::write(store.join("metadata.yaml"), "{}").unwrap();
    let exec = ExecContext::new(
        Environment::default(),
        None,
        root.into(),
        root.into(),
        ToolPaths::default(),
        std::sync::Arc::new(SystemClock::default()),
    );
    use crate::gate::ledger::{ledger_dir, store::ledger_file, EventBody, GateEvent};
    let head = "a".repeat(40);
    let change = ChangeKey::pr(1);
    let ledger = ledger_file(root, change);
    std::fs::create_dir(ledger_dir(root)).unwrap();
    crate::gate::ledger::bindings::bootstrap(root).unwrap();
    let state_entries = std::fs::read_dir(&state).unwrap().count();
    let ledger_entries = std::fs::read_dir(ledger_dir(root)).unwrap().count();
    let scan = || {
        scan_open(&exec, TimeDelta::minutes(60), |_| {
            Ok(vec![serde_json::json!({
                "number":1, "headRefOid":head, "headRefName":"fix", "title":"Fix review"
            })])
        })
        .unwrap()
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
    std::fs::write(sessions.join("running.toml"), lease).unwrap();
    assert!(scan().is_empty());
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

// A rework lane covers a spec by queued work, or a retained implementer
// lease by scope (any ID spelling) or by the PR's branch.
// trace:STORY-3131 | ai:claude
#[test]
fn story_3131_rework_lanes_cover_queued_or_leased_work() {
    let req = |spec: &str| {
        let mut r = aida_core::models::Requirement::new(format!("Title {spec}"), String::new());
        r.spec_id = Some(spec.into());
        r
    };
    let mut store = aida_core::models::RequirementsStore::new();
    let queued = req("STORY-1");
    store.requirements.push(queued.clone());
    store.requirements.push(req("STORY-2"));
    let lease = |scope: &str, branch: &str| -> crate::leases::SessionLease {
        serde_json::from_value(serde_json::json!({
            "id": "abc123abc123", "scope": scope, "slug": scope, "owner": "t",
            "worktree_path": "/fixture", "branch": branch,
            "started_at": "2026-11-26T00:00:00Z", "hostname": "h",
        }))
        .unwrap()
    };
    let lanes = ReworkLanes {
        queued: [queued.id].into_iter().collect(),
        active: vec![
            lease("story-3", "story-3-work"),
            lease("TASK-9", "fix-branch"),
        ],
    };
    assert!(lanes.covers(&store, "STORY-1", "x"), "queued work");
    assert!(!lanes.covers(&store, "STORY-2", "x"), "known spec, no lane");
    assert!(
        lanes.covers(&store, "STORY-3", "x"),
        "lease scope, any case"
    );
    assert!(
        lanes.covers(&store, "STORY-4", "fix-branch"),
        "lease branch"
    );
    assert!(!lanes.covers(&store, "STORY-4", "other"));
    let none = ReworkLanes {
        queued: Default::default(),
        active: Vec::new(),
    };
    assert!(!none.covers(&store, "STORY-1", "story-3-work"));
}
