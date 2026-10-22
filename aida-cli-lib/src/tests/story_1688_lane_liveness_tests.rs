//! Tests for STORY-1688: the `lane-liveness` substrate job. Every fixture is
//! written through the production writer for its record: session leases in
//! the on-disk form `aida claim` writes, the driver registry, drain state, the
//! headless child lane hooks, merge holds, the mailbox, and review verdicts.
//! The one input AIDA does not write itself, `gh pr list` output, is fed in
//! its real JSON shape through the [`PrSource`] seam. Unix-only: fixtures
//! spawn and reap real processes (`sh`, `sleep`) for live and dead pids.
// trace:STORY-1688 | ai:claude
#![cfg(unix)]

use super::*;
use crate::leases::{lease_path, leases_dir, SessionLease};
use aida_core::driver_registry::{self, DriverKind, Registration};
use aida_core::mailbox::{Intent, Message, Recipient};

const HEAD_A: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678";
const HEAD_B: &str = "b2c3d4e5f60718293a4b5c6d7e8f901234567890";

type Pid = (u32, Option<String>);

/// A process that really existed and is now dead, with its start identity.
fn dead_pid() -> Pid {
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let pid = child.id();
    let start = crate::process_probe::process_start_identity(pid);
    child.kill().unwrap();
    child.wait().unwrap();
    (pid, start)
}

/// This test process: alive for the whole test.
fn live_pid() -> Pid {
    let pid = std::process::id();
    (pid, crate::process_probe::process_start_identity(pid))
}

fn lease(id: &str, scope: &str, pid: &Pid, started_at: DateTime<Utc>) -> SessionLease {
    SessionLease {
        tree_kind: Default::default(),
        id: id.to_string(),
        scope: scope.to_string(),
        slug: scope.to_ascii_lowercase(),
        owner: "codex".into(),
        worktree_path: PathBuf::from(format!("/nonexistent/story1688/{id}")),
        branch: format!("codex/{}", scope.to_ascii_lowercase()),
        started_at,
        hostname: "h".into(),
        role: Some("implementer".into()),
        creator_pid: None,
        creator_pid_start_time: None,
        active_pid: Some(pid.0),
        active_pid_start_time: pid.1.clone(),
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
        claim_verb: true,
        manual_enter_at: None,
        interrupted_at: None,
        interrupted_reason: None,
        driver: None,
        child_grant_id: None,
    }
}

/// Persist a lease exactly as `aida claim` does.
fn write_lease(root: &Path, l: &SessionLease) {
    std::fs::create_dir_all(leases_dir(root)).unwrap();
    aida_core::write_atomic(&lease_path(root, &l.id), toml::to_string_pretty(l).unwrap()).unwrap();
}

fn register_driver(root: &Path, id: &str, pid: &Pid, seat: Option<&str>, handoff: Option<&Path>) {
    driver_registry::register(
        &crate::driver_view::driver_registry_root(root),
        Registration {
            kind: DriverKind::Agent,
            id: id.to_string(),
            pid: pid.0,
            selector: None,
            seat: seat.map(str::to_string),
            handoff: handoff.map(|p| p.display().to_string()),
            ttl_secs: None,
        },
        pid.1.clone(),
        Utc::now(),
    )
    .unwrap();
}

/// Write one mailbox message through the production writer.
fn mail(
    root: &Path,
    id: &str,
    thread: &str,
    from: &str,
    to: &str,
    intent: Intent,
    at: DateTime<Utc>,
) {
    crate::mailbox_store::write_message(
        root,
        &Message {
            id: id.to_string(),
            thread_id: thread.to_string(),
            from: from.to_string(),
            to: Recipient::Agent(to.to_string()),
            timestamp: at.timestamp_millis(),
            in_reply_to: (id != thread).then(|| thread.to_string()),
            body: format!("{id} body"),
            subject: Some(format!("{id} subject")),
            urgent: false,
            intent,
            retracted: false,
            deleted: false,
            archived: false,
            from_source: Default::default(),
            from_role: None,
            relayed_from: None,
        },
    )
    .unwrap();
}

fn record_verdict(root: &Path, key: &str, sha: &str) {
    let path = crate::review_verdict::verdict_path(root, key);
    let obj = crate::review_verdict::build_verdict_object(
        root,
        &path,
        Some("approved"),
        Some(sha),
        None,
        Some("looks right"),
        &[],
        "reviewer seat",
    )
    .unwrap();
    crate::review_verdict::write_verdict_object(&path, &obj).unwrap();
}

fn rfc(ts: DateTime<Utc>) -> String {
    ts.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// One entry of real `gh pr list --json number,title,isDraft,headRefOid,labels,statusCheckRollup`
/// output. `green_at: None` is a head whose only check is still running.
fn gh_pr(
    number: u64,
    head: &str,
    green_at: Option<DateTime<Utc>>,
    draft: bool,
) -> serde_json::Value {
    let started = rfc(Utc::now() - Duration::hours(2));
    let check = match green_at {
        Some(done) => serde_json::json!({
            "__typename": "CheckRun",
            "completedAt": rfc(done),
            "conclusion": "SUCCESS",
            "detailsUrl": "https://github.com/joemooney/aida/actions/runs/1/job/1",
            "name": "Build",
            "startedAt": started,
            "status": "COMPLETED",
            "workflowName": "CI"
        }),
        None => serde_json::json!({
            "__typename": "CheckRun",
            "completedAt": "0001-01-01T00:00:00Z",
            "conclusion": "",
            "detailsUrl": "https://github.com/joemooney/aida/actions/runs/1/job/1",
            "name": "Build",
            "startedAt": started,
            "status": "IN_PROGRESS",
            "workflowName": "CI"
        }),
    };
    serde_json::json!({
        "headRefOid": head,
        "isDraft": draft,
        "labels": [{"id": "LA_1", "name": "factory", "description": "", "color": "ededed"}],
        "number": number,
        "statusCheckRollup": [check],
        "title": format!("[AI:codex] fix: change {number} (TASK-{number})")
    })
}

/// A PR at `head` whose Build check went green at `green_at` and whose
/// `merge-hold-gate` check fails, as it does while the hold label is on.
fn gh_pr_held(number: u64, head: &str, green_at: DateTime<Utc>) -> serde_json::Value {
    let mut pr = gh_pr(number, head, Some(green_at), false);
    pr["statusCheckRollup"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "__typename": "CheckRun",
            "completedAt": rfc(green_at - Duration::minutes(5)),
            "conclusion": "FAILURE",
            "detailsUrl": "https://github.com/joemooney/aida/actions/runs/2/job/2",
            "name": "merge-hold-gate",
            "startedAt": rfc(green_at - Duration::minutes(6)),
            "status": "COMPLETED",
            "workflowName": "merge-hold-gate"
        }));
    pr
}

