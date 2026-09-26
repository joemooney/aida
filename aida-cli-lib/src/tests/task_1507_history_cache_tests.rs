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
    /// How custom edges are written: the legacy `!Custom name` tag (false)
    /// or the current `{custom: name}` mapping (true). Flipping it models
    /// a store-wide format rewrite, which must decode to no edge change.
    // trace:BUG-1631 | ai:claude
    rel_mapping_form: bool,
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
            rel_mapping_form: false,
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
        // BUG-1631: real `rel_type`/`target_id` edges in every stored form
        // (plain, tagged custom, mapping custom) so index/walk parity
        // covers relationship edges. trace:BUG-1631 | ai:claude
        for i in 0..self.relationships {
            // The first edge is the custom one, so every spec's first
            // edge add or last edge removal exercises the custom forms.
            let rel_type = match i % 3 {
                0 if self.rel_mapping_form => "\n      custom: depends-on".to_string(),
                0 => "!Custom depends-on".to_string(),
                1 => "Parent".to_string(),
                _ => "References".to_string(),
            };
            y.push_str(&format!(
                "  - rel_type: {rel_type}\n    target_id: 00000000-0000-4000-8000-{i:012}\n"
            ));
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

    /// Commit with committer/author time exactly `ts`.
    fn commit_at(&mut self, ts: i64, msg: &str) -> String {
        self.clock = ts - 60;
        self.commit(msg)
    }

    /// `git merge --no-ff <branch>` at time `ts`, with extra merge args.
    fn merge_at(&mut self, ts: i64, branch: &str, extra: &[&str]) -> String {
        self.clock = ts;
        let mut args = vec!["merge", "-q", "--no-ff", "--no-edit"];
        args.extend_from_slice(extra);
        args.push(branch);
        self.git(&args);
        self.head()
    }

    fn drop_index(&self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.db.display()));
        }
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
        to_status: None,
        from_status: None,
        opened_only: false,
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
    // Date-priority order: the index reproduces the walk exactly, merges
    // included.
    assert_eq!(got.events, walk);
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
    // What `store compact --squash` leaves: one root commit, same tree,
    // and a backup branch at the old tip (which must not keep it "live").
    fx.git(&["branch", "aida-store-pre-squash-1", "HEAD"]);
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
    let records = records.unwrap().events;
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
            edges: vec![crate::history::RelEdge {
                added: true,
                rel_type: "Parent".into(),
                target_id: "u".into(),
                target: None,
            }],
        },
    ];
    let snapshot = serde_json::to_string(&kinds).unwrap();
    // trace:BUG-1631 | ai:claude
    const V4: &str = r#"[{"Added":{"title":"t","req_type":"r","priority":"p"}},{"Deleted":{"title":"t"}},{"StatusChange":{"from":"a","to":"b"}},{"PriorityChange":{"from":"a","to":"b"}},{"TitleChange":{"from":"a","to":"b"}},"DescriptionEdited",{"OwnerChange":{"from":"a","to":"b"}},{"FeatureChange":{"from":"a","to":"b"}},{"TypeChange":{"from":"a","to":"b"}},{"TagsChange":{"added":["x"],"removed":["y"]}},{"CommentsAdded":{"count":2,"author":"a"}},{"RelationshipsChange":{"added":1,"removed":0,"edges":[{"added":true,"rel_type":"Parent","target_id":"u"}]}}]"#;
    assert_eq!(
        (history_cache::HISTORY_DECODER_VERSION, snapshot.as_str()),
        (4, V4),
        "EventKind's serialized shape changed: bump HISTORY_DECODER_VERSION \
         and record the new shape here"
    );
    // Round-trip: stored events decode back to the same values.
    let back: Vec<EventKind> = serde_json::from_str(&snapshot).unwrap();
    assert_eq!(back, kinds);
}

// ---------------------------------------------------------------------------
// Rework: exact since-eligibility (B1), date-priority order and coverage
// (R1-R5), `--id` across merges (N1b, now BUG-1620), stale readers (N2), no
// checkpoint on close (N3), lock-safe prune (N4).
// trace:TASK-1507 | ai:claude
// ---------------------------------------------------------------------------

const STATUSES: [&str; 3] = ["Approved", "Done", "Draft"];

/// Base commit, then a side branch and a main line with the given commit
/// times (offsets from `BASE_TS`), a merge at `merge`, and main commits at
/// `after`. Every commit decodes to events; side and main touch different
/// specs so the merge is clean.
fn build_dated_merge(fx: &mut Fixture, side: &[i64], main: &[i64], merge: i64, after: &[i64]) {
    let base = Spec::new("EPIC-1", "Epic", "Base");
    fx.put(&base);
    fx.commit_at(BASE_TS, "add EPIC-1");
    fx.git(&["checkout", "-q", "-b", "side"]);
    let mut sb = Spec::new("BUG-2", "Bug", "Side work");
    for (i, off) in side.iter().enumerate() {
        if i > 0 {
            sb.status = STATUSES[i % 3].into();
        }
        fx.put(&sb);
        fx.commit_at(BASE_TS + off, "side: BUG-2");
    }
    fx.git(&["checkout", "-q", "aida-store"]);
    let mut mf = Spec::new("FR-3", "Functional", "Main work");
    for (i, off) in main.iter().enumerate() {
        if i > 0 {
            mf.status = STATUSES[i % 3].into();
        }
        fx.put(&mf);
        fx.commit_at(BASE_TS + off, "main: FR-3");
    }
    fx.merge_at(BASE_TS + merge, "side", &[]);
    for (i, off) in after.iter().enumerate() {
        mf.status = STATUSES[(i + main.len()) % 3].into();
        fx.put(&mf);
        fx.commit_at(BASE_TS + off, "main: FR-3 again");
    }
}

fn commit_times(fx: &Fixture) -> Vec<i64> {
    fx.git(&["log", "--format=%ct", "HEAD"])
        .lines()
        .map(|l| l.trim().parse().unwrap())
        .collect()
}

/// Every `--since` bound around every commit time, plus none.
fn since_probes(fx: &Fixture) -> Vec<(String, HistoryOpts)> {
    let mut out = vec![("no since".to_string(), opts())];
    for ts in commit_times(fx) {
        for t in [ts - 1, ts, ts + 1] {
            let mut o = opts();
            o.since = Some(rfc3339(t));
            out.push((format!("since {t}"), o));
        }
    }
    out
}

/// Every `-n` and every `--max-commits` cut (plus a few combined with a
/// filter).
fn cut_probes(fx: &Fixture) -> Vec<(String, HistoryOpts)> {
    let commits = commit_times(fx).len();
    let events = collect_filtered_events_git(&fx.store, &opts())
        .unwrap()
        .0
        .len();
    let mut out = Vec::new();
    for n in 0..=events + 1 {
        let mut o = opts();
        o.limit = n;
        out.push((format!("-n {n}"), o));
    }
    for m in 0..=commits + 1 {
        let mut o = opts();
        o.max_commits = m;
        o.max_commits_explicit = true;
        out.push((format!("--max-commits {m}"), o.clone()));
        o.limit = 2;
        out.push((format!("--max-commits {m} -n 2"), o.clone()));
        o.limit = 1000;
        o.status_changes_only = true;
        out.push((format!("--max-commits {m} --status-changes"), o));
    }
    out
}

type Walked = (Vec<Event>, bool);

fn walk_all(fx: &Fixture, probes: &[(String, HistoryOpts)]) -> Vec<Walked> {
    probes
        .iter()
        .map(|(_, o)| {
            let (e, _, x) = collect_filtered_events_git(&fx.store, o).unwrap();
            (e, x)
        })
        .collect()
}

/// A served answer must equal the walk exactly (order included).
fn assert_same(got: &history_cache::CacheAnswer, walk: &Walked, label: &str) {
    assert_eq!(
        got.events, walk.0,
        "[{label}] served events differ from the walk"
    );
    assert_eq!(
        got.window_exhausted, walk.1,
        "[{label}] window_exhausted differs"
    );
}

/// Build every partial index reachable with back-fill chunks of 1-3
/// commits and check every probe against the walk: it must match or fall
/// back. Returns (served, fell back).
fn check_partial_states(fx: &Fixture, probes: &[(String, HistoryOpts)]) -> (usize, usize) {
    let walks = walk_all(fx, probes);
    let total = commit_times(fx).len();
    let (mut served, mut fell_back) = (0, 0);
    for chunk in 1..=3usize {
        for chunks in 1..=total.div_ceil(chunk) {
            fx.drop_index();
            test_support::partial_build(&fx.store, &fx.db, chunk, chunks).unwrap();
            for ((label, o), walk) in probes.iter().zip(&walks) {
                let label = format!("chunk {chunk} x{chunks}: {label}");
                match test_support::query_only(&fx.store, &fx.db, o).unwrap() {
                    Some(got) => {
                        assert_same(&got, walk, &label);
                        served += 1;
                    }
                    None => fell_back += 1,
                }
            }
        }
    }
    fx.drop_index();
    (served, fell_back)
}

#[test]
fn task_1507_since_on_partial_index_with_old_side_branch_matches_or_falls_back() {
    // B1: the side branch predates every main-line commit, so topo-order
    // back-fill indexes old side commits before newer main-line ones.
    let mut fx = Fixture::new();
    build_dated_merge(&mut fx, &[10, 20, 30], &[1000, 1100, 1200, 1300], 1400, &[]);
    let probes = since_probes(&fx);
    let (served, fell_back) = check_partial_states(&fx, &probes);
    assert!(served > 0, "some partial states must serve");
    assert!(fell_back > 0, "some partial states must fall back");

    // The exact state the review reproduced: two chunks of two commits
    // hold the merge and side commits only; `--since` at a main commit
    // must fall back, not serve an empty answer.
    test_support::partial_build(&fx.store, &fx.db, 2, 2).unwrap();
    assert_eq!(
        test_support::meta(&fx.db, "unfilled_max_ts"),
        Some((BASE_TS + 1300).to_string())
    );
    let mut o = opts();
    o.since = Some(rfc3339(BASE_TS + 1100));
    assert!(test_support::query_only(&fx.store, &fx.db, &o)
        .unwrap()
        .is_none());
    // Newer than every unfilled commit: served, and correct.
    o.since = Some(rfc3339(BASE_TS + 1301));
    let got = test_support::query_only(&fx.store, &fx.db, &o)
        .unwrap()
        .expect("covered since serves");
    let (walk, _, x) = collect_filtered_events_git(&fx.store, &o).unwrap();
    assert_same(&got, &(walk, x), "since after the watermark");
}

