//! History index (history.db) tests: parity with the git walk, window
//! rules, catch-up / back-fill / reset mechanics, fail-open behavior, lock
//! independence, location, diagnostics and decoder-version discipline.
//!
//! Every test builds its own throwaway git store under a temp dir and
//! points the index at an explicit path inside that temp dir; nothing here
//! touches `~/.aida`, the live store, or any shared location.
// trace:TASK-1507 | ai:claude

use crate::history::{collect_filtered_events_git, Event, EventKind, HistoryOpts};
use crate::history_cache::{self, test_support, Budget};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// 2026-05-28T20:26:40Z — commits advance one minute at a time from here.
const BASE_TS: i64 = 1_780_000_000;
const GENEROUS: Duration = Duration::from_secs(120);

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Spec {
    id: &'static str,
    title: String,
    req_type: &'static str,
    status: String,
    priority: String,
    owner: String,
    feature: String,
    description: String,
    modified_at: String,
    last_modified_by: Option<String>,
    tags: Vec<String>,
    comments: Vec<(String, String)>,
    relationships: usize,
}

impl Spec {
    fn new(id: &'static str, req_type: &'static str, title: &str) -> Self {
        Spec {
            id,
            title: title.to_string(),
            req_type,
            status: "Draft".into(),
            priority: "Medium".into(),
            owner: String::new(),
            feature: "Core".into(),
            description: "first".into(),
            modified_at: "2026-05-28T00:00:00Z".into(),
            last_modified_by: None,
            tags: Vec::new(),
            comments: Vec::new(),
            relationships: 0,
        }
    }

    fn yaml(&self) -> String {
        let mut y = format!(
            "spec_id: {}\ntitle: {:?}\nreq_type: {}\nstatus: {}\npriority: {}\nowner: {:?}\n\
             feature: {:?}\ndescription: {:?}\nmodified_at: {:?}\n",
            self.id,
            self.title,
            self.req_type,
            self.status,
            self.priority,
            self.owner,
            self.feature,
            self.description,
            self.modified_at
        );
        if let Some(by) = &self.last_modified_by {
            y.push_str(&format!("last_modified_by: {by:?}\n"));
        }
        y.push_str("tags:\n");
        for t in &self.tags {
            y.push_str(&format!("  - {t:?}\n"));
        }
        y.push_str("comments:\n");
        for (a, t) in &self.comments {
            y.push_str(&format!("  - author: {a:?}\n    text: {t:?}\n"));
        }
        y.push_str("relationships:\n");
        for i in 0..self.relationships {
            y.push_str(&format!("  - rel: depends-on\n    target: X-{i}\n"));
        }
        y
    }
}

struct Fixture {
    tmp: tempfile::TempDir,
    store: PathBuf,
    db: PathBuf,
    clock: i64,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join("store");
        std::fs::create_dir_all(&store).unwrap();
        let idx = tmp.path().join("idx");
        std::fs::create_dir_all(&idx).unwrap();
        let db = idx.join(history_cache::history_db_file_name());
        let fx = Fixture {
            tmp,
            store,
            db,
            clock: BASE_TS,
        };
        fx.git(&["init", "-q", "-b", "aida-store"]);
        fx.git(&["config", "user.email", "fixture@example.com"]);
        fx.git(&["config", "user.name", "Fixture"]);
        fx.git(&["config", "commit.gpgsign", "false"]);
        fx
    }

    fn git(&self, args: &[&str]) -> String {
        git_in(&self.store, args, self.clock)
    }

    fn put(&self, spec: &Spec) {
        let rel = aida_core::object_store::relative_object_path(spec.id).unwrap();
        let path = self.store.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, spec.yaml()).unwrap();
    }

    fn remove(&self, id: &str) {
        let rel = aida_core::object_store::relative_object_path(id).unwrap();
        std::fs::remove_file(self.store.join(rel)).unwrap();
    }

    fn commit(&mut self, msg: &str) -> String {
        self.clock += 60;
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", msg]);
        self.head()
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    fn commit_ts(&self, rev: &str) -> i64 {
        self.git(&["show", "-s", "--format=%ct", rev])
            .trim()
            .parse()
            .unwrap()
    }
}

fn git_in(dir: &Path, args: &[&str], clock: i64) -> String {
    let date = format!("@{clock} +0000");
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", &date)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn rfc3339(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .unwrap()
        .to_rfc3339()
}

fn opts() -> HistoryOpts {
    HistoryOpts {
        limit: 1000,
        max_commits: 1000,
        max_commits_explicit: false,
        events_mode: true,
        id_filter: None,
        type_filter: None,
        author_filter: None,
        since: None,
        until: None,
        status_changes_only: false,
        shipped_only: false,
        comments_only: false,
        oneline: false,
        archived_specs: HashSet::new(),
        archived_only_specs: None,
        deferred_specs: HashSet::new(),
        deferred_only_specs: None,
        exclude_meta: false,
    }
}

