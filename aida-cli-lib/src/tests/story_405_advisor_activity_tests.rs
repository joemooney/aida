use super::*;

#[test]
fn parses_minimal_pr_ship_activity_event() {
    let line = r#"{"ts":"2026-06-12T10:00:00Z","command":"aida pr ship","step":"pr-merge","status":"ok","pr":801}"#;
    let event = parse_advisor_activity_line(line).expect("valid event");
    assert_eq!(event.command, "aida pr ship");
    assert_eq!(event.step, "pr-merge");
    assert_eq!(event.status, "ok");
    assert_eq!(event.pr, Some(801));
    assert_eq!(event.target_label(), "PR #801");
}

#[test]
fn activity_reader_ignores_malformed_lines_filters_and_sorts_newest_first() {
    let tmp = tempfile::tempdir().unwrap();
    let aida_dir = tmp.path().join(".aida");
    std::fs::create_dir_all(&aida_dir).unwrap();
    std::fs::write(
            aida_dir.join("advisor-activity.jsonl"),
            concat!(
                "not-json\n",
                "{\"ts\":\"2026-06-12T10:00:00Z\",\"command\":\"aida pr ship\",\"step\":\"pr-watch-ci\",\"status\":\"ok\",\"pr\":1}\n",
                "{\"ts\":\"2026-06-12T10:10:00Z\",\"command\":\"aida pr ship\",\"step\":\"pr-merge\",\"status\":\"failed\",\"pr\":2}\n",
                "{\"ts\":\"garbage\",\"command\":\"aida pr ship\",\"step\":\"pr-pull\",\"status\":\"ok\"}\n",
            ),
        )
        .unwrap();
    let since = chrono::DateTime::parse_from_rfc3339("2026-06-12T10:05:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let events = read_advisor_activity_events(tmp.path(), Some(since));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].step, "pr-merge");
    assert_eq!(events[0].status, "failed");

    let all = read_advisor_activity_events(tmp.path(), None);
    assert_eq!(
        all.iter().map(|e| e.pr).collect::<Vec<_>>(),
        vec![Some(2), Some(1)]
    );
}
