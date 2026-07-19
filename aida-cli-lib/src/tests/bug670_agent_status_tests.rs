use super::*;

fn line_index(lines: &[String], key: &str) -> Option<usize> {
    lines.iter().position(|l| l.starts_with(&format!("{key}:")))
}

// BUG-675: even when the raw depth DIFFERS from actionable (a padded queue),
// `queue_depth` is now emitted — trustworthy total-routed context — but always
// AFTER the lead `queue_actionable` work-signal.
#[test]
fn divergent_depth_rides_along_after_actionable() {
    let snap = FastStatusSnapshot {
        role: "implementer".to_string(),
        queue_depth: 64,
        queue_actionable: 0,
        cache_present: true,
        ..Default::default()
    };
    let lines = toon_status_scalar_lines(&snap);

    let actionable_at =
        line_index(&lines, "queue_actionable").expect("queue_actionable must be emitted");
    // Lead signal appears…
    assert!(
        lines.iter().any(|l| l == "queue_actionable: 0"),
        "actionable count must be the work-signal: {lines:?}"
    );
    // …and the (now trustworthy) raw depth rides along after it.
    let depth_at = line_index(&lines, "queue_depth")
        .expect("BUG-675: trustworthy queue_depth is always emitted");
    assert!(lines.iter().any(|l| l == "queue_depth: 64"));
    assert!(
        actionable_at < depth_at,
        "queue_actionable must lead queue_depth: {lines:?}"
    );
    // "Leads" = actionable precedes every requirement-count scalar.
    for k in ["open", "in_progress", "draft", "total"] {
        if let Some(i) = line_index(&lines, k) {
            assert!(actionable_at < i, "queue_actionable must precede {k}");
        }
    }
}

// When the two counts AGREE, `queue_depth` still rides along AFTER the lead
// `queue_actionable`, and never before it.
#[test]
fn agreeing_depth_rides_along_after_actionable() {
    let snap = FastStatusSnapshot {
        role: "implementer".to_string(),
        queue_depth: 3,
        queue_actionable: 3,
        cache_present: true,
        ..Default::default()
    };
    let lines = toon_status_scalar_lines(&snap);
    let actionable_at = line_index(&lines, "queue_actionable").unwrap();
    let depth_at = line_index(&lines, "queue_depth").expect("agreeing depth may be emitted");
    assert!(
        actionable_at < depth_at,
        "queue_actionable must lead queue_depth: {lines:?}"
    );
    assert!(lines.iter().any(|l| l == "queue_actionable: 3"));
    assert!(lines.iter().any(|l| l == "queue_depth: 3"));
}

