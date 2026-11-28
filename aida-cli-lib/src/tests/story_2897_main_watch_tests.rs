//! STORY-2897 S3a: the observational red-main watch.
// trace:STORY-2897 | ai:claude
// trace:STORY-2897.acaf82b5 | ai:claude
// trace:STORY-2897.Tests | ai:claude
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
        job_id: Some(1),
        run_attempt: Some(attempt),
        run_in_progress: false,
    }
}

fn green(run: &str) -> Vec<CommitCheck> {
    vec![check(BUILD, CheckOutcome::Passed, run, 1)]
}

fn red(run: &str, attempt: u64) -> Vec<CommitCheck> {
    vec![
        check(BUILD, CheckOutcome::Failed, run, attempt),
        check("CI (lib-0)", CheckOutcome::Failed, run, attempt),
    ]
}

fn pending(run: &str) -> Vec<CommitCheck> {
    vec![check(BUILD, CheckOutcome::Pending, run, 1)]
}

#[derive(Default)]
struct Fake {
    now: Option<DateTime<Utc>>,
    /// Oldest first.
    commits: Vec<Commit>,
    checks: HashMap<String, Vec<CommitCheck>>,
    jobs: HashMap<(String, String), JobReport>,
    job_unavailable: bool,
    log_failures: usize,
    mails: Vec<(String, String, String)>,
    fail_mail: bool,
    fail_range: bool,
    deadline_recent: bool,
    deadline_range: bool,
    after_until_deadline: Option<usize>,
    stall_check: Option<String>,
}

impl Fake {
    fn push(&mut self, n: u8, subject: &str, checks: Vec<CommitCheck>) {
        self.commits.push(Commit {
            sha: sha(n),
            subject: subject.into(),
            body: String::new(),
            at: self.now.unwrap_or_else(t0),
        });
        self.checks.insert(sha(n), checks);
    }
    fn at(&mut self, minutes: i64) {
        self.now = Some(t0() + Duration::minutes(minutes));
    }
    fn mail_keys(&self) -> Vec<&str> {
        self.mails.iter().map(|(k, _, _)| k.as_str()).collect()
    }
    fn mail(&self, key: &str) -> &str {
        &self.mails.iter().find(|(k, _, _)| k == key).unwrap().2
    }
}

