// trace:STORY-2906 | ai:claude
use super::*;

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-11-20T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn ago(mins: i64) -> DateTime<Utc> {
    now() - Duration::minutes(mins)
}

fn pr(number: u64, state: State, action: Action, waited_mins: i64) -> PrFact {
    PrFact {
        number,
        title: format!("change {number} (TASK-{number})"),
        head: format!("{number:040x}"),
        state,
        action,
        actor: None,
        reason: "approved at this head".into(),
        ci: Some("green".into()),
        since: Some(ago(waited_mins)),
        approval: None,
        red: None,
    }
}

fn red(tests: &[&str], changed: &[&str], base: BaseChecks) -> RedFact {
    RedFact {
        failed_checks: vec!["Test (full)".into()],
        check_ids: vec![11],
        failing_tests: tests.iter().map(|t| t.to_string()).collect(),
        changed_paths: Ok(changed.iter().map(|p| p.to_string()).collect()),
        base,
        rerun_routed: None,
        unread: None,
    }
}

fn red_pr(number: u64, red: RedFact) -> PrFact {
    PrFact {
        ci: Some("red: Test (full)".into()),
        approval: Some("approved at this head".into()),
        red: Some(red),
        ..pr(number, State::CiFailed, Action::DispatchRework, 45)
    }
}

fn class_of(fact: &PrFact, policy: &Policy) -> Option<Class> {
    classify_pr(fact, policy, now(), &[]).map(|item| item.class)
}

fn facts(prs: Vec<PrFact>) -> Facts {
    Facts {
        prs,
        landings: Some(Ok(vec![ago(10)])),
        last_landing: Some(ago(10)),
        ..Facts::default()
    }
}

fn event(test: &str, mins_ago: i64, source: &str) -> TestEvent {
    TestEvent {
        test: test.into(),
        at: ago(mins_ago),
        source: source.into(),
        url: format!("https://forge.example/{source}"),
    }
}

#[test]
fn each_reconciler_state_maps_to_its_stall_class_past_the_threshold() {
    let policy = Policy::default();
    let cases = [
        (State::Ready, Action::Train, Class::StuckMerge),
        (State::Ready, Action::ClearHoldAndLand, Class::StuckMerge),
        (State::NeedsReview, Action::LaunchReview, Class::StuckReview),
        (
            State::ChangesRequested,
            Action::DispatchRework,
            Class::StuckRework,
        ),
        (
            State::Conflict,
            Action::DispatchRework,
            Class::StuckConflict,
        ),
    ];
    for (state, action, class) in cases {
        let stuck = pr(7, state.clone(), action.clone(), 45);
        let item = classify_pr(&stuck, &policy, now(), &[]).expect("past the threshold");
        assert_eq!(item.class, class);
        assert_eq!(item.seat, "advisor");
        assert_eq!(item.pr, Some(7));
        assert_eq!(item.age_mins, Some(45));
        assert!(item.key.starts_with(&format!("{}:PR-7@", class.label())));
        // Inside the threshold the same change is moving.
        assert_eq!(class_of(&pr(7, state, action, 5), &policy), None);
    }
}

#[test]
fn a_change_someone_is_working_or_that_is_not_stalled_is_moving() {
    let policy = Policy::default();
    // A lane holds the rework or the conflict.
    for state in [State::ChangesRequested, State::Conflict] {
        let mut held = pr(3, state, Action::Wait, 600);
        held.actor = Some("lane implementer-1".into());
        assert_eq!(class_of(&held, &policy), None);
    }
    // A review is running, a train is landing, a stacked change waits on its base.
    for (state, action) in [
        (State::Reviewing, Action::Wait),
        (State::Landing, Action::Wait),
        (State::Ready, Action::Wait),
        (State::Unknown, Action::Wait),
        (State::Blocked, Action::AdvisorRuling),
    ] {
        assert_eq!(class_of(&pr(3, state, action, 600), &policy), None);
    }
    // How long it has waited is unknown: never guessed to be past a threshold.
    let mut unknown_age = pr(3, State::Ready, Action::Train, 600);
    unknown_age.since = None;
    assert_eq!(class_of(&unknown_age, &policy), None);
}