// Consistency-on-a-fixture: `role_queue_actionable` (which feeds the lead
// `queue_actionable` scalar) filters a padded role queue down to the SAME
// live/workable set `aida queue list` would surface — archived / completed /
// draft / done corpses drop out — while the raw depth stays padded. Proves
// the status lead-signal and the queue-list count agree on whether there is
// actionable work.
#[test]
fn actionable_matches_queue_list_on_a_padded_fixture() {
    // BUG-698: this test resolves the queue owner via `current_user_id`
    // (which reads $AIDA_USER/$USER) to WRITE the fixture, then
    // `read_queue_depth` re-resolves it to READ — an env-derived read at two
    // points. Without the ONE unified env lock, a sibling test's env swap
    // (or the documented `setenv` realloc race) between them makes the read
    // resolve a different user, so `<user>.yaml` isn't found and
    // `read_queue_depth` returns None → the rare CI flake. BUG-697 unified
    // the env WRITERS; this reader was missed. Hold the lock across the
    // whole window. trace:BUG-698 (follow-up BUG-697) | ai:claude
    let _env = crate::test_env::env_lock();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let user = current_user_id(None);

    // A role queue padded with corpses: one live Approved spec plus a
    // Completed, an archived-Approved, and a Draft that queue list also hides.
    let queue_dir = root.join(".aida-store/registry/queues");
    std::fs::create_dir_all(&queue_dir).unwrap();
    std::fs::write(
        queue_dir.join(format!("{user}.yaml")),
        "- requirement_id: u-live\n  for_role: implementer\n  position: 0\n\
             - requirement_id: u-done\n  for_role: implementer\n  position: 1\n\
             - requirement_id: u-archived\n  for_role: implementer\n  position: 2\n\
             - requirement_id: u-draft\n  for_role: implementer\n  position: 3\n",
    )
    .unwrap();

    // Cache with the liveness fields role_queue_actionable reads.
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    let cache_path = root.join(".aida/cache.db");
    let conn = rusqlite::Connection::open(&cache_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE requirements_cache (
                 id TEXT PRIMARY KEY NOT NULL,
                 agreed_id TEXT,
                 spec_id TEXT,
                 title TEXT NOT NULL,
                 status TEXT NOT NULL,
                 archived INTEGER NOT NULL DEFAULT 0,
                 deferred INTEGER NOT NULL DEFAULT 0,
                 blocked INTEGER NOT NULL DEFAULT 0
             );
             INSERT INTO requirements_cache VALUES
                 ('u-live','STORY-1',NULL,'Live one','Approved',0,0,0),
                 ('u-done','STORY-2',NULL,'Shipped','Completed',0,0,0),
                 ('u-archived','STORY-3',NULL,'Old one','Approved',1,0,0),
                 ('u-draft','STORY-4',NULL,'Not yet','Draft',0,0,0);",
    )
    .unwrap();
    drop(conn);

    // Raw depth is padded (all four routed entries); actionable resolves to
    // exactly the one live/workable spec — the queue-list-consistent count.
    let depth = read_queue_depth(root, Some("implementer")).unwrap();
    assert_eq!(depth, 4, "raw depth counts every routed entry");
    let actionable = role_queue_actionable(root, "implementer");
    assert_eq!(
        actionable,
        vec![(
            "STORY-1".to_string(),
            "Live one".to_string(),
            "Approved".to_string()
        )],
        "actionable filters the padded queue to the live/workable set"
    );

    // Fed into the projection, the actionable count leads and the (now
    // identity-trustworthy, BUG-675) raw depth rides along after it — both are
    // emitted, actionable first.
    let snap = FastStatusSnapshot {
        role: "implementer".to_string(),
        queue_depth: depth,
        queue_actionable: actionable.len(),
        cache_present: true,
        ..Default::default()
    };
    let lines = toon_status_scalar_lines(&snap);
    assert!(lines.iter().any(|l| l == "queue_actionable: 1"));
    assert!(lines.iter().any(|l| l == "queue_depth: 4"));
    let actionable_at = line_index(&lines, "queue_actionable").unwrap();
    let depth_at = line_index(&lines, "queue_depth").unwrap();
    assert!(
        actionable_at < depth_at,
        "queue_actionable must lead queue_depth: {lines:?}"
    );
}

// BUG-675: the statusline `queue_depth` must resolve the current user through
// the SAME case-folding identity path `aida queue list` uses (TASK-951), so a
// shell whose USER/AIDA_USER differs only in case from the stored queue key
// counts the same items — never suppressed to zero on a resolvable identity
// match. trace:BUG-675 | ai:claude
#[test]
fn queue_depth_matches_queue_list_under_case_only_identity() {
    // Pin identity to `Joe` (upper) under the env lock; the stored queue lives
    // under lowercase `joe` (a prior shell's casing). EnvVarsGuard holds the
    // ENV_LOCK for the whole body so no sibling env-mutating test races in.
    let _g = crate::test_env::EnvVarsGuard::set(&[("AIDA_USER", "Joe"), ("USER", "Joe")]);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let store = root.join(".aida-store");
    let qdir = store.join("registry/queues");
    std::fs::create_dir_all(&qdir).unwrap();

    let mk = |role: &str, pos: i64| aida_core::QueueEntry {
        user_id: "joe".to_string(),
        requirement_id: Uuid::new_v4(),
        position: pos,
        added_by: "joe".to_string(),
        note: None,
        added_at: chrono::Utc::now(),
        for_role: Some(role.to_string()),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    };
    let entries = vec![
        mk("implementer", 0),
        mk("implementer", 1),
        mk("reviewer", 2),
    ];
    std::fs::write(
        qdir.join("joe.yaml"),
        serde_yaml::to_string(&entries).unwrap(),
    )
    .unwrap();

    // Reference path: `aida queue list` resolves `Joe` → `joe.yaml` (TASK-951)
    // and returns all three entries; two are routed to implementer.
    let listed = Storage::new(&store).queue_list("Joe", false).unwrap();
    let list_impl = listed
        .iter()
        .filter(|e| e.for_role.as_deref() == Some("implementer"))
        .count();
    assert_eq!(
        list_impl, 2,
        "queue list must fold identity and see the queue"
    );

    // BUG-675: the statusline depth resolves identity the SAME way and EQUALS
    // the queue-list count — never suppressed to zero on a resolvable match.
    let depth = read_queue_depth(root, Some("implementer"))
        .expect("queue_depth must resolve the folded identity, not read zero");
    assert_eq!(
        depth, list_impl,
        "queue_depth must equal the queue-list count under a case-only identity mismatch"
    );
    assert_ne!(
        depth, 0,
        "queue_depth must not collapse to zero on a resolvable identity match"
    );
}
