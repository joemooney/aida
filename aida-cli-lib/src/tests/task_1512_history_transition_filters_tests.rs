//! TASK-1512: `aida history --to <status>` / `--from <status>` select status
//! transitions by target and source status, and `--opened` (alias
//! `--created`) selects spec-creation events. All three imply events mode,
//! use the 250-commit default window, compose with the other filters, and
//! get the same answer from the history index as from the git walk.
//!
//! Every test builds a throwaway git store (and index file) in its own temp
//! dir; nothing here touches `~/.aida` or the live store.
// trace:TASK-1512 | ai:claude

use crate::cli::{Cli, Command};
use crate::history::{
    collect_filtered_events_git, default_max_commits, events_json, render_events_feed,
    resolve_events_mode, resolve_status_filter, Event, EventKind, HistoryOpts, HistoryOutput,
    HistorySource,
};
use crate::history_cache;
use clap::Parser;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::Duration;

fn git(store: &Path, args: &[&str]) {
    let out = ProcessCommand::new("git")
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

fn write_spec(store: &Path, spec_id: &str, req_type: &str, status: &str, comments: &[&str]) {
    let rel = aida_core::object_store::relative_object_path(spec_id).unwrap();
    let path = store.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut y = format!(
        "spec_id: {spec_id}\ntitle: {spec_id} title\nreq_type: {req_type}\nstatus: {status}\npriority: Medium\ncomments:\n"
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

/// Oldest to newest:
/// 1. TASK-1 and TASK-2 filed as Draft.
/// 2. TASK-1 Draft → Approved.
/// 3. BUG-3 filed directly at Approved.
/// 4. TASK-1 Approved → In Progress; TASK-2 gets a comment.
/// 5. TASK-1 In Progress → Approved (bounced back).
/// 6. BUG-3 Approved → In Progress.
/// 7. BUG-3 In Progress → Completed.
///
/// Five transitions, three creations, one comment event.
fn store(tmp: &Path) -> PathBuf {
    let store = tmp.join("store");
    std::fs::create_dir_all(&store).unwrap();
    git(&store, &["init", "-q", "-b", "aida-store"]);
    git(&store, &["config", "user.email", "t@example.com"]);
    git(&store, &["config", "user.name", "t"]);
    git(&store, &["config", "commit.gpgsign", "false"]);
    write_spec(&store, "TASK-1", "Task", "Draft", &[]);
    write_spec(&store, "TASK-2", "Task", "Draft", &[]);
    commit(&store, "file 1 and 2");
    write_spec(&store, "TASK-1", "Task", "Approved", &[]);
    commit(&store, "approve 1");
    write_spec(&store, "BUG-3", "Bug", "Approved", &[]);
    commit(&store, "file 3 approved");
    write_spec(&store, "TASK-1", "Task", "In Progress", &[]);
    write_spec(&store, "TASK-2", "Task", "Draft", &["later"]);
    commit(&store, "start 1, comment 2");
    write_spec(&store, "TASK-1", "Task", "Approved", &[]);
    commit(&store, "bounce 1");
    write_spec(&store, "BUG-3", "Bug", "InProgress", &[]);
    commit(&store, "start 3");
    write_spec(&store, "BUG-3", "Bug", "Completed", &[]);
    commit(&store, "ship 3");
    store
}

fn base() -> HistoryOpts {
    HistoryOpts {
        limit: 100,
        max_commits: 250,
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

/// The opts the CLI builds for `--to`/`--from`/`--opened` as typed.
fn sel(to: Option<&str>, from: Option<&str>, opened: bool) -> HistoryOpts {
    HistoryOpts {
        to_status: to.map(|s| resolve_status_filter("--to", s).unwrap()),
        from_status: from.map(|s| resolve_status_filter("--from", s).unwrap()),
        opened_only: opened,
        ..base()
    }
}

fn walk(store: &Path, o: &HistoryOpts) -> Vec<Event> {
    collect_filtered_events_git(store, o).unwrap().0
}

/// `(spec_id, from, to)` for each status transition, `(spec_id, "+", "")`
/// for a creation, `(spec_id, "c", "")` for a comment.
fn shape(events: &[Event]) -> Vec<(String, String, String)> {
    events
        .iter()
        .map(|e| match &e.kind {
            EventKind::StatusChange { from, to } => (e.spec_id.clone(), from.clone(), to.clone()),
            EventKind::Added { .. } => (e.spec_id.clone(), "+".into(), String::new()),
            EventKind::CommentsAdded { .. } => (e.spec_id.clone(), "c".into(), String::new()),
            other => panic!("unexpected event kind {other:?}"),
        })
        .collect()
}

fn t(id: &str, from: &str, to: &str) -> (String, String, String) {
    (id.into(), from.into(), to.into())
}

#[test]
fn to_lists_only_transitions_into_that_status_newest_first() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(tmp.path());
    let events = walk(&store, &sel(Some("approved"), None, false));
    assert_eq!(
        shape(&events),
        vec![
            t("TASK-1", "In Progress", "Approved"),
            t("TASK-1", "Draft", "Approved")
        ]
    );
}

#[test]
fn from_and_to_must_both_match_and_from_alone_matches_any_exit() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(tmp.path());
    let both = walk(&store, &sel(Some("approved"), Some("in-progress"), false));
    assert_eq!(shape(&both), vec![t("TASK-1", "In Progress", "Approved")]);

    // `--from in-progress` matches the stored `In Progress` and
    // `InProgress` spellings alike.
    let from = walk(&store, &sel(None, Some("in-progress"), false));
    assert_eq!(
        shape(&from),
        vec![
            t("BUG-3", "InProgress", "Completed"),
            t("TASK-1", "In Progress", "Approved")
        ]
    );
}

#[test]
fn status_values_accept_edit_spellings_and_aliases() {
    for raw in [
        "in-progress",
        "in_progress",
        "InProgress",
        "IN-PROGRESS",
        "in progress",
    ] {
        assert_eq!(
            resolve_status_filter("--to", raw).unwrap(),
            "InProgress",
            "{raw}"
        );
    }
    assert_eq!(
        resolve_status_filter("--to", "accepted").unwrap(),
        "Approved"
    );
    assert_eq!(
        resolve_status_filter("--to", "Approved").unwrap(),
        "Approved"
    );
    assert_eq!(
        resolve_status_filter("--from", "needs-attention").unwrap(),
        "NeedsAttention"
    );
    assert_eq!(
        resolve_status_filter("--to", "superseded").unwrap(),
        "Superseded"
    );

    let err = resolve_status_filter("--to", "approvedd")
        .unwrap_err()
        .to_string();
    assert!(err.contains("--to"), "{err}");
    assert!(err.contains("approvedd"), "{err}");
    for valid in [
        "draft",
        "approved",
        "in-progress",
        "completed",
        "needs-attention",
    ] {
        assert!(err.contains(valid), "missing {valid}: {err}");
    }

    // `accepted` finds the same events as `approved`.
    let tmp = tempfile::tempdir().unwrap();
    let store = store(tmp.path());
    assert_eq!(
        walk(&store, &sel(Some("accepted"), None, false)),
        walk(&store, &sel(Some("approved"), None, false))
    );
}

#[test]
fn opened_lists_creations_including_specs_filed_past_draft() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(tmp.path());
    let opened = walk(&store, &sel(None, None, true));
    assert_eq!(
        shape(&opened),
        vec![
            t("BUG-3", "+", ""),
            // One commit filed both; they keep git's path order.
            t("TASK-1", "+", ""),
            t("TASK-2", "+", ""),
        ]
    );
    // Creation is not a transition: `--to draft` finds nothing, and BUG-3
    // (filed at Approved) is only reachable through `--opened`.
    assert!(walk(&store, &sel(Some("draft"), None, false)).is_empty());
}

#[test]
fn opened_combined_with_to_or_from_is_the_union() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(tmp.path());
    let union = walk(&store, &sel(Some("approved"), None, true));
    assert_eq!(
        shape(&union),
        vec![
            t("TASK-1", "In Progress", "Approved"),
            t("BUG-3", "+", ""),
            t("TASK-1", "Draft", "Approved"),
            t("TASK-1", "+", ""),
            t("TASK-2", "+", ""),
        ]
    );

    // `--status-changes --to approved` narrows to the approvals.
    let narrowed = walk(
        &store,
        &HistoryOpts {
            status_changes_only: true,
            ..sel(Some("approved"), None, false)
        },
    );
    assert_eq!(narrowed, walk(&store, &sel(Some("approved"), None, false)));

    // `--comments --to approved` is the union of comments and approvals.
    let with_comments = walk(
        &store,
        &HistoryOpts {
            comments_only: true,
            ..sel(Some("approved"), None, false)
        },
    );
    assert_eq!(
        shape(&with_comments),
        vec![
            t("TASK-1", "In Progress", "Approved"),
            t("TASK-2", "c", ""),
            t("TASK-1", "Draft", "Approved"),
        ]
    );
}

#[test]
fn to_completed_is_exactly_shipped() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(tmp.path());
    let shipped = walk(
        &store,
        &HistoryOpts {
            shipped_only: true,
            ..base()
        },
    );
    assert_eq!(shape(&shipped), vec![t("BUG-3", "InProgress", "Completed")]);
    assert_eq!(walk(&store, &sel(Some("completed"), None, false)), shipped);
}

