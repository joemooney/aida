// trace:STORY-3131 | ai:claude
use super::*;

fn ready(number: u64, age_min: i64, now: DateTime<Utc>) -> Ready {
    Ready {
        number,
        since: now - TimeDelta::minutes(age_min),
        approved: true,
        review: None,
        retained: None,
        conflict: false,
        repairing: false,
        spec: Some("STORY-1".into()),
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
    let findings = evaluate(&[old, fresh, unrouted], &policy, now, None, None, "main");
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

/// From a seat worktree, the train lock is read from the main worktree, so
/// a running train is seen (not misreported as an unknown cause).
// trace:STORY-3131 | ai:claude
#[test]
fn story_3131_s2_scan_reads_the_train_lock_from_the_main_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let (main, seat) = (tmp.path().join("main"), tmp.path().join("seat"));
    std::fs::create_dir_all(&main).unwrap();
    let git = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(&main)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["commit", "-q", "--allow-empty", "-m", "init"]);
    git(&["worktree", "add", "-q", "--detach", seat.to_str().unwrap()]);
    let dir = main.join(".aida/train");
    std::fs::create_dir_all(&dir).unwrap();
    let _held = crate::pr_train::hold_run_lock_for_test(&dir).unwrap();
    let lock = train_lock(&seat, Utc::now()).unwrap();
    assert_eq!(lock.and_then(|e| e.pid), Some(std::process::id()));
    let local = crate::pr_train::lock_driver(&seat, Utc::now()).unwrap();
    assert!(local.is_none(), "the seat's own .aida holds no train lock");
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
            lock_hold_minutes: 45,
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
    let findings = evaluate(&prs, &policy, now, recent_landing, None, "main");
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
    let dry = evaluate(
        &prs,
        &policy,
        now,
        Some(now - TimeDelta::minutes(90)),
        None,
        "main",
    );
    assert_eq!(dry.len(), 5);
    assert!(dry[4]
        .summary
        .contains("no landing on main for 90m while 2 landable PRs"));
    // Only unlandable (red) PRs stuck: no drought finding.
    let mut only_red = ready(8, 45, now);
    only_red.ci = Ci::Red("build".into());
    let red_only = evaluate(&[only_red], &policy, now, None, None, "main");
    assert_eq!(red_only.len(), 1);
    assert_eq!(red_only[0].id, "PR-8");
    // No drought finding without a ready PR.
    let fresh = [ready(13, 5, now)];
    assert!(evaluate(&fresh, &policy, now, None, None, "main").is_empty());
    let off = Policy {
        enabled: false,
        ..policy
    };
    assert!(evaluate(&prs, &off, now, None, None, "main").is_empty());
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
    let findings = evaluate(
        &[conflict, repairing, no_spec],
        &policy,
        now,
        None,
        None,
        "main",
    );
    let ids: Vec<_> = findings.iter().map(|f| f.id.as_str()).collect();
    // A repair lane owns PR-21: informational, and not landable either.
    assert_eq!(ids, ["PR-20", "PR-21", "PR-22"]);
    assert!(findings[0]
        .summary
        .contains("merge conflict with no repair lane"));
    assert_eq!(
        findings[0].action,
        "route the conflict repair: `aida rework STORY-1`"
    );
    assert_eq!(findings[1].category, INFO_CATEGORY);
    assert!(findings[1]
        .summary
        .contains("conflict repair in progress: the STORY-1 rework lane"));
    assert!(!findings[1].summary.contains("unknown"));
    assert_eq!(failing(&findings), 2, "only the unowned conflicts fail");
    assert!(findings[2].action.contains("rebase PR 22 onto its base"));
    assert!(!findings.iter().any(|f| f.id == "main"));
}