#[test]
fn thresholds_are_project_settings_and_the_guard_ships_off() {
    assert!(!Policy::default().enabled);
    assert!(!Policy::from_config(None).enabled);
    let cfg: toml::Value = toml::from_str(
        r#"
        [throughput_guard]
        enabled = true
        merge_mins = 5
        review_mins = 90
        rework_mins = 0      # not positive: the default stays
        conflict_mins = "x"  # not an integer: the default stays
        escalate_after_mins = 240
        "#,
    )
    .unwrap();
    let policy = Policy::from_config(Some(&cfg));
    assert!(policy.enabled);
    assert_eq!(policy.merge_mins, 5);
    assert_eq!(policy.review_mins, 90);
    assert_eq!(policy.rework_mins, Policy::default().rework_mins);
    assert_eq!(policy.conflict_mins, Policy::default().conflict_mins);
    assert_eq!(policy.escalate_after_mins, 240);
    // The same waits classify differently under the project's thresholds.
    let ready = pr(1, State::Ready, Action::Train, 10);
    assert_eq!(class_of(&ready, &Policy::default()), None);
    assert_eq!(class_of(&ready, &policy), Some(Class::StuckMerge));
    let unreviewed = pr(2, State::NeedsReview, Action::LaunchReview, 45);
    assert_eq!(
        class_of(&unreviewed, &Policy::default()),
        Some(Class::StuckReview)
    );
    assert_eq!(class_of(&unreviewed, &policy), None);
    // `enabled` must be the boolean true.
    let off: toml::Value = toml::from_str("[throughput_guard]\nenabled = \"true\"\n").unwrap();
    assert!(!Policy::from_config(Some(&off)).enabled);
}

#[test]
fn approved_red_on_an_untouched_test_that_passes_on_base_is_a_probable_flake() {
    let policy = Policy::default();
    let fact = red_pr(
        4344,
        red(
            &["tests::task_3792_tests::retries_once"],
            &["aida-cli-lib/src/landing_policy.rs", "docs/landing.md"],
            BaseChecks::Pass,
        ),
    );
    let (cause, reason) = red_cause(fact.red.as_ref().unwrap());
    assert_eq!(cause, RedCause::ProbableFlake);
    assert!(reason.contains("pass on the base branch"), "{reason}");
    let item = classify_pr(&fact, &policy, now(), &[]).unwrap();
    assert_eq!(item.class, Class::StuckMergeRed);
    assert!(item.key.ends_with(":rerun"), "{}", item.key);
    assert!(item
        .summary
        .contains("tests::task_3792_tests::retries_once"));
    assert!(item.summary.contains("probable flake"));
    assert!(item.summary.contains(&reason), "the reason is shown");
    assert!(item.action.contains("rerun the failed job on #4344 once"));
    assert!(item.action.contains("fix-forward"));
    // Inside the merge threshold it is not yet stuck.
    let mut fresh = fact.clone();
    fresh.since = Some(ago(5));
    assert_eq!(class_of(&fresh, &policy), None);
    // A lane already working the red head is moving it.
    let mut held = fact.clone();
    held.actor = Some("lane implementer-2".into());
    assert_eq!(class_of(&held, &policy), None);
}

#[test]
fn approved_red_on_a_test_the_change_touches_routes_rework() {
    let fact = red_pr(
        12,
        red(
            &[
                "landing_policy::tests::parses_mode",
                "tests::other_tests::x",
            ],
            &["aida-cli-lib/src/landing_policy.rs"],
            BaseChecks::Pass,
        ),
    );
    let (cause, reason) = red_cause(fact.red.as_ref().unwrap());
    assert_eq!(cause, RedCause::PrCaused);
    assert!(
        reason.contains("aida-cli-lib/src/landing_policy.rs"),
        "{reason}"
    );
    assert!(
        reason.contains("landing_policy::tests::parses_mode"),
        "{reason}"
    );
    let item = classify_pr(&fact, &Policy::default(), now(), &[]).unwrap();
    assert!(item.key.ends_with(":rework"));
    assert!(item.action.contains("route rework of #12"));
    assert!(item.summary.contains(&reason));
}