fn prs_json(prs: Vec<serde_json::Value>) -> String {
    serde_json::Value::Array(prs).to_string()
}

fn no_prs(_: &Path) -> Result<String, String> {
    Ok("[]".to_string())
}

fn findings_at(root: &Path, now: DateTime<Utc>, prs: PrSource<'_>) -> Vec<LaneLivenessFinding> {
    let policy = LaneLivenessPolicy::default();
    evaluate_snapshot(&load_snapshot(root, now, &policy, prs), &policy, now)
}

fn ids(findings: &[LaneLivenessFinding]) -> Vec<&str> {
    findings.iter().map(|f| f.id.as_str()).collect()
}

fn route_of(findings: &[LaneLivenessFinding], id: &str) -> Route {
    findings
        .iter()
        .find(|f| f.id == id)
        .unwrap_or_else(|| panic!("no finding {id} in {:?}", ids(findings)))
        .route
}

fn headless_log(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(".aida").join("headless-logs");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.jsonl"));
    std::fs::write(&path, "{}\n").unwrap();
    path
}

/// Run a real child through the production lane hooks, as
/// `run_vendor_headless_child` does.
#[cfg(unix)]
fn run_headless_child(log: &Path, lease_id: &str, script: &str) {
    let mut child = std::process::Command::new("sh")
        .args(["-c", script])
        .spawn()
        .unwrap();
    register_headless_lane_for_child(log, Some(lease_id), "codex", child.id());
    let status = child.wait().unwrap();
    finish_headless_lane_for_child(log, Some(lease_id), Some(status));
}

fn seat_ledger_due(root: &Path, seat: &str) -> bool {
    schedule_ledger::load(&root.join(".aida-store"), &route_job_name(seat))
        .is_some_and(|l| l.due_since.is_some())
}

fn fired_events(root: &Path) -> Vec<String> {
    events::read_all(root)
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::CronJobFired { job, .. } => Some(job),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Criterion 1: a fixture per check
// ---------------------------------------------------------------------------

// Dead lanes from each production lane source: a driver record, a crashed
// drain, and a headless child that exited non-zero. A lane's own lease is
// not reported a second time as orphaned work.
// trace:STORY-1688.acd96462 | ai:claude
#[test]
fn dead_lane_from_driver_drain_and_headless_child() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    let dead = dead_pid();

    register_driver(root, "codex-1", &dead, Some("integrator"), None);
    let mut driven = lease("drv1", "TASK-10", &dead, now - Duration::hours(1));
    driven.driver = Some("agent:codex-1".into());
    write_lease(root, &driven);

    let mut drain = crate::drain_state::DrainState::new_batch(
        "night",
        &["TASK-20".to_string(), "TASK-21".to_string()],
    );
    drain.orchestrator_pid = dead.0;
    drain.orchestrator_pid_start = dead.1.clone();
    drain.write(root).unwrap();

    write_lease(root, &lease("hl1", "TASK-30", &live_pid(), now));
    run_headless_child(&headless_log(root, "codex-hl1"), "hl1", "exit 3");

    let found = findings_at(root, now, &no_prs);
    let got = ids(&found);
    for want in [
        "lane-liveness:dead-lane:agent:codex-1",
        "lane-liveness:dead-lane:drain-batch-night",
        "lane-liveness:dead-lane:codex-headless-hl1",
    ] {
        assert!(got.contains(&want), "missing {want}: {got:?}");
    }
    assert_eq!(
        route_of(&found, "lane-liveness:dead-lane:agent:codex-1"),
        Route::Seat("integrator"),
        "a driver's registered seat owns its dead lane"
    );
    assert!(
        !got.contains(&"lane-liveness:orphaned-work:drv1"),
        "the dead driver's lease is reported once, as the dead lane: {got:?}"
    );
    let headless = found
        .iter()
        .find(|f| f.id == "lane-liveness:dead-lane:codex-headless-hl1")
        .unwrap();
    assert!(
        headless.summary.contains("exit status: 3"),
        "{}",
        headless.summary
    );
}

// A drain halted on purpose (`aida drain stop` sets `stopped_at`) is not a dead lane.
// trace:STORY-1688.acd96462 | ai:claude
#[test]
fn deliberately_halted_drain_is_not_a_dead_lane() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let dead = dead_pid();
    let mut drain = crate::drain_state::DrainState::new_batch("night", &["TASK-20".to_string()]);
    drain.orchestrator_pid = dead.0;
    drain.orchestrator_pid_start = dead.1.clone();
    drain.stopped_at = Some(rfc(Utc::now()));
    drain.stopped_reason = Some("operator stop".into());
    drain.write(root).unwrap();
    assert!(findings_at(root, Utc::now(), &no_prs).is_empty());
}

// A live driver whose handoff file has not moved past the budget is a stale
// lane; under the budget it is silent.
// trace:STORY-1688.acd96462 | ai:claude
#[test]
fn stale_log_on_a_live_driver_lane() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    let handoff = root.join("handoff.md");
    std::fs::write(&handoff, "working").unwrap();
    register_driver(root, "relay-run", &live_pid(), None, Some(&handoff));
    let mut driven = lease("drv2", "TASK-40", &live_pid(), now);
    driven.driver = Some("agent:relay-run".into());
    write_lease(root, &driven);

    assert!(findings_at(root, now + Duration::minutes(20), &no_prs).is_empty());
    let found = findings_at(root, now + Duration::minutes(45), &no_prs);
    assert_eq!(ids(&found), ["lane-liveness:stale-log:agent:relay-run"]);
    assert_eq!(found[0].route, Route::Seat("advisor"));
}