impl Ports for Fake {
    fn now(&self) -> DateTime<Utc> {
        self.now.unwrap_or_else(t0)
    }
    fn recent(&mut self, max: usize) -> Result<Vec<Commit>> {
        if self.deadline_recent {
            return Err(PassDeadline.into());
        }
        Ok(self.commits.iter().rev().take(max).cloned().collect())
    }
    fn after(&mut self, cursor: &str, max: usize) -> Result<Option<Vec<Commit>>> {
        if let Some(left) = self.after_until_deadline.as_mut() {
            if *left == 0 {
                return Err(PassDeadline.into());
            }
            *left -= 1;
        }
        let Some(i) = self.commits.iter().position(|c| c.sha == cursor) else {
            return Ok(None);
        };
        Ok(Some(
            self.commits[i + 1..].iter().take(max).cloned().collect(),
        ))
    }
    fn range(&mut self, from: &str, to: &str) -> Result<Vec<Commit>> {
        if self.deadline_range {
            return Err(PassDeadline.into());
        }
        anyhow::ensure!(!self.fail_range, "git log failed");
        let a = self.commits.iter().position(|c| c.sha == from).unwrap();
        let b = self.commits.iter().position(|c| c.sha == to).unwrap();
        Ok(self.commits[a + 1..=b].to_vec())
    }
    fn checks(&mut self, sha: &str) -> Result<CommitChecks> {
        if self.stall_check.as_deref() == Some(sha) {
            let deadline = crate::forge_lookup::forge_lookup_deadline().unwrap();
            let mut child = std::process::Command::new("sleep");
            child.arg("10");
            assert!(matches!(
                crate::command_output_with_timeout_capped(
                    child,
                    deadline.saturating_duration_since(std::time::Instant::now()),
                    1024
                ),
                crate::BoundedCommandOutput::TimedOut
            ));
            self.at(31);
            anyhow::bail!("check deadline reached");
        }
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
    fn job(&mut self, run_id: &str, job: &str, _id: u64, _attempt: u64) -> Result<JobReport> {
        anyhow::ensure!(!self.job_unavailable, "job unavailable");
        let mut report = self
            .jobs
            .get(&(run_id.to_string(), job.to_string()))
            .cloned()
            .unwrap_or(JobReport {
                link: format!("https://ci.example/runs/{run_id}/{job}"),
                failed_steps: vec!["Run tests".into()],
                excerpt: format!("{job}: test foo ... FAILED"),
                log_complete: true,
            });
        if self.log_failures > 0 {
            self.log_failures -= 1;
            report.excerpt = "(the job log could not be read yet)".into();
            report.log_complete = false;
        }
        Ok(report)
    }
    fn mail(&mut self, key: &str, subject: &str, body: &str) -> Result<()> {
        anyhow::ensure!(!self.fail_mail, "mailbox unavailable");
        if !self.mails.iter().any(|(k, _, _)| k == key) {
            self.mails.push((key.into(), subject.into(), body.into()));
        }
        Ok(())
    }
}

const WINDOW: std::time::Duration = std::time::Duration::from_secs(7200);

fn run(root: &Path, fake: &mut Fake) -> Vec<String> {
    pass(root, &required(), WINDOW, fake).unwrap()
}

fn try_run(root: &Path, fake: &mut Fake) -> Result<Vec<String>> {
    pass(root, &required(), WINDOW, fake)
}

fn state(root: &Path) -> State {
    read_state(&root.join(".aida/land")).unwrap()
}

fn journal(root: &Path) -> Vec<Event> {
    read_journal(root).unwrap()
}

fn count(root: &Path, kind: &str) -> usize {
    journal(root).iter().filter(|e| e.event == kind).count()
}

fn outcomes(root: &Path) -> Vec<String> {
    journal(root)
        .into_iter()
        .filter_map(|e| e.outcome)
        .collect()
}

fn fail_at(point: Option<&'static str>) {
    FAIL_AT.with(|f| *f.borrow_mut() = point);
}

/// A green base the watch anchors on, then a red merge on top.
fn red_on_green() -> (tempfile::TempDir, Fake) {
    let dir = tempfile::tempdir().unwrap();
    let mut fake = Fake::default();
    fake.push(1, "feat: green base (STORY-1) (#10)", green("100"));
    run(dir.path(), &mut fake);
    fake.push(2, "fix(x): break main (STORY-2) (#11)", red("200", 1));
    (dir, fake)
}

/// `red_on_green`, opened and then confirmed by time.
fn confirmed() -> (tempfile::TempDir, Fake) {
    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    fake.at(31);
    run(dir.path(), &mut fake);
    assert!(state(dir.path()).episode.unwrap().confirmed_at.is_some());
    (dir, fake)
}

// ---------------------------------------------------------------------------
// Judging (p24 :277, forge_commit_ci :336).
// ---------------------------------------------------------------------------

#[test]
fn judging_follows_the_required_checks_and_keeps_others_as_diagnostics() {
    let judge_now = |c: Vec<CommitCheck>, age_hours: i64| {
        judge(
            Ok(CommitChecks {
                sha: sha(1),
                checks: c,
            }),
            &required(),
            t0() - Duration::hours(age_hours),
            t0(),
        )
    };
    // A required success with an optional failure or pending run is green.
    let mut optional = green("1");
    optional.push(check("docs-lint", CheckOutcome::Failed, "9", 1));
    optional.push(check("nightly", CheckOutcome::Pending, "8", 1));
    assert_eq!(judge_now(optional, 0), Verdict::Green);
    // A required failure is red; same-run failures contribute, others are
    // diagnostics only.
    let mut mixed = red("1", 1);
    mixed.push(check("docs-lint", CheckOutcome::Failed, "1", 1));
    let Verdict::Red {
        required: req,
        contributing,
        diagnostics,
    } = judge_now(mixed, 0)
    else {
        panic!("red")
    };
    assert_eq!(names(&req), BUILD);
    assert_eq!(names(&contributing), "CI (lib-0)");
    assert_eq!(diagnostics, vec!["docs-lint".to_string()]);
    // Pending, re-running, cancelled-only and unreadable are not final.
    assert!(matches!(judge_now(pending("1"), 0), Verdict::Pending(_)));
    let mut rerun = red("1", 2);
    rerun[0].run_in_progress = true;
    assert!(matches!(judge_now(rerun, 0), Verdict::Pending(_)));
    let mut cancelled = red("1", 1);
    cancelled.truncate(1);
    cancelled[0].cancelled = true;
    assert!(matches!(
        judge_now(cancelled.clone(), 0),
        Verdict::Pending(_)
    ));
    assert!(
        matches!(judge_now(cancelled, 3), Verdict::Skipped(_)),
        "passed over after the grace period"
    );
    assert!(matches!(
        judge(Err(anyhow::anyhow!("gh down")), &required(), t0(), t0()),
        Verdict::Pending(_)
    ));
}

/// A check row that finished before its run's latest attempt started is
/// not that attempt's result.
#[test]
fn a_result_older_than_its_runs_latest_attempt_is_not_final() {
    use serde_json::json;
    let head = sha(1);
    let runs = json!({"total_count": 2, "check_runs": [
        {"name": "a", "head_sha": head, "status": "completed", "conclusion": "cancelled",
         "completed_at": "2026-11-20T10:00:00Z",
         "details_url": "https://github.com/o/r/actions/runs/7/job/1"},
        {"name": "b", "head_sha": head, "status": "completed", "conclusion": "failure",
         "completed_at": "2026-11-20T10:00:00Z",
         "details_url": "https://github.com/o/r/actions/runs/8/job/2"}]});
    let mut api = |endpoint: &str| -> Result<Vec<u8>> {
        let body = if endpoint.contains("/check-runs") {
            runs.clone()
        } else if endpoint.ends_with("/actions/runs/7") {
            json!({"id": 7, "head_sha": head, "workflow_id": 3, "run_attempt": 1,
                   "status": "completed", "run_started_at": "2026-11-20T09:50:00Z"})
        } else if endpoint.ends_with("/actions/runs/8") {
            // A re-run started (and finished) after b's failure was listed.
            json!({"id": 8, "head_sha": head, "workflow_id": 3, "run_attempt": 2,
                   "status": "completed", "run_started_at": "2026-11-20T10:05:00Z"})
        } else if endpoint.contains("/jobs?") {
            json!({"total_count": 0, "jobs": []})
        } else {
            json!({"sha": head, "total_count": 0, "statuses": []})
        };
        Ok(serde_json::to_vec(&body).unwrap())
    };
    let checks = crate::forge::github_checks_with_attempts("o/r", &head, &mut api, true).unwrap();
    let by = |n: &str| checks.checks.iter().find(|c| c.name == n).unwrap().clone();
    assert!(by("a").cancelled && !by("a").run_in_progress);
    assert_eq!(by("b").outcome, CheckOutcome::Failed, "gates are unchanged");
    assert!(
        by("b").run_in_progress,
        "the stale row is not attempt 2's result"
    );
    let gitlab = crate::forge::parse_glab_commit_statuses(
        &[json!({"name": "x", "sha": head, "id": 1, "status": "canceled"})],
        &head,
    )
    .unwrap();
    assert!(gitlab[0].cancelled);
}

// ---------------------------------------------------------------------------
// The ordered scan (p24 :565, :620) and attribution.
// ---------------------------------------------------------------------------

#[test]
fn the_first_pass_anchors_at_the_newest_green_and_mails_nobody() {
    let dir = tempfile::tempdir().unwrap();
    let mut fake = Fake::default();
    fake.push(1, "old red (#1)", red("1", 1));
    fake.push(2, "green (#2)", green("2"));
    run(dir.path(), &mut fake);
    let s = state(dir.path());
    assert_eq!(s.cursor, Some(sha(2)));
    assert_eq!(s.last_green, Some(sha(2)));
    assert!(fake.mails.is_empty(), "old history is not alerted");
}

#[test]
fn a_red_opens_one_episode_mails_the_advisor_once_with_evidence_without_initiating_a_rerun() {
    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    fake.at(5);
    run(dir.path(), &mut fake);
    let episode = state(dir.path()).episode.expect("open");
    assert_eq!(episode.red_sha, sha(2));
    assert_eq!(episode.pr, Some(11));
    assert_eq!(episode.specs, vec!["STORY-2".to_string()]);
    assert_eq!(episode.held_green, Some(sha(1)));
    let key = format!("main-red-{}", sha(2));
    assert_eq!(fake.mail_keys(), vec![key.as_str()]);
    let body = fake.mail(&key);
    assert!(body.contains("https://ci.example/runs/200/"), "{body}");
    assert!(body.contains("test foo ... FAILED"), "{body}");
    assert!(body.contains(&sha(1)[..12]), "held green: {body}");
    assert!(!include_str!("../forge.rs").contains("/rerun-failed-jobs"));
    assert_eq!(count(dir.path(), "red_start"), 1);
}

#[test]
fn a_red_then_green_between_two_passes_is_still_an_episode() {
    let (dir, mut fake) = red_on_green();
    fake.push(3, "fix (#12)", green("300"));
    run(dir.path(), &mut fake);
    assert_eq!(count(dir.path(), "red_start"), 1);
    assert_eq!(outcomes(dir.path()), vec!["cleared".to_string()]);
    assert_eq!(state(dir.path()).last_green, Some(sha(3)));
}

#[test]
fn a_pending_older_commit_holds_the_scan_so_the_first_red_is_attributed() {
    let dir = tempfile::tempdir().unwrap();
    let mut fake = Fake::default();
    fake.push(1, "base (#1)", green("1"));
    run(dir.path(), &mut fake);
    fake.push(2, "A, CI running (#2)", pending("2"));
    fake.push(3, "B red (#3)", red("3", 1));
    let notes = run(dir.path(), &mut fake);
    assert!(state(dir.path()).episode.is_none(), "B waits behind A");
    assert!(notes.iter().any(|n| n.contains("waiting on")), "{notes:?}");
    fake.checks.insert(sha(2), red("2", 1));
    run(dir.path(), &mut fake);
    let episode = state(dir.path()).episode.unwrap();
    assert_eq!(episode.red_sha, sha(2), "A is the first red");
    assert_eq!(episode.red_merges, vec![sha(3)]);
}

#[test]
fn more_commits_than_one_batch_are_judged_over_passes_without_a_gap() {
    let dir = tempfile::tempdir().unwrap();
    let mut fake = Fake::default();
    fake.push(1, "base (#1)", green("1"));
    run(dir.path(), &mut fake);
    let last = BATCH as u8 + 5;
    for n in 2..=last {
        fake.push(n, &format!("c{n} (#{n})"), green(&n.to_string()));
    }
    fake.checks.insert(sha(last - 1), red("99", 1));
    run(dir.path(), &mut fake);
    assert_eq!(state(dir.path()).cursor, Some(sha(BATCH as u8 + 1)));
    run(dir.path(), &mut fake);
    assert_eq!(state(dir.path()).cursor, Some(sha(last)));
    assert_eq!(
        count(dir.path(), "red_start"),
        1,
        "the red past the first batch"
    );
    assert_eq!(count(dir.path(), "red_end"), 1);
}

#[test]
fn a_cursor_no_longer_on_main_re_anchors() {
    let dir = tempfile::tempdir().unwrap();
    let mut fake = Fake::default();
    fake.push(1, "base (#1)", green("1"));
    run(dir.path(), &mut fake);
    fake.commits.clear();
    fake.push(5, "rewritten (#5)", green("5"));
    let notes = run(dir.path(), &mut fake);
    assert!(
        notes.iter().any(|n| n.contains("re-anchoring")),
        "{notes:?}"
    );
    run(dir.path(), &mut fake);
    assert_eq!(state(dir.path()).cursor, Some(sha(5)));
}

// ---------------------------------------------------------------------------
// Episode boundaries.
// ---------------------------------------------------------------------------

#[test]
fn a_green_rerun_of_the_red_is_a_flake_and_a_newer_red_gets_its_own_episode() {
    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    fake.push(3, "B breaks too (STORY-3) (#12)", red("300", 1));
    run(dir.path(), &mut fake);
    assert_eq!(state(dir.path()).episode.unwrap().red_merges, vec![sha(3)]);
    fake.checks.insert(sha(2), green("200"));
    fake.at(5);
    run(dir.path(), &mut fake);
    assert_eq!(outcomes(dir.path()), vec!["flake".to_string()]);
    let episode = state(dir.path()).episode.unwrap();
    assert_eq!(episode.red_sha, sha(3), "B is judged again on its own");
    assert_eq!(episode.held_green, Some(sha(2)));
}

#[test]
fn an_intervening_green_closes_the_old_episode_before_a_newer_red() {
    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    fake.push(3, "fix (#12)", green("300"));
    fake.push(4, "C breaks (#13)", red("400", 1));
    fake.at(5);
    run(dir.path(), &mut fake);
    assert_eq!(outcomes(dir.path()), vec!["cleared".to_string()]);
    assert_eq!(state(dir.path()).episode.unwrap().red_sha, sha(4));
}

#[test]
fn a_confirmed_red_recovers_reverts_or_is_unattributed_by_its_closing_range() {
    let (dir, mut fake) = confirmed();
    fake.push(3, "fix: repair (#12)", green("300"));
    run(dir.path(), &mut fake);
    assert_eq!(outcomes(dir.path()), vec!["recovered".to_string()]);

    let (dir, mut fake) = confirmed();
    fake.push(
        3,
        "Revert \"fix(x): break main (STORY-2) (#11)\" (#13)",
        green("300"),
    );
    fake.commits.last_mut().unwrap().body = format!("This reverts commit {}.", sha(2));
    fake.push(4, "docs: later (#14)", green("400"));
    run(dir.path(), &mut fake);
    assert_eq!(outcomes(dir.path()), vec!["reverted".to_string()]);

    // The closing commits unreadable: never guessed from other history.
    let (dir, mut fake) = confirmed();
    fake.fail_range = true;
    fake.push(3, "docs: later (#13)", green("300"));
    run(dir.path(), &mut fake);
    assert_eq!(outcomes(dir.path()), vec!["unattributed".to_string()]);
}

// ---------------------------------------------------------------------------
// Confirmation, evidence and recommendations (p24 :659, :690, :1040).
// ---------------------------------------------------------------------------

#[test]
fn a_red_surviving_its_rerun_is_confirmed_and_mailed_once() {
    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    fake.checks.insert(sha(2), red("200", 2));
    fake.at(8);
    run(dir.path(), &mut fake);
    fake.at(9);
    run(dir.path(), &mut fake);
    assert!(state(dir.path()).episode.unwrap().confirmed_at.is_some());
    let key = format!("main-red-{}-confirmed", sha(2));
    assert_eq!(fake.mail_keys().iter().filter(|k| **k == key).count(), 1);
    assert_eq!(count(dir.path(), "confirmed"), 1);
}

#[test]
fn multiply_linked_journal_is_refused_before_tail_repair() {
    let dir = tempfile::tempdir().unwrap();
    let land = land_dir(dir.path()).unwrap();
    let original = dir.path().join("original");
    std::fs::write(&original, b"unfinished").unwrap();
    std::fs::hard_link(&original, land.join(JOURNAL_FILE)).unwrap();
    assert!(append(&land, &event(t0(), "red_start", &sha(1))).is_err());
    assert_eq!(std::fs::read(original).unwrap(), b"unfinished");
}

#[test]
fn unbuildable_is_judged_from_the_current_attempt_only() {
    let compile = JobReport {
        link: "l".into(),
        failed_steps: vec!["Build all packages".into()],
        excerpt: "error[E0425]".into(),
        log_complete: true,
    };
    let (dir, mut fake) = red_on_green();
    fake.jobs
        .insert(("200".into(), BUILD.into()), compile.clone());
    run(dir.path(), &mut fake);
    // The re-run builds and fails only a test.
    fake.checks.insert(sha(2), red("200", 2));
    fake.jobs.insert(
        ("200".into(), BUILD.into()),
        JobReport {
            failed_steps: vec!["Run tests".into()],
            ..compile.clone()
        },
    );
    fake.at(8);
    run(dir.path(), &mut fake);
    let episode = state(dir.path()).episode.unwrap();
    assert!(episode.confirmed_at.is_some());
    assert!(
        !episode.recommended.contains(&"unbuildable".to_string()),
        "attempt 2 built: {:?}",
        episode.recommended
    );
    // A compile failure on the current attempt is recommended, once.
    let (dir, mut fake) = red_on_green();
    fake.jobs.insert(("200".into(), BUILD.into()), compile);
    run(dir.path(), &mut fake);
    fake.at(31);
    run(dir.path(), &mut fake);
    fake.at(32);
    run(dir.path(), &mut fake);
    let key = format!("main-red-{}-revert-unbuildable", sha(2));
    assert_eq!(fake.mail_keys().iter().filter(|k| **k == key).count(), 1);
}

#[test]
fn the_fix_window_runs_from_confirmation() {
    let (dir, mut fake) = red_on_green();
    let window = std::time::Duration::from_secs(10 * 60);
    pass(dir.path(), &required(), window, &mut fake).unwrap();
    fake.at(31);
    pass(dir.path(), &required(), window, &mut fake).unwrap();
    let key = format!("main-red-{}-revert-fix-window-elapsed", sha(2));
    assert!(
        !fake.mail_keys().contains(&key.as_str()),
        "not on confirmation itself"
    );
    fake.at(40);
    pass(dir.path(), &required(), window, &mut fake).unwrap();
    assert!(!fake.mail_keys().contains(&key.as_str()));
    fake.at(42);
    pass(dir.path(), &required(), window, &mut fake).unwrap();
    assert!(fake.mail_keys().contains(&key.as_str()));
}

#[test]
fn evidence_is_kept_per_workflow_run_and_job() {
    let dir = tempfile::tempdir().unwrap();
    let mut fake = Fake::default();
    fake.push(1, "base (#1)", green("1"));
    run(dir.path(), &mut fake);
    let mut other = check(BUILD, CheckOutcome::Failed, "77", 1);
    other.workflow_id = Some("release".into());
    let mut checks = red("70", 1);
    checks.push(other);
    fake.push(2, "two workflows (#2)", checks);
    run(dir.path(), &mut fake);
    let episode = state(dir.path()).episode.unwrap();
    let builds: Vec<&Evidence> = episode
        .evidence
        .iter()
        .filter(|e| e.check == BUILD)
        .collect();
    assert_eq!(builds.len(), 2, "{:?}", episode.evidence);
    assert!(builds.iter().any(|e| e.run_id == "70") && builds.iter().any(|e| e.run_id == "77"));
}

#[test]
fn an_unread_log_is_retried_and_mailed_as_a_follow_up() {
    let (dir, mut fake) = red_on_green();
    fake.log_failures = 2;
    run(dir.path(), &mut fake);
    assert!(state(dir.path()).episode.unwrap().evidence_keys.is_empty());
    fake.at(2);
    run(dir.path(), &mut fake);
    let key = format!("main-red-{}-evidence-200-", sha(2));
    assert!(
        fake.mail_keys().iter().any(|k| k.starts_with(&key)),
        "{:?}",
        fake.mail_keys()
    );
}

#[test]
fn undelivered_mail_is_kept_and_retried_after_the_episode_closes() {
    let (dir, mut fake) = red_on_green();
    fake.fail_mail = true;
    run(dir.path(), &mut fake);
    fake.push(3, "fix (#12)", green("300"));
    run(dir.path(), &mut fake);
    assert!(state(dir.path()).episode.is_none());
    assert_eq!(
        state(dir.path()).outbox.len(),
        2,
        "opening and green-again mail"
    );
    fake.fail_mail = false;
    run(dir.path(), &mut fake);
    assert!(state(dir.path()).outbox.is_empty());
    assert_eq!(fake.mails.len(), 2);
}

// ---------------------------------------------------------------------------
// Durability (p24 :341/:350, :382, :517).
// ---------------------------------------------------------------------------

#[test]
fn transitions_survive_a_crash_at_each_boundary_once() {
    for point in ["journal", "flushed"] {
        let (dir, mut fake) = red_on_green();
        fail_at(Some(point));
        assert!(try_run(dir.path(), &mut fake).is_err(), "{point}");
        fail_at(None);
        run(dir.path(), &mut fake);
        assert_eq!(count(dir.path(), "red_start"), 1, "{point}");
        let raw = std::fs::read_to_string(dir.path().join(JOURNAL)).unwrap();
        assert_eq!(raw.matches("red_start").count(), 1, "{point}: {raw}");
    }
    let (dir, mut fake) = red_on_green();
    fail_at(Some("state"));
    assert!(try_run(dir.path(), &mut fake).is_err());
    fail_at(None);
    run(dir.path(), &mut fake);
    assert_eq!(count(dir.path(), "red_start"), 1);
}

// trace:TASK-3195 | ai:codex
#[test]
fn state_dir_sync_failure_preserves_the_pending_transition_for_restart() {
    let (dir, mut fake) = red_on_green();
    fail_at(Some("state_dir_sync"));
    let result = try_run(dir.path(), &mut fake);
    assert!(format!("{:#}", result.unwrap_err()).contains("injected failure at state_dir_sync"));

    let published = state(dir.path());
    assert_eq!(published.cursor, Some(sha(2)));
    assert_eq!(published.episode.unwrap().red_sha, sha(2));
    assert_eq!(published.pending.len(), 1);
    let transition = &published.pending[0];
    assert_eq!(transition.event, "red_start");
    assert!(read_journal(dir.path()).unwrap().is_empty());

    let retry = try_run(dir.path(), &mut fake);
    fail_at(None);
    assert!(format!("{:#}", retry.unwrap_err()).contains("injected failure at state_dir_sync"));
    assert!(read_journal(dir.path()).unwrap().is_empty());

    run(dir.path(), &mut fake);
    let recovered = state(dir.path());
    assert_eq!(recovered.cursor, Some(sha(2)));
    assert_eq!(recovered.episode.unwrap().red_sha, sha(2));
    assert!(recovered.pending.is_empty());
    assert_eq!(count(dir.path(), "red_start"), 1);
    let raw = std::fs::read_to_string(dir.path().join(JOURNAL)).unwrap();
    assert!(raw.ends_with('\n'));
    let copies = raw
        .lines()
        .map(|line| serde_json::from_str::<Event>(line).unwrap())
        .filter(|event| event.id == transition.id)
        .count();
    assert_eq!(copies, 1, "{raw}");
}

#[test]
fn an_unterminated_or_torn_multibyte_tail_is_uncommitted() {
    let (dir, mut fake) = red_on_green();
    fail_at(Some("journal"));
    assert!(try_run(dir.path(), &mut fake).is_err());
    fail_at(None);
    let pending = state(dir.path()).pending;
    let path = dir.path().join(JOURNAL);
    // The whole event written, its newline lost: not committed.
    std::fs::write(&path, serde_json::to_vec(&pending[0]).unwrap()).unwrap();
    assert!(read_journal(dir.path()).unwrap().is_empty());
    // A crash inside a multibyte character does not block a read either.
    let mut torn = serde_json::to_vec(&pending[0]).unwrap();
    torn.extend_from_slice(b"\n{\"reason\":\"\xe2\x82");
    std::fs::write(&path, &torn).unwrap();
    assert_eq!(read_journal(dir.path()).unwrap().len(), 1);
    std::fs::write(&path, serde_json::to_vec(&pending[0]).unwrap()).unwrap();
    run(dir.path(), &mut fake);
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.ends_with('\n'));
    assert_eq!(raw.matches(&pending[0].id).count(), 1, "{raw}");
}

