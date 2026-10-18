//! Tests for STORY-1688: the `lane-liveness` substrate job over fixtures and
//! replayed 2026-10-13 incidents.
// trace:STORY-1688 | ai:antigravity

use super::*;

fn ts(rfc3339: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(rfc3339)
        .unwrap()
        .with_timezone(&Utc)
}

#[test]
fn clean_snapshot_is_silent() {
    let now = ts("2026-10-13T18:00:00Z");
    let snap = LaneLivenessSnapshot {
        lanes: vec![RegisteredLane {
            id: "codex-factory".into(),
            kind: LaneKind::Factory,
            pid: Some(4242),
            process_alive: true,
            log_path: None,
            log_updated_at: Some(now - Duration::minutes(5)),
            assigned_work: vec!["TASK-100".into()],
            owning_seat: "advisor".into(),
            max_log_stale_mins: None,
            stopped_reason: None,
        }],
        open_prs: vec![OpenPrFact {
            number: 2600,
            title: "[AI:codex] fix: example (TASK-100)".into(),
            head_sha: "abcdef123456".into(),
            is_factory: true,
            factory_ready: Some(("abcdef123456".into(), now - Duration::minutes(10))),
            review_started: false,
            awaiting_advisor_signoff: None,
            needs_operator_hold_clear: None,
            review_seat: "merge".into(),
        }],
        leases: vec![OrphanedLeaseFact {
            lease_id: "lease-1".into(),
            spec_id: "TASK-100".into(),
            owner: "codex".into(),
            pid: Some(4242),
            process_alive: true,
            last_progress_at: now - Duration::minutes(45),
            owning_seat: "advisor".into(),
        }],
    };
    let findings = evaluate_snapshot(&snap, &LaneLivenessPolicy::default(), now);
    assert!(findings.is_empty(), "expected clean snapshot: {findings:?}");
}

#[test]
fn dead_lane_and_stale_log_fixtures_produce_findings_only_with_assigned_work() {
    let now = ts("2026-10-13T18:00:00Z");
    let tmp = tempfile::tempdir().unwrap();
    let lanes_dir = tmp.path().join(".aida").join("lanes");
    std::fs::create_dir_all(&lanes_dir).unwrap();

    // 1. Dead headless-exec lane with assigned work -> reported
    std::fs::write(
        lanes_dir.join("headless-dead.toml"),
        r#"
id = "codex-headless-dead"
kind = "headless-exec"
pid = 99999
alive = false
assigned_work = ["BUG-2014", "TASK-1742"]
owning_seat = "advisor"
"#,
    )
    .unwrap();

    // 2. Alive factory lane whose log has not moved for 45m (> 30m default) -> reported
    std::fs::write(
        lanes_dir.join("factory-stale.toml"),
        r#"
id = "codex-factory-stale"
kind = "factory"
pid = 12345
alive = true
log_updated_at = "2026-10-13T17:15:00Z"
assigned_work = ["BUG-2002"]
owning_seat = "advisor"
"#,
    )
    .unwrap();

    // 3. Dead lane with NO assigned work -> silent (idle/retired lane)
    std::fs::write(
        lanes_dir.join("idle-dead.toml"),
        r#"
id = "idle-lane"
kind = "relay"
pid = 88888
alive = false
assigned_work = []
owning_seat = "advisor"
"#,
    )
    .unwrap();

    let findings = scan_at(tmp.path(), now);
    let ids: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "lane-liveness:dead-lane:codex-headless-dead",
            "lane-liveness:stale-log:codex-factory-stale",
        ]
    );
    assert_eq!(findings[0].kind, FindingKind::DeadLane);
    assert_eq!(findings[0].owning_seat, "advisor");
    assert_eq!(findings[1].kind, FindingKind::StaleLog);
    assert!(findings[1].summary.contains("45m (threshold 30m)"));
}