// A PR green at its head past the budget with no review at that head routes
// to the reviewer. A live review lease or a verdict AT THE HEAD counts as
// review started; a verdict at an older head, a draft, a pending head and a
// PR under the budget do not trip.
// trace:STORY-1688.acd96462 | ai:claude
#[test]
fn unpicked_ready_pr_and_its_negatives() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    let green = Some(now - Duration::minutes(40));
    let head_c = "c3d4e5f60718293a4b5c6d7e8f901234567890ab";

    let mut review = lease("rv1", "PR-2702", &live_pid(), now);
    review.review_verb = true;
    review.claim_verb = false;
    review.worktree_path = PathBuf::new();
    review.creator_pid = Some(std::process::id());
    review.creator_pid_start_time = live_pid().1;
    review.pr_head_sha = Some(HEAD_B.into());
    write_lease(root, &review);
    record_verdict(root, "TASK-2703", head_c);
    record_verdict(
        root,
        "TASK-2704",
        "d4e5f60718293a4b5c6d7e8f901234567890abcd",
    );

    let feed = prs_json(vec![
        gh_pr(2701, HEAD_A, green, false),
        gh_pr(2702, HEAD_B, green, false),
        gh_pr(2703, head_c, green, false),
        gh_pr(
            2704,
            "e5f60718293a4b5c6d7e8f901234567890abcdef",
            green,
            false,
        ),
        gh_pr(
            2705,
            "f60718293a4b5c6d7e8f901234567890abcdef01",
            Some(now - Duration::minutes(10)),
            false,
        ),
        gh_pr(
            2706,
            "0718293a4b5c6d7e8f901234567890abcdef0123",
            green,
            true,
        ),
        gh_pr(
            2707,
            "18293a4b5c6d7e8f901234567890abcdef012345",
            None,
            false,
        ),
    ]);
    let prs = move |_: &Path| Ok(feed.clone());
    let found = findings_at(root, now, &prs);
    assert_eq!(
        ids(&found),
        [
            "lane-liveness:unpicked-ready-pr:PR-2701@a1b2c3d4",
            "lane-liveness:unpicked-ready-pr:PR-2704@e5f60718",
        ]
    );
    assert!(found.iter().all(|f| f.route == Route::Seat("reviewer")));
}

// Advisor condition 1: a forge failure or timeout skips the review-pickup
// check for the tick. It routes nothing false and the other checks still run.
// trace:STORY-1688.acd96462 | ai:claude
#[test]
fn forge_failure_skips_review_pickup_without_failing_the_tick() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    write_lease(
        root,
        &lease("or1", "TASK-50", &dead_pid(), now - Duration::hours(1)),
    );

    let timed_out =
        |_: &Path| Err("`gh pr list` timed out after 20s or failed to start".to_string());
    let garbled = |_: &Path| Ok("not json".to_string());
    for prs in [&timed_out as PrSource<'_>, &garbled as PrSource<'_>] {
        let policy = LaneLivenessPolicy::default();
        let snapshot = load_snapshot(root, now, &policy, prs);
        assert!(snapshot.forge_error.is_some());
        assert!(snapshot.open_prs.is_empty());
        let routed = scan_and_route_at(root, now, true, prs);
        assert_eq!(
            routed.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            ["lane-liveness:orphaned-work:or1"]
        );
    }
    assert!(!seat_ledger_due(root, "reviewer"));
}

// A human-only merge hold past the budget is an operator notification, not
// a seat job. A reviewer refusal (rework) hold is not a human gate.
// trace:STORY-1688.acd96462 | ai:claude
#[test]
fn stale_human_merge_hold_routes_to_the_operator_only() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    crate::merge_hold::write_typed_hold(
        root,
        &crate::merge_hold::typed_hold(
            2710,
            crate::merge_hold::HoldReasonKind::Supervision,
            "supervised spec needs a human",
            Some(HEAD_A.into()),
        ),
    )
    .unwrap();
    crate::merge_hold::write_typed_hold(
        root,
        &crate::merge_hold::typed_hold(
            2711,
            crate::merge_hold::HoldReasonKind::Rework,
            "CHANGES REQUESTED",
            Some(HEAD_B.into()),
        ),
    )
    .unwrap();

    assert!(findings_at(root, now + Duration::minutes(10), &no_prs).is_empty());
    let found = findings_at(root, now + Duration::minutes(20), &no_prs);
    assert_eq!(
        ids(&found),
        ["lane-liveness:gate-wait-operator:PR-2710@a1b2c3d4"]
    );
    assert_eq!(found[0].route, Route::OperatorNotify);
}

// Advisor fork (A): a request or handoff mailed to the advisor with no reply
// in its thread is the advisor gate. FYI mail, a replied thread, the
// advisor's own mail and a request older than the age window do not trip.
// trace:STORY-1688.acd96462 | ai:claude
#[test]
fn unanswered_advisor_request_and_its_negatives() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    let old = now - Duration::hours(30);
    mail(
        root,
        "m-open",
        "m-open",
        "fix-2743",
        "advisor",
        Intent::Request,
        now,
    );
    mail(
        root,
        "m-fyi",
        "m-fyi",
        "fix-2743",
        "advisor",
        Intent::Fyi,
        now,
    );
    mail(
        root,
        "m-answered",
        "m-answered",
        "fix-2744",
        "advisor",
        Intent::Request,
        now,
    );
    let reply_at = now + Duration::minutes(5);
    mail(
        root,
        "m-reply",
        "m-answered",
        "advisor",
        "fix-2744",
        Intent::Fyi,
        reply_at,
    );
    mail(
        root,
        "m-self",
        "m-self",
        "advisor",
        "advisor",
        Intent::Request,
        now,
    );
    mail(
        root,
        "m-old",
        "m-old",
        "fix-2600",
        "advisor",
        Intent::Handoff,
        old,
    );

    assert!(findings_at(root, now + Duration::minutes(10), &no_prs).is_empty());
    let found = findings_at(root, now + Duration::minutes(20), &no_prs);
    assert_eq!(ids(&found), ["lane-liveness:gate-wait-advisor:m-open"]);
    assert_eq!(found[0].route, Route::Seat("advisor"));
}