#[test]
fn task_1507_interleaved_merge_every_cut_matches_walk_exactly() {
    // R1/R5: side and main commit dates interleave across the merge.
    let side = [150, 250, 350];
    let main = [100, 200, 300, 400];
    let after = [600, 700];

    // (i) a complete index.
    let mut fx = Fixture::new();
    build_dated_merge(&mut fx, &side, &main, 500, &after);
    let mut probes = cut_probes(&fx);
    probes.extend(since_probes(&fx));
    let walks = walk_all(&fx, &probes);
    history_cache::rebuild_full_at(&fx.store, &fx.db).unwrap();
    for ((label, o), walk) in probes.iter().zip(&walks) {
        let got = test_support::query_only(&fx.store, &fx.db, o)
            .unwrap()
            .unwrap_or_else(|| panic!("[complete: {label}] did not serve"));
        assert_same(&got, walk, &format!("complete: {label}"));
    }

    // (ii) an index built before the merge and caught up across it: the
    // side commits get higher seq than newer main-line commits.
    let mut fx = Fixture::new();
    build_dated_merge(&mut fx, &side, &main, 500, &after);
    let merged_head = fx.head();
    fx.git(&["checkout", "-q", "-b", "pre-merge", "HEAD~3"]);
    history_cache::rebuild_full_at(&fx.store, &fx.db).unwrap();
    fx.git(&["checkout", "-q", "aida-store"]);
    assert_eq!(fx.head(), merged_head);
    for ((label, o), walk) in probes.iter().zip(&walks) {
        let got = serve(&fx, o).unwrap_or_else(|| panic!("[catch-up: {label}] did not serve"));
        assert_same(&got, walk, &format!("catch-up: {label}"));
    }

    // (iii) every partial back-fill state.
    let (served, _) = check_partial_states(&fx, &probes);
    assert!(served > 0);
}

#[test]
fn task_1507_skewed_commit_falls_back_near_it_only() {
    // R3: B is older than its parent A, so git's walk shows B before A.
    let mut fx = Fixture::new();
    let mut s = Spec::new("FR-1", "Functional", "Skew");
    fx.put(&s);
    fx.commit_at(BASE_TS, "add FR-1");
    for (off, st) in [
        (1000, "Approved"),
        (500, "Done"),
        (2000, "Draft"),
        (2100, "Approved"),
    ] {
        s.status = st.into();
        fx.put(&s);
        fx.commit_at(BASE_TS + off, "update FR-1");
    }
    history_cache::rebuild_full_at(&fx.store, &fx.db).unwrap();
    let run = |o: &HistoryOpts| test_support::query_only(&fx.store, &fx.db, o).unwrap();
    let check = |o: &HistoryOpts, label: &str| {
        let (walk, _, x) = collect_filtered_events_git(&fx.store, o).unwrap();
        let got = run(o).unwrap_or_else(|| panic!("[{label}] did not serve"));
        assert_same(&got, &(walk, x), label);
    };

    // Unbounded, or reaching the skewed range: fall back.
    assert!(run(&opts()).is_none(), "unbounded");
    let mut o = opts();
    o.max_commits = 3;
    assert!(run(&o).is_none(), "window reaching the skewed parent");
    let mut o = opts();
    o.since = Some(rfc3339(BASE_TS + 1000));
    assert!(run(&o).is_none(), "since at the skewed parent");
    // Clear of it: served, exactly.
    let mut o = opts();
    o.max_commits = 2;
    check(&o, "window above the skew");
    let mut o = opts();
    o.limit = 1;
    check(&o, "-n 1");
    let mut o = opts();
    o.since = Some(rfc3339(BASE_TS + 1500));
    check(&o, "since above the skew");
}

#[test]
fn task_1507_boundary_tie_inside_merge_region_falls_back() {
    // B2 (was R4): a side and a main commit in the same second inside the merge's
    // parallel region; then a parent/child tie on one line after it.
    let mut fx = Fixture::new();
    build_dated_merge(&mut fx, &[300], &[300], 600, &[900, 900]);
    history_cache::rebuild_full_at(&fx.store, &fx.db).unwrap();
    let run = |o: &HistoryOpts| test_support::query_only(&fx.store, &fx.db, o).unwrap();
    let check = |o: &HistoryOpts, label: &str| {
        let (walk, _, x) = collect_filtered_events_git(&fx.store, o).unwrap();
        let got = run(o).unwrap_or_else(|| panic!("[{label}] did not serve"));
        assert_same(&got, &(walk, x), label);
    };
    // Order: 900 (child), 900 (parent), merge 600, then the 300 tie.
    let mut o = opts();
    o.max_commits = 4;
    assert!(run(&o).is_none(), "window boundary on the cross-branch tie");
    let mut o = opts();
    o.max_commits = 1;
    check(&o, "boundary on the same-line tie");
    let mut o = opts();
    o.max_commits = 3;
    check(&o, "boundary on the merge");
    // A limit cut landing on the cross-branch tie also falls back.
    let events_above = collect_filtered_events_git(&fx.store, &{
        let mut o = opts();
        o.max_commits = 3;
        o
    })
    .unwrap()
    .0
    .len();
    let mut o = opts();
    o.limit = events_above + 1;
    assert!(run(&o).is_none(), "-n boundary on the cross-branch tie");
}

/// A spec edited only on a side branch that an `-s ours` merge discards,
/// plus a main-line spec, and one untouched after the base. The side
/// branch's changes never reach HEAD's tree, so `git log -- <path>` without
/// `--full-history` simplifies the side commits away.
fn build_ours_merge(fx: &mut Fixture) {
    let mut bug = Spec::new("BUG-70", "Bug", "Both sides");
    let epic = Spec::new("EPIC-72", "Epic", "Untouched after the base");
    fx.put(&bug);
    fx.put(&epic);
    fx.commit_at(BASE_TS, "add BUG-70 EPIC-72");
    fx.git(&["checkout", "-q", "-b", "side"]);
    for (off, st) in [(100, "Approved"), (200, "Done")] {
        bug.status = st.into();
        fx.put(&bug);
        fx.commit_at(BASE_TS + off, "side: update BUG-70");
    }
    fx.git(&["checkout", "-q", "aida-store"]);
    let mut fr = Spec::new("FR-71", "Functional", "Main");
    fx.put(&fr);
    fx.commit_at(BASE_TS + 150, "main: add FR-71");
    fx.merge_at(BASE_TS + 300, "side", &["-s", "ours"]);
    fr.status = "Approved".into();
    fx.put(&fr);
    fx.commit_at(BASE_TS + 400, "main: update FR-71");
}

#[test]
fn bug_1620_id_on_an_ours_merged_side_branch_agrees_with_unfiltered_events() {
    // BUG-1620 (was N1b): the `--id` walk uses `--full-history`, so the
    // side commits of an `-s ours` merge appear under `--id` exactly as they
    // do in unfiltered `history events`, and the index serves `--id` on
    // merge-touched paths instead of routing them to the walk.
    // trace:BUG-1620 | ai:claude
    let mut fx = Fixture::new();
    build_ours_merge(&mut fx);
    let (all, _, _) = collect_filtered_events_git(&fx.store, &opts()).unwrap();
    for id in ["BUG-70", "FR-71", "EPIC-72"] {
        let mut o = opts();
        o.id_filter = Some(id.into());
        let (walk, _, _) = collect_filtered_events_git(&fx.store, &o).unwrap();
        let unfiltered: Vec<Event> = all.iter().filter(|e| e.spec_id == id).cloned().collect();
        assert_eq!(
            walk, unfiltered,
            "--id {id} must agree with unfiltered events"
        );
    }
    let mut o = opts();
    o.id_filter = Some("BUG-70".into());
    let (walk, _, _) = collect_filtered_events_git(&fx.store, &o).unwrap();
    assert!(
        walk.iter().any(|e| e.kind
            == EventKind::StatusChange {
                from: "Approved".into(),
                to: "Done".into()
            }),
        "the side-branch status change shows under --id"
    );

    history_cache::rebuild_full_at(&fx.store, &fx.db).unwrap();
    for id in ["BUG-70", "FR-71", "EPIC-72"] {
        let mut o = opts();
        o.id_filter = Some(id.into());
        assert_parity(&fx, &o, &format!("--id {id} across the ours merge"));
    }
    // Still served once HEAD moves past the merge.
    let mut epic = Spec::new("EPIC-72", "Epic", "Untouched after the base");
    epic.status = "Approved".into();
    fx.put(&epic);
    fx.commit_at(BASE_TS + 500, "main: update EPIC-72");
    let mut o = opts();
    o.id_filter = Some("EPIC-72".into());
    assert_parity(&fx, &o, "--id after catch-up");
    o.since = Some(rfc3339(BASE_TS + 450));
    assert_parity(&fx, &o, "--id above the merge");
}

#[test]
fn bug_1620_id_every_cut_across_merges_matches_the_walk() {
    // BUG-1620: every `-n`, `--max-commits` and `--since` cut of `--id` on
    // every spec, across an interleaved merge and an `-s ours` merge, on a
    // complete index, one caught up across the merges, and every partial
    // back-fill. A complete index must serve all of them (no merge
    // routing is left for `--id`); partial states match or fall back.
    // trace:BUG-1620 | ai:claude
    let mut fx = Fixture::new();
    build_dated_merge(
        &mut fx,
        &[150, 250, 350],
        &[100, 200, 300, 400],
        500,
        &[600],
    );
    // A second side branch, discarded by an ours merge.
    fx.git(&["checkout", "-q", "-b", "dropped"]);
    let mut b = Spec::new("BUG-2", "Bug", "Side work");
    b.status = "Rejected".into();
    fx.put(&b);
    fx.commit_at(BASE_TS + 650, "dropped: BUG-2");
    fx.git(&["checkout", "-q", "aida-store"]);
    let mut e = Spec::new("EPIC-1", "Epic", "Base");
    e.status = "Approved".into();
    fx.put(&e);
    fx.commit_at(BASE_TS + 700, "main: EPIC-1");
    fx.merge_at(BASE_TS + 800, "dropped", &["-s", "ours"]);
    e.status = "Done".into();
    fx.put(&e);
    fx.commit_at(BASE_TS + 900, "main: EPIC-1 done");

    let mut probes = Vec::new();
    for id in ["EPIC-1", "BUG-2", "FR-3"] {
        for (l, mut o) in all_probes(&fx) {
            o.id_filter = Some(id.into());
            probes.push((format!("--id {id} {l}"), o));
        }
    }
    let (served, fell_back) = check_complete(&fx, &probes, "bug-1620");
    assert_eq!(fell_back, 0, "a complete index serves every --id cut");
    assert!(served > 0);

    // Caught up across both merges from an index built before them.
    let walks = walk_all(&fx, &probes);
    let head = fx.head();
    fx.drop_index();
    fx.git(&["checkout", "-q", "-b", "pre-merges", "HEAD~6"]);
    history_cache::rebuild_full_at(&fx.store, &fx.db).unwrap();
    fx.git(&["checkout", "-q", "aida-store"]);
    assert_eq!(fx.head(), head);
    for ((label, o), walk) in probes.iter().zip(&walks) {
        let got = serve(&fx, o).unwrap_or_else(|| panic!("[catch-up: {label}] did not serve"));
        assert_same(&got, walk, &format!("catch-up: {label}"));
    }

    let (served, _) = check_partial_states(&fx, &probes);
    assert!(served > 0, "some partial states serve --id");
}

