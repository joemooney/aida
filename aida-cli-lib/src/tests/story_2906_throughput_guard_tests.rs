// trace:STORY-2906 | ai:claude
use super::*;
use crate::land::{derive, Evidence, Pr, Rounds, Row};
use std::cell::{Cell, RefCell};

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-11-20T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn ago(mins: i64) -> DateTime<Utc> {
    now() - Duration::minutes(mins)
}

fn stamp(mins_ago: i64) -> Option<String> {
    Some(ago(mins_ago).to_rfc3339())
}

/// The evidence of one change, as the reconciler and the guard both read
/// it. Each test changes only what it is about.
struct Fake {
    conflict: std::result::Result<bool, String>,
    reviewing: std::result::Result<Option<String>, String>,
    ci: std::result::Result<Ci, String>,
    /// The failed checks at the head, when read apart from `ci`.
    head_red: Option<std::result::Result<Option<String>, String>>,
    approval: std::result::Result<Approval, String>,
    hold: std::result::Result<Option<Hold>, String>,
    rounds: std::result::Result<Rounds, String>,
    lane: std::result::Result<Option<String>, String>,
    ci_gates: bool,
    head_at: Option<String>,
    approved_at: Option<String>,
    conflict_since: Option<String>,
}

impl Default for Fake {
    fn default() -> Self {
        Self {
            conflict: Ok(false),
            reviewing: Ok(None),
            ci: Ok(Ci::Green),
            head_red: None,
            approval: Ok(Approval::Refused("no approval covers the head".into())),
            hold: Ok(None),
            rounds: Ok(Rounds::default()),
            lane: Ok(None),
            ci_gates: true,
            head_at: stamp(45),
            approved_at: None,
            conflict_since: None,
        }
    }
}

impl Evidence for Fake {
    fn ci_gates_landing(&self) -> bool {
        self.ci_gates
    }
    fn conflict(&self, _: &Pr) -> std::result::Result<bool, String> {
        self.conflict.clone()
    }
    fn reviewing(&self, _: &Pr) -> std::result::Result<Option<String>, String> {
        self.reviewing.clone()
    }
    fn ci(&self, _: &Pr) -> std::result::Result<Ci, String> {
        self.ci.clone()
    }
    fn head_red(&self, _: &Pr) -> std::result::Result<Option<String>, String> {
        match &self.head_red {
            Some(read) => read.clone(),
            None => Ok(match self.ci.clone()? {
                Ci::Red(names) => Some(names),
                _ => None,
            }),
        }
    }
    fn approval(&self, _: &Pr) -> std::result::Result<Approval, String> {
        self.approval.clone()
    }
    fn hold(&self, _: &Pr) -> std::result::Result<Option<Hold>, String> {
        self.hold.clone()
    }
    fn rounds(&self, _: &Pr) -> std::result::Result<Rounds, String> {
        self.rounds.clone()
    }
    fn lane(&self, _: &Pr) -> std::result::Result<Option<String>, String> {
        self.lane.clone()
    }
    fn head_at(&self, _: &Pr) -> Option<String> {
        self.head_at.clone()
    }
    fn approved_at(&self, _: &Pr) -> Option<String> {
        self.approved_at.clone()
    }
    fn conflict_since(&self, _: &Pr) -> Option<String> {
        self.conflict_since.clone()
    }
}

fn change(number: u64, base: &str, branch: &str) -> Pr {
    Pr {
        id: number,
        head: format!("{number:040x}"),
        base: base.into(),
        branch: Some(branch.into()),
        title: format!("change {number} (TASK-{number})"),
        updated_at: None,
        hold_label: None,
        ci: None,
    }
}

/// The reconciler's own row and the guard's fact, from the same evidence.
fn observed_as(number: u64, base: &str, branch: &str, fake: &Fake) -> (Row, PrFact) {
    let pr = change(number, base, branch);
    let row = derive(&pr, fake);
    let fact = pr_fact(&pr, &row, fake);
    (row, fact)
}

fn observed(number: u64, fake: &Fake) -> (Row, PrFact) {
    observed_as(number, "main", "feature", fake)
}

fn fact(number: u64, fake: &Fake) -> PrFact {
    observed(number, fake).1
}

fn class_of(fact: &PrFact, policy: &Policy) -> Option<Class> {
    classify_pr(fact, policy, now()).map(|item| item.class)
}

fn approved(approved_mins_ago: i64) -> Fake {
    Fake {
        approval: Ok(Approval::Approved("approved at 0123456789ab".into())),
        approved_at: stamp(approved_mins_ago),
        head_at: stamp(approved_mins_ago + 30),
        ..Fake::default()
    }
}

fn changes_requested(verdict_mins_ago: i64) -> Fake {
    Fake {
        rounds: Ok(Rounds {
            at_content: true,
            content_rounds: 1,
            since: stamp(verdict_mins_ago),
        }),
        head_at: stamp(verdict_mins_ago + 30),
        ..Fake::default()
    }
}

fn red_ci() -> std::result::Result<Ci, String> {
    Ok(Ci::Red("Test (full)".into()))
}

fn facts(prs: Vec<PrFact>) -> Facts {
    Facts {
        prs,
        landings: Some(Ok(vec![ago(10)])),
        last_landing: Some(ago(10)),
        ..Facts::default()
    }
}

fn check(name: &str, id: u64, tests: &[&str], base: BaseCheck) -> FailedCheck {
    FailedCheck {
        name: name.into(),
        id,
        tests: Ok(tests.iter().map(|t| t.to_string()).collect()),
        base,
    }
}

fn red(checks: Vec<FailedCheck>, changed: &[&str]) -> RedFact {
    RedFact {
        checks,
        changed_paths: Ok(changed.iter().map(|p| p.to_string()).collect()),
        rerun_routed: None,
        unread: None,
    }
}

/// An approved head, red for 45 minutes, with this CI detail.
fn red_pr(number: u64, red: RedFact) -> PrFact {
    let mut fact = fact(
        number,
        &Fake {
            ci: red_ci(),
            ..approved(45)
        },
    );
    assert!(fact.wants_red_detail());
    fact.red = Some(red);
    fact
}

fn item_of(fact: &PrFact) -> Item {
    classify_pr(fact, &Policy::default(), now()).expect("stuck")
}