// Orphaned work is keyed on the lease, so two dead leases on one spec are two
// findings. Review, manual and interrupted leases, and leases past the age
// window, are not orphaned work.
// trace:STORY-1688.acd96462 | ai:claude
#[test]
fn orphaned_work_keys_on_lease_and_skips_non_work_leases() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    let dead = dead_pid();
    let hour_ago = now - Duration::hours(1);
    write_lease(root, &lease("bug-a", "BUG-1918", &dead, hour_ago));
    write_lease(root, &lease("bug-b", "BUG-1918", &dead, hour_ago));
    let mut review = lease("rv", "PR-2720", &dead, hour_ago);
    review.review_verb = true;
    write_lease(root, &review);
    let mut manual = lease("man", "TASK-60", &dead, hour_ago);
    manual.manual_enter_at = Some(hour_ago);
    write_lease(root, &manual);
    let mut interrupted = lease("int", "TASK-61", &dead, hour_ago);
    interrupted.interrupted_at = Some(hour_ago);
    write_lease(root, &interrupted);
    write_lease(
        root,
        &lease("old", "TASK-62", &dead, now - Duration::days(3)),
    );
    write_lease(
        root,
        &lease("fresh", "TASK-63", &dead, now - Duration::minutes(10)),
    );

    let found = findings_at(root, now, &no_prs);
    assert_eq!(
        ids(&found),
        [
            "lane-liveness:orphaned-work:bug-a",
            "lane-liveness:orphaned-work:bug-b",
        ]
    );
}

// R1: the live backlog (21 dead leases, oldest 2026-10-04) does not trip the
// guard once it ships; only work that died inside the age window does.
// trace:STORY-1688.acd96462 | ai:claude
#[test]
fn old_dead_lease_backlog_does_not_trip_the_guard() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    let dead = dead_pid();
    for i in 0..21 {
        write_lease(
            root,
            &lease(
                &format!("backlog{i:02}"),
                &format!("TASK-{}", 1000 + i),
                &dead,
                now - Duration::days(2 + i64::from(i % 14)),
            ),
        );
    }
    write_lease(
        root,
        &lease("today", "TASK-2000", &dead, now - Duration::minutes(45)),
    );
    assert_eq!(
        ids(&findings_at(root, now, &no_prs)),
        ["lane-liveness:orphaned-work:today"]
    );
}

/// A distributed store holding `specs`, configured for `root`.
fn store_with(root: &Path, specs: &[(&str, aida_core::RequirementStatus)]) {
    use aida_core::DatabaseBackend;
    let store = root.join(".aida-store");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    aida_core::git_ops::init(&store).unwrap();
    aida_core::git_ops::configure_user(&store, "Test", "test@example.com").unwrap();
    std::fs::write(
        root.join(".aida").join("config.toml"),
        "store_path = \".aida-store\"\n",
    )
    .unwrap();
    let cache = aida_core::CachedGitBackend::default_cache_path(&store);
    let backend = aida_core::CachedGitBackend::open(&store, &cache).unwrap();
    for (spec, status) in specs {
        let mut req = aida_core::Requirement::new(format!("spec {spec}"), "body".into());
        req.spec_id = Some(spec.to_string());
        req.status = status.clone();
        backend.add_requirement(req).unwrap();
    }
}

// Work on a completed or rejected spec is not stranded: no orphaned-work or
// dead-lane finding, and the routed tick deletes the closed headless record.
// trace:STORY-1688.acd96462 | ai:claude
#[test]
fn closed_specs_are_not_stranded_and_their_lane_record_is_collected() {
    use aida_core::RequirementStatus as S;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    store_with(
        root,
        &[
            ("BUG-3", S::Completed),
            ("BUG-4", S::Rejected),
            ("BUG-5", S::InProgress),
        ],
    );
    let dead = dead_pid();
    write_lease(
        root,
        &lease("done", "BUG-3", &dead, now - Duration::hours(1)),
    );
    write_lease(
        root,
        &lease("open", "BUG-5", &dead, now - Duration::hours(1)),
    );
    write_lease(root, &lease("hl", "BUG-4", &live_pid(), now));
    run_headless_child(&headless_log(root, "codex-hl"), "hl", "exit 1");
    let record = root.join(".aida/lanes/headless-hl.json");
    assert!(record.exists());

    let routed = scan_and_route_at(root, now, true, &no_prs);
    assert_eq!(
        routed.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
        ["lane-liveness:orphaned-work:open"]
    );
    assert!(
        !record.exists(),
        "the closed spec's lane record is collected"
    );
}

// R2: the headless record follows its lease. A child that released its lease
// leaves no record; a record whose lease is later released, or that aged past
// the window, is collected by the routed tick and reports nothing.
// trace:STORY-1688.acd96462 | ai:claude
#[test]
fn headless_lane_record_expires_with_its_lease_and_age() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    let log = headless_log(root, "codex-a");

    // The child releases its own lease before exiting: finished work.
    write_lease(root, &lease("a", "TASK-70", &live_pid(), now));
    let mut child = std::process::Command::new("true").spawn().unwrap();
    register_headless_lane_for_child(&log, Some("a"), "codex", child.id());
    assert!(root.join(".aida/lanes/headless-a.json").exists());
    std::fs::remove_file(lease_path(root, "a")).unwrap();
    finish_headless_lane_for_child(&log, Some("a"), Some(child.wait().unwrap()));
    assert!(!root.join(".aida/lanes/headless-a.json").exists());

    // Exit with the lease held, then the lease is released later.
    write_lease(root, &lease("b", "TASK-71", &live_pid(), now));
    run_headless_child(&log, "b", "exit 2");
    let record_b = root.join(".aida/lanes/headless-b.json");
    assert!(record_b.exists());
    std::fs::remove_file(lease_path(root, "b")).unwrap();
    assert!(scan_and_route_at(root, now, true, &no_prs).is_empty());
    assert!(!record_b.exists());

    // Exit with the lease held, then 25 hours pass.
    write_lease(root, &lease("c", "TASK-72", &live_pid(), now));
    run_headless_child(&log, "c", "exit 2");
    let record_c = root.join(".aida/lanes/headless-c.json");
    assert!(!findings_at(root, now, &no_prs).is_empty());
    assert!(scan_and_route_at(root, now + Duration::hours(25), true, &no_prs).is_empty());
    assert!(!record_c.exists());
}

