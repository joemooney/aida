//! The status flow (bare `aida history`): rows from the history index, the
//! three output formats, and the fallback to the digest, which must never
//! run the per-commit git walk.
//!
//! Every test builds a throwaway git store under its own temp dir, so the
//! index file resolves inside that temp dir.
// trace:STORY-1650 | ai:claude

use crate::history::{self, git_walk_count, EventKind, HistoryOpts, HistoryOutput};
use crate::history_cache::{self, HistoryCacheStatus, IndexMiss};
use crate::history_flow::{self, FlowAnswer, FlowRow};
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(store: &Path, args: &[&str], date: Option<&str>) {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(store).args(args);
    if let Some(d) = date {
        cmd.env("GIT_AUTHOR_DATE", d).env("GIT_COMMITTER_DATE", d);
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write_spec(store: &Path, id: &str, title: &str, status: &str) {
    let rel = aida_core::object_store::relative_object_path(id).unwrap();
    let path = store.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let req_type = id.split('-').next().unwrap();
    std::fs::write(
        &path,
        format!("spec_id: {id}\ntitle: {title}\nreq_type: {req_type}\nstatus: {status}\n"),
    )
    .unwrap();
}

fn commit(store: &Path, msg: &str, date: &str) {
    git(store, &["add", "-A"], None);
    git(store, &["commit", "-q", "-m", msg], Some(date));
}

/// A store at `<tmp>/proj/store` (so the index file lands in the temp dir):
/// STORY-1 goes Draft → Approved → InProgress → Completed over three
/// commits; TASK-2 is filed straight at Approved in the last one.
fn store_with_flow(tmp: &Path) -> PathBuf {
    let store = tmp.join("proj").join("store");
    std::fs::create_dir_all(&store).unwrap();
    git(&store, &["init", "-q", "-b", "aida-store"], None);
    git(&store, &["config", "user.email", "t@example.com"], None);
    git(&store, &["config", "user.name", "t"], None);
    git(&store, &["config", "commit.gpgsign", "false"], None);
    write_spec(&store, "STORY-1", "Flowing story", "Draft");
    commit(&store, "add STORY-1", "2025-03-01T10:00:00Z");
    write_spec(&store, "STORY-1", "Flowing story", "Approved");
    commit(&store, "update STORY-1", "2025-03-02T10:00:00Z");
    write_spec(&store, "STORY-1", "Flowing story", "InProgress");
    commit(&store, "update STORY-1", "2025-03-03T10:00:00Z");
    write_spec(&store, "STORY-1", "Flowing story", "Completed");
    write_spec(&store, "TASK-2", "Filed approved", "Approved");
    commit(&store, "update STORY-1", "2025-03-04T10:00:00Z");
    store
}

fn opts() -> HistoryOpts {
    HistoryOpts {
        limit: 20,
        max_commits: 250,
        max_commits_explicit: false,
        events_mode: false,
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
        archived_specs: Default::default(),
        archived_only_specs: None,
        deferred_specs: Default::default(),
        deferred_only_specs: None,
        exclude_meta: false,
    }
}

/// Runs `f` with the index switched on for this test thread, and always
/// switches it back off.
fn with_index_on<T>(f: impl FnOnce() -> T) -> T {
    struct Off;
    impl Drop for Off {
        fn drop(&mut self) {
            history_cache::set_test_serve_enabled(false);
        }
    }
    let _off = Off;
    history_cache::set_test_serve_enabled(true);
    f()
}

fn transitions(rows: &[FlowRow]) -> Vec<(String, Option<String>, String)> {
    rows.iter()
        .map(|r| (r.id.clone(), r.from.clone(), r.to.clone()))
        .collect()
}

fn s(v: &str) -> String {
    v.to_string()
}

#[test]
fn story_1650_a_spec_with_several_transitions_gets_one_row_each() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_with_flow(tmp.path());
    let answer = with_index_on(|| history_flow::answer(&store, &opts())).expect("index answers");
    assert_eq!(
        transitions(&answer.rows),
        [
            (s("STORY-1"), Some(s("InProgress")), s("Completed")),
            (s("TASK-2"), None, s("Approved")),
            (s("STORY-1"), Some(s("Approved")), s("InProgress")),
            (s("STORY-1"), Some(s("Draft")), s("Approved")),
            (s("STORY-1"), None, s("Draft")),
        ]
    );
    assert!(answer.rows.iter().all(|r| !r.title.is_empty()));
    assert_eq!(answer.rows[1].kind, "added");
    assert_eq!(answer.rows[0].kind, "status_change");
}

#[test]
fn story_1650_window_selectors_narrow_the_flow_like_the_feed() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_with_flow(tmp.path());
    let run = |o: HistoryOpts| {
        transitions(
            &with_index_on(|| history_flow::answer(&store, &o))
                .unwrap()
                .rows,
        )
    };
    assert_eq!(
        run(HistoryOpts {
            to_status: Some(s("Completed")),
            ..opts()
        }),
        [(s("STORY-1"), Some(s("InProgress")), s("Completed"))]
    );
    assert_eq!(
        run(HistoryOpts {
            opened_only: true,
            ..opts()
        }),
        [
            (s("TASK-2"), None, s("Approved")),
            (s("STORY-1"), None, s("Draft"))
        ]
    );
    assert_eq!(run(HistoryOpts { limit: 2, ..opts() }).len(), 2);
}

