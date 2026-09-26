//! History index slice 2: where an answer came from (`source` /
//! `index_tip`), the human fallback footer, and the doctor freshness line.
//!
//! Every test builds a throwaway git store under its own temp dir, and the
//! index file resolves inside that temp dir; nothing here touches `~/.aida`,
//! the live store, or any shared location.
// trace:TASK-1508 | ai:claude

use crate::doctor_cmd::history_index_doctor_line;
use crate::history::{collect_event_records, fallback_footer_text, HistoryOpts, HistorySource};
use crate::history_cache::{self, HistoryCacheStatus};
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(store: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(store)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A two-commit store at `<tmp>/proj/store`, so the fallback index file
/// (`<tmp>/proj/store.history-*.db`) lands inside the test's temp dir.
fn store_with_history(tmp: &Path) -> PathBuf {
    let store = tmp.join("proj").join("store");
    std::fs::create_dir_all(&store).unwrap();
    git(&store, &["init", "-q", "-b", "aida-store"]);
    git(&store, &["config", "user.email", "t@example.com"]);
    git(&store, &["config", "user.name", "t"]);
    git(&store, &["config", "commit.gpgsign", "false"]);
    let yaml = store.join("objects/BUG/000/BUG-1.yaml");
    std::fs::create_dir_all(yaml.parent().unwrap()).unwrap();
    std::fs::write(
        &yaml,
        "spec_id: BUG-1\ntitle: t\nreq_type: Bug\nstatus: Draft\n",
    )
    .unwrap();
    git(&store, &["add", "-A"]);
    git(&store, &["commit", "-q", "-m", "seed"]);
    std::fs::write(
        &yaml,
        "spec_id: BUG-1\ntitle: t\nreq_type: Bug\nstatus: Approved\n",
    )
    .unwrap();
    git(&store, &["add", "-A"]);
    git(&store, &["commit", "-q", "-m", "flip"]);
    store
}

/// The doctor line with the index switched on (the default).
fn history_index_doctor_line_on(st: &HistoryCacheStatus) -> String {
    history_index_doctor_line(st, true)
}

fn opts() -> HistoryOpts {
    HistoryOpts {
        limit: 50,
        max_commits: 500,
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

#[test]
fn task_1508_index_answer_reports_source_and_tip() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_with_history(tmp.path());
    let head = git(&store, &["rev-parse", "HEAD"]);

    // trace:BUG-1643 | ai:claude
    // The query builds the index itself; under test that inline build is
    // unbounded, so this answer never depends on machine load.
    let served = with_index_on(|| collect_event_records(&store, &opts()).unwrap());
    assert_eq!(
        served.source,
        HistorySource::Index { tip: head.clone() },
        "a fresh small store is answered from the index, as of HEAD"
    );
    assert_eq!(served.source.as_str(), "history-cache");
    assert_eq!(served.source.index_tip(), Some(head.as_str()));
    assert_eq!(fallback_footer_text(&served.source, false), None);

    // Same events either way: provenance is additive, never a new answer.
    let walked = collect_event_records(&store, &opts()).unwrap();
    let as_json = |r: &crate::history::EventRecords| serde_json::to_value(&r.events).unwrap();
    assert_eq!(as_json(&served), as_json(&walked));
    assert!(!served.events.is_empty());
    assert!(history_cache::history_db_path(&store).starts_with(tmp.path().canonicalize().unwrap()));
}

#[test]
fn task_1508_switched_off_index_is_a_walk_without_footer() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_with_history(tmp.path());

    // Tests run with the index switched off unless they opt in, which is
    // the same path `AIDA_HISTORY_CACHE=0` takes.
    let rec = collect_event_records(&store, &opts()).unwrap();
    assert_eq!(rec.source, HistorySource::GitWalk { fallback: false });
    assert_eq!(rec.source.as_str(), "git-walk");
    assert_eq!(rec.source.index_tip(), None);
    assert_eq!(
        fallback_footer_text(&rec.source, false),
        None,
        "switching the index off on purpose is not news to the user"
    );
    assert!(
        !history_cache::history_db_path(&store).exists(),
        "a switched-off index is never created"
    );
}

#[test]
fn task_1508_unusable_index_falls_back_with_footer() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_with_history(tmp.path());
    // A directory where the index file should be: opening it fails, so the
    // query must fall back to the walk and say so.
    std::fs::create_dir_all(history_cache::history_db_path(&store)).unwrap();

    let rec = with_index_on(|| collect_event_records(&store, &opts()).unwrap());
    assert_eq!(rec.source, HistorySource::GitWalk { fallback: true });
    assert_eq!(rec.source.as_str(), "git-walk");
    assert_eq!(rec.source.index_tip(), None);
    assert!(!rec.events.is_empty(), "the walk still answers");

    let footer = fallback_footer_text(&rec.source, false).expect("a fallback owes a footer");
    assert_eq!(
        fallback_footer_text(&rec.source, true),
        None,
        "agent/piped output never gets the footer; MCP reports `source` instead"
    );
    // Worded for users: no internal nouns.
    for internal in ["index", "cache", "git", "walk", "sqlite", "history.db"] {
        assert!(
            !footer.to_ascii_lowercase().contains(internal),
            "footer leaks internal noun {internal:?}: {footer}"
        );
    }
}