/// A linear store exercising add / modify / delete / status / ship /
/// comment / tag / owner / feature / relationships / META rows, a commit
/// that touches no object, a commit that touches an object but decodes to
/// no events, and a run of status flips for window tests. Returns the
/// SHAs in commit order.
fn build_linear(fx: &mut Fixture) -> Vec<String> {
    let mut shas = Vec::new();
    let mut fr = Spec::new("FR-1", "Functional", "Login flow");
    fr.priority = "High".into();
    fr.last_modified_by = Some("alice".into());
    let mut task = Spec::new("TASK-2", "Task", "Wire the login button");
    task.last_modified_by = Some("bob".into());
    let mut meta = Spec::new("META-3", "Meta", "Prompt template");
    fx.put(&fr);
    fx.put(&task);
    fx.put(&meta);
    shas.push(fx.commit("add FR-1 TASK-2 META-3"));

    fr.status = "Approved".into();
    fr.priority = "Critical".into();
    fx.put(&fr);
    shas.push(fx.commit("update FR-1"));

    task.comments.push(("alice".into(), "looks good".into()));
    task.tags = vec!["ui".into(), "login".into()];
    task.last_modified_by = Some("Bob".into());
    fx.put(&task);
    shas.push(fx.commit("update TASK-2"));

    fr.status = "Done".into();
    fx.put(&fr);
    shas.push(fx.commit("update FR-1"));

    fr.status = "Completed".into();
    fx.put(&fr);
    shas.push(fx.commit("ship FR-1"));

    // A delete plus a near-identical add: git would pair these as a rename.
    fx.remove("TASK-2");
    let mut bug = task.clone();
    bug.id = "BUG-5";
    bug.req_type = "Bug";
    fx.put(&bug);
    shas.push(fx.commit("chore: update 1 requirements, delete 1"));

    std::fs::write(fx.store.join("metadata.yaml"), "version: 2\n").unwrap();
    shas.push(fx.commit("chore: metadata only"));

    // Touches FR-1 but changes nothing the decoder reports.
    fr.modified_at = "2026-05-29T00:00:00Z".into();
    fx.put(&fr);
    shas.push(fx.commit("touch FR-1"));

    fr.title = "Login and logout flow".into();
    fr.owner = "carol".into();
    fr.feature = "Auth".into();
    fr.description = "second".into();
    bug.relationships = 2;
    bug.tags = vec!["ui".into(), "regression".into()];
    meta.status = "Approved".into();
    fx.put(&fr);
    fx.put(&bug);
    fx.put(&meta);
    shas.push(fx.commit("chore: update requirements store"));

    bug.status = "Done".into();
    bug.comments.push(("carol".into(), "fixed".into()));
    bug.comments.push(("dave".into(), "verified".into()));
    bug.last_modified_by = Some("carol".into());
    fx.put(&bug);
    shas.push(fx.commit("update BUG-5"));

    bug.status = "Completed".into();
    fx.put(&bug);
    shas.push(fx.commit("ship BUG-5"));

    let mut flip = Spec::new("TASK-8", "Task", "Flip-flop");
    fx.put(&flip);
    shas.push(fx.commit("add TASK-8"));
    for i in 0..12 {
        flip.status = if i % 2 == 0 { "Approved" } else { "Draft" }.into();
        fx.put(&flip);
        shas.push(fx.commit("update TASK-8"));
    }
    shas
}

/// Two side branches merged back: one merge resolves a conflict (so its
/// combined diff lists the file), one merges cleanly.
fn build_merged(fx: &mut Fixture) -> Vec<String> {
    let mut fr = Spec::new("FR-1", "Functional", "Base");
    let mut task = Spec::new("TASK-2", "Task", "Shared");
    fx.put(&fr);
    fx.put(&task);
    fx.commit("add FR-1 TASK-2");
    fr.status = "Approved".into();
    fx.put(&fr);
    let base = fx.commit("update FR-1");

    fx.git(&["checkout", "-q", "-b", "side"]);
    let mut side_task = task.clone();
    side_task.status = "Approved".into();
    side_task.title = "Shared (side)".into();
    fx.put(&side_task);
    fx.commit("side: update TASK-2");
    let bug = Spec::new("BUG-9", "Bug", "Side bug");
    fx.put(&bug);
    fx.commit("side: add BUG-9");
    let mut bug2 = bug.clone();
    bug2.status = "Done".into();
    fx.put(&bug2);
    fx.commit("side: update BUG-9");

    fx.git(&["checkout", "-q", "aida-store"]);
    task.priority = "High".into();
    task.title = "Shared (main)".into();
    fx.put(&task);
    fx.commit("main: update TASK-2");
    fr.status = "Done".into();
    fx.put(&fr);
    fx.commit("main: update FR-1");

    // Conflicting merge, resolved by hand.
    fx.clock += 60;
    let out = Command::new("git")
        .arg("-C")
        .arg(&fx.store)
        .args(["merge", "--no-ff", "--no-edit", "side"])
        .env("GIT_AUTHOR_DATE", format!("@{} +0000", fx.clock))
        .env("GIT_COMMITTER_DATE", format!("@{} +0000", fx.clock))
        .env("HOME", &fx.store)
        .output()
        .unwrap();
    assert!(!out.status.success(), "fixture merge should conflict");
    let mut resolved = task.clone();
    resolved.title = "Shared (merged)".into();
    resolved.status = "Approved".into();
    resolved.comments.push(("erin".into(), "merged".into()));
    fx.put(&resolved);
    fx.git(&["add", "-A"]);
    fx.git(&["commit", "-q", "--no-edit"]);

    // A clean merge of a second branch.
    fx.git(&["checkout", "-q", "-b", "side2", &base]);
    let story = Spec::new("STORY-4", "Story", "Clean side");
    fx.put(&story);
    fx.commit("side2: add STORY-4");
    fx.git(&["checkout", "-q", "aida-store"]);
    fx.clock += 60;
    fx.git(&["merge", "--no-ff", "--no-edit", "side2"]);
    fr.status = "Completed".into();
    fx.put(&fr);
    fx.commit("ship FR-1");
    fx.git(&["rev-list", "HEAD"])
        .lines()
        .map(String::from)
        .collect()
}

