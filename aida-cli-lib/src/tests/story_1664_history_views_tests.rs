// trace:STORY-1664 | ai:claude
use super::*;
use crate::history_indexer::tests::Fixture;

fn status(from: &str, to: &str) -> EventKind {
    EventKind::StatusChange {
        from: from.to_string(),
        to: to.to_string(),
        authority: None,
    }
}

fn added() -> EventKind {
    EventKind::Added {
        title: "t".to_string(),
        req_type: "Task".to_string(),
        priority: "Medium".to_string(),
        status: "Draft".to_string(),
    }
}

fn event(spec_id: &str, ts: &str, kind: EventKind) -> Event {
    Event {
        sha: format!("sha-{spec_id}-{ts}"),
        timestamp: ts.to_string(),
        author: "tester".to_string(),
        spec_id: spec_id.to_string(),
        req_type: "Task".to_string(),
        kind,
    }
}

fn summary(
    n: u128,
    spec_id: &str,
    req_type: &str,
    status: &str,
    tags: &[&str],
) -> aida_core::RequirementSummary {
    aida_core::RequirementSummary {
        id: uuid::Uuid::from_u128(n),
        spec_id: Some(spec_id.to_string()),
        agreed_id: None,
        title: format!("{spec_id} title"),
        description: String::new(),
        status: status.to_string(),
        priority: "Medium".to_string(),
        owner: "tester".to_string(),
        assignee: None,
        feature: "Uncategorized".to_string(),
        req_type: req_type.to_string(),
        tags: tags.iter().map(|t| t.to_string()).collect(),
        created_at: String::new(),
        modified_at: String::new(),
        archived: false,
        archived_at: None,
        deferred: false,
        deferred_at: None,
        deferred_until: None,
        deferred_reason: None,
        in_degree: 0,
        out_degree: 0,
        heft: 0,
        blocked: false,
        has_pending_decision: false,
        execution_mode: None,
        weight: None,
        origin: None,
        completed_at: None,
        yaml_path: String::new(),
    }
}

fn ids(rows: &[ViewRow]) -> Vec<String> {
    rows.iter()
        .map(|r| r.summary.spec_id.clone().unwrap_or_default())
        .collect()
}

#[test]
fn closed_selects_the_terminal_set_only() {
    let v = HistoryView::Closed;
    assert!(v.matches(&status("Approved", "Completed")));
    assert!(v.matches(&status("Draft", "Rejected")));
    assert!(v.matches(&status("Approved", "Superseded")));
    assert!(!v.matches(&status("Draft", "Approved")));
    assert!(!v.matches(&status("Draft", "Archived")));
    assert!(!v.matches(&added()));
}

#[test]
fn status_views_need_a_real_transition() {
    assert!(HistoryView::Completed.matches(&status("In Progress", "Completed")));
    assert!(!HistoryView::Completed.matches(&status("Approved", "Rejected")));
    assert!(HistoryView::Approved.matches(&status("Draft", "Approved")));
    assert!(HistoryView::Rejected.matches(&status("Draft", "Rejected")));
    // A respelling is not a transition.
    assert!(!HistoryView::Closed.matches(&status("completed", "Completed")));
}

#[test]
fn opened_selects_only_creations() {
    assert!(HistoryView::Opened.matches(&added()));
    assert!(!HistoryView::Opened.matches(&status("Draft", "Approved")));
}

#[test]
fn rows_show_current_status_one_per_spec_unless_all_events() {
    let events = vec![
        event(
            "TASK-1",
            "2026-10-14T03:00:00Z",
            status("Draft", "Approved"),
        ),
        event(
            "TASK-2",
            "2026-10-14T02:00:00Z",
            status("Draft", "Approved"),
        ),
        event(
            "TASK-1",
            "2026-10-14T01:00:00Z",
            status("Draft", "Approved"),
        ),
    ];
    let summaries = vec![
        summary(1, "TASK-1", "Task", "Completed", &[]),
        summary(2, "TASK-2", "Task", "Approved", &[]),
    ];
    let rows = select_rows(&events, &summaries, &ViewQuery::default());
    assert_eq!(ids(&rows), ["TASK-1", "TASK-2"]);
    assert_eq!(rows[0].when, "2026-10-14T03:00:00Z", "latest event wins");
    assert_eq!(rows[0].summary.status, "Completed", "current status shown");

    let all = ViewQuery {
        all_events: true,
        ..ViewQuery::default()
    };
    assert_eq!(
        ids(&select_rows(&events, &summaries, &all)),
        ["TASK-1", "TASK-2", "TASK-1"]
    );
}