#[test]
fn unpicked_ready_pr_and_stale_gate_wait_fixtures_are_detected_and_routed() {
    let now = ts("2026-10-13T18:00:00Z");
    let tmp = tempfile::tempdir().unwrap();
    let ll_dir = tmp.path().join(".aida").join("lane-liveness");
    std::fs::create_dir_all(&ll_dir).unwrap();

    // Configure an operator notification command on the trusted default branch
    let notify_out = tmp.path().join("notify-fired.txt");
    std::fs::write(
        tmp.path().join(".aida").join("config.toml"),
        format!("[notify]\ncommand = \"cat > '{}'\"\n", notify_out.display()),
    )
    .unwrap();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.email", "t@t.t"],
        &["config", "user.name", "t"],
        &["add", ".aida/config.toml"],
        &["commit", "-q", "-m", "trusted notify config"],
    ] {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(args)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?} failed");
    }
    let sha = String::from_utf8(
        std::process::Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(std::process::Command::new("git")
        .arg("-C")
        .arg(tmp.path())
        .args(["update-ref", "refs/remotes/origin/main", sha.trim()])
        .output()
        .unwrap()
        .status
        .success());

    // Write PR fixtures in `gh pr list --json` compatible format
    let prs_json = serde_json::json!([
        {
            "number": 2601,
            "title": "[AI:codex] feat: unpicked ready pr (STORY-1661)",
            "headRefOid": "11111111aaaa",
            "labels": [{"name": "factory"}],
            "comments": [
                {
                    "body": "FACTORY READY: PR #2601 green at 11111111aaaa",
                    "createdAt": "2026-10-13T17:20:00Z"
                }
            ]
        },
        {
            "number": 2602,
            "title": "[AI:codex] feat: already under review (STORY-1662)",
            "headRefOid": "22222222bbbb",
            "labels": [{"name": "factory"}],
            "comments": [
                {
                    "body": "FACTORY READY: PR #2602 green at 22222222bbbb",
                    "createdAt": "2026-10-13T17:10:00Z"
                },
                {
                    "body": "REVIEW (merge): REQUEST CHANGES at 22222222bbbb",
                    "createdAt": "2026-10-13T17:25:00Z"
                }
            ]
        },
        {
            "number": 2587,
            "title": "[AI:codex] fix(authority): awaiting advisor signoff (BUG-1983)",
            "headRefOid": "33333333cccc",
            "labels": [],
            "comments": [
                {
                    "body": "MERGE: approved at 33333333cccc, awaiting advisor signoff",
                    "createdAt": "2026-10-13T17:40:00Z"
                }
            ]
        },
        {
            "number": 2592,
            "title": "[AI:codex] fix: needs operator hold clear (BUG-2005)",
            "headRefOid": "44444444dddd",
            "labels": [],
            "comments": [
                {
                    "body": "MERGE: ready, needs operator hold clear: aida merge-hold clear 2592",
                    "createdAt": "2026-10-13T17:35:00Z"
                }
            ]
        }
    ]);
    std::fs::write(ll_dir.join("prs.json"), prs_json.to_string()).unwrap();

    let findings = scan_and_route_at(tmp.path(), now, true);
    let ids: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "lane-liveness:gate-wait-advisor:PR-2587",
            "lane-liveness:gate-wait-operator:PR-2592",
            "lane-liveness:unpicked-ready-pr:PR-2601",
        ]
    );

    // Verify each finding routed to its owning seat as a due seat job in the store ledger
    // AND surfaces in `due_seat_jobs(root, Some(seat))` (`aida schedule due --seat <seat>`).
    let store = tmp.path().join(".aida-store");
    let merge_ledger = schedule_ledger::load(&store, "lane-liveness-merge")
        .expect("merge seat job must be routed");
    assert_eq!(merge_ledger.due_since, Some(now));
    assert!(merge_ledger
        .note
        .as_deref()
        .unwrap_or("")
        .contains("PR #2601"));
    assert!(merge_ledger.due_failure.is_some());
    let merge_due = crate::maintenance_schedule::due_seat_jobs(tmp.path(), Some("merge"));
    assert!(
        merge_due.iter().any(|j| j.name == "lane-liveness-merge"),
        "merge seat must see lane-liveness-merge in due_seat_jobs: {merge_due:?}"
    );

    let advisor_ledger = schedule_ledger::load(&store, "lane-liveness-advisor")
        .expect("advisor seat job must be routed");
    assert_eq!(advisor_ledger.due_since, Some(now));
    assert!(advisor_ledger
        .note
        .as_deref()
        .unwrap_or("")
        .contains("PR #2587"));
    let advisor_due = crate::maintenance_schedule::due_seat_jobs(tmp.path(), Some("advisor"));
    assert!(
        advisor_due
            .iter()
            .any(|j| j.name == "lane-liveness-advisor"),
        "advisor seat must see lane-liveness-advisor in due_seat_jobs: {advisor_due:?}"
    );

    let operator_due = crate::maintenance_schedule::due_seat_jobs(tmp.path(), Some("operator"));
    assert!(
        operator_due
            .iter()
            .any(|j| j.name == "lane-liveness-operator"),
        "operator seat must see lane-liveness-operator in due_seat_jobs: {operator_due:?}"
    );

    // `lane-liveness-route` is owned by the scheduler's `on = ["CronJobFailed:lane-liveness"]`
    // fan-out, never written directly by `route_findings`.
    assert!(
        schedule_ledger::load(&store, "lane-liveness-route").is_none(),
        "route_findings must not write lane-liveness-route directly"
    );

    // Verify the human-only gate wait (`needs operator hold clear`) triggered an operator push notification
    let pushed = std::fs::read_to_string(&notify_out)
        .expect("human-only gate wait must push an operator notification");
    assert!(pushed.contains("PR #2592"), "{pushed}");
    assert!(pushed.contains("aida merge-hold clear 2592"), "{pushed}");
    assert!(pushed.contains("(threshold 15m)"), "{pushed}");
}