fn serve(fx: &Fixture, o: &HistoryOpts) -> Option<history_cache::CacheAnswer> {
    history_cache::serve_at(&fx.store, &fx.db, o, GENEROUS).unwrap()
}

fn assert_parity(fx: &Fixture, o: &HistoryOpts, label: &str) {
    let (events, hidden, exhausted) = collect_filtered_events_git(&fx.store, o).unwrap();
    let got = serve(fx, o).unwrap_or_else(|| panic!("[{label}] index did not serve"));
    assert_eq!(got.events, events, "[{label}] events differ");
    assert_eq!(
        got.hidden_archived, hidden,
        "[{label}] hidden count differs"
    );
    assert_eq!(
        got.window_exhausted, exhausted,
        "[{label}] window_exhausted differs"
    );
}

fn assert_query_only_parity(fx: &Fixture, o: &HistoryOpts, label: &str) {
    let (events, _, exhausted) = collect_filtered_events_git(&fx.store, o).unwrap();
    let got = test_support::query_only(&fx.store, &fx.db, o)
        .unwrap()
        .unwrap_or_else(|| panic!("[{label}] partial index did not serve"));
    assert_eq!(got.events, events, "[{label}] events differ");
    assert_eq!(
        got.window_exhausted, exhausted,
        "[{label}] window_exhausted differs"
    );
}

fn by_commit(events: &[Event]) -> BTreeMap<String, Vec<Event>> {
    let mut m: BTreeMap<String, Vec<Event>> = BTreeMap::new();
    for e in events {
        m.entry(e.sha.clone()).or_default().push(e.clone());
    }
    m
}

// ---------------------------------------------------------------------------
// Parity with the git walk
// ---------------------------------------------------------------------------

#[test]
fn history_cache_parity_with_git_walk_all_filters() {
    let mut fx = Fixture::new();
    let shas = build_linear(&mut fx);
    let since = rfc3339(fx.commit_ts(&shas[3]));
    let until = rfc3339(fx.commit_ts(&shas[9]));

    let mut cases: Vec<(&str, HistoryOpts)> = vec![("base", opts())];
    let mut push = |label: &'static str, f: &dyn Fn(&mut HistoryOpts)| {
        let mut o = opts();
        f(&mut o);
        cases.push((label, o));
    };
    push("limit 1", &|o| o.limit = 1);
    push("limit 4", &|o| o.limit = 4);
    push("id FR-1", &|o| o.id_filter = Some("FR-1".into()));
    push("id fr-1 lowercase", &|o| o.id_filter = Some("fr-1".into()));
    push("id TASK-2 (deleted)", &|o| {
        o.id_filter = Some("TASK-2".into())
    });
    push("type task", &|o| o.type_filter = Some("task".into()));
    push("type FR", &|o| o.type_filter = Some("FR".into()));
    push("author ali", &|o| o.author_filter = Some("ali".into()));
    push("author Bob", &|o| o.author_filter = Some("Bob".into()));
    push("author bob", &|o| o.author_filter = Some("bob".into()));
    push("status changes", &|o| o.status_changes_only = true);
    push("comments", &|o| o.comments_only = true);
    push("status + comments", &|o| {
        o.status_changes_only = true;
        o.comments_only = true;
    });
    push("shipped", &|o| o.shipped_only = true);
    push("shipped limit 1", &|o| {
        o.shipped_only = true;
        o.limit = 1;
    });
    push("exclude meta", &|o| o.exclude_meta = true);
    push("archived hidden", &|o| {
        o.archived_specs = ["TASK-8".to_string()].into_iter().collect()
    });
    push("archived only", &|o| {
        o.archived_only_specs = Some(["FR-1".to_string()].into_iter().collect())
    });
    push("deferred hidden", &|o| {
        o.deferred_specs = ["BUG-5".to_string()].into_iter().collect()
    });
    push("deferred only", &|o| {
        o.deferred_only_specs = Some(["BUG-5".to_string()].into_iter().collect())
    });
    push("id + status + limit", &|o| {
        o.id_filter = Some("FR-1".into());
        o.status_changes_only = true;
        o.limit = 2;
    });
    push("type + author + shipped", &|o| {
        o.type_filter = Some("bug".into());
        o.author_filter = Some("carol".into());
        o.shipped_only = true;
    });
    for (label, o) in &cases {
        assert_parity(&fx, o, label);
    }

    // Time windows: bounds are inclusive commit times.
    let mut o = opts();
    o.since = Some(since.clone());
    assert_parity(&fx, &o, "since");
    o.until = Some(until.clone());
    assert_parity(&fx, &o, "since + until");
    o.since = None;
    assert_parity(&fx, &o, "until");
    o.since = Some(since);
    o.id_filter = Some("FR-1".into());
    assert_parity(&fx, &o, "since + until + id");
}