#[test]
fn story_3131_s2_held_review_at_head_fails() {
    use crate::review_watch::ReviewWatch;
    let now = Utc::now();
    let policy = Policy::read(None).unwrap();
    let mut unreviewed = ready(30, 45, now);
    unreviewed.approved = false;
    unreviewed.review = Some(ReviewWatch::Held);
    unreviewed.full_tier_gap = Some("missing"); // not reported before review
    let findings = evaluate(&[unreviewed], &policy, now, None, None, "main");
    assert_eq!(
        findings.len(),
        1,
        "an unreviewed PR is not landable: no drought"
    );
    assert_eq!(findings[0].category, CATEGORY);
    assert!(findings[0].summary.starts_with("PR-30 green 45m"));
    assert!(findings[0]
        .summary
        .contains("review held at the current head"));
    assert!(findings[0]
        .action
        .contains("aida review watch --release PR-30"));
}

fn holder(held: i64, idle: Option<i64>, ci_live: Option<bool>) -> LockHolder {
    LockHolder {
        pid: Some(4242),
        held_minutes: Some(held),
        command: Some("aida pr train 4000".into()),
        idle_minutes: idle,
        ci_live,
        threshold_minutes: 45,
    }
}

#[test]
fn story_3131_s2_train_lock_advice_follows_progress() {
    let now = Utc::now();
    let policy = Policy::read(None).unwrap();
    let mut red = ready(41, 45, now);
    red.ci = Ci::Red("build".into());
    let prs = [ready(40, 45, now), red];
    let run = |h: &LockHolder| evaluate(&prs, &policy, now, None, Some(h), "main");

    // Stuck: past the threshold, no step change and no CI job.
    let stuck = run(&holder(90, Some(60), Some(false)));
    assert_eq!(stuck[0].category, CATEGORY);
    assert!(stuck[0]
        .summary
        .contains("train lock held 90m by pid 4242 (aida pr train 4000)"));
    assert!(stuck[0].action.contains("stop pid 4242: it is stuck"));
    assert!(
        stuck[1].summary.contains("red required check"),
        "red outranks the lock"
    );

    // A long train with live CI, or a recent step change: busy, never "stop".
    for busy in [
        holder(120, Some(60), Some(true)),
        holder(120, Some(5), None),
        holder(30, None, None),
        holder(30, Some(40), Some(false)),
    ] {
        assert_eq!(busy.state(), LockState::Busy, "{busy:?}");
        let rows = run(&busy);
        assert_eq!(rows[0].category, INFO_CATEGORY, "{busy:?}");
        assert!(rows[0].summary.contains("waiting for the running train"));
        assert!(!rows.iter().any(|f| f.action.contains("stop")));
    }
    // Past the threshold but CI unread or no journal: investigate, not stop.
    for slow in [holder(90, Some(60), None), holder(90, None, Some(false))] {
        assert_eq!(slow.state(), LockState::Slow, "{slow:?}");
        let rows = run(&slow);
        assert_eq!(rows[0].category, CATEGORY);
        assert!(rows[0].summary.contains("past the 45m threshold"));
        assert!(rows[0].action.starts_with("investigate"));
        assert!(!rows[0].action.contains("stop"));
    }
    // A busy train turns main's idleness informational too.
    let dry = Some(now - TimeDelta::minutes(90));
    let busy = evaluate(
        &prs,
        &policy,
        now,
        dry,
        Some(&holder(70, Some(2), None)),
        "main",
    );
    let main = busy.iter().find(|f| f.id == "main").unwrap();
    assert_eq!(main.category, INFO_CATEGORY);
    assert_eq!(failing(&busy), 1, "only the red PR fails");

    // A dead or unidentified driver with no progress is orphaned at any age.
    let orphan = LockHolder {
        pid: None,
        command: None,
        ..holder(3, None, None)
    };
    assert_eq!(orphan.state(), LockState::Orphaned);
    let dead = run(&orphan);
    assert!(dead[0]
        .summary
        .contains("train lock held 3m by an unidentified process"));
    assert!(dead[0].action.starts_with("investigate"));
    assert_eq!(failing(&dead), dead.len(), "an orphaned lock fails the job");
    // The knob is read and validated like the other minutes.
    let cfg: toml::Value = toml::from_str("[landing_watchdog]\nlock_hold_minutes = 5").unwrap();
    assert_eq!(Policy::read(Some(&cfg)).unwrap().lock_hold_minutes, 5);
    let zero: toml::Value = toml::from_str("[landing_watchdog]\nlock_hold_minutes = 0").unwrap();
    assert!(Policy::read(Some(&zero)).is_err());
}

