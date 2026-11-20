// trace:TASK-3827 | ai:claude
use super::claims::{claim_facts, classify_claims, ClaimFact};
use super::flakes::{
    flake_recurrences, parse_attempt_jobs, parse_base_runs, read_base_failures, read_pr_failures,
    AttemptJob, BaseRun, TestFailure,
};
use super::lanes::{classify_lane_outcomes, lane_outcome_facts, FollowUps, LaneOutcomeFact};
use super::*;
use crate::ci_failures::CheckRun;
use crate::land::{derive, Evidence, Pr, Rounds};
use std::cell::RefCell;

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-11-20T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn ago(mins: i64) -> DateTime<Utc> {
    now() - Duration::minutes(mins)
}

fn facts() -> Facts {
    Facts {
        landings: Some(Ok(vec![ago(10)])),
        last_landing: Some(ago(10)),
        ..Facts::default()
    }
}

fn of_class(report: &Report, class: Class) -> Vec<&Item> {
    report
        .stuck
        .iter()
        .filter(|item| item.class == class)
        .collect()
}

// ---------------------------------------------------------------------------
// MAIN-FLAKE-RECURRENCE
// ---------------------------------------------------------------------------

fn failure(test: &str, mins_ago: i64, source: &str, flake: bool) -> TestFailure {
    TestFailure {
        test: test.into(),
        at: ago(mins_ago),
        source: source.into(),
        url: format!("https://ci.example/{}", source.replace(' ', "/")),
        flake,
    }
}

#[test]
fn a_flaky_test_that_fails_again_inside_the_window_is_a_recurrence_with_the_earlier_run_linked() {
    let policy = Policy::default();
    assert_eq!(policy.flake_window_hours, 24);
    let failures = vec![
        failure("queue::tests::drains", 600, "run 11#1", true),
        failure("queue::tests::drains", 60, "PR-7@0000abcd", false),
        // A test that failed once is not a flake, and one flake is not a
        // recurrence.
        failure("land::tests::once", 300, "run 12#1", false),
        failure("land::tests::once", 30, "PR-8@0000beef", false),
        failure("mail::tests::flaked_once", 200, "run 13#1", true),
    ];
    let items = flake_recurrences(&failures, &policy, now());
    assert_eq!(items.len(), 1, "{items:?}");
    let item = &items[0];
    assert_eq!(item.class, Class::MainFlakeRecurrence);
    assert_eq!(item.seat, "advisor");
    assert_eq!(
        item.key,
        "MAIN-FLAKE-RECURRENCE:queue::tests::drains@PR-7@0000abcd"
    );
    assert_eq!(item.age_mins, Some(60));
    // Both occurrences are linked: the earlier run and the new failure.
    assert!(
        item.summary
            .contains("run 11#1 https://ci.example/run/11#1"),
        "{}",
        item.summary
    );
    assert!(item.summary.contains("PR-7@0000abcd"), "{}", item.summary);
    assert!(item
        .action
        .contains("fix-forward for `queue::tests::drains`"));

    // The same attempt is not its own recurrence; a failure before the flake
    // is not one; nor is one more than the window after it, or one that is
    // itself older than the window.
    let none = |failures: Vec<TestFailure>| flake_recurrences(&failures, &policy, now()).is_empty();
    assert!(none(vec![failure("a::b", 60, "run 1#1", true)]));
    assert!(none(vec![
        failure("a::b", 60, "run 1#1", true),
        failure("a::b", 90, "PR-1@00000001", false),
    ]));
    assert!(none(vec![
        failure("a::b", 26 * 60, "run 1#1", true),
        failure("a::b", 60, "PR-1@00000001", false),
    ]));
    assert!(none(vec![
        failure("a::b", 40 * 60, "run 1#1", true),
        failure("a::b", 30 * 60, "run 2#1", false),
    ]));
    // The window is a setting.
    let wide = Policy {
        flake_window_hours: 48,
        ..Policy::default()
    };
    let far = vec![
        failure("a::b", 26 * 60, "run 1#1", true),
        failure("a::b", 60, "PR-1@00000001", false),
    ];
    assert_eq!(flake_recurrences(&far, &wide, now()).len(), 1);
    // The newest failure after a flake is the one reported, so a further
    // failure elsewhere is a new stall.
    let twice = vec![
        failure("a::b", 600, "run 1#1", true),
        failure("a::b", 120, "PR-1@00000001", false),
        failure("a::b", 30, "run 9#1", false),
    ];
    let items = flake_recurrences(&twice, &policy, now());
    assert_eq!(items[0].key, "MAIN-FLAKE-RECURRENCE:a::b@run 9#1");
    assert!(items[0].summary.contains("run 1#1"));
}

/// A forge that answers from tables and records every call.
#[derive(Default)]
struct Forge {
    base: Vec<BaseRun>,
    jobs: BTreeMap<(u64, u64), Vec<AttemptJob>>,
    tests: BTreeMap<u64, Vec<String>>,
    heads: BTreeMap<String, Vec<CheckRun>>,
    calls: RefCell<Vec<String>>,
}

impl Forge {
    fn calls(&self) -> Vec<String> {
        std::mem::take(&mut self.calls.borrow_mut())
    }
}

impl CiReads for Forge {
    fn check_runs(
        &self,
        rev: &str,
        _: std::time::Duration,
    ) -> std::result::Result<Vec<CheckRun>, String> {
        self.calls.borrow_mut().push(format!("checks:{rev}"));
        self.heads
            .get(rev)
            .cloned()
            .ok_or_else(|| "no such commit".to_string())
    }
    fn job_tests(
        &self,
        job: u64,
        _: std::time::Duration,
    ) -> std::result::Result<Vec<String>, String> {
        self.calls.borrow_mut().push(format!("log:{job}"));
        self.tests
            .get(&job)
            .cloned()
            .ok_or_else(|| "log expired".to_string())
    }
    fn base_runs(
        &self,
        branch: &str,
        _: std::time::Duration,
    ) -> std::result::Result<Vec<BaseRun>, String> {
        self.calls.borrow_mut().push(format!("runs:{branch}"));
        Ok(self.base.clone())
    }
    fn attempt_jobs(
        &self,
        run: u64,
        attempt: u64,
        _: std::time::Duration,
    ) -> std::result::Result<Vec<AttemptJob>, String> {
        self.calls
            .borrow_mut()
            .push(format!("jobs:{run}#{attempt}"));
        self.jobs
            .get(&(run, attempt))
            .cloned()
            .ok_or_else(|| "no such attempt".to_string())
    }
}

