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
        drift: None,
    }
}

// trace:BUG-2777 | ai:claude
#[test]
fn bug2777_stacked_diff_drift_over_30m_is_its_own_cause() {
    let now = Utc::now();
    let policy = Policy::read(None).unwrap();
    let route = "the branch does not update cleanly (conflict); conflict repair".to_string();
    let mut old = ready(4464, 120, now);
    old.drift = Some((now - TimeDelta::minutes(45), route.clone()));
    let mut fresh = ready(4465, 120, now);
    fresh.drift = Some((now - TimeDelta::minutes(10), route.clone()));
    let mut unrouted = ready(4466, 5, now);
    unrouted.drift = Some((now - TimeDelta::minutes(31), String::new()));
    let findings = evaluate(&[old, fresh, unrouted], &policy, now, None);
    let ids: Vec<_> = findings.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, ["PR-4464", "PR-4465", "PR-4466"]);
    assert!(findings[0]
        .summary
        .contains("ejected for stacked-diff drift 45m ago"));
    assert_eq!(findings[0].action, route);
    assert!(
        findings[1].summary.contains("unknown"),
        "under 30m: the ordinary cause"
    );
    assert!(
        findings[2].summary.contains("stacked-diff drift 31m"),
        "reported past 30m even inside the ready dwell"
    );
    assert!(findings[2].action.contains("update PR 4466's branch"));
    assert!(
        !findings.iter().any(|f| f.id == "main"),
        "a drifted head is not landable"
    );
}

/// The scan reads the drift row from the main worktree, where the train
/// writes it, even when `aida doctor` runs in a seat worktree.
// trace:BUG-2777 | ai:claude
#[test]
fn bug2777_scan_reads_train_state_from_the_main_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path().join("main");
    let seat = tmp.path().join("seat");
    std::fs::create_dir_all(&main).unwrap();
    let git = |dir: &std::path::Path, args: &[&str]| {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&main, &["init", "-q", "-b", "main"]);
    git(&main, &["commit", "-q", "--allow-empty", "-m", "init"]);
    git(
        &main,
        &["worktree", "add", "-q", "--detach", seat.to_str().unwrap()],
    );
    crate::pr_train::culprit::record_drift(&main, 4464, "aaaa1111", None, "conflict repair")
        .unwrap();
    let resolved = crate::main_worktree_root_from(&seat);
    let (on_train, drift) = train_state(&resolved, 4464, "aaaa1111").unwrap();
    assert!(!on_train);
    assert_eq!(drift.map(|(_, r)| r).as_deref(), Some("conflict repair"));
    let (_, local) = train_state(&seat, 4464, "aaaa1111").unwrap();
    assert_eq!(local, None, "the seat's own .aida has no train state");
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
        .contains("no landing on main for 90m while 2 landable PRs"));
    // Only unlandable (red) PRs stuck: no drought finding.
    let mut only_red = ready(8, 45, now);
    only_red.ci = Ci::Red("build".into());
    let red_only = evaluate(&[only_red], &policy, now, None);
    assert_eq!(red_only.len(), 1);
    assert_eq!(red_only[0].id, "PR-8");
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
    let got = approved_at_head(
        vec![
            rec("approved", HEAD, t2),
            rec("changes_requested", HEAD, t1),
        ],
        HEAD,
    )
    .unwrap();
    assert_eq!(got, Some(DateTime::parse_from_rfc3339(t2).unwrap().into()));
    // A later refusal supersedes an earlier approval.
    let refused = vec![
        rec("approved", HEAD, t1),
        rec("changes_requested", HEAD, t2),
    ];
    assert_eq!(approved_at_head(refused, HEAD).unwrap(), None);
    // An approval of an older head does not count.
    assert_eq!(
        approved_at_head(vec![rec("approved", OLD, t1)], HEAD).unwrap(),
        None
    );
    // A closed (merged) verdict is skipped.
    let (at, mut closed) = rec("approved", HEAD, t1);
    closed.insert("closed_by_merge".into(), "abc123".into());
    assert_eq!(approved_at_head(vec![(at, closed)], HEAD).unwrap(), None);
    assert_eq!(approved_at_head(Vec::new(), HEAD).unwrap(), None);
}