#[test]
fn history_cache_max_commits_window_parity() {
    let mut fx = Fixture::new();
    build_linear(&mut fx);
    for max_commits in [0usize, 1, 2, 3, 5, 12, 24, 25, 26, 100] {
        for limit in [1usize, 3, 50] {
            for shipped in [false, true] {
                let mut o = opts();
                o.max_commits = max_commits;
                o.max_commits_explicit = true;
                o.limit = limit;
                o.shipped_only = shipped;
                assert_parity(
                    &fx,
                    &o,
                    &format!("max_commits={max_commits} limit={limit} shipped={shipped}"),
                );
            }
        }
    }
    // The window really runs out on this fixture for a narrow window.
    let mut o = opts();
    o.max_commits = 3;
    o.limit = 50;
    assert!(serve(&fx, &o).unwrap().window_exhausted);
}

#[test]
fn history_cache_id_window_parity() {
    let mut fx = Fixture::new();
    build_linear(&mut fx);
    // FR-1 has a commit that touches it but decodes to no events, so an
    // `--id` window must count touching commits, not event rows.
    for max_commits in 1usize..=8 {
        for limit in [1usize, 2, 50] {
            let mut o = opts();
            o.id_filter = Some("FR-1".into());
            o.max_commits = max_commits;
            o.limit = limit;
            assert_parity(
                &fx,
                &o,
                &format!("id FR-1 max_commits={max_commits} limit={limit}"),
            );
        }
    }
}

#[test]
fn history_cache_merge_commit_decodes_like_git_show_cc() {
    let mut fx = Fixture::new();
    build_merged(&mut fx);
    let o = opts();
    let (walk, _, _) = collect_filtered_events_git(&fx.store, &o).unwrap();
    let got = serve(&fx, &o).expect("index serves a complete merged history");
    // On merge histories the index's topological order is authoritative;
    // the bar is the same events, in the same order within each commit.
    assert_eq!(by_commit(&got.events), by_commit(&walk));
    let merges: Vec<String> = fx
        .git(&["rev-list", "--merges", "HEAD"])
        .lines()
        .map(String::from)
        .collect();
    assert_eq!(merges.len(), 2);
    let conflicted = walk
        .iter()
        .filter(|e| merges.contains(&e.sha))
        .collect::<Vec<_>>();
    assert!(
        !conflicted.is_empty(),
        "the conflict-resolving merge must decode to events"
    );
    assert!(conflicted.iter().all(|e| e.spec_id == "TASK-2"));
}

// ---------------------------------------------------------------------------
// Catch-up, back-fill and reset mechanics
// ---------------------------------------------------------------------------

#[test]
fn history_cache_incremental_catch_up_from_tip() {
    let mut fx = Fixture::new();
    build_linear(&mut fx);
    assert_parity(&fx, &opts(), "initial");
    let built_at = test_support::meta(&fx.db, "built_at");
    let before = test_support::commit_count(&fx.db);

    let mut s = Spec::new("STORY-11", "Story", "Later work");
    fx.put(&s);
    fx.commit("add STORY-11");
    s.status = "Approved".into();
    fx.put(&s);
    let head = fx.commit("update STORY-11");

    assert_parity(&fx, &opts(), "after catch-up");
    assert_eq!(test_support::commit_count(&fx.db), before + 2);
    assert_eq!(test_support::meta(&fx.db, "tip_sha"), Some(head));
    assert_eq!(
        test_support::meta(&fx.db, "built_at"),
        built_at,
        "catch-up appends; it does not rebuild"
    );
    assert_eq!(test_support::meta(&fx.db, "last_reset_reason"), None);
    assert!(test_support::meta(&fx.db, "last_append_at").is_some());
    assert!(test_support::meta(&fx.db, "last_event_at").is_some());
}

#[test]
fn history_cache_budgeted_catch_up_commits_only_an_ancestor_closed_prefix() {
    let mut fx = Fixture::new();
    build_linear(&mut fx);
    assert_parity(&fx, &opts(), "initial");
    let old_tip = fx.head();
    let mut s = Spec::new("STORY-12", "Story", "Budgeted");
    let mut added = Vec::new();
    for status in ["Draft", "Approved", "Done", "Draft", "Approved"] {
        s.status = status.into();
        fx.put(&s);
        added.push(fx.commit("update STORY-12"));
    }
    // A zero budget indexes one commit, then must stop at a valid boundary.
    test_support::index(&fx.store, &fx.db, Budget::for_duration(Duration::ZERO)).unwrap();
    let tip = test_support::meta(&fx.db, "tip_sha").unwrap();
    assert_ne!(tip, fx.head(), "a zero budget cannot reach HEAD");
    assert_ne!(tip, old_tip, "the first commit is kept, not rolled back");
    assert_eq!(tip, added[0]);
    // Behind HEAD: the reader falls back rather than serve a partial answer.
    assert!(test_support::query_only(&fx.store, &fx.db, &opts())
        .unwrap()
        .is_none());
    // An unbudgeted pass finishes the gap.
    assert_parity(&fx, &opts(), "after finishing");
}