fn base_run(id: u64, attempt: u64, conclusion: &str, updated_mins_ago: i64) -> BaseRun {
    BaseRun {
        id,
        attempt,
        completed: true,
        conclusion: conclusion.into(),
        updated_at: Some(ago(updated_mins_ago)),
        url: format!("https://ci.example/runs/{id}"),
    }
}

fn failed_job(id: u64, finished_mins_ago: i64) -> AttemptJob {
    AttemptJob {
        id,
        conclusion: "failure".into(),
        completed_at: Some(ago(finished_mins_ago)),
    }
}

fn budget_of(elapsed: &dyn Fn() -> std::time::Duration) -> Budget<'_> {
    Budget {
        limit: DETAIL_BUDGET,
        elapsed,
    }
}

// F5: test T flakes in run A and again in run B, and both were rerun green
// before the guard looked. Each failed attempt is still a failure with its
// own identity and time, so the second is the recurrence of the first.
#[test]
fn two_flakes_of_one_test_read_in_one_pass_are_a_flake_and_its_recurrence() {
    let policy = Policy::default();
    let mut forge = Forge {
        base: vec![
            base_run(200, 2, "success", 50),
            base_run(100, 2, "success", 280),
            // Green first time: nothing to read.
            base_run(300, 1, "success", 20),
        ],
        ..Forge::default()
    };
    forge.jobs.insert((100, 1), vec![failed_job(1001, 300)]);
    forge.jobs.insert((200, 1), vec![failed_job(2001, 70)]);
    forge
        .tests
        .insert(1001, vec!["queue::tests::drains".into()]);
    forge
        .tests
        .insert(2001, vec!["queue::tests::drains".into()]);
    let none = || std::time::Duration::ZERO;
    let mut memory = Memory::default();
    let mut unknown = Vec::new();
    read_base_failures(
        &forge,
        &budget_of(&none),
        "main",
        &mut memory,
        &policy,
        now(),
        &mut unknown,
    );
    assert_eq!(unknown, Vec::<String>::new());
    assert_eq!(
        forge.calls(),
        [
            "runs:main",
            "jobs:100#1",
            "log:1001",
            "jobs:200#1",
            "log:2001"
        ]
    );
    // Both attempts are kept as failures, at the time each job failed.
    assert_eq!(memory.failures.len(), 2);
    assert!(memory.failures.iter().all(|failure| failure.flake));
    let times: Vec<_> = memory.failures.iter().map(|failure| failure.at).collect();
    assert!(times.contains(&ago(300)) && times.contains(&ago(70)));

    let report = build_report(&facts(), &policy, &mut memory, now());
    let items = of_class(&report, Class::MainFlakeRecurrence);
    assert_eq!(items.len(), 1, "{:?}", report.stuck);
    assert_eq!(
        items[0].key,
        "MAIN-FLAKE-RECURRENCE:queue::tests::drains@run 200#1"
    );
    assert!(items[0]
        .summary
        .contains("run 100#1 https://ci.example/runs/100"));
    assert!(items[0]
        .summary
        .contains("run 200#1 https://ci.example/runs/200"));
    assert_eq!(items[0].seat, "advisor");

    // An attempt read once is not read again.
    read_base_failures(
        &forge,
        &budget_of(&none),
        "main",
        &mut memory,
        &policy,
        now(),
        &mut unknown,
    );
    assert_eq!(forge.calls(), ["runs:main"]);
    assert_eq!(memory.failures.len(), 2);
}

// A run read while it was still red is a failure; once its rerun is green
// the same attempt is a flake, with no second read of its log. A log that
// could not be read is asked for again and reported meanwhile.
#[test]
fn a_red_base_run_becomes_a_flake_when_its_rerun_goes_green() {
    let policy = Policy::default();
    let mut forge = Forge {
        base: vec![base_run(100, 1, "failure", 120)],
        ..Forge::default()
    };
    forge
        .jobs
        .insert((100, 1), vec![failed_job(1001, 125), failed_job(1002, 124)]);
    forge.tests.insert(1001, vec!["a::b".into()]);
    let none = || std::time::Duration::ZERO;
    let mut memory = Memory::default();
    let mut unknown = Vec::new();
    let read = |forge: &Forge, memory: &mut Memory, unknown: &mut Vec<String>| {
        read_base_failures(
            forge,
            &budget_of(&none),
            "main",
            memory,
            &policy,
            now(),
            unknown,
        )
    };
    read(&forge, &mut memory, &mut unknown);
    assert_eq!(memory.failures.len(), 1);
    assert!(!memory.failures[0].flake);
    // Job 1002's log was unreadable: said once, and the attempt stays unread.
    assert_eq!(unknown.len(), 1, "{unknown:?}");
    assert!(unknown[0].contains("log expired"), "{unknown:?}");
    assert!(memory.seen_runs.is_empty());

    forge.tests.insert(1002, vec!["c::d".into()]);
    forge.base = vec![base_run(100, 2, "success", 60)];
    unknown.clear();
    forge.calls();
    read(&forge, &mut memory, &mut unknown);
    assert_eq!(unknown, Vec::<String>::new());
    assert_eq!(memory.failures.len(), 2);
    assert!(memory.failures.iter().all(|failure| failure.flake));
    assert!(memory.seen_runs.contains_key("run 100#1"));
    // A run still on its rerun: the attempt before it already failed.
    let mut pending = base_run(400, 2, "", 5);
    pending.completed = false;
    forge.base = vec![pending];
    forge.jobs.insert((400, 1), vec![failed_job(4001, 10)]);
    forge.tests.insert(4001, vec!["a::b".into()]);
    read(&forge, &mut memory, &mut unknown);
    let again = memory
        .failures
        .iter()
        .find(|failure| failure.source == "run 400#1")
        .expect("the failed attempt of a run being rerun");
    assert!(!again.flake);
    // a::b flaked in run 100 and failed again in run 400.
    let report = build_report(&facts(), &policy, &mut memory, now());
    let items = of_class(&report, Class::MainFlakeRecurrence);
    assert_eq!(items.len(), 1, "{:?}", report.stuck);
    assert!(items[0].key.ends_with("a::b@run 400#1"), "{}", items[0].key);
}

