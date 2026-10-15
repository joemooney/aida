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
        head_sha: None,
        url: None,
    }
}

fn run_at_sha(
    status: &str,
    conclusion: Option<&str>,
    hours_ago: i64,
    id: u64,
    now: chrono::DateTime<chrono::Utc>,
    head_sha: &str,
) -> GhWorkflowRun {
    GhWorkflowRun {
        head_sha: Some(head_sha.to_string()),
        ..run(status, conclusion, hours_ago, id, now)
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

#[test]
fn scheduled_failures_render_nightly_red_streak() {
    let now = chrono::Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap();
    let red = summarize_nightly_red_runs(
        now,
        Some(vec![
            run("completed", Some("failure"), 6, 300, now),
            run("completed", Some("failure"), 30, 299, now),
            run("completed", Some("success"), 54, 298, now),
        ]),
        None,
        |_, _| false,
    )
    .expect("latest scheduled failure should render");

    assert_eq!(red.run_id, Some(300));
    assert_eq!(red.nights, 2);
    assert!(red.summary.contains("cross-platform nightly red since"));
    // The since date renders in the viewer's local timezone, so derive the
    // expectation through the same conversion instead of hardcoding a day
    // that shifts with the test runner's TZ.
    let since_local = (now - chrono::Duration::hours(30))
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%d")
        .to_string();
    assert!(red.summary.contains(&since_local));
    assert!(red.summary.contains("run 300"));
    assert!(red.summary.contains("2 nights"));
}

#[test]
fn latest_scheduled_success_clears_nightly_red() {
    let now = chrono::Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap();
    assert!(summarize_nightly_red_runs(
        now,
        Some(vec![run("completed", Some("success"), 6, 301, now)]),
        None,
        |_, _| false,
    )
    .is_none());
}

#[test]
fn newer_successful_main_dispatch_clears_nightly_red_when_descendant() {
    let now = chrono::Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap();
    let latest_main = run_at_sha("completed", Some("success"), 1, 401, now, "main-green");

    assert!(summarize_nightly_red_runs(
        now,
        Some(vec![run_at_sha(
            "completed",
            Some("failure"),
            6,
            400,
            now,
            "scheduled-red"
        )]),
        Some(&latest_main),
        |ancestor, descendant| ancestor == "scheduled-red" && descendant == "main-green",
    )
    .is_none());
}

#[test]
fn non_main_success_does_not_clear_nightly_red() {
    let now = chrono::Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap();
    let red = summarize_nightly_red_runs(
        now,
        Some(vec![run_at_sha(
            "completed",
            Some("failure"),
            6,
            400,
            now,
            "scheduled-red",
        )]),
        None,
        |_, _| false,
    )
    .expect("branch-only success is not passed as a main run");

    assert_eq!(red.run_id, Some(400));
    assert_eq!(red.nights, 1);
}

#[test]
fn newer_failed_main_dispatch_does_not_clear_nightly_red() {
    let now = chrono::Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap();
    let latest_main = run_at_sha("completed", Some("failure"), 1, 401, now, "main-red");
    let red = summarize_nightly_red_runs(
        now,
        Some(vec![run_at_sha(
            "completed",
            Some("failure"),
            6,
            400,
            now,
            "scheduled-red",
        )]),
        Some(&latest_main),
        |ancestor, descendant| ancestor == "scheduled-red" && descendant == "main-red",
    )
    .expect("failed main dispatch should keep nightly red set");

    assert_eq!(red.run_id, Some(400));
}

// ADR-72: the cross-platform workflow is dispatch-only, so `aida awaiting` and
// `aida doctor` must not surface a nightly red, even when an old cache entry
// still holds one. Returns before any `gh` call, so this needs no network.
// trace:TASK-1723 | ai:claude
#[test]
fn task_1723_nightly_red_is_silenced_while_cross_platform_is_unscheduled() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!cross_platform_workflow_scheduled(dir.path()));
    assert!(nightly_red_status(dir.path()).is_none());

    let cache_dir = dir.path().join(".aida").join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let stale = NightlyRedCache {
        fetched_at: chrono::Utc::now(),
        item: Some(crate::awaiting_you::NightlyRedItem {
            summary: "cross-platform nightly red since 2026-09-23".into(),
            run_id: Some(1),
            nights: 3,
        }),
    };
    std::fs::write(
        cache_dir.join("awaiting-nightly-red.json"),
        serde_json::to_vec(&stale).unwrap(),
    )
    .unwrap();
    assert!(cached_nightly_red_status(dir.path()).is_none());
}

