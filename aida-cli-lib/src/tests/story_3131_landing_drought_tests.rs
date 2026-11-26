// trace:STORY-3131 | ai:claude
use super::*;

fn ready(number: u64, age_min: i64, now: DateTime<Utc>) -> Ready {
    Ready {
        number,
        since: now - TimeDelta::minutes(age_min),
        approved: true,
        review_in_flight: false,
        conflict: false,
        repairing: false,
        spec: Some("STORY-1".into()),
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
            lock_hold_minutes: 20,
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
    // Fast CI still running with no full tier yet: not ready, not "missing".
    let mut running_no_full = ready(14, 45, now);
    running_no_full.ci = Ci::Pending;
    running_no_full.full_tier_gap = Some("missing");
    let prs = [
        supervised,
        red,
        full,
        ready(12, 45, now),
        ready(13, 5, now),
        trained,
        running,
        running_no_full,
    ];
    let recent_landing = Some(now - TimeDelta::minutes(10));
    let findings = evaluate(&prs, &policy, now, recent_landing, None);
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
    let dry = evaluate(&prs, &policy, now, Some(now - TimeDelta::minutes(90)), None);
    assert_eq!(dry.len(), 5);
    assert!(dry[4]
        .summary
        .contains("no landing on main for 90m while 2 landable PRs"));
    // Only unlandable (red) PRs stuck: no drought finding.
    let mut only_red = ready(8, 45, now);
    only_red.ci = Ci::Red("build".into());
    let red_only = evaluate(&[only_red], &policy, now, None, None);
    assert_eq!(red_only.len(), 1);
    assert_eq!(red_only[0].id, "PR-8");
    // No drought finding without a ready PR.
    let fresh = [ready(13, 5, now)];
    assert!(evaluate(&fresh, &policy, now, None, None).is_empty());
    let off = Policy {
        enabled: false,
        ..policy
    };
    assert!(evaluate(&prs, &off, now, None, None).is_empty());
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

#[test]
fn story_3131_unset_or_unresolved_spec_is_supervised() {
    use aida_core::ExecutionMode;
    let drain = Some(Some(ExecutionMode::Drain));
    assert!(!needs_supervision([drain].into_iter()));
    // Unset mode is supervised, as the ship gate treats it.
    assert!(needs_supervision([Some(None)].into_iter()));
    assert!(needs_supervision([drain, Some(None)].into_iter()));
    // An unresolved spec ID, or no spec ID at all, fails closed.
    assert!(needs_supervision([drain, None].into_iter()));
    assert!(needs_supervision(std::iter::empty()));
}

#[test]
fn story_3131_latest_ledger_record_wins_and_must_approve_head() {
    use crate::gate::review_verdict::JsonObj;
    const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OLD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let rec = |word: &str, sha: &str, at: &str| {
        let mut r = JsonObj::new();
        r.insert("verdict".into(), word.into());
        r.insert("reviewed_sha".into(), sha.into());
        r.insert("recorded_at".into(), at.into());
        r.insert("recorded_by".into(), "reviewer seat".into());
        (at.to_string(), r)
    };
    let t1 = "2026-11-02T10:00:00+00:00";
    let t2 = "2026-11-02T11:00:00+00:00";
    // Latest approval at head wins over an older refusal, in any order.
    let got = ledger_at_head(
        vec![
            rec("approved", HEAD, t2),
            rec("changes_requested", HEAD, t1),
        ],
        HEAD,
    )
    .unwrap();
    assert_eq!(
        got,
        AtHead::Approved(DateTime::parse_from_rfc3339(t2).unwrap().into())
    );
    // A later refusal supersedes an earlier approval.
    let refused = vec![
        rec("approved", HEAD, t1),
        rec("changes_requested", HEAD, t2),
    ];
    assert_eq!(ledger_at_head(refused, HEAD).unwrap(), AtHead::Settled);
    // An approval of an older head does not count.
    assert_eq!(
        ledger_at_head(vec![rec("approved", OLD, t1)], HEAD).unwrap(),
        AtHead::Missing
    );
    // A closed (merged) verdict is skipped.
    let (at, mut closed) = rec("approved", HEAD, t1);
    closed.insert("closed_by_merge".into(), "abc123".into());
    assert_eq!(
        ledger_at_head(vec![(at, closed)], HEAD).unwrap(),
        AtHead::Settled
    );
    assert_eq!(ledger_at_head(Vec::new(), HEAD).unwrap(), AtHead::Missing);
}

#[test]
fn story_3131_s2_conflict_without_repair_lane() {
    let now = Utc::now();
    let policy = Policy::read(None).unwrap();
    let mut conflict = ready(20, 45, now);
    conflict.conflict = true;
    conflict.ci = Ci::Pending; // a conflict holds CI; still reported
    let mut repairing = ready(21, 45, now);
    repairing.conflict = true;
    repairing.repairing = true;
    let mut no_spec = conflict.clone();
    no_spec.number = 22;
    no_spec.spec = None;
    let findings = evaluate(&[conflict, repairing, no_spec], &policy, now, None, None);
    let ids: Vec<_> = findings.iter().map(|f| f.id.as_str()).collect();
    // A repair lane owns PR-21: not a conflict cause, and not landable either.
    assert_eq!(ids, ["PR-20", "PR-21", "PR-22"]);
    assert!(findings[0]
        .summary
        .contains("merge conflict with no repair lane"));
    assert_eq!(
        findings[0].action,
        "route the conflict repair: `aida rework STORY-1`"
    );
    assert!(!findings[1].summary.contains("conflict"));
    assert!(findings[2].action.contains("rebase PR 22 onto main"));
    assert!(!findings.iter().any(|f| f.id == "main"));
}

#[test]
fn story_3131_s2_review_verdict_missing_at_head() {
    let now = Utc::now();
    let policy = Policy::read(None).unwrap();
    let mut unreviewed = ready(30, 45, now);
    unreviewed.approved = false;
    unreviewed.full_tier_gap = Some("missing"); // not reported before review
    let findings = evaluate(&[unreviewed], &policy, now, None, None);
    assert_eq!(
        findings.len(),
        1,
        "an unreviewed PR is not landable: no drought"
    );
    assert!(findings[0].summary.starts_with("PR-30 green 45m"));
    assert!(findings[0]
        .summary
        .contains("review verdict missing or held"));
    assert!(findings[0]
        .action
        .contains("aida review watch --release PR-30"));
}

#[test]
fn story_3131_s2_train_lock_held_past_threshold() {
    let now = Utc::now();
    let policy = Policy::read(None).unwrap();
    let mut supervised = ready(40, 45, now);
    supervised.supervised = true;
    let mut red = ready(41, 45, now);
    red.ci = Ci::Red("build".into());
    let prs = [supervised, red];
    let holder = |m: i64| LockHolder {
        pid: Some(4242),
        held_minutes: Some(m),
        command: Some("aida pr train 4000".into()),
    };
    let held = evaluate(&prs, &policy, now, None, Some(&holder(25)));
    assert!(held[0]
        .summary
        .contains("train lock held 25m by pid 4242 (aida pr train 4000)"));
    assert!(held[0].action.contains("stop pid 4242 if it is hung"));
    assert!(
        held[1].summary.contains("red required check"),
        "red outranks the lock"
    );
    // Within lock_hold_minutes, or of unknown age: the first-pass cause stands.
    let fresh = evaluate(&prs, &policy, now, None, Some(&holder(20)));
    assert!(fresh[0].summary.contains("supervised"));
    let unknown_age = LockHolder {
        held_minutes: None,
        ..holder(0)
    };
    let unknown = evaluate(&prs, &policy, now, None, Some(&unknown_age));
    assert!(unknown[0].summary.contains("supervised"));
    // A dead or unidentified driver still holding the lock is orphaned:
    // stale at any age, aged from the record when it has one.
    let orphan = |m: Option<i64>| LockHolder {
        pid: None,
        held_minutes: m,
        command: None,
    };
    let dead = evaluate(&prs, &policy, now, None, Some(&orphan(Some(3))));
    assert!(dead[0]
        .summary
        .contains("train lock held 3m by an unidentified process"));
    assert!(dead[0].action.contains("stop an unidentified process"));
    let ageless = evaluate(&prs, &policy, now, None, Some(&orphan(None)));
    assert!(ageless[0]
        .summary
        .contains("train lock held by an unidentified process"));
    assert_eq!(failing(&dead), dead.len(), "an orphaned lock fails the job");
    // The knob is read and validated like the other minutes.
    let cfg: toml::Value = toml::from_str("[landing_watchdog]\nlock_hold_minutes = 5").unwrap();
    assert_eq!(Policy::read(Some(&cfg)).unwrap().lock_hold_minutes, 5);
    let zero: toml::Value = toml::from_str("[landing_watchdog]\nlock_hold_minutes = 0").unwrap();
    assert!(Policy::read(Some(&zero)).is_err());
}

#[test]
fn story_3131_s2_review_backlog_is_informational() {
    let now = Utc::now();
    let policy = Policy::read(None).unwrap();
    let mut queued = ready(50, 45, now);
    queued.approved = false;
    queued.review_in_flight = true;
    let mut idle = queued.clone();
    idle.number = 51;
    idle.review_in_flight = false;
    let mut fresh = idle.clone();
    fresh.number = 52;
    fresh.since = now - TimeDelta::minutes(10);
    let findings = evaluate(&[queued, idle, fresh], &policy, now, None, None);
    let rows: Vec<_> = findings
        .iter()
        .map(|f| (f.id.as_str(), f.category.as_str()))
        .collect();
    // Within the dwell nothing shows; a queued or running review is shown
    // but does not fail; no review at all past the dwell is a finding.
    assert_eq!(rows, [("PR-50", INFO_CATEGORY), ("PR-51", CATEGORY)]);
    assert!(findings[0].summary.contains("review queued or running"));
    assert!(findings[1]
        .summary
        .contains("review verdict missing or held"));
    assert_eq!(failing(&findings), 1);
    assert!(policy.fail_reason(failing(&findings[..1])).is_none());
}