#[test]
fn story_1650_flow_opts_mirror_the_events_feed_selectors() {
    let bare = history_flow::flow_opts(&opts());
    assert!(bare.events_mode && bare.status_changes_only && bare.opened_only);
    let to = history_flow::flow_opts(&HistoryOpts {
        to_status: Some(s("Approved")),
        ..opts()
    });
    assert!(!to.status_changes_only && !to.opened_only);
    let opened = history_flow::flow_opts(&HistoryOpts {
        opened_only: true,
        ..opts()
    });
    assert!(opened.opened_only && !opened.status_changes_only);
}

#[test]
fn story_1650_disabled_index_falls_back_without_the_git_walk() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_with_flow(tmp.path());
    // In-crate tests run with the index switched off unless a test opts in.
    assert_eq!(
        history_flow::answer(&store, &opts()).unwrap_err(),
        IndexMiss::Disabled
    );
    let before = git_walk_count();
    for output in [
        HistoryOutput::Human,
        HistoryOutput::Toon,
        HistoryOutput::Json,
    ] {
        history_flow::run(&store, &opts(), output).unwrap();
    }
    assert_eq!(git_walk_count(), before, "the fallback never walks git");
    // The counter does see a walk: the events feed with the index off.
    let mut feed = opts();
    feed.events_mode = true;
    history::collect_event_records(&store, &feed).unwrap();
    assert_eq!(git_walk_count(), before + 1);
}

#[test]
fn story_1650_stale_index_falls_back_without_the_git_walk() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_with_flow(tmp.path());
    let db = history_cache::history_db_path(&store);
    with_index_on(|| {
        history_flow::answer(&store, &opts()).expect("warm");
        // Pause inline catch-up (the stalled state), then move the store on.
        history_cache::test_support::set_meta(&db, "catch_up_failures", "3");
        write_spec(&store, "TASK-2", "Filed approved", "Completed");
        commit(&store, "update TASK-2", "2025-03-05T10:00:00Z");
        let before = git_walk_count();
        // The stall guard only applies to a budgeted (interactive) query.
        history_cache::set_test_flow_budget(Some(std::time::Duration::from_secs(30)));
        let miss = history_flow::answer(&store, &opts());
        history_flow::run(&store, &opts(), HistoryOutput::Toon).unwrap();
        history_cache::set_test_flow_budget(None);
        assert_eq!(miss.unwrap_err(), IndexMiss::Behind);
        assert_eq!(git_walk_count(), before, "the fallback never walks git");
    });
}

#[test]
fn story_1650_misses_are_classified_from_the_index_status() {
    let base = HistoryCacheStatus {
        exists: true,
        tip: Some(s("a")),
        head: Some(s("a")),
        complete: true,
        ..Default::default()
    };
    let cases = [
        (
            HistoryCacheStatus {
                exists: false,
                ..base.clone()
            },
            IndexMiss::NotBuilt,
        ),
        (
            HistoryCacheStatus {
                error: Some(s("bad")),
                ..base.clone()
            },
            IndexMiss::Unreadable,
        ),
        (
            HistoryCacheStatus {
                head: Some(s("b")),
                ..base.clone()
            },
            IndexMiss::Behind,
        ),
        (
            HistoryCacheStatus {
                complete: false,
                ..base.clone()
            },
            IndexMiss::Filling,
        ),
        (
            HistoryCacheStatus {
                indexer_running: true,
                ..base.clone()
            },
            IndexMiss::Busy,
        ),
        (base.clone(), IndexMiss::Unprovable),
    ];
    for (st, want) in cases {
        assert_eq!(history_cache::classify_miss(&st), want, "{st:?}");
    }
}