/// Evidence of one change whose head is red and that nobody approved.
struct Unapproved {
    reviewing: Option<String>,
    conflict: bool,
}

impl Evidence for Unapproved {
    fn conflict(&self, _: &Pr) -> std::result::Result<bool, String> {
        Ok(self.conflict)
    }
    fn reviewing(&self, _: &Pr) -> std::result::Result<Option<String>, String> {
        Ok(self.reviewing.clone())
    }
    fn ci(&self, _: &Pr) -> std::result::Result<Ci, String> {
        Ok(Ci::Red("Test (full)".into()))
    }
    fn approval(&self, _: &Pr) -> std::result::Result<Approval, String> {
        Ok(Approval::Refused("no approval covers the head".into()))
    }
    fn hold(&self, _: &Pr) -> std::result::Result<Option<Hold>, String> {
        Ok(None)
    }
    fn rounds(&self, _: &Pr) -> std::result::Result<Rounds, String> {
        Ok(Rounds::default())
    }
    fn lane(&self, _: &Pr) -> std::result::Result<Option<String>, String> {
        Ok(None)
    }
    fn head_at(&self, _: &Pr) -> Option<String> {
        Some(ago(5).to_rfc3339())
    }
}

fn unapproved_red(number: u64, evidence: &Unapproved) -> PrFact {
    let pr = Pr {
        id: number,
        head: format!("{number:040x}"),
        base: "main".into(),
        branch: Some("feature".into()),
        title: format!("change {number} (TASK-{number})"),
        updated_at: None,
        hold_label: None,
        ci: None,
    };
    let row = derive(&pr, evidence);
    pr_fact(&pr, &row, evidence)
}

// F4: a known base-branch flake fails on a change nobody approved. Red
// detail is read for approved heads only, so the recurrence reader takes
// every red head: unreviewed, under review or conflicting.
#[test]
fn a_known_flake_failing_on_an_unapproved_change_is_a_recurrence() {
    let policy = Policy::default();
    let prs = vec![
        unapproved_red(
            7,
            &Unapproved {
                reviewing: None,
                conflict: false,
            },
        ),
        unapproved_red(
            8,
            &Unapproved {
                reviewing: Some("reviewer@dell1".into()),
                conflict: false,
            },
        ),
        unapproved_red(
            9,
            &Unapproved {
                reviewing: None,
                conflict: true,
            },
        ),
    ];
    for fact in &prs {
        assert!(fact.head_red, "#{} {:?}", fact.number, fact.standing);
        assert!(!fact.wants_red_detail());
    }
    let heads: Vec<(String, String)> = prs
        .iter()
        .filter(|fact| fact.head_red)
        .map(|fact| (rerun_key(fact.number, &fact.head), fact.head.clone()))
        .collect();
    let run = |id: u64, mins_ago: i64| CheckRun {
        id,
        name: "Test (full)".into(),
        status: "completed".into(),
        conclusion: "failure".into(),
        html_url: format!("https://ci.example/job/{id}"),
        completed_at: ago(mins_ago).to_rfc3339(),
    };
    let mut forge = Forge::default();
    for (index, (_, head)) in heads.iter().enumerate() {
        let id = 70 + index as u64;
        forge.heads.insert(head.clone(), vec![run(id, 30)]);
        forge.tests.insert(
            id,
            vec![if index == 0 {
                "queue::tests::drains".to_string()
            } else {
                format!("other::tests::t{index}")
            }],
        );
    }
    let none = || std::time::Duration::ZERO;
    let mut unknown = Vec::new();

    // With no failure on the base branch nothing can recur: nothing is read.
    let mut memory = Memory::default();
    read_pr_failures(
        &forge,
        &budget_of(&none),
        &heads,
        &mut memory,
        &policy,
        now(),
        &mut unknown,
    );
    assert_eq!(forge.calls(), Vec::<String>::new());

    memory
        .failures
        .push(failure("queue::tests::drains", 600, "run 11#1", true));
    read_pr_failures(
        &forge,
        &budget_of(&none),
        &heads,
        &mut memory,
        &policy,
        now(),
        &mut unknown,
    );
    assert_eq!(unknown, Vec::<String>::new());
    assert_eq!(forge.calls().len(), 6, "one listing and one log per head");
    let report = build_report(
        &Facts {
            prs: prs.clone(),
            ..facts()
        },
        &policy,
        &mut memory,
        now(),
    );
    let items = of_class(&report, Class::MainFlakeRecurrence);
    assert_eq!(items.len(), 1, "{:?}", report.stuck);
    assert_eq!(
        items[0].key,
        "MAIN-FLAKE-RECURRENCE:queue::tests::drains@PR-7@00000000"
    );
    assert_eq!(items[0].age_mins, Some(30), "the time the check failed");
    assert!(items[0].summary.contains("https://ci.example/job/70"));

    // Each head is read once; a head that is gone is forgotten.
    read_pr_failures(
        &forge,
        &budget_of(&none),
        &heads,
        &mut memory,
        &policy,
        now(),
        &mut unknown,
    );
    assert_eq!(forge.calls(), Vec::<String>::new());
    read_pr_failures(
        &forge,
        &budget_of(&none),
        &heads[..1],
        &mut memory,
        &policy,
        now(),
        &mut unknown,
    );
    assert_eq!(memory.pr_failures_read.len(), 1);

    // A log the approved-red reader already kept is not downloaded again,
    // and only max_red_logs heads are read per run: the rest are said to wait.
    let tight = Policy {
        max_red_logs: 1,
        ..Policy::default()
    };
    let mut memory = Memory::default();
    memory
        .failures
        .push(failure("queue::tests::drains", 600, "run 11#1", true));
    memory.red_detail.insert(
        heads[0].0.clone(),
        RedDetail {
            at: now(),
            checks: vec![FailedCheck {
                name: "Test (full)".into(),
                id: 70,
                tests: Ok(vec!["queue::tests::drains".into()]),
                base: BaseCheck::default(),
            }],
        },
    );
    read_pr_failures(
        &forge,
        &budget_of(&none),
        &heads,
        &mut memory,
        &tight,
        now(),
        &mut unknown,
    );
    assert_eq!(forge.calls(), [format!("checks:{}", heads[0].1)]);
    assert_eq!(unknown.len(), 1, "{unknown:?}");
    assert!(unknown[0].contains("2 not read on this run"), "{unknown:?}");
}

