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
        run_in_progress: false,
    }
}

fn green(run: &str) -> Vec<CommitCheck> {
    vec![check(BUILD, CheckOutcome::Passed, run, 1)]
}

fn red(run: &str, attempt: u64) -> Vec<CommitCheck> {
    vec![check(BUILD, CheckOutcome::Failed, run, attempt)]
}

fn pending(run: &str) -> Vec<CommitCheck> {
    vec![check(BUILD, CheckOutcome::Pending, run, 1)]
}

#[derive(Default)]
struct Fake {
    now: Option<DateTime<Utc>>,
    /// Newest first.
    commits: Vec<Commit>,
    checks: HashMap<String, Vec<CommitCheck>>,
    evidence: HashMap<String, Evidence>,
    evidence_failures: usize,
    log_failures: usize,
    comments: Vec<(String, String)>,
    reruns: Vec<String>,
    mails: Vec<(String, String, String)>,
    notices: Vec<String>,
    human_opens: Vec<(String, String)>,
    human_resolves: Vec<String>,
    filed: Vec<FixSpec>,
    fix_state: FixState,
    fail_mail: bool,
    fail_operator: bool,
    fail_human: bool,
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
    fn history(&mut self, stop: Option<&str>, max: usize) -> Result<Vec<Commit>> {
        Ok(self
            .commits
            .iter()
            .take_while(|c| Some(c.sha.as_str()) != stop)
            .take(max)
            .cloned()
            .collect())
    }
    fn range(&mut self, from: &str, to: &str) -> Result<Vec<Commit>> {
        Ok(self
            .commits
            .iter()
            .skip_while(|c| c.sha != to)
            .take_while(|c| c.sha != from)
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
    fn evidence(&mut self, run_id: &str, job: &str) -> Result<Evidence> {
        if self.evidence_failures > 0 {
            self.evidence_failures -= 1;
            anyhow::bail!("gh unavailable");
        }
        let mut found = self.evidence.get(run_id).cloned().unwrap_or(Evidence {
            check: job.into(),
            link: format!("https://ci.example/runs/{run_id}"),
            failed_steps: vec!["Run tests".into()],
            excerpt: "test foo ... FAILED".into(),
            log_complete: true,
        });
        if self.log_failures > 0 {
            self.log_failures -= 1;
            found.excerpt = "(the job log could not be read yet)".into();
            found.log_complete = false;
        }
        Ok(found)
    }
    fn rerun_failed(&mut self, run_id: &str) -> Result<()> {
        self.reruns.push(run_id.into());
        Ok(())
    }
    fn mail(&mut self, key: &str, subject: &str, body: &str) -> Result<()> {
        anyhow::ensure!(!self.fail_mail, "mailbox unavailable");
        if !self.mails.iter().any(|(k, _, _)| k == key) {
            self.mails.push((key.into(), subject.into(), body.into()));
        }
        Ok(())
    }
    fn notify_operator(&mut self, key: &str, _title: &str, _body: &str) -> Result<()> {
        anyhow::ensure!(!self.fail_operator, "push channel down");
        self.notices.push(key.into());
        Ok(())
    }
    fn comment_fix_spec(&mut self, id: &str, text: &str) -> Result<()> {
        self.comments.push((id.into(), text.into()));
        Ok(())
    }
    fn human_open(&mut self, red_sha: &str, _title: &str, body: &str) -> Result<String> {
        anyhow::ensure!(!self.fail_human, "store busy");
        self.human_opens.push((red_sha.into(), body.into()));
        Ok("TASK-7000".into())
    }
    fn human_resolve(&mut self, red_sha: &str) -> Result<()> {
        anyhow::ensure!(!self.fail_human, "store busy");
        self.human_resolves.push(red_sha.into());
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

fn try_run(root: &Path, fake: &mut Fake) -> Result<Vec<String>> {
    pass(
        root,
        &required(),
        std::time::Duration::from_secs(7200),
        fake,
    )
}

fn state(root: &Path) -> State {
    read_state(root).unwrap()
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

/// A base fixture: a green commit, then a red merge on top.
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
    (dir, fake)
}

// ---------------------------------------------------------------------------
// Detection.
// ---------------------------------------------------------------------------

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
    assert!(matches!(
        judge(checks(pending("1")), &required()),
        Verdict::Unknown(_)
    ));
    assert!(matches!(
        judge(Err(anyhow::anyhow!("gh down")), &required()),
        Verdict::Unknown(_)
    ));
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
    fake.push(1, "feat: a (#1)", pending("1"));
    let notes = run(dir.path(), &mut fake);
    assert!(state(dir.path()).episode.is_none());
    assert!(notes.iter().any(|n| n.contains("unknown")), "{notes:?}");
    assert!(fake.mails.is_empty());
}

// ---------------------------------------------------------------------------
// Opening, flake damping and confirmation (signed amendment).
// ---------------------------------------------------------------------------

#[test]
fn a_red_opens_one_episode_mails_once_with_evidence_and_files_nothing_yet() {
    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    fake.at(5);
    run(dir.path(), &mut fake);
    let episode = state(dir.path()).episode.expect("an open episode");
    assert_eq!(episode.red_sha, sha(2));
    assert_eq!(episode.pr, Some(11));
    assert_eq!(episode.specs, vec!["STORY-2".to_string()]);
    assert_eq!(episode.held_green, Some(sha(1)));
    let key = format!("main-red-{}", sha(2));
    assert_eq!(fake.mail_keys(), vec![key.as_str()]);
    // F9: the alert carries the link, the log excerpt and the held commit.
    let body = fake.mail(&key);
    assert!(body.contains("https://ci.example/runs/200"), "{body}");
    assert!(body.contains("test foo ... FAILED"), "{body}");
    assert!(body.contains(&sha(1)[..12]), "{body}");
    assert_eq!(
        fake.reruns,
        vec!["200".to_string()],
        "one confirming re-run"
    );
    assert!(fake.filed.is_empty() && fake.notices.is_empty() && fake.human_opens.is_empty());
    assert_eq!(count(dir.path(), "red_start"), 1);
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
    assert!(fake.filed.is_empty() && fake.notices.is_empty() && fake.human_opens.is_empty());
    let m = metrics(&journal(dir.path()), t0(), t0() + Duration::hours(1));
    assert_eq!(m["flakes"], 1);
    assert_eq!(m["resolved"], 0, "a flake is not a resolved red");
    assert_eq!(m["fix_latency_seconds_median"], serde_json::Value::Null);
}

#[test]
fn a_red_that_survives_its_rerun_files_one_fix_spec_with_evidence() {
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
    assert!(fix.description.contains("https://ci.example/runs/200"));
    assert!(fix.description.contains("test foo ... FAILED"));
    assert!(fix.description.contains(&sha(1)[..12]), "held last green");
    assert_eq!(episode.fix_spec.as_deref(), Some("BUG-9000"));
    assert_eq!(fake.notices, vec![format!("main-red-{}-confirmed", sha(2))]);
    let (red_sha, body) = fake.human_opens.last().unwrap();
    assert_eq!(red_sha, &sha(2));
    assert!(body.contains("BUG-9000"), "{body}");
}

#[test]
fn a_red_still_pending_its_rerun_is_confirmed_after_thirty_minutes() {
    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    fake.checks.insert(sha(2), pending("200"));
    fake.at(29);
    run(dir.path(), &mut fake);
    assert!(fake.filed.is_empty());
    fake.at(31);
    run(dir.path(), &mut fake);
    assert!(state(dir.path()).episode.unwrap().confirmed_at.is_some());
    assert_eq!(fake.filed.len(), 1);
}

// ---------------------------------------------------------------------------
// F1, F6: episode boundaries and the commits recorded in them.
// ---------------------------------------------------------------------------

#[test]
fn a_green_rerun_of_the_old_red_closes_it_and_a_newer_red_opens_its_own_episode() {
    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    fake.checks.insert(sha(2), green("200"));
    fake.push(3, "feat: b breaks (STORY-3) (#12)", red("300", 1));
    fake.at(5);
    run(dir.path(), &mut fake);
    assert_eq!(outcomes(dir.path()), vec!["flake".to_string()]);
    let episode = state(dir.path()).episode.unwrap();
    assert_eq!(episode.red_sha, sha(3), "B is its own episode");
    assert_eq!(episode.held_green, Some(sha(2)));
    assert_eq!(count(dir.path(), "red_start"), 2);
}

#[test]
fn an_intervening_green_commit_closes_the_old_episode_before_a_newer_red() {
    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    fake.push(3, "fix: repair (#12)", green("300"));
    fake.push(4, "feat: c breaks (#13)", red("400", 1));
    fake.at(5);
    run(dir.path(), &mut fake);
    assert_eq!(outcomes(dir.path()), vec!["cleared".to_string()]);
    let episode = state(dir.path()).episode.unwrap();
    assert_eq!(episode.red_sha, sha(4));
    assert!(episode.red_merges.is_empty());
}

#[test]
fn every_commit_landing_during_an_episode_is_recorded_even_if_main_turns_green_next() {
    let dir = tempfile::tempdir().unwrap();
    let mut fake = Fake::default();
    fake.push(1, "base (#1)", green("1"));
    run(dir.path(), &mut fake);
    fake.push(2, "first bad (STORY-2) (#2)", red("2", 1));
    fake.push(3, "second (#3)", red("3", 1));
    fake.push(4, "third, CI pending (#4)", pending("4"));
    run(dir.path(), &mut fake);
    let episode = state(dir.path()).episode.unwrap();
    assert_eq!(episode.red_sha, sha(2));
    assert_eq!(
        episode.red_merges,
        vec![sha(3), sha(4)],
        "recorded on opening"
    );
    fake.at(10);
    fake.push(5, "fix (#5)", green("5"));
    run(dir.path(), &mut fake);
    let m = metrics(&journal(dir.path()), t0(), t0() + Duration::hours(1));
    assert_eq!(m["red_merges"], 3);
    assert_eq!(m["episodes"], 1);
}

#[test]
fn a_commit_older_than_the_red_neither_joins_nor_closes_the_episode() {
    let dir = tempfile::tempdir().unwrap();
    let mut fake = Fake::default();
    fake.push(1, "base (#1)", green("1"));
    run(dir.path(), &mut fake);
    fake.push(2, "pending first (#2)", pending("2"));
    fake.push(3, "red (#3)", red("3", 1));
    run(dir.path(), &mut fake);
    assert_eq!(state(dir.path()).episode.as_ref().unwrap().red_sha, sha(3));
    fake.checks.insert(sha(2), green("2"));
    run(dir.path(), &mut fake);
    let episode = state(dir.path()).episode.expect("still open");
    assert!(episode.red_merges.is_empty());
}

// ---------------------------------------------------------------------------
// Revert triggers (F10, F13) and the configured window.
// ---------------------------------------------------------------------------

#[test]
fn each_revert_trigger_is_escalated_once_after_confirmation() {
    let (dir, mut fake) = red_on_green();
    fake.evidence.insert(
        "200".into(),
        Evidence {
            check: BUILD.into(),
            link: "l".into(),
            failed_steps: vec!["Build all packages".into()],
            excerpt: "error[E0425]".into(),
            log_complete: true,
        },
    );
    run(dir.path(), &mut fake);
    let unbuildable = format!("main-red-{}-revert-unbuildable", sha(2));
    assert!(!fake.mail_keys().contains(&unbuildable.as_str()));
    fake.at(31);
    run(dir.path(), &mut fake);
    fake.at(32);
    run(dir.path(), &mut fake);
    let n = |fake: &Fake, key: &str| fake.mail_keys().iter().filter(|k| **k == key).count();
    assert_eq!(n(&fake, &unbuildable), 1);
    fake.fix_state.needs_design = true;
    fake.at(33);
    run(dir.path(), &mut fake);
    assert_eq!(
        n(&fake, &format!("main-red-{}-revert-needs-design", sha(2))),
        1
    );
    fake.at(121);
    run(dir.path(), &mut fake);
    fake.at(125);
    run(dir.path(), &mut fake);
    let window = format!("main-red-{}-revert-fix-window-elapsed", sha(2));
    assert_eq!(n(&fake, &window), 1);
    assert_eq!(
        state(dir.path()).episode.unwrap().recommended,
        vec!["unbuildable", "needs-design", "fix-window-elapsed"]
    );
    assert_eq!(count(dir.path(), "revert_recommended"), 3);
}

#[test]
fn a_transient_evidence_read_is_retried_and_still_finds_an_unbuildable_main() {
    let (dir, mut fake) = red_on_green();
    fake.evidence_failures = 1;
    fake.evidence.insert(
        "200".into(),
        Evidence {
            check: BUILD.into(),
            link: "l".into(),
            failed_steps: vec!["Build aida binary for lib unit tests".into()],
            excerpt: String::new(),
            log_complete: true,
        },
    );
    run(dir.path(), &mut fake);
    assert!(state(dir.path()).episode.unwrap().unbuildable.is_none());
    fake.at(31);
    run(dir.path(), &mut fake);
    let episode = state(dir.path()).episode.unwrap();
    assert!(episode.unbuildable.is_some(), "the probe was retried");
    assert!(episode.recommended.contains(&"unbuildable".to_string()));
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
fn a_design_fork_punt_needs_design_and_other_punts_do_not() {
    let punt = |category| {
        let mut req = aida_core::Requirement::new("fix".into(), String::new());
        req.attention_reason = Some(aida_core::AttentionReason {
            category,
            detail: "d".into(),
            lean: None,
            raised_by: None,
            raised_at: Utc::now(),
        });
        fix_state_of(&req)
    };
    assert!(punt(aida_core::PuntCategory::DesignFork).needs_design);
    assert!(!punt(aida_core::PuntCategory::BlockedDependency).needs_design);
    let mut tagged = aida_core::Requirement::new("fix".into(), String::new());
    tagged.tags.insert("needs-design".into());
    assert!(fix_state_of(&tagged).needs_design);
}

// ---------------------------------------------------------------------------
// Closing (F7) and notices (F3, F4).
// ---------------------------------------------------------------------------

#[test]
fn a_confirmed_red_closes_as_fixed_and_resolves_the_attention_item() {
    let (dir, mut fake) = confirmed();
    fake.at(50);
    fake.push(3, "fix(x): repair main (BUG-9000) (#12)", green("300"));
    run(dir.path(), &mut fake);
    assert_eq!(outcomes(dir.path()), vec!["fixed".to_string()]);
    assert!(fake
        .mail_keys()
        .contains(&format!("main-red-{}-green", sha(2)).as_str()));
    assert_eq!(fake.human_resolves, vec![sha(2)]);
    let m = metrics(&journal(dir.path()), t0(), t0() + Duration::hours(2));
    assert_eq!(m["fix_latency_seconds_median"], 50 * 60);
    assert_eq!(m["reverted"], 0);
}

#[test]
fn a_revert_followed_by_another_green_commit_still_closes_as_reverted() {
    let (dir, mut fake) = confirmed();
    fake.push(
        3,
        "Revert \"fix(x): break main (STORY-2) (#11)\" (#13)",
        green("300"),
    );
    fake.push(4, "docs: unrelated (#14)", green("400"));
    fake.at(40);
    run(dir.path(), &mut fake);
    assert_eq!(outcomes(dir.path()), vec!["reverted".to_string()]);
}

#[test]
fn failed_notices_are_kept_and_retried_including_after_the_episode_closes() {
    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    fake.fail_operator = true;
    fake.fail_human = true;
    fake.at(31);
    run(dir.path(), &mut fake);
    assert!(fake.notices.is_empty() && fake.human_opens.is_empty());
    fake.push(3, "fix (#12)", green("300"));
    fake.at(40);
    run(dir.path(), &mut fake);
    assert!(state(dir.path()).episode.is_none(), "closed");
    assert!(
        !state(dir.path()).outbox.is_empty(),
        "notices outlive the episode"
    );
    fake.fail_operator = false;
    fake.fail_human = false;
    fake.push(4, "later (#13)", green("400"));
    run(dir.path(), &mut fake);
    assert!(state(dir.path()).outbox.is_empty());
    assert!(fake
        .notices
        .contains(&format!("main-red-{}-confirmed", sha(2))));
    assert!(fake
        .notices
        .contains(&format!("main-red-{}-green-operator", sha(2))));
    assert_eq!(fake.human_opens.len(), 1, "the newest attention text, once");
    assert_eq!(fake.human_resolves, vec![sha(2)], "opened before resolved");
}

// ---------------------------------------------------------------------------
// F8: crash safety at each boundary.
// ---------------------------------------------------------------------------

#[test]
fn a_crash_before_the_journal_append_is_replayed_once() {
    let (dir, mut fake) = red_on_green();
    fail_at(Some("journal"));
    let result = try_run(dir.path(), &mut fake);
    fail_at(None);
    assert!(result.is_err());
    assert_eq!(
        state(dir.path()).pending.len(),
        1,
        "the state was written first"
    );
    assert_eq!(count(dir.path(), "red_start"), 0);
    run(dir.path(), &mut fake);
    assert_eq!(count(dir.path(), "red_start"), 1);
    assert!(state(dir.path()).pending.is_empty());
}

#[test]
fn a_crash_after_the_journal_append_does_not_duplicate_it() {
    let (dir, mut fake) = red_on_green();
    fail_at(Some("flushed"));
    assert!(try_run(dir.path(), &mut fake).is_err());
    fail_at(None);
    assert_eq!(count(dir.path(), "red_start"), 1);
    run(dir.path(), &mut fake);
    assert_eq!(count(dir.path(), "red_start"), 1);
    let raw = std::fs::read_to_string(dir.path().join(JOURNAL)).unwrap();
    assert_eq!(raw.matches("red_start").count(), 1, "{raw}");
}

#[test]
fn a_failed_state_write_records_nothing_and_the_next_pass_opens_once() {
    let (dir, mut fake) = red_on_green();
    fail_at(Some("state"));
    assert!(try_run(dir.path(), &mut fake).is_err());
    fail_at(None);
    assert!(state(dir.path()).episode.is_none());
    assert_eq!(count(dir.path(), "red_start"), 0);
    run(dir.path(), &mut fake);
    assert_eq!(count(dir.path(), "red_start"), 1);
    assert_eq!(
        fake.mail_keys(),
        vec![format!("main-red-{}", sha(2)).as_str()]
    );
}

#[test]
fn metrics_count_a_transition_id_once() {
    let mut e = event(t0(), "red_start", &sha(2));
    e.id = "x".into();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida/land")).unwrap();
    append(dir.path(), &e).unwrap();
    append(dir.path(), &e).unwrap();
    assert_eq!(read_journal(dir.path()).unwrap().len(), 1);
}

#[test]
fn a_failed_filing_is_retried_without_confirming_twice() {
    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    fake.fail_file = true;
    fake.at(31);
    let notes = run(dir.path(), &mut fake);
    assert!(notes.iter().any(|n| n.contains("not filed")), "{notes:?}");
    fake.fail_file = false;
    fake.at(33);
    run(dir.path(), &mut fake);
    assert_eq!(fake.filed.len(), 1);
    assert_eq!(count(dir.path(), "confirmed"), 1);
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
// Metrics (F5, F14).
// ---------------------------------------------------------------------------

fn at(minutes: i64) -> DateTime<Utc> {
    t0() + Duration::minutes(minutes)
}

fn ev(kind: &str, minutes: i64, outcome: Option<&str>) -> Event {
    let mut e = event(at(minutes), kind, &sha(2));
    e.id = format!("{kind}-{minutes}");
    e.outcome = outcome.map(str::to_string);
    e
}

#[test]
fn a_window_strictly_inside_an_episode_counts_only_its_own_slice() {
    let events = vec![
        ev("red_start", 0, None),
        ev("red_merge", 10, None),
        ev("red_merge", 70, None),
        ev("red_merge", 200, None),
        ev("red_end", 300, Some("fixed")),
    ];
    let m = metrics(&events, at(60), at(120));
    assert_eq!(m["episodes"], 1);
    assert_eq!(m["red_minutes"], 60);
    assert_eq!(m["red_merges"], 1, "only the merge at 70 is inside");
    assert_eq!(
        m["resolved"], 0,
        "the end after the window is still open there"
    );
    assert_eq!(m["fix_latency_seconds_median"], serde_json::Value::Null);
}

#[test]
fn the_median_of_an_even_count_averages_the_middle_pair() {
    assert_eq!(median(&[600, 1800]), Some(1200));
    assert_eq!(median(&[1, 2, 9]), Some(2));
    assert_eq!(median(&[]), None);
}

// ---------------------------------------------------------------------------
// F2, F4: the real store paths.
// ---------------------------------------------------------------------------

#[test]
fn a_fix_spec_saved_before_a_failed_queue_write_is_queued_on_the_retry() {
    let _user = crate::test_env::pin_queue_user();
    let (_dir, _root, store_root, _) =
        crate::bug_1638_race_seam_tests::project_with(aida_core::RequirementStatus::Approved);
    let storage = crate::test_env::home_fixture_storage(&store_root);
    let fix = FixSpec {
        red_sha: sha(2),
        title: "main red at x: Build".into(),
        description: "d".into(),
        fixes: vec![crate::bug_1638_race_seam_tests::SPEC.into()],
    };
    fail_at(Some("queue_add"));
    let first = file_fix_spec(&storage, &fix);
    fail_at(None);
    assert!(first.is_err(), "the queue write failed after the save");
    let queued = |storage: &aida_core::Storage| {
        storage
            .queue_list(&crate::current_user_id(None), true)
            .unwrap()
            .len()
    };
    assert_eq!(queued(&storage), 0);
    let id = file_fix_spec(&storage, &fix).unwrap();
    assert_eq!(
        file_fix_spec(&storage, &fix).unwrap(),
        id,
        "reused, not refiled"
    );
    assert_eq!(queued(&storage), 1, "the reuse restored the queue entry");
    let store = storage.load().unwrap();
    assert_eq!(
        store
            .requirements
            .iter()
            .filter(|r| r.tags.contains(&fix_tag(&sha(2))))
            .count(),
        1
    );
}

#[test]
fn the_attention_item_is_human_only_queued_and_resolved_on_close() {
    let _user = crate::test_env::pin_queue_user();
    let (_dir, _root, store_root, _) =
        crate::bug_1638_race_seam_tests::project_with(aida_core::RequirementStatus::Approved);
    let storage = crate::test_env::home_fixture_storage(&store_root);
    let id = human_open(&storage, &sha(2), "Main is red", "first").unwrap();
    assert_eq!(
        human_open(&storage, &sha(2), "Main is red", "second").unwrap(),
        id
    );
    let item = |storage: &aida_core::Storage| {
        storage
            .load()
            .unwrap()
            .requirements
            .into_iter()
            .find(|r| r.tags.contains(&human_tag(&sha(2))))
            .unwrap()
    };
    assert!(item(&storage).human_only);
    assert_eq!(item(&storage).description, "second");
    let entries = storage
        .queue_list(&crate::current_user_id(None), true)
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].for_role.as_deref(), Some("human"));
    human_resolve(&storage, &sha(2)).unwrap();
    assert_eq!(
        item(&storage).status,
        aida_core::RequirementStatus::Completed
    );
    assert!(storage
        .queue_list(&crate::current_user_id(None), true)
        .unwrap()
        .is_empty());
}

// ---------------------------------------------------------------------------
// F12: a stalled fetch never stalls the caller.
// ---------------------------------------------------------------------------

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
        root: root.to_path_buf(),
        forge: crate::forge::forge_for(root),
        call_timeout: std::time::Duration::from_secs(1),
    };
    let started = std::time::Instant::now();
    let result = pass(
        root,
        &required(),
        std::time::Duration::from_secs(60),
        &mut live,
    );
    assert!(result.is_err(), "a stalled fetch is a failed pass");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "returned in {:?}",
        started.elapsed()
    );
}