#[test]
fn replay_2026_10_13_four_incidents_each_produces_a_finding_within_one_tick() {
    let tmp = tempfile::tempdir().unwrap();
    let aida_dir = tmp.path().join(".aida");
    let lanes_dir = aida_dir.join("lanes");
    let ll_dir = aida_dir.join("lane-liveness");
    let sessions_dir = aida_dir.join("sessions");
    std::fs::create_dir_all(&lanes_dir).unwrap();
    std::fs::create_dir_all(&ll_dir).unwrap();
    std::fs::create_dir_all(&sessions_dir).unwrap();

    // Incident 1 (2026-10-13 09:50): The role leak (BUG-2007) stopped Codex at 09:50
    // while assigned BUG-2002, TASK-1724, BUG-1994, STORY-1664.
    // Within one 10m tick after the 30m log-staleness window (10:21):
    std::fs::write(
        lanes_dir.join("incident1-codex-role-leak.toml"),
        r#"
id = "codex-factory-20261013"
kind = "factory"
pid = 2135413
alive = true
log_updated_at = "2026-10-13T09:50:00Z"
assigned_work = ["BUG-2002", "TASK-1724", "BUG-1994", "STORY-1664"]
owning_seat = "advisor"
stopped_reason = "role leak (BUG-2007): received advisor role context"
"#,
    )
    .unwrap();
    let tick1_findings = scan_at(tmp.path(), ts("2026-10-13T10:21:00Z"));
    assert!(
        tick1_findings
            .iter()
            .any(|f| f.id == "lane-liveness:stale-log:codex-factory-20261013"),
        "Incident 1 must produce a finding within one tick: {tick1_findings:?}"
    );

    // Incident 2 (2026-10-13): Merge seat's PR watcher used a fixed list, so 8
    // FACTORY READY PRs went unseen for up to 5h.
    // Replay all 8 FACTORY READY PRs at 31m after READY (first tick past 30m):
    let ready_prs: Vec<serde_json::Value> = (2601..=2608)
        .map(|pr| {
            serde_json::json!({
                "number": pr,
                "title": format!("[AI:codex] feat: factory item (TASK-{pr})"),
                "headRefOid": format!("sha{pr}abcdef"),
                "labels": [{"name": "factory"}],
                "comments": [{
                    "body": format!("FACTORY READY: PR #{pr} green at sha{pr}abcdef"),
                    "createdAt": "2026-10-13T12:00:00Z"
                }]
            })
        })
        .collect();
    std::fs::write(
        ll_dir.join("prs.json"),
        serde_json::to_string(&ready_prs).unwrap(),
    )
    .unwrap();
    let tick2_findings = scan_at(tmp.path(), ts("2026-10-13T12:31:00Z"));
    for pr in 2601..=2608 {
        let expected_id = format!("lane-liveness:unpicked-ready-pr:PR-{pr}");
        assert!(
            tick2_findings.iter().any(|f| f.id == expected_id),
            "Incident 2 must flag unpicked READY PR #{pr} within one tick: {tick2_findings:?}"
        );
    }

    // Incident 3 (2026-10-13 16:51): The headless Codex run exited at 16:51
    // with assigned work BUG-2014 and TASK-1742.
    // Detected on the very next tick (17:00, within 10m):
    std::fs::write(
        lanes_dir.join("incident3-headless-codex.toml"),
        r#"
id = "codex-headless-2026-10-13-c"
kind = "headless-exec"
pid = 314159
alive = false
log_updated_at = "2026-10-13T16:51:00Z"
assigned_work = ["BUG-2014", "TASK-1742"]
owning_seat = "advisor"
stopped_reason = "exited at 16:51"
"#,
    )
    .unwrap();
    let tick3_findings = scan_at(tmp.path(), ts("2026-10-13T17:00:00Z"));
    assert!(
        tick3_findings
            .iter()
            .any(|f| f.id == "lane-liveness:dead-lane:codex-headless-2026-10-13-c"),
        "Incident 3 must produce a dead-lane finding within one tick: {tick3_findings:?}"
    );

    // Incident 4 (2026-10-13 16:51): An implementer subagent (for BUG-1929) died
    // when its parent advisor session (de277165) rotated at 16:51, leaving an
    // orphaned spec lease held by a dead process with no pushed progress > 30m.
    // Detected on the first tick past 30m (17:22):
    let dead_subagent_lease = crate::SessionLease {
        tree_kind: Default::default(),
        id: "de277165sub8".into(),
        scope: "BUG-1929".into(),
        slug: "bug-1929".into(),
        owner: "claude-subagent-de277165".into(),
        worktree_path: tmp.path().join("wt-bug-1929"),
        branch: "bug-1929".into(),
        started_at: ts("2026-10-13T16:51:00Z"),
        hostname: "test-host".into(),
        role: Some("implementer".into()),
        creator_pid: Some(999_991),
        creator_pid_start_time: Some("2026-10-13T16:08:00+00:00".into()),
        active_pid: Some(999_991),
        active_pid_start_time: Some("2026-10-13T16:08:00+00:00".into()),
        cargo_target_dir: None,
        parent_project_root: Some(tmp.path().to_path_buf()),
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: false,
        claim_verb: false,
        manual_enter_at: None,
        interrupted_at: None,
        interrupted_reason: None,
        driver: None,
        child_grant_id: None,
    };
    std::fs::write(
        sessions_dir.join("de277165sub8.toml"),
        toml::to_string(&dead_subagent_lease).unwrap(),
    )
    .unwrap();

    let tick4_findings = scan_and_route_at(tmp.path(), ts("2026-10-13T17:22:00Z"), true);
    assert!(
        tick4_findings
            .iter()
            .any(|f| f.id == "lane-liveness:orphaned-work:BUG-1929"),
        "Incident 4 must produce an orphaned-work finding within one tick: {tick4_findings:?}"
    );
    let advisor_due = crate::maintenance_schedule::due_seat_jobs(tmp.path(), Some("advisor"));
    assert!(
        advisor_due
            .iter()
            .any(|j| j.name == "lane-liveness-advisor"),
        "lane-liveness-advisor must be marked due within the same tick: {advisor_due:?}"
    );
    let merge_due = crate::maintenance_schedule::due_seat_jobs(tmp.path(), Some("merge"));
    assert!(
        merge_due.iter().any(|j| j.name == "lane-liveness-merge"),
        "lane-liveness-merge must be marked due within the same tick: {merge_due:?}"
    );
}