#[test]
fn base_runs_and_attempt_jobs_are_read_from_the_forges_json() {
    let runs = parse_base_runs(
        r#"[
          {"databaseId": 44779869955, "attempt": 2, "conclusion": "success", "status": "completed",
           "updatedAt": "2026-11-20T10:00:00Z", "url": "https://github.com/o/r/actions/runs/44779869955"},
          {"databaseId": 5, "attempt": 1, "conclusion": "", "status": "in_progress",
           "updatedAt": "2026-11-20T11:00:00Z", "url": ""},
          {"attempt": 1}
        ]"#,
    )
    .unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!((runs[0].id, runs[0].attempt), (44779869955, 2));
    assert!(runs[0].completed && runs[0].conclusion == "success");
    assert_eq!(runs[0].updated_at, Some(ago(120)));
    assert!(!runs[1].completed);
    assert!(parse_base_runs("{}").is_err());

    let jobs = parse_attempt_jobs(
        r#"{"total_count": 2, "jobs": [
          {"id": 136439424371, "conclusion": "failure", "completed_at": "2026-11-20T09:30:00Z"},
          {"id": 2, "conclusion": "success", "completed_at": null}
        ]}"#,
    )
    .unwrap();
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].id, 136439424371);
    assert_eq!(jobs[0].completed_at, Some(ago(150)));
    assert_eq!(jobs[1].completed_at, None);
    assert!(parse_attempt_jobs("[]").is_err());
}

// ---------------------------------------------------------------------------
// LANE-OUTCOME
// ---------------------------------------------------------------------------

fn outcome(attempt: &str, spec: &str, result: &'static str, mins_ago: i64) -> LaneOutcomeFact {
    LaneOutcomeFact {
        attempt: attempt.into(),
        spec: spec.into(),
        result,
        reason_code: "fast-gate".into(),
        detail: "fast gate needs a clean\ncommitted seat".into(),
        finished_at: ago(mins_ago),
        launched_by: None,
        followed_up: None,
    }
}

#[test]
fn a_lane_exit_nobody_followed_up_routes_to_the_seat_that_launched_it() {
    let policy = Policy::default();
    assert_eq!(policy.lane_outcome_mins, 20);
    let mut by_advisor = outcome("a2", "TASK-2", "punted", 45);
    by_advisor.launched_by = Some(" Advisor ".into());
    let mut by_operator = outcome("a3", "TASK-3", "no-change-with-reason", 45);
    by_operator.launched_by = Some("operator".into());
    let mut odd_seat = outcome("a4", "TASK-4", "refused-before-start", 45);
    odd_seat.launched_by = Some("seat; rm -rf".into());
    let mut followed = outcome("a5", "TASK-5", "punted", 45);
    followed.followed_up = Some("queued for advisor at 2026-11-20 11:30 UTC".into());
    let outcomes = vec![
        outcome("a1", "TASK-1", "failed-after-start", 45),
        by_advisor,
        by_operator,
        odd_seat,
        followed,
        // Inside the wait, and past the horizon.
        outcome("a6", "TASK-6", "failed-after-start", 20),
        outcome("a7", "TASK-7", "failed-after-start", 25 * 60),
    ];
    let items = classify_lane_outcomes(&outcomes, &policy, now());
    let seats: Vec<(&str, &str)> = items
        .iter()
        .map(|item| (item.key.as_str(), item.seat.as_str()))
        .collect();
    assert_eq!(
        seats,
        [
            ("LANE-OUTCOME:a1", "product"),
            ("LANE-OUTCOME:a2", "advisor"),
            ("LANE-OUTCOME:a3", "product"),
            ("LANE-OUTCOME:a4", "product"),
        ]
    );
    // The exit line and the reason, on one line.
    let item = &items[0];
    assert_eq!(item.class, Class::LaneOutcome);
    assert_eq!(item.age_mins, Some(45));
    assert!(
        item.summary.contains(
            "the lane for TASK-1 exited `failed-after-start reason=fast-gate` (fast gate needs a clean committed seat)"
        ),
        "{}",
        item.summary
    );
    assert!(item.action.contains("relaunch the lane for TASK-1"));
    // The wait is a setting.
    let patient = Policy {
        lane_outcome_mins: 60,
        ..Policy::default()
    };
    assert!(classify_lane_outcomes(&outcomes, &patient, now()).is_empty());
}

