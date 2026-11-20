//! STORY-2897 S3: the red-main watch.
// trace:STORY-2897 | ai:claude
use super::*;
use crate::forge::{CheckOutcome, CommitCheck, CommitChecks};
use std::collections::HashMap;

const BUILD: &str = "Build (ubuntu-latest)";

fn required() -> Vec<String> {
    vec!["merge-hold-gate".into(), BUILD.into()]
}

fn t0() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-11-20T10:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn sha(n: u8) -> String {
    format!("{:040x}", n)
}

fn check(name: &str, outcome: CheckOutcome, run: &str, attempt: u64) -> CommitCheck {
    CommitCheck {
        name: name.into(),
        outcome,
        run_id: Some(run.into()),
        workflow_id: Some("ci".into()),
        cancelled: false,
        run_attempt: Some(attempt),
    }
}

fn green(run: &str) -> Vec<CommitCheck> {
    vec![check(BUILD, CheckOutcome::Passed, run, 1)]
}

fn red(run: &str, attempt: u64) -> Vec<CommitCheck> {
    vec![
        check("CI (lib-0)", CheckOutcome::Failed, run, attempt),
        check(BUILD, CheckOutcome::Failed, run, attempt),
    ]
}

#[derive(Default)]
struct Fake {
    now: Option<DateTime<Utc>>,
    /// Newest first.
    commits: Vec<Commit>,
    checks: HashMap<String, Vec<CommitCheck>>,
    steps: HashMap<String, Vec<String>>,
    reruns: Vec<String>,
    mails: Vec<(String, String)>,
    notices: Vec<String>,
    filed: Vec<FixSpec>,
    fix_state: FixState,
    fail_mail: bool,
    fail_file: bool,
}

impl Fake {
    fn push(&mut self, n: u8, subject: &str, checks: Vec<CommitCheck>) {
        self.commits.insert(
            0,
            Commit {
                sha: sha(n),
                subject: subject.into(),
            },
        );
        self.checks.insert(sha(n), checks);
    }
    fn at(&mut self, minutes: i64) {
        self.now = Some(t0() + Duration::minutes(minutes));
    }
    fn mail_keys(&self) -> Vec<&str> {
        self.mails.iter().map(|(k, _)| k.as_str()).collect()
    }
}

impl Ports for Fake {
    fn now(&self) -> DateTime<Utc> {
        self.now.unwrap_or_else(t0)
    }
    fn history(&mut self, stop: Option<&str>, max: usize) -> Result<Vec<Commit>> {
        Ok(self
            .commits
            .iter()
            .take_while(|c| Some(c.sha.as_str()) != stop)
            .take(max)
            .cloned()
            .collect())
    }
    fn checks(&mut self, sha: &str) -> Result<CommitChecks> {
        let checks = self
            .checks
            .get(sha)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no CI for {sha}"))?;
        Ok(CommitChecks {
            sha: sha.into(),
            checks,
        })
    }
    fn failed_steps(&mut self, run_id: &str, _job: &str) -> Result<Vec<String>> {
        Ok(self.steps.get(run_id).cloned().unwrap_or_default())
    }
    fn rerun_failed(&mut self, run_id: &str) -> Result<()> {
        self.reruns.push(run_id.into());
        Ok(())
    }
    fn mail(&mut self, key: &str, subject: &str, _body: &str) -> Result<()> {
        anyhow::ensure!(!self.fail_mail, "mailbox unavailable");
        // Production mail is idempotent by key.
        if !self.mails.iter().any(|(k, _)| k == key) {
            self.mails.push((key.into(), subject.into()));
        }
        Ok(())
    }
    fn notify_operator(&mut self, key: &str, _title: &str, _body: &str) -> Result<()> {
        self.notices.push(key.into());
        Ok(())
    }
    fn file_fix_spec(&mut self, fix: &FixSpec) -> Result<String> {
        anyhow::ensure!(!self.fail_file, "store busy");
        if let Some(i) = self.filed.iter().position(|f| f.red_sha == fix.red_sha) {
            return Ok(format!("BUG-{}", 9000 + i));
        }
        self.filed.push(fix.clone());
        Ok(format!("BUG-{}", 9000 + self.filed.len() - 1))
    }
    fn fix_spec_state(&mut self, _id: &str) -> Result<FixState> {
        Ok(self.fix_state)
    }
}

fn run(root: &Path, fake: &mut Fake) -> Vec<String> {
    pass(
        root,
        &required(),
        std::time::Duration::from_secs(7200),
        fake,
    )
    .unwrap()
}