#[test]
fn task_1508_doctor_line_states() {
    let base = HistoryCacheStatus {
        path: PathBuf::from("/x/history.db"),
        exists: true,
        head: Some("abc".into()),
        tip: Some("abc".into()),
        complete: true,
        events: 7,
        commits: 3,
        ..Default::default()
    };

    let missing = history_index_doctor_line_on(&HistoryCacheStatus {
        exists: false,
        ..Default::default()
    });
    assert!(missing.contains("not built yet"), "{missing}");
    assert!(
        missing.contains("aida cache rebuild --history"),
        "{missing}"
    );

    let unreadable = history_index_doctor_line_on(&HistoryCacheStatus {
        error: Some("file is not a database".into()),
        ..base.clone()
    });
    assert!(unreadable.contains("unreadable"), "{unreadable}");
    assert!(
        unreadable.contains("file is not a database"),
        "{unreadable}"
    );

    let fresh = history_index_doctor_line_on(&base.clone());
    assert!(fresh.contains("up to date"), "{fresh}");
    assert!(fresh.contains("7 event(s)"), "{fresh}");
    assert!(fresh.contains("whole store history"), "{fresh}");

    let filling = history_index_doctor_line_on(&HistoryCacheStatus {
        complete: false,
        floor_commit_at: Some("2026-09-20T10:00:00Z".into()),
        ..base.clone()
    });
    assert!(filling.contains("recent history"), "{filling}");
    assert!(
        filling.contains("back to 2026-09-20T10:00:00Z"),
        "{filling}"
    );
    assert!(filling.contains("still filling"), "{filling}");

    let behind = history_index_doctor_line_on(&HistoryCacheStatus {
        head: Some("def".into()),
        indexer_running: true,
        ..base.clone()
    });
    assert!(behind.contains("behind the store"), "{behind}");
    assert!(behind.contains("indexing now"), "{behind}");
}

#[test]
fn task_1508_doctor_line_reads_a_real_index() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_with_history(tmp.path());
    let db = tmp
        .path()
        .join("idx")
        .join(history_cache::history_db_file_name());
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();

    let before = history_index_doctor_line_on(&history_cache::status_at(&store, &db));
    assert!(before.contains("not built yet"), "{before}");
    assert!(!db.exists(), "the doctor line never creates the index");

    history_cache::rebuild_full_at(&store, &db).unwrap();
    let fresh = history_index_doctor_line_on(&history_cache::status_at(&store, &db));
    assert!(fresh.contains("up to date"), "{fresh}");
    assert!(fresh.contains("whole store history"), "{fresh}");

    let yaml = store.join("objects/BUG/000/BUG-1.yaml");
    std::fs::write(
        &yaml,
        "spec_id: BUG-1\ntitle: t2\nreq_type: Bug\nstatus: Approved\n",
    )
    .unwrap();
    git(&store, &["commit", "-qam", "retitle"]);
    let behind = history_index_doctor_line_on(&history_cache::status_at(&store, &db));
    assert!(behind.contains("behind the store"), "{behind}");
}

#[test]
fn task_1508_footer_is_off_for_agent_output_in_every_source() {
    let sources = [
        HistorySource::GitWalk { fallback: true },
        HistorySource::GitWalk { fallback: false },
        HistorySource::Index { tip: "abc".into() },
    ];
    for src in &sources {
        assert_eq!(fallback_footer_text(src, true), None, "{src:?}");
    }
    assert!(fallback_footer_text(&sources[0], false).is_some());
}

#[test]
fn task_1508_doctor_line_switched_off() {
    let built = HistoryCacheStatus {
        exists: true,
        tip: Some("abc".into()),
        head: Some("abc".into()),
        complete: true,
        ..Default::default()
    };
    for st in [HistoryCacheStatus::default(), built] {
        let line = history_index_doctor_line(&st, false);
        assert!(line.contains("switched off"), "{line}");
        assert!(line.contains("AIDA_HISTORY_CACHE=0"), "{line}");
        assert!(
            !line.contains("builds it as it goes") && !line.contains("up to date"),
            "{line}"
        );
    }
}