#[test]
fn rows_filter_on_current_fields_and_skip_deleted_specs() {
    let events = vec![
        event("BUG-1", "2026-10-14T04:00:00Z", added()),
        event("TASK-2", "2026-10-14T03:00:00Z", added()),
        event("META-3", "2026-10-14T02:00:00Z", added()),
        event("TASK-GONE", "2026-10-14T01:00:00Z", added()),
    ];
    let summaries = vec![
        summary(1, "BUG-1", "Bug", "Draft", &["ui", "cli"]),
        summary(2, "TASK-2", "Task", "Draft", &["cli"]),
        summary(3, "META-3", "Meta", "Draft", &[]),
    ];
    let q = ViewQuery::default();
    assert_eq!(
        ids(&select_rows(&events, &summaries, &q)),
        ["BUG-1", "TASK-2"]
    );

    let bugs = ViewQuery {
        filters: ViewFilters {
            req_type: Some("bug".to_string()),
            ..ViewFilters::default()
        },
        ..ViewQuery::default()
    };
    assert_eq!(ids(&select_rows(&events, &summaries, &bugs)), ["BUG-1"]);

    let tagged = ViewQuery {
        filters: ViewFilters {
            tags: vec!["cli".to_string(), "ui".to_string()],
            ..ViewFilters::default()
        },
        ..ViewQuery::default()
    };
    assert_eq!(ids(&select_rows(&events, &summaries, &tagged)), ["BUG-1"]);

    let meta = ViewQuery {
        filters: ViewFilters {
            include_meta: true,
            ..ViewFilters::default()
        },
        ..ViewQuery::default()
    };
    assert_eq!(
        ids(&select_rows(&events, &summaries, &meta)),
        ["BUG-1", "TASK-2", "META-3"]
    );
}

#[test]
fn oldest_first_reverses_before_the_limit() {
    let events = vec![
        event("TASK-1", "2026-10-14T03:00:00Z", added()),
        event("TASK-2", "2026-10-14T02:00:00Z", added()),
        event("TASK-3", "2026-10-14T01:00:00Z", added()),
    ];
    let summaries = vec![
        summary(1, "TASK-1", "Task", "Draft", &[]),
        summary(2, "TASK-2", "Task", "Draft", &[]),
        summary(3, "TASK-3", "Task", "Draft", &[]),
    ];
    let q = ViewQuery {
        oldest_first: true,
        limit: Some(2),
        ..ViewQuery::default()
    };
    assert_eq!(
        ids(&select_rows(&events, &summaries, &q)),
        ["TASK-3", "TASK-2"]
    );
}

/// The fixture flips TASK-1661 Draft → Approved on every odd commit.
fn approved_query(max_commits: Option<usize>) -> ViewQuery {
    ViewQuery {
        max_commits,
        all_events: true,
        ..ViewQuery::default()
    }
}

#[test]
fn index_answers_the_whole_window_past_the_walk_cap() {
    let _env = crate::test_env::EnvVarGuard::set("AIDA_HISTORY_CACHE", "1");
    let fx = Fixture::new();
    for n in 1..=(VIEW_WALK_DEFAULT_MAX_COMMITS + 10) {
        fx.append(n);
    }
    let c = collect_events(&fx.store, HistoryView::Approved, &approved_query(None)).unwrap();
    assert!(
        matches!(c.source, HistorySource::Index { .. }),
        "{:?}",
        c.source
    );
    assert!(!c.truncated);
    assert!(c.truncated_at.is_none());
    assert_eq!(c.events.len(), (VIEW_WALK_DEFAULT_MAX_COMMITS + 10) / 2);
}

#[test]
fn capped_walk_reports_where_it_stopped() {
    let fx = Fixture::new();
    for n in 1..=20 {
        fx.append(n);
    }
    crate::history_cache::set_test_serve_enabled(false);
    let c = collect_events(&fx.store, HistoryView::Approved, &approved_query(Some(10))).unwrap();
    assert!(
        matches!(c.source, HistorySource::GitWalk { .. }),
        "{:?}",
        c.source
    );
    assert!(c.truncated);
    assert!(c.truncated_at.is_some());
    assert_eq!(c.events.len(), 5);
    assert!(truncation_notice(&c).unwrap().contains("--max-commits"));

    let summaries = vec![summary(1, "TASK-1661", "Task", "Approved", &[])];
    let rows = select_rows(&c.events, &summaries, &approved_query(Some(10)));
    let json: serde_json::Value =
        serde_json::from_str(&render_json(HistoryView::Approved, &c, &rows)).unwrap();
    assert_eq!(json["truncated"], true);
    assert_eq!(json["view"], "approved");
    assert_eq!(json["count"], 5);
    let toon = render_toon(HistoryView::Approved, &c, &rows);
    assert!(toon.contains("truncated: true"), "{toon}");
}