#[test]
fn each_stall_class_is_found_past_its_threshold_from_the_evidence() {
    let policy = Policy::default();
    let cases = [
        (approved(45), State::Ready, Class::StuckMerge),
        (
            Fake {
                ci: red_ci(),
                ..approved(45)
            },
            State::CiFailed,
            Class::StuckMergeRed,
        ),
        (Fake::default(), State::NeedsReview, Class::StuckReview),
        (
            changes_requested(45),
            State::ChangesRequested,
            Class::StuckRework,
        ),
        (
            Fake {
                conflict: Ok(true),
                conflict_since: stamp(45),
                ..Fake::default()
            },
            State::Conflict,
            Class::StuckConflict,
        ),
    ];
    for (n, (fake, state, class)) in cases.into_iter().enumerate() {
        let (row, fact) = observed(n as u64 + 1, &fake);
        assert_eq!(row.state, state);
        let item = classify_pr(&fact, &policy, now()).expect("stuck");
        assert_eq!(item.class, class);
        assert_eq!(item.seat, "advisor");
        assert_eq!(item.pr, Some(n as u64 + 1));
        assert_eq!(item.age_mins, Some(45));
        assert!(item.key.starts_with(class.label()), "{}", item.key);
    }
}

// The reconciler's state is one label in a fixed precedence: a red head is
// CI-FAILED before its review history is read, and a held approval is
// BLOCKED. The guard reads each fact, so these are still stalls.
#[test]
fn a_stall_the_reconcilers_precedence_hides_is_still_found() {
    let policy = Policy::default();
    // An old head with no verdict, red CI and nobody on it.
    let (row, fact) = observed(
        1,
        &Fake {
            ci: red_ci(),
            head_at: stamp(180),
            ..Fake::default()
        },
    );
    assert_eq!(row.state, State::CiFailed);
    let item = classify_pr(&fact, &policy, now()).expect("stuck");
    assert_eq!(item.class, Class::StuckReview);
    assert!(item.summary.contains("its CI is red"), "{}", item.summary);
    assert!(item.action.contains("launch a reviewer"), "{}", item.action);
    // Changes requested at the head, red CI and no owner.
    let (row, fact) = observed(
        2,
        &Fake {
            ci: red_ci(),
            ..changes_requested(90)
        },
    );
    assert_eq!(row.state, State::CiFailed);
    let item = classify_pr(&fact, &policy, now()).expect("stuck");
    assert_eq!(item.class, Class::StuckRework);
    assert_eq!(item.age_mins, Some(90));
    // An approved head behind a hold nothing releases.
    let (row, fact) = observed(
        3,
        &Fake {
            hold: Ok(Some(Hold {
                detail: "operator hold: wait for the release".into(),
                release: HoldRelease::Unmet("a human clears it".into()),
                since: stamp(50),
            })),
            ..approved(45)
        },
    );
    assert_eq!(row.state, State::Blocked);
    let item = classify_pr(&fact, &policy, now()).expect("stuck");
    assert_eq!(item.class, Class::StuckMerge);
    assert!(item.key.ends_with(":held"), "{}", item.key);
    assert!(item.summary.contains("a human clears it"));
    assert!(item.action.contains("rule on the hold"));
    // Three content rounds: the reconciler asks for a ruling, and the stall
    // carries that, not a lane.
    let (row, fact) = observed(
        4,
        &Fake {
            rounds: Ok(Rounds {
                at_content: true,
                content_rounds: 3,
                since: stamp(45),
            }),
            ..Fake::default()
        },
    );
    assert_eq!(row.action, Action::AdvisorRuling);
    let item = classify_pr(&fact, &policy, now()).expect("stuck");
    assert_eq!(item.class, Class::StuckRework);
    assert!(item.action.starts_with("rule on #4"), "{}", item.action);
}

#[test]
fn a_change_someone_is_working_or_that_cannot_be_read_is_moving() {
    let policy = Policy::default();
    let lane = || Ok(Some("lane@dell1 (l1)".to_string()));
    let moving = [
        // A review is running.
        Fake {
            reviewing: Ok(Some("reviewer@dell1".into())),
            ..Fake::default()
        },
        // A lane holds the rework, the conflict, the red approved head and
        // the red unreviewed head.
        Fake {
            lane: lane(),
            ..changes_requested(45)
        },
        Fake {
            conflict: Ok(true),
            conflict_since: stamp(45),
            lane: lane(),
            ..Fake::default()
        },
        Fake {
            ci: red_ci(),
            lane: lane(),
            ..approved(45)
        },
        Fake {
            ci: red_ci(),
            lane: lane(),
            ..Fake::default()
        },
        // An unreadable fact is unknown, never a stall.
        Fake {
            approval: Err("the forge did not report the change's commits".into()),
            ..Fake::default()
        },
        Fake {
            lane: Err("leases unreadable".into()),
            ..changes_requested(45)
        },
    ];
    for (n, fake) in moving.iter().enumerate() {
        assert_eq!(class_of(&fact(n as u64, fake), &policy), None, "case {n}");
    }
    // A train is the landing itself; a stacked approval lands after its base.
    let (row, train) = observed_as(8, "main", "train/2026-11-20", &approved(45));
    assert_eq!(row.state, State::Landing);
    assert_eq!(train.standing, Standing::Landing);
    assert_eq!(class_of(&train, &policy), None);
    let (_, stacked) = observed_as(9, "story-1-base", "feature", &approved(45));
    assert_eq!(class_of(&stacked, &policy), None);
    // What could not be read is said, never dropped.
    let unread = fact(
        10,
        &Fake {
            ci: Err("discovery did not report checks".into()),
            ..Fake::default()
        },
    );
    let report = build_report(&facts(vec![unread]), &policy, &mut Memory::default(), now());
    assert_eq!(report.changes[0].class, Class::Moving);
    assert!(
        report.unknown.iter().any(|u| u.contains("#10: CI unknown")),
        "{:?}",
        report.unknown
    );
}

