// trace:STORY-1664 | ai:claude
use super::*;
use crate::history_indexer::tests::Fixture;

fn status(from: &str, to: &str) -> EventKind {
    EventKind::StatusChange {
        from: from.to_string(),
        to: to.to_string(),
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
        let cli = Cli::try_parse_from([
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
        let Command::History {
            cmd: Some(ref cmd),
            ref since,
            ref r#type,
            limit,
            ..
        } = cli.command
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
    let cli = Cli::try_parse_from(["aida", "history", "TASK-1"]).unwrap();
    assert!(matches!(cli.command, Command::History { cmd: None, .. }));
    let cli = Cli::try_parse_from(["aida", "history", "events"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::History {
            cmd: Some(HistoryCommand::Events { .. }),
            ..
        }
    ));
}