#[test]
fn only_what_is_dated_after_the_exit_or_holds_the_spec_now_is_a_follow_up() {
    let mut follow = FollowUps::default();
    follow
        .dated
        .push(("k1".into(), ago(50), "queued for implementer".into()));
    follow
        .dated
        .push(("k1".into(), ago(10), "queued for advisor".into()));
    follow
        .dated
        .push(("k1".into(), ago(5), "requeued (edit)".into()));
    follow.live.push(("k2".into(), "lease abc holds it".into()));
    follow.closed.insert("k3".into(), "Completed".into());
    // The queue entry from before the exit is not a follow-up; the first
    // one after it is.
    assert_eq!(
        follow.after("k1", ago(30)).as_deref(),
        Some("queued for advisor at 2026-11-20 11:50 UTC")
    );
    assert_eq!(follow.after("k1", ago(5)), None);
    assert_eq!(
        follow.after("k2", ago(30)).as_deref(),
        Some("lease abc holds it")
    );
    assert_eq!(
        follow.after("k3", ago(30)).as_deref(),
        Some("the spec is Completed")
    );
    assert_eq!(follow.after("k4", ago(30)), None);
}

fn git_init(root: &Path) {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(root)
        .args(["init", "-q", "-b", "main"])
        .output()
        .unwrap();
    assert!(out.status.success());
}

fn spec(store: &mut aida_core::RequirementsStore, id: &str) -> uuid::Uuid {
    let mut requirement = aida_core::Requirement::new(format!("spec {id}"), String::new());
    requirement.spec_id = Some(id.into());
    let uuid = requirement.id;
    store.requirements.push(requirement);
    uuid
}

fn finished(root: &Path, spec: &str, kind: crate::lane_result::Kind) -> DateTime<Utc> {
    use crate::lane_result::{Attempt, Kind};
    let attempt = Attempt::begin(root, spec, None).unwrap();
    if kind != Kind::RefusedBeforeStart {
        attempt.started(1);
    }
    attempt
        .finish(kind, "reason-code", "what happened", None, None, true)
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(3));
    Utc::now()
}

// F6: the production loader reads the queue and the routing events. A punt
// that was queued for the advisor, and a failure that was requeued, are
// followed up although no lane was relaunched; an exit with only an older
// queue entry is not.
#[test]
fn the_lane_loader_counts_queue_entries_and_routing_events_after_the_exit_as_follow_up() {
    use crate::lane_result::Kind;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init(root);
    let store_root = root.join(".aida-store");
    let mut store = aida_core::RequirementsStore::new();
    let queued = spec(&mut store, "TASK-1");
    spec(&mut store, "TASK-2");
    let stale_queue = spec(&mut store, "TASK-3");
    let closed = spec(&mut store, "TASK-4");
    for id in ["TASK-5", "TASK-6", "TASK-7", "TASK-8"] {
        spec(&mut store, id);
    }
    store
        .requirements
        .iter_mut()
        .find(|requirement| requirement.id == closed)
        .unwrap()
        .status = aida_core::RequirementStatus::Completed;

    let started = Utc::now();
    finished(root, "TASK-1", Kind::Punted);
    finished(root, "TASK-2", Kind::FailedAfterStart);
    finished(root, "TASK-3", Kind::RefusedBeforeStart);
    finished(root, "TASK-4", Kind::NoChangeWithReason);
    finished(root, "TASK-5", Kind::Held);
    finished(root, "TASK-6", Kind::FailedAfterStart);
    // A relaunch of TASK-6 that is still running.
    let _running = crate::lane_result::Attempt::begin(root, "task-6", None).unwrap();
    finished(root, "TASK-7", Kind::FailedAfterStart);
    let after = finished(root, "TASK-8", Kind::Punted) + Duration::seconds(1);

    let entry = |requirement_id, added_at, role: &str| aida_core::QueueEntry {
        user_id: "u".into(),
        requirement_id,
        position: 1000,
        added_by: "u".into(),
        note: None,
        added_at,
        for_role: Some(role.into()),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    };
    let queues = store_root.join("registry/queues");
    std::fs::create_dir_all(&queues).unwrap();
    std::fs::write(
        queues.join("u.yaml"),
        serde_yaml::to_string(&vec![
            entry(queued, after, "advisor"),
            entry(stale_queue, started - Duration::hours(1), "implementer"),
        ])
        .unwrap(),
    )
    .unwrap();
    let requeued = Event {
        ts: after,
        ..Event::new(
            Some("TASK-2".into()),
            "",
            EventKind::SpecRequeued {
                via: "queue-rework".into(),
                actor: None,
                from: "Needs Attention".into(),
                to: "Approved".into(),
                cleared_tags: Vec::new(),
                kept_tags: Vec::new(),
            },
        )
    };
    // An event that routes nothing.
    let noise = Event {
        ts: after,
        ..Event::new(Some("TASK-8".into()), "", EventKind::RunStarted)
    };
    let log = events::events_path(root);
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    std::fs::write(
        &log,
        format!(
            "{}\n{}\n",
            serde_json::to_string(&requeued).unwrap(),
            serde_json::to_string(&noise).unwrap()
        ),
    )
    .unwrap();

    let later = after + Duration::minutes(30);
    let mut unknown = Vec::new();
    let local = vec![("task-7".to_string(), "lease abc holds it".to_string())];
    let outcomes = lane_outcome_facts(root, &store_root, &store, &local, &[], later, &mut unknown);
    assert_eq!(unknown, Vec::<String>::new());
    let follow = |spec: &str| {
        outcomes
            .iter()
            .find(|outcome| outcome.spec == spec)
            .unwrap_or_else(|| panic!("no outcome for {spec}"))
            .followed_up
            .clone()
    };
    // A held lane asks for nothing.
    assert!(outcomes.iter().all(|outcome| outcome.spec != "TASK-5"));
    assert_eq!(outcomes.len(), 7);
    assert!(follow("TASK-1")
        .unwrap()
        .starts_with("queued for advisor at"));
    assert!(follow("TASK-2")
        .unwrap()
        .starts_with("requeued (queue-rework) at"));
    assert_eq!(follow("TASK-3"), None, "queued before the exit only");
    assert_eq!(follow("TASK-4").as_deref(), Some("the spec is Completed"));
    assert!(follow("TASK-6").unwrap().contains("was launched at"));
    assert_eq!(follow("TASK-7").as_deref(), Some("lease abc holds it"));
    assert_eq!(follow("TASK-8"), None);

    // Only the two exits nothing followed are stalls.
    let report = build_report(
        &Facts {
            lane_outcomes: outcomes,
            ..Facts::default()
        },
        &Policy::default(),
        &mut Memory::default(),
        later,
    );
    let items = of_class(&report, Class::LaneOutcome);
    assert_eq!(items.len(), 2, "{:?}", report.stuck);
    let said: Vec<&str> = items.iter().map(|item| item.summary.as_str()).collect();
    assert!(
        said.iter().any(|summary| summary
            .contains("the lane for TASK-3 exited `refused-before-start reason=reason-code`")),
        "{said:?}"
    );
    assert!(
        said.iter()
            .any(|summary| summary.contains("the lane for TASK-8 exited `punted")),
        "{said:?}"
    );
}