fn state(root: &Path) -> State {
    read_state(root).unwrap()
}

fn outcomes(root: &Path) -> Vec<String> {
    read_journal(root)
        .unwrap()
        .into_iter()
        .filter_map(|e| e.outcome)
        .collect()
}

/// A base fixture: a green commit, then a red merge on top.
fn red_on_green() -> (tempfile::TempDir, Fake) {
    let dir = tempfile::tempdir().unwrap();
    let mut fake = Fake::default();
    fake.push(1, "feat: green base (STORY-1) (#10)", green("100"));
    run(dir.path(), &mut fake);
    fake.push(2, "fix(x): break main (STORY-2) (#11)", red("200", 1));
    (dir, fake)
}

#[test]
fn judge_tells_red_from_cancelled_pending_and_unreadable() {
    let checks = |c: Vec<CommitCheck>| -> Result<CommitChecks> {
        Ok(CommitChecks {
            sha: sha(1),
            checks: c,
        })
    };
    assert!(matches!(
        judge(checks(red("1", 1)), &required()),
        Verdict::Red(_)
    ));
    assert_eq!(judge(checks(green("1")), &required()), Verdict::Green);
    let mut cancelled = check(BUILD, CheckOutcome::Failed, "1", 1);
    cancelled.cancelled = true;
    assert!(
        matches!(
            judge(checks(vec![cancelled]), &required()),
            Verdict::Unknown(_)
        ),
        "a cancelled run is not a red main"
    );
    let pending = check(BUILD, CheckOutcome::Pending, "1", 1);
    assert!(matches!(
        judge(checks(vec![pending]), &required()),
        Verdict::Unknown(_)
    ));
    assert!(matches!(
        judge(Err(anyhow::anyhow!("gh down")), &required()),
        Verdict::Unknown(_)
    ));
    // merge-hold-gate runs on pull requests only: its absence on a main
    // commit does not make the commit unknown.
    assert_eq!(judge(checks(green("1")), &required()), Verdict::Green);
}

#[test]
fn a_green_main_records_the_last_green_commit_and_alerts_nobody() {
    let dir = tempfile::tempdir().unwrap();
    let mut fake = Fake::default();
    fake.push(1, "feat: a (#1)", green("1"));
    run(dir.path(), &mut fake);
    assert_eq!(state(dir.path()).last_green, Some(sha(1)));
    assert!(fake.mails.is_empty() && fake.notices.is_empty() && fake.filed.is_empty());
}

#[test]
fn unknown_ci_opens_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut fake = Fake::default();
    fake.push(
        1,
        "feat: a (#1)",
        vec![check(BUILD, CheckOutcome::Pending, "1", 1)],
    );
    let notes = run(dir.path(), &mut fake);
    assert!(state(dir.path()).episode.is_none());
    assert!(notes.iter().any(|n| n.contains("unknown")), "{notes:?}");
    assert!(fake.mails.is_empty());
}

#[test]
fn a_red_opens_one_episode_mails_once_and_reruns_once_but_files_nothing_yet() {
    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    fake.at(5);
    run(dir.path(), &mut fake);
    let episode = state(dir.path()).episode.expect("an open episode");
    assert_eq!(episode.red_sha, sha(2));
    assert_eq!(episode.pr, Some(11));
    assert_eq!(episode.specs, vec!["STORY-2".to_string()]);
    assert_eq!(fake.mail_keys(), vec![format!("main-red-{}", sha(2))]);
    assert_eq!(
        fake.reruns,
        vec!["200".to_string()],
        "one confirming re-run"
    );
    assert!(
        fake.filed.is_empty(),
        "no fix spec before the red is confirmed"
    );
    assert!(
        fake.notices.is_empty(),
        "no operator notice before the red is confirmed"
    );
    let starts = read_journal(dir.path())
        .unwrap()
        .iter()
        .filter(|e| e.event == "red_start")
        .count();
    assert_eq!(starts, 1);
}