// A wait counts from the push or the verdict, not from when the guard first
// looked.
#[test]
fn each_wait_counts_from_the_push_or_verdict_that_started_it() {
    let policy = Policy::default();
    // A head pushed three hours ago is stuck the first time it is seen.
    let old_head = fact(
        1,
        &Fake {
            head_at: stamp(180),
            ..Fake::default()
        },
    );
    assert_eq!(item_of(&old_head).age_mins, Some(180));
    // A head red for hours and approved five minutes ago has waited five.
    let just_approved = Fake {
        ci: red_ci(),
        head_at: stamp(300),
        approved_at: stamp(5),
        ..approved(5)
    };
    let fact_2 = fact(2, &just_approved);
    assert_eq!(fact_2.since, Some(ago(5)));
    assert_eq!(class_of(&fact_2, &policy), None);
    // An approval carried over a newer head counts from that head.
    let carried = Fake {
        head_at: stamp(10),
        approved_at: stamp(240),
        ..approved(240)
    };
    assert_eq!(fact(3, &carried).since, Some(ago(10)));
    // Requested changes count from the verdict, whatever the head's age.
    let fact_4 = fact(4, &changes_requested(31));
    assert_eq!(fact_4.since, Some(ago(31)));
    assert_eq!(class_of(&fact_4, &policy), Some(Class::StuckRework));
    assert_eq!(class_of(&fact(5, &changes_requested(29)), &policy), None);
    // A conflict counts from the base commit that first conflicted; a head
    // pushed after it counts from the push.
    let conflict = |since: i64, head: i64| Fake {
        conflict: Ok(true),
        conflict_since: stamp(since),
        head_at: stamp(head),
        ..Fake::default()
    };
    assert_eq!(fact(6, &conflict(90, 300)).since, Some(ago(90)));
    assert_eq!(fact(7, &conflict(90, 20)).since, Some(ago(20)));
    // A time that cannot be read is unknown: not stuck, and said so.
    let undated = fact(
        8,
        &Fake {
            approved_at: None,
            ..approved(45)
        },
    );
    assert_eq!(undated.since, None);
    let report = build_report(
        &facts(vec![undated]),
        &policy,
        &mut Memory::default(),
        now(),
    );
    assert!(report.stuck.is_empty());
    assert!(report.unknown[0].contains("#8: how long it has waited is unknown"));
}

#[test]
fn thresholds_are_project_settings_and_the_guard_ships_off() {
    let default = Policy::default();
    assert!(!default.enabled);
    assert_eq!(Policy::from_config(None), default);
    let cfg: toml::Value = toml::from_str(
        r#"
[throughput_guard]
enabled = true
merge_mins = 5
review_mins = 240
rework_mins = 0
conflict_mins = "soon"
escalate_after_mins = 600
max_red_logs = 2
"#,
    )
    .unwrap();
    let policy = Policy::from_config(Some(&cfg));
    assert!(policy.enabled);
    assert_eq!(policy.merge_mins, 5);
    assert_eq!(policy.review_mins, 240);
    assert_eq!(policy.escalate_after_mins, 600);
    assert_eq!(policy.max_red_logs, 2);
    // A non-positive or non-integer value keeps its default.
    assert_eq!(policy.rework_mins, default.rework_mins);
    assert_eq!(policy.conflict_mins, default.conflict_mins);
    // The same change is stuck or moving by the setting alone.
    let merge = fact(1, &approved(10));
    assert_eq!(class_of(&merge, &default), None);
    assert_eq!(class_of(&merge, &policy), Some(Class::StuckMerge));
    let review = fact(2, &Fake::default());
    assert_eq!(class_of(&review, &default), Some(Class::StuckReview));
    assert_eq!(class_of(&review, &policy), None);
    // Exactly at the threshold is not past it.
    assert_eq!(class_of(&fact(3, &approved(20)), &default), None);
    assert_eq!(
        class_of(&fact(3, &approved(21)), &default),
        Some(Class::StuckMerge)
    );
}

#[test]
fn approved_red_on_an_untouched_test_that_passes_on_base_is_a_probable_flake() {
    let fact = red_pr(
        4344,
        red(
            vec![check(
                "Test (full)",
                11,
                &["tests::task_3792_tests::retries"],
                BaseCheck::Pass,
            )],
            &["aida-cli-lib/src/leases.rs"],
        ),
    );
    let verdict = red_cause(fact.red.as_ref().unwrap());
    assert_eq!(verdict.cause, RedCause::ProbableFlake);
    let item = item_of(&fact);
    assert_eq!(item.class, Class::StuckMergeRed);
    assert!(item.key.ends_with(":rerun"), "{}", item.key);
    assert!(item
        .summary
        .contains("failing tests: tests::task_3792_tests::retries"));
    assert!(item.summary.contains("probable flake"));
    assert!(item.summary.contains("passes on the base branch"));
    assert!(item.action.contains("rerun the failed job on #4344 once"));
}

#[test]
fn approved_red_on_a_test_the_change_touches_routes_rework() {
    let fact = red_pr(
        7,
        red(
            vec![check(
                "Test (full)",
                11,
                &["leases::tests::heals", "tests::other::x"],
                BaseCheck::Pass,
            )],
            &["aida-cli-lib/src/leases.rs"],
        ),
    );
    let item = item_of(&fact);
    assert!(item.key.ends_with(":rework"), "{}", item.key);
    assert!(item.summary.contains("the change plausibly caused it"));
    assert!(item.summary.contains("aida-cli-lib/src/leases.rs"));
    assert_eq!(item.action, "route rework of #7 for the failing tests");
}

// One failed job names an untouched test; another failed job's log could
// not be read. The unread job may hold the change's own failure.
#[test]
fn an_unread_failed_job_keeps_the_cause_undetermined_never_a_flake() {
    let unread = FailedCheck {
        name: "Test (windows)".into(),
        id: 12,
        tests: Err("gh timed out after 20s or did not start".into()),
        base: BaseCheck::Pass,
    };
    let fact = red_pr(
        7,
        red(
            vec![
                check("Test (full)", 11, &["tests::other::x"], BaseCheck::Pass),
                unread.clone(),
            ],
            &["aida-cli-lib/src/leases.rs"],
        ),
    );
    let verdict = red_cause(fact.red.as_ref().unwrap());
    assert_eq!(verdict.cause, RedCause::Undetermined);
    let item = item_of(&fact);
    assert!(item.key.ends_with(":inspect"), "{}", item.key);
    assert!(
        !item.summary.contains("probable flake:"),
        "{}",
        item.summary
    );
    assert!(item
        .summary
        .contains("the log of `Test (windows)` was not read"));
    assert!(!item.action.contains("fix-forward"));
    // Whatever was read still decides when it shows the change's own failure.
    let touched = red_pr(
        8,
        red(
            vec![
                check(
                    "Test (full)",
                    11,
                    &["leases::tests::heals"],
                    BaseCheck::Pass,
                ),
                unread,
            ],
            &["aida-cli-lib/src/leases.rs"],
        ),
    );
    assert!(item_of(&touched).key.ends_with(":rework"));
    // Nothing listed, no test named, or the base unknown: undetermined too.
    let mut nothing = red(Vec::new(), &[]);
    nothing.unread = Some("gh is not installed".into());
    assert_eq!(red_cause(&nothing).cause, RedCause::Undetermined);
    assert_eq!(
        red_cause(&red(Vec::new(), &[])).cause,
        RedCause::Undetermined
    );
    let unnamed = red(vec![check("Build", 1, &[], BaseCheck::Pass)], &["a.rs"]);
    assert_eq!(red_cause(&unnamed).cause, RedCause::Undetermined);
    let base_unknown = red(
        vec![check("T", 1, &["a::b"], BaseCheck::default())],
        &["z.rs"],
    );
    assert_eq!(red_cause(&base_unknown).cause, RedCause::Undetermined);
    let mut unlisted = red(vec![check("T", 1, &["a::b"], BaseCheck::Pass)], &[]);
    unlisted.changed_paths = Err("unknown revision".into());
    assert_eq!(red_cause(&unlisted).cause, RedCause::Undetermined);
    // A red head whose detail was never attached is inspected.
    let mut bare = red_pr(9, red(Vec::new(), &[]));
    bare.red = None;
    assert!(item_of(&bare).key.ends_with(":inspect"));
}