#[cfg(unix)]
#[test]
fn the_watch_never_writes_through_a_link() {
    let (dir, mut fake) = red_on_green();
    let elsewhere = tempfile::tempdir().unwrap();
    let target = elsewhere.path().join("precious");
    std::fs::write(&target, b"keep").unwrap();
    let journal_path = dir.path().join(JOURNAL);
    let _ = std::fs::remove_file(&journal_path);
    std::os::unix::fs::symlink(&target, &journal_path).unwrap();
    assert!(
        try_run(dir.path(), &mut fake).is_err(),
        "a linked journal is refused"
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"keep");

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), dir.path().join(".aida/land")).unwrap();
    let mut fake = Fake::default();
    fake.push(1, "a (#1)", green("1"));
    assert!(
        try_run(dir.path(), &mut fake).is_err(),
        "a linked .aida/land is refused"
    );
    assert!(!elsewhere.path().join("main-watch.json").exists());
}

#[test]
fn a_busy_lock_is_skipped_but_a_broken_lock_is_an_error() {
    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    let land = dir.path().join(".aida/land");
    let held = try_lock(&land).unwrap().unwrap();
    let notes = run(dir.path(), &mut fake);
    assert!(notes.iter().any(|n| n.contains("skipped")), "{notes:?}");
    drop(held);
    let lock = land.join("main-watch.lock");
    std::fs::remove_file(&lock).unwrap();
    std::fs::create_dir(&lock).unwrap();
    assert!(try_run(dir.path(), &mut fake).is_err(), "not a quiet skip");
}