#[test]
fn opened_spec_since_completed_shows_filed_time_and_current_status() {
    let fx = Fixture::new();
    crate::history_cache::set_test_serve_enabled(false);
    let c = collect_events(&fx.store, HistoryView::Opened, &ViewQuery::default()).unwrap();
    assert_eq!(c.events.len(), 1);
    assert!(!c.truncated);
    let summaries = vec![summary(1, "TASK-1661", "Task", "Completed", &[])];
    let rows = select_rows(&c.events, &summaries, &ViewQuery::default());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].when, c.events[0].timestamp);
    assert_eq!(rows[0].summary.status, "Completed");
}

#[test]
fn cli_parses_each_view_with_list_style_filters() {
    use crate::cli::{Cli, Command, HistoryCommand};
    use clap::Parser;
    for (word, view) in [
        ("opened", HistoryView::Opened),
        ("completed", HistoryView::Completed),
        ("rejected", HistoryView::Rejected),
        ("approved", HistoryView::Approved),
        ("closed", HistoryView::Closed),
    ] {
        let cli = crate::TEST_CLI
            .try_parse_from([
                "aida",
                "history",
                word,
                "--since",
                "30d",
                "--type",
                "bug",
                "--tags",
                "cli,ui",
                "--priority",
                "high",
                "--all-events",
                "--oldest-first",
                "-n",
                "5",
            ])
            .unwrap();
        let crate::history::HistoryArgs {
            cmd: Some(ref cmd),
            ref since,
            ref r#type,
            limit,
            ..
        } = cli.command.registered::<crate::history::HistoryArgs>()
        else {
            panic!("{word}: not a history view");
        };
        let (parsed, args) = cmd.view().expect(word);
        assert_eq!(parsed, view);
        assert_eq!(args.tags, ["cli", "ui"]);
        assert_eq!(args.priority.as_deref(), Some("high"));
        assert!(args.all_events && args.oldest_first);
        assert_eq!(since.as_deref(), Some("30d"));
        assert_eq!(r#type.as_deref(), Some("bug"));
        assert_eq!(limit, Some(5));
    }
    // The existing surfaces are unchanged.
    let cli = crate::TEST_CLI
        .try_parse_from(["aida", "history", "TASK-1"])
        .unwrap();
    assert!(matches!(
        cli.command.registered::<crate::history::HistoryArgs>(),
        crate::history::HistoryArgs { cmd: None, .. }
    ));
    let cli = crate::TEST_CLI
        .try_parse_from(["aida", "history", "events"])
        .unwrap();
    assert!(matches!(
        cli.command.registered::<crate::history::HistoryArgs>(),
        crate::history::HistoryArgs {
            cmd: Some(HistoryCommand::Events { .. }),
            ..
        }
    ));
}

// ---- TASK-1954 sweep (TASK-1762 follow-ups) ----
// trace:TASK-1954 trace:TASK-1762 | ai:claude

fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[test]
fn view_status_filter_accepts_open_closed_groups_and_lists() {
    let closed = resolve_view_status_filter("closed").unwrap();
    for s in ["Done", "Completed", "Rejected", "Superseded"] {
        assert!(
            closed
                .iter()
                .any(|c| history::status_key(c) == history::status_key(s)),
            "closed lacks {s}: {closed:?}"
        );
    }
    let open = resolve_view_status_filter("open").unwrap();
    assert!(
        open.iter().any(|c| history::status_key(c) == "draft"),
        "{open:?}"
    );
    assert!(!open.iter().any(|c| history::status_key(c) == "completed"));
    assert_eq!(
        resolve_view_status_filter("accepted").unwrap(),
        ["Approved"]
    );
    assert_eq!(
        resolve_view_status_filter("draft,approved").unwrap().len(),
        2
    );
    let err = resolve_view_status_filter("bogus").unwrap_err().to_string();
    assert!(err.contains("`open` / `closed`"), "{err}");

    let events = vec![
        event("TASK-1", "2026-10-14T03:00:00Z", added()),
        event("TASK-2", "2026-10-14T02:00:00Z", added()),
    ];
    let summaries = vec![
        summary(1, "TASK-1", "Task", "Completed", &[]),
        summary(2, "TASK-2", "Task", "Draft", &[]),
    ];
    let q = ViewQuery {
        filters: ViewFilters {
            statuses: closed,
            ..ViewFilters::default()
        },
        ..ViewQuery::default()
    };
    assert_eq!(ids(&select_rows(&events, &summaries, &q)), ["TASK-1"]);
}

#[test]
fn view_priority_filter_refuses_an_unknown_value() {
    assert_eq!(resolve_view_priority_filter("HIGH").unwrap(), "High");
    let err = resolve_view_priority_filter("urgent")
        .unwrap_err()
        .to_string();
    assert!(err.contains("invalid priority `urgent`"), "{err}");
}

#[test]
fn json_when_carries_an_offset() {
    let when = when_rfc3339("2026-10-14 03:00");
    let parsed =
        chrono::DateTime::parse_from_rfc3339(&when).unwrap_or_else(|e| panic!("{when}: {e}"));
    assert_eq!(
        parsed.naive_local().format("%Y-%m-%d %H:%M").to_string(),
        "2026-10-14 03:00"
    );
    assert_eq!(when_rfc3339("2026-10-14T03:00:00Z"), "2026-10-14T03:00:00Z");
    assert_eq!(when_rfc3339("not a time"), "not a time");
}

/// A renumber writes the object at a new path, which the decoder reads as
/// an add for the new ID. `opened` must not list it as newly filed.
#[test]
fn opened_skips_a_renumbered_spec_filed_before_the_window() {
    let events = vec![
        event("TASK-9", "2026-10-14T03:00:00Z", added()),
        event("TASK-2", "2026-10-14T02:00:00Z", added()),
    ];
    let mut renumbered = summary(9, "TASK-9", "Task", "Draft", &[]);
    renumbered.created_at = "2026-01-01T00:00:00+00:00".to_string();
    let mut fresh = summary(2, "TASK-2", "Task", "Draft", &[]);
    fresh.created_at = "2026-10-14T02:00:00+00:00".to_string();
    let summaries = vec![renumbered, fresh];
    let floor = chrono::DateTime::parse_from_rfc3339("2026-10-07T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let q = ViewQuery {
        created_floor: Some(floor),
        ..ViewQuery::default()
    };
    assert_eq!(ids(&select_rows(&events, &summaries, &q)), ["TASK-2"]);
    // Without a floor (any non-`opened` view) nothing is dropped.
    assert_eq!(
        ids(&select_rows(&events, &summaries, &ViewQuery::default())),
        ["TASK-9", "TASK-2"]
    );
}

/// Golden for the human table: a `When` column in front of the `aida
/// list --fields id,type,status,priority,title` table.
#[test]
fn human_table_golden() {
    let rows = vec![
        ViewRow {
            when: "2026-10-14 03:00".to_string(),
            summary: summary(1, "TASK-1", "Task", "Completed", &[]),
        },
        ViewRow {
            when: "2026-10-13 21:15".to_string(),
            summary: summary(2, "BUG-22", "Bug", "Draft", &[]),
        },
    ];
    let lines: Vec<String> = render_human_table(&rows)
        .iter()
        .map(|l| strip_ansi(l).trim_end().to_string())
        .collect();
    let golden = [
        "When             ID     Type Status    Priority Title",
        "──────────────── ───────────────────────────────────────────",
        "2026-10-14 03:00 TASK-1 task completed medium   TASK-1 title",
        "2026-10-13 21:15 BUG-22 bug  draft     medium   BUG-22 title",
    ];
    assert_eq!(lines, golden, "actual:\n{}", lines.join("\n"));
}

// trace:TASK-3112 | ai:codex
#[test]
fn task3112_opened_without_since_uses_original_creation_time() {
    let events = vec![event("TASK-9", "2026-10-14T03:00:00Z", added())];
    let mut s = summary(9, "TASK-9", "Task", "Draft", &[]);
    s.created_at = "2026-01-01T00:00:00Z".into();
    let q = ViewQuery {
        created_floor: Some(chrono::DateTime::<chrono::Utc>::MIN_UTC),
        ..Default::default()
    };
    let rows = select_rows(&events, &[s.clone()], &q);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].when, history::human_timestamp(&s.created_at));
}