// Check A is red on the base and on the change; check B fails only on the
// change, in code it touches.
#[test]
fn a_base_red_check_does_not_exonerate_another_failing_check() {
    let fact = red_pr(
        7,
        red(
            vec![
                check("Lint", 10, &["tests::lint::x"], BaseCheck::Red),
                check(
                    "Test (full)",
                    11,
                    &["leases::tests::heals"],
                    BaseCheck::Pass,
                ),
            ],
            &["aida-cli-lib/src/leases.rs"],
        ),
    );
    let verdict = red_cause(fact.red.as_ref().unwrap());
    assert_eq!(verdict.cause, RedCause::PrCaused);
    assert_eq!(verdict.base_red, ["Lint"]);
    assert_eq!(verdict.reasons.len(), 2);
    let item = item_of(&fact);
    assert!(item.key.ends_with(":rework+base-red"), "{}", item.key);
    assert!(
        item.action.contains("route rework of #7"),
        "{}",
        item.action
    );
    assert!(
        item.action.contains("fix the base branch forward for Lint"),
        "{}",
        item.action
    );
    // Only base-red checks: the base is repaired and the change is left alone.
    let only_base = red_pr(
        8,
        red(
            vec![check("Lint", 10, &["leases::tests::heals"], BaseCheck::Red)],
            &["aida-cli-lib/src/leases.rs"],
        ),
    );
    let item = item_of(&only_base);
    assert!(item.key.ends_with(":base-red"), "{}", item.key);
    assert!(item.action.contains("#8 needs no rework for that"));
    // A base-red check beside an untouched failure that passes on base: the
    // base decides, and nothing is called rework.
    let with_flake = red_pr(
        9,
        red(
            vec![
                check("Lint", 10, &[], BaseCheck::Red),
                check("Test (full)", 11, &["tests::other::x"], BaseCheck::Pass),
            ],
            &["aida-cli-lib/src/leases.rs"],
        ),
    );
    assert_eq!(
        red_cause(with_flake.red.as_ref().unwrap()).cause,
        RedCause::BaseRed
    );
}

#[test]
fn a_flake_reruns_once_then_becomes_a_fix_forward() {
    let policy = Policy::default();
    let mut memory = Memory::default();
    let flake = |id: u64, routed: Option<Vec<u64>>| {
        let mut red = red(
            vec![check(
                "Test (full)",
                id,
                &["tests::other::x"],
                BaseCheck::Pass,
            )],
            &["aida-cli-lib/src/leases.rs"],
        );
        red.rerun_routed = routed;
        red_pr(7, red)
    };
    let report = build_report(&facts(vec![flake(11, None)]), &policy, &mut memory, now());
    assert!(report.stuck[0].key.ends_with(":rerun"));
    let key = rerun_key(7, &format!("{:040x}", 7));
    assert_eq!(memory.reruns[&key], [11]);
    // The same failed run again: nobody reran it yet, still one rerun.
    let routed = memory.reruns.get(&key).cloned();
    let report = build_report(
        &facts(vec![flake(11, routed.clone())]),
        &policy,
        &mut memory,
        now(),
    );
    assert!(report.stuck[0].key.ends_with(":rerun"));
    // A new failed run at the same head: the rerun failed.
    let report = build_report(&facts(vec![flake(12, routed)]), &policy, &mut memory, now());
    let item = &report.stuck[0];
    assert!(item.key.ends_with(":fix-forward"), "{}", item.key);
    assert!(item.action.contains("do not rerun #7 again"));
    assert_eq!(memory.reruns[&key], [11]);
}

/// A forge whose every call costs `cost` on a shared clock, and times out
/// when that is more than the bound it was given.
struct FakeCi<'a> {
    runs: BTreeMap<String, Vec<crate::ci_failures::CheckRun>>,
    clock: &'a Cell<std::time::Duration>,
    cost: std::time::Duration,
    calls: RefCell<Vec<String>>,
    bounds: RefCell<Vec<std::time::Duration>>,
}

impl FakeCi<'_> {
    fn call<T>(
        &self,
        what: String,
        timeout: std::time::Duration,
        answer: impl FnOnce() -> std::result::Result<T, String>,
    ) -> std::result::Result<T, String> {
        self.calls.borrow_mut().push(what);
        self.bounds.borrow_mut().push(timeout);
        if self.cost > timeout {
            self.clock.set(self.clock.get() + timeout);
            return Err("gh timed out".into());
        }
        self.clock.set(self.clock.get() + self.cost);
        answer()
    }

    fn job_calls(&self) -> Vec<String> {
        self.calls
            .borrow()
            .iter()
            .filter(|call| call.starts_with("job:"))
            .cloned()
            .collect()
    }
}

impl CiReads for FakeCi<'_> {
    fn check_runs(
        &self,
        rev: &str,
        timeout: std::time::Duration,
    ) -> std::result::Result<Vec<crate::ci_failures::CheckRun>, String> {
        self.call(format!("runs:{rev}"), timeout, || {
            self.runs
                .get(rev)
                .cloned()
                .ok_or_else(|| "no such commit".to_string())
        })
    }
    fn job_tests(
        &self,
        job: u64,
        timeout: std::time::Duration,
    ) -> std::result::Result<Vec<String>, String> {
        self.call(format!("job:{job}"), timeout, || {
            Ok(vec![format!("tests::job_{job}::fails")])
        })
    }
}