#[test]
fn a_broken_state_file_fails_the_pass_without_overwriting_it() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida/land")).unwrap();
    std::fs::write(dir.path().join(STATE), b"{not json").unwrap();
    let mut fake = Fake::default();
    fake.push(1, "a (#1)", green("1"));
    assert!(try_run(dir.path(), &mut fake).is_err());
    assert_eq!(std::fs::read(dir.path().join(STATE)).unwrap(), b"{not json");
}

// ---------------------------------------------------------------------------
// Status (p24 :1218) and metrics.
// ---------------------------------------------------------------------------

#[test]
fn the_status_reports_the_tip_apart_from_the_last_green_and_never_green_while_red() {
    let dir = tempfile::tempdir().unwrap();
    let mut fake = Fake::default();
    fake.push(1, "base (#1)", green("1"));
    run(dir.path(), &mut fake);
    fake.push(2, "new tip (#2)", pending("2"));
    run(dir.path(), &mut fake);
    let line = status_line(dir.path()).unwrap();
    assert!(
        line.contains(&format!("tip {}: pending", &sha(2)[..12])),
        "{line}"
    );
    assert!(
        line.contains(&format!("last green {}", &sha(1)[..12])),
        "{line}"
    );
    let (dir, mut fake) = confirmed();
    fake.at(40);
    run(dir.path(), &mut fake);
    let line = status_line(dir.path()).unwrap();
    assert!(
        line.contains("main is red") && line.contains("confirmed"),
        "{line}"
    );
}