#[test]
fn filters_compose_with_id_type_author_limit_window_and_view_filters() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(tmp.path());

    let by_id = HistoryOpts {
        id_filter: Some("BUG-3".into()),
        ..sel(None, None, true)
    };
    assert_eq!(shape(&walk(&store, &by_id)), vec![t("BUG-3", "+", "")]);

    let by_type = HistoryOpts {
        type_filter: Some("task".into()),
        ..sel(None, None, true)
    };
    assert_eq!(walk(&store, &by_type).len(), 2);

    let by_author = HistoryOpts {
        author_filter: Some("nobody".into()),
        ..sel(Some("approved"), None, false)
    };
    assert!(walk(&store, &by_author).is_empty());

    let limited = HistoryOpts {
        limit: 1,
        ..sel(Some("approved"), None, false)
    };
    assert_eq!(
        shape(&walk(&store, &limited)),
        vec![t("TASK-1", "In Progress", "Approved")]
    );

    // A window after every commit finds nothing; one before them all
    // finds everything.
    let later = HistoryOpts {
        since: Some((chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339()),
        ..sel(Some("approved"), None, true)
    };
    assert!(walk(&store, &later).is_empty());
    let earlier = HistoryOpts {
        since: Some((chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339()),
        ..sel(Some("approved"), None, true)
    };
    assert_eq!(walk(&store, &earlier).len(), 5);

    // The default view hides archived specs; `--archived` narrows to them.
    let hide = HistoryOpts {
        archived_specs: HashSet::from(["TASK-1".to_string()]),
        ..sel(Some("approved"), None, true)
    };
    assert_eq!(
        shape(&walk(&store, &hide)),
        vec![t("BUG-3", "+", ""), t("TASK-2", "+", "")]
    );
    let only = HistoryOpts {
        deferred_only_specs: Some(HashSet::from(["TASK-2".to_string()])),
        ..sel(Some("approved"), None, true)
    };
    assert_eq!(shape(&walk(&store, &only)), vec![t("TASK-2", "+", "")]);

    // --oneline renders one line per selected event.
    let o = HistoryOpts {
        oneline: true,
        ..sel(Some("approved"), None, false)
    };
    let events = walk(&store, &o);
    let text = render_events_feed(&events, &o, HistoryOutput::Oneline);
    assert_eq!(text.lines().count(), 2, "{text}");
}

#[test]
fn selectors_imply_events_mode_on_the_250_commit_window() {
    // With or without a SPEC-ID, and whatever the output format.
    for single in [false, true] {
        assert!(resolve_events_mode(
            false, single, true, false, false, false, false
        ));
        assert!(resolve_events_mode(
            false, single, true, false, false, false, true
        ));
    }
    for json in [false, true] {
        for single in [false, true] {
            assert_eq!(
                default_max_commits(20, false, single, json, true, false, false, false),
                250
            );
        }
    }
}

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

#[test]
fn human_toon_and_json_agree() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(tmp.path());
    for (label, o, n) in [
        ("--to approved", sel(Some("approved"), None, false), 2),
        (
            "--from in-progress",
            sel(None, Some("in-progress"), false),
            2,
        ),
        ("--opened", sel(None, None, true), 3),
        (
            "--opened --to approved",
            sel(Some("approved"), None, true),
            5,
        ),
    ] {
        let events = walk(&store, &o);
        assert_eq!(events.len(), n, "[{label}]");
        let human = render_events_feed(&events, &o, HistoryOutput::Human);
        let toon = render_events_feed(&events, &o, HistoryOutput::Toon);
        let json = events_json(&events, false, &HistorySource::GitWalk { fallback: false });
        assert_eq!(rows(&human), n, "[{label}] {human}");
        assert_eq!(rows(&toon), n, "[{label}] {toon}");
        assert_eq!(toon_count(&toon), n, "[{label}]");
        assert_eq!(json["count"], n, "[{label}]");
        // JSON carries the transition ends the filter selected on.
        for (row, e) in json["events"].as_array().unwrap().iter().zip(&events) {
            match &e.kind {
                EventKind::StatusChange { from, to } => {
                    assert_eq!(row["kind"], "status_change");
                    assert_eq!(row["from"], from.as_str());
                    assert_eq!(row["to"], to.as_str());
                }
                EventKind::Added { .. } => assert_eq!(row["kind"], "added"),
                other => panic!("[{label}] unexpected {other:?}"),
            }
        }
    }
}