#[test]
fn approved_red_with_a_red_base_or_unread_evidence_is_never_called_a_flake() {
    let base_red = red(
        &["tests::a_tests::x"],
        &["docs/a.md"],
        BaseChecks::Red(vec!["Test (full)".into()]),
    );
    assert_eq!(red_cause(&base_red).0, RedCause::BaseRed);
    let item = classify_pr(&red_pr(5, base_red), &Policy::default(), now(), &[]).unwrap();
    assert!(item.key.ends_with(":base-red"));
    assert!(item.action.contains("fix the base branch forward"));

    let unknown_base = red(
        &["tests::a_tests::x"],
        &["docs/a.md"],
        BaseChecks::Unknown("gh timed out".into()),
    );
    let (cause, reason) = red_cause(&unknown_base);
    assert_eq!(cause, RedCause::Undetermined);
    assert!(reason.contains("gh timed out"));

    let no_names = red(&[], &["docs/a.md"], BaseChecks::Pass);
    assert_eq!(red_cause(&no_names).0, RedCause::Undetermined);

    let mut unlisted = red(&["tests::a_tests::x"], &[], BaseChecks::Pass);
    unlisted.changed_paths = Err("the head was not fetched".into());
    assert_eq!(red_cause(&unlisted).0, RedCause::Undetermined);

    let mut unread = red(&[], &["docs/a.md"], BaseChecks::Pass);
    unread.unread = Some("time budget for this run spent".into());
    let item = classify_pr(&red_pr(6, unread), &Policy::default(), now(), &[]).unwrap();
    assert!(item.key.ends_with(":inspect"));
    assert!(item.summary.contains("time budget for this run spent"));
}

#[test]
fn a_red_head_in_review_gated_mode_is_stuck_merge_red_and_unapproved_red_is_not() {
    // Review-gated: red CI does not gate, the row is READY with red CI.
    let mut ready_red = pr(8, State::Ready, Action::Train, 45);
    ready_red.ci = Some("red: Test (full)".into());
    assert_eq!(
        class_of(&ready_red, &Policy::default()),
        Some(Class::StuckMergeRed)
    );
    // Red without an approval is the lane's rework, not a stuck merge.
    let mut unapproved = pr(9, State::CiFailed, Action::DispatchRework, 45);
    unapproved.ci = Some("red: Test (full)".into());
    assert_eq!(class_of(&unapproved, &Policy::default()), None);
}

#[test]
fn a_flake_reruns_once_then_becomes_a_fix_forward() {
    let policy = Policy::default();
    let flake = || {
        red(
            &["tests::task_3792_tests::retries_once"],
            &["docs/a.md"],
            BaseChecks::Pass,
        )
    };
    let mut memory = Memory::default();
    // Run 1: a rerun is asked for and the failed check runs are remembered.
    let first = build_report(
        &facts(vec![red_pr(20, flake())]),
        &policy,
        &mut memory,
        now(),
    );
    assert!(first.stuck[0].key.ends_with(":rerun"));
    let key = rerun_key(20, &format!("{:040x}", 20));
    assert_eq!(memory.reruns.get(&key), Some(&vec![11]));
    // Run 2, nobody reran: the same check run is still the failed one.
    let mut waiting = flake();
    waiting.rerun_routed = memory.reruns.get(&key).cloned();
    let item = classify_pr(&red_pr(20, waiting), &policy, now(), &[]).unwrap();
    assert!(item.key.ends_with(":rerun"), "same key: not routed twice");
    // Run 3: the rerun happened (a new check run) and failed again.
    let mut again = flake();
    again.rerun_routed = memory.reruns.get(&key).cloned();
    again.check_ids = vec![12];
    let item = classify_pr(&red_pr(20, again), &policy, now(), &[]).unwrap();
    assert!(item.key.ends_with(":fix-forward"), "{}", item.key);
    assert!(item.action.contains("do not rerun #20 again"));
    // A test already known to flake goes straight to a fix-forward, linked.
    let known = [event(
        "tests::task_3792_tests::retries_once",
        300,
        "run 77#2",
    )];
    let item = classify_pr(&red_pr(21, flake()), &policy, now(), &known).unwrap();
    assert!(item.key.ends_with(":fix-forward"));
    assert!(item.summary.contains("run 77#2"));
    assert!(item.action.contains("https://forge.example/run 77#2"));
}

