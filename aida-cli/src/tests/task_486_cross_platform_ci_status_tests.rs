use super::*;
use chrono::TimeZone;

fn run(
    status: &str,
    conclusion: Option<&str>,
    hours_ago: i64,
    id: u64,
    now: chrono::DateTime<chrono::Utc>,
) -> GhWorkflowRun {
    GhWorkflowRun {
        status: Some(status.to_string()),
        conclusion: conclusion.map(str::to_string),
        created_at: Some(now - chrono::Duration::hours(hours_ago)),
        database_id: Some(id),
        url: None,
    }
}

#[test]
fn green_recent_cross_platform_ci_marks_release_ready() {
    let now = chrono::Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap();
    let summary = summarize_cross_platform_ci_runs(
        now,
        Ok(vec![run("completed", Some("success"), 6, 26321099991, now)]),
    );

    assert_eq!(summary.release_gate, CrossPlatformReleaseGate::Ready);
    assert!(summary.summary.contains("green"));
    assert!(summary.summary.contains("6h ago"));
    assert!(summary.detail.is_none());
}

#[test]
fn red_cross_platform_ci_blocks_release_and_reports_last_green() {
    let now = chrono::Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap();
    let summary = summarize_cross_platform_ci_runs(
        now,
        Ok(vec![
            run("completed", Some("failure"), 14, 26280474891, now),
            run("completed", Some("success"), 50, 26154859707, now),
        ]),
    );

    assert_eq!(summary.release_gate, CrossPlatformReleaseGate::Blocked);
    assert!(summary.summary.contains("failure"));
    assert!(summary.summary.contains("14h ago"));
    assert!(summary.summary.contains("26280474891"));
    assert_eq!(
        summary.detail.as_deref(),
        Some("Last green: 2 days ago. Releases require <24h green.")
    );
}

#[test]
fn stale_green_cross_platform_ci_blocks_release() {
    let now = chrono::Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap();
    let summary = summarize_cross_platform_ci_runs(
        now,
        Ok(vec![run(
            "completed",
            Some("success"),
            25,
            26280474891,
            now,
        )]),
    );

    assert_eq!(summary.release_gate, CrossPlatformReleaseGate::Blocked);
    assert!(summary.summary.contains("stale"));
    assert!(summary.summary.contains("25h ago"));
    assert_eq!(
        summary.detail.as_deref(),
        Some("Last green: 25h ago. Releases require <24h green.")
    );
}

#[test]
fn unreachable_gh_reports_unknown_without_blocking_status_command() {
    let now = chrono::Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap();
    let summary = summarize_cross_platform_ci_runs(now, Err("gh unavailable".to_string()));

    assert_eq!(summary.release_gate, CrossPlatformReleaseGate::Unknown);
    assert!(summary.summary.contains("unknown"));
    assert!(summary.summary.contains("gh unreachable"));
    assert!(summary
        .detail
        .as_deref()
        .unwrap()
        .contains("cannot be confirmed"));
}