#[test]
fn a_red_cleared_by_a_green_rerun_is_a_flake_with_no_spec_or_notice() {
    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    fake.at(12);
    fake.checks.insert(sha(2), green("200"));
    run(dir.path(), &mut fake);
    assert!(state(dir.path()).episode.is_none());
    assert_eq!(state(dir.path()).last_green, Some(sha(2)));
    assert_eq!(outcomes(dir.path()), vec!["flake".to_string()]);
    assert!(fake.filed.is_empty() && fake.notices.is_empty());
    let m = metrics(
        &read_journal(dir.path()).unwrap(),
        t0(),
        t0() + Duration::hours(1),
    );
    assert_eq!(m["flakes"], 1);
    assert_eq!(m["resolved"], 0, "a flake is not a resolved red");
    assert_eq!(m["fix_latency_seconds_median"], serde_json::Value::Null);
}

#[test]
fn a_red_that_survives_its_rerun_is_confirmed_and_files_one_fix_spec() {
    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    fake.at(8);
    fake.checks.insert(sha(2), red("200", 2));
    run(dir.path(), &mut fake);
    fake.at(10);
    run(dir.path(), &mut fake);
    let episode = state(dir.path()).episode.unwrap();
    assert!(episode.confirmed_at.is_some());
    assert_eq!(fake.filed.len(), 1, "filed once across passes");
    let fix = &fake.filed[0];
    assert_eq!(fix.fixes, vec!["STORY-2".to_string()]);
    assert!(fix
        .title
        .starts_with(&format!("main red at {}", &sha(2)[..12])));
    assert_eq!(episode.fix_spec.as_deref(), Some("BUG-9000"));
    assert_eq!(fake.notices, vec![format!("main-red-{}", sha(2))]);
}

#[test]
fn a_red_still_pending_its_rerun_is_confirmed_after_thirty_minutes() {
    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    fake.checks
        .insert(sha(2), vec![check(BUILD, CheckOutcome::Pending, "200", 2)]);
    fake.at(29);
    run(dir.path(), &mut fake);
    assert!(fake.filed.is_empty());
    fake.at(31);
    run(dir.path(), &mut fake);
    assert!(state(dir.path()).episode.unwrap().confirmed_at.is_some());
    assert_eq!(fake.filed.len(), 1);
}

#[test]
fn the_episode_names_the_oldest_red_and_counts_later_red_merges() {
    let dir = tempfile::tempdir().unwrap();
    let mut fake = Fake::default();
    fake.push(1, "base (#1)", green("1"));
    run(dir.path(), &mut fake);
    fake.push(2, "first bad (STORY-2) (#2)", red("2", 1));
    fake.push(3, "second (#3)", red("3", 1));
    run(dir.path(), &mut fake);
    assert_eq!(state(dir.path()).episode.unwrap().red_sha, sha(2));
    fake.push(4, "third (#4)", red("4", 1));
    run(dir.path(), &mut fake);
    let episode = state(dir.path()).episode.unwrap();
    assert_eq!(episode.red_merges, vec![sha(3), sha(4)]);
    fake.at(40);
    fake.push(5, "fix (#5)", green("5"));
    run(dir.path(), &mut fake);
    let m = metrics(
        &read_journal(dir.path()).unwrap(),
        t0(),
        t0() + Duration::hours(1),
    );
    assert_eq!(m["red_merges"], 3);
    assert_eq!(m["episodes"], 1);
}

#[test]
fn each_revert_trigger_is_escalated_once_after_confirmation() {
    let (dir, mut fake) = red_on_green();
    fake.steps
        .insert("200".into(), vec!["Build all packages".into()]);
    run(dir.path(), &mut fake);
    let unbuildable = format!("main-red-{}-revert-unbuildable", sha(2));
    assert!(
        !fake.mail_keys().contains(&unbuildable.as_str()),
        "no revert recommendation before confirmation"
    );
    fake.at(31);
    run(dir.path(), &mut fake);
    fake.at(32);
    run(dir.path(), &mut fake);
    let count = |fake: &Fake, key: &str| fake.mail_keys().iter().filter(|k| **k == key).count();
    assert_eq!(count(&fake, &unbuildable), 1);
    assert_eq!(
        fake.notices.iter().filter(|k| **k == unbuildable).count(),
        1
    );

    fake.fix_state.needs_design = true;
    fake.at(33);
    run(dir.path(), &mut fake);
    let design = format!("main-red-{}-revert-needs-design", sha(2));
    assert_eq!(count(&fake, &design), 1);

    fake.at(121);
    run(dir.path(), &mut fake);
    fake.at(125);
    run(dir.path(), &mut fake);
    let window = format!("main-red-{}-revert-fix-window-elapsed", sha(2));
    assert_eq!(count(&fake, &window), 1);
    assert_eq!(
        state(dir.path()).episode.unwrap().recommended,
        vec!["unbuildable", "needs-design", "fix-window-elapsed"]
    );
}