#[test]
fn a_flaky_test_that_fails_again_inside_the_window_is_a_recurrence_with_a_link() {
    let policy = Policy::default();
    let flakes = [
        event("tests::bug_2684_tests::x", 500, "run 1#2"),
        event("tests::old_tests::y", 60 * 30, "run 2#2"),
    ];
    let failures = [
        // The same test, elsewhere, later: a recurrence.
        event("tests::bug_2684_tests::x", 240, "PR-9@abcdef12"),
        // The flake's own run is not its recurrence.
        event("tests::bug_2684_tests::x", 500, "run 1#2"),
        // A failure before the flake is not a recurrence of it.
        event("tests::bug_2684_tests::x", 900, "run 0#1"),
        // The earlier flake is outside the 24 h window.
        event("tests::old_tests::y", 30, "run 3#1"),
        // No earlier flake at all.
        event("tests::fresh_tests::z", 30, "run 3#1"),
    ];
    let items = flake_recurrences(&flakes, &failures, &policy, now());
    assert_eq!(items.len(), 1, "{items:?}");
    let item = &items[0];
    assert_eq!(item.class, Class::MainFlakeRecurrence);
    assert_eq!(item.seat, "advisor");
    assert_eq!(
        item.key,
        "MAIN-FLAKE-RECURRENCE:tests::bug_2684_tests::x@PR-9@abcdef12"
    );
    assert!(
        item.summary.contains("https://forge.example/run 1#2"),
        "links the earlier occurrence"
    );
    assert!(item.summary.contains("PR-9@abcdef12"));
    assert!(item.action.contains("fix-forward"));
    // A shorter window drops it.
    let short = Policy {
        flake_window_hours: 2,
        ..Policy::default()
    };
    assert!(flake_recurrences(&flakes, &failures, &short, now()).is_empty());
}

#[test]
fn a_probable_flake_on_one_change_recurring_on_another_is_reported_once_seen() {
    let policy = Policy::default();
    let mut memory = Memory::default();
    let flake = |n, waited_mins| PrFact {
        since: Some(ago(waited_mins)),
        ..red_pr(
            n,
            red(
                &["tests::bug_2684_tests::x"],
                &["docs/a.md"],
                BaseChecks::Pass,
            ),
        )
    };
    let first = build_report(&facts(vec![flake(30, 200)]), &policy, &mut memory, ago(120));
    assert!(first
        .stuck
        .iter()
        .all(|item| item.class != Class::MainFlakeRecurrence));
    assert_eq!(memory.flakes.len(), 1, "the probable flake is remembered");
    // The same test now fails on another change.
    let second = build_report(&facts(vec![flake(31, 45)]), &policy, &mut memory, now());
    let recurrence = second
        .stuck
        .iter()
        .find(|item| item.class == Class::MainFlakeRecurrence)
        .expect("a recurrence");
    assert!(recurrence.summary.contains("PR-30@"));
    assert!(recurrence.summary.contains("PR-31@"));
    let red_item = second
        .stuck
        .iter()
        .find(|item| item.class == Class::StuckMergeRed)
        .unwrap();
    assert!(red_item.key.ends_with(":fix-forward"), "{}", red_item.key);
}

fn outcome(attempt: &str, finished_mins_ago: i64) -> LaneOutcomeFact {
    LaneOutcomeFact {
        attempt: attempt.into(),
        spec: "TASK-3821".into(),
        result: "failed-after-start",
        reason_code: "no-new-published-head".into(),
        detail: "fast gate needs a clean committed seat".into(),
        finished_at: ago(finished_mins_ago),
        launched_by: None,
        followed_up: None,
    }
}

#[test]
fn a_lane_exit_nobody_followed_up_routes_to_the_seat_that_launched_it() {
    let policy = Policy::default();
    let mut by_codex_advisor = outcome("a2", 60);
    by_codex_advisor.launched_by = Some("Advisor".into());
    let mut relaunched = outcome("a3", 60);
    relaunched.followed_up = Some("attempt a9".into());
    let items = classify_lane_outcomes(
        &[
            outcome("a1", 60),
            by_codex_advisor,
            relaunched,
            outcome("fresh", 5),
            outcome("ancient", 60 * 30),
        ],
        &policy,
        now(),
    );
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(items[0].key, "LANE-OUTCOME:a1");
    assert_eq!(
        items[0].seat, "product",
        "no launcher recorded: the launch seat"
    );
    assert_eq!(items[1].seat, "advisor", "the seat that launched it");
    let summary = &items[0].summary;
    assert!(summary.contains("TASK-3821"));
    assert!(summary.contains("failed-after-start reason=no-new-published-head"));
    assert!(summary.contains("fast gate needs a clean committed seat"));
    // The threshold is a setting.
    let patient = Policy {
        lane_outcome_mins: 90,
        ..Policy::default()
    };
    assert!(classify_lane_outcomes(&[outcome("a1", 60)], &patient, now()).is_empty());
    // An operator or malformed launcher never becomes a routed seat.
    assert_eq!(launch_seat(Some("operator")), "product");
    assert_eq!(launch_seat(Some("../x")), "product");
}