#[test]
fn history_cache_rebuilds_on_non_ancestor_tip() {
    let mut fx = Fixture::new();
    build_linear(&mut fx);
    assert_parity(&fx, &opts(), "initial");
    fx.git(&["reset", "-q", "--hard", "HEAD~3"]);
    let s = Spec::new("STORY-13", "Story", "After a force-reset");
    fx.put(&s);
    fx.commit("add STORY-13");
    assert_parity(&fx, &opts(), "after rewrite");
    assert_eq!(
        test_support::meta(&fx.db, "last_reset_reason").as_deref(),
        Some("store history was rewritten")
    );
    let expected = fx.git(&["rev-list", "--count", "HEAD"]).trim().to_string();
    assert_eq!(
        test_support::commit_count(&fx.db).to_string(),
        expected,
        "rewritten commits must not linger"
    );
}

#[test]
fn history_cache_rebuilds_after_store_compact_squash() {
    let mut fx = Fixture::new();
    build_linear(&mut fx);
    assert_parity(&fx, &opts(), "initial");
    // What `store compact --squash` leaves: one root commit, same tree.
    let squashed = fx
        .git(&["commit-tree", "HEAD^{tree}", "-m", "squashed"])
        .trim()
        .to_string();
    fx.git(&["reset", "-q", "--soft", &squashed]);
    assert_parity(&fx, &opts(), "after squash");
    assert_eq!(test_support::commit_count(&fx.db), 1);
    assert!(test_support::meta(&fx.db, "last_reset_reason")
        .unwrap()
        .contains("root commit changed"));
}

#[test]
fn history_cache_rebuilds_on_schema_or_decoder_version_bump() {
    let mut fx = Fixture::new();
    build_linear(&mut fx);
    assert_parity(&fx, &opts(), "initial");
    for key in ["decoder_version", "schema_version"] {
        test_support::set_meta(&fx.db, key, "0");
        // A stale version never serves, even without re-indexing.
        assert!(test_support::query_only(&fx.store, &fx.db, &opts())
            .unwrap()
            .is_none());
        assert_parity(&fx, &opts(), key);
        assert_eq!(
            test_support::meta(&fx.db, "last_reset_reason").as_deref(),
            Some("index format changed")
        );
    }
}

#[test]
fn history_cache_rebuilds_on_foreign_store_root() {
    let mut a = Fixture::new();
    build_linear(&mut a);
    assert_parity(&a, &opts(), "store A");
    let mut b = Fixture::new();
    let s = Spec::new("EPIC-20", "Epic", "Unrelated store");
    b.put(&s);
    b.commit("add EPIC-20");
    // Point store B at store A's index file.
    let o = opts();
    let (walk, _, _) = collect_filtered_events_git(&b.store, &o).unwrap();
    let got = history_cache::serve_at(&b.store, &a.db, &o, GENEROUS)
        .unwrap()
        .expect("served after reset");
    assert_eq!(got.events, walk);
    assert!(test_support::meta(&a.db, "last_reset_reason")
        .unwrap()
        .contains("root commit changed"));
    assert_eq!(test_support::commit_count(&a.db), 1);
}

#[test]
fn history_cache_partial_backfill_serves_covered_window_only() {
    let mut fx = Fixture::new();
    let shas = build_linear(&mut fx);
    test_support::partial_build(&fx.store, &fx.db, 5, 1).unwrap();
    assert_eq!(test_support::commit_count(&fx.db), 5);
    assert_eq!(test_support::meta(&fx.db, "complete").as_deref(), Some("0"));

    // `limit` matches found inside the indexed commits.
    let mut o = opts();
    o.limit = 2;
    assert_query_only_parity(&fx, &o, "limit met");
    // The whole window (plus the one-commit probe) lies inside the index.
    let mut o = opts();
    o.max_commits = 3;
    o.max_commits_explicit = true;
    assert_query_only_parity(&fx, &o, "window inside");
    // `--since` newer than the oldest indexed commit.
    let mut o = opts();
    o.since = Some(rfc3339(fx.commit_ts(&shas[shas.len() - 3])));
    assert_query_only_parity(&fx, &o, "since inside");
}

#[test]
fn history_cache_partial_backfill_falls_back_when_uncovered() {
    let mut fx = Fixture::new();
    build_linear(&mut fx);
    test_support::partial_build(&fx.store, &fx.db, 5, 1).unwrap();
    // Needs more than the indexed commits: never a partial answer.
    for (label, o) in [
        ("everything", opts()),
        ("shipped", {
            let mut o = opts();
            o.shipped_only = true;
            o
        }),
        ("window past the floor", {
            let mut o = opts();
            o.max_commits = 5;
            o
        }),
    ] {
        assert!(
            test_support::query_only(&fx.store, &fx.db, &o)
                .unwrap()
                .is_none(),
            "[{label}] must fall back"
        );
    }
}

