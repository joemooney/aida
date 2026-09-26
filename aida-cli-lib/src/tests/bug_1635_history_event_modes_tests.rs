//! BUG-1635: per-event flags no longer fall back to the per-spec digest.
//! `--status-changes`, `--comments` and `--oneline` imply events mode for a
//! multi-spec query; a single SPEC-ID renders its progression in human,
//! TOON and JSON alike; the JSON rows carry `id`, `ts`, `author`, `kind`,
//! `from`, `to`, `summary`; and the history index serves exactly what the
//! git walk returns for every one of these shapes.
//!
//! Every test builds a throwaway git store (and index file) in its own
//! temp dir; nothing here touches `~/.aida` or the live store.
// trace:BUG-1635 | ai:claude

use crate::history::{
    collect_filtered_events_git, default_max_commits, events_json, progression_json,
    render_events_feed, render_progression_human, render_progression_toon, resolve_events_mode,
    Event, HistoryOpts, HistoryOutput, HistorySource, Progression,
};
use crate::history_cache;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

fn git(store: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(store)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", store)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write_spec(store: &Path, spec_id: &str, status: &str, priority: &str, comments: &[&str]) {
    let rel = aida_core::object_store::relative_object_path(spec_id).unwrap();
    let path = store.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut y = format!(
        "spec_id: {spec_id}\ntitle: {spec_id} title\nreq_type: Task\nstatus: {status}\npriority: {priority}\ncomments:\n"
    );
    for c in comments {
        y.push_str(&format!("  - author: joe\n    content: {c:?}\n"));
    }
    std::fs::write(path, y).unwrap();
}

fn commit(store: &Path, msg: &str) {
    git(store, &["add", "-A"]);
    git(store, &["commit", "-q", "-m", msg]);
}

/// TASK-1 goes Draft → Approved → In Progress (with a comment on the way
/// and a priority edit); TASK-2 goes Draft → Approved and gets a comment.
/// Three status transitions, two comment events, one priority change.
fn store(tmp: &Path) -> PathBuf {
    let store = tmp.join("store");
    std::fs::create_dir_all(&store).unwrap();
    git(&store, &["init", "-q", "-b", "aida-store"]);
    git(&store, &["config", "user.email", "t@example.com"]);
    git(&store, &["config", "user.name", "t"]);
    git(&store, &["config", "commit.gpgsign", "false"]);
    write_spec(&store, "TASK-1", "Draft", "Medium", &[]);
    write_spec(&store, "TASK-2", "Draft", "Medium", &[]);
    commit(&store, "add");
    write_spec(&store, "TASK-1", "Approved", "Medium", &[]);
    commit(&store, "approve 1");
    write_spec(&store, "TASK-1", "Approved", "High", &["looks good"]);
    commit(&store, "comment + priority 1");
    write_spec(&store, "TASK-2", "Approved", "Medium", &["go"]);
    commit(&store, "approve 2");
    write_spec(&store, "TASK-1", "In Progress", "High", &["looks good"]);
    commit(&store, "start 1");
    store
}

fn base() -> HistoryOpts {
    HistoryOpts {
        limit: 100,
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
        comments_only: false,
        oneline: false,
        archived_specs: HashSet::new(),
        archived_only_specs: None,
        deferred_specs: HashSet::new(),
        deferred_only_specs: None,
        exclude_meta: false,
    }
}

/// The opts the CLI builds for a multi-spec query with these flags.
fn multi(status_changes: bool, comments: bool, oneline: bool) -> HistoryOpts {
    HistoryOpts {
        events_mode: resolve_events_mode(
            false,
            false,
            false,
            status_changes,
            comments,
            oneline,
            false,
        ),
        status_changes_only: status_changes,
        comments_only: comments,
        oneline,
        ..base()
    }
}