// R5/R6: routing writes one due job per registered seat with a new finding,
// sends human-only holds to the operator only, and never names an
// unregistered seat. A finding wakes its seat once, not every tick.
// trace:STORY-1688.acd96462 | ai:claude
#[test]
fn routing_is_one_wake_per_finding_on_registered_seats() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    let feed = prs_json(vec![gh_pr(
        2730,
        HEAD_A,
        Some(now - Duration::minutes(40)),
        false,
    )]);
    let prs = move |_: &Path| Ok(feed.clone());

    // Reviewer-only findings do not wake the advisor.
    scan_and_route_at(root, now, true, &prs);
    assert!(seat_ledger_due(root, "reviewer"));
    assert!(!seat_ledger_due(root, "advisor"));
    assert_eq!(fired_events(root), ["lane-liveness-reviewer"]);

    // Add an advisor finding and a human-only hold.
    write_lease(
        root,
        &lease("or2", "TASK-80", &dead_pid(), now - Duration::hours(1)),
    );
    crate::merge_hold::write_typed_hold(
        root,
        &crate::merge_hold::typed_hold(
            2731,
            crate::merge_hold::HoldReasonKind::Decision,
            "needs a human decision",
            Some(HEAD_B.into()),
        ),
    )
    .unwrap();
    let later = now + Duration::minutes(20);
    let found = scan_and_route_at(root, later, true, &prs);
    assert_eq!(found.len(), 3, "{found:?}");
    assert!(seat_ledger_due(root, "advisor"));
    assert_eq!(
        fired_events(root),
        ["lane-liveness-reviewer", "lane-liveness-advisor"],
        "the advisor wakes once for its own finding; the hold wakes no seat"
    );
    let store = root.join(".aida-store");
    for unregistered in ["lane-liveness-merge", "lane-liveness-operator"] {
        assert!(schedule_ledger::load(&store, unregistered).is_none());
    }

    // The seats handle their jobs; the same findings on the next tick are not re-routed.
    for seat in ["advisor", "reviewer"] {
        schedule_ledger::write_cas(&store, &route_job_name(seat), |l| l.due_since = None).unwrap();
    }
    scan_and_route_at(root, later + Duration::minutes(10), true, &prs);
    assert!(!seat_ledger_due(root, "advisor") && !seat_ledger_due(root, "reviewer"));
    assert_eq!(fired_events(root).len(), 2);
}

// Round 2 (major): a rework push past a reviewer refusal waits on re-review.
// The hold's own merge-hold-gate check is red at every head while the label
// is on, so it is no readiness signal there. A PR still at the refused head,
// or with the gate red and no refusal on record, is not ready.
// trace:STORY-1688.acd96462 | ai:claude
#[test]
fn rework_push_past_a_refusal_is_ready_for_re_review() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    let head_c = "c3d4e5f60718293a4b5c6d7e8f901234567890ab";
    for (pr, refused_at) in [(2740, HEAD_A), (2741, head_c)] {
        crate::merge_hold::write_typed_hold(
            root,
            &crate::merge_hold::typed_hold(
                pr,
                crate::merge_hold::HoldReasonKind::Rework,
                "CHANGES REQUESTED",
                Some(refused_at.into()),
            ),
        )
        .unwrap();
    }
    let green = now - Duration::minutes(40);
    let feed = prs_json(vec![
        gh_pr_held(2740, HEAD_B, green),
        gh_pr_held(2741, head_c, green),
        gh_pr_held(2742, "e5f60718293a4b5c6d7e8f901234567890abcdef", green),
    ]);
    let prs = move |_: &Path| Ok(feed.clone());
    let found = findings_at(root, now, &prs);
    assert_eq!(
        ids(&found),
        ["lane-liveness:unpicked-ready-pr:PR-2740@b2c3d4e5"]
    );
    assert_eq!(found[0].route, Route::Seat("reviewer"));
}

// Round 2: orphaned-work uses the canonical lease state. A session judged
// live keeps its lease even when the shell that created it is gone; a live
// creator shell does not keep a lease whose session is gone.
// trace:STORY-1688.acd96462 | ai:claude
#[test]
fn orphaned_work_follows_canonical_lease_state_not_the_creator_shell() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    let hour_ago = now - Duration::hours(1);
    let worktree = root.join("wt-live");
    std::fs::create_dir_all(&worktree).unwrap();

    let mut live_session = lease("live", "TASK-95", &live_pid(), hour_ago);
    live_session.worktree_path = worktree;
    let dead_shell = dead_pid();
    live_session.creator_pid = Some(dead_shell.0);
    live_session.creator_pid_start_time = dead_shell.1;
    write_lease(root, &live_session);

    let mut gone = lease("gone", "TASK-96", &dead_pid(), hour_ago);
    gone.active_pid = None;
    gone.active_pid_start_time = None;
    gone.creator_pid = Some(std::process::id());
    gone.creator_pid_start_time = live_pid().1;
    write_lease(root, &gone);

    assert_eq!(
        ids(&findings_at(root, now, &no_prs)),
        ["lane-liveness:orphaned-work:gone"]
    );
}

// Round 2: a dead driver's open work is only the spec work it stranded. A
// review lease under its label and a lease whose own session is still live
// are not; the live one is not orphaned, and a dead lease the driver did not
// strand still reports on its own.
// trace:STORY-1688.acd96462 | ai:claude
#[test]
fn dead_driver_counts_only_stranded_spec_leases() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    let dead = dead_pid();
    let hour_ago = now - Duration::hours(1);
    register_driver(root, "advisor-drive", &dead, None, None);
    for (id, scope, pid) in [
        ("stranded", "TASK-90", &dead),
        ("child", "TASK-91", &live_pid()),
        ("review", "PR-2750", &dead),
    ] {
        let mut l = lease(id, scope, pid, hour_ago);
        l.driver = Some("agent:advisor-drive".into());
        write_lease(root, &l);
    }
    let found = findings_at(root, now, &no_prs);
    assert_eq!(ids(&found), ["lane-liveness:dead-lane:agent:advisor-drive"]);
    assert!(
        found[0].summary.contains("[TASK-90]"),
        "{}",
        found[0].summary
    );

    // A dead driver whose only labelled lease is live strands nothing.
    std::fs::remove_file(lease_path(root, "stranded")).unwrap();
    assert!(findings_at(root, now, &no_prs).is_empty());
}

// Round 2: a route that fails is not recorded, so the next tick retries it.
// trace:STORY-1688.acd96462 | ai:claude
#[test]
fn failed_route_is_retried_next_tick() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    write_lease(
        root,
        &lease("or3", "TASK-97", &dead_pid(), now - Duration::hours(1)),
    );
    // The schedule store cannot be written: a file stands where its dir goes.
    let store = root.join(".aida-store");
    std::fs::write(&store, "not a directory").unwrap();
    scan_and_route_at(root, now, true, &no_prs);
    assert!(fired_events(root).is_empty());
    assert!(!load_routed(root).contains_key("lane-liveness:orphaned-work:or3"));

    std::fs::remove_file(&store).unwrap();
    scan_and_route_at(root, now + Duration::minutes(10), true, &no_prs);
    assert!(seat_ledger_due(root, "advisor"));
    assert_eq!(fired_events(root), ["lane-liveness-advisor"]);
    assert!(load_routed(root).contains_key("lane-liveness:orphaned-work:or3"));
}