#[test]
fn a_busy_watch_lock_is_skipped_not_awaited() {
    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    let held = try_lock(dir.path(), std::time::Duration::ZERO)
        .unwrap()
        .unwrap();
    let notes = run(dir.path(), &mut fake);
    assert!(notes.iter().any(|n| n.contains("skipped")), "{notes:?}");
    let started = std::time::Instant::now();
    assert!(note_smoke_failure(dir.path(), &sha(2)).is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    drop(held);
}

// ---------------------------------------------------------------------------
// Supporting pieces.
// ---------------------------------------------------------------------------

/// Merging continues while main is red, and nothing that lands can stall
/// on the watch: no merge decision reads it and no landing path calls it.
/// It runs only as its own scheduled job and by hand (round-5 ruling).
#[test]
fn no_merge_or_landing_path_reads_or_calls_the_red_main_watch() {
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
            "land_run.rs",
            "auto_complete.rs",
            "burndown_run.rs",
            "upgrade_release.rs",
        ]
        .iter()
        .map(|f| src.join(f)),
    );
    for entry in std::fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if name.starts_with("advisor_loop") && name.ends_with(".rs") {
            stack.push(path);
        }
    }
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
}

/// F11: the fix window is set through the config editor as a validated
/// duration, written where the landing policy reads it.
#[cfg(feature = "tui")]
#[test]
fn the_fix_window_is_settable_as_a_validated_duration() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    let config = dir.path().join(".aida/config.toml");
    std::fs::write(&config, "[landing]\nmode = \"ci-gated\"\n").unwrap();
    let item = aida_tui::ConfigMenuItem {
        section: "landing".into(),
        name: "red_main.fix_window".into(),
        value: "120m".into(),
        default: "2h".into(),
        scope: ".aida/config.toml".into(),
        explanation: String::new(),
        edit: crate::config_cmd::config_knob_edit_kind("landing", "red_main.fix_window"),
        read_only_reason: None,
    };
    assert_eq!(item.edit, aida_tui::EditKind::Duration);
    let set = |value| crate::config_cmd::cli_edit_config_knob(dir.path(), &item, Some(value));
    assert!(matches!(set("soon"), aida_tui::EditOutcome::Blocked(_)));
    assert!(matches!(set("30s"), aida_tui::EditOutcome::Blocked(_)));
    assert!(matches!(set("90m"), aida_tui::EditOutcome::Updated { .. }));
    let body = std::fs::read_to_string(&config).unwrap();
    assert_eq!(
        crate::landing_policy::from_body(Some(&body)).red_main_fix_window,
        std::time::Duration::from_secs(90 * 60),
        "{body}"
    );
    assert!(
        body.contains("mode = \"ci-gated\""),
        "the rest is kept: {body}"
    );
}