fn ev(kind: &str, minutes: i64, outcome: Option<&str>) -> Event {
    let mut e = event(t0() + Duration::minutes(minutes), kind, &sha(2));
    e.id = format!("{kind}-{minutes}");
    e.outcome = outcome.map(str::to_string);
    e
}

#[test]
fn metrics_count_only_the_window_and_flakes_apart() {
    let at = |m: i64| t0() + Duration::minutes(m);
    let events = vec![
        ev("red_start", 0, None),
        ev("red_merge", 10, None),
        ev("red_merge", 70, None),
        ev("red_end", 300, Some("recovered")),
    ];
    let m = metrics(&events, at(60), at(120));
    assert_eq!(m["red_minutes"], 60);
    assert_eq!(m["red_merges"], 1);
    assert_eq!(
        m["resolved"], 0,
        "the end after the window is still open there"
    );
    let m = metrics(&events, at(-10), at(400));
    assert_eq!(m["fix_latency_seconds_median"], 300 * 60);
    let flake = vec![ev("red_start", 0, None), ev("red_end", 5, Some("flake"))];
    let m = metrics(&flake, at(-10), at(100));
    assert_eq!(m["flakes"], 1);
    assert_eq!(m["resolved"], 0);
    assert_eq!(median(&[600, 1800]), Some(1200));
    assert_eq!(median(&[1, 2, 9]), Some(2));
}

// ---------------------------------------------------------------------------
// Supporting pieces.
// ---------------------------------------------------------------------------

/// Merging continues while main is red, and nothing that lands can stall
/// on the watch: no merge or landing path reads or calls it. In land.rs
/// only the `land watch` arm may name it.
#[test]
fn no_merge_or_landing_path_reads_or_calls_the_red_main_watch() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stack = vec![src.join("gate")];
    for module in [
        "pr_cmd",
        "pr_service",
        "pr_train",
        "pr_ship",
        "land_run",
        "land_housekeeping",
        "dev_autopromote",
        "dev_build",
        "drain_cmd",
        "land_bindings",
        "upgrade_cmd",
        "land_ci_base",
        "promotion_failures",
        "auto_complete",
        "burndown_run",
        "upgrade_release",
    ] {
        let (file, dir) = (src.join(format!("{module}.rs")), src.join(module));
        assert!(
            file.is_file() || dir.is_dir(),
            "landing module {module} is gone"
        );
        stack.extend([file, dir].into_iter().filter(|p| p.exists()));
    }
    for entry in std::fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if [
            "advisor_loop",
            "drain_",
            "burndown_",
            "pr_train_",
            "pr_ship_",
            "land_run_",
        ]
        .iter()
        .any(|prefix| name.starts_with(prefix))
        {
            stack.push(path);
        }
    }
    let mut offenders = Vec::new();
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            stack.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
        } else if path.extension().is_some_and(|e| e == "rs")
            && std::fs::read_to_string(&path)
                .unwrap()
                .contains("main_watch")
        {
            offenders.push(path.display().to_string());
        }
    }
    assert!(
        offenders.is_empty(),
        "landing paths name main_watch: {offenders:?}"
    );
    // land.rs: every mention sits inside the `LandCommand::Watch` arm.
    let land = std::fs::read_to_string(src.join("land.rs")).unwrap();
    let arm = land.find("LandCommand::Watch =>").expect("the watch arm");
    let arm_end = arm + land[arm..].find("\n        }\n").expect("arm end");
    for (at, _) in land.match_indices("main_watch") {
        assert!(
            (arm..arm_end).contains(&at),
            "land.rs names main_watch outside the watch arm at byte {at}"
        );
    }
}