// ---------------------------------------------------------------------------
// STALE-CLAIM
// ---------------------------------------------------------------------------

fn dead_claim(mins: i64, remote: bool) -> ClaimFact {
    ClaimFact {
        scope: "TASK-1".into(),
        record: PathBuf::from("/store/coordination/leases/task-1.toml"),
        generation: "2026-11-01T00:00:00Z".into(),
        evidence: "heartbeat 9000s old, past its 1800s TTL".into(),
        blocks: Some("implementer admission for TASK-1 refuses while it stands".into()),
        unhealable: Some("the record carries no claim id".into()),
        stale_mins: mins,
        remote,
    }
}

#[test]
fn only_a_blocking_dead_claim_the_doctor_cannot_heal_is_routed_with_its_record_and_evidence() {
    let policy = Policy::default();
    assert_eq!(
        (policy.stale_claim_mins, policy.stale_claim_remote_mins),
        (60, 720)
    );
    let healable = ClaimFact {
        unhealable: None,
        ..dead_claim(90, false)
    };
    let blocks_nothing = ClaimFact {
        blocks: None,
        ..dead_claim(90, false)
    };
    let claims = vec![
        dead_claim(90, false),
        healable,
        blocks_nothing,
        dead_claim(60, false),
        // Another host's holder cannot be looked at: a longer wait.
        dead_claim(90, true),
    ];
    let items = classify_claims(&claims, &policy);
    assert_eq!(items.len(), 1, "{items:?}");
    let item = &items[0];
    assert_eq!(item.class, Class::StaleClaim);
    assert_eq!(item.seat, "advisor");
    assert_eq!(item.age_mins, Some(90));
    assert_eq!(
        item.key,
        "STALE-CLAIM:/store/coordination/leases/task-1.toml@2026-11-01T00:00:00Z"
    );
    for part in [
        "record /store/coordination/leases/task-1.toml",
        "heartbeat 9000s old, past its 1800s TTL",
        "`aida doctor heal stale-leases` does not heal it: the record carries no claim id",
        "implementer admission for TASK-1 refuses",
    ] {
        assert!(item.summary.contains(part), "{part}: {}", item.summary);
    }
    // No age is too old: a claim that still blocks is reported.
    assert_eq!(
        classify_claims(&[dead_claim(90 * 24 * 60, false)], &policy).len(),
        1
    );
    assert_eq!(classify_claims(&[dead_claim(721, true)], &policy).len(), 1);
    let patient = Policy {
        stale_claim_mins: 120,
        ..Policy::default()
    };
    assert!(classify_claims(&claims, &patient).is_empty());
}

fn shared_claim(scope: &str, host: &str, heartbeat: DateTime<Utc>) -> crate::coordination::Claim {
    serde_json::from_value(serde_json::json!({
        "scope": scope,
        "node_id": "node-1",
        "clone_path": "/home/other/aida",
        "host": host,
        "pid": 4242,
        "agent": "implementer",
        "started_at": heartbeat.to_rfc3339(),
        "heartbeat_at": heartbeat.to_rfc3339(),
        "ttl_secs": 1800,
    }))
    .unwrap()
}

fn write_claim(path: &Path, claim: &crate::coordination::Claim) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, toml::to_string_pretty(claim).unwrap()).unwrap();
}

