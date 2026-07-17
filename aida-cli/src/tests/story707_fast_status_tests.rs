use super::*;

// The pure counter excludes META + standing-artifact types, treats
// completed/rejected/released as terminal (not "open"), and matches the
// in-progress / draft statuses case-insensitively across the Debug-form and
// hyphen/underscore variants the cache may store. trace:STORY-707
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
        // Excluded entirely — not real / standing-artifact types.
        ("Draft", "Meta"),
        ("Approved", "Vision"),
        ("InProgress", "Principle"),
    ];
    let c = fast_status_counts(rows.iter().map(|(s, t)| (*s, *t)));
    // 7 real rows counted (3 META/standing excluded).
    assert_eq!(c.total, 7);
    // open = not terminal: Draft, Approved, InProgress, in-progress = 4.
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
                 archived INTEGER NOT NULL DEFAULT 0
             );
             INSERT INTO requirements_cache VALUES ('a','Draft','Story',0);
             INSERT INTO requirements_cache VALUES ('b','InProgress','Task',0);
             INSERT INTO requirements_cache VALUES ('c','Completed','Bug',0);
             -- archived rows are excluded by the WHERE archived = 0 filter
             INSERT INTO requirements_cache VALUES ('d','Draft','Story',1);
             -- META is excluded by the pure counter
             INSERT INTO requirements_cache VALUES ('e','Draft','Meta',0);",
    )
    .unwrap();
    drop(conn);

    let c = fast_status_counts_from_cache(&cache_path);
    // Counted: a (Draft/Story), b (InProgress/Task), c (Completed/Bug).
    assert_eq!(c.total, 3);
    assert_eq!(c.open, 2); // a + b (c is terminal)
    assert_eq!(c.in_progress, 1); // b
    assert_eq!(c.draft, 1); // a
}

// A missing cache yields zeroed counts (no panic) — the fresh-`aida init`
// case where no read has built `.aida/cache.db` yet. trace:STORY-707
#[test]
fn fast_status_counts_from_cache_absent_is_zeroed() {
    let dir = tempfile::tempdir().unwrap();
    let c = fast_status_counts_from_cache(&dir.path().join("nope.db"));
    assert_eq!(c, FastStatusCounts::default());
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
                 archived INTEGER NOT NULL DEFAULT 0
             );
             INSERT INTO requirements_cache VALUES ('a','Approved','Story',0);
             INSERT INTO requirements_cache VALUES ('b','InProgress','Task',0);",
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