#[test]
fn only_a_stale_claim_the_doctor_cannot_heal_is_routed_with_its_record_and_evidence() {
    let policy = Policy::default();
    let claim = |scope: &str, unhealable: Option<&str>, expired_mins| ClaimFact {
        scope: scope.into(),
        record: PathBuf::from(format!("/store/claims/{scope}.json")),
        evidence: "heartbeat 9000s old, past its 1800s TTL".into(),
        unhealable: unhealable.map(str::to_string),
        expired_mins,
    };
    let items = classify_claims(
        &[
            claim(
                "BUG-2697",
                Some("its recorded host `disrad-x` is not this host `dell1`"),
                120,
            ),
            claim("TASK-1", None, 120),
            claim("TASK-2", Some("the record carries no claim id"), 10),
        ],
        &policy,
    );
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].class, Class::StaleClaim);
    assert_eq!(items[0].seat, "advisor");
    assert!(items[0].summary.contains("/store/claims/BUG-2697.json"));
    assert!(items[0].summary.contains("past its 1800s TTL"));
    assert!(items[0].summary.contains("disrad-x"));
}

#[test]
fn the_report_counts_landings_lists_hosts_and_escalates_only_after_two_dry_hours_with_stalls() {
    let policy = Policy::default();
    let stuck = || vec![pr(1, State::Ready, Action::Train, 45)];
    let moving = || vec![pr(2, State::Reviewing, Action::Wait, 45)];
    let with_landings = |prs, landings: Vec<DateTime<Utc>>| Facts {
        prs,
        last_landing: landings.iter().max().copied().or(Some(ago(300))),
        landings: Some(Ok(landings)),
        hosts: vec![HostLine {
            host: "dell1".into(),
            lanes: 4,
            memory: "20.0 GiB available of 64.0 GiB".into(),
        }],
        ..Facts::default()
    };
    // Landings 30 and 90 minutes ago: one in the last hour, not dry.
    let report = build_report(
        &with_landings(stuck(), vec![ago(30), ago(90)]),
        &policy,
        &mut Memory::default(),
        now(),
    );
    assert_eq!(report.landings.count, Some(1));
    assert_eq!(report.landings.last_mins_ago, Some(30));
    assert!(!report.landings.dry);
    assert!(!report.escalate);
    assert_eq!(report.hosts[0].lanes, 4);
    assert_eq!(report.changes[0].class, Class::StuckMerge);
    // One landing 90 minutes ago: none this hour, but not two dry hours.
    let report = build_report(
        &with_landings(stuck(), vec![ago(90)]),
        &policy,
        &mut Memory::default(),
        now(),
    );
    assert_eq!(report.landings.count, Some(0));
    assert!(!report.escalate);
    // Two dry hours with something stuck: escalate.
    let report = build_report(
        &with_landings(stuck(), Vec::new()),
        &policy,
        &mut Memory::default(),
        now(),
    );
    assert!(report.landings.dry);
    assert!(report.escalate);
    // Two dry hours with nothing stuck: a quiet project is not an escalation.
    let report = build_report(
        &with_landings(moving(), Vec::new()),
        &policy,
        &mut Memory::default(),
        now(),
    );
    assert!(report.landings.dry);
    assert!(!report.escalate);
    assert_eq!(report.changes[0].class, Class::Moving);
    // Unreadable history is unknown, never "no landings".
    let mut unread = with_landings(stuck(), Vec::new());
    unread.landings = Some(Err("unknown revision".into()));
    let report = build_report(&unread, &policy, &mut Memory::default(), now());
    assert_eq!(report.landings.count, None);
    assert!(!report.escalate);
    assert!(report
        .unknown
        .iter()
        .any(|u| u.contains("unknown revision")));
    // The escalation window is a setting.
    let patient = Policy {
        escalate_after_mins: 600,
        ..Policy::default()
    };
    let report = build_report(
        &with_landings(stuck(), vec![ago(300)]),
        &patient,
        &mut Memory::default(),
        now(),
    );
    assert!(!report.escalate);
}