// F7: the production inventory. An implementer claim still blocks however
// long ago it expired, so a month-old one on an open spec is reported; the
// drain and solo locks are read too, with the path each is recorded at; a
// lease of this clone whose holder nobody can establish is reported as well.
#[test]
fn the_claim_loader_keeps_old_blockers_and_reads_locks_and_unknown_leases() {
    use crate::coordination::{claim_path, hostname, lock_claim_path, LockKind};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init(root);
    let store_root = root.join(".aida-store");
    let mut store = aida_core::RequirementsStore::new();
    let done = spec(&mut store, "TASK-2");
    for id in ["TASK-1", "TASK-3", "TASK-5"] {
        spec(&mut store, id);
    }
    store
        .requirements
        .iter_mut()
        .find(|requirement| requirement.id == done)
        .unwrap()
        .status = aida_core::RequirementStatus::Completed;
    let here = hostname();
    let now = Utc::now();
    let month = now - Duration::days(30);
    let write = |claim: crate::coordination::Claim| {
        write_claim(&claim_path(&store_root, &claim.scope), &claim)
    };
    // Expired a month ago, on an open spec: admission still refuses it.
    write(shared_claim("TASK-1", &here, month));
    // The same on a completed spec blocks nothing.
    write(shared_claim("TASK-2", &here, month));
    // Another host's, expired two hours ago.
    let mut elsewhere = shared_claim("TASK-3", "elsewhere", now - Duration::hours(2));
    elsewhere.claim_id = Some("claim-3".into());
    write(elsewhere);
    // A reviewer's expired claim is reclaimed by age.
    let mut review = shared_claim("PR-9", &here, month);
    review.review_verb = true;
    write(review);
    // Still inside its TTL: not dead.
    write(shared_claim("TASK-5", &here, now - Duration::minutes(5)));

    // A drain lock of an older build, with no process identity: its holder
    // cannot be shown to run, and the lock's admission refuses another clone
    // until its long TTL passes.
    let namespace = crate::agent_registry::current_pid_namespace();
    let mut lock = shared_claim("drain", &here, now - Duration::hours(2));
    lock.process_backed = true;
    lock.pid = std::process::id();
    lock.pid_namespace = namespace;
    lock.ttl_secs = 86_400;
    lock.agent = "queue work --auto-complete TASK-9".into();
    let lock_path = lock_claim_path(&store_root, LockKind::Drain);
    write_claim(&lock_path, &lock);
    // A solo lock past its TTL is reclaimed by the next solo loop.
    let mut solo = shared_claim("solo loop", &here, month);
    solo.process_backed = true;
    write_claim(&lock_claim_path(&store_root, LockKind::Solo), &solo);

    // A lease of this clone stamped in another PID namespace, silent for
    // two hours: neither reader can say whether its holder runs.
    let lease: crate::SessionLease = serde_json::from_value(serde_json::json!({
        "id": "lease-unknown-1", "scope": "TASK-1", "slug": "lane",
        "owner": "tester", "worktree_path": root, "branch": "task-1-lane",
        "started_at": (now - Duration::hours(2)).to_rfc3339(),
        "hostname": here, "role": "implementer",
        "pid_namespace": namespace.map_or(1, |ns| ns ^ 1),
    }))
    .unwrap();
    let lease_file = crate::leases::lease_path(root, &lease.id);
    std::fs::create_dir_all(lease_file.parent().unwrap()).unwrap();
    std::fs::write(&lease_file, toml::to_string(&lease).unwrap()).unwrap();

    let mut unknown = Vec::new();
    let facts = claim_facts(root, &store_root, &store, now, &mut unknown);
    assert_eq!(unknown, Vec::<String>::new());
    let of = |scope: &str| {
        facts
            .iter()
            .find(|fact| fact.scope == scope && fact.record.starts_with(&store_root))
            .unwrap_or_else(|| panic!("no fact for {scope}: {facts:?}"))
    };
    let old = of("TASK-1");
    assert_eq!(old.record, claim_path(&store_root, "TASK-1"));
    assert!(old.stale_mins > 29 * 24 * 60, "{}", old.stale_mins);
    assert!(old.blocks.is_some() && old.unhealable.is_some(), "{old:?}");
    assert!(old.evidence.contains("past its 1800s TTL"), "{old:?}");
    assert!(old.evidence.contains("names no lease"), "{old:?}");
    assert_eq!(of("TASK-2").blocks, None);
    let foreign = of("TASK-3");
    assert!(foreign.remote && foreign.blocks.is_some());
    assert!(
        foreign
            .unhealable
            .as_deref()
            .unwrap()
            .contains("recorded on host `elsewhere`"),
        "{foreign:?}"
    );
    assert!(facts.iter().all(|fact| fact.scope != "PR-9"));
    assert!(facts.iter().all(|fact| fact.scope != "TASK-5"));
    assert!(
        facts.iter().all(|fact| fact.scope != "the solo loop lock")
            || of("the solo loop lock").blocks.is_none(),
        "an expired lock blocks nothing: {facts:?}"
    );

    let mut expected = vec![claim_path(&store_root, "TASK-1")];
    // The lock is judged by pid only where this process can see the pid
    // namespace it was stamped in.
    if aida_core::pid_observer::PidObserver::current().can_observe(namespace) {
        let drain = of("the drain lock");
        assert_eq!(drain.record, lock_path);
        assert!(drain.blocks.is_some(), "{drain:?}");
        assert!(
            drain
                .unhealable
                .as_deref()
                .unwrap()
                .contains("process lock"),
            "{drain:?}"
        );
        assert!(drain.evidence.contains("queue work --auto-complete TASK-9"));
        expected.push(lock_path.clone());
    }
    let unknown_lease = facts
        .iter()
        .find(|fact| fact.record == lease_file)
        .unwrap_or_else(|| panic!("no fact for the lease: {facts:?}"));
    assert_eq!(unknown_lease.stale_mins, 120);
    assert!(
        unknown_lease.evidence.contains("its holder is unknown"),
        "{unknown_lease:?}"
    );
    assert!(unknown_lease.blocks.is_some() && unknown_lease.unhealable.is_some());
    expected.push(lease_file.clone());

    // Routed: the old blocker, the lock and the lease. The other host's
    // claim waits for the longer bar; the rest block nothing.
    let mut routed: Vec<PathBuf> = Vec::new();
    for item in classify_claims(&facts, &Policy::default()) {
        let fact = facts
            .iter()
            .find(|fact| item.key.contains(&fact.record.display().to_string()))
            .unwrap();
        assert!(
            item.summary
                .contains(&format!("record {}", fact.record.display())),
            "{}",
            item.summary
        );
        routed.push(fact.record.clone());
    }
    routed.sort();
    expected.sort();
    assert_eq!(routed, expected);
}

// ---------------------------------------------------------------------------
// The report, routing and settings
// ---------------------------------------------------------------------------