#[test]
fn the_excerpt_keeps_the_error_lines() {
    let log = "Run tests\nok 1\nerror[E0425]: cannot find value\ntest a ... FAILED\nthread 'x' panicked at y\nfine\n";
    let text = excerpt(log, Some("Run tests"));
    assert!(text.contains("error[E0425]"));
    assert!(text.contains("FAILED"));
    assert!(text.contains("panicked"));
    assert!(!text.contains("fine"));
    assert_eq!(
        excerpt("all good\n", None),
        "(no error lines in the job log)"
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

// ---------------------------------------------------------------------------
// Round 4.
// ---------------------------------------------------------------------------

/// P1: a held store lock fails a store effect at once; it succeeds once the
/// lock is free (the watch retries it on its next pass).
#[test]
fn a_held_store_lock_fails_store_effects_at_once_and_they_succeed_later() {
    let _user = crate::test_env::pin_queue_user();
    let (_dir, _root, store_root, _) =
        crate::bug_1638_race_seam_tests::project_with(aida_core::RequirementStatus::Approved);
    let storage = crate::test_env::home_fixture_storage(&store_root);
    let fix = FixSpec {
        red_sha: sha(2),
        title: "main red at x: Build".into(),
        description: "d".into(),
        fixes: Vec::new(),
    };
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let holder_root = store_root.clone();
    let holder = std::thread::spawn(move || {
        aida_core::db::with_store_write_lock(&holder_root, || {
            held_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(())
        })
        .unwrap();
    });
    held_rx.recv().unwrap();
    let started = std::time::Instant::now();
    assert!(file_fix_spec(&storage, &fix).is_err());
    assert!(human_open(&storage, &sha(2), "t", "b").is_err());
    assert!(human_resolve(&storage, &sha(2)).is_err());
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "never waits: {:?}",
        started.elapsed()
    );
    release_tx.send(()).unwrap();
    holder.join().unwrap();
    file_fix_spec(&storage, &fix).unwrap();
    human_open(&storage, &sha(2), "t", "b").unwrap();
}

/// P2: a re-run that started between the check-run read and the run read
/// is pending, not a confirmed failure.
#[test]
fn a_rerun_in_progress_is_pending_not_a_confirmed_failure() {
    use serde_json::json;
    let head = sha(1);
    let runs = json!({"total_count": 1, "check_runs": [
        {"name": BUILD, "head_sha": head, "status": "completed", "conclusion": "failure",
         "details_url": "https://github.com/o/r/actions/runs/7/job/1"}]});
    let mut api = |endpoint: &str| -> Result<Vec<u8>> {
        let body = if endpoint.contains("/check-runs") {
            runs.clone()
        } else if endpoint.ends_with("/actions/runs/7") {
            json!({"id": 7, "head_sha": head, "workflow_id": 3, "run_attempt": 2, "status": "in_progress"})
        } else {
            json!({"sha": head, "total_count": 0, "statuses": []})
        };
        Ok(serde_json::to_vec(&body).unwrap())
    };
    let checks = crate::forge::github_commit_checks("o/r", &head, &mut api).unwrap();
    assert!(checks.checks[0].run_in_progress);
    assert!(matches!(
        judge(Ok(checks), &required()),
        Verdict::Unknown(_)
    ));

    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    let mut rerun = red("200", 2);
    rerun[0].run_in_progress = true;
    fake.checks.insert(sha(2), rerun);
    fake.at(10);
    run(dir.path(), &mut fake);
    assert!(state(dir.path()).episode.unwrap().confirmed_at.is_none());
    assert!(fake.filed.is_empty());
}

/// P2: evidence is fresh per (run, attempt) of each failed job, not per the
/// newest attempt overall.
#[test]
fn a_rerun_failing_again_has_its_new_attempt_probed() {
    let dir = tempfile::tempdir().unwrap();
    let mut fake = Fake::default();
    fake.push(1, "base (#1)", green("1"));
    run(dir.path(), &mut fake);
    fake.push(
        2,
        "two jobs (#2)",
        vec![
            check(BUILD, CheckOutcome::Failed, "10", 3),
            check("CI (lib-1)", CheckOutcome::Failed, "20", 1),
        ],
    );
    run(dir.path(), &mut fake);
    fake.evidence.insert(
        "20".into(),
        Evidence {
            check: "CI (lib-1)".into(),
            link: "l".into(),
            failed_steps: vec!["Build all packages".into()],
            excerpt: "error".into(),
            log_complete: true,
        },
    );
    fake.checks.insert(
        sha(2),
        vec![
            check(BUILD, CheckOutcome::Failed, "10", 3),
            check("CI (lib-1)", CheckOutcome::Failed, "20", 2),
        ],
    );
    fake.at(5);
    run(dir.path(), &mut fake);
    let episode = state(dir.path()).episode.unwrap();
    assert!(
        episode.evidence_keys.contains("20:2:CI (lib-1)"),
        "{:?}",
        episode.evidence_keys
    );
    assert!(
        episode.unbuildable.is_some(),
        "B's new failing step was read"
    );
}

/// P2: an unread log keeps the link and steps, is retried, and reaches the
/// alert and the filed fix spec as follow-ups once it is read.
#[test]
fn an_unread_log_is_retried_and_followed_up_on_the_alert_and_fix_spec() {
    let (dir, mut fake) = red_on_green();
    fake.log_failures = 3;
    run(dir.path(), &mut fake);
    let episode = state(dir.path()).episode.unwrap();
    assert!(episode.evidence_keys.is_empty());
    assert_eq!(episode.evidence[0].link, "https://ci.example/runs/200");
    fake.at(31);
    run(dir.path(), &mut fake);
    assert_eq!(fake.filed.len(), 1);
    fake.at(33);
    run(dir.path(), &mut fake);
    assert!(fake.comments.is_empty(), "the log is still unread");
    fake.at(35);
    run(dir.path(), &mut fake);
    let key = format!("main-red-{}-evidence-{}", sha(2), "Build--ubuntu-latest-");
    assert!(
        fake.mail_keys().contains(&key.as_str()),
        "{:?}",
        fake.mail_keys()
    );
    assert_eq!(fake.comments.len(), 1);
    assert_eq!(fake.comments[0].0, "BUG-9000");
    assert!(fake.comments[0].1.contains("test foo ... FAILED"));
}

/// P2: a torn last journal line does not stop the replay.
#[test]
fn a_torn_last_journal_line_is_skipped_and_the_replay_proceeds() {
    let (dir, mut fake) = red_on_green();
    fail_at(Some("journal"));
    assert!(try_run(dir.path(), &mut fake).is_err());
    fail_at(None);
    let journal_path = dir.path().join(JOURNAL);
    std::fs::write(&journal_path, b"{\"id\":\"torn").unwrap();
    assert!(read_journal(dir.path()).unwrap().is_empty());
    run(dir.path(), &mut fake);
    assert_eq!(count(dir.path(), "red_start"), 1);
    let raw = std::fs::read_to_string(&journal_path).unwrap();
    assert!(!raw.contains("torn"), "the torn line was cut: {raw}");
    assert_eq!(
        fake.mail_keys(),
        vec![format!("main-red-{}", sha(2)).as_str()]
    );
}

/// Round 5: sibling failed jobs of one run attempt are read independently.
#[test]
fn two_failing_jobs_in_one_run_both_get_their_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let mut fake = Fake::default();
    fake.push(1, "base (#1)", green("1"));
    run(dir.path(), &mut fake);
    fake.push(
        2,
        "two jobs fail (#2)",
        vec![
            check(BUILD, CheckOutcome::Failed, "77", 1),
            check("CI (lib-0)", CheckOutcome::Failed, "77", 1),
        ],
    );
    run(dir.path(), &mut fake);
    let episode = state(dir.path()).episode.unwrap();
    assert_eq!(episode.evidence.len(), 2, "{:?}", episode.evidence);
    for job in [BUILD, "CI (lib-0)"] {
        let found = episode.evidence.iter().find(|e| e.check == job).unwrap();
        assert!(found.link.contains("77") && found.log_complete);
        assert!(episode.evidence_keys.contains(&format!("77:1:{job}")));
    }
    let body = fake.mail(&format!("main-red-{}", sha(2)));
    assert_eq!(body.matches("test foo ... FAILED").count(), 2, "{body}");
}