#[test]
fn production_drain_driver_and_headless_child_lanes_are_discovered() {
    let now = ts("2026-10-13T18:00:00Z");
    let tmp = tempfile::tempdir().unwrap();
    let aida_dir = tmp.path().join(".aida");
    let drivers_dir = aida_dir.join("drivers");
    let sessions_dir = aida_dir.join("sessions");
    let logs_dir = aida_dir.join("headless-logs");
    std::fs::create_dir_all(&drivers_dir).unwrap();
    std::fs::create_dir_all(&sessions_dir).unwrap();
    std::fs::create_dir_all(&logs_dir).unwrap();

    // 1. `.aida/drain-state.json` from a dead drain process with active work
    std::fs::write(
        aida_dir.join("drain-state.json"),
        serde_json::json!({
            "command": "aida queue work --batch b1 --auto-complete",
            "mode": "batch",
            "batch": "b1",
            "members": [
                {"spec": "TASK-1701", "state": "in-phase-1"},
                {"spec": "TASK-1702", "state": "queued"}
            ],
            "orchestrator_pid": 999_992,
            "started_at": "2026-10-13T17:00:00Z",
            "on_drain_complete": "stop"
        })
        .to_string(),
    )
    .unwrap();

    // 2. `.aida/drivers/<kind>-<id>.json` from a dead driver process with a selector
    std::fs::write(
        drivers_dir.join("agent-drv-1.json"),
        serde_json::json!({
            "kind": "agent",
            "id": "drv-1",
            "pid": 999_993,
            "selector": "STORY-1600",
            "started_at": "2026-10-13T17:00:00Z",
            "heartbeat_at": "2026-10-13T17:10:00Z",
            "seat": "advisor"
        })
        .to_string(),
    )
    .unwrap();

    // 3. Headless child lane registration + non-zero exit retention
    let lease = crate::SessionLease {
        tree_kind: Default::default(),
        id: "lease-hl-1".into(),
        scope: "BUG-2014".into(),
        slug: "bug-2014".into(),
        owner: "codex".into(),
        worktree_path: tmp.path().to_path_buf(),
        branch: "bug-2014".into(),
        started_at: now - Duration::minutes(5),
        hostname: "test-host".into(),
        role: Some("implementer".into()),
        creator_pid: Some(999_994),
        creator_pid_start_time: None,
        active_pid: Some(999_994),
        active_pid_start_time: None,
        cargo_target_dir: None,
        parent_project_root: None,
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: false,
        claim_verb: false,
        manual_enter_at: None,
        interrupted_at: None,
        interrupted_reason: None,
        driver: None,
        child_grant_id: None,
    };
    std::fs::write(
        sessions_dir.join("lease-hl-1.toml"),
        toml::to_string(&lease).unwrap(),
    )
    .unwrap();
    let log_path = logs_dir.join("sess-hl-1.log");
    std::fs::write(&log_path, "headless output\n").unwrap();

    register_headless_lane_for_child(&log_path, "sess-hl-1", Some("lease-hl-1"), "codex", 999_994);
    let fail_status = std::process::Command::new("git")
        .arg("--not-a-valid-git-flag")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    finish_headless_lane_for_child(
        &log_path,
        "sess-hl-1",
        Some("lease-hl-1"),
        Some(fail_status),
    );

    let findings = scan_at(tmp.path(), now);
    let ids: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
    assert!(
        ids.contains(&"lane-liveness:dead-lane:drain-batch-b1"),
        "expected dead drain lane finding in {ids:?}"
    );
    assert!(
        ids.contains(&"lane-liveness:dead-lane:drv-1"),
        "expected dead driver lane finding in {ids:?}"
    );
    assert!(
        ids.contains(&"lane-liveness:dead-lane:codex-headless-lease-hl-1"),
        "expected dead headless child lane finding in {ids:?}"
    );

    // Clean exit (`exit 0`) removes the headless lane record so finished runs do not false-positive
    let ok_status = std::process::Command::new("git")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    finish_headless_lane_for_child(&log_path, "sess-hl-1", Some("lease-hl-1"), Some(ok_status));
    assert!(
        !aida_dir
            .join("lanes")
            .join("headless-lease-hl-1.json")
            .exists(),
        "clean headless exit must remove its lane record"
    );
}