/// The history index narrows these queries in SQL on the stored event
/// kind and re-checks `--to`/`--from` in Rust; it must return exactly what
/// the git walk does, and render identically.
#[test]
fn index_serves_the_same_answer_as_the_git_walk_for_transition_filters() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(tmp.path());
    let idx = tmp.path().join("idx");
    std::fs::create_dir_all(&idx).unwrap();
    let db = idx.join(history_cache::history_db_file_name());

    let cases: Vec<(&str, HistoryOpts)> = vec![
        ("--to approved", sel(Some("approved"), None, false)),
        ("--from in-progress", sel(None, Some("in-progress"), false)),
        (
            "--from in-progress --to approved",
            sel(Some("approved"), Some("in-progress"), false),
        ),
        ("--to completed", sel(Some("completed"), None, false)),
        ("--opened", sel(None, None, true)),
        ("--opened --to approved", sel(Some("approved"), None, true)),
        (
            "--status-changes --to approved",
            HistoryOpts {
                status_changes_only: true,
                ..sel(Some("approved"), None, false)
            },
        ),
        (
            "--comments --to approved",
            HistoryOpts {
                comments_only: true,
                ..sel(Some("approved"), None, false)
            },
        ),
        (
            "--opened --id BUG-3",
            HistoryOpts {
                id_filter: Some("BUG-3".into()),
                ..sel(None, None, true)
            },
        ),
        (
            "--to approved -n 1",
            HistoryOpts {
                limit: 1,
                ..sel(Some("approved"), None, false)
            },
        ),
        (
            "--opened --max-commits 3",
            HistoryOpts {
                max_commits: 3,
                max_commits_explicit: true,
                ..sel(None, None, true)
            },
        ),
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
        for out in [
            HistoryOutput::Toon,
            HistoryOutput::Oneline,
            HistoryOutput::Human,
        ] {
            assert_eq!(
                render_events_feed(&served.events, o, out),
                render_events_feed(&walked, o, out),
                "[{label}] {out:?} differs"
            );
        }
        let src = HistorySource::GitWalk { fallback: false };
        assert_eq!(
            events_json(&served.events, exhausted, &src),
            events_json(&walked, exhausted, &src),
            "[{label}] JSON differs"
        );
    }
}