#[test]
fn a_stalled_fetch_returns_within_the_call_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let git = |args: &[&str]| {
        assert!(crate::test_git::git_command()
            .current_dir(root)
            .args(args)
            .status()
            .unwrap()
            .success());
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "protocol.ext.allow", "always"]);
    git(&["remote", "add", "origin", "ext::sh -c sleep% 60"]);
    let mut live = Live {
        pinned_tip: None,
        root: root.to_path_buf(),
        forge: crate::forge::forge_for(root),
        call_timeout: std::time::Duration::from_secs(1),
    };
    let started = std::time::Instant::now();
    assert!(pass(root, &required(), WINDOW, &mut live).is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
}

#[cfg(unix)]
#[test]
fn a_job_log_is_read_as_a_bounded_tail() {
    let dir = tempfile::tempdir().unwrap();
    let cli = dir.path().join("fake-gh");
    crate::test_exec::write_executable(
        &cli,
        "#!/bin/sh\nhead -c 3000000 /dev/zero | tr '\\0' 'a'\nprintf 'ERROR at the end\\n'\n",
    );
    let tail = crate::forge::commit_ci_api_tail(&cli, dir.path(), "x", 1024).unwrap();
    assert_eq!(tail.len(), 1024);
    assert!(String::from_utf8_lossy(&tail).ends_with("ERROR at the end\n"));
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
    }
}

/// The fix window is set through the config editor as a validated duration;
/// an inline `landing` table is never replaced (its keys would be lost).
#[cfg(feature = "tui")]
#[test]
fn the_fix_window_is_settable_and_inline_tables_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    let config = dir.path().join(".aida/config.toml");
    let item = aida_tui::ConfigMenuItem {
        section: "landing".into(),
        name: "red_main.fix_window".into(),
        value: "2h".into(),
        default: "2h".into(),
        scope: ".aida/config.toml".into(),
        explanation: String::new(),
        edit: crate::config_cmd::config_knob_edit_kind("landing", "red_main.fix_window"),
        read_only_reason: None,
    };
    assert_eq!(item.edit, aida_tui::EditKind::Duration);
    let set = |value| crate::config_cmd::cli_edit_config_knob(dir.path(), &item, Some(value));
    std::fs::write(&config, "[landing]\nmode = \"ci-gated\"\n").unwrap();
    assert!(matches!(set("soon"), aida_tui::EditOutcome::Blocked(_)));
    assert!(matches!(set("90m"), aida_tui::EditOutcome::Updated { .. }));
    let body = std::fs::read_to_string(&config).unwrap();
    assert_eq!(
        crate::landing_policy::from_body(Some(&body)).red_main_fix_window,
        std::time::Duration::from_secs(90 * 60)
    );
    assert!(body.contains("mode = \"ci-gated\""), "{body}");
    for inline in [
        "landing = { mode = \"ci-gated\" }\n",
        "[landing]\nmode = \"ci-gated\"\nred_main = { fix_window = \"2h\" }\n",
    ] {
        std::fs::write(&config, inline).unwrap();
        assert!(
            matches!(set("90m"), aida_tui::EditOutcome::Blocked(_)),
            "{inline}"
        );
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            inline,
            "unchanged"
        );
    }
}

#[test]
fn durations_display_without_loss() {
    use crate::landing_policy::duration_text;
    let d = std::time::Duration::from_secs;
    assert_eq!(duration_text(d(90)), "1m30s");
    assert_eq!(duration_text(d(7200)), "2h");
    for secs in [60, 90, 5400, 3661] {
        assert_eq!(
            crate::presence::parse_duration_secs(&duration_text(d(secs))),
            Some(secs)
        );
    }
}

#[test]
fn the_excerpt_keeps_the_error_lines() {
    let log = "Run tests\nerror[E0425]: x\ntest a ... FAILED\nthread 'x' panicked\nfine\n";
    let text = excerpt(log, Some("Run tests"));
    assert!(text.contains("error[E0425]") && text.contains("FAILED") && text.contains("panicked"));
    assert!(!text.contains("fine"));
}

#[test]
fn subject_pr_reads_the_squash_suffix() {
    assert_eq!(subject_pr("feat: x (STORY-1) (#4324)"), Some(4324));
    assert_eq!(subject_pr("feat: x (STORY-1)"), None);
}

#[test]
fn first_pass_with_only_red_history_includes_oldest_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let mut fake = Fake::default();
    fake.push(1, "first red", red("1", 1));
    fake.at(12);
    fake.push(2, "still red", red("2", 1));
    fake.at(20);
    run(dir.path(), &mut fake);
    let episode = state(dir.path()).episode.unwrap();
    assert_eq!(episode.red_sha, sha(1));
    assert_eq!(episode.red_merges, vec![sha(2)]);
    assert_eq!(episode.opened_at, t0() + Duration::minutes(20));
    assert_eq!(journal(dir.path())[0].at, t0());
    assert_eq!(journal(dir.path())[1].at, t0() + Duration::minutes(12));
}

#[test]
fn closing_stdout_does_not_release_the_job_log_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let cli = dir.path().join("fake-gh");
    crate::test_exec::write_executable(&cli, "#!/bin/sh\nexec 1>&-\nexec sleep 10\n");
    let start = std::time::Instant::now();
    let result = crate::forge_lookup::with_forge_lookup_deadline(
        std::time::Duration::from_millis(150),
        || crate::forge::commit_ci_api_tail(&cli, dir.path(), "endpoint", 64),
    );
    assert!(result.is_err());
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn github_attempt_evidence_preserves_carried_job_and_rejects_replaced_job() {
    use serde_json::json;
    let head = sha(1);
    let mut api = |endpoint: &str| -> Result<Vec<u8>> {
        let body = if endpoint.contains("/check-runs") {
            let rows: Vec<_> = [(1, "carried"), (2, "replaced"), (3, "current")]
                .into_iter()
                .map(|(id, name)| {
                    json!({"id": id, "name": name, "head_sha": head,
                    "status": "completed", "conclusion": "success",
                    "details_url": format!("https://github.com/o/r/actions/runs/7/job/{id}")})
                })
                .collect();
            json!({"total_count": rows.len(), "check_runs": rows})
        } else if endpoint.contains("/jobs?") {
            assert!(endpoint.contains("filter=all"));
            json!({"total_count": 4, "jobs": [
                {"id": 2, "run_id": 7, "run_attempt": 1, "status":"completed", "name": "replaced"},
                {"id": 1, "run_id": 7, "run_attempt": 1, "status":"completed", "name": "carried"},
                {"id": 4, "run_id": 7, "run_attempt": 2, "status":"completed", "name": "replaced"},
                {"id": 3, "run_id": 7, "run_attempt": 2, "status":"completed", "name": "current"}
            ]})
        } else if endpoint.ends_with("/actions/runs/7") {
            json!({"id": 7, "head_sha": head, "workflow_id": 3, "run_attempt": 2, "status": "completed"})
        } else {
            json!({"sha": head, "total_count": 0, "statuses": []})
        };
        Ok(serde_json::to_vec(&body)?)
    };
    let checks = crate::forge::github_checks_with_attempts("o/r", &head, &mut api, true)
        .unwrap()
        .checks;
    assert_eq!(
        checks
            .iter()
            .map(|c| (c.run_attempt, c.run_in_progress))
            .collect::<Vec<_>>(),
        vec![(Some(1), false), (None, true), (Some(2), false)]
    );
}