fn check_run(id: u64, name: &str, conclusion: &str) -> crate::ci_failures::CheckRun {
    crate::ci_failures::CheckRun {
        id,
        name: name.into(),
        status: "completed".into(),
        conclusion: conclusion.into(),
        html_url: String::new(),
    }
}

fn fake_ci<'a>(
    clock: &'a Cell<std::time::Duration>,
    cost_secs: u64,
    heads: &[(&str, Vec<crate::ci_failures::CheckRun>)],
) -> FakeCi<'a> {
    let mut runs: BTreeMap<String, Vec<crate::ci_failures::CheckRun>> = heads
        .iter()
        .map(|(head, runs)| (head.to_string(), runs.clone()))
        .collect();
    runs.insert("main".into(), vec![check_run(1, "Test (full)", "success")]);
    FakeCi {
        runs,
        clock,
        cost: std::time::Duration::from_secs(cost_secs),
        calls: RefCell::default(),
        bounds: RefCell::default(),
    }
}

// More red changes than one run reads: every one is examined within a few
// runs, and a log read once is not read again.
#[test]
fn the_red_log_allowance_reaches_every_red_change_across_runs() {
    let policy = Policy::default();
    assert_eq!(policy.max_red_logs, 6);
    let clock = Cell::new(std::time::Duration::ZERO);
    let elapsed = || clock.get();
    let budget = Budget {
        limit: DETAIL_BUDGET,
        elapsed: &elapsed,
    };
    let heads: Vec<(String, String)> = (1..=8u64)
        .map(|n| (format!("PR-{n}@h{n}"), format!("h{n}")))
        .collect();
    let forge_heads: Vec<(&str, Vec<crate::ci_failures::CheckRun>)> = heads
        .iter()
        .enumerate()
        .map(|(n, (_, head))| {
            (
                head.as_str(),
                vec![
                    check_run(100 + n as u64, "Test (full)", "failure"),
                    check_run(200 + n as u64, "Lint", "success"),
                ],
            )
        })
        .collect();
    let ci = fake_ci(&clock, 0, &forge_heads);
    let mut memory = Memory::default();
    let read = |details: &BTreeMap<String, (Vec<FailedCheck>, Option<String>)>| -> Vec<String> {
        details
            .iter()
            .filter(|(_, (checks, _))| checks.iter().all(|check| check.tests.is_ok()))
            .map(|(key, _)| key.clone())
            .collect()
    };
    let first = read_red_details(&ci, &budget, "main", &heads, &mut memory, &policy, now());
    assert_eq!(first.len(), 8);
    assert_eq!(read(&first).len(), 6);
    assert_eq!(ci.job_calls().len(), 6);
    let waiting = &first["PR-7@h7"].0[0];
    assert!(
        waiting.tests.as_ref().unwrap_err().contains("later run"),
        "{waiting:?}"
    );
    assert_eq!(waiting.base, BaseCheck::Pass);
    // The next run reads the two that waited, and only those.
    let later = now() + Duration::minutes(20);
    let second = read_red_details(&ci, &budget, "main", &heads, &mut memory, &policy, later);
    assert_eq!(read(&second).len(), 8);
    assert_eq!(
        ci.job_calls()[6..],
        ["job:106".to_string(), "job:107".to_string()]
    );
    assert_eq!(
        second["PR-1@h1"].0[0].tests,
        Ok(vec!["tests::job_100::fails".to_string()])
    );
    // A third run reads no log at all.
    let third = read_red_details(&ci, &budget, "main", &heads, &mut memory, &policy, later);
    assert_eq!(read(&third).len(), 8);
    assert_eq!(ci.job_calls().len(), 8);
    // A head that is no longer red is forgotten.
    read_red_details(
        &ci,
        &budget,
        "main",
        &heads[..1],
        &mut memory,
        &policy,
        later,
    );
    assert_eq!(memory.red_detail.keys().collect::<Vec<_>>(), ["PR-1@h1"]);
}

// One red head with a hundred failed jobs on a forge where every call takes
// its full bound: the reads stop inside the budget and the rest is unread.
#[test]
fn slow_job_logs_stop_at_the_detail_budget_and_the_rest_is_reported_unread() {
    let policy = Policy::default();
    let clock = Cell::new(std::time::Duration::ZERO);
    let elapsed = || clock.get();
    let budget = Budget {
        limit: DETAIL_BUDGET,
        elapsed: &elapsed,
    };
    let jobs: Vec<crate::ci_failures::CheckRun> = (0..100u64)
        .map(|n| check_run(500 + n, &format!("Test ({n})"), "failure"))
        .collect();
    let ci = fake_ci(&clock, 20, &[("h1", jobs)]);
    let heads = vec![("PR-1@h1".to_string(), "h1".to_string())];
    let mut memory = Memory::default();
    let details = read_red_details(&ci, &budget, "main", &heads, &mut memory, &policy, now());
    // Never past the budget, and every call was bounded by what was left.
    assert!(clock.get() <= DETAIL_BUDGET, "{:?}", clock.get());
    assert_eq!(
        *ci.bounds.borrow(),
        [20, 20, 20, 15].map(std::time::Duration::from_secs)
    );
    assert_eq!(ci.job_calls(), ["job:500", "job:501"]);
    // What was read is kept; the rest says why it was not.
    let (checks, unread) = &details["PR-1@h1"];
    assert_eq!(unread, &None);
    assert_eq!(checks.len(), 100);
    assert!(checks[0].tests.is_ok());
    assert_eq!(checks[1].tests, Err("gh timed out".to_string()));
    assert!(checks[2..].iter().all(|check| check
        .tests
        .as_ref()
        .is_err_and(|why| why.contains("time budget"))));
    // The partial evidence still classifies, and is never called a flake.
    let fact = red_pr(
        1,
        RedFact {
            checks: checks.clone(),
            changed_paths: Ok(vec!["docs/x.md".into()]),
            rerun_routed: None,
            unread: None,
        },
    );
    assert!(item_of(&fact).key.ends_with(":inspect"));
    assert_eq!(
        memory.red_detail["PR-1@h1"].checks[0].tests,
        checks[0].tests
    );
    // With the budget spent before the first call nothing is asked at all.
    let asked = ci.calls.borrow().len();
    let details = read_red_details(&ci, &budget, "main", &heads, &mut memory, &policy, now());
    assert_eq!(ci.calls.borrow().len(), asked);
    assert!(details["PR-1@h1"]
        .1
        .as_ref()
        .unwrap()
        .contains("time budget"));
    // A base check that fails, passes or is absent at the base's tip.
    let tip = Ok(vec![
        check_run(1, "A", "failure"),
        check_run(2, "B", "success"),
        check_run(3, "C", "skipped"),
    ]);
    assert_eq!(base_check(&tip, "A"), BaseCheck::Red);
    assert_eq!(base_check(&tip, "B"), BaseCheck::Pass);
    assert!(matches!(base_check(&tip, "C"), BaseCheck::Unknown(_)));
    assert!(matches!(base_check(&tip, "D"), BaseCheck::Unknown(_)));
    assert_eq!(
        base_check(&Err("offline".into()), "A"),
        BaseCheck::Unknown("offline".into())
    );
}

