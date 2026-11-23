// trace:STORY-2856 | ai:claude
use super::*;
use serde_json::json;
use std::os::unix::process::ExitStatusExt;

const GATE: u64 = 359309600;
const T0: i64 = 1_763_308_800;

fn at(secs: i64) -> String {
    DateTime::from_timestamp(T0 + secs, 0)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// API-shaped `workflow_runs` row. `event` decides provenance, mirroring
/// run 43924182664: `pull_request_target`, source `head_branch`, empty
/// `pull_requests`.
fn api_run(id: u64, event: &str, status: &str, conclusion: Option<&str>, created: i64) -> Value {
    json!({
        "id": id,
        "event": event,
        "head_branch": if event == "push" { "main" } else { "bug-379" },
        "status": status,
        "conclusion": conclusion,
        "created_at": at(created),
        "html_url": format!("https://github.com/o/r/actions/runs/{id}"),
        "pull_requests": [],
    })
}
fn sf(id: u64, created: i64) -> Value {
    api_run(
        id,
        "pull_request_target",
        "completed",
        Some("startup_failure"),
        created,
    )
}
fn ok(id: u64, created: i64) -> Value {
    api_run(
        id,
        "pull_request_target",
        "completed",
        Some("success"),
        created,
    )
}

#[derive(Default)]
struct Forge {
    responses: BTreeMap<String, Value>,
    reviews: BTreeMap<u64, &'static str>,
    budget: Option<usize>,
    review_heads: BTreeMap<u64, String>,
    calls: Vec<String>,
    reviewed: Vec<u64>,
    sent: Vec<(String, String, String)>,
    persisted: usize,
    fail_push: bool,
    channel_attempts: Vec<(&'static str, String)>,
}
impl Forge {
    fn new(required: &[&str], workflows: Value) -> Self {
        let mut forge = Self::default();
        forge.set("repos/{owner}/{repo}", json!({"default_branch": "main"}));
        forge.set(
            "repos/{owner}/{repo}/branches/main/protection",
            json!({"required_status_checks": {"contexts": required}}),
        );
        forge.set(
            "repos/{owner}/{repo}/actions/workflows?per_page=100",
            json!({ "workflows": workflows }),
        );
        forge
    }
    fn set(&mut self, endpoint: &str, value: Value) {
        self.responses.insert(endpoint.to_owned(), value);
    }
    fn runs(&mut self, workflow: u64, runs: Vec<Value>) {
        self.set(
            &format!("repos/{{owner}}/{{repo}}/actions/workflows/{workflow}/runs?per_page=100"),
            json!({ "workflow_runs": runs }),
        );
    }
    /// One observation pass through the production runner.
    fn pass(&mut self, state: &mut State, config: &Config, now: i64) -> Result<()> {
        self.calls.clear();
        let (responses, budget, calls) = (&self.responses, self.budget, &mut self.calls);
        let spent = std::cell::Cell::new(0);
        let spend = || -> Result<()> {
            if budget.is_some_and(|b| spent.get() >= b) {
                return Err(BudgetExhausted.into());
            }
            spent.set(spent.get() + 1);
            Ok(())
        };
        let mut api = |endpoint: &str| -> Result<Output> {
            spend()?;
            calls.push(endpoint.to_owned());
            let value = if endpoint.starts_with("repos/{owner}/{repo}/rules/branches/") {
                json!([])
            } else {
                responses
                    .get(endpoint)
                    .cloned()
                    .with_context(|| format!("unexpected endpoint {endpoint}"))?
            };
            Ok(Output {
                status: std::process::ExitStatus::from_raw(0),
                stdout: serde_json::to_vec(&value)?,
                stderr: Vec::new(),
            })
        };
        let (reviews, reviewed) = (&self.reviews, &mut self.reviewed);
        let review_heads = &self.review_heads;
        let mut review = |number: u64, expected_head: &str| -> Result<Option<String>> {
            spend()?;
            reviewed.push(number);
            let head = review_heads
                .get(&number)
                .cloned()
                .unwrap_or_else(|| format!("s{number}"));
            Ok((head == expected_head).then(|| {
                reviews
                    .get(&number)
                    .copied()
                    .unwrap_or("REVIEW_REQUIRED")
                    .into()
            }))
        };
        let sent = &mut self.sent;
        let attempts = &mut self.channel_attempts;
        let fail_push = self.fail_push;
        let mut send = |rule: &str,
                        title: &str,
                        message: &str,
                        accepted: &mut BTreeSet<String>|
         -> Result<bool> {
            sent.push((rule.into(), title.into(), message.into()));
            for channel in ["notify", "push"] {
                if !accepted.contains(channel) {
                    attempts.push((channel, title.into()));
                    if channel != "push" || !fail_push {
                        accepted.insert(channel.into());
                    }
                }
            }
            Ok(accepted.len() == 2)
        };
        let persisted = &mut self.persisted;
        let mut persist = |state: &State| -> Result<()> {
            // Round-trip like the state file does between processes.
            serde_json::from_slice::<State>(&serde_json::to_vec(state)?)?;
            *persisted += 1;
            Ok(())
        };
        let mut port = Port {
            api: &mut api,
            review: &mut review,
            send: &mut send,
            persist: &mut persist,
        };
        observe(&mut port, config, state, now)
    }
}

fn reload(state: &State) -> State {
    serde_json::from_slice(&serde_json::to_vec(state).unwrap()).unwrap()
}

// AC1: protection discovery, PR-target provenance, workflow/job mapping,
// blocked approved-PR count, payload, transport and persistence.
// trace:STORY-2856 | ai:claude
#[test]
fn story2856_ac1_pr_target_startup_failures_alert_end_to_end() {
    let mut forge = Forge::new(
        &["merge-hold-gate", "build"],
        json!([{"id": 7, "name": "ci"}, {"id": GATE, "name": "merge-hold-gate"}]),
    );
    let mut foreign_push = ok(10, 3000);
    foreign_push["event"] = "push".into();
    foreign_push["head_branch"] = "feature".into();
    let mut release_pr = ok(9, 2000);
    release_pr["pull_requests"] = json!([{"number": 70, "base": {"ref": "release"}}]);
    forge.runs(
        GATE,
        vec![
            sf(13, 7199),
            foreign_push,
            sf(12, 3600),
            release_pr,
            sf(11, 60),
            ok(8, -3600),
        ],
    );
    forge.set(
        "repos/{owner}/{repo}/actions/runs/8/jobs?per_page=100",
        json!({"total_count": 1, "jobs": [{"name": "gate"}]}),
    );
    forge.runs(
        7,
        vec![api_run(20, "push", "completed", Some("success"), 100)],
    );
    forge.set(
        "repos/{owner}/{repo}/actions/runs/20/jobs?per_page=100",
        json!({"total_count": 1, "jobs": [{"name": "build"}]}),
    );
    let pr = |n: u64, draft: bool, base: &str| json!({"number": n, "draft": draft, "base": {"ref": base}, "head": {"sha": format!("s{n}")}});
    forge.set(
        "repos/{owner}/{repo}/pulls?state=open&per_page=100",
        json!([
            pr(1, false, "main"),
            pr(2, false, "main"),
            pr(3, true, "main"),
            pr(4, false, "release"),
            pr(5, false, "main")
        ]),
    );
    for n in [1, 2, 5] {
        forge.set(
            &format!("repos/{{owner}}/{{repo}}/commits/s{n}/check-runs?per_page=100"),
            json!({"check_runs": []}),
        );
        let statuses = if n == 2 {
            json!([{"id": 1, "context": "merge-hold-gate", "state": "success"}])
        } else {
            json!([])
        };
        forge.set(
            &format!("repos/{{owner}}/{{repo}}/commits/s{n}/status?per_page=100"),
            json!({ "statuses": statuses }),
        );
    }
    forge.reviews.insert(1, "APPROVED");
    let mut state = State::new();
    let now = T0 + 7200;
    forge.pass(&mut state, &Config::default(), now).unwrap();

    assert!(
        !forge.calls.iter().any(|c| c.contains("branch=")),
        "PR-target runs carry source branches; never filter runs by head branch"
    );
    assert_eq!(forge.sent.len(), 1, "{:?}", forge.sent);
    let (_, title, message) = &forge.sent[0];
    assert_eq!(title, "Required CI never started");
    for needle in [
        "Workflow: merge-hold-gate",
        "Required checks: merge-hold-gate",
        "Latest run: https://github.com/o/r/actions/runs/13",
        &format!("First failure: {}\n", at(60)),
        "Approved PRs blocked by missing check: 1",
    ] {
        assert!(
            message.contains(needle),
            "{needle:?} missing from {message}"
        );
    }
    assert_eq!(
        forge.reviewed,
        [1, 5],
        "only PRs lacking the check are reviewed"
    );
    assert!(forge.persisted >= 1);
    let mut state = reload(&state);
    let gate = &state[&GATE.to_string()];
    assert_eq!(
        (gate.notices, gate.latest_failure, gate.next),
        (1, 13, now + 300)
    );
    assert_eq!(state["7"].jobs, ["build"]);

    // Within the backoff window: no repeat, and the cached job mapping is
    // not re-read.
    forge
        .pass(&mut state, &Config::default(), now + 299)
        .unwrap();
    assert_eq!(forge.sent.len(), 1);
    assert!(!forge.calls.iter().any(|c| c.contains("/jobs")));
    forge
        .pass(&mut state, &Config::default(), now + 300)
        .unwrap();
    assert_eq!(forge.sent.len(), 2, "one notice per backoff window");
}

// AC2: an intervening success under newer queued/running runs recovers
// exactly once, and a later outage is a new episode with a fresh start and
// reset backoff.
// trace:STORY-2856 | ai:claude
#[test]
fn story2856_ac2_recovery_survives_newer_runs_and_resets_episode() {
    let mut forge = Forge::new(
        &["merge-hold-gate"],
        json!([{"id": GATE, "name": "merge-hold-gate"}]),
    );
    forge.set(
        "repos/{owner}/{repo}/pulls?state=open&per_page=100",
        json!([]),
    );
    forge.runs(GATE, vec![sf(3, 300), sf(2, 200), sf(1, 100)]);
    let config = Config::default();
    let mut state = State::new();
    forge.pass(&mut state, &config, T0 + 400).unwrap();
    assert_eq!(forge.sent.len(), 1);

    let mut running = ok(5, 1000);
    running["status"] = "in_progress".into();
    running["conclusion"] = Value::Null;
    let queued = api_run(6, "pull_request_target", "queued", None, 1990);
    forge.set(
        "repos/{owner}/{repo}/actions/runs/5/jobs?per_page=100",
        json!({"total_count": 1, "jobs": [{"name": "gate"}]}),
    );
    let history = vec![
        queued,
        running,
        ok(4, 900),
        sf(3, 300),
        sf(2, 200),
        sf(1, 100),
    ];
    forge.runs(GATE, history.clone());
    // The latest run is queued; the PR inventory is never read for recovery.
    forge
        .responses
        .remove("repos/{owner}/{repo}/pulls?state=open&per_page=100");
    for now in [T0 + 2000, T0 + 2001] {
        forge.pass(&mut state, &config, now).unwrap();
    }
    assert_eq!(
        forge.sent.len(),
        2,
        "exactly one recovery: {:?}",
        forge.sent
    );
    assert_eq!(forge.sent[1].1, "Required CI recovered");
    assert!(forge.sent[1].2.contains("A run succeeded again."));
    let gate = &state[&GATE.to_string()];
    assert_eq!((gate.notices, gate.next, gate.first.as_str()), (0, 0, ""));

    forge.set(
        "repos/{owner}/{repo}/pulls?state=open&per_page=100",
        json!([]),
    );
    let mut outage = vec![sf(9, 3200), sf(8, 3100), sf(7, 3000)];
    outage.extend(history[1..].iter().cloned());
    forge.runs(GATE, outage);
    forge.pass(&mut state, &config, T0 + 3300).unwrap();
    assert_eq!(forge.sent.len(), 3);
    assert!(forge.sent[2]
        .2
        .contains(&format!("First failure: {}\n", at(3000))));
    assert_eq!(state[&GATE.to_string()].next, T0 + 3300 + 300);
}

// AC1: the earliest failure spans both never-started kinds, and a streak that
// fills the visible history says its start may be earlier.
// trace:STORY-2856 | ai:claude
#[test]
fn story2856_ac1_queued_and_mixed_history_report_earliest_failure() {
    let config = Config::default();
    let now = T0 + 100_000;
    let queued =
        |id: u64, created: i64| api_run(id, "pull_request_target", "queued", None, created);
    let only_queued: Vec<Value> = (1..=10).rev().map(|i| queued(i, i as i64 * 10)).collect();
    let mut episode = Episode::default();
    assert_eq!(
        evaluate(&mut episode, &only_queued, true, &config, now).unwrap(),
        Some(false)
    );
    assert_eq!(
        (episode.first.as_str(), episode.first_partial),
        (at(10).as_str(), false)
    );

    let mut mixed: Vec<Value> = (1..=10)
        .rev()
        .map(|i| {
            if i % 2 == 0 {
                queued(i, i as i64 * 10)
            } else {
                sf(i, i as i64 * 10)
            }
        })
        .collect();
    mixed.push(ok(0, 0));
    let mut episode = Episode::default();
    evaluate(&mut episode, &mixed, false, &config, now).unwrap();
    assert_eq!(
        (episode.first.as_str(), episode.first_partial),
        (at(10).as_str(), false)
    );

    // A fresh queued run is unresolved: it neither hides the outage nor counts.
    let mut fresh = vec![queued(11, 99_990)];
    fresh.extend(mixed.iter().cloned());
    let mut episode = Episode::default();
    assert_eq!(
        evaluate(&mut episode, &fresh, false, &config, now).unwrap(),
        Some(false)
    );
    assert_eq!(episode.latest_failure, 10);

    let mut forge = Forge::new(
        &["merge-hold-gate"],
        json!([{"id": GATE, "name": "merge-hold-gate"}]),
    );
    forge.set(
        "repos/{owner}/{repo}/pulls?state=open&per_page=100",
        json!([]),
    );
    forge.runs(GATE, (1..=100).rev().map(|i| sf(i, i as i64)).collect());
    // Fresh state looks past the full page for a started run to map jobs
    // (BUG-2689); none exists, so the first failure stays partial.
    forge.set(
        &format!("repos/{{owner}}/{{repo}}/actions/workflows/{GATE}/runs?per_page=99&page=2"),
        json!({"workflow_runs": []}),
    );
    let mut state = State::new();
    forge.pass(&mut state, &config, now).unwrap();
    assert!(
        forge.sent[0]
            .2
            .contains(&format!("First failure: {} or earlier", at(1))),
        "{}",
        forge.sent[0].2
    );
}

// AC1: budget-exhausted passes keep their progress, so a required workflow
// listed after unrelated ones is still reached and alerts.
// trace:STORY-2856 | ai:claude
#[test]
fn story2856_ac1_budget_exhausted_passes_eventually_alert() {
    let mut workflows: Vec<Value> = (100..106)
        .map(|id| json!({"id": id, "name": format!("lint{id}")}))
        .collect();
    workflows.push(json!({"id": GATE, "name": "merge-hold-gate"}));
    let mut forge = Forge::new(&["merge-hold-gate"], json!(workflows));
    for id in 100..106 {
        let started = 1000 + id;
        let runs = vec![
            sf(2000 + id, 500),
            api_run(started, "push", "completed", Some("success"), 400),
        ];
        forge.runs(id, runs);
        forge.set(
            &format!("repos/{{owner}}/{{repo}}/actions/runs/{started}/jobs?per_page=100"),
            json!({"total_count": 1, "jobs": [{"name": "lint"}]}),
        );
    }
    forge.runs(GATE, vec![sf(3, 300), sf(2, 200), sf(1, 100)]);
    forge.set(
        "repos/{owner}/{repo}/pulls?state=open&per_page=100",
        json!([]),
    );
    // Repo, protection, one rules page and the workflow list leave four reads.
    forge.budget = Some(8);
    let mut state = State::new();
    let mut exhausted = 0;
    for pass in 0..6 {
        if let Err(error) = forge.pass(&mut state, &Config::default(), T0 + 1000 + pass) {
            assert!(
                error.downcast_ref::<BudgetExhausted>().is_some(),
                "{error:#}"
            );
            exhausted += 1;
        }
        state = reload(&state);
        if !forge.sent.is_empty() {
            break;
        }
    }
    assert!(exhausted >= 1, "the fixture must exhaust at least one pass");
    assert_eq!(
        forge.sent.len(),
        1,
        "the required workflow is eventually observed"
    );
    assert!(forge.sent[0].2.contains("Workflow: merge-hold-gate"));
}

// AC1: a serial PR inventory takes multiple budgets, including a pause
// between check/status evidence and the corresponding review decision.
#[test]
fn story2856_ac1_resumes_blocked_pr_count_across_budgets() {
    let mut forge = Forge::new(
        &["merge-hold-gate"],
        json!([{"id": GATE, "name": "merge-hold-gate"}, {"id": 7, "name": "lint"}]),
    );
    forge.runs(GATE, vec![sf(3, 300), sf(2, 200), sf(1, 100)]);
    forge.runs(7, vec![]);
    let pulls = "repos/{owner}/{repo}/pulls?state=open&per_page=100";
    forge.set(
        pulls,
        json!((1..=4)
            .map(|n| json!({
                "number": n, "draft": false, "base": {"ref": "main"},
                "head": {"sha": format!("s{n}")}
            }))
            .collect::<Vec<_>>()),
    );
    for n in 1..=4 {
        forge.set(
            &format!("repos/{{owner}}/{{repo}}/commits/s{n}/check-runs?per_page=100"),
            json!({"check_runs": []}),
        );
        let statuses = if n == 2 {
            json!([{"id": 1, "context": "merge-hold-gate", "state": "success"}])
        } else {
            json!([])
        };
        forge.set(
            &format!("repos/{{owner}}/{{repo}}/commits/s{n}/status?per_page=100"),
            json!({"statuses": statuses}),
        );
    }
    forge.reviews.insert(1, "APPROVED");
    forge.reviews.insert(4, "APPROVED");
    forge.budget = Some(8);
    let config = Config::default();
    let mut state = State::new();
    for window in 0..2 {
        let mut inventory_reads = 0;
        for pass in 0..12 {
            let result = forge.pass(&mut state, &config, T0 + 1000 + window * 600 + pass);
            inventory_reads += forge.calls.iter().filter(|call| *call == pulls).count();
            state = reload(&state);
            if forge.sent.len() > window as usize {
                break;
            }
            assert!(result.unwrap_err().is::<BudgetExhausted>());
            assert_eq!(state[&GATE.to_string()].notices, window as u32);
            if window == 0 && pass == 0 {
                assert!(forge.reviewed.is_empty());
                assert!(matches!(
                    state[&GATE.to_string()].blocked_scan.as_ref().unwrap().step,
                    PrStep::Review
                ));
            }
            if window == 0 && pass == 1 {
                assert_eq!(state["7"].checked, T0 + 1001, "peer gets the next turn");
                assert_eq!(forge.reviewed, [1], "resume at the interrupted review");
            }
        }
        assert_eq!(forge.sent.len(), window as usize + 1);
        assert_eq!(inventory_reads, 1, "one pinned inventory per notice");
        assert!(forge.sent[window as usize]
            .2
            .contains("Approved PRs blocked by missing check: 2\n"));
    }
    assert_eq!(forge.reviewed, [1, 3, 4, 1, 3, 4]);
}

#[test]
fn story2856_ac1_changed_pr_head_discards_partial_count() {
    let mut forge = Forge::new(
        &["merge-hold-gate"],
        json!([{"id": GATE, "name": "merge-hold-gate"}]),
    );
    forge.runs(GATE, vec![sf(3, 300), sf(2, 200), sf(1, 100)]);
    let pulls = "repos/{owner}/{repo}/pulls?state=open&per_page=100";
    let inventory = |sha: &str| {
        json!([
            {"number": 1, "head": {"sha": "s1"}, "base": {"ref": "main"}},
            {"number": 2, "head": {"sha": sha}, "base": {"ref": "main"}}
        ])
    };
    forge.set(pulls, inventory("s2"));
    for sha in ["s1", "s2", "new"] {
        forge.set(
            &format!("repos/{{owner}}/{{repo}}/commits/{sha}/check-runs?per_page=100"),
            json!({"check_runs": []}),
        );
        forge.set(
            &format!("repos/{{owner}}/{{repo}}/commits/{sha}/status?per_page=100"),
            json!({"statuses": []}),
        );
    }
    forge.reviews.insert(1, "APPROVED");
    forge.reviews.insert(2, "APPROVED");
    let config = Config::default();
    let mut state = State::new();
    forge.budget = Some(11);
    assert!(forge
        .pass(&mut state, &config, T0 + 1000)
        .unwrap_err()
        .is::<BudgetExhausted>());
    state = reload(&state);
    assert_eq!(
        state[&GATE.to_string()]
            .blocked_scan
            .as_ref()
            .unwrap()
            .count,
        1
    );

    forge.budget = None;
    forge.review_heads.insert(2, "new".into());
    assert!(forge
        .pass(&mut state, &config, T0 + 1001)
        .unwrap_err()
        .to_string()
        .contains("PR head changed"));
    assert!(forge.sent.is_empty());
    assert!(state[&GATE.to_string()].blocked_scan.is_none());

    forge.set(pulls, inventory("new"));
    forge.pass(&mut state, &config, T0 + 1002).unwrap();
    assert_eq!(forge.sent.len(), 1);
    assert!(forge.sent[0]
        .2
        .contains("Approved PRs blocked by missing check: 2\n"));
}

// AC1: a workflow whose observation keeps failing (here an open incident
// with a truncated job inventory, which always sorts first) uses up its turn
// instead of ending the pass, so the required workflow behind it still alerts.
// trace:STORY-2856 | ai:claude
#[test]
fn story2856_ac1_broken_workflow_does_not_starve_required_one() {
    let mut forge = Forge::new(
        &["merge-hold-gate", "build"],
        json!([{"id": 7, "name": "ci"}, {"id": GATE, "name": "merge-hold-gate"}]),
    );
    forge.runs(
        7,
        vec![api_run(20, "push", "completed", Some("success"), 100)],
    );
    forge.set(
        "repos/{owner}/{repo}/actions/runs/20/jobs?per_page=100",
        json!({"total_count": 100, "jobs": []}),
    );
    forge.runs(GATE, vec![sf(3, 300), sf(2, 200), sf(1, 100)]);
    forge.set(
        "repos/{owner}/{repo}/pulls?state=open&per_page=100",
        json!([]),
    );
    let mut state = State::new();
    state.entry("7".into()).or_default().notices = 1;
    let error = forge
        .pass(&mut state, &Config::default(), T0 + 400)
        .unwrap_err();
    assert!(format!("{error:#}").contains("truncated"), "{error:#}");
    assert_eq!(forge.sent.len(), 1, "the required workflow still alerts");
    assert_eq!(
        state["7"].notices, 1,
        "an unreadable workflow keeps its episode"
    );
    assert_eq!(state["7"].checked, T0 + 400);
}

#[test]
fn story2856_missing_custom_status_and_latest_check_attempt() {
    let checks = json!([
        {"id": 1, "name": "build", "conclusion": "success"},
        {"id": 2, "name": "build", "conclusion": null}
    ]);
    assert!(!head_has_context("build", checks.as_array().unwrap(), &[]));
    let statuses = json!([
        {"id": 9, "context": "merge-hold-gate", "state": "pending"},
        {"id": 10, "context": "merge-hold-gate", "state": "success"}
    ]);
    assert!(head_has_context(
        "merge-hold-gate",
        &[],
        statuses.as_array().unwrap()
    ));
    assert!(!head_has_context(
        "missing",
        &[],
        statuses.as_array().unwrap()
    ));
}

#[test]
fn story2856_threshold_backoff_and_recovery() {
    let config = Config::default();
    let mut state = Episode::default();
    let runs = vec![sf(3, 0), sf(2, 0), sf(1, 0)];
    assert_eq!(
        evaluate(&mut state, &runs[..2], true, &config, 1000).unwrap(),
        None
    );
    assert_eq!(
        evaluate(&mut state, &runs, true, &config, 1000).unwrap(),
        Some(false)
    );
    state.notices = 1;
    state.next = 1300;
    let mut state: Episode = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    assert_eq!(
        evaluate(&mut state, &runs, true, &config, 1299).unwrap(),
        None
    );
    assert_eq!(
        evaluate(&mut state, &runs, true, &config, 1300).unwrap(),
        Some(false)
    );
    assert_eq!(
        evaluate(&mut state, &[ok(4, 0)], true, &config, 1301).unwrap(),
        Some(true)
    );
    state = Episode::default();
    assert_eq!(
        evaluate(&mut state, &[ok(4, 0)], true, &config, 1302).unwrap(),
        None
    );
}

// AC4: N startup failures give one alert per backoff window plus one recovery.
#[test]
fn story2856_one_delivery_per_window_and_one_recovery() {
    let config = Config::default();
    let runs = vec![sf(3, 0), sf(2, 0), sf(1, 0)];
    let mut episode = Episode::default();
    let mut sent = Vec::new();
    for now in [1000, 1001, 1299, 1300, 1301, 1899, 1900] {
        if let Some(recovered) = evaluate(&mut episode, &runs, true, &config, now).unwrap() {
            episode
                .deliver(recovered, now, || {
                    sent.push((now, recovered));
                    Ok(true)
                })
                .unwrap();
        }
        // Simulate a process restart between observations.
        episode = serde_json::from_slice(&serde_json::to_vec(&episode).unwrap()).unwrap();
    }
    assert_eq!(sent, [(1000, false), (1300, false), (1900, false)]);
    for now in [1901, 1902] {
        if let Some(recovered) = evaluate(&mut episode, &[ok(4, 0)], true, &config, now).unwrap() {
            episode
                .deliver(recovered, now, || {
                    sent.push((now, recovered));
                    Ok(true)
                })
                .unwrap();
        }
    }
    assert_eq!(sent.last(), Some(&(1901, true)));
    assert_eq!(sent.len(), 4);
    let mut episode = Episode::default();
    assert_eq!(
        evaluate(&mut episode, &runs, true, &config, 2000).unwrap(),
        Some(false)
    );
    assert!(!episode.deliver(false, 2000, || Ok(false)).unwrap());
    assert_eq!(
        evaluate(&mut episode, &runs, true, &config, 2001).unwrap(),
        Some(false)
    );
}

#[test]
fn story2856_started_jobs_break_streak_and_queued_timeout() {
    let mut runs = vec![sf(3, 0), sf(2, 0), sf(1, 0)];
    runs[1]["conclusion"] = "failure".into();
    assert_eq!(
        evaluate(
            &mut Episode::default(),
            &runs,
            true,
            &Config::default(),
            T0 + 99_999
        )
        .unwrap(),
        None
    );
    for r in &mut runs {
        r["status"] = "queued".into();
        r["conclusion"] = Value::Null;
    }
    let created = timestamp(&runs[0], "created_at").unwrap();
    assert_eq!(
        evaluate(
            &mut Episode::default(),
            &runs,
            true,
            &Config::default(),
            created + 599
        )
        .unwrap(),
        None
    );
    assert_eq!(
        evaluate(
            &mut Episode::default(),
            &runs,
            true,
            &Config::default(),
            created + 600
        )
        .unwrap(),
        Some(false)
    );
}

// Persisted state from the previous release still loads.
#[test]
fn story2856_legacy_state_file_loads() {
    let state: State = serde_json::from_str(
        r#"{"7": {"first": "2026-11-16T16:01:00Z", "next": 5, "notices": 2}}"#,
    )
    .unwrap();
    assert_eq!((state["7"].notices, state["7"].latest_failure), (2, 0));
}

// A rerun keeps the run id: the alerted failure itself succeeding on a later
// attempt is recovery, not a silent stuck episode.
// trace:BUG-2689 | ai:claude
#[test]
fn bug2689_rerun_success_of_alerted_run_recovers() {
    let config = Config::default();
    let mut episode = Episode::default();
    let runs = vec![sf(3, 0), sf(2, 0), sf(1, 0)];
    assert_eq!(
        evaluate(&mut episode, &runs, true, &config, 1000).unwrap(),
        Some(false)
    );
    assert!(episode.deliver(false, 1000, || Ok(true)).unwrap());
    assert_eq!(episode.latest_failure, 3);
    let mut rerun = runs.clone();
    rerun[0]["conclusion"] = "success".into();
    rerun[0]["run_attempt"] = 2.into();
    assert_eq!(
        evaluate(&mut episode, &rerun, true, &config, 1001).unwrap(),
        Some(true)
    );
    assert!(episode.deliver(true, 1001, || Ok(true)).unwrap());
    assert!(!episode.active());
    assert_eq!(
        evaluate(&mut episode, &rerun, true, &config, 1002).unwrap(),
        None,
        "the recovered rerun is not reported twice"
    );
}

// Fresh state, a full first page with no started run: an older page supplies
// the job-name mapping, and the cached mapping is reused afterwards.
// trace:BUG-2689 | ai:claude
#[test]
fn bug2689_maps_jobs_from_older_runs_when_first_page_never_started() {
    let mut forge = Forge::new(&["build"], json!([{"id": 7, "name": "ci"}]));
    let page1 = (0..100u64)
        .map(|i| {
            api_run(
                1000 - i,
                "push",
                "completed",
                Some("startup_failure"),
                -(i as i64),
            )
        })
        .collect();
    forge.runs(7, page1);
    let older = "repos/{owner}/{repo}/actions/workflows/7/runs?per_page=99&page=2";
    forge.set(
        older,
        json!({"workflow_runs": [sf(800, -500), api_run(799, "push", "completed", Some("success"), -600)]}),
    );
    forge.set(
        "repos/{owner}/{repo}/actions/runs/799/jobs?per_page=100",
        json!({"total_count": 1, "jobs": [{"name": "build"}]}),
    );
    forge.set(
        "repos/{owner}/{repo}/pulls?state=open&per_page=100",
        json!([]),
    );
    let mut state = State::new();
    forge.pass(&mut state, &Config::default(), T0 + 10).unwrap();
    assert!(forge.calls.iter().any(|c| c == older), "{:?}", forge.calls);
    assert_eq!(state["7"].jobs, ["build"]);
    assert_eq!(forge.sent.len(), 1, "{:?}", forge.sent);
    assert!(forge.sent[0].2.contains("Required checks: build"));

    forge.responses.remove(older);
    let mut state = reload(&state);
    forge.pass(&mut state, &Config::default(), T0 + 20).unwrap();
    assert!(!forge.calls.iter().any(|c| c == older), "{:?}", forge.calls);
}

// More than 100 open PRs: the inventory is paginated, not an error.
// trace:BUG-2689 | ai:claude
#[test]
fn bug2689_paginates_open_prs_past_100() {
    let mut forge = Forge::new(
        &["merge-hold-gate"],
        json!([{"id": GATE, "name": "merge-hold-gate"}]),
    );
    forge.runs(GATE, vec![sf(13, 30), sf(12, 20), sf(11, 10)]);
    let pr = |n: u64, base: &str| json!({"number": n, "draft": false, "base": {"ref": base}, "head": {"sha": format!("s{n}")}});
    forge.set(
        "repos/{owner}/{repo}/pulls?state=open&per_page=100",
        Value::Array((1..=100).map(|n| pr(n, "release")).collect()),
    );
    // PR 100 shifted onto page 2 while the inventory was read.
    forge.set(
        "repos/{owner}/{repo}/pulls?state=open&per_page=99&page=2",
        json!([pr(100, "release"), pr(101, "main")]),
    );
    forge.set(
        "repos/{owner}/{repo}/commits/s101/check-runs?per_page=100",
        json!({"check_runs": []}),
    );
    forge.set(
        "repos/{owner}/{repo}/commits/s101/status?per_page=100",
        json!({"statuses": []}),
    );
    forge.reviews.insert(101, "APPROVED");
    let mut state = State::new();
    forge
        .pass(&mut state, &Config::default(), T0 + 100)
        .unwrap();
    assert_eq!(forge.reviewed, [101]);
    assert_eq!(forge.sent.len(), 1, "{:?}", forge.sent);
    assert!(
        forge.sent[0]
            .2
            .contains("Approved PRs blocked by missing check: 1"),
        "{}",
        forge.sent[0].2
    );
}

// trace:BUG-2694 | ai:codex
#[test]
fn bug2694_retries_only_unaccepted_recovery_channels_after_reload() {
    let mut forge = Forge::new(&["gate"], json!([{"id": GATE, "name": "gate"}]));
    let config = Config::default();
    let mut state = State::new();
    forge.runs(GATE, vec![sf(3, 30), sf(2, 20), sf(1, 10)]);
    forge.set(
        "repos/{owner}/{repo}/pulls?state=open&per_page=100",
        json!([]),
    );
    forge.pass(&mut state, &config, T0 + 40).unwrap();

    forge.runs(GATE, vec![ok(4, 50), sf(3, 30)]);
    forge.set(
        "repos/{owner}/{repo}/actions/runs/4/jobs?per_page=100",
        json!({"total_count": 1, "jobs": [{"name": "gate"}]}),
    );
    forge.fail_push = true;
    forge.channel_attempts.clear();
    forge.pass(&mut state, &config, T0 + 60).unwrap();
    state = reload(&state);
    let key = GATE.to_string();
    assert_eq!(
        state[&key].recoveries.front().unwrap().accepted,
        BTreeSet::from(["notify".into()])
    );
    assert!(!state[&key].active());

    forge.fail_push = false;
    forge.runs(GATE, vec![]);
    forge.pass(&mut state, &config, T0 + 61).unwrap();
    state = reload(&state);
    forge.pass(&mut state, &config, T0 + 62).unwrap();
    assert!(state[&key].recoveries.is_empty());
    assert!(!state[&key].active());
    assert_eq!(
        forge.sent[1], forge.sent[2],
        "the retry uses the observed recovery payload"
    );
    assert_eq!(
        forge.channel_attempts,
        [
            ("notify", "Required CI recovered".into()),
            ("push", "Required CI recovered".into()),
            ("push", "Required CI recovered".into()),
        ]
    );
}

// trace:BUG-2694 | ai:codex
#[test]
fn bug2694_both_paginated_discovery_scans_resume_after_reload() {
    check_paginated_discovery(false);
}

// trace:BUG-2592 | ai:codex
#[test]
fn bug2592_discovers_boundary_pr_and_run_after_deletions_between_passes() {
    check_paginated_discovery(true);
}

fn check_paginated_discovery(remove_rows: bool) {
    let mut forge = Forge::new(&["build"], json!([{"id": 7, "name": "ci"}]));
    let runs = "repos/{owner}/{repo}/actions/workflows/7/runs";
    let pulls = "repos/{owner}/{repo}/pulls?state=open";
    let mut history: Vec<_> = (803..=1000).rev().map(|id| sf(id, id as i64)).collect();
    history.push(ok(800, 800));
    forge.runs(7, history[..100].to_vec());
    forge.set(&paged(runs, 2), json!({"workflow_runs": &history[99..198]}));
    forge.set(&paged(runs, 3), json!({"workflow_runs": &history[196..]}));
    forge.set(
        "repos/{owner}/{repo}/actions/runs/800/jobs?per_page=100",
        json!({"total_count": 1, "jobs": [{"name": "build"}]}),
    );
    let pr = |number, branch| json!({"number": number, "base": {"ref": branch}, "head": {"sha": format!("s{number}")}});
    let mut live_prs: Vec<_> = (1..=101).map(|n| pr(n, "release")).collect();
    live_prs[0] = pr(1, "main");
    live_prs[100] = pr(101, "main");
    forge.set(&paged(pulls, 1), json!(&live_prs[..100]));
    forge.set(&paged(pulls, 2), json!([pr(1, "main"), pr(101, "main")]));
    for n in [1, 101] {
        forge.set(
            &format!("repos/{{owner}}/{{repo}}/commits/s{n}/check-runs?per_page=100"),
            json!({"check_runs": []}),
        );
        forge.set(
            &format!("repos/{{owner}}/{{repo}}/commits/s{n}/status?per_page=100"),
            json!({"statuses": []}),
        );
        forge.reviews.insert(n, "APPROVED");
    }
    // The shared prologue and fresh run page leave just one read per pass.
    forge.budget = Some(6);
    let mut state = State::new();
    let mut calls = Vec::new();
    for pass in 0..12 {
        let result = forge.pass(&mut state, &Config::default(), T0 + 1100 + pass);
        if remove_rows && forge.calls.contains(&paged(runs, 2)) {
            // A started run at index 198 moves to 197 when a newer run is
            // deleted. Derive both possible page-three responses by offset.
            history.remove(120);
            for size in [98, 100] {
                let offset = (2 * size).min(history.len());
                forge.set(
                    &format!("{runs}?per_page={size}&page=3"),
                    json!({"workflow_runs": &history[offset..]}),
                );
            }
        }
        // Delete an excluded PR after the inventory checkpoint. Model the
        // forge offsets independently so fixed-size pagination misses PR 101.
        // trace:BUG-2592 | ai:codex
        if remove_rows && forge.calls.contains(&paged(pulls, 1)) && live_prs.len() == 101 {
            live_prs.remove(1);
            for size in [99, 100] {
                forge.set(
                    &format!("{pulls}&per_page={size}&page=2"),
                    json!(&live_prs[size..]),
                );
            }
        }
        calls.extend(forge.calls.clone());
        state = reload(&state);
        if !forge.sent.is_empty() {
            result.unwrap();
            break;
        }
        assert!(result.unwrap_err().is::<BudgetExhausted>());
    }
    for endpoint in [
        &paged(runs, 2),
        &paged(runs, 3),
        &paged(pulls, 1),
        &paged(pulls, 2),
    ] {
        assert_eq!(
            calls.iter().filter(|call| *call == endpoint).count(),
            1,
            "{endpoint}"
        );
    }
    assert_eq!(forge.reviewed, [1, 101]);
    assert_eq!(forge.sent.len(), 1);
    assert!(forge.sent[0]
        .2
        .contains("Approved PRs blocked by missing check: 2"));
}