#[test]
fn task_1507_stale_or_diverged_reader_falls_back_without_reset() {
    // N2: a second checkout of the store behind the shared index.
    let mut fx = Fixture::new();
    build_linear(&mut fx);
    assert_parity(&fx, &opts(), "initial");
    let tip = fx.head();
    let count = test_support::commit_count(&fx.db);
    let built_at = test_support::meta(&fx.db, "built_at");
    let old = fx.tmp.path().join("old-checkout");
    fx.git(&[
        "worktree",
        "add",
        "-q",
        "--detach",
        old.to_str().unwrap(),
        "HEAD~3",
    ]);

    let unchanged = |label: &str| {
        assert_eq!(
            test_support::meta(&fx.db, "tip_sha"),
            Some(tip.clone()),
            "[{label}]"
        );
        assert_eq!(test_support::commit_count(&fx.db), count, "[{label}]");
        assert_eq!(
            test_support::meta(&fx.db, "built_at"),
            built_at,
            "[{label}]"
        );
        assert_eq!(
            test_support::meta(&fx.db, "last_reset_reason"),
            None,
            "[{label}]"
        );
    };
    // Behind the tip.
    assert!(history_cache::serve_at(&old, &fx.db, &opts(), GENEROUS)
        .unwrap()
        .is_none());
    unchanged("behind");
    // Diverged from the tip while the main checkout still has it.
    let s = Spec::new("STORY-30", "Story", "On the old checkout");
    let rel = aida_core::object_store::relative_object_path(s.id).unwrap();
    std::fs::create_dir_all(old.join(&rel).parent().unwrap()).unwrap();
    std::fs::write(old.join(&rel), s.yaml()).unwrap();
    git_in(&old, &["add", "-A"], fx.clock + 60);
    git_in(&old, &["commit", "-q", "-m", "diverge"], fx.clock + 60);
    assert!(history_cache::serve_at(&old, &fx.db, &opts(), GENEROUS)
        .unwrap()
        .is_none());
    unchanged("diverged");
    // The main checkout is still served from the untouched index.
    assert_parity(&fx, &opts(), "main checkout after stale readers");
    unchanged("main served");

    // A real rewrite (no checkout has the indexed tip any more) resets.
    fx.git(&["reset", "-q", "--hard", "HEAD~2"]);
    let s = Spec::new("STORY-31", "Story", "After a rewrite");
    fx.put(&s);
    fx.commit("add STORY-31");
    assert_parity(&fx, &opts(), "after rewrite");
    assert_eq!(
        test_support::meta(&fx.db, "last_reset_reason").as_deref(),
        Some("store history was rewritten")
    );
}

#[test]
fn task_1507_connection_close_skips_the_wal_checkpoint() {
    // N3: the WAL is left for SQLite's auto-checkpoint, not folded back
    // (with fsyncs) every time a query closes the connection.
    let mut fx = Fixture::new();
    build_linear(&mut fx);
    assert_parity(&fx, &opts(), "initial");
    let s = Spec::new("STORY-40", "Story", "One more");
    fx.put(&s);
    fx.commit("add STORY-40");
    assert_parity(&fx, &opts(), "after a one-commit catch-up");
    let wal = PathBuf::from(format!("{}-wal", fx.db.display()));
    assert!(
        wal.metadata().map(|m| m.len() > 0).unwrap_or(false),
        "the WAL survives the close"
    );
    // A fresh connection still sees every committed row.
    assert_eq!(test_support::meta(&fx.db, "tip_sha"), Some(fx.head()));
    // A bulk build leaves no WAL behind for later processes to re-read and
    // re-checkpoint.
    history_cache::rebuild_full_at(&fx.store, &fx.db).unwrap();
    assert_eq!(wal.metadata().map(|m| m.len()).unwrap_or(0), 0);
    assert_parity(&fx, &opts(), "after rebuild");
}

#[test]
fn task_1507_prune_skips_other_versions_whose_lock_is_held() {
    // N4: another `aida` version still indexing keeps its files.
    use fs2::FileExt;
    let mut fx = Fixture::new();
    build_linear(&mut fx);
    let dir = fx.db.parent().unwrap().to_path_buf();
    for f in [
        "history-v0-8.db",
        "history-v0-8.db-wal",
        "history-v0-8.db.lock",
        "history-v0-9.db",
        "history-v0-9.db.lock",
    ] {
        std::fs::write(dir.join(f), "old").unwrap();
    }
    let held = std::fs::OpenOptions::new()
        .write(true)
        .open(dir.join("history-v0-8.db.lock"))
        .unwrap();
    held.lock_exclusive().unwrap();
    let report = history_cache::rebuild_full_at(&fx.store, &fx.db).unwrap();
    let mut pruned: Vec<String> = report
        .pruned
        .iter()
        .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
        .collect();
    pruned.sort();
    assert_eq!(pruned, vec!["history-v0-9.db", "history-v0-9.db.lock"]);
    for f in [
        "history-v0-8.db",
        "history-v0-8.db-wal",
        "history-v0-8.db.lock",
    ] {
        assert!(dir.join(f).exists(), "{f} belongs to a running indexer");
    }
    drop(held);
}

// ---------------------------------------------------------------------------
// Second rework: cross-branch same-second ties anywhere in the served range
// (B2), ref-aware rewrite detection (N2a), and pinned guards (T1).
// trace:TASK-1507 | ai:claude
// ---------------------------------------------------------------------------

#[test]
fn task_1507_cross_branch_same_second_ties_fall_back_anywhere_in_range() {
    // The reviewer's fixtures: git orders same-second commits from the two
    // sides of a merge first-parent-first, which seq cannot express.
    for (side, main) in [
        (&[300][..], &[300][..]),
        (&[100, 300][..], &[200, 300][..]),
        (&[250, 400][..], &[100, 250][..]),
    ] {
        let mut fx = Fixture::new();
        build_dated_merge(&mut fx, side, main, 600, &[700]);
        history_cache::rebuild_full_at(&fx.store, &fx.db).unwrap();
        let run = |o: &HistoryOpts| test_support::query_only(&fx.store, &fx.db, o).unwrap();
        let label = format!("side {side:?} main {main:?}");
        assert!(run(&opts()).is_none(), "[{label}] unbounded");
        let mut o = opts();
        o.since = Some(rfc3339(BASE_TS + 50));
        assert!(run(&o).is_none(), "[{label}] --since below the tie");
        let mut o = opts();
        o.max_commits = 5;
        o.max_commits_explicit = true;
        assert!(run(&o).is_none(), "[{label}] --max-commits 5");
        // Clear of every tied second: served, exactly.
        for (what, o) in [
            ("--max-commits 2", {
                let mut o = opts();
                o.max_commits = 2;
                o
            }),
            ("--since after the ties", {
                let mut o = opts();
                o.since = Some(rfc3339(BASE_TS + 500));
                o
            }),
        ] {
            let (walk, _, x) = collect_filtered_events_git(&fx.store, &o).unwrap();
            let got = run(&o).unwrap_or_else(|| panic!("[{label}] {what} did not serve"));
            assert_same(&got, &(walk, x), &format!("{label} {what}"));
        }
    }
}

#[test]
fn task_1507_branch_switch_in_one_checkout_does_not_reset() {
    // N2a: one store checkout switches to a diverged branch and back. The
    // old tip is still on a branch, so this is not a rewrite.
    let mut fx = Fixture::new();
    build_linear(&mut fx);
    assert_parity(&fx, &opts(), "initial");
    let built_at = test_support::meta(&fx.db, "built_at");
    let count = test_support::commit_count(&fx.db);
    fx.git(&["checkout", "-q", "-b", "other", "HEAD~3"]);
    let s = Spec::new("STORY-50", "Story", "On another branch");
    fx.put(&s);
    fx.commit("add STORY-50");
    assert!(serve(&fx, &opts()).is_none(), "diverged branch falls back");
    fx.git(&["checkout", "-q", "aida-store"]);
    assert_parity(&fx, &opts(), "back on the indexed branch");
    assert_eq!(test_support::meta(&fx.db, "built_at"), built_at);
    assert_eq!(test_support::meta(&fx.db, "last_reset_reason"), None);
    assert_eq!(test_support::commit_count(&fx.db), count);
}