#[test]
fn the_report_counts_landings_lists_hosts_and_escalates_only_after_two_dry_hours_with_stalls() {
    let policy = Policy::default();
    let stuck = || vec![fact(1, &approved(45))];
    let moving = || {
        vec![fact(
            2,
            &Fake {
                reviewing: Ok(Some("reviewer@dell1".into())),
                ..Fake::default()
            },
        )]
    };
    let with_landings = |prs, landings: Vec<DateTime<Utc>>| Facts {
        prs,
        last_landing: landings.iter().max().copied().or(Some(ago(300))),
        landings: Some(Ok(landings)),
        hosts: vec![HostLine {
            host: "dell1".into(),
            lanes: 4,
            memory: "20.0 GiB available of 64.0 GiB".into(),
            ..HostLine::default()
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

// A scheduled run with nothing stuck still saves what it observed.
#[test]
fn a_scheduled_run_saves_landings_and_each_hosts_lanes_and_memory_even_with_no_stall() {
    let tmp = tempfile::tempdir().unwrap();
    let policy = Policy::default();
    let mut memory = Memory::default();
    let quiet = Facts {
        landings: Some(Ok(vec![ago(10), ago(50)])),
        last_landing: Some(ago(10)),
        hosts: vec![
            HostLine {
                host: "dell1".into(),
                lanes: 4,
                memory: "20.0 GiB available of 64.0 GiB".into(),
                ..HostLine::default()
            },
            HostLine {
                host: "disrad-x".into(),
                lanes: 1,
                memory: NO_REMOTE_MEMORY.into(),
                ..HostLine::default()
            },
        ],
        ..Facts::default()
    };
    let report = build_report(&quiet, &policy, &mut memory, now());
    assert!(report.stuck.is_empty());
    route(tmp.path(), &report, &mut memory, &policy, now());
    record_run(&mut memory, &report);
    save_memory(tmp.path(), &memory);
    let saved = load_memory(tmp.path());
    assert_eq!(saved, memory);
    let run = &saved.runs[0];
    assert_eq!(run.at, now());
    assert_eq!(run.landings.count, Some(2));
    assert_eq!(run.landings.last_mins_ago, Some(10));
    assert_eq!((run.changes, run.stuck), (0, 0));
    assert_eq!(run.hosts[0].lanes, 4);
    assert!(run.hosts[0].memory.contains("20.0 GiB available"));
    assert_eq!(run.hosts[1].host, "disrad-x");
    let line = snapshot_line(run);
    assert!(line.contains("2 landing(s) in 60 min"), "{line}");
    assert!(line.contains("dell1: 4 lane(s), memory 20.0 GiB"), "{line}");
    // `aida land stalls` shows the newest saved run.
    let mut shown = report.clone();
    shown.last_scheduled_run = saved.runs.last().cloned();
    assert!(render(&shown).contains("Last scheduled run 2026-11-20 12:00 UTC: 2 landing(s)"));
    assert_eq!(
        serde_json::to_value(&shown).unwrap()["last_scheduled_run"]["hosts"][0]["lanes"],
        4
    );
    // The history is bounded.
    for _ in 0..200 {
        record_run(&mut memory, &report);
    }
    assert_eq!(memory.runs.len(), MAX_RUNS);
}

fn two_stalls() -> Vec<PrFact> {
    vec![fact(1, &approved(45)), fact(2, &Fake::default())]
}

#[test]
fn routing_is_one_item_per_seat_and_the_same_stall_is_never_routed_twice() {
    let policy = Policy::default();
    let mut memory = Memory::default();
    let mut facts = facts(two_stalls());
    let report = build_report(&facts, &policy, &mut memory, now());
    let plan = plan_routes(&report, &memory, &policy, now());
    assert_eq!(
        plan.seats.keys().collect::<Vec<_>>(),
        ["advisor"],
        "one consolidated item per seat"
    );
    assert_eq!(plan.seats["advisor"].all.len(), 2);
    assert_eq!(plan.seats["advisor"].new.len(), 2);
    assert!(!plan.escalate);
    // Everything was routed: the next run wakes no one.
    for item in &report.stuck {
        memory.routed.insert(item.key.clone(), now());
    }
    assert!(plan_routes(&report, &memory, &policy, now())
        .seats
        .is_empty());
    // A new stall wakes its seat with all of the seat's current stalls.
    facts.prs.push(fact(
        3,
        &Fake {
            conflict: Ok(true),
            conflict_since: stamp(45),
            ..Fake::default()
        },
    ));
    let report = build_report(&facts, &policy, &mut memory, now());
    let plan = plan_routes(&report, &memory, &policy, now());
    assert_eq!(plan.seats["advisor"].all.len(), 3);
    assert_eq!(plan.seats["advisor"].new.len(), 1);
    assert_eq!(plan.seats["advisor"].new[0].pr, Some(3));
    // The same change with a new head is a new blocker.
    let mut pushed = fact(1, &approved(45));
    pushed.head = "f".repeat(40);
    let report = build_report(&self::facts(vec![pushed]), &policy, &mut memory, now());
    assert_eq!(
        plan_routes(&report, &memory, &policy, now()).seats["advisor"]
            .new
            .len(),
        1
    );
}

#[test]
fn the_operator_is_escalated_to_once_per_window() {
    let policy = Policy::default();
    let mut memory = Memory::default();
    let mut dry = facts(vec![fact(1, &approved(45))]);
    dry.landings = Some(Ok(Vec::new()));
    let report = build_report(&dry, &policy, &mut memory, now());
    assert!(plan_routes(&report, &memory, &policy, now()).escalate);
    memory.escalated_at = Some(now());
    let later = now() + Duration::minutes(20);
    assert!(!plan_routes(&report, &memory, &policy, later).escalate);
    let much_later = now() + Duration::minutes(policy.escalate_after_mins);
    assert!(plan_routes(&report, &memory, &policy, much_later).escalate);
}

fn guard_mail(root: &Path) -> Vec<aida_core::mailbox::Message> {
    crate::mailbox_store::read_local_messages(root)
        .unwrap()
        .into_iter()
        .filter(|m| m.from == SENDER)
        .collect()
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
    let mut dry = facts(two_stalls());
    dry.landings = Some(Ok(Vec::new()));
    let report = build_report(&dry, &policy, &mut memory, now());
    route(root, &report, &mut memory, &policy, now());

    let advisor = schedule_ledger::load(&store, &route_job_name("advisor")).unwrap();
    assert_eq!(advisor.due_since, Some(now()));
    let note = advisor.note.unwrap();
    assert!(note.contains("[STUCK-MERGE] #1"), "{note}");
    assert!(note.contains("[STUCK-REVIEW] #2"), "{note}");
    let operator = schedule_ledger::load(&store, &route_job_name("operator")).unwrap();
    assert!(operator
        .note
        .unwrap()
        .contains("no landing for 120 min with 2 stall(s)"));
    assert_eq!(memory.escalated_at, Some(now()));
    assert_eq!(memory.routed.len(), 2);
    assert!(memory.mail_pending.is_empty());
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
    let sent = guard_mail(root);
    assert_eq!(sent.len(), 1, "one mail wake per routed seat");
    assert!(matches!(
        &sent[0].to,
        aida_core::mailbox::Recipient::Agent(seat) if seat == "advisor"
    ));
    assert!(sent[0].body.contains("[STUCK-REVIEW] #2"));

    // The next run, 20 minutes on, with the same stalls: nothing new is sent.
    let later = now() + Duration::minutes(20);
    let report = build_report(&dry, &policy, &mut memory, later);
    route(root, &report, &mut memory, &policy, later);
    assert_eq!(
        guard_mail(root).len(),
        1,
        "no second wake for the same stalls"
    );
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

// The due job is written but the mailbox fails: the wake is still owed and
// goes out on the next run, with no second due job.
#[test]
fn a_mail_wake_that_failed_is_sent_again_on_the_next_run() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let store = root.join(".aida-store");
    let policy = Policy::default();
    let mut memory = Memory::default();
    let stalls = facts(two_stalls());
    let report = build_report(&stalls, &policy, &mut memory, now());
    let broken: Mailer = &|_, _| anyhow::bail!("mailbox is read-only");
    route_with(root, &report, &mut memory, &policy, now(), broken);
    let due = |store: &Path| {
        schedule_ledger::load(store, &route_job_name("advisor"))
            .unwrap()
            .due_since
    };
    assert_eq!(due(&store), Some(now()));
    assert_eq!(memory.routed.len(), 2, "the due job was delivered");
    assert_eq!(memory.mail_pending.len(), 2, "the wake was not");
    assert!(guard_mail(root).is_empty());
    // The plan asks for the mail again, and for no new route.
    let later = now() + Duration::minutes(20);
    let report = build_report(&stalls, &policy, &mut memory, later);
    let plan = plan_routes(&report, &memory, &policy, later);
    assert!(plan.seats["advisor"].new.is_empty());
    assert_eq!(plan.seats["advisor"].remail.len(), 2);
    // Still broken: still owed.
    route_with(root, &report, &mut memory, &policy, later, broken);
    assert_eq!(memory.mail_pending.len(), 2);
    // The mailbox works again: one mail, and the due job is not re-raised.
    route(root, &report, &mut memory, &policy, later);
    let sent = guard_mail(root);
    assert_eq!(sent.len(), 1);
    assert!(
        sent[0].body.contains("[STUCK-MERGE] #1"),
        "{}",
        sent[0].body
    );
    assert!(memory.mail_pending.is_empty());
    assert_eq!(due(&store), Some(now()));
    assert_eq!(memory.routed.values().copied().max(), Some(now()));
    // Delivered: the next run sends nothing more.
    let last = later + Duration::minutes(20);
    let report = build_report(&stalls, &policy, &mut memory, last);
    assert!(plan_routes(&report, &memory, &policy, last)
        .seats
        .is_empty());
    route(root, &report, &mut memory, &policy, last);
    assert_eq!(guard_mail(root).len(), 1);
    // A stall that clears while its mail is owed is forgotten with it.
    let mut owed = Memory::default();
    let report = build_report(&stalls, &policy, &mut owed, now());
    route_with(root, &report, &mut owed, &policy, now(), broken);
    let report = build_report(&facts(Vec::new()), &policy, &mut owed, now());
    route_with(root, &report, &mut owed, &policy, now(), broken);
    assert!(owed.mail_pending.is_empty() && owed.routed.is_empty());
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
        fact(1, &approved(45)),
        fact(
            2,
            &Fake {
                reviewing: Ok(Some("reviewer@dell1".into())),
                ..Fake::default()
            },
        ),
    ]);
    facts.unknown = vec!["this clone's leases could not be read".into()];
    let report = build_report(&facts, &Policy::default(), &mut Memory::default(), now());
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["changes"][0]["class"], "STUCK-MERGE");
    assert_eq!(json["changes"][0]["state"], "READY");
    assert_eq!(json["changes"][0]["age_mins"], 45);
    assert_eq!(json["changes"][1]["class"], "moving");
    assert_eq!(json["changes"][1]["state"], "REVIEWING");
    assert_eq!(json["stuck"][0]["class"], "STUCK-MERGE");
    assert_eq!(json["stuck"][0]["seat"], "advisor");
    assert_eq!(json["landings"]["count"], 1);
    assert_eq!(json["escalate"], false);
    assert!(json.get("last_scheduled_run").is_none());
    let text = render(&report);
    assert!(text.contains("#1      STUCK-MERGE"), "{text}");
    assert!(text.contains("#2      moving"), "{text}");
    assert!(text.contains("Stuck (1):"), "{text}");
    assert!(text.contains("-> advisor: land #1"), "{text}");
    assert!(text.contains("Landings in the last 60 min: 1"), "{text}");
    assert!(text.contains("Not read: this clone's leases"), "{text}");
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
fn the_guards_memory_round_trips_and_an_older_file_still_loads() {
    let mut memory = Memory::default();
    memory
        .routed
        .insert("STUCK-MERGE:PR-1@00000000".into(), now());
    memory
        .mail_pending
        .insert("STUCK-MERGE:PR-1@00000000".into());
    memory.reruns.insert("PR-1@00000000".into(), vec![11]);
    memory.red_detail.insert(
        "PR-1@00000000".into(),
        RedDetail {
            at: now(),
            checks: vec![
                check("Test (full)", 11, &["a::b"], BaseCheck::default()),
                FailedCheck {
                    name: "Lint".into(),
                    id: 12,
                    tests: Err("gh timed out".into()),
                    base: BaseCheck::default(),
                },
            ],
        },
    );
    let text = serde_json::to_string(&memory).unwrap();
    assert_eq!(serde_json::from_str::<Memory>(&text).unwrap(), memory);
    assert_eq!(
        serde_json::from_str::<Memory>("{}").unwrap(),
        Memory::default()
    );
    // Keys an earlier build wrote are ignored.
    assert_eq!(
        serde_json::from_str::<Memory>(r#"{"flakes": [], "seen_runs": {}}"#).unwrap(),
        Memory::default()
    );
    // The seat a route job names.
    assert_eq!(
        seat_for_job("throughput-guard-route-advisor"),
        Some("advisor")
    );
    assert_eq!(seat_for_job("throughput-guard-route-"), None);
    assert!(route_prompt("advisor").contains("aida schedule done throughput-guard-route-advisor"));
    assert!(route_prompt("operator").contains("Nothing has landed"));
}

// Red checks at the exact head are red whatever is known of the base: the
// reconciler's CI answer turns stale or unreadable once the base moves.
#[test]
fn approved_red_head_is_stuck_merge_red_when_the_base_is_stale_or_unknown() {
    let stale = "CI run 20 tested base bbbbbbbb, current base is cccccccc";
    let unread = "CI run 20's tested base is unknown: logs expired";
    for (ci, why) in [
        (Ok(Ci::StaleBase(stale.into())), stale),
        (Err(unread.to_string()), unread),
    ] {
        let fake = Fake {
            ci,
            head_red: Some(Ok(Some("Test (full)".into()))),
            ..approved(45)
        };
        let mut fact = fact(4344, &fake);
        assert!(fact.wants_red_detail(), "{:?}", fact.standing);
        let unexamined = item_of(&fact);
        assert_eq!(unexamined.class, Class::StuckMergeRed);
        fact.red = Some(red(
            vec![check(
                "Test (full)",
                11,
                &["tests::task_3792_tests::retries"],
                BaseCheck::Pass,
            )],
            &["aida-cli-lib/src/leases.rs"],
        ));
        let item = item_of(&fact);
        assert_eq!(item.class, Class::StuckMergeRed);
        assert!(item.summary.contains("Test (full)"), "{}", item.summary);
        assert!(
            item.summary.contains(&format!("base not fresh: {why}")),
            "{}",
            item.summary
        );
        assert!(item
            .summary
            .contains("failing tests: tests::task_3792_tests::retries"));
        // The same stall whatever the base says: routed once.
        assert_eq!(
            item.key,
            item_of(&red_pr(4344, fact.red.clone().unwrap())).key
        );
    }
}

// A stale or unreadable base with no failed check at the head is not red.
#[test]
fn a_stale_or_unknown_base_without_failed_checks_is_not_called_red() {
    let stale = Fake {
        ci: Ok(Ci::StaleBase("CI run 20 tested an older base".into())),
        head_red: Some(Ok(None)),
        ..approved(45)
    };
    let fact_stale = fact(7, &stale);
    assert!(!fact_stale.wants_red_detail());
    assert_eq!(
        class_of(&fact_stale, &Policy::default()),
        Some(Class::StuckMerge)
    );
    for head_red in [Ok(None), Err("the rollup was not read".to_string())] {
        let unread = Fake {
            ci: Err("logs expired".into()),
            head_red: Some(head_red),
            ..approved(45)
        };
        assert!(matches!(
            fact(8, &unread).standing,
            Standing::Unknown(what) if what.contains("CI unknown (logs expired)")
        ));
    }
    let unreadable_head = Fake {
        ci: Ok(Ci::StaleBase("CI run 20 tested an older base".into())),
        head_red: Some(Err("the rollup was not read".into())),
        ..approved(45)
    };
    assert!(matches!(
        fact(9, &unreadable_head).standing,
        Standing::Unknown(_)
    ));
}

// This host's memory is measured; another host's is unknown with the reason
// unless the default branch's configuration names it for a probe (the probe
// and the lane counts are covered in task_3827_throughput_guard_tests.rs).
#[test]
fn host_lines_measure_local_memory_and_say_why_a_foreign_hosts_is_unknown() {
    let local = || "20.0 GiB available of 64.0 GiB".to_string();
    let never = |_: &str| -> std::result::Result<String, String> { panic!("probed") };
    let others: BTreeMap<String, crate::lane_hosts::HostCount> = [
        (
            "disrad-x".to_string(),
            crate::lane_hosts::HostCount {
                reporting: 2,
                silent: 0,
                invalid: 0,
                newest_report_mins: Some(3),
            },
        ),
        (
            "other".to_string(),
            crate::lane_hosts::HostCount {
                reporting: 1,
                silent: 0,
                invalid: 0,
                newest_report_mins: Some(0),
            },
        ),
    ]
    .into();
    // A host the project did not name is never probed.
    let lines = host_lines_with("dell1", 4, &others, &Policy::default(), &local, &never);
    let line = |host: &str| lines.iter().find(|line| line.host == host).unwrap();
    assert_eq!(lines.len(), 3);
    assert_eq!((line("dell1").lanes, line("disrad-x").lanes), (4, 2));
    assert_eq!(line("dell1").memory, "20.0 GiB available of 64.0 GiB");
    assert_eq!(line("disrad-x").memory, "unknown (no remote memory source)");
    assert_eq!(line("other").memory, NO_REMOTE_MEMORY);
    assert_eq!(remote_memory(&Policy::default(), "disrad-x", &never), None);
    let report = Report {
        hosts: lines,
        ..build_report(
            &facts(Vec::new()),
            &Policy::default(),
            &mut Memory::default(),
            now(),
        )
    };
    assert!(
        render(&report).contains("Host other: 1 lane(s) by the host's own records, newest report 0 min ago; memory unknown (no remote memory source)")
    );
}