#[test]
fn the_fix_window_is_the_configured_one() {
    let (dir, mut fake) = red_on_green();
    let short = std::time::Duration::from_secs(45 * 60);
    pass(dir.path(), &required(), short, &mut fake).unwrap();
    fake.at(44);
    pass(dir.path(), &required(), short, &mut fake).unwrap();
    let window = format!("main-red-{}-revert-fix-window-elapsed", sha(2));
    assert!(!fake.mail_keys().contains(&window.as_str()));
    fake.at(46);
    pass(dir.path(), &required(), short, &mut fake).unwrap();
    assert!(fake.mail_keys().contains(&window.as_str()));
}

#[test]
fn a_smoke_failure_on_an_episode_commit_recommends_a_revert() {
    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    assert!(
        !note_smoke_failure(dir.path(), &sha(9)).unwrap(),
        "not in the episode"
    );
    assert!(note_smoke_failure(dir.path(), &sha(2)).unwrap());
    fake.at(31);
    run(dir.path(), &mut fake);
    let smoke = format!("main-red-{}-revert-smoke-fail", sha(2));
    assert!(fake.mail_keys().contains(&smoke.as_str()));
}

#[test]
fn a_confirmed_red_closes_as_fixed_or_reverted() {
    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    fake.at(31);
    run(dir.path(), &mut fake);
    fake.at(50);
    fake.push(3, "fix(x): repair main (BUG-9000) (#12)", green("300"));
    run(dir.path(), &mut fake);
    assert_eq!(outcomes(dir.path()), vec!["fixed".to_string()]);
    assert!(fake
        .mail_keys()
        .contains(&format!("main-red-{}-green", sha(2)).as_str()));
    let m = metrics(
        &read_journal(dir.path()).unwrap(),
        t0(),
        t0() + Duration::hours(2),
    );
    assert_eq!(m["fix_latency_seconds_median"], 50 * 60);
    assert_eq!(m["reverted"], 0);

    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    fake.at(31);
    run(dir.path(), &mut fake);
    fake.push(
        3,
        "Revert \"fix(x): break main (STORY-2) (#11)\" (#13)",
        green("300"),
    );
    run(dir.path(), &mut fake);
    assert_eq!(outcomes(dir.path()), vec!["reverted".to_string()]);
}

#[test]
fn red_minutes_are_clipped_to_the_window_and_an_open_episode_counts_to_its_end() {
    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    let events = read_journal(dir.path()).unwrap();
    let m = metrics(
        &events,
        t0() + Duration::minutes(30),
        t0() + Duration::minutes(90),
    );
    assert_eq!(m["red_minutes"], 60);
    assert_eq!(m["episodes"], 1);
    let none = metrics(
        &events,
        t0() - Duration::hours(2),
        t0() - Duration::hours(1),
    );
    assert_eq!(none["episodes"], 0);
}

#[test]
fn a_broken_state_file_fails_the_pass_without_overwriting_it() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida/land")).unwrap();
    std::fs::write(dir.path().join(STATE), b"{not json").unwrap();
    let mut fake = Fake::default();
    fake.push(1, "a (#1)", green("1"));
    assert!(pass(
        dir.path(),
        &required(),
        std::time::Duration::from_secs(60),
        &mut fake
    )
    .is_err());
    assert_eq!(std::fs::read(dir.path().join(STATE)).unwrap(), b"{not json");
}

/// Merging continues while main is red: no merge decision reads the watch.
#[test]
fn no_merge_path_reads_the_red_main_watch() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    let mut stack = vec![src.join("gate")];
    stack.extend(
        [
            "pr_cmd.rs",
            "pr_service.rs",
            "pr_train.rs",
            "pr_ship.rs",
            "land.rs",
        ]
        .iter()
        .map(|f| src.join(f)),
    );
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            stack.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            let allowed = path.ends_with("land.rs") && text.contains("LandCommand::Watch");
            if text.contains("main_watch") && !allowed {
                offenders.push(path.display().to_string());
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "merge paths read main_watch: {offenders:?}"
    );
}