#[test]
fn story_1650_every_fallback_note_is_one_line_with_the_fix() {
    for miss in [
        IndexMiss::Disabled,
        IndexMiss::NotBuilt,
        IndexMiss::Behind,
        IndexMiss::Filling,
        IndexMiss::Busy,
        IndexMiss::Unreadable,
        IndexMiss::Unprovable,
    ] {
        let note = history_flow::fallback_note(miss);
        assert!(note.starts_with("note: "), "{note}");
        assert!(!note.contains('\n'), "{note}");
        assert!(note.contains("aida cache rebuild --history"), "{note}");
        assert!(!note.contains("STORY-"), "no internal IDs: {note}");
    }
    assert!(history_flow::fallback_note(IndexMiss::Disabled).contains("AIDA_HISTORY_CACHE=0"));
    assert!(history_flow::fallback_note(IndexMiss::Behind).contains("behind"));
}

fn sample() -> FlowAnswer {
    let row = |when: &str, id: &str, from: Option<&str>, to: &str, title: &str| FlowRow {
        when: s(when),
        id: s(id),
        from: from.map(s),
        to: s(to),
        title: s(title),
        kind: if from.is_some() {
            "status_change"
        } else {
            "added"
        },
        author: s("joe"),
        sha: s("abc123"),
    };
    FlowAnswer {
        rows: vec![
            row(
                "2025-03-04 10:00",
                "STORY-1",
                Some("InProgress"),
                "Completed",
                "Flowing story",
            ),
            row(
                "2025-03-04 10:00",
                "TASK-22",
                None,
                "Approved",
                "Filed approved",
            ),
        ],
        window_exhausted: false,
        tip: s("deadbeef"),
    }
}

#[test]
fn story_1650_human_output_aligns_one_line_per_transition() {
    colored::control::set_override(false);
    let (out, err) = history_flow::render(&sample(), HistoryOutput::Human).unwrap();
    colored::control::unset_override();
    assert!(err.is_empty());
    assert_eq!(
        out,
        "WHEN              ID       CHANGE                  TITLE\n\
         2025-03-04 10:00  STORY-1  InProgress → Completed  Flowing story\n\
         2025-03-04 10:00  TASK-22       (new) → Approved   Filed approved\n"
    );
}

#[test]
fn story_1650_toon_output_has_a_transitions_table() {
    let (out, _) = history_flow::render(&sample(), HistoryOutput::Toon).unwrap();
    assert_eq!(
        out,
        "view: history-flow\ncount: 2\n\
         transitions[2]{when,id,from,to,title}:\n  \
         \"2025-03-04 10:00\",STORY-1,InProgress,Completed,Flowing story\n  \
         \"2025-03-04 10:00\",TASK-22,(new),Approved,Filed approved\n"
    );
}

#[test]
fn story_1650_json_output_marks_creations_with_a_null_from() {
    let (out, _) = history_flow::render(&sample(), HistoryOutput::Json).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(doc["view"], "history-flow");
    assert_eq!(doc["count"], 2);
    assert_eq!(doc["source"], "history-cache");
    assert_eq!(doc["index_tip"], "deadbeef");
    assert_eq!(doc["window_exhausted"], false);
    let rows = doc["transitions"].as_array().unwrap();
    assert_eq!(rows[0]["from"], "InProgress");
    assert_eq!(rows[0]["kind"], "status_change");
    assert!(rows[1]["from"].is_null());
    assert_eq!(rows[1]["kind"], "added");
    assert_eq!(rows[1]["to"], "Approved");
}

#[test]
fn story_1650_an_empty_flow_says_so_on_stderr() {
    let empty = FlowAnswer {
        rows: vec![],
        window_exhausted: false,
        tip: s("t"),
    };
    let (out, err) = history_flow::render(&empty, HistoryOutput::Human).unwrap();
    assert!(out.is_empty());
    assert!(err.contains("no status changes"));
}

#[test]
fn story_1650_added_events_record_the_status_filed_at() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_with_flow(tmp.path());
    let mut feed = opts();
    feed.events_mode = true;
    feed.opened_only = true;
    let records = history::collect_event_records(&store, &feed).unwrap();
    assert_eq!(records.events.len(), 2);
    // The status is stored on the event but the feed's rendering of an
    // added event is unchanged.
    let added = EventKind::Added {
        title: s("t"),
        req_type: s("Task"),
        priority: s("High"),
        status: s("Approved"),
    };
    let json = serde_json::to_string(&added).unwrap();
    assert!(json.contains("\"status\":\"Approved\""));
    let v4: EventKind =
        serde_json::from_str(r#"{"Added":{"title":"t","req_type":"r","priority":"p"}}"#).unwrap();
    assert!(matches!(v4, EventKind::Added { status, .. } if status.is_empty()));
}