#[test]
fn routing_is_one_item_per_seat_and_the_same_stall_is_never_routed_twice() {
    let policy = Policy::default();
    let mut memory = Memory::default();
    let mut facts = facts(vec![
        pr(1, State::Ready, Action::Train, 45),
        pr(2, State::NeedsReview, Action::LaunchReview, 45),
    ]);
    facts.lane_outcomes = vec![outcome("a1", 60)];
    let report = build_report(&facts, &policy, &mut memory, now());
    let plan = plan_routes(&report, &memory, &policy, now());
    assert_eq!(
        plan.seats.keys().collect::<Vec<_>>(),
        ["advisor", "product"],
        "one consolidated item per seat"
    );
    assert_eq!(plan.seats["advisor"].0.len(), 2);
    assert_eq!(plan.seats["advisor"].1.len(), 2);
    assert!(!plan.escalate);
    // Everything was routed: the next run wakes no one.
    for item in &report.stuck {
        memory.routed.insert(item.key.clone(), now());
    }
    assert!(plan_routes(&report, &memory, &policy, now())
        .seats
        .is_empty());
    // A new stall wakes its seat with all of the seat's current stalls.
    facts
        .prs
        .push(pr(3, State::Conflict, Action::DispatchRework, 45));
    let report = build_report(&facts, &policy, &mut memory, now());
    let plan = plan_routes(&report, &memory, &policy, now());
    assert_eq!(plan.seats.keys().collect::<Vec<_>>(), ["advisor"]);
    assert_eq!(plan.seats["advisor"].0.len(), 3);
    assert_eq!(plan.seats["advisor"].1.len(), 1);
    assert_eq!(plan.seats["advisor"].1[0].pr, Some(3));
    // The same change with a new head is a new blocker.
    let mut pushed = pr(1, State::Ready, Action::Train, 45);
    pushed.head = "f".repeat(40);
    let report = build_report(&self::facts(vec![pushed]), &policy, &mut memory, now());
    assert_eq!(
        plan_routes(&report, &memory, &policy, now()).seats["advisor"]
            .1
            .len(),
        1
    );
}

#[test]
fn the_operator_is_escalated_to_once_per_window() {
    let policy = Policy::default();
    let mut memory = Memory::default();
    let mut dry = facts(vec![pr(1, State::Ready, Action::Train, 45)]);
    dry.landings = Some(Ok(Vec::new()));
    let report = build_report(&dry, &policy, &mut memory, now());
    assert!(plan_routes(&report, &memory, &policy, now()).escalate);
    memory.escalated_at = Some(now());
    let later = now() + Duration::minutes(20);
    assert!(!plan_routes(&report, &memory, &policy, later).escalate);
    let much_later = now() + Duration::minutes(policy.escalate_after_mins);
    assert!(plan_routes(&report, &memory, &policy, much_later).escalate);
}