#[test]
fn task_1507_partial_index_before_any_chunk_never_serves() {
    // T1: after a reset and before chunk 0 the watermark is unknown.
    let mut fx = Fixture::new();
    build_linear(&mut fx);
    test_support::partial_build(&fx.store, &fx.db, 5, 0).unwrap();
    assert_eq!(test_support::commit_count(&fx.db), 0);
    for (label, o) in [
        ("everything", opts()),
        ("-n 1", {
            let mut o = opts();
            o.limit = 1;
            o
        }),
        ("--since", {
            let mut o = opts();
            o.since = Some(rfc3339(BASE_TS));
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
fn task_1507_boundary_equal_to_watermark_falls_back() {
    // T1: an indexed boundary in the same second as the newest unfilled
    // commit is not "strictly newer" than the watermark.
    let mut fx = Fixture::new();
    let mut s = Spec::new("FR-1", "Functional", "Ties");
    for (off, st) in [
        (0, "Draft"),
        (200, "Approved"),
        (200, "Done"),
        (200, "Draft"),
        (300, "Approved"),
    ] {
        s.status = st.into();
        fx.put(&s);
        fx.commit_at(BASE_TS + off, "update FR-1");
    }
    // Chunks of one: the three newest commits are indexed; the newest
    // unfilled one shares its second with the two below the top.
    test_support::partial_build(&fx.store, &fx.db, 1, 3).unwrap();
    assert_eq!(
        test_support::meta(&fx.db, "unfilled_max_ts"),
        Some((BASE_TS + 200).to_string())
    );
    let run = |o: &HistoryOpts| test_support::query_only(&fx.store, &fx.db, o).unwrap();
    // A capped window whose last row sits on the watermark second.
    let mut o = opts();
    o.max_commits = 2;
    o.max_commits_explicit = true;
    assert!(run(&o).is_none(), "window boundary ts == watermark");
    // A limit cut landing on that second.
    let top = collect_filtered_events_git(&fx.store, &{
        let mut o = opts();
        o.max_commits = 1;
        o
    })
    .unwrap()
    .0
    .len();
    let mut o = opts();
    o.limit = top + 1;
    assert!(run(&o).is_none(), "-n boundary ts == watermark");
    let mut o = opts();
    o.since = Some(rfc3339(BASE_TS + 200));
    assert!(run(&o).is_none(), "since == watermark");
    // Strictly above it: served, exactly.
    let mut o = opts();
    o.max_commits = 1;
    o.max_commits_explicit = true;
    let (walk, _, x) = collect_filtered_events_git(&fx.store, &o).unwrap();
    let got = run(&o).expect("boundary above the watermark serves");
    assert_same(&got, &(walk, x), "boundary above the watermark");
}

// ---------------------------------------------------------------------------
// Round 4: fork-point ties (B3), ancestor-checked partial catch-up and
// self-repair (B4), compact-aware liveness (N2a-b). The reviewer's probes,
// kept as regression tests.
// trace:TASK-1507 | ai:claude
// ---------------------------------------------------------------------------

fn all_probes(fx: &Fixture) -> Vec<(String, HistoryOpts)> {
    let mut p = cut_probes(fx);
    p.extend(since_probes(fx));
    p
}

/// Complete index: every served probe must equal the walk. Returns
/// (served, fell back).
fn check_complete(fx: &Fixture, probes: &[(String, HistoryOpts)], tag: &str) -> (usize, usize) {
    let walks = walk_all(fx, probes);
    fx.drop_index();
    history_cache::rebuild_full_at(&fx.store, &fx.db).unwrap();
    let (mut s, mut f) = (0, 0);
    for ((label, o), walk) in probes.iter().zip(&walks) {
        match test_support::query_only(&fx.store, &fx.db, o).unwrap() {
            Some(got) => {
                assert_same(&got, walk, &format!("{tag} complete: {label}"));
                s += 1;
            }
            None => f += 1,
        }
    }
    (s, f)
}

#[test]
fn task_1507_fork_point_tie_complete_and_partial() {
    // B3: the side branch's first commit shares a second with its parent,
    // the fork point. git gives the fork point first (FIFO), seq the child.
    let mut fx = Fixture::new();
    build_dated_merge(&mut fx, &[0, 50], &[200], 300, &[]);
    let probes = all_probes(&fx);
    let (served, _) = check_complete(&fx, &probes, "fork-point");
    assert!(served > 0, "cuts above the tie still serve");
    for n in [3usize, 4, 5] {
        let mut o = opts();
        o.limit = n;
        assert!(
            test_support::query_only(&fx.store, &fx.db, &o)
                .unwrap()
                .is_none(),
            "-n {n} reaches the fork-point tie"
        );
    }
    let mut fx = Fixture::new();
    build_dated_merge(&mut fx, &[0, 50], &[200], 300, &[400]);
    let probes = all_probes(&fx);
    check_partial_states(&fx, &probes);
}

#[test]
fn task_1507_octopus_merge_ties() {
    for (b2, b3, main) in [
        (&[100][..], &[100][..], &[200][..]),
        (&[0][..], &[50][..], &[200][..]),
        (&[100, 150][..], &[120][..], &[100][..]),
        (&[120][..], &[0, 150][..], &[120][..]),
    ] {
        let mut fx = Fixture::new();
        let base = Spec::new("EPIC-1", "Epic", "Base");
        fx.put(&base);
        fx.commit_at(BASE_TS, "base");
        for (name, offs, id) in [("b2", b2, "BUG-2"), ("b3", b3, "BUG-3")] {
            fx.git(&["checkout", "-q", "-b", name, "aida-store"]);
            let mut s = Spec::new(id, "Bug", name);
            for (i, off) in offs.iter().enumerate() {
                s.status = STATUSES[i % 3].into();
                fx.put(&s);
                fx.commit_at(BASE_TS + off, name);
            }
        }
        fx.git(&["checkout", "-q", "aida-store"]);
        let mut m = Spec::new("FR-4", "Functional", "main");
        for (i, off) in main.iter().enumerate() {
            m.status = STATUSES[i % 3].into();
            fx.put(&m);
            fx.commit_at(BASE_TS + off, "main");
        }
        git_in(
            &fx.store,
            &["merge", "-q", "--no-ff", "--no-edit", "b2", "b3"],
            BASE_TS + 500,
        );
        fx.clock = BASE_TS + 500;
        let parents = fx.git(&["rev-list", "--parents", "-n1", "HEAD"]);
        assert_eq!(parents.split_whitespace().count(), 4, "octopus");
        let probes = all_probes(&fx);
        let tag = format!("octopus {b2:?} {b3:?} {main:?}");
        check_complete(&fx, &probes, &tag);
        check_partial_states(&fx, &probes);
    }
}

#[test]
fn task_1507_nested_merge_ties() {
    // The outer side branch contains an inner merge.
    for (inner, side, main) in [
        (&[100][..], &[80][..], &[100][..]),
        (&[60][..], &[60][..], &[200][..]),
        (&[10][..], &[60][..], &[10][..]),
        (&[0][..], &[60][..], &[200][..]),
    ] {
        let mut fx = Fixture::new();
        let base = Spec::new("EPIC-1", "Epic", "Base");
        fx.put(&base);
        fx.commit_at(BASE_TS, "base");
        fx.git(&["checkout", "-q", "-b", "side"]);
        fx.git(&["checkout", "-q", "-b", "inner"]);
        let mut a = Spec::new("BUG-5", "Bug", "inner");
        for (i, off) in inner.iter().enumerate() {
            a.status = STATUSES[i % 3].into();
            fx.put(&a);
            fx.commit_at(BASE_TS + off, "inner");
        }
        fx.git(&["checkout", "-q", "side"]);
        let mut b = Spec::new("BUG-6", "Bug", "side");
        for (i, off) in side.iter().enumerate() {
            b.status = STATUSES[i % 3].into();
            fx.put(&b);
            fx.commit_at(BASE_TS + off, "side");
        }
        fx.merge_at(BASE_TS + 250, "inner", &[]);
        fx.git(&["checkout", "-q", "aida-store"]);
        let mut m = Spec::new("FR-7", "Functional", "main");
        for (i, off) in main.iter().enumerate() {
            m.status = STATUSES[i % 3].into();
            fx.put(&m);
            fx.commit_at(BASE_TS + off, "main");
        }
        fx.merge_at(BASE_TS + 300, "side", &[]);
        let probes = all_probes(&fx);
        let tag = format!("nested {inner:?} {side:?} {main:?}");
        check_complete(&fx, &probes, &tag);
        check_partial_states(&fx, &probes);
    }
}

#[test]
fn task_1507_catch_up_across_tie_merges_from_partial_states() {
    // Index a partial state before the merge (from merge^1, merge^2, or
    // one commit below HEAD), then catch up with a zero and an unbounded
    // budget. Every served answer must equal the walk; nothing may error.
    for (side, main) in [
        (&[300][..], &[300][..]),
        (&[100, 300][..], &[200, 300][..]),
        (&[0, 50][..], &[200][..]),
    ] {
        let mut fx = Fixture::new();
        build_dated_merge(&mut fx, side, main, 600, &[700, 800]);
        let merged = fx.head();
        let probes = all_probes(&fx);
        let walks = walk_all(&fx, &probes);
        for pre in ["aida-store~1", "aida-store~2^1", "aida-store~2^2"] {
            for chunks in 0..=4usize {
                fx.drop_index();
                fx.git(&["checkout", "-q", "--detach", pre]);
                if fx.head() == merged {
                    continue;
                }
                test_support::partial_build(&fx.store, &fx.db, 1, chunks).unwrap();
                fx.git(&["checkout", "-q", "aida-store"]);
                let tag = format!("{side:?}/{main:?} from {pre} x{chunks}");
                for budget in [Budget::for_duration(Duration::ZERO), Budget::unbounded()] {
                    test_support::index(&fx.store, &fx.db, budget)
                        .unwrap_or_else(|e| panic!("[{tag}] catch-up failed: {e:#}"));
                    for ((label, o), walk) in probes.iter().zip(&walks) {
                        if let Some(got) = test_support::query_only(&fx.store, &fx.db, o).unwrap() {
                            assert_same(&got, walk, &format!("{tag}: {label}"));
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn task_1507_id_queries_with_merge_ties() {
    for (side, main, after) in [
        (&[300][..], &[300][..], &[700][..]),
        (&[0, 50][..], &[200][..], &[300][..]),
    ] {
        let mut fx = Fixture::new();
        build_dated_merge(&mut fx, side, main, 600, &[]);
        let mut e = Spec::new("EPIC-1", "Epic", "Base");
        for (i, off) in after.iter().enumerate() {
            e.status = STATUSES[i % 3].into();
            fx.put(&e);
            fx.commit_at(BASE_TS + off, "after: EPIC-1");
        }
        let mut probes = Vec::new();
        for id in ["EPIC-1", "BUG-2", "FR-3"] {
            for (l, mut o) in all_probes(&fx) {
                o.id_filter = Some(id.into());
                probes.push((format!("--id {id} {l}"), o));
            }
        }
        check_complete(&fx, &probes, &format!("id {side:?} {main:?}"));
        check_partial_states(&fx, &probes);
    }
}

#[test]
fn task_1507_budgeted_catch_up_never_moves_the_tip_onto_a_side_branch() {
    // B4: a complete index at merge^1, then a catch-up whose budget runs
    // out inside the side branch.
    let mut fx = Fixture::new();
    build_dated_merge(&mut fx, &[100, 200, 300], &[150], 600, &[700]);
    fx.git(&["checkout", "-q", "--detach", "aida-store~1^1"]);
    let old_tip = fx.head();
    history_cache::rebuild_full_at(&fx.store, &fx.db).unwrap();
    fx.git(&["checkout", "-q", "aida-store"]);
    test_support::index(&fx.store, &fx.db, Budget::for_duration(Duration::ZERO)).unwrap();
    let tip = test_support::meta(&fx.db, "tip_sha").unwrap();
    let is_anc = |a: &str, b: &str| {
        Command::new("git")
            .arg("-C")
            .arg(&fx.store)
            .args(["merge-base", "--is-ancestor", a, b])
            .status()
            .unwrap()
            .success()
    };
    assert!(
        is_anc(&old_tip, &tip),
        "the new tip descends from the old one"
    );

    // A reader checked out at the first side commit (another worktree).
    let side1 = fx
        .git(&["rev-parse", "aida-store~1^2~2"])
        .trim()
        .to_string();
    let wt = fx.tmp.path().join("side-wt");
    fx.git(&[
        "worktree",
        "add",
        "-q",
        "--detach",
        wt.to_str().unwrap(),
        &side1,
    ]);
    let o = opts();
    let (walk, _, x) = collect_filtered_events_git(&wt, &o).unwrap();
    if let Some(got) = test_support::query_only(&wt, &fx.db, &o).unwrap() {
        assert_same(&got, &(walk, x), "reader at a side commit");
    }
    // The main checkout keeps catching up.
    test_support::index(&fx.store, &fx.db, Budget::unbounded()).unwrap();
    assert_parity(&fx, &opts(), "after the catch-up finishes");
}

#[test]
fn task_1507_inconsistent_index_resets_instead_of_wedging() {
    // B4: an index whose tip sits on a side commit (what the old boundary
    // rule could leave) makes the next catch-up re-insert indexed commits.
    // That query falls back and resets; the next one serves again.
    let mut fx = Fixture::new();
    build_dated_merge(&mut fx, &[100, 200, 300], &[150], 600, &[700]);
    fx.git(&["checkout", "-q", "--detach", "aida-store~1^1"]);
    history_cache::rebuild_full_at(&fx.store, &fx.db).unwrap();
    fx.git(&["checkout", "-q", "aida-store"]);
    let side1 = fx
        .git(&["rev-parse", "aida-store~1^2~2"])
        .trim()
        .to_string();
    // Plant the wedged state: side1 indexed as the tip, beside merge^1.
    fx.git(&["checkout", "-q", "--detach", &side1]);
    let one = history_cache::serve_at(&fx.store, &fx.db, &opts(), GENEROUS);
    assert!(one.is_ok(), "reader at side1 does not error: {one:?}");
    test_support::set_meta(&fx.db, "tip_sha", &side1);
    fx.git(&["checkout", "-q", "aida-store"]);
    let o = opts();
    let first = history_cache::serve_at(&fx.store, &fx.db, &o, GENEROUS);
    assert!(first.is_err(), "the inconsistent catch-up falls back");
    assert_eq!(
        test_support::meta(&fx.db, "last_reset_reason").as_deref(),
        Some("the index disagreed with the store history")
    );
    assert_parity(&fx, &opts(), "the index is usable again");
}

#[test]
fn task_1507_compact_with_backup_branch_resets() {
    // N2a-b: what the real `store compact` does, backup branch first.
    let mut fx = Fixture::new();
    build_linear(&mut fx);
    assert_parity(&fx, &opts(), "initial");
    fx.git(&["branch", "aida-store-pre-squash-1", "HEAD"]);
    let squashed = fx
        .git(&["commit-tree", "HEAD^{tree}", "-m", "squashed"])
        .trim()
        .to_string();
    fx.git(&["reset", "-q", "--soft", &squashed]);
    assert_parity(&fx, &opts(), "after compact");
    assert!(test_support::meta(&fx.db, "last_reset_reason")
        .unwrap()
        .contains("root commit changed"));
    fx.commit("one more");
    assert_parity(&fx, &opts(), "after compact + commit");
}

#[test]
fn task_1507_stale_remote_tracking_ref_does_not_block_reset() {
    let mut fx = Fixture::new();
    build_linear(&mut fx);
    assert_parity(&fx, &opts(), "initial");
    fx.git(&["update-ref", "refs/remotes/origin/aida-store", "HEAD"]);
    fx.git(&["reset", "-q", "--hard", "HEAD~2"]);
    fx.commit("rewritten");
    assert_parity(&fx, &opts(), "after a rewrite with a stale remote ref");
    assert_eq!(
        test_support::meta(&fx.db, "last_reset_reason").as_deref(),
        Some("store history was rewritten")
    );
}

#[test]
fn task_1507_pre_squash_backup_branch_never_keeps_a_rewritten_tip_live() {
    // N2a-b: even when the root survives a rewrite, a `store compact`
    // backup branch at the old tip does not count as a live checkout.
    let mut fx = Fixture::new();
    build_linear(&mut fx);
    assert_parity(&fx, &opts(), "initial");
    fx.git(&["branch", "aida-store-pre-squash-2", "HEAD"]);
    fx.git(&["reset", "-q", "--hard", "HEAD~2"]);
    fx.commit("rewritten");
    assert_parity(&fx, &opts(), "after a rewrite with a backup branch");
    assert_eq!(
        test_support::meta(&fx.db, "last_reset_reason").as_deref(),
        Some("store history was rewritten")
    );
}

// ---------------------------------------------------------------------------
// Round 5: merge-own paths (B5), conservative `--id` across merges, long
// side branches make progress (N6), pinned liveness and self-repair
// branches (T2), and a small fixed-seed random sweep.
// trace:TASK-1507 | ai:claude
// ---------------------------------------------------------------------------

#[test]
fn task_1507_reader_behind_with_no_live_ref_does_not_reset() {
    // T2: an undo in the only checkout (the branch moves back, so nothing
    // else holds the tip). The reader is behind: fall back, keep the index.
    let mut fx = Fixture::new();
    build_linear(&mut fx);
    assert_parity(&fx, &opts(), "initial");
    let tip = fx.head();
    let built_at = test_support::meta(&fx.db, "built_at");
    fx.git(&["reset", "-q", "--hard", "HEAD~2"]);
    assert!(serve(&fx, &opts()).is_none());
    assert_eq!(test_support::meta(&fx.db, "tip_sha"), Some(tip));
    assert_eq!(test_support::meta(&fx.db, "built_at"), built_at);
    assert_eq!(test_support::meta(&fx.db, "last_reset_reason"), None);
}

#[test]
fn task_1507_root_change_resets_even_when_a_branch_keeps_the_old_tip() {
    // T2: the root check runs before liveness. A user branch still holds
    // the pre-compact tip, but the store history was replaced.
    let mut fx = Fixture::new();
    build_linear(&mut fx);
    assert_parity(&fx, &opts(), "initial");
    fx.git(&["branch", "keep-old", "HEAD"]);
    let squashed = fx
        .git(&["commit-tree", "HEAD^{tree}", "-m", "squashed"])
        .trim()
        .to_string();
    fx.git(&["reset", "-q", "--soft", &squashed]);
    assert_parity(&fx, &opts(), "after compact");
    assert!(test_support::meta(&fx.db, "last_reset_reason")
        .unwrap()
        .contains("root commit changed"));
}

#[test]
fn task_1507_failed_reset_deletes_an_inconsistent_index() {
    // T2: an inconsistent index whose reset also fails is deleted, so the
    // next query rebuilds it.
    let mut fx = Fixture::new();
    build_dated_merge(&mut fx, &[100, 200, 300], &[150], 600, &[700]);
    fx.git(&["checkout", "-q", "--detach", "aida-store~1^1"]);
    history_cache::rebuild_full_at(&fx.store, &fx.db).unwrap();
    fx.git(&["checkout", "-q", "aida-store"]);
    let side1 = fx
        .git(&["rev-parse", "aida-store~1^2~2"])
        .trim()
        .to_string();
    test_support::set_meta(&fx.db, "tip_sha", &side1);
    history_cache::fail_next_reset();
    let first = history_cache::serve_at(&fx.store, &fx.db, &opts(), GENEROUS);
    assert!(first.is_err(), "the inconsistent catch-up falls back");
    assert!(!fx.db.exists(), "the unrepairable index is deleted");
    assert_parity(&fx, &opts(), "rebuilt");
}

// ---------------------------------------------------------------------------
// BUG-1636: `--shipped` is every transition into Completed
// ---------------------------------------------------------------------------

/// Ships from several prior statuses, non-ships around them, and a reopen
/// followed by a re-complete.
// trace:BUG-1636 | ai:claude
fn build_ships(fx: &mut Fixture) {
    let mut merged = Spec::new("BUG-40", "Bug", "Merged straight from InProgress");
    let mut legacy = Spec::new("FR-41", "Functional", "Shipped through Done");
    let mut stalled = Spec::new("TASK-42", "Task", "Reaches Done, never ships");
    let mut reopened = Spec::new("STORY-43", "Story", "Shipped, reopened, shipped again");
    let mut direct = Spec::new("TASK-44", "Task", "Approved straight to Completed");
    for s in [&merged, &legacy, &stalled, &reopened, &direct] {
        fx.put(s);
    }
    fx.commit("add specs");

    merged.status = "InProgress".into();
    fx.put(&merged);
    fx.commit("claim BUG-40");
    merged.status = "Completed".into();
    fx.put(&merged);
    fx.commit("merge BUG-40");

    legacy.status = "Done".into();
    fx.put(&legacy);
    fx.commit("FR-41 done");
    legacy.status = "Completed".into();
    fx.put(&legacy);
    fx.commit("merge FR-41");

    stalled.status = "InProgress".into();
    fx.put(&stalled);
    fx.commit("claim TASK-42");
    stalled.status = "Done".into();
    fx.put(&stalled);
    fx.commit("TASK-42 done");

    direct.status = "Approved".into();
    fx.put(&direct);
    fx.commit("approve TASK-44");
    direct.status = "Completed".into();
    fx.put(&direct);
    fx.commit("merge TASK-44");

    reopened.status = "InProgress".into();
    fx.put(&reopened);
    fx.commit("claim STORY-43");
    reopened.status = "Completed".into();
    fx.put(&reopened);
    fx.commit("merge STORY-43");
    // A Completed spec edited without a status change: no ship.
    reopened.description = "follow-up note".into();
    fx.put(&reopened);
    fx.commit("edit STORY-43");
    // The reopen itself leaves Completed: not a ship.
    reopened.status = "InProgress".into();
    fx.put(&reopened);
    fx.commit("reopen STORY-43");
    reopened.status = "Completed".into();
    fx.put(&reopened);
    fx.commit("merge STORY-43 again");
}

fn status_pairs(events: &[Event]) -> Vec<(String, String, String)> {
    events
        .iter()
        .map(|e| match &e.kind {
            EventKind::StatusChange { from, to } => (e.spec_id.clone(), from.clone(), to.clone()),
            other => panic!("--shipped returned a non-status event: {other:?}"),
        })
        .collect()
}

#[test]
fn bug_1636_shipped_matches_every_transition_into_completed() {
    // trace:BUG-1636 | ai:claude
    let mut fx = Fixture::new();
    build_ships(&mut fx);
    let mut o = opts();
    o.shipped_only = true;
    let (walk, _, _) = collect_filtered_events_git(&fx.store, &o).unwrap();
    let pair = |id: &str, from: &str| (id.to_string(), from.to_string(), "Completed".to_string());
    // Newest first.
    assert_eq!(
        status_pairs(&walk),
        vec![
            pair("STORY-43", "InProgress"),
            pair("STORY-43", "InProgress"),
            pair("TASK-44", "Approved"),
            pair("FR-41", "Done"),
            pair("BUG-40", "InProgress"),
        ],
        "every transition into Completed, and only those"
    );

    // Non-ships stay out: InProgress→Done, the reopen, and plain edits.
    let (all, _, _) = collect_filtered_events_git(&fx.store, &opts()).unwrap();
    let non_ships: Vec<&Event> = all
        .iter()
        .filter(|e| !walk.contains(e))
        .filter(|e| matches!(&e.kind, EventKind::StatusChange { .. }))
        .collect();
    assert!(
        non_ships.iter().any(|e| e.spec_id == "TASK-42"
            && e.kind
                == EventKind::StatusChange {
                    from: "InProgress".into(),
                    to: "Done".into()
                }),
        "InProgress → Done is not a ship"
    );
    assert!(
        non_ships.iter().any(|e| e.spec_id == "STORY-43"
            && e.kind
                == EventKind::StatusChange {
                    from: "Completed".into(),
                    to: "InProgress".into()
                }),
        "a reopen away from Completed is not a ship"
    );
}

#[test]
fn bug_1636_is_ship_event_predicate() {
    // trace:BUG-1636 | ai:claude
    use crate::history::is_ship_event;
    let sc = |from: &str, to: &str| EventKind::StatusChange {
        from: from.into(),
        to: to.into(),
    };
    for from in [
        "InProgress",
        "Done",
        "Approved",
        "Draft",
        "done",
        "Rejected",
    ] {
        assert!(is_ship_event(&sc(from, "Completed")), "{from} → Completed");
    }
    assert!(is_ship_event(&sc("in_progress", "completed")));
    assert!(!is_ship_event(&sc("Completed", "Completed")), "no-op");
    assert!(!is_ship_event(&sc("Completed", "InProgress")), "reopen");
    assert!(!is_ship_event(&sc("InProgress", "Done")));
    assert!(!is_ship_event(&sc("Done", "Released")));
}

#[test]
fn bug_1636_shipped_index_matches_the_walk() {
    // The index narrows `--shipped` in SQL on its stored `is_ship` column;
    // every window of that answer must equal the walk's.
    // trace:BUG-1636 | ai:claude
    let mut fx = Fixture::new();
    build_ships(&mut fx);
    history_cache::rebuild_full_at(&fx.store, &fx.db).unwrap();
    let total = commit_times(&fx).len();
    let mut o = opts();
    o.shipped_only = true;
    assert_parity(&fx, &o, "--shipped");
    let got = serve(&fx, &o).unwrap();
    assert_eq!(got.events.len(), 5, "the index serves all five ships");
    for limit in 0..=6 {
        let mut o = opts();
        o.shipped_only = true;
        o.limit = limit;
        assert_parity(&fx, &o, &format!("--shipped -n {limit}"));
    }
    for max_commits in 0..=total + 1 {
        let mut o = opts();
        o.shipped_only = true;
        o.max_commits = max_commits;
        o.max_commits_explicit = true;
        assert_parity(&fx, &o, &format!("--shipped --max-commits {max_commits}"));
    }
    for id in ["BUG-40", "FR-41", "TASK-42", "STORY-43", "TASK-44"] {
        let mut o = opts();
        o.shipped_only = true;
        o.id_filter = Some(id.into());
        assert_parity(&fx, &o, &format!("--shipped --id {id}"));
    }
    // Caught up incrementally from a partial state, too.
    fx.drop_index();
    test_support::partial_build(&fx.store, &fx.db, 3, 2).unwrap();
    let mut o = opts();
    o.shipped_only = true;
    o.since = Some(rfc3339(fx.commit_ts("HEAD~3")));
    assert_query_only_parity(&fx, &o, "--shipped on a partial index");
}

mod sweep {
    //! Random DAGs (after the round-4 reviewer's probe), trimmed to a
    //! small fixed-seed set: every served answer must equal the walk.
    use super::*;
    use std::collections::HashMap;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x.wrapping_mul(0x2545F4914F6CDD1D)
        }
        fn below(&mut self, n: usize) -> usize {
            if n == 0 {
                0
            } else {
                (self.next() % n as u64) as usize
            }
        }
        fn pct(&mut self, p: usize) -> bool {
            self.below(100) < p
        }
        fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
            &v[self.below(v.len())]
        }
    }

    const IDS: [(&str, &str); 5] = [
        ("FR-1", "Functional"),
        ("FR-2", "Functional"),
        ("BUG-3", "Bug"),
        ("TASK-4", "Task"),
        ("EPIC-5", "Epic"),
    ];
    // BUG-1636: InProgress too, so the sweep's `--shipped` probe sees
    // `InProgress → Completed` ships, not only `Done → Completed`.
    // trace:BUG-1636 | ai:claude
    const STAT: [&str; 5] = ["Draft", "Approved", "InProgress", "Done", "Completed"];

    type State = BTreeMap<&'static str, Spec>;

    pub(super) struct Dag {
        pub fx: Fixture,
        ts: HashMap<String, i64>,
        parents: HashMap<String, Vec<String>>,
        states: HashMap<String, State>,
        pub main_hist: Vec<String>,
        all: Vec<String>,
    }

    impl Dag {
        pub fn new() -> Self {
            Dag {
                fx: Fixture::new(),
                ts: HashMap::new(),
                parents: HashMap::new(),
                states: HashMap::new(),
                main_hist: Vec::new(),
                all: Vec::new(),
            }
        }

        fn ancestors(&self, c: &str) -> HashSet<String> {
            let mut seen = HashSet::new();
            let mut st = vec![c.to_string()];
            while let Some(x) = st.pop() {
                if seen.insert(x.clone()) {
                    for p in &self.parents[&x] {
                        st.push(p.clone());
                    }
                }
            }
            seen
        }

        /// Commit `state` (the whole objects tree) with `parents` at `ts`.
        pub fn write_commit(
            &mut self,
            parents: &[String],
            state: State,
            ts: i64,
            msg: &str,
        ) -> String {
            let obj = self.fx.store.join("objects");
            let _ = std::fs::remove_dir_all(&obj);
            for s in state.values() {
                self.fx.put(s);
            }
            self.fx.clock = ts;
            self.fx.git(&["add", "-A"]);
            let tree = self.fx.git(&["write-tree"]).trim().to_string();
            let mut args: Vec<String> = vec!["commit-tree".into(), tree];
            for p in parents {
                args.push("-p".into());
                args.push(p.clone());
            }
            args.push("-m".into());
            args.push(msg.into());
            let a: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            let sha = self.fx.git(&a).trim().to_string();
            self.ts.insert(sha.clone(), ts);
            self.parents.insert(sha.clone(), parents.to_vec());
            self.states.insert(sha.clone(), state);
            self.all.push(sha.clone());
            sha
        }

        pub fn set_head(&self, sha: &str) {
            self.fx.git(&["update-ref", "refs/heads/aida-store", sha]);
            self.fx
                .git(&["symbolic-ref", "HEAD", "refs/heads/aida-store"]);
            self.fx.git(&["reset", "-q", "--hard", sha]);
        }
    }

    fn edit(rng: &mut Rng, st: &mut State, n: usize) {
        for _ in 0..n {
            let (id, ty) = *rng.pick(&IDS);
            match st.get_mut(id) {
                None => {
                    let mut s = Spec::new(id, ty, &format!("{id} title"));
                    s.status = (*rng.pick(&STAT)).into();
                    // BUG-1631: start with 0-3 edges so the custom slot
                    // (every third edge) is reached. trace:BUG-1631 | ai:claude
                    s.relationships = rng.below(4);
                    s.rel_mapping_form = rng.pct(50);
                    st.insert(id, s);
                }
                // trace:BUG-1631 | ai:claude — 9 to 12 exercise edges.
                Some(s) => match rng.below(14) {
                    0 => {
                        st.remove(id);
                    }
                    1..=4 => s.status = (*rng.pick(&STAT)).into(),
                    5 | 6 => s
                        .comments
                        .push(("alice".into(), format!("c{}", rng.below(1000)))),
                    7 => s.tags.push(format!("t{}", rng.below(100))),
                    8 => s.owner = format!("o{}", rng.below(5)),
                    9..=11 => {
                        if s.relationships > 0 && rng.pct(40) {
                            s.relationships -= 1;
                        } else {
                            s.relationships += 1;
                        }
                    }
                    12 => s.rel_mapping_form = !s.rel_mapping_form,
                    _ => s.description = format!("d{}", rng.below(1000)),
                },
            }
        }
    }

    fn random_dag(seed: u64, steps: usize, skew: bool) -> Dag {
        let mut rng = Rng(seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407)
            | 1);
        let mut dag = Dag::new();
        let mut st = State::new();
        edit(&mut rng, &mut st, 2);
        let root = dag.write_commit(&[], st, BASE_TS, "root");
        let mut tips: Vec<String> = vec![root.clone()];
        dag.main_hist.push(root);
        let deltas = [0i64, 0, 0, 10, 10, 20, 50, 120];
        let choose_ts = |rng: &mut Rng, dag: &Dag, parents: &[String]| -> i64 {
            let maxp = parents.iter().map(|p| dag.ts[p]).max().unwrap();
            if skew && rng.pct(7) {
                return maxp - 1 - rng.below(60) as i64;
            }
            if rng.pct(15) {
                let cands: Vec<i64> = dag.ts.values().copied().filter(|t| *t >= maxp).collect();
                if !cands.is_empty() {
                    return *rng.pick(&cands);
                }
            }
            maxp + *rng.pick(&deltas)
        };
        for step in 0..steps {
            let op = rng.below(100);
            if op < 55 || tips.len() == 1 && op < 70 {
                let b = if rng.pct(40) {
                    0
                } else {
                    rng.below(tips.len())
                };
                let p = tips[b].clone();
                let mut s = dag.states[&p].clone();
                let n = rng.below(3);
                edit(&mut rng, &mut s, n);
                let ts = choose_ts(&mut rng, &dag, std::slice::from_ref(&p));
                let c = dag.write_commit(&[p], s, ts, &format!("c{step}"));
                tips[b] = c.clone();
                if b == 0 {
                    dag.main_hist.push(c);
                }
            } else if op < 72 {
                if tips.len() >= 4 {
                    continue;
                }
                let from = if rng.pct(60) {
                    tips[rng.below(tips.len())].clone()
                } else {
                    dag.all[rng.below(dag.all.len())].clone()
                };
                tips.push(from);
            } else {
                if tips.len() < 2 {
                    continue;
                }
                let target = if rng.pct(70) {
                    0
                } else {
                    rng.below(tips.len())
                };
                let mut others: Vec<usize> = (0..tips.len()).filter(|i| *i != target).collect();
                let k = if others.len() >= 2 && rng.pct(25) {
                    2
                } else {
                    1
                };
                let mut srcs = Vec::new();
                for _ in 0..k {
                    let i = rng.below(others.len());
                    srcs.push(others.remove(i));
                }
                let mut parents = vec![tips[target].clone()];
                let tanc = dag.ancestors(&tips[target]);
                for &s in &srcs {
                    let t = &tips[s];
                    if tanc.contains(t) || parents.contains(t) {
                        continue;
                    }
                    parents.push(t.clone());
                }
                if parents.len() < 2 {
                    continue;
                }
                let mut ms = State::new();
                for (id, _) in IDS {
                    let p = rng.pick(&parents).clone();
                    if let Some(s) = dag.states[&p].get(id) {
                        ms.insert(id, s.clone());
                    }
                }
                if rng.pct(20) {
                    edit(&mut rng, &mut ms, 1);
                }
                let ts = choose_ts(&mut rng, &dag, &parents);
                let m = dag.write_commit(&parents, ms, ts, &format!("merge{step}"));
                tips[target] = m.clone();
                if target == 0 {
                    dag.main_hist.push(m);
                }
                if rng.pct(50) {
                    let mut rm: Vec<usize> = srcs.clone();
                    rm.sort_unstable();
                    for i in rm.into_iter().rev() {
                        if i != 0 && tips.len() > 1 {
                            tips.remove(i);
                        }
                    }
                }
            }
        }
        let head = dag.main_hist.last().unwrap().clone();
        dag.set_head(&head);
        dag
    }

    fn probes_for(rng: &mut Rng, fx: &Fixture) -> Vec<(String, HistoryOpts)> {
        let times = commit_times(fx);
        let ncommits = times.len();
        let nevents = collect_filtered_events_git(&fx.store, &opts())
            .unwrap()
            .0
            .len();
        let rt = |rng: &mut Rng| -> i64 { times[rng.below(times.len())] + rng.below(3) as i64 - 1 };
        let mut out = vec![("all".to_string(), opts())];
        for _ in 0..3 {
            let mut o = opts();
            o.limit = rng.below(nevents + 2);
            out.push((format!("-n {}", o.limit), o));
            let mut o = opts();
            o.max_commits = rng.below(ncommits + 2);
            o.max_commits_explicit = true;
            out.push((format!("--max-commits {}", o.max_commits), o));
            let mut o = opts();
            let s = rt(rng);
            o.since = Some(rfc3339(s));
            out.push((format!("--since {s}"), o));
            let mut o = opts();
            let (id, _) = *rng.pick(&IDS);
            o.id_filter = Some(id.to_string());
            o.limit = rng.below(4);
            out.push((format!("--id {id} -n {}", o.limit), o));
            // trace:BUG-1620 | ai:claude
            // `--id` windows and `--since` now serve across merges too.
            let mut o = opts();
            let (id, _) = *rng.pick(&IDS);
            o.id_filter = Some(id.to_string());
            o.max_commits = rng.below(ncommits + 2);
            o.max_commits_explicit = true;
            out.push((format!("--id {id} --max-commits {}", o.max_commits), o));
            let mut o = opts();
            let (id, _) = *rng.pick(&IDS);
            let s = rt(rng);
            o.id_filter = Some(id.to_string());
            o.since = Some(rfc3339(s));
            out.push((format!("--id {id} --since {s}"), o));
        }
        let mut o = opts();
        o.shipped_only = true;
        out.push(("--shipped".into(), o));
        out
    }

    fn compare(
        bad: &mut Vec<String>,
        fx: &Fixture,
        probes: &[(String, HistoryOpts)],
        walks: &[Walked],
        tag: &str,
    ) {
        for ((label, o), w) in probes.iter().zip(walks) {
            match test_support::query_only(&fx.store, &fx.db, o) {
                Ok(Some(got)) => {
                    if got.events != w.0 || got.window_exhausted != w.1 {
                        bad.push(format!("[{tag}] {label}: served answer differs"));
                    }
                }
                Ok(None) => {}
                Err(e) => bad.push(format!("[{tag}] {label}: query error {e:#}")),
            }
        }
    }

    /// Relationship events that add or remove a custom edge, by the form
    /// the custom edge was stored in: (tagged `!Custom`, mapping).
    // trace:BUG-1631 | ai:claude
    fn custom_edge_forms(fx: &Fixture, events: &[Event]) -> (usize, usize) {
        let (mut tagged, mut mapping) = (0, 0);
        for e in events {
            let EventKind::RelationshipsChange { edges, .. } = &e.kind else {
                continue;
            };
            let Some(edge) = edges.iter().find(|x| x.rel_type == "depends-on") else {
                continue;
            };
            // The stored form: from the commit for an add, from its first
            // parent for a removal.
            let path = aida_core::object_store::relative_object_path(&e.spec_id).unwrap();
            let rev = if edge.added {
                e.sha.clone()
            } else {
                format!("{}^", e.sha)
            };
            let yaml = fx.git(&["show", &format!("{rev}:{path}")]);
            if yaml.contains("!Custom depends-on") {
                tagged += 1;
            } else if yaml.contains("custom: depends-on") {
                mapping += 1;
            }
        }
        (tagged, mapping)
    }

    fn run_seed(seed: u64, skew: bool, bad: &mut Vec<String>, forms: &mut (usize, usize)) {
        let dag = random_dag(seed, 8 + (seed % 12) as usize, skew);
        let fx = &dag.fx;
        let mut rng = Rng(seed ^ 0x9E3779B97F4A7C15);
        let head = dag.main_hist.last().unwrap().clone();
        let probes = probes_for(&mut rng, fx);
        let walks = walk_all(fx, &probes);
        // probes[0] is the unfiltered "all" probe.
        let (t, m) = custom_edge_forms(fx, &walks[0].0);
        forms.0 += t;
        forms.1 += m;
        let total = commit_times(fx).len();

        fx.drop_index();
        history_cache::rebuild_full_at(&fx.store, &fx.db).unwrap();
        compare(bad, fx, &probes, &walks, &format!("seed {seed} complete"));

        let c = *rng.pick(&[1usize, 2, 3]);
        let k = 1 + rng.below(total.div_ceil(c));
        fx.drop_index();
        test_support::partial_build(&fx.store, &fx.db, c, k).unwrap();
        compare(
            bad,
            fx,
            &probes,
            &walks,
            &format!("seed {seed} partial c{c} x{k}"),
        );

        if dag.main_hist.len() >= 2 {
            let h1 = dag.main_hist[rng.below(dag.main_hist.len() - 1)].clone();
            dag.set_head(&h1);
            fx.drop_index();
            history_cache::rebuild_full_at(&fx.store, &fx.db).unwrap();
            dag.set_head(&head);
            for _ in 0..50 {
                test_support::index(&fx.store, &fx.db, Budget::for_duration(Duration::ZERO))
                    .unwrap_or_else(|e| panic!("seed {seed}: catch-up error {e:#}"));
                if test_support::meta(&fx.db, "tip_sha").as_deref() == Some(head.as_str()) {
                    break;
                }
            }
            if test_support::meta(&fx.db, "tip_sha").as_deref() != Some(head.as_str()) {
                bad.push(format!("[seed {seed}] catch-up never reached HEAD"));
            }
            compare(bad, fx, &probes, &walks, &format!("seed {seed} catch-up"));
        }
        fx.drop_index();
    }

    /// Slow (about 100 s of git processes): run with `--ignored`. CI runs it
    /// nightly in cross-platform.yml.
    #[test]
    #[ignore = "slow random sweep; run with --ignored (nightly CI)"]
    fn task_1507_random_dag_sweep_fixed_seeds() {
        let mut bad = Vec::new();
        let mut forms = (0usize, 0usize);
        // Kept small: each graph costs a few seconds of git processes.
        for seed in 0..10 {
            run_seed(seed, false, &mut bad, &mut forms);
        }
        for seed in 20_000..20_002 {
            run_seed(seed, true, &mut bad, &mut forms);
        }
        // BUG-1631: parity must cover custom edges in both stored forms.
        // trace:BUG-1631 | ai:claude
        eprintln!(
            "sweep custom-edge events: tagged={}, mapping={}",
            forms.0, forms.1
        );
        assert!(
            bad.is_empty(),
            "{} mismatches: {:#?}",
            bad.len(),
            &bad[..bad.len().min(20)]
        );
        // The seeds are fixed, so these counts are deterministic.
        assert!(
            forms.0 >= 3 && forms.1 >= 3,
            "sweep must change custom edges in both forms, got tagged={} mapping={}",
            forms.0,
            forms.1
        );
    }

    #[test]
    fn task_1507_id_over_an_evil_merge_on_a_discarded_side_matches_the_walk() {
        // B5: the side branch's own merge M1 edits FR-1 (an evil merge);
        // the outer merge keeps main's FR-1. Without `--full-history`,
        // `git log -- FR-1` dropped the side and never showed M1, so the
        // index had to route `--id FR-1` to the walk. BUG-1620: the walk
        // now shows M1 and the index serves it, exactly.
        // trace:BUG-1620 | ai:claude
        let mut d = Dag::new();
        let mut st = State::new();
        st.insert("FR-1", Spec::new("FR-1", "Functional", "fr"));
        st.insert("BUG-3", Spec::new("BUG-3", "Bug", "bug"));
        let root = d.write_commit(&[], st.clone(), BASE_TS, "root");
        let mut a = st.clone();
        a.get_mut("BUG-3").unwrap().status = "Approved".into();
        let a1 = d.write_commit(&[root.clone()], a.clone(), BASE_TS + 100, "main");
        let mut s1s = st.clone();
        s1s.insert("TASK-4", Spec::new("TASK-4", "Task", "t"));
        let s1 = d.write_commit(&[root.clone()], s1s.clone(), BASE_TS + 10, "s1");
        let mut s2s = st.clone();
        s2s.insert("EPIC-5", Spec::new("EPIC-5", "Epic", "e"));
        let s2 = d.write_commit(&[root.clone()], s2s.clone(), BASE_TS + 20, "s2");
        let mut m1s = s1s.clone();
        m1s.insert("EPIC-5", s2s["EPIC-5"].clone());
        m1s.get_mut("FR-1").unwrap().status = "Done".into();
        let m1 = d.write_commit(&[s1, s2], m1s.clone(), BASE_TS + 30, "m1 evil");
        let m1_sha = m1.clone();
        let mut ms = a.clone();
        ms.insert("TASK-4", m1s["TASK-4"].clone());
        ms.insert("EPIC-5", m1s["EPIC-5"].clone());
        let m = d.write_commit(&[a1, m1], ms, BASE_TS + 200, "outer merge");
        let m1 = m1_sha;
        d.set_head(&m);
        let fx = &d.fx;
        let mut probes = Vec::new();
        for id in ["FR-1", "BUG-3", "TASK-4", "EPIC-5"] {
            let mut o = opts();
            o.id_filter = Some(id.into());
            probes.push((format!("--id {id}"), o));
        }
        probes.push(("all".into(), opts()));
        let walks = walk_all(fx, &probes);
        history_cache::rebuild_full_at(&fx.store, &fx.db).unwrap();
        let mut bad = Vec::new();
        compare(&mut bad, fx, &probes, &walks, "evil side merge");
        assert!(bad.is_empty(), "{bad:#?}");
        let mut o = opts();
        o.id_filter = Some("FR-1".into());
        let got = test_support::query_only(&fx.store, &fx.db, &o)
            .unwrap()
            .expect("--id FR-1 is served");
        assert!(
            got.events.iter().any(|e| e.sha == m1),
            "the evil merge's own FR-1 change shows under --id"
        );
    }

    #[test]
    fn task_1507_long_side_branch_catch_up_makes_progress() {
        // N6: an 80-commit side branch, longer than the boundary-probe
        // limit, caught up with a zero budget.
        let mut d = Dag::new();
        let mut st = State::new();
        st.insert("FR-1", Spec::new("FR-1", "Functional", "fr"));
        let root = d.write_commit(&[], st.clone(), BASE_TS, "root");
        let mut prev = root.clone();
        let mut s = st.clone();
        for i in 0..80 {
            s.get_mut("FR-1").unwrap().description = format!("side {i}");
            prev = d.write_commit(&[prev.clone()], s.clone(), BASE_TS + 10 + i, "side");
        }
        let mut a = st.clone();
        a.insert("BUG-3", Spec::new("BUG-3", "Bug", "b"));
        let a1 = d.write_commit(&[root.clone()], a.clone(), BASE_TS + 5, "main");
        d.set_head(&a1);
        history_cache::rebuild_full_at(&d.fx.store, &d.fx.db).unwrap();
        let mut ms = s.clone();
        ms.insert("BUG-3", a["BUG-3"].clone());
        let m = d.write_commit(&[a1, prev], ms, BASE_TS + 500, "merge");
        d.set_head(&m);
        let mut reached = None;
        for i in 0..3 {
            test_support::index(&d.fx.store, &d.fx.db, Budget::for_duration(Duration::ZERO))
                .unwrap();
            if test_support::meta(&d.fx.db, "tip_sha").as_deref() == Some(m.as_str()) {
                reached = Some(i);
                break;
            }
        }
        assert!(reached.is_some(), "a zero budget still reaches HEAD");
        let (walk, _, x) = collect_filtered_events_git(&d.fx.store, &opts()).unwrap();
        let got = history_cache::serve_at(&d.fx.store, &d.fx.db, &opts(), Duration::from_millis(1))
            .unwrap()
            .expect("served once caught up");
        assert_same(&got, &(walk, x), "after the long side branch");
    }

    // Round 6 (T3, T4, B6).
    // trace:TASK-1507 | ai:claude

    #[test]
    fn bug_1620_octopus_merge_touch_from_the_third_parent_only() {
        // T5: an octopus merge M(a1, s1, s2) keeps root's FR-1, which only
        // s2 changed. M differs from its THIRD parent alone, so
        // `git log --full-history -- FR-1` lists M (no events: its combined
        // diff is empty) and counts it against `--max-commits`. The index
        // must record that touch from every parent, not just the first two.
        // trace:BUG-1620 | ai:claude
        let mut d = Dag::new();
        let mut st = State::new();
        st.insert("FR-1", Spec::new("FR-1", "Functional", "fr"));
        st.insert("BUG-3", Spec::new("BUG-3", "Bug", "b"));
        let root = d.write_commit(&[], st.clone(), BASE_TS, "root");
        let mut a = st.clone();
        a.get_mut("BUG-3").unwrap().status = "Approved".into();
        let a1 = d.write_commit(&[root.clone()], a.clone(), BASE_TS + 10, "a1");
        let mut s1s = st.clone();
        s1s.insert("TASK-4", Spec::new("TASK-4", "Task", "t"));
        let s1 = d.write_commit(&[root.clone()], s1s.clone(), BASE_TS + 20, "s1");
        let mut s2s = st.clone();
        s2s.get_mut("FR-1").unwrap().status = "Done".into();
        let s2 = d.write_commit(&[root.clone()], s2s, BASE_TS + 30, "s2");
        let mut ms = a.clone();
        ms.insert("TASK-4", s1s["TASK-4"].clone());
        let m = d.write_commit(&[a1.clone(), s1, s2], ms.clone(), BASE_TS + 40, "octopus");
        let mut zs = ms.clone();
        zs.get_mut("FR-1").unwrap().description = "z1".into();
        let z1 = d.write_commit(&[m], zs, BASE_TS + 50, "z1");
        d.set_head(&z1);
        let fx = &d.fx;

        let commits = commit_times(fx).len();
        let mut probes = Vec::new();
        for n in 0..=6 {
            let mut o = opts();
            o.id_filter = Some("FR-1".into());
            o.limit = n;
            probes.push((format!("--id FR-1 -n {n}"), o));
        }
        for max in 0..=commits + 1 {
            let mut o = opts();
            o.id_filter = Some("FR-1".into());
            o.max_commits = max;
            o.max_commits_explicit = true;
            probes.push((format!("--id FR-1 --max-commits {max}"), o));
        }
        let walks = walk_all(fx, &probes);

        // The pinned example: M fills the second slot of the window.
        let two = probes
            .iter()
            .position(|(l, _)| l == "--id FR-1 --max-commits 2")
            .unwrap();
        let shas: Vec<&str> = walks[two].0.iter().map(|e| e.sha.as_str()).collect();
        assert!(
            shas.iter().all(|s| *s == z1),
            "walk --max-commits 2 is [z1]"
        );
        assert!(!shas.is_empty());
        assert!(walks[two].1, "walk --max-commits 2 is window-exhausted");

        let check = |tag: &str| {
            for ((label, o), walk) in probes.iter().zip(&walks) {
                let got = test_support::query_only(&fx.store, &fx.db, o)
                    .unwrap()
                    .unwrap_or_else(|| panic!("[{tag}: {label}] did not serve"));
                assert_same(&got, walk, &format!("{tag}: {label}"));
            }
        };
        // (i) a complete index.
        history_cache::rebuild_full_at(&fx.store, &fx.db).unwrap();
        check("complete");
        // (ii) an index built at a1 and caught up across the octopus.
        fx.drop_index();
        d.set_head(&a1);
        history_cache::rebuild_full_at(&fx.store, &fx.db).unwrap();
        d.set_head(&z1);
        history_cache::serve_at(&fx.store, &fx.db, &opts(), GENEROUS).unwrap();
        assert_eq!(
            test_support::meta(&fx.db, "tip_sha").as_deref(),
            Some(z1.as_str())
        );
        check("catch-up");
        fx.drop_index();
    }

    #[test]
    fn task_1507_id_plus_one_probe_below_an_ours_merge() {
        // T3: an ours-merge drops the side commit that added FR-1; a later
        // main commit adds FR-1. `--id FR-1 --max-commits 1` has no merge
        // in its served range, but its +1 probe candidate (the side
        // commit) decides window_exhausted. BUG-1620: the `--full-history`
        // walk keeps that side commit too, so a complete index serves every
        // probe, exactly.
        // trace:BUG-1620 | ai:claude
        let mut d = Dag::new();
        let mut st = State::new();
        st.insert("BUG-3", Spec::new("BUG-3", "Bug", "b"));
        let root = d.write_commit(&[], st.clone(), BASE_TS, "root");
        let mut side = st.clone();
        side.insert("FR-1", Spec::new("FR-1", "Functional", "f"));
        let s1 = d.write_commit(&[root.clone()], side, BASE_TS + 10, "side fr1");
        let mut a = st.clone();
        a.get_mut("BUG-3").unwrap().status = "Done".into();
        let a1 = d.write_commit(&[root.clone()], a.clone(), BASE_TS + 20, "main");
        let m = d.write_commit(&[a1, s1], a.clone(), BASE_TS + 30, "ours");
        let mut b = a.clone();
        b.insert("FR-1", Spec::new("FR-1", "Functional", "f"));
        let b1 = d.write_commit(&[m], b, BASE_TS + 40, "fr1 on main");
        d.set_head(&b1);
        let fx = &d.fx;
        let mut probes = Vec::new();
        for (max, status) in [(1usize, false), (2, false), (1, true), (2, true)] {
            let mut o = opts();
            o.id_filter = Some("FR-1".into());
            o.max_commits = max;
            o.max_commits_explicit = true;
            o.status_changes_only = status;
            probes.push((format!("--id FR-1 --max-commits {max} status={status}"), o));
        }
        let walks = walk_all(fx, &probes);
        let mut bad = Vec::new();
        history_cache::rebuild_full_at(&fx.store, &fx.db).unwrap();
        compare(&mut bad, fx, &probes, &walks, "complete");
        for (label, o) in &probes {
            assert!(
                test_support::query_only(&fx.store, &fx.db, o)
                    .unwrap()
                    .is_some(),
                "[{label}] a complete index serves it"
            );
        }
        for c in 1..=3usize {
            for k in 1..=5usize.div_ceil(c) {
                fx.drop_index();
                test_support::partial_build(&fx.store, &fx.db, c, k).unwrap();
                compare(&mut bad, fx, &probes, &walks, &format!("partial c{c} x{k}"));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    /// A side branch of `side_len` commits merged into main (which the
    /// index already holds), then `after` more main commits. Returns
    /// (dag, merge sha, head sha).
    fn long_side_then_more(side_len: i64, after: i64) -> (Dag, String, String) {
        let mut d = Dag::new();
        let mut st = State::new();
        st.insert("FR-1", Spec::new("FR-1", "Functional", "fr"));
        let root = d.write_commit(&[], st.clone(), BASE_TS, "root");
        let mut prev = root.clone();
        let mut s = st.clone();
        for i in 0..side_len {
            s.get_mut("FR-1").unwrap().description = format!("side {i}");
            prev = d.write_commit(&[prev.clone()], s.clone(), BASE_TS + 10 + i, "side");
        }
        let mut a = st.clone();
        a.insert("BUG-3", Spec::new("BUG-3", "Bug", "b"));
        let a1 = d.write_commit(&[root.clone()], a.clone(), BASE_TS + 5, "main");
        d.set_head(&a1);
        history_cache::rebuild_full_at(&d.fx.store, &d.fx.db).unwrap();
        let mut ms = s.clone();
        ms.insert("BUG-3", a["BUG-3"].clone());
        let m = d.write_commit(&[a1, prev], ms.clone(), BASE_TS + 500, "merge");
        let mut h = m.clone();
        for i in 0..after {
            ms.get_mut("BUG-3").unwrap().description = format!("after {i}");
            h = d.write_commit(&[h.clone()], ms.clone(), BASE_TS + 600 + i, "after");
        }
        d.set_head(&h);
        (d, m, h)
    }

    #[test]
    fn task_1507_long_side_branch_commits_at_the_merge_before_head() {
        // T4: the N6 path's own commit: the first zero-budget call must
        // stop AT the merge (not roll back, not only reach HEAD).
        let (d, m, h) = long_side_then_more(60, 5);
        test_support::index(&d.fx.store, &d.fx.db, Budget::for_duration(Duration::ZERO)).unwrap();
        assert_eq!(
            test_support::meta(&d.fx.db, "tip_sha").as_deref(),
            Some(m.as_str()),
            "the first call commits at the merge"
        );
        for _ in 0..10 {
            test_support::index(&d.fx.store, &d.fx.db, Budget::for_duration(Duration::ZERO))
                .unwrap();
        }
        assert_eq!(
            test_support::meta(&d.fx.db, "tip_sha").as_deref(),
            Some(h.as_str())
        );
        assert_parity(&d.fx, &opts(), "after the long side branch");
    }

    fn fast_import(store: &Path, stream: &str) {
        use std::io::Write;
        let mut ch = Command::new("git")
            .arg("-C")
            .arg(store)
            .args(["fast-import", "--quiet", "--force"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        ch.stdin
            .take()
            .unwrap()
            .write_all(stream.as_bytes())
            .unwrap();
        assert!(ch.wait().unwrap().success());
    }

    #[test]
    fn task_1507_catch_up_past_the_probe_cap_probes_only_merges() {
        // B6: HEAD fast-forwards to another hub's merge whose FIRST parent
        // forked before our tip. Reverse topo lists that foreign line, then
        // K of our descendants that can never be a boundary. Past the probe
        // cap only merges are probed, so the cost stays bounded.
        const K: usize = 500;
        let fx = Fixture::new();
        let mut s = String::new();
        let mut mark = 0usize;
        let mut t = BASE_TS;
        let mut commit = |s: &mut String, r: &str, parents: &[usize], t: i64| -> usize {
            mark += 1;
            s.push_str(&format!(
                "commit {r}\nmark :{mark}\ncommitter a <a@b> {t} +0000\ndata 1\nc\n"
            ));
            if let Some(p) = parents.first() {
                s.push_str(&format!("from :{p}\n"));
            }
            for p in parents.iter().skip(1) {
                s.push_str(&format!("merge :{p}\n"));
            }
            s.push_str(&format!("M 644 inline f{mark}\ndata 1\nx\n"));
            mark
        };
        let root = commit(&mut s, "refs/heads/aida-store", &[], t);
        let mut b = root;
        for _ in 0..3 {
            t += 1;
            b = commit(&mut s, "refs/heads/aida-store", &[b], t);
        }
        let mut x = b;
        for _ in 0..3 {
            t += 1;
            x = commit(&mut s, "refs/heads/aida-store", &[x], t);
        }
        let mut sd = b;
        for _ in 0..K {
            t += 1;
            sd = commit(&mut s, "refs/heads/side", &[sd], t);
        }
        let mut ours = x;
        for _ in 0..K {
            t += 1;
            ours = commit(&mut s, "refs/heads/ours", &[ours], t);
        }
        t += 1;
        commit(&mut s, "refs/heads/aida-store", &[sd, ours], t);
        s.push_str(&format!("reset refs/tags/X\nfrom :{x}\n"));
        fast_import(&fx.store, &s);
        let head = fx.git(&["rev-parse", "aida-store"]).trim().to_string();
        let xs = fx.git(&["rev-parse", "X"]).trim().to_string();
        fx.git(&["update-ref", "refs/heads/aida-store", &xs]);
        fx.git(&["symbolic-ref", "HEAD", "refs/heads/aida-store"]);
        history_cache::rebuild_full_at(&fx.store, &fx.db).unwrap();
        fx.git(&["update-ref", "refs/heads/aida-store", &head]);
        history_cache::take_boundary_probes();
        test_support::index(&fx.store, &fx.db, Budget::for_duration(Duration::ZERO)).unwrap();
        let probes = history_cache::take_boundary_probes();
        assert_eq!(
            test_support::meta(&fx.db, "tip_sha").as_deref(),
            Some(head.as_str()),
            "one zero-budget call reaches HEAD"
        );
        assert!(
            probes <= 40,
            "{probes} boundary probes for {K} non-boundary descendants"
        );
    }
}