#[test]
fn the_three_classes_are_reported_and_each_goes_to_its_seat_once() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let store = root.join(".aida-store");
    let policy = Policy::default();
    let mut memory = Memory::default();
    memory.failures = vec![
        failure("queue::tests::drains", 600, "run 11#1", true),
        failure("queue::tests::drains", 60, "run 12#1", false),
    ];
    let stalls = Facts {
        lane_outcomes: vec![outcome("a1", "TASK-1", "punted", 45)],
        claims: vec![dead_claim(90, false)],
        ..facts()
    };
    let report = build_report(&stalls, &policy, &mut memory, now());
    let classes: Vec<&str> = report.stuck.iter().map(|item| item.class.label()).collect();
    assert_eq!(
        classes,
        ["MAIN-FLAKE-RECURRENCE", "LANE-OUTCOME", "STALE-CLAIM"]
    );
    let text = render(&report);
    for label in ["[MAIN-FLAKE-RECURRENCE]", "[LANE-OUTCOME]", "[STALE-CLAIM]"] {
        assert!(text.contains(label), "{label}: {text}");
    }
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["stuck"][1]["class"], "LANE-OUTCOME");
    assert_eq!(json["stuck"][1]["seat"], "product");

    route(root, &report, &mut memory, &policy, now());
    let due = |seat: &str| {
        schedule_ledger::load(&store, &route_job_name(seat))
            .and_then(|ledger| ledger.note)
            .unwrap_or_default()
    };
    assert!(due("advisor").contains("[MAIN-FLAKE-RECURRENCE]"));
    assert!(due("advisor").contains("[STALE-CLAIM]"));
    assert!(!due("advisor").contains("[LANE-OUTCOME]"));
    assert!(due("product").contains("[LANE-OUTCOME] the lane for TASK-1 exited `punted"));
    assert_eq!(
        known_route_seat(&store, "throughput-guard-route-product"),
        Some("product")
    );
    assert!(route_prompt("product").contains("relaunch or route a lane"));
    assert_eq!(memory.routed.len(), 3);
    // The same stalls on the next run wake nobody.
    let plan = plan_routes(&report, &memory, &policy, now() + Duration::minutes(20));
    assert!(plan.seats.is_empty());
    // Failures older than twice the window are forgotten.
    let much_later = now() + Duration::hours(60);
    build_report(&facts(), &policy, &mut memory, much_later);
    assert!(memory.failures.is_empty());
}

#[test]
fn the_new_waits_and_the_probed_hosts_are_project_settings() {
    let default = Policy::default();
    assert!(default.remote_hosts.is_empty());
    let cfg: toml::Value = toml::from_str(
        r#"
[throughput_guard]
lane_outcome_mins = 5
stale_claim_mins = 15
stale_claim_remote_mins = 0
flake_window_hours = 48

[throughput_guard.remote_hosts]
disrad-x = "dell1"
" " = "skipped"
other = 7
"#,
    )
    .unwrap();
    let policy = Policy::from_config(Some(&cfg));
    assert!(!policy.enabled, "the guard still ships off");
    assert_eq!(policy.lane_outcome_mins, 5);
    assert_eq!(policy.stale_claim_mins, 15);
    assert_eq!(policy.flake_window_hours, 48);
    assert_eq!(
        policy.stale_claim_remote_mins,
        default.stale_claim_remote_mins
    );
    assert_eq!(
        policy.remote_hosts,
        BTreeMap::from([("disrad-x".to_string(), "dell1".to_string())])
    );
}

// AC2 as amended: another lane host's memory is measured by a bounded probe
// of the host the project named, and is unknown with the reason otherwise.
#[test]
fn a_configured_lane_host_is_probed_for_its_memory_and_a_failed_probe_says_why() {
    let policy = Policy {
        remote_hosts: BTreeMap::from([
            ("disrad-x".to_string(), "dell1".to_string()),
            ("quiet-host".to_string(), "quiet".to_string()),
            ("down-host".to_string(), "down".to_string()),
        ]),
        ..Policy::default()
    };
    let probed = RefCell::new(Vec::new());
    let probe = |alias: &str| -> std::result::Result<String, String> {
        probed.borrow_mut().push(alias.to_string());
        match alias {
            "dell1" => Ok(
                "MemTotal:       16777216 kB\nMemFree:         1000 kB\nMemAvailable:    3145728 kB\n"
                    .into(),
            ),
            "quiet" => Ok("MemTotal: 8388608 kB\nMemAvailable: 4194304 kB\n".into()),
            _ => Err("ssh exited with exit status: 255\nsecond line".into()),
        }
    };
    let local = || "20.0 GiB available of 64.0 GiB".to_string();
    let lines = host_lines_with(
        "imac",
        2,
        &["disrad-x", "disrad-x", "unnamed-host"],
        &policy,
        &local,
        &probe,
    );
    let line = |host: &str| lines.iter().find(|line| line.host == host).unwrap();
    assert_eq!(line("imac").memory, "20.0 GiB available of 64.0 GiB");
    assert_eq!(
        (line("disrad-x").lanes, line("disrad-x").memory.as_str()),
        (2, "3.0 GiB available of 16.0 GiB")
    );
    // A named host with no live lane known here is still measured.
    assert_eq!(
        (line("quiet-host").lanes, line("quiet-host").memory.as_str()),
        (0, "4.0 GiB available of 8.0 GiB")
    );
    assert_eq!(
        line("down-host").memory,
        "unknown (the probe of `down` failed: ssh exited with exit status: 255 second line)"
    );
    // A host the project did not name is not contacted.
    assert_eq!(line("unnamed-host").memory, NO_REMOTE_MEMORY);
    let mut asked = probed.borrow().clone();
    asked.sort();
    assert_eq!(asked, ["dell1", "down", "quiet"]);
    // A probe that answers without the figures is unknown too.
    let empty = |_: &str| -> std::result::Result<String, String> { Ok("garbage".into()) };
    assert_eq!(
        remote_memory(&policy, "disrad-x", &empty).unwrap(),
        "unknown (the probe of `dell1` returned no memory figures)"
    );
    assert_eq!(memory_of("MemAvailable: 1 kB"), None);
    // The production probe refuses anything but a host alias before SSH
    // runs, and never runs in a unit test.
    let refused = crate::doctor_cmd::lane_binaries::meminfo(
        "-oProxyCommand=id",
        std::time::Duration::from_secs(1),
    )
    .unwrap_err();
    assert!(format!("{refused:#}").contains("not an SSH host alias"));
    assert!(ssh_meminfo("dell1").is_err());
}