// trace:BUG-1943 | ai:claude
fn write_cross_platform_workflow(root: &std::path::Path, body: &str) {
    let workflows = root.join(".github").join("workflows");
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::write(workflows.join("cross-platform.yml"), body).unwrap();
}

// trace:BUG-1943 | ai:claude
fn seed_red_nightly_cache(root: &std::path::Path) {
    let cache_dir = root.join(".aida").join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let red = NightlyRedCache {
        fetched_at: chrono::Utc::now(),
        item: Some(crate::awaiting_you::NightlyRedItem {
            summary: "cross-platform nightly red since 2026-09-23".into(),
            run_id: Some(37323717377),
            nights: 13,
        }),
    };
    std::fs::write(
        cache_dir.join("awaiting-nightly-red.json"),
        serde_json::to_vec(&red).unwrap(),
    )
    .unwrap();
}

// BUG-1943: a dispatch-only cross-platform workflow has no nightly, so a red
// cached from its old scheduled runs must never reach awaiting or doctor.
// trace:BUG-1943 | ai:claude
#[test]
fn bug_1943_dispatch_only_workflow_reports_no_nightly_red() {
    let dir = tempfile::tempdir().unwrap();
    write_cross_platform_workflow(
        dir.path(),
        "name: Cross-platform (manual)\non:\n  workflow_dispatch:\njobs: {}\n",
    );
    seed_red_nightly_cache(dir.path());
    assert!(!cross_platform_workflow_scheduled(dir.path()));
    assert!(cached_nightly_red_status(dir.path()).is_none());
    // Returns before any `gh` call, so this stays offline.
    assert!(nightly_red_status(dir.path()).is_none());
}

// BUG-1943: a missing workflow file means no nightly either.
// trace:BUG-1943 | ai:claude
#[test]
fn bug_1943_missing_workflow_reports_no_nightly_red() {
    let dir = tempfile::tempdir().unwrap();
    seed_red_nightly_cache(dir.path());
    assert!(!cross_platform_workflow_scheduled(dir.path()));
    assert!(cached_nightly_red_status(dir.path()).is_none());
}

// BUG-1943: with a schedule trigger the existing behaviour holds — the fresh
// cached red is surfaced (the cache hit keeps this offline).
// trace:BUG-1943 | ai:claude
#[test]
fn bug_1943_scheduled_workflow_keeps_nightly_red() {
    let dir = tempfile::tempdir().unwrap();
    write_cross_platform_workflow(
        dir.path(),
        "name: Cross-platform\non:\n  schedule:\n    - cron: '0 6 * * *'\n  workflow_dispatch:\njobs: {}\n",
    );
    seed_red_nightly_cache(dir.path());
    assert!(cross_platform_workflow_scheduled(dir.path()));
    let red = cached_nightly_red_status(dir.path()).expect("scheduled workflow keeps red");
    assert_eq!(red.run_id, Some(37323717377));
    assert_eq!(red.nights, 13);
}

// trace:BUG-1943 | ai:claude
#[test]
fn bug_1943_schedule_detection_covers_every_on_shape() {
    assert!(workflow_yaml_declares_schedule("on: schedule\n"));
    assert!(workflow_yaml_declares_schedule("on: [push, schedule]\n"));
    assert!(workflow_yaml_declares_schedule(
        "on:\n  schedule:\n    - cron: '0 0 * * *'\n"
    ));
    assert!(!workflow_yaml_declares_schedule("on: workflow_dispatch\n"));
    assert!(!workflow_yaml_declares_schedule(
        "on: [push, pull_request]\n"
    ));
    assert!(!workflow_yaml_declares_schedule(
        "on:\n  workflow_dispatch:\n# schedule:\n#   - cron: '0 0 * * *'\n"
    ));
    assert!(!workflow_yaml_declares_schedule("jobs: {}\n"));
    assert!(!workflow_yaml_declares_schedule(": not yaml: ["));
}