/// No automatic train picks up a supervised PR: a held lock never hides
/// that it needs an operator train.
#[test]
fn story_3131_s2_supervised_outranks_a_busy_train_lock() {
    let now = Utc::now();
    let policy = Policy::read(None).unwrap();
    let mut supervised = ready(50, 45, now);
    supervised.supervised = true;
    let prs = [supervised, ready(51, 45, now)];
    let busy = holder(10, Some(2), None);
    assert_eq!(busy.state(), LockState::Busy);
    let rows = evaluate(&prs, &policy, now, Some(now), Some(&busy), "main");
    assert_eq!(rows[0].id, "PR-50");
    assert_eq!(rows[0].category, CATEGORY);
    assert!(rows[0].summary.contains("supervised, not drain-eligible"));
    assert_eq!(rows[0].action, "needs operator train: `aida pr train 50`");
    assert!(rows[1].summary.contains("waiting for the running train"));
    assert_eq!(rows[1].category, INFO_CATEGORY);
}

#[test]
fn story_3131_s2_lock_threshold_from_history() {
    let policy = Policy::read(None).unwrap();
    assert_eq!(lock_threshold(&policy, &[]), 45, "no history: the floor");
    // Short runs never lower the floor.
    assert_eq!(lock_threshold(&policy, &[600, 900]), 45);
    // p90 of ten runs (5m..50m) is the 9th smallest: 45m, doubled.
    let runs: Vec<u64> = (1..=10).map(|m| m * 5 * 60).collect();
    assert_eq!(lock_threshold(&policy, &runs), 90);
    assert_eq!(lock_threshold(&policy, &[50 * 60]), 100);
}

#[test]
fn story_3131_s2_deep_review_backlog_is_informational() {
    use crate::review_watch::ReviewWatch;
    let now = Utc::now();
    let policy = Policy::read(None).unwrap();
    // Far more green, unreviewed PRs than watcher capacity: some in flight
    // (reserved, running, remote-held, superseding), most not admitted yet.
    let prs: Vec<Ready> = (60..80)
        .map(|n| {
            let mut r = ready(n, 120, now);
            r.approved = false;
            r.review = (n < 63).then_some(ReviewWatch::InFlight);
            r
        })
        .collect();
    let findings = evaluate(&prs, &policy, now, None, None, "main");
    assert_eq!(findings.len(), 20);
    assert_eq!(failing(&findings), 0, "a review backlog is not a drought");
    assert!(policy.fail_reason(failing(&findings)).is_none());
    assert!(findings[0].summary.contains("review in flight"));
    assert!(findings[5].summary.contains("waiting for watcher capacity"));
}

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn listing(number: u64) -> serde_json::Value {
    serde_json::json!({
        "number": number,
        "title": format!("feat: thing (STORY-{number})"),
        "headRefName": format!("story-{number}"),
        "headRefOid": HEAD.to_uppercase(),
        "baseRefName": "trunk",
        "isDraft": false,
        "mergeable": "MERGEABLE",
        "statusCheckRollup": [{
            "name": "build", "status": "COMPLETED", "conclusion": "SUCCESS",
            "completedAt": "2026-11-26T10:00:00Z"
        }],
    })
}

