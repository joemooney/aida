// trace:STORY-3131 | ai:claude
use super::*;

fn ready(number: u64, age_min: i64, now: DateTime<Utc>) -> Ready {
    Ready {
        number,
        since: now - TimeDelta::minutes(age_min),
        ci: Ci::Green,
        full_tier_gap: None,
        supervised: false,
        on_train: false,
    }
}

#[test]
fn story_3131_defaults_when_section_absent() {
    let policy = Policy::read(None).unwrap();
    assert_eq!(
        policy,
        Policy {
            enabled: true,
            drought_minutes: 60,
            ready_dwell_minutes: 30,
            action: Action::Investigate,
        }
    );
    let other: toml::Value = toml::from_str("[landing_health]\nenabled = false").unwrap();
    assert_eq!(Policy::read(Some(&other)).unwrap(), policy);
    let bad: toml::Value = toml::from_str("[landing_watchdog]\naction = \"page\"").unwrap();
    assert!(Policy::read(Some(&bad)).is_err());
    let zero: toml::Value = toml::from_str("[landing_watchdog]\nready_dwell_minutes = 0").unwrap();
    assert!(Policy::read(Some(&zero)).is_err());
}

#[test]
fn story_3131_old_approval_reported_with_cause_fresh_one_not() {
    let now = Utc::now();
    let policy = Policy::read(None).unwrap();
    let mut supervised = ready(7, 45, now);
    supervised.supervised = true;
    let mut red = ready(8, 45, now);
    red.ci = Ci::Red("build".into());
    let mut full = ready(9, 45, now);
    full.ci = Ci::Pending;
    full.full_tier_gap = Some("cancelled");
    let mut trained = ready(10, 45, now);
    trained.on_train = true;
    let mut running = ready(11, 45, now);
    running.ci = Ci::Pending;
    let prs = [
        supervised,
        red,
        full,
        ready(12, 45, now),
        ready(13, 5, now),
        trained,
        running,
    ];
    let recent_landing = Some(now - TimeDelta::minutes(10));
    let findings = evaluate(&prs, &policy, now, recent_landing);
    let ids: Vec<_> = findings.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, ["PR-7", "PR-8", "PR-9", "PR-12"]);
    assert!(findings[0].summary.contains("supervised"));
    assert_eq!(
        findings[0].action,
        "needs operator train: `aida pr train 7`"
    );
    assert!(findings[0].summary.contains("ready 45m"));
    assert!(findings[1].summary.contains("red required check (build)"));
    assert!(findings[2].summary.contains("full CI tier cancelled"));
    assert!(findings[2].action.contains("ci:full"));
    assert!(findings[3].summary.contains("unknown"));
    assert!(findings[3].action.starts_with("investigate"));

    // Drought: main idle past drought_minutes while ready PRs wait.
    let dry = evaluate(&prs, &policy, now, Some(now - TimeDelta::minutes(90)));
    assert_eq!(dry.len(), 5);
    assert!(dry[4]
        .summary
        .contains("no landing on main for 90m while 4 ready PRs"));
    // No drought finding without a ready PR.
    let fresh = [ready(13, 5, now)];
    assert!(evaluate(&fresh, &policy, now, None).is_empty());
    let off = Policy {
        enabled: false,
        ..policy
    };
    assert!(evaluate(&prs, &off, now, None).is_empty());
}

#[test]
fn story_3131_report_vs_investigate() {
    let report: toml::Value = toml::from_str("[landing_watchdog]\naction = \"report\"").unwrap();
    let report = Policy::read(Some(&report)).unwrap();
    assert_eq!(report.action, Action::Report);
    assert!(report.fail_reason(3).is_none());
    let investigate = Policy::read(None).unwrap();
    assert!(investigate.fail_reason(0).is_none());
    assert!(investigate
        .fail_reason(3)
        .unwrap()
        .contains("3 landing-drought"));
}

#[test]
fn story_3131_full_tier_gap_from_rollup() {
    let row = |c: &str| serde_json::json!({"name": crate::full_tier::FULL_MARKER, "conclusion": c});
    assert_eq!(full_tier_gap(&serde_json::json!([])), Some("missing"));
    assert_eq!(
        full_tier_gap(&serde_json::json!([row("CANCELLED")])),
        Some("cancelled")
    );
    assert_eq!(
        full_tier_gap(&serde_json::json!([row("CANCELLED"), row("SUCCESS")])),
        None
    );
    assert_eq!(full_tier_gap(&serde_json::json!([row("")])), None);
}