// The I/O layer: a due job per seat with the consolidated evidence, a mail
// wake to the seat, the operator escalation, and no repeat on the next run.
#[test]
fn a_scheduled_run_raises_due_jobs_and_mail_once_and_escalates_to_the_operator() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let store = root.join(".aida-store");
    let policy = Policy::default();
    let mut memory = Memory::default();
    let mut dry = facts(vec![
        pr(1, State::Ready, Action::Train, 45),
        pr(2, State::NeedsReview, Action::LaunchReview, 45),
    ]);
    dry.lane_outcomes = vec![outcome("a1", 60)];
    dry.landings = Some(Ok(Vec::new()));
    let report = build_report(&dry, &policy, &mut memory, now());
    route(root, &report, &mut memory, &policy, now());

    let advisor = schedule_ledger::load(&store, &route_job_name("advisor")).unwrap();
    assert_eq!(advisor.due_since, Some(now()));
    let note = advisor.note.unwrap();
    assert!(note.contains("[STUCK-MERGE] #1"), "{note}");
    assert!(note.contains("[STUCK-REVIEW] #2"), "{note}");
    let product = schedule_ledger::load(&store, &route_job_name("product")).unwrap();
    assert!(product.note.unwrap().contains("[LANE-OUTCOME]"));
    let operator = schedule_ledger::load(&store, &route_job_name("operator")).unwrap();
    assert!(operator
        .note
        .unwrap()
        .contains("no landing for 120 min with 3 stall(s)"));
    assert_eq!(memory.escalated_at, Some(now()));
    assert_eq!(memory.routed.len(), 3);
    assert_eq!(
        known_route_seat(&store, "throughput-guard-route-advisor"),
        Some("advisor")
    );
    assert_eq!(
        known_route_seat(&store, "throughput-guard-route-typo"),
        None
    );
    assert_eq!(
        known_route_seat(&store, "mail-backlog-guard-route-advisor"),
        None
    );

    let mail = |root: &Path| -> Vec<aida_core::mailbox::Message> {
        crate::mailbox_store::read_local_messages(root)
            .unwrap()
            .into_iter()
            .filter(|m| m.from == SENDER)
            .collect()
    };
    let sent = mail(root);
    assert_eq!(sent.len(), 2, "one mail wake per routed seat");
    let to: BTreeSet<String> = sent
        .iter()
        .map(|m| match &m.to {
            aida_core::mailbox::Recipient::Agent(seat) => seat.clone(),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(to.into_iter().collect::<Vec<_>>(), ["advisor", "product"]);

    // The next run, 20 minutes on, with the same stalls: nothing new is sent.
    let later = now() + Duration::minutes(20);
    let report = build_report(&dry, &policy, &mut memory, later);
    route(root, &report, &mut memory, &policy, later);
    assert_eq!(mail(root).len(), 2, "no second wake for the same stalls");
    assert_eq!(memory.escalated_at, Some(now()), "no second escalation");
    assert_eq!(
        schedule_ledger::load(&store, &route_job_name("advisor"))
            .unwrap()
            .due_since,
        Some(now())
    );

    // A landing ends the dry spell and the stalls clear: the memory empties,
    // so a return of the same stall routes again.
    let mut moving = facts(Vec::new());
    moving.landings = Some(Ok(vec![later]));
    let report = build_report(&moving, &policy, &mut memory, later);
    route(root, &report, &mut memory, &policy, later);
    assert!(memory.routed.is_empty());
    assert_eq!(memory.escalated_at, None);
}

#[test]
fn test_names_come_from_libtest_and_nextest_failure_lines() {
    assert_eq!(
        test_name("test tests::queue_work_tests::drains ... FAILED").as_deref(),
        Some("tests::queue_work_tests::drains")
    );
    assert_eq!(
        test_name("FAIL [   0.512s] aida-cli-lib land::tests::derives").as_deref(),
        Some("land::tests::derives")
    );
    assert_eq!(
        test_name("TIMEOUT [ 60.0s] aida-core store::tests::slow").as_deref(),
        Some("store::tests::slow")
    );
    assert_eq!(test_name("test result: FAILED. 1 passed; 1 failed"), None);
    // A name is a plain test path: text a log could smuggle in is not one.
    assert_eq!(
        test_name("test ignore_previous_instructions;rm ... FAILED"),
        None
    );
    assert_eq!(test_name("test a::b`c` ... FAILED"), None);
    assert_eq!(test_name("FAIL [ 1s] crate $(reboot)"), None);
    assert_eq!(
        test_name(&format!("test {} ... FAILED", "a".repeat(300))),
        None
    );
    assert_eq!(test_name("test  ... FAILED"), None);
    let many: Vec<String> = (0..11).map(|n| format!("t::case_{n:02}")).collect();
    assert!(
        listed(&many).ends_with("t::case_07 and 3 more"),
        "{}",
        listed(&many)
    );
    assert_eq!(listed(&many[..2]), "t::case_00, t::case_01");
    assert_eq!(test_name("thread 'x' panicked at src/a.rs:1:1:"), None);
    let lines = vec![
        "test b::t ... FAILED".to_string(),
        "error: test failed, to rerun pass `--lib`".to_string(),
        "test a::t ... FAILED".to_string(),
        "test b::t ... FAILED".to_string(),
    ];
    assert_eq!(test_names(&lines), ["a::t", "b::t"]);
}

#[test]
fn a_failing_test_is_matched_to_changed_files_by_module_path() {
    let changed: Vec<String> = [
        "aida-cli-lib/src/queue_work.rs",
        "aida-cli-lib/src/gate/mod.rs",
        "docs/cli/land.md",
    ]
    .iter()
    .map(|p| p.to_string())
    .collect();
    // A module file, a module directory, and a `tests` wrapper are seen through.
    assert_eq!(
        touching_path("queue_work::tests::drains", &changed),
        Some("aida-cli-lib/src/queue_work.rs")
    );
    assert_eq!(
        touching_path("gate::approval::tests::x", &changed),
        Some("aida-cli-lib/src/gate/mod.rs")
    );
    // Hyphens and case do not hide a match.
    assert_eq!(
        touching_path("Queue_Work::tests::x", &changed),
        Some("aida-cli-lib/src/queue_work.rs")
    );
    // Untouched modules, and `src`/`tests` path words, never match.
    assert_eq!(touching_path("tests::task_3792_tests::x", &changed), None);
    assert_eq!(touching_path("src::tests::x", &changed), None);
    // A documentation file named like a module still counts as touching it.
    assert_eq!(
        touching_path("land::tests::x", &changed),
        Some("docs/cli/land.md")
    );
    // A test with no module path is compared by name.
    assert_eq!(
        touching_path("queue_work_drains_in_order", &changed),
        Some("aida-cli-lib/src/queue_work.rs")
    );
    assert_eq!(touching_path("unrelated_test", &changed), None);
    assert_eq!(touching_path("x", &[]), None);
}

#[test]
fn the_json_report_and_the_table_carry_the_same_classification() {
    let mut facts = facts(vec![
        pr(1, State::Ready, Action::Train, 45),
        pr(2, State::Reviewing, Action::Wait, 5),
    ]);
    facts.unknown = vec!["flaky tests on `main`: unknown (gh is not installed)".into()];
    let report = build_report(&facts, &Policy::default(), &mut Memory::default(), now());
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["changes"][0]["class"], "STUCK-MERGE");
    assert_eq!(json["changes"][0]["state"], "READY");
    assert_eq!(json["changes"][1]["class"], "moving");
    assert_eq!(json["stuck"][0]["class"], "STUCK-MERGE");
    assert_eq!(json["stuck"][0]["seat"], "advisor");
    assert_eq!(json["landings"]["count"], 1);
    assert_eq!(json["escalate"], false);
    let text = render(&report);
    assert!(text.contains("#1      STUCK-MERGE"), "{text}");
    assert!(text.contains("#2      moving"), "{text}");
    assert!(text.contains("Stuck (1):"), "{text}");
    assert!(text.contains("-> advisor: land #1"), "{text}");
    assert!(text.contains("Landings in the last 60 min: 1"), "{text}");
    assert!(text.contains("Not read: flaky tests on `main`"), "{text}");
    let quiet = build_report(
        &self::facts(Vec::new()),
        &Policy::default(),
        &mut Memory::default(),
        now(),
    );
    let text = render(&quiet);
    assert!(text.contains("(no open changes)"));
    assert!(text.contains("Nothing is stuck."));
}

#[test]
fn the_guards_memory_forgets_what_left_the_window() {
    let policy = Policy::default();
    let mut memory = Memory {
        flakes: vec![
            event("a::t", 60, "run 1#2"),
            event("b::t", 60 * 30, "run 2#2"),
        ],
        base_failures: vec![event("c::t", 60 * 30, "run 3#1")],
        seen_runs: [
            ("1#2".to_string(), ago(60)),
            ("0#1".to_string(), ago(60 * 24 * 10)),
        ]
        .into_iter()
        .collect(),
        ..Memory::default()
    };
    build_report(&facts(Vec::new()), &policy, &mut memory, now());
    assert_eq!(memory.flakes.len(), 1);
    assert_eq!(memory.flakes[0].test, "a::t");
    assert!(memory.base_failures.is_empty());
    assert_eq!(memory.seen_runs.keys().collect::<Vec<_>>(), ["1#2"]);
    // The memory round-trips, and an older file without newer keys still loads.
    let text = serde_json::to_string(&memory).unwrap();
    assert_eq!(serde_json::from_str::<Memory>(&text).unwrap(), memory);
    assert_eq!(
        serde_json::from_str::<Memory>("{}").unwrap(),
        Memory::default()
    );
}