#[test]
fn history_cache_backfill_resumes_across_a_merged_side_branch() {
    let mut fx = Fixture::new();
    build_merged(&mut fx);
    let all: HashSet<String> = fx
        .git(&["rev-list", "HEAD"])
        .lines()
        .map(String::from)
        .collect();
    let mut rounds = 0;
    loop {
        test_support::partial_build(&fx.store, &fx.db, 2, 1).unwrap();
        rounds += 1;
        let indexed = test_support::indexed_shas(&fx.db);
        let unique: HashSet<String> = indexed.iter().cloned().collect();
        assert_eq!(unique.len(), indexed.len(), "no commit indexed twice");
        assert!(unique.is_subset(&all));
        if test_support::meta(&fx.db, "complete").as_deref() == Some("1") {
            assert_eq!(unique, all, "complete means every commit is indexed");
            break;
        }
        assert!(unique.len() < all.len());
        assert!(rounds < 50, "back-fill never completed");
    }
    assert!(rounds > 3, "the fixture must span several chunk boundaries");
    let o = opts();
    let (walk, _, _) = collect_filtered_events_git(&fx.store, &o).unwrap();
    let got = serve(&fx, &o).unwrap();
    assert_eq!(by_commit(&got.events), by_commit(&walk));
}

// ---------------------------------------------------------------------------
// Fail-open behavior
// ---------------------------------------------------------------------------

#[test]
fn history_cache_corrupt_file_self_heals_and_falls_back() {
    let mut fx = Fixture::new();
    build_linear(&mut fx);
    std::fs::write(&fx.db, "this is not a sqlite database\n".repeat(200)).unwrap();
    let first = history_cache::serve_at(&fx.store, &fx.db, &opts(), GENEROUS);
    assert!(first.is_err(), "a corrupt index must not serve");
    assert!(!fx.db.exists(), "the corrupt file is removed");
    assert_parity(&fx, &opts(), "after self-heal");
}

#[cfg(unix)]
#[test]
fn history_cache_unwritable_dir_falls_back_to_git_walk() {
    use std::os::unix::fs::PermissionsExt;
    let mut fx = Fixture::new();
    build_linear(&mut fx);
    let ro = fx.tmp.path().join("readonly");
    std::fs::create_dir_all(&ro).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(ro.join("probe"), "x").is_ok() {
        // Running with privileges that ignore permissions: nothing to test.
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let db = ro.join(history_cache::history_db_file_name());
    let res = history_cache::serve_at(&fx.store, &db, &opts(), GENEROUS);
    assert!(res.is_err(), "an unwritable location cannot serve");
    // The caller's fallback still answers.
    assert!(!collect_filtered_events_git(&fx.store, &opts())
        .unwrap()
        .0
        .is_empty());
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn history_cache_disabled_by_env() {
    assert!(history_cache::cache_enabled_from(None));
    assert!(history_cache::cache_enabled_from(Some("1")));
    assert!(history_cache::cache_enabled_from(Some("")));
    for off in ["0", "false", "OFF", " no "] {
        assert!(!history_cache::cache_enabled_from(Some(off)), "{off}");
    }
    assert_eq!(
        history_cache::budget_from(None),
        Duration::from_millis(1500)
    );
    assert_eq!(
        history_cache::budget_from(Some("250")),
        Duration::from_millis(250)
    );
    assert_eq!(
        history_cache::budget_from(Some("soon")),
        Duration::from_millis(1500)
    );
}

// ---------------------------------------------------------------------------
// Lock independence and concurrency
// ---------------------------------------------------------------------------

#[test]
fn history_cache_ignores_requirements_cache_and_store_locks() {
    use fs2::FileExt;
    let mut fx = Fixture::new();
    build_linear(&mut fx);
    // A project layout: <proj>/.aida next to the store.
    let proj = fx.tmp.path().to_path_buf();
    let aida = proj.join(".aida");
    std::fs::create_dir_all(&aida).unwrap();
    let db = history_cache::history_db_path(&fx.store);
    assert_eq!(
        db,
        aida.canonicalize()
            .unwrap()
            .join(history_cache::history_db_file_name())
    );

    // Hold the requirements cache's write transaction and its lock-info
    // sidecar, and the store write lock (both the project and store copies).
    let cache = rusqlite::Connection::open(aida.join("cache.db")).unwrap();
    cache
        .execute_batch("CREATE TABLE t (x); BEGIN IMMEDIATE; INSERT INTO t VALUES (1);")
        .unwrap();
    std::fs::write(aida.join("cache.db.lock-info"), "pid=1 holder=test\n").unwrap();
    let mut held = Vec::new();
    for dir in [aida.clone(), fx.store.join(".aida")] {
        std::fs::create_dir_all(&dir).unwrap();
        let f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("store-write.lock"))
            .unwrap();
        f.lock_exclusive().unwrap();
        held.push(f);
    }

    // Index once, then move HEAD so the call under the locks has to write
    // (catch up) as well as read. A path that waited on any of these locks
    // would block for their whole multi-second timeout ladder, or fail.
    history_cache::serve_at(&fx.store, &db, &opts(), GENEROUS)
        .unwrap()
        .expect("initial build");
    let s = Spec::new("STORY-16", "Story", "Written while locked");
    fx.put(&s);
    fx.commit("add STORY-16");
    let (walk, _, _) = collect_filtered_events_git(&fx.store, &opts()).unwrap();
    let started = Instant::now();
    let got = history_cache::serve_at(&fx.store, &db, &opts(), GENEROUS)
        .unwrap()
        .expect("served while other locks are held");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "history must not wait on other locks (took {:?})",
        started.elapsed()
    );
    assert_eq!(got.events, walk);
    drop(held);
    cache.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn history_cache_concurrent_indexer_serves_snapshot_or_falls_back() {
    let mut fx = Fixture::new();
    build_linear(&mut fx);
    assert_parity(&fx, &opts(), "initial");
    let lock = test_support::hold_lock(&fx.db);
    // Another process is indexing, and the snapshot is current: serve it.
    let started = Instant::now();
    assert!(serve(&fx, &opts()).is_some());
    // HEAD moved and this reader cannot index: fall back, without waiting.
    let s = Spec::new("STORY-14", "Story", "While locked");
    fx.put(&s);
    fx.commit("add STORY-14");
    assert!(serve(&fx, &opts()).is_none());
    assert!(started.elapsed() < Duration::from_secs(5));
    drop(lock);
    assert_parity(&fx, &opts(), "after the lock is released");
}

// ---------------------------------------------------------------------------
// Location
// ---------------------------------------------------------------------------

#[test]
fn history_db_path_is_shared_through_a_symlinked_store() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(proj.join(".aida")).unwrap();
    std::fs::create_dir_all(proj.join(".aida-store")).unwrap();
    let canonical = history_cache::history_db_path(&proj.join(".aida-store"));
    assert_eq!(
        canonical,
        proj.canonicalize()
            .unwrap()
            .join(".aida")
            .join(history_cache::history_db_file_name())
    );
    assert!(canonical
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("history-v"));

    #[cfg(unix)]
    {
        // A sibling session worktree reaches the store through a symlink
        // and has its own `.aida`; it must still share the canonical index.
        let wt = tmp.path().join("wt");
        std::fs::create_dir_all(wt.join(".aida")).unwrap();
        std::os::unix::fs::symlink(proj.join(".aida-store"), wt.join(".aida-store")).unwrap();
        assert_eq!(
            history_cache::history_db_path(&wt.join(".aida-store")),
            canonical
        );
    }
}