// Round 2: a tick whose forge read failed keeps the review-pickup ids it
// could not re-check, so the next good read does not re-wake the reviewer.
// trace:STORY-1688.acd96462 | ai:claude
#[test]
fn forge_failure_tick_keeps_review_pickup_dedupe() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    let feed = prs_json(vec![gh_pr(
        2760,
        HEAD_A,
        Some(now - Duration::minutes(40)),
        false,
    )]);
    let good = move |_: &Path| Ok(feed.clone());
    let down = |_: &Path| Err("`gh pr list` timed out after 20s or failed to start".to_string());
    let id = "lane-liveness:unpicked-ready-pr:PR-2760@a1b2c3d4";

    scan_and_route_at(root, now, true, &good);
    let store = root.join(".aida-store");
    schedule_ledger::write_cas(&store, &route_job_name("reviewer"), |l| l.due_since = None)
        .unwrap();
    scan_and_route_at(root, now + Duration::minutes(10), true, &down);
    assert!(
        load_routed(root).contains_key(id),
        "kept through the outage"
    );
    scan_and_route_at(root, now + Duration::minutes(20), true, &good);
    assert_eq!(fired_events(root), ["lane-liveness-reviewer"]);
    assert!(
        !seat_ledger_due(root, "reviewer"),
        "the same PR is not re-woken"
    );
}

// Fork (A) end to end in the templates: the sketch author mails the request,
// and the signing advisor answers in that thread, which is the reply that
// clears the gate (see `unanswered_advisor_request_and_its_negatives`).
// trace:STORY-1688.acd96462 | ai:claude
#[test]
fn orchestrate_templates_send_and_answer_the_sketch_request() {
    for body in [
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../aida-core/templates/skills/aida-orchestrate.md"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../aida-core/templates/skills-portable/aida-orchestrate.md"
        )),
    ] {
        assert!(body.contains("aida mailbox send --to advisor --intent request"));
        assert!(body.contains("aida mailbox send --to <sender> --in-reply-to <request-id>"));
    }
}