#[test]
fn cli_parses_to_from_opened_and_the_created_alias() {
    let cli = Cli::try_parse_from([
        "aida",
        "history",
        "--from",
        "in-progress",
        "--to",
        "approved",
        "--since",
        "7d",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        Command::History { to: Some(ref t), from: Some(ref f), .. }
            if t == "approved" && f == "in-progress"
    ));
    for flag in ["--opened", "--created"] {
        let cli = Cli::try_parse_from(["aida", "history", flag]).unwrap();
        assert!(
            matches!(cli.command, Command::History { opened: true, .. }),
            "{flag}"
        );
    }
    // Global, like the other filters: they parse after `events` too.
    assert!(Cli::try_parse_from(["aida", "history", "events", "--to", "approved"]).is_ok());
    // `--shipped` is `--to completed`; combining it with --to or --opened
    // is refused rather than silently intersected.
    assert!(Cli::try_parse_from(["aida", "history", "--shipped", "--to", "approved"]).is_err());
    assert!(Cli::try_parse_from(["aida", "history", "--shipped", "--opened"]).is_err());
    assert!(Cli::try_parse_from(["aida", "history", "--shipped", "--from", "in-progress"]).is_ok());
}

/// `--help` names the flags and carries no internal spec IDs.
#[test]
fn help_describes_the_flags_without_spec_ids() {
    use clap::CommandFactory;
    let mut cmd = Cli::command();
    let history = cmd
        .find_subcommand_mut("history")
        .expect("history subcommand");
    let help = history.render_long_help().to_string();
    for needle in ["--to <STATUS>", "--from <STATUS>", "--opened", "created"] {
        assert!(help.contains(needle), "missing {needle}");
    }
    let spec_id = regex::Regex::new(r"\b(TASK|BUG|STORY|SPIKE|FR|EPIC)-\d+").unwrap();
    for arg in ["to", "from", "opened", "shipped"] {
        let a = history
            .get_arguments()
            .find(|a| a.get_id() == arg)
            .unwrap_or_else(|| panic!("no --{arg}"));
        let text = format!("{:?} {:?}", a.get_help(), a.get_long_help());
        assert!(
            !spec_id.is_match(&text),
            "--{arg} help leaks a spec ID: {text}"
        );
    }
}