#[test]
fn production_history_returns_oldest_bounded_batch_and_rejects_side_parent() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let git = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .current_dir(root)
            .env("GIT_AUTHOR_NAME", "fixture")
            .env("GIT_COMMITTER_NAME", "fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.org")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.org")
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{:?}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["commit", "-q", "--allow-empty", "-m", "root"]);
    let cursor = git(&["rev-parse", "HEAD"]);
    git(&["checkout", "-q", "-b", "side"]);
    git(&["commit", "-q", "--allow-empty", "-m", "side"]);
    let side = git(&["rev-parse", "HEAD"]);
    git(&["checkout", "-q", "main"]);
    for name in ["one", "two", "three"] {
        git(&["commit", "-q", "--allow-empty", "-m", name]);
    }
    git(&["merge", "-q", "--no-ff", "side", "-m", "merge"]);
    let mut live = Live {
        root: root.into(),
        forge: crate::forge::forge_for(root),
        pinned_tip: Some(git(&["rev-parse", "HEAD"])),
        call_timeout: std::time::Duration::from_secs(5),
    };
    let batch = live.after(&cursor, 2).unwrap().unwrap();
    assert_eq!(
        batch.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(),
        ["one", "two"]
    );
    assert!(live.after(&side, 2).unwrap().is_none());
    assert!(live.after(&"f".repeat(40), 2).is_err());
}

#[test]
fn github_attempt_evidence_reads_page_two_and_refuses_another_attempt() {
    use serde_json::json;
    let mut api = |endpoint: &str| -> Result<Vec<u8>> {
        assert!(endpoint.contains("/runs/17/attempts/2/jobs?per_page=100&page="));
        let jobs = if endpoint.ends_with("page=1") {
            (0..100).map(|id| json!({"id":id})).collect::<Vec<_>>()
        } else {
            vec![
                json!({"id":110, "name":"CI (rest)", "run_id":17, "run_attempt":2,
                "status":"completed", "conclusion":"failure", "html_url":"job",
                "steps":[{"name":"Build skipped","conclusion":"skipped"},{"name":"Run tests","conclusion":"failure"}]}),
            ]
        };
        Ok(serde_json::to_vec(
            &json!({"total_count":101, "jobs":jobs}),
        )?)
    };
    let report =
        crate::forge::github_watch_job("o/r", "17", 2, 110, "CI (rest)", &mut api).unwrap();
    assert_eq!(report, ("job".into(), vec!["Run tests".into()]));
    let mut stale = |_: &str| {
        Ok(serde_json::to_vec(&json!({"total_count":1,"jobs":[{
        "id":110,"name":"CI (rest)", "run_id":17,"run_attempt":1, "status":"completed",
        "conclusion":"failure","steps":[] }]}))?)
    };
    assert!(crate::forge::github_watch_job("o/r", "17", 2, 110, "CI (rest)", &mut stale).is_err());
}

#[test]
fn journal_reads_stay_with_pinned_directory_and_refuse_oversized_input() {
    let dir = tempfile::tempdir().unwrap();
    let pinned = land_dir(dir.path()).unwrap();
    let visible = dir.path().join(".aida/land");
    std::fs::rename(&visible, dir.path().join(".aida/original")).unwrap();
    std::fs::create_dir(&visible).unwrap();
    append(&pinned, &event(t0(), "red_start", &sha(1))).unwrap();
    assert_eq!(journal_in(&pinned).unwrap().len(), 1);
    assert!(read_journal(dir.path()).unwrap().is_empty());
    let journal = std::fs::File::create(visible.join(JOURNAL_FILE)).unwrap();
    journal.set_len(WATCH_FILE_LIMIT + 1).unwrap();
    assert!(read_journal(dir.path()).is_err());
}

#[test]
fn a_pending_first_candidate_survives_restart_and_a_later_green() {
    let dir = tempfile::tempdir().unwrap();
    let mut fake = Fake::default();
    fake.push(1, "first", pending("1"));
    run(dir.path(), &mut fake);
    fake.checks.insert(sha(1), red("1", 1));
    fake.push(2, "second", green("2"));
    run(dir.path(), &mut fake);
    assert_eq!(count(dir.path(), "red_start"), 1);
    assert_eq!(count(dir.path(), "red_end"), 1);
}

#[test]
fn replacement_lookup_retries_without_reporting_a_previous_jobs_log() {
    let (dir, mut fake) = red_on_green();
    let root = dir.path();
    fake.log_failures = 2;
    run(root, &mut fake);
    fake.at(1);
    run(root, &mut fake);
    let logs = |fake: &Fake| {
        fake.mails
            .iter()
            .filter(|(k, _, _)| k.contains("-evidence-"))
            .count()
    };
    assert_eq!(logs(&fake), 2);

    let mut replacement = check(BUILD, CheckOutcome::Failed, "200", 2);
    replacement.job_id = Some(22);
    fake.checks.insert(sha(2), vec![replacement]);
    fake.job_unavailable = true;
    fake.at(2);
    run(root, &mut fake);
    let mail = fake.mail(&format!("main-red-{}-confirmed", sha(2)));
    assert!(mail.contains("evidence not read yet"));
    assert!(!mail.contains("ci.example") && !mail.contains("Run tests") && !mail.contains("foo"));

    fake.job_unavailable = false;
    fake.jobs.insert(
        ("200".into(), BUILD.into()),
        JobReport {
            link: "replacement-link".into(),
            failed_steps: vec![],
            excerpt: "replacement-log".into(),
            log_complete: true,
        },
    );
    fake.at(3);
    run(root, &mut fake);
    run(root, &mut fake);
    assert_eq!(logs(&fake), 3);
    assert!(fake.mails.last().unwrap().2.contains("replacement-log"));
}

#[test]
fn a_green_tip_rerun_opens_an_episode_without_a_merge() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut fake = Fake::default();
    fake.push(1, "base", green("100"));
    fake.push(2, "tip", green("200"));
    run(root, &mut fake);
    fake.checks.insert(sha(2), red("200", 2));
    fake.at(1);
    run(root, &mut fake);
    run(root, &mut fake);
    let s = state(root);
    let ep = s.episode.unwrap();
    assert_eq!(ep.red_sha, sha(2));
    assert_eq!(s.last_green, Some(sha(1)));
    assert_eq!(ep.held_green, s.last_green);
    assert_eq!(count(root, "red_start"), 1);
    assert_eq!(fake.mails.len(), 1);
}