fn git(root: &Path, args: &[&str]) {
    let status = crate::test_git::git_command()
        .arg("-C")
        .arg(root)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// A git repo on `main` whose committed `.aida/config.toml` is `config`.
fn repo_with_config(root: &Path, config: &str) {
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.email", "test@example.com"]);
    git(root, &["config", "user.name", "AIDA Test"]);
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(root.join(".aida").join("config.toml"), config).unwrap();
    git(root, &["add", ".aida/config.toml"]);
    git(root, &["commit", "-qm", "seed"]);
}

// Round 3 (major): a reviewer session minted by the production writer
// (`aida queue work PR-<n> --role reviewer` -> `session_start`) has a
// worktree and no agent pid. Its PR counts as review started, not unpicked.
// trace:STORY-1688.acd96462 | ai:claude
#[test]
fn reviewer_session_lease_counts_as_review_started() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    repo_with_config(&root, "");
    // The forge's PR head ref, which `session_start` fetches for a PR scope.
    let origin = tmp.path().join("origin.git");
    git(
        tmp.path(),
        &["init", "-q", "--bare", origin.to_str().unwrap()],
    );
    git(
        &root,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(
        &root,
        &["push", "-q", "origin", "main", "HEAD:refs/pull/2770/head"],
    );
    let head = String::from_utf8(
        crate::test_git::git_command()
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let _ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat_and(&root, "reviewer", &[], &[]);
    let worktree = root.join("review-wt");
    crate::session_start::session_start(
        "PR-2770",
        None,
        None,
        false,
        Some(worktree.to_str().unwrap()),
        Some("github"),
        "auto",
        false,
        None,
        false,
        None,
        None,
        false,
        Some("reviewer".to_string()),
        false,
        Some(false),
        /* self_claim */ true,
        /* review_coexistence */ None,
        /* contained_seat */ false,
    )
    .unwrap();
    let minted = crate::leases::list_leases(&root);
    assert_eq!(minted.len(), 1, "{minted:?}");
    assert!(minted[0].review_verb && minted[0].active_pid.is_none());

    let now = Utc::now();
    let feed = prs_json(vec![gh_pr(
        2770,
        &head,
        Some(now - Duration::minutes(40)),
        false,
    )]);
    let prs = move |_: &Path| Ok(feed.clone());
    assert!(findings_at(&root, now, &prs).is_empty());

    // Once the review session is gone, the PR is unpicked again.
    std::fs::remove_file(lease_path(&root, &minted[0].id)).unwrap();
    assert_eq!(
        ids(&findings_at(&root, now, &prs)),
        [format!(
            "lane-liveness:unpicked-ready-pr:PR-2770@{}",
            &head[..8]
        )]
    );
}

// Round 3 (minor): a human-only hold counts as routed only once a notice
// reached the operator. With notify unconfigured it stays unrouted and
// retries every tick; with a configured command it is recorded once.
// trace:STORY-1688.acd96462 | ai:claude
#[test]
fn operator_gate_is_routed_only_when_a_notice_is_sent() {
    let hold = |root: &Path| {
        crate::merge_hold::write_typed_hold(
            root,
            &crate::merge_hold::typed_hold(
                2780,
                crate::merge_hold::HoldReasonKind::Supervision,
                "supervised spec needs a human",
                Some(HEAD_A.into()),
            ),
        )
        .unwrap();
    };
    let id = "lane-liveness:gate-wait-operator:PR-2780@a1b2c3d4";
    let later = Utc::now() + Duration::minutes(20);

    let unconfigured = tempfile::tempdir().unwrap();
    hold(unconfigured.path());
    for tick in 0..2 {
        let found = scan_and_route_at(
            unconfigured.path(),
            later + Duration::minutes(10 * tick),
            true,
            &no_prs,
        );
        assert_eq!(found.len(), 1);
        assert!(!load_routed(unconfigured.path()).contains_key(id));
    }

    let configured = tempfile::tempdir().unwrap();
    repo_with_config(configured.path(), "[notify]\ncommand = \"true\"\n");
    hold(configured.path());
    scan_and_route_at(configured.path(), later, true, &no_prs);
    assert!(load_routed(configured.path()).contains_key(id));
}

// Every route target is a registered agent role, and every route has a job.
// trace:STORY-1688.acd96462 | ai:claude
#[test]
fn every_route_seat_is_a_registered_agent_role() {
    for route in ROUTES {
        assert!(
            crate::role::AGENT_ROLES.contains(&route.seat),
            "`{}` is not a registered agent role",
            route.seat
        );
        assert_eq!(
            route_for_job(&route_job_name(route.seat)).map(|r| r.seat),
            Some(route.seat)
        );
    }
    assert_eq!(registered_seat(Some("merge")), "advisor");
    assert_eq!(registered_seat(Some("operator")), "advisor");
    assert_eq!(registered_seat(Some("dialog")), "advisor");
    assert_eq!(registered_seat(Some("reviewer")), "reviewer");
}

// ---------------------------------------------------------------------------
// Criterion 2: the 2026-10-13 incidents, replayed
// ---------------------------------------------------------------------------

// Each 2026-10-13 incident, reproduced through the production writer that
// recorded it, yields its finding in ONE routed tick, routed to the seat that
// owns it:
// 1. 09:50: the role leak stopped the Codex factory lane (a driver whose
//    process died while it held work).
// 2. Eight READY PRs sat unseen for up to 5h (green at their heads, no
//    review lease or verdict).
// 3. 16:51: the headless Codex run exited CLEANLY with its lease still held.
// 4. 16:51: an implementer subagent died with its rotated parent, holding a
//    lease with no pushed progress.
// trace:STORY-1688.ace1f616 | ai:claude
#[test]
fn replay_2026_10_13_incidents_each_produce_a_finding_in_one_tick() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    let dead = dead_pid();

    // 1. Codex factory lane.
    register_driver(root, "codex-factory", &dead, None, None);
    let mut factory = lease("factory", "TASK-1800", &dead, now - Duration::minutes(40));
    factory.driver = Some("agent:codex-factory".into());
    write_lease(root, &factory);

    // 2. Eight unreviewed green PRs.
    let heads: Vec<String> = (0..8).map(|i| format!("{i:x}{}", &HEAD_A[1..])).collect();
    let feed = prs_json(
        heads
            .iter()
            .enumerate()
            .map(|(i, head)| {
                gh_pr(
                    2600 + i as u64,
                    head,
                    Some(now - Duration::minutes(45)),
                    false,
                )
            })
            .collect(),
    );
    let prs = move |_: &Path| Ok(feed.clone());

    // 3. Headless Codex exits 0 with work held.
    write_lease(root, &lease("headless", "BUG-2007", &live_pid(), now));
    run_headless_child(&headless_log(root, "codex-headless"), "headless", "exit 0");

    // 4. Subagent orphaned by its parent's rotation.
    write_lease(
        root,
        &lease("subagent", "STORY-1650", &dead, now - Duration::minutes(90)),
    );

    let found: Vec<String> = scan_and_route_at(root, now, true, &prs)
        .into_iter()
        .map(|f| f.id)
        .collect();
    let mut want = vec![
        "lane-liveness:dead-lane:agent:codex-factory".to_string(),
        "lane-liveness:dead-lane:codex-headless-headless".to_string(),
        "lane-liveness:orphaned-work:subagent".to_string(),
    ];
    want.extend(heads.iter().enumerate().map(|(i, head)| {
        format!(
            "lane-liveness:unpicked-ready-pr:PR-{}@{}",
            2600 + i,
            &head[..8]
        )
    }));
    want.sort();
    assert_eq!(found, want);
    assert!(
        seat_ledger_due(root, "advisor"),
        "incidents 1, 3 and 4 wake the advisor"
    );
    assert!(
        seat_ledger_due(root, "reviewer"),
        "incident 2 wakes the reviewer"
    );
    let mut fired = fired_events(root);
    fired.sort();
    assert_eq!(fired, ["lane-liveness-advisor", "lane-liveness-reviewer"]);
}

/// TASK-2030 (a): with a required-check set, only required checks are the
/// readiness signal; a failing optional check no longer blocks pickup, a
/// missing or red required check still does, and an unknown or empty set
/// keeps every check in play.
// trace:TASK-1955 | ai:claude (TASK-2030)
#[test]
fn readiness_counts_only_required_checks_when_the_set_is_known() {
    let now = Utc::now();
    let green_at = now - Duration::minutes(30);
    let mut pr = gh_pr(1, HEAD_A, Some(green_at), false);
    let optional_red = serde_json::json!({
        "__typename": "CheckRun",
        "completedAt": rfc(now - Duration::minutes(10)),
        "conclusion": "FAILURE",
        "name": "optional-lint",
        "startedAt": rfc(now - Duration::minutes(20)),
        "status": "COMPLETED",
        "workflowName": "Lint"
    });
    pr["statusCheckRollup"]
        .as_array_mut()
        .unwrap()
        .push(optional_red);
    let rollup = pr["statusCheckRollup"].as_array().unwrap().clone();
    let build = ["Build".to_string()];
    let ready = rollup_green_at(&rollup, false, Some(&build)).expect("required Build is green");
    assert_eq!(ready.timestamp(), green_at.timestamp());
    assert_eq!(rollup_green_at(&rollup, false, None), None, "unknown set");
    assert_eq!(
        rollup_green_at(&rollup, false, Some(&[])),
        None,
        "unprotected"
    );
    let missing = ["Build".to_string(), "Deploy".to_string()];
    assert_eq!(rollup_green_at(&rollup, false, Some(&missing)), None);
    let red = ["optional-lint".to_string()];
    assert_eq!(rollup_green_at(&rollup, false, Some(&red)), None);
}

/// TASK-2030 (b): a routed `lane-liveness-<seat>` job surfaces through the
/// seat's due jobs and clears with `aida schedule done lane-liveness-<seat>`.
// trace:TASK-1955 | ai:claude (TASK-2030)
#[test]
fn routed_seat_job_is_due_for_its_seat_and_cleared_by_schedule_done() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    let feed = prs_json(vec![gh_pr(
        2730,
        HEAD_A,
        Some(now - Duration::minutes(40)),
        false,
    )]);
    let prs = move |_: &Path| Ok(feed.clone());
    scan_and_route_at(root, now, true, &prs);
    let due = |seat: &str| {
        crate::maintenance_schedule::due_seat_jobs(root, Some(seat))
            .into_iter()
            .map(|j| j.name)
            .collect::<Vec<_>>()
    };
    assert!(
        due("reviewer").contains(&"lane-liveness-reviewer".to_string()),
        "{:?}",
        due("reviewer")
    );
    assert!(!due("advisor").contains(&"lane-liveness-reviewer".to_string()));

    crate::maintenance_schedule::done(root, "lane-liveness-reviewer", None).unwrap();
    assert!(!due("reviewer").contains(&"lane-liveness-reviewer".to_string()));
}