#[test]
fn fix_window_parses_and_falls_back_to_two_hours() {
    let policy = |body: &str| crate::landing_policy::from_body(Some(body));
    assert_eq!(
        policy("[landing.red_main]\nfix_window = \"90m\"\n").red_main_fix_window,
        std::time::Duration::from_secs(90 * 60)
    );
    for bad in ["\"soon\"", "\"10s\"", "\"30d\"", "5"] {
        let p = policy(&format!("[landing.red_main]\nfix_window = {bad}\n"));
        assert_eq!(
            p.red_main_fix_window,
            crate::landing_policy::DEFAULT_RED_MAIN_FIX_WINDOW
        );
        assert_eq!(
            p.origin("red_main.fix_window"),
            crate::landing_policy::Origin::Invalid,
            "{bad}"
        );
        assert!(!p.warnings.is_empty(), "{bad}");
    }
    let unset = policy("[landing]\nmode = \"ci-gated\"\n");
    assert_eq!(
        unset.red_main_fix_window,
        crate::landing_policy::DEFAULT_RED_MAIN_FIX_WINDOW
    );
    assert_eq!(
        unset.origin("red_main.fix_window"),
        crate::landing_policy::Origin::Default
    );
}

#[test]
fn subject_pr_reads_the_squash_suffix() {
    assert_eq!(subject_pr("feat: x (STORY-1) (#4324)"), Some(4324));
    assert_eq!(subject_pr("feat: x (STORY-1)"), None);
}

/// The watch needs two facts the gates never did: whether a failed check
/// was cancelled, and which attempt of its run reported it.
#[test]
fn the_ci_readers_keep_cancellation_and_the_run_attempt() {
    use serde_json::json;
    let head = sha(1);
    let runs = json!({"total_count": 2, "check_runs": [
        {"name": "a", "head_sha": head, "status": "completed", "conclusion": "cancelled",
         "details_url": "https://github.com/o/r/actions/runs/7/job/1"},
        {"name": "b", "head_sha": head, "status": "completed", "conclusion": "failure",
         "details_url": "https://github.com/o/r/actions/runs/7/job/2"}]});
    let mut api = |endpoint: &str| -> Result<Vec<u8>> {
        let body = if endpoint.contains("/check-runs") {
            runs.clone()
        } else if endpoint.ends_with("/actions/runs/7") {
            json!({"id": 7, "head_sha": head, "workflow_id": 3, "run_attempt": 2})
        } else {
            json!({"sha": head, "total_count": 0, "statuses": []})
        };
        Ok(serde_json::to_vec(&body).unwrap())
    };
    let checks = crate::forge::github_commit_checks("o/r", &head, &mut api).unwrap();
    let by = |n: &str| checks.checks.iter().find(|c| c.name == n).unwrap().clone();
    assert_eq!(
        by("a").outcome,
        CheckOutcome::Failed,
        "gates still fail a cancelled check"
    );
    assert!(by("a").cancelled && !by("b").cancelled);
    assert_eq!(by("b").run_attempt, Some(2));

    let gitlab = crate::forge::parse_glab_commit_statuses(
        &[json!({"name": "x", "sha": head, "id": 1, "status": "canceled"})],
        &head,
    )
    .unwrap();
    assert_eq!(gitlab[0].outcome, CheckOutcome::Failed);
    assert!(gitlab[0].cancelled);
}

#[test]
fn a_failed_opening_mail_records_nothing_and_the_next_pass_opens_again() {
    let (dir, mut fake) = red_on_green();
    fake.fail_mail = true;
    assert!(pass(
        dir.path(),
        &required(),
        std::time::Duration::from_secs(7200),
        &mut fake
    )
    .is_err());
    assert!(state(dir.path()).episode.is_none());
    assert!(read_journal(dir.path()).unwrap().is_empty());
    fake.fail_mail = false;
    run(dir.path(), &mut fake);
    assert!(state(dir.path()).episode.is_some());
    let starts = read_journal(dir.path())
        .unwrap()
        .iter()
        .filter(|e| e.event == "red_start")
        .count();
    assert_eq!(starts, 1);
}

#[test]
fn a_failed_filing_is_retried_without_confirming_twice() {
    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    fake.fail_file = true;
    fake.at(31);
    let notes = run(dir.path(), &mut fake);
    assert!(notes.iter().any(|n| n.contains("not filed")), "{notes:?}");
    assert!(state(dir.path()).episode.unwrap().confirmed_at.is_some());
    fake.fail_file = false;
    fake.at(33);
    run(dir.path(), &mut fake);
    assert_eq!(fake.filed.len(), 1);
    let confirmed = read_journal(dir.path())
        .unwrap()
        .iter()
        .filter(|e| e.event == "confirmed")
        .count();
    assert_eq!(confirmed, 1);
}