#[test]
fn live_pass_preserves_stale_pure_git_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for args in [
        vec!["init", "-q"],
        vec![
            "remote",
            "add",
            "origin",
            "https://github.com/example/watch.git",
        ],
    ] {
        assert!(crate::test_git::git_command()
            .current_dir(root)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    let land = land_dir(root).unwrap();
    let config = root.join(".aida/config.toml");
    let original = "[forge]\nprovider = \"pure-git\"\n";
    std::fs::write(&config, original).unwrap();
    // Exercise production provider selection, then stop before any network access.
    let _lock = try_lock(&land).unwrap().unwrap();
    assert!(live_pass(root)
        .unwrap()
        .iter()
        .any(|n| n.contains("skipped")));
    assert_eq!(std::fs::read_to_string(config).unwrap(), original);
}

// trace:TASK-3209 | ai:codex
#[test]
fn exhausted_pass_confirms_verified_red_and_retries_unresolved_commit() {
    use std::time::{Duration as StdDuration, Instant};
    assert!(
        PASS_BUDGET + StdDuration::from_secs(30)
            <= crate::maintenance_schedule::SCHEDULED_COMMAND_TIMEOUT
    );
    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    fake.at(29);
    fake.push(3, "repair", green("300"));
    fake.stall_check = Some(sha(3));
    let started = Instant::now();
    let notes = crate::forge_lookup::with_forge_lookup_deadline(StdDuration::from_secs(1), || {
        run(dir.path(), &mut fake)
    });
    assert!(started.elapsed() < StdDuration::from_secs(8));
    assert!(notes.iter().any(|n| n.contains("resuming next pass")));
    assert_eq!(state(dir.path()).cursor, Some(sha(2)));
    assert_eq!(count(dir.path(), "confirmed"), 1);
    fake.stall_check = None;
    run(dir.path(), &mut fake);
    assert_eq!(state(dir.path()).cursor, Some(sha(3)));
    assert!(state(dir.path()).episode.is_none());
}

// trace:TASK-3209 | ai:codex
#[test]
fn history_deadlines_checkpoint_and_resume() {
    for stage in 0..3 {
        let (dir, mut fake) = confirmed();
        fake.checks.insert(sha(2), green("200"));
        fake.push(3, "next", green("300"));
        fake.deadline_recent = stage == 0;
        fake.after_until_deadline = match stage {
            1 => Some(0),
            2 => Some(1),
            _ => None,
        };
        let notes = run(dir.path(), &mut fake);
        assert!(notes.iter().any(|n| n.contains("resuming next pass")));
        assert_eq!(state(dir.path()).episode.is_none(), stage == 2);
        fake.deadline_recent = false;
        fake.after_until_deadline = None;
        run(dir.path(), &mut fake);
        assert_eq!(state(dir.path()).cursor, Some(sha(3)));
        assert_eq!(count(dir.path(), "red_end"), 1);
    }
}

// trace:TASK-3209 | ai:codex
#[test]
fn closing_range_deadline_retries_same_green() {
    let (dir, mut fake) = confirmed();
    fake.push(3, "repair", green("300"));
    fake.deadline_range = true;
    run(dir.path(), &mut fake);
    let held = state(dir.path());
    assert!(held.episode.is_some());
    assert_eq!(held.cursor, Some(sha(2)));
    assert_eq!(held.last_green, Some(sha(1)));
    assert_eq!(count(dir.path(), "red_end"), 0);
    fake.deadline_range = false;
    run(dir.path(), &mut fake);
    assert_eq!(outcomes(dir.path()), vec!["recovered"]);
}

// trace:TASK-3209 | ai:codex
#[test]
fn git_timeout_at_call_ceiling_observes_pass_deadline() {
    let end = std::time::Instant::now() + CALL_TIMEOUT;
    let tick = std::time::Duration::from_nanos(1);
    for (deadline, observed, retry) in [
        (Some(end), end - tick, false),
        (Some(end), end, true),
        (Some(end), end + tick, true),
        (None, end, false),
    ] {
        let error = git_timeout_error(&["log"], CALL_TIMEOUT, deadline, observed);
        assert_eq!(error.is::<PassDeadline>(), retry);
    }
}

// trace:TASK-3859 | ai:codex
#[test]
fn task_3859_watch_propagates_gh_auth_errors_but_waits_on_transient_reads() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &[
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.org",
            "commit",
            "-qm",
            "seed",
            "--allow-empty",
        ],
        &["remote", "add", "origin", "."],
    ] {
        assert!(crate::test_git::git_command()
            .current_dir(root)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    let gh = root.join("gh");
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_TEST_GH_BINARY", gh.to_str())]);
    let mut live = Live {
        pinned_tip: None,
        root: root.into(),
        forge: crate::forge::forge_for_kind(root, crate::forge::ForgeKind::GitHub),
        call_timeout: CALL_TIMEOUT,
    };
    for (status, diagnostic) in [
        (1, "gh: Bad credentials (HTTP 401)"),
        (4, "login required"),
        (
            1,
            "To get started with GitHub CLI, please run: gh auth login",
        ),
    ] {
        crate::test_exec::write_executable(
            &gh,
            format!("#!/bin/sh\necho '{diagnostic}' >&2\nexit {status}\n"),
        );
        let error = pass(root, &required(), WINDOW, &mut live).unwrap_err();
        assert!(format!("{error:#}").contains("GitHub CLI authentication failed"));
    }
    crate::test_exec::write_executable(&gh, "#!/bin/sh\necho 'HTTP 503' >&2\nexit 1\n");
    pass(root, &required(), WINDOW, &mut live).unwrap();
    let status = status_line(root).unwrap();
    assert!(
        status.contains("pending") && status.contains("503"),
        "{status}"
    );
}

/// `open_episode` reports an episode only with a full red SHA, at least one
/// failed check and no blank check name; anything else reads as none.
// trace:STORY-2897 | ai:claude
#[test]
fn open_episode_refuses_a_short_sha_an_empty_failed_list_and_a_blank_name() {
    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    fake.at(5);
    run(dir.path(), &mut fake);
    let root = dir.path();
    let (red, names) = open_episode_at(root).expect("open episode");
    assert_eq!(red, sha(2));
    assert!(names.iter().any(|n| n == BUILD), "{names:?}");
    let good = state(root);
    let rewrite = |edit: &dyn Fn(&mut Episode)| {
        let mut changed = good.clone();
        edit(changed.episode.as_mut().unwrap());
        write_state(&root.join(".aida/land"), &changed).unwrap();
        open_episode_at(root)
    };
    assert!(rewrite(&|e| e.red_sha = sha(2)[..12].to_string()).is_none());
    assert!(rewrite(&|e| e.red_sha = "g".repeat(40)).is_none());
    assert!(rewrite(&|e| e.failed.clear()).is_none());
    assert!(rewrite(&|e| e.failed[0].name = "  ".into()).is_none());
    assert!(rewrite(&|e| e.diagnostics.push(String::new())).is_none());
    assert!(
        rewrite(&|_| ()).is_some(),
        "the unedited episode still reads"
    );
    let mut closed = good.clone();
    closed.episode = None;
    write_state(&root.join(".aida/land"), &closed).unwrap();
    assert!(open_episode_at(root).is_none());
}
