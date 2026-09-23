use super::*;

// The pure counter excludes META + standing-artifact types, counts only
// list-open statuses as "open", and matches the in-progress / draft statuses
// case-insensitively across the Debug-form and hyphen/underscore variants the
// cache may store. trace:STORY-707
#[test]
fn fast_status_counts_partitions_open_inprogress_draft() {
    let rows = vec![
        ("Draft", "Story"),
        ("Approved", "Task"),
        ("InProgress", "Bug"),
        ("in-progress", "Feature"),
        ("Completed", "Story"),
        ("Rejected", "Task"),
        ("Released", "Story"),
        ("Done", "Bug"),
        ("Superseded", "Decision"),
        // Excluded entirely — not real / standing-artifact types.
        ("Draft", "Meta"),
        ("Approved", "Vision"),
        ("InProgress", "Principle"),
    ];
    let c = fast_status_counts(rows.iter().map(|(s, t)| (*s, *t)));
    // 9 real rows counted (3 META/standing excluded).
    assert_eq!(c.total, 9);
    // open = list-open statuses: Draft, Approved, InProgress, in-progress = 4.
    assert_eq!(c.open, 4);
    // in_progress matches "InProgress" + "in-progress" = 2.
    assert_eq!(c.in_progress, 2);
    // draft = 1 (the Meta draft is excluded).
    assert_eq!(c.draft, 1);
}

#[test]
fn fast_status_counts_empty_is_zeroed() {
    let c = fast_status_counts(std::iter::empty());
    assert_eq!(c, FastStatusCounts::default());
}

#[test]
fn fast_status_counts_with_defer_matches_list_open_lens() {
    let rows = [
        ("Draft", "Story", false, "[]"),
        ("Approved", "Task", false, "[]"),
        ("InProgress", "Bug", true, "[]"),
        ("InProgress", "Bug", false, r#"["deferred:on-demand"]"#),
        ("NeedsAttention", "Bug", false, "[]"),
        ("Done", "Bug", false, "[]"),
        ("Completed", "Story", false, "[]"),
        ("Approved", "Decision", false, "[]"),
    ];

    let c = fast_status_counts_with_defer(rows);
    assert_eq!(
        c.total, 6,
        "deferred rows are outside the active work lens; closed/accepted rows remain counted"
    );
    assert_eq!(
        c.open, 3,
        "open = list-open statuses minus deferred rows and accepted decisions"
    );
    assert_eq!(c.in_progress, 0, "deferred in-progress work is parked");
}

// Build a real cache DB with known rows and assert the fast snapshot reads
// its counts FROM THE CACHE — `fast_status_counts_from_cache` opens the
// sqlite read-only, never `backend.load()`. trace:STORY-707
#[test]
fn fast_status_counts_from_cache_reads_only_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let cache_path = dir.path().join("cache.db");
    let conn = rusqlite::Connection::open(&cache_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE requirements_cache (
                 id TEXT PRIMARY KEY NOT NULL,
                 status TEXT NOT NULL,
                 req_type TEXT NOT NULL,
                 tags_json TEXT NOT NULL DEFAULT '[]',
                 archived INTEGER NOT NULL DEFAULT 0,
                 deferred INTEGER NOT NULL DEFAULT 0
             );
             INSERT INTO requirements_cache VALUES ('a','Draft','Story','[]',0,0);
             INSERT INTO requirements_cache VALUES ('b','InProgress','Task','[]',0,0);
             INSERT INTO requirements_cache VALUES ('c','Completed','Bug','[]',0,0);
             -- archived rows are excluded by the WHERE archived = 0 filter
             INSERT INTO requirements_cache VALUES ('d','Draft','Story','[]',1,0);
             -- META is excluded by the pure counter
             INSERT INTO requirements_cache VALUES ('e','Draft','Meta','[]',0,0);
             -- deferred rows are parked outside the list-open lens
             INSERT INTO requirements_cache VALUES ('f','InProgress','Story','[]',0,1);
             -- legacy deferred:* tags are the same parked shelf
             INSERT INTO requirements_cache VALUES ('h','InProgress','Story','[\"deferred:on-demand\"]',0,0);
             -- Done is no longer open, even though it is not terminal for all guards
             INSERT INTO requirements_cache VALUES ('g','Done','Bug','[]',0,0);",
    )
    .unwrap();
    drop(conn);

    let (c, by_status) = fast_status_counts_from_cache(&cache_path);
    // Counted: a, b, c, g. d archived, e META, f/h deferred.
    assert_eq!(c.total, 4);
    assert_eq!(c.open, 2); // a + b (c is terminal)
    assert_eq!(c.in_progress, 1); // b
    assert_eq!(c.draft, 1); // a
                            // BUG-1503: the by_status breakdown sums to the same total, over the
                            // same row set (d/e/f/h excluded), keyed by the raw cache status string.
    assert_eq!(
        by_status,
        std::collections::BTreeMap::from([
            ("Draft".to_string(), 1),
            ("InProgress".to_string(), 1),
            ("Completed".to_string(), 1),
            ("Done".to_string(), 1),
        ])
    );
}

// A missing cache yields zeroed counts (no panic) — the fresh-`aida init`
// case where no read has built `.aida/cache.db` yet. trace:STORY-707
#[test]
fn fast_status_counts_from_cache_absent_is_zeroed() {
    let dir = tempfile::tempdir().unwrap();
    let (c, by_status) = fast_status_counts_from_cache(&dir.path().join("nope.db"));
    assert_eq!(c, FastStatusCounts::default());
    assert!(by_status.is_empty());
}

// The fast snapshot's collector takes ONLY a project_root — it has no
// `CachedGitBackend` parameter and never loads the full store. This test
// runs it against a dir with NO `.aida-store/objects` (so a full
// `backend.load()` would FAIL), proving the snapshot is sourced purely from
// the read-only cache + cheap fs/git reads. trace:STORY-707
#[test]
fn collect_fast_status_snapshot_works_without_a_loadable_store() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    let cache_path = dir.path().join(".aida/cache.db");
    let conn = rusqlite::Connection::open(&cache_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE requirements_cache (
                 id TEXT PRIMARY KEY NOT NULL,
                 status TEXT NOT NULL,
                 req_type TEXT NOT NULL,
                 tags_json TEXT NOT NULL DEFAULT '[]',
                 archived INTEGER NOT NULL DEFAULT 0,
                 deferred INTEGER NOT NULL DEFAULT 0
             );
             INSERT INTO requirements_cache VALUES ('a','Approved','Story','[]',0,0);
             INSERT INTO requirements_cache VALUES ('b','InProgress','Task','[]',0,0);",
    )
    .unwrap();
    drop(conn);

    // No .aida-store/objects → no loadable full store. The snapshot must
    // still come back populated from the cache.
    let snap = collect_fast_status_snapshot(dir.path());
    assert!(snap.cache_present);
    assert_eq!(snap.counts.total, 2);
    assert_eq!(snap.counts.open, 2);
    assert_eq!(snap.counts.in_progress, 1);
}