/// Final review F2: the status line never says green while an episode is
/// open, however quiet the pass was.
#[test]
fn a_steady_confirmed_red_never_reports_green() {
    let (dir, mut fake) = confirmed();
    fake.at(40);
    let notes = run(dir.path(), &mut fake);
    assert!(notes.iter().all(|n| !n.contains("green")), "{notes:?}");
    let line = status_line(dir.path()).unwrap();
    assert!(!line.contains("green"), "{line}");
    assert!(
        line.contains("red") && line.contains("confirmed; fix BUG-9000"),
        "{line}"
    );
    let (dir, mut fake) = red_on_green();
    run(dir.path(), &mut fake);
    assert!(status_line(dir.path())
        .unwrap()
        .contains("waiting for its re-run"));
    let dir = tempfile::tempdir().unwrap();
    let mut fake = Fake::default();
    fake.push(1, "a (#1)", green("1"));
    run(dir.path(), &mut fake);
    assert!(status_line(dir.path()).unwrap().contains("main is green"));
}

/// Final review F3: durations display without loss.
#[test]
fn durations_display_without_loss() {
    use crate::landing_policy::duration_text;
    let d = std::time::Duration::from_secs;
    assert_eq!(duration_text(d(90)), "1m30s");
    assert_eq!(duration_text(d(7200)), "2h");
    assert_eq!(duration_text(d(5400)), "1h30m");
    assert_eq!(duration_text(d(45)), "45s");
    assert_eq!(duration_text(d(3661)), "1h1m1s");
    for secs in [60, 90, 5400, 3661, 7 * 24 * 3600] {
        assert_eq!(
            crate::presence::parse_duration_secs(&duration_text(d(secs))),
            Some(secs),
            "round-trips"
        );
    }
}