/// TASK-2094 / TASK-1665: an attested reviewer whose creator shell exited is
/// read with the live probes. A crashed reviewer (nothing live in its
/// worktree) no longer suppresses unpicked-ready-pr; a Codex-style reviewer
/// registered inside its worktree still counts as the review started.
// trace:TASK-1955 | ai:claude (TASK-2094, TASK-1665)
#[test]
fn reviewer_lease_with_a_dead_creator_is_read_with_live_probes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let now = Utc::now();
    let worktree = root.join("aida-pr-2730");
    std::fs::create_dir_all(&worktree).unwrap();
    let mut review = lease("rv30", "PR-2730", &dead_pid(), now);
    review.role = Some("reviewer".into());
    review.review_verb = true;
    review.claim_verb = false;
    review.worktree_path = worktree.clone();
    review.active_pid = None;
    review.active_pid_start_time = None;
    let creator = dead_pid();
    review.creator_pid = Some(creator.0);
    review.creator_pid_start_time = creator.1;
    review.pr_head_sha = Some(HEAD_A.into());
    write_lease(&root, &review);
    let feed = prs_json(vec![gh_pr(
        2730,
        HEAD_A,
        Some(now - Duration::minutes(40)),
        false,
    )]);
    let prs = move |_: &Path| Ok(feed.clone());

    assert_eq!(
        ids(&findings_at(&root, now, &prs)),
        ["lane-liveness:unpicked-ready-pr:PR-2730@a1b2c3d4"],
        "a crashed reviewer does not hold the PR"
    );

    // A Codex reviewer's SessionStart registration: this live process,
    // inside the review worktree.
    let me = std::process::id();
    crate::agent_registry::write_entry(
        &root,
        &crate::agent_registry::AgentRegistryEntry {
            id: format!("codex-{me}"),
            agent_type: "codex".into(),
            pid: me,
            pid_start: crate::process_probe::process_start_identity(me),
            name: Some("codex-reviewer".into()),
            description: None,
            tty: None,
            terminal: None,
            started_at: now,
            last_active_at: now,
            role: Some("reviewer".into()),
            current_spec: None,
            worktree_path: worktree.clone(),
            source: "session-start-hook".into(),
            binary_version: None,
            build_sha: None,
            availability: crate::agent_registry::Availability::Available,
            paused_since: None,
            paused_reason: None,
            expected_back: None,
            native_session_id: None,
            claude_session_id: None,
            ended_at: None,
            spec_status_at_end: None,
            resumed_from: None,
        },
    )
    .unwrap();
    assert!(
        ids(&findings_at(&root, now, &prs)).is_empty(),
        "a live registered reviewer in the worktree has started the review"
    );
}

/// TASK-2120: the live-session arm of `review_lease_running`. A Claude
/// reviewer still running in its review worktree after its creator shell
/// exited has started the review, with no agent-registry entry; with no live
/// session there, the lease is not running. The probes are pre-filled, so no
/// real process table is read.
// trace:TASK-1955 | ai:claude (TASK-2120)
#[test]
fn reviewer_lease_with_a_dead_creator_counts_a_live_claude_session_in_its_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let now = Utc::now();
    let worktree = root.join("aida-pr-2731");
    std::fs::create_dir_all(&worktree).unwrap();
    let mut review = lease("rv31", "PR-2731", &dead_pid(), now);
    review.role = Some("reviewer".into());
    review.review_verb = true;
    review.claim_verb = false;
    review.worktree_path = worktree.clone();
    review.active_pid = None;
    review.active_pid_start_time = None;
    let creator = dead_pid();
    review.creator_pid = Some(creator.0);
    review.creator_pid_start_time = creator.1;

    let claude = |cwd: &Path| crate::process_probe::LiveSession {
        // Not the inspecting process or its ancestors (BUG-2197 excludes those).
        pid: 4_000_001,
        cwd: cwd.to_path_buf(),
        jsonl: None,
        stale_cwd: false,
    };
    let running = |sessions: Vec<crate::process_probe::LiveSession>| {
        review_lease_running(
            &root,
            &review,
            &mut Some(sessions),
            &mut Some(Vec::new()),
            now,
        )
    };
    assert!(
        running(vec![claude(&worktree.join("src"))]),
        "a live Claude session in the review worktree has started the review"
    );
    assert!(!running(Vec::new()), "nothing live: the reviewer crashed");
    assert!(
        !running(vec![claude(&root.join("elsewhere"))]),
        "a session outside the review worktree does not count"
    );
}

/// TASK-2025 / TASK-1664: an operator notice counts as delivered only when a
/// configured notify command sent or queued it; with `[notify]` unconfigured
/// the finding stays unrouted and the tick warns that it reaches no one.
// trace:TASK-1955 | ai:claude (TASK-2025, TASK-1664)
#[test]
fn operator_notice_is_routed_only_when_delivered_and_warns_when_notify_is_missing() {
    use crate::notify::DirectNotifyOutcome;
    let outcome = |configured, sent, suppressed, pending| DirectNotifyOutcome {
        configured,
        sent,
        suppressed,
        pending,
    };
    assert!(operator_notice_delivered(&outcome(true, 1, 0, 0)));
    assert!(operator_notice_delivered(&outcome(true, 0, 0, 1)));
    assert!(!operator_notice_delivered(&outcome(true, 0, 0, 0)));
    assert!(!operator_notice_delivered(&outcome(true, 1, 1, 0)));
    assert!(!operator_notice_delivered(&outcome(false, 0, 0, 0)));

    let warning = notify_unreachable_warning(&outcome(false, 0, 0, 0), 2).unwrap();
    assert!(warning.contains("2 merge hold(s)"), "{warning}");
    assert!(warning.contains("[notify] command"), "{warning}");
    assert_eq!(notify_unreachable_warning(&outcome(true, 0, 1, 0), 2), None);
}