#[test]
fn history_db_path_falls_back_to_a_sibling_and_never_adopts_a_temp_root() {
    let tmp = tempfile::tempdir().unwrap();
    let fake_temp_root = tmp.path().to_path_buf();
    // An ambient `.aida` planted at the (fake) temp root must be ignored.
    std::fs::create_dir_all(fake_temp_root.join(".aida")).unwrap();
    let store = fake_temp_root.join("lone").join(".aida-store");
    std::fs::create_dir_all(&store).unwrap();
    let p = test_support::path_with_roots(&store, std::slice::from_ref(&fake_temp_root));
    let lone = fake_temp_root.join("lone").canonicalize().unwrap();
    assert_eq!(
        p,
        lone.join(format!(
            ".aida-store.{}",
            history_cache::history_db_file_name()
        ))
    );
    assert_ne!(p.parent().unwrap(), fake_temp_root.canonicalize().unwrap());
}

// ---------------------------------------------------------------------------
// Explicit rebuild, diagnostics, wiring
// ---------------------------------------------------------------------------

#[test]
fn cache_rebuild_history_builds_everything_and_prunes_other_versions() {
    let mut fx = Fixture::new();
    build_linear(&mut fx);
    let dir = fx.db.parent().unwrap().to_path_buf();
    for stale in [
        "history-v0-9.db",
        "history-v0-9.db-wal",
        "history-v0-9.db.lock",
    ] {
        std::fs::write(dir.join(stale), "old").unwrap();
    }
    std::fs::write(dir.join("cache.db"), "requirements cache").unwrap();
    let report = history_cache::rebuild_full_at(&fx.store, &fx.db).unwrap();
    let expected: i64 = fx
        .git(&["rev-list", "--count", "HEAD"])
        .trim()
        .parse()
        .unwrap();
    assert_eq!(report.commits, expected);
    assert!(report.events > 0);
    assert_eq!(report.pruned.len(), 3);
    assert!(dir.join("cache.db").exists(), "other files are untouched");
    assert_eq!(test_support::meta(&fx.db, "complete").as_deref(), Some("1"));
    assert_eq!(test_support::meta(&fx.db, "tip_sha"), Some(fx.head()));
    assert_parity(&fx, &opts(), "after rebuild");
}