#[test]
fn story_3131_s2_ready_of_maps_the_listing_row() {
    use crate::review_watch::ReviewWatch;
    let watch = vec![
        ("PR-1".to_string(), HEAD.to_string(), ReviewWatch::Held),
        ("PR-10".to_string(), HEAD.to_string(), ReviewWatch::InFlight),
        ("PR-2".to_string(), "b".repeat(40), ReviewWatch::Held),
    ];
    let retained = vec![("PR-10".to_string(), "kept; fix: drop it".to_string())];
    let verdict = |number: u64, head: &str| -> Result<AtHead> {
        assert_eq!(head, HEAD.to_uppercase());
        Ok(match number {
            3 => AtHead::Approved(Utc::now()),
            4 => AtHead::Settled,
            _ => AtHead::Missing,
        })
    };
    let lookups = Lookups {
        base: "trunk",
        tiered: false,
        watch: &watch,
        retained: &retained,
        verdict: &verdict,
        repairing: &|spec, _| spec == "STORY-3",
        supervised: &|specs| specs.is_empty(),
        train: &|n, _| Ok((n == 3, (n == 3).then(|| (Utc::now(), "route".into())))),
    };
    let of = |row: &serde_json::Value| ready_of(row, &lookups).unwrap();

    // Unreviewed and green: included, with its review state matched by key
    // and head (case-insensitively), never by a prefix key or another head.
    let one = of(&listing(1)).unwrap();
    assert!(!one.approved);
    assert_eq!(one.review, Some(ReviewWatch::Held));
    assert_eq!(of(&listing(2)).unwrap().review, None, "other head");
    assert_eq!(
        of(&listing(10)).unwrap().review,
        Some(ReviewWatch::InFlight)
    );
    assert!(!one.conflict && !one.repairing && !one.supervised && !one.on_train);
    assert_eq!(one.retained, None);
    assert_eq!(
        of(&listing(10)).unwrap().retained.as_deref(),
        Some("kept; fix: drop it"),
        "the watcher's retained block, matched by exact key"
    );
    assert_eq!(one.spec.as_deref(), Some("STORY-1"));

    let mut conflicted = listing(3);
    conflicted["mergeable"] = "CONFLICTING".into();
    let three = of(&conflicted).unwrap();
    assert!(three.approved && three.conflict && three.repairing && three.on_train);
    assert_eq!(three.drift.map(|(_, r)| r).as_deref(), Some("route"));
    assert_eq!(one.drift, None);

    // Settled verdicts, drafts, other bases and red unreviewed heads drop out.
    assert!(of(&listing(4)).is_none());
    let mut draft = listing(1);
    draft["isDraft"] = true.into();
    assert!(of(&draft).is_none());
    let mut main = listing(1);
    main["baseRefName"] = "main".into();
    assert!(of(&main).is_none(), "the resolved base, not a literal main");
    let mut red = listing(1);
    red["statusCheckRollup"][0]["conclusion"] = "FAILURE".into();
    assert!(of(&red).is_none());
}

/// BUG-2776's retained-checkout block is the held review's cause: one
/// finding per PR, carrying the watcher's own fix. trace:STORY-3131 | ai:claude
#[test]
fn story_3131_s2_retained_block_is_the_held_reviews_cause() {
    use crate::review_watch::ReviewWatch;
    let now = Utc::now();
    let policy = Policy::read(None).unwrap();
    let mut held = ready(7, 90, now);
    held.approved = false;
    held.review = Some(ReviewWatch::Held);
    held.retained = Some("review blocked by a retained reviewer worktree; fix: end it".into());
    let findings = evaluate(&[held], &policy, now, Some(now), None, "main");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].category, CATEGORY);
    assert!(
        findings[0]
            .summary
            .ends_with("review blocked by a retained reviewer worktree"),
        "{}",
        findings[0].summary
    );
    assert_eq!(findings[0].action, "end it");

    // A retained PR still waiting for capacity is not backlog: it is held.
    let mut queued = ready(8, 90, now);
    queued.approved = false;
    queued.retained = Some("kept; fix: end it".into());
    let findings = evaluate(&[queued], &policy, now, Some(now), None, "main");
    assert_eq!(findings[0].category, CATEGORY);
}