/// The opts the CLI builds for `aida history <ID>` (optionally --comments).
fn single(id: &str, comments: bool) -> HistoryOpts {
    let events_mode = resolve_events_mode(false, true, false, false, comments, false, false);
    HistoryOpts {
        events_mode,
        id_filter: Some(id.to_string()),
        // The CLI folds "neither kind flag" into status changes.
        status_changes_only: !comments,
        comments_only: comments,
        ..base()
    }
}

/// Rows in a rendered view: lines that start with two spaces (the human
/// event lines, the human progression lines, and the TOON table rows).
fn rows(text: &str) -> usize {
    text.lines().filter(|l| l.starts_with("  ")).count()
}

fn toon_count(text: &str) -> usize {
    text.lines()
        .find_map(|l| l.strip_prefix("count: "))
        .expect("count line")
        .trim()
        .parse()
        .unwrap()
}

fn walk() -> HistorySource {
    HistorySource::GitWalk { fallback: false }
}

#[test]
fn per_event_flags_imply_events_mode_only_without_a_spec_id() {
    // Multi-spec: each per-event flag leaves the digest.
    for (sc, c, o, j) in [
        (true, false, false, false),
        (false, true, false, false),
        (false, false, true, false),
        (false, false, false, true),
    ] {
        assert!(resolve_events_mode(false, false, false, sc, c, o, j));
    }
    // No flag: the digest stays the default.
    assert!(!resolve_events_mode(
        false, false, false, false, false, false, false
    ));
    // With a SPEC-ID they shape the progression view instead.
    for (sc, c, o, j) in [
        (true, false, false, false),
        (false, true, false, false),
        (false, false, true, false),
        (false, false, false, true),
    ] {
        assert!(!resolve_events_mode(false, true, false, sc, c, o, j));
    }
    // --full/events and --shipped always select the feed.
    assert!(resolve_events_mode(
        true, true, false, false, false, false, false
    ));
    assert!(resolve_events_mode(
        false, true, true, false, false, false, false
    ));
    assert!(resolve_events_mode(
        false, false, true, false, false, false, false
    ));
}

#[test]
fn status_changes_human_toon_and_json_agree_on_event_count() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(tmp.path());
    let o = multi(true, false, false);
    assert!(o.events_mode, "--status-changes must imply events mode");
    let (events, _, _) = collect_filtered_events_git(&store, &o).unwrap();
    assert_eq!(events.len(), 3, "each transition is its own row");

    let human = render_events_feed(&events, &o, HistoryOutput::Human);
    let toon = render_events_feed(&events, &o, HistoryOutput::Toon);
    let json = events_json(&events, false, &walk());
    assert_eq!(rows(&human), 3, "{human}");
    assert_eq!(rows(&toon), 3, "{toon}");
    assert_eq!(toon_count(&toon), 3);
    assert_eq!(json["count"], 3);
    assert_eq!(json["events"].as_array().unwrap().len(), 3);
}

#[test]
fn comments_human_toon_and_json_agree_on_event_count() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(tmp.path());
    let o = multi(false, true, false);
    assert!(o.events_mode, "--comments must imply events mode");
    let (events, _, _) = collect_filtered_events_git(&store, &o).unwrap();
    assert_eq!(events.len(), 2);

    let human = render_events_feed(&events, &o, HistoryOutput::Human);
    let toon = render_events_feed(&events, &o, HistoryOutput::Toon);
    assert_eq!(rows(&human), 2, "{human}");
    assert_eq!(rows(&toon), 2, "{toon}");
    assert_eq!(toon_count(&toon), 2);
    assert_eq!(events_json(&events, false, &walk())["count"], 2);
}

#[test]
fn oneline_without_a_kind_flag_lists_every_event() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(tmp.path());
    let o = multi(false, false, true);
    assert!(o.events_mode, "--oneline must imply events mode");
    let (events, _, _) = collect_filtered_events_git(&store, &o).unwrap();
    let out = render_events_feed(&events, &o, HistoryOutput::Oneline);
    assert_eq!(out.lines().count(), events.len());
    assert!(out.lines().all(|l| l.contains("TASK-")), "{out}");
    // Both the transitions and the other edits are there, not one digest
    // row per spec.
    assert!(events.len() > 2);
}