#[test]
fn review_round1_regressions_sha_prefix_hold_clear_unknown_kind_and_unpushed_commits() {
    let now = ts("2026-10-13T18:00:00Z");
    assert_eq!(LaneKind::parse("unknown-kind"), None);
    assert_eq!(LaneKind::parse("factory"), Some(LaneKind::Factory));

    let tmp = tempfile::tempdir().unwrap();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.email", "t@t.t"],
        &["config", "user.name", "t"],
        &["commit", "--allow-empty", "-q", "-m", "initial"],
        &[
            "remote",
            "add",
            "origin",
            "https://example.invalid/repo.git",
        ],
        &["checkout", "-q", "-b", "feat-unpushed"],
        &["commit", "--allow-empty", "-q", "-m", "local only commit"],
    ] {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(args)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?} failed");
    }
    // Because `origin` remote is configured and `refs/remotes/origin/feat-unpushed`
    // does not exist, unpushed local commits do NOT count as pushed branch progress.
    assert_eq!(
        pushed_branch_progress_ts(tmp.path(), "feat-unpushed"),
        None,
        "local-only commits on a repo with origin must not count as pushed progress"
    );

    // Unknown lane kind in `.aida/lanes/` is skipped rather than coerced to `Factory`
    let lanes_dir = tmp.path().join(".aida").join("lanes");
    std::fs::create_dir_all(&lanes_dir).unwrap();
    std::fs::write(
        lanes_dir.join("bogus.toml"),
        r#"
id = "bogus-lane"
kind = "typo-kind"
pid = 99999
alive = false
assigned_work = ["TASK-1"]
"#,
    )
    .unwrap();

    // 7-char abbreviated SHA in REVIEW comment marks review_started = true;
    // later hold-clear comment clears needs_operator_hold_clear.
    let ll_dir = tmp.path().join(".aida").join("lane-liveness");
    std::fs::create_dir_all(&ll_dir).unwrap();
    let prs_json = serde_json::json!([
        {
            "number": 2701,
            "title": "[AI:codex] feat: short sha review (TASK-2701)",
            "headRefOid": "1234567890abcdef1234567890abcdef12345678",
            "labels": [{"name": "factory"}],
            "comments": [
                {
                    "body": "FACTORY READY: PR #2701 green at 1234567",
                    "createdAt": "2026-10-13T17:00:00Z"
                },
                {
                    "body": "REVIEW (merge): picked up at 1234567",
                    "createdAt": "2026-10-13T17:05:00Z"
                }
            ]
        },
        {
            "number": 2702,
            "title": "[AI:codex] fix: cleared hold (TASK-2702)",
            "headRefOid": "abcdef1234567890abcdef1234567890abcdef12",
            "labels": [],
            "comments": [
                {
                    "body": "MERGE: ready, needs operator hold clear: aida merge-hold clear 2702",
                    "createdAt": "2026-10-13T17:00:00Z"
                },
                {
                    "body": "Cleared merge-hold on PR #2702",
                    "createdAt": "2026-10-13T17:10:00Z"
                }
            ]
        }
    ]);
    std::fs::write(ll_dir.join("prs.json"), prs_json.to_string()).unwrap();

    let findings = scan_at(tmp.path(), now);
    assert!(
        findings.is_empty(),
        "7-char SHA review pickup, cleared hold, and unknown lane kind must produce no findings: {findings:?}"
    );
}

#[test]
fn production_gh_pr_list_fallback_reads_open_prs() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let now = ts("2026-10-13T18:00:00Z");
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let gh_script = bin_dir.join("gh");
    crate::test_exec::write_executable(
        &gh_script,
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo "gh version 2.0.0"
  exit 0
fi
if [ "$1" = "pr" ] && [ "$2" = "list" ]; then
  cat <<'EOF'
[{"number":2699,"title":"[AI:codex] feat: live gh fallback (TASK-2699)","headRefOid":"deadbeef1234","labels":[{"name":"factory"}],"comments":[{"body":"FACTORY READY: PR #2699 green at deadbeef","createdAt":"2026-10-13T17:00:00Z"}]}]
EOF
  exit 0
fi
exit 1
"#,
    );
    let old_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{old_path}", bin_dir.display());
    let _guards = crate::test_env::EnvVarsGuard::set(&[
        ("PATH", new_path.as_str()),
        ("AIDA_LANE_LIVENESS_GH", "1"),
    ]);

    let findings = scan_at(tmp.path(), now);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].id, "lane-liveness:unpicked-ready-pr:PR-2699");
}