#[test]
fn cache_status_reports_history_freshness() {
    let render = |fx: &Fixture| {
        crate::cache_cmd::history_status_lines(&history_cache::status_at(&fx.store, &fx.db))
            .join("\n")
    };
    let mut fx = Fixture::new();
    build_linear(&mut fx);
    assert!(render(&fx).contains("not built yet"));

    test_support::partial_build(&fx.store, &fx.db, 5, 1).unwrap();
    let partial = render(&fx);
    assert!(partial.contains("FILLING"), "{partial}");
    assert!(
        partial.contains("still filling in older history"),
        "{partial}"
    );

    history_cache::rebuild_full_at(&fx.store, &fx.db).unwrap();
    let fresh = render(&fx);
    assert!(fresh.contains("FRESH"), "{fresh}");
    assert!(fresh.contains("the start of the store history"), "{fresh}");
    assert!(fresh.contains("explicit rebuild"), "{fresh}");

    let s = Spec::new("STORY-15", "Story", "Moves HEAD");
    fx.put(&s);
    fx.commit("add STORY-15");
    let behind = render(&fx);
    assert!(behind.contains("BEHIND"), "{behind}");
    // Status never creates or repairs anything.
    assert!(test_support::meta(&fx.db, "tip_sha").is_some());
}

#[test]
fn collect_filtered_events_is_served_from_the_index_when_enabled() {
    let tmp = tempfile::tempdir().unwrap();
    let mut fx = Fixture::new();
    // Re-home the fixture store one level down so the fallback index file
    // lands inside this test's temp dir, not in the shared temp root.
    let store = tmp.path().join("proj").join("store");
    std::fs::create_dir_all(store.parent().unwrap()).unwrap();
    build_linear(&mut fx);
    std::fs::rename(&fx.store, &store).unwrap();
    fx.store = store.clone();

    let o = opts();
    let (walk, _, _) = collect_filtered_events_git(&store, &o).unwrap();
    history_cache::set_test_serve_enabled(true);
    let records = crate::history::collect_event_records(&store, &o);
    history_cache::set_test_serve_enabled(false);
    let (records, _) = records.unwrap();
    let expected: Vec<_> = walk
        .iter()
        .map(|e| serde_json::to_value(crate::history::event_record(e)).unwrap())
        .collect();
    let got: Vec<_> = records
        .iter()
        .map(|r| serde_json::to_value(r).unwrap())
        .collect();
    assert_eq!(got, expected);
    let db = history_cache::history_db_path(&store);
    assert!(db.starts_with(tmp.path().canonicalize().unwrap()));
    assert!(db.exists(), "the query built the index");
}

/// Decoder-version discipline: the serialized shape of every event kind is
/// pinned here. If this fails, the stored events no longer mean what the
/// index recorded: bump `HISTORY_DECODER_VERSION` and update the snapshot.
#[test]
fn history_decoder_version_matches_event_kind_shape() {
    let kinds = vec![
        EventKind::Added {
            title: "t".into(),
            req_type: "r".into(),
            priority: "p".into(),
        },
        EventKind::Deleted { title: "t".into() },
        EventKind::StatusChange {
            from: "a".into(),
            to: "b".into(),
        },
        EventKind::PriorityChange {
            from: "a".into(),
            to: "b".into(),
        },
        EventKind::TitleChange {
            from: "a".into(),
            to: "b".into(),
        },
        EventKind::DescriptionEdited,
        EventKind::OwnerChange {
            from: "a".into(),
            to: "b".into(),
        },
        EventKind::FeatureChange {
            from: "a".into(),
            to: "b".into(),
        },
        EventKind::TypeChange {
            from: "a".into(),
            to: "b".into(),
        },
        EventKind::TagsChange {
            added: vec!["x".into()],
            removed: vec!["y".into()],
        },
        EventKind::CommentsAdded {
            count: 2,
            author: Some("a".into()),
        },
        EventKind::RelationshipsChange {
            added: 1,
            removed: 0,
        },
    ];
    let snapshot = serde_json::to_string(&kinds).unwrap();
    const V1: &str = r#"[{"Added":{"title":"t","req_type":"r","priority":"p"}},{"Deleted":{"title":"t"}},{"StatusChange":{"from":"a","to":"b"}},{"PriorityChange":{"from":"a","to":"b"}},{"TitleChange":{"from":"a","to":"b"}},"DescriptionEdited",{"OwnerChange":{"from":"a","to":"b"}},{"FeatureChange":{"from":"a","to":"b"}},{"TypeChange":{"from":"a","to":"b"}},{"TagsChange":{"added":["x"],"removed":["y"]}},{"CommentsAdded":{"count":2,"author":"a"}},{"RelationshipsChange":{"added":1,"removed":0}}]"#;
    assert_eq!(
        (history_cache::HISTORY_DECODER_VERSION, snapshot.as_str()),
        (1, V1),
        "EventKind's serialized shape changed: bump HISTORY_DECODER_VERSION \
         and record the new shape here"
    );
    // Round-trip: stored events decode back to the same values.
    let back: Vec<EventKind> = serde_json::from_str(&snapshot).unwrap();
    assert_eq!(back, kinds);
}