fn progression_view<'a>(id: &'a str, events: &'a [Event]) -> Progression<'a> {
    Progression {
        id,
        title: format!("{id} title"),
        current_status: Some("In Progress".into()),
        events,
    }
}

#[test]
fn single_spec_progression_agrees_across_human_toon_and_json() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(tmp.path());
    let o = single("TASK-1", false);
    assert!(!o.events_mode, "a SPEC-ID keeps the progression view");
    let (mut events, _, _) = collect_filtered_events_git(&store, &o).unwrap();
    events.reverse();
    assert_eq!(events.len(), 2);
    let view = progression_view("TASK-1", &events);

    let human = render_progression_human(&view, &o);
    let toon = render_progression_toon(&view, &o);
    let json = progression_json(&view, &o, false, &walk());
    assert_eq!(rows(&human), 2, "{human}");
    assert_eq!(rows(&toon), 2, "{toon}");
    assert_eq!(toon_count(&toon), 2);
    assert_eq!(json["count"], 2);
    assert!(toon.starts_with("view: history-progression\n"), "{toon}");
    assert!(toon.contains("order: oldest-first"), "{toon}");

    // Oldest first, with old → new status in the dedicated columns.
    let rows_json = json["events"].as_array().unwrap();
    assert_eq!(rows_json[0]["from"], "Draft");
    assert_eq!(rows_json[0]["to"], "Approved");
    assert_eq!(rows_json[1]["from"], "Approved");
    assert_eq!(rows_json[1]["to"], "In Progress");
    assert_eq!(json["view"], "history-progression");
    assert_eq!(json["id"], "TASK-1");
    assert_eq!(json["current_status"], "In Progress");
    assert_eq!(json["order"], "oldest-first");
    let first_toon_row = toon.lines().find(|l| l.starts_with("  ")).unwrap();
    assert!(first_toon_row.contains("Draft"), "{first_toon_row}");
    assert!(first_toon_row.contains("Approved"), "{first_toon_row}");
}

#[test]
fn single_spec_comment_timeline_agrees_across_formats() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(tmp.path());
    let o = single("TASK-1", true);
    let (mut events, _, _) = collect_filtered_events_git(&store, &o).unwrap();
    events.reverse();
    assert_eq!(events.len(), 1);
    let view = progression_view("TASK-1", &events);
    let human = render_progression_human(&view, &o);
    let toon = render_progression_toon(&view, &o);
    assert_eq!(rows(&human), 1, "{human}");
    assert_eq!(rows(&toon), 1, "{toon}");
    assert!(
        toon.starts_with("view: history-comment-timeline\n"),
        "{toon}"
    );
    assert_eq!(progression_json(&view, &o, false, &walk())["count"], 1);
}

#[test]
fn json_rows_carry_the_documented_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(tmp.path());
    let mut o = multi(false, false, false);
    o.events_mode = true;
    let (events, _, _) = collect_filtered_events_git(&store, &o).unwrap();
    let json = events_json(&events, false, &walk());
    for row in json["events"].as_array().unwrap() {
        for key in ["id", "ts", "author", "kind", "from", "to", "summary"] {
            assert!(row.get(key).is_some(), "missing {key}: {row}");
        }
        assert_eq!(row["id"], row["spec_id"]);
        assert_eq!(row["ts"], row["timestamp"]);
        match row["kind"].as_str().unwrap() {
            "status_change" | "priority_change" => {
                assert!(row["from"].is_string() && row["to"].is_string(), "{row}");
            }
            _ => assert!(row["from"].is_null() && row["to"].is_null(), "{row}"),
        }
    }
    let prio = json["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["kind"] == "priority_change")
        .expect("a priority change");
    assert_eq!(prio["from"], "Medium");
    assert_eq!(prio["to"], "High");
}

/// The history index must answer every BUG-1635 query shape exactly as
/// the git walk does, and the rendered TOON/JSON must not depend on which
/// one answered.
#[test]
fn index_serves_the_same_answer_as_the_git_walk_for_every_new_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(tmp.path());
    let idx = tmp.path().join("idx");
    std::fs::create_dir_all(&idx).unwrap();
    let db = idx.join(history_cache::history_db_file_name());

    let cases: Vec<(&str, HistoryOpts)> = vec![
        ("--status-changes", multi(true, false, false)),
        ("--comments", multi(false, true, false)),
        ("--oneline", multi(false, false, true)),
        ("--status-changes --comments", multi(true, true, false)),
        ("<ID>", single("TASK-1", false)),
        ("<ID> --comments", single("TASK-1", true)),
        ("<ID> (other spec)", single("TASK-2", false)),
    ];
    for (label, o) in &cases {
        let (walked, hidden, exhausted) = collect_filtered_events_git(&store, o).unwrap();
        let served = history_cache::serve_at(&store, &db, o, Duration::from_secs(120))
            .unwrap()
            .unwrap_or_else(|| panic!("[{label}] index did not serve"));
        assert_eq!(served.events, walked, "[{label}] events differ");
        assert_eq!(served.hidden_archived, hidden, "[{label}] hidden differs");
        assert_eq!(
            served.window_exhausted, exhausted,
            "[{label}] window_exhausted differs"
        );

        if o.events_mode {
            for out in [HistoryOutput::Toon, HistoryOutput::Oneline] {
                assert_eq!(
                    render_events_feed(&served.events, o, out),
                    render_events_feed(&walked, o, out),
                    "[{label}] {out:?} differs"
                );
            }
            assert_eq!(
                events_json(&served.events, exhausted, &walk()),
                events_json(&walked, exhausted, &walk()),
                "[{label}] JSON differs"
            );
        } else {
            let id = o.id_filter.as_deref().unwrap();
            let mut a = served.events.clone();
            let mut b = walked.clone();
            a.reverse();
            b.reverse();
            let va = progression_view(id, &a);
            let vb = progression_view(id, &b);
            assert_eq!(
                render_progression_toon(&va, o),
                render_progression_toon(&vb, o),
                "[{label}] TOON differs"
            );
            assert_eq!(
                progression_json(&va, o, exhausted, &walk()),
                progression_json(&vb, o, exhausted, &walk()),
                "[{label}] JSON differs"
            );
        }
    }
}

/// The default commit window follows the mode, not the output format:
/// adding `--json` to a filtered query keeps the 250-commit walk.
#[test]
fn json_does_not_change_the_commit_window_of_a_filtered_query() {
    let limit = 20;
    // (explicit_events, single_spec, json, shipped, status, comments, oneline)
    let w = |e, id, j, sh, sc, c, o| default_max_commits(limit, e, id, j, sh, sc, c, o);
    // --status-changes and --status-changes --json: both 250.
    assert_eq!(w(false, false, false, false, true, false, false), 250);
    assert_eq!(w(false, false, true, false, true, false, false), 250);
    // --shipped and --shipped --json: both 250.
    assert_eq!(w(false, false, false, true, false, false, false), 250);
    assert_eq!(w(false, false, true, true, false, false, false), 250);
    // --comments --json and --oneline --json: 250.
    assert_eq!(w(false, false, true, false, false, true, false), 250);
    assert_eq!(w(false, false, true, false, false, false, true), 250);
    // <SPEC-ID> --json: 250.
    assert_eq!(w(false, true, true, false, false, false, false), 250);
    // A bare --json and --full/events: 5x limit, at least 50.
    assert_eq!(w(false, false, true, false, false, false, false), 100);
    assert_eq!(w(true, false, false, false, false, false, false), 100);
    assert_eq!(w(true, false, true, true, false, false, false), 100);
    assert_eq!(
        default_max_commits(2, true, false, false, false, false, false, false),
        50
    );
    // The digest: 250.
    assert_eq!(w(false, false, false, false, false, false, false), 250);
}
