//! Fixed train pass; authority is adopted only between journaled trains.
// trace:STORY-2691 | ai:codex
use super::*;

/// [`eligible_in`] with the supervised lane closed.
#[cfg(test)]
pub(super) fn eligible(
    change: &forge::Change,
    evidence: &dyn ready::Evidence,
) -> std::result::Result<(), crate::pr_train::MemberGateError> {
    eligible_in(change, evidence, false)
}

/// The automatic train's member predicate. With the supervised lane open (`[landing]
/// auto_train_supervised_under_mandate` on and an active mandate lifting
/// supervision), a held PR passes on to the member gate, whose mandate cover
/// decides; its approval must then be at the exact head.
// trace:STORY-3181 | ai:claude
pub(super) fn eligible_in(
    change: &forge::Change,
    evidence: &dyn ready::Evidence,
    supervised_lane: bool,
) -> std::result::Result<(), crate::pr_train::MemberGateError> {
    use crate::pr_train::MemberGateError::{Refused, Unavailable};
    macro_rules! ensure {
        ($condition:expr, $message:expr $(,)?) => {
            if !$condition {
                return Err(Refused($message.into()));
            }
        };
    }
    ensure!(!change.gitlab, "automatic trains require GitHub");
    ensure!(
        change
            .head_branch
            .as_deref()
            .is_some_and(|b| !b.starts_with(crate::pr_train::BRANCH_PREFIX)),
        "train branch or unknown source branch"
    );
    ensure!(
        evidence.under_review(change).is_none(),
        "review still running"
    );
    let held = evidence
        .hold(change)
        .map_err(|e| Unavailable(format!("{e:#}")))?
        .is_some();
    ensure!(!held || supervised_lane, "merge hold present");
    // The authoritative gate proves carry-over under protected fast policy;
    // strict policy and invalid proofs refuse there. CI below still binds head.
    // trace:STORY-2763 | ai:codex
    let approval = evidence.train_approval(change)?;
    ensure!(
        approval.reviewed == change.head || (!held && approval.carried.is_some()),
        "approval must cover the head directly or by authoritative carry-over"
    );
    let mut checks = evidence
        .ci(&change.head)
        .map_err(|e| Unavailable(format!("{e:#}")))?;
    ensure!(
        checks.sha.eq_ignore_ascii_case(&change.head),
        "CI is for another head"
    );
    let required = evidence
        .required_checks()
        .map_err(|e| Unavailable(format!("{e:#}")))?;
    // The ordinary train gate also resolves forge-required policy. With no
    // configured list, preserve the conservative all-checks readiness rule.
    // Informational checks cannot delay an explicitly configured required set.
    if !required.is_empty() {
        checks.checks.retain(|c| required.contains(&c.name));
    }
    let names: Vec<&str> = required.iter().map(String::as_str).collect();
    ensure!(
        matches!(
            crate::forge::judge_commit_checks(&checks, &names),
            crate::forge::CommitCi::Green
        ),
        "required CI is not green at the head"
    );
    ensure!(
        evidence
            .head(change)
            .map_err(|e| Unavailable(format!("{e:#}")))?
            == change.head,
        "head moved during evaluation"
    );

    Ok(())
}

/// Hold the auto-train's admission only for a pass that will run a train
/// (STORY-2905 round 2): while it stands down, or cannot read the policy,
/// the admission is released so the land loop, a manual ship or a manual
/// train can take it. A pass that would run takes it (once) first; when
/// another holder has it, the error says so and the pass does not run. A
/// running train is never cut short: this is called between passes only.
/// A waiting land, ship or manual train, or a drain, also releases it and
/// the pass does not run (BUG-2703): the error names who it yielded to.
// trace:STORY-2905 | ai:claude
// trace:BUG-2703 | ai:claude
pub(super) fn hold_admission(
    root: &Path,
    stand_down: &std::result::Result<Option<&'static str>, String>,
    held: &mut Option<crate::admission_owner::Claim>,
) -> Result<()> {
    if !matches!(stand_down, Ok(None)) {
        *held = None;
        return Ok(());
    }
    if let Some(reason) = crate::admission_owner::yield_reason(root, ADMISSION_PURPOSE) {
        *held = None;
        return Err(Yielded(reason).into());
    }
    if held.is_none() {
        *held = Some(crate::admission_owner::acquire(
            root,
            ADMISSION_PURPOSE,
            true,
        )?);
    }
    Ok(())
}

const ADMISSION_PURPOSE: &str = "legacy auto-train";

/// The auto-train gave its admission up to a waiting land, ship or manual
/// train, or to a drain, named by the string.
// trace:BUG-2703 | ai:claude
#[derive(Debug)]
pub(super) struct Yielded(pub(super) String);
impl std::fmt::Display for Yielded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "yielded admission to {}", self.0)
    }
}
impl std::error::Error for Yielded {}

/// The service-log line for a pass that did not run because admission is
/// not held. A yield says who it yielded to; anything else (another holder
/// has the claim) keeps its own wording. Neither is a landing-policy fault.
// trace:BUG-2703 | ai:claude
pub(super) fn not_held_line(error: &anyhow::Error) -> String {
    match error.downcast_ref::<Yielded>() {
        Some(yielded) => format!("not running: {yielded}"),
        None => format!("not running: admission is not held ({error:#})"),
    }
}

pub(super) fn run(root: &Path) -> Result<()> {
    let mut refused = Refusals::default();
    run_passes(
        root,
        180,
        || train_wanted(root, &mut refused),
        crate::pr_train::advisor_train_handler,
    )
}

/// Whether an automatic pass has a train to assemble: a journaled train to
/// recover, or an open change the pass's own member predicate admits
/// (judged read-only). Read without admission, so a poll with nothing to
/// train never holds it. Evidence that cannot be read counts as wanted: the
/// full pass then judges (and reports) that change exactly as before.
// trace:BUG-2769 trace:TASK-3917 | ai:claude
fn train_wanted(root: &Path, refused: &mut Refusals) -> Result<bool> {
    use crate::pr_train::{AutomaticPrescan, MemberGateError::Refused};
    let main = crate::main_worktree_root_from(root);
    if crate::pr_train::automatic_recovery_needed(&main)? {
        return Ok(true);
    }
    let repo = forge::Repository::load(&main)?;
    let changes: Vec<forge::Change> = repo
        .discover(&main)?
        .into_iter()
        .filter(|c| {
            !c.head_branch
                .as_deref()
                .is_some_and(|b| b.starts_with(crate::pr_train::BRANCH_PREFIX))
        })
        .collect();
    // Loaded only when a change must be judged: it lists the live trains.
    let mut prescan: Option<Option<AutomaticPrescan>> = None;
    wanted_among(&changes, refused, REJUDGE, |change| {
        if prescan.is_none() {
            prescan = Some(AutomaticPrescan::load(root)?);
        }
        Ok(match prescan.as_ref().and_then(Option::as_ref) {
            None => Err(Refused("automatic trains require GitHub".into())),
            Some(p) if p.abandoned => Ok(()),
            Some(p) if p.base() != change.base => Err(Refused("another base".into())),
            Some(p) => p.admits(change.id),
        })
    })
}

/// A refused change is judged again after this, or when its head moves.
const REJUDGE: std::time::Duration = std::time::Duration::from_secs(600);

/// The worker's pre-scan refusals: change id to the head refused and when.
/// An idle poll whose open changes are all refused at their heads spends one
/// discovery listing on the shared forge budget, not a member gate per change.
/// Unreadable evidence is never kept. trace:TASK-3917 | ai:claude
#[derive(Default)]
pub(super) struct Refusals(std::collections::HashMap<u64, (String, std::time::Instant)>);

/// Whether `judge` admits any change (or cannot read its evidence), asking
/// only about changes with no refusal at their head within `window`.
// trace:TASK-3917 | ai:claude
fn wanted_among(
    changes: &[forge::Change],
    refused: &mut Refusals,
    window: std::time::Duration,
    mut judge: impl FnMut(
        &forge::Change,
    ) -> Result<std::result::Result<(), crate::pr_train::MemberGateError>>,
) -> Result<bool> {
    let now = std::time::Instant::now();
    refused.0.retain(|id, (head, at)| {
        now.duration_since(*at) < window && changes.iter().any(|c| c.id == *id && c.head == *head)
    });
    for change in changes {
        if refused.0.contains_key(&change.id) {
            continue;
        }
        match judge(change)? {
            Err(crate::pr_train::MemberGateError::Refused(_)) => {
                refused.0.insert(change.id, (change.head.clone(), now));
            }
            Ok(()) | Err(crate::pr_train::MemberGateError::Unavailable(_)) => return Ok(true),
        }
    }
    Ok(false)
}

/// A claim held longer than this is reported, with its phase, every
/// `HOLD_WARN` while the hold lasts. trace:BUG-2769 | ai:claude
const HOLD_WARN: std::time::Duration = std::time::Duration::from_secs(30);
#[cfg(test)]
static TEST_HOLD_WARN_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
fn hold_warn() -> std::time::Duration {
    #[cfg(test)]
    match TEST_HOLD_WARN_MS.load(std::sync::atomic::Ordering::SeqCst) {
        0 => {}
        ms => return std::time::Duration::from_millis(ms),
    }
    HOLD_WARN
}

/// The live long-hold line `train-status` shows; a pass's watchdog writes it.
fn hold_file(root: &Path) -> std::path::PathBuf {
    crate::main_worktree_root_from(root).join(".aida/auto-train-hold")
}

/// The live long-hold line, while an auto-train claim it names is held.
// trace:BUG-2769 | ai:claude
pub(super) fn live_hold_line(root: &Path) -> Option<String> {
    let line = std::fs::read_to_string(hold_file(root)).ok()?;
    let holders = crate::admission_owner::holders(root).ok()?;
    holders
        .iter()
        .any(|h| {
            h.state == "claim"
                && h.purpose == ADMISSION_PURPOSE
                && line.contains(&format!("holder pid {},", h.pid))
        })
        .then_some(line)
}

/// While a pass holds admission, report the hold every `hold_warn()` once it
/// is long, with its current phase, until `stop` is dropped. A recovery
/// pass says so; otherwise the train journal's run lock separates a running
/// train from assembly. trace:BUG-2769 | ai:claude
fn watch_hold(
    root: &Path,
    since: std::time::Instant,
    recovering: bool,
    stop: std::sync::mpsc::Receiver<()>,
) {
    let (file, main) = (hold_file(root), crate::main_worktree_root_from(root));
    let _ = std::fs::remove_file(&file);
    while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = stop.recv_timeout(hold_warn()) {
        let phase = match (recovering, crate::pr_train::admission_train_running(&main)) {
            (true, _) => "recovery",
            (false, Ok(true)) => "running train",
            (false, _) => "assembly",
        };
        if let Some(line) = long_hold_line(since.elapsed(), phase) {
            eprintln!("{line}");
            let _ = std::fs::create_dir_all(main.join(".aida"));
            let _ = std::fs::write(&file, &line);
        }
    }
    let _ = std::fs::remove_file(&file);
}

/// The report line for a hold of `held` in `phase`, when it is long.
// trace:BUG-2769 | ai:claude
fn long_hold_line(held: std::time::Duration, phase: &str) -> Option<String> {
    (held > hold_warn()).then(|| {
        format!(
            "auto-train: held admission {}s (holder pid {}, phase {phase})",
            held.as_secs(),
            std::process::id()
        )
    })
}

/// The worker loop, with its pre-scan, its pass (the train) and idle length
/// as parameters so a regression can run this exact loop around a
/// controlled train.
// trace:BUG-2703 | ai:claude
// The claim is taken only for a pass the unclaimed pre-scan wants, and given
// up when that pass returns: idle polls hold nothing, so a waiting manual
// train, land or ship is admitted at once. trace:BUG-2769 | ai:claude
pub(super) fn run_passes(
    root: &Path,
    idle_seconds: u64,
    mut wanted: impl FnMut() -> Result<bool>,
    mut pass: impl FnMut() -> Result<()>,
) -> Result<()> {
    let mut admission: Option<crate::admission_owner::Claim> = None;
    loop {
        train_service::check_controller(root)?;
        // trace:STORY-2856 | ai:codex
        if let Err(error) = crate::ci_startup_monitor::check(root) {
            eprintln!("auto-train: startup monitor unavailable: {error:#}");
        }
        let stand_down = crate::gate::landing_merge::train_stand_down(root);
        // Nothing to assemble: no claim, no pass. A stand-down still reaches
        // auto_train_pass below, which reports it. trace:BUG-2769 | ai:claude
        if matches!(stand_down, Ok(None)) {
            match wanted() {
                Ok(true) => {}
                Ok(false) => {
                    train_service::record_failure(root, None)?;
                    idle(root, idle_seconds, &mut admission)?;
                    continue;
                }
                Err(error) => {
                    record_pass_failure(root, error)?;
                    idle(root, idle_seconds, &mut admission)?;
                    continue;
                }
            }
        }
        // A pass without admission does not run, and says why in its own
        // words: it is not a landing-policy fault. trace:BUG-2703 | ai:claude
        let held_since = std::time::Instant::now();
        if let Err(error) = hold_admission(root, &stand_down, &mut admission) {
            eprintln!("auto-train: {}", not_held_line(&error));
            train_service::record_failure(root, None)?;
            idle(root, idle_seconds, &mut admission)?;
            continue;
        }
        // In review-gated mode the land loop is the one merge hand, and a
        // policy that cannot be read runs nothing: the pass decision is
        // landing_merge::auto_train_pass. trace:STORY-2905 | ai:claude
        // The hold is reported while it lasts, not only after. trace:BUG-2769 | ai:claude
        let recovering = admission.is_some()
            && crate::pr_train::automatic_recovery_needed(&crate::main_worktree_root_from(root))
                .unwrap_or(false);
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let outcome = std::thread::scope(|scope| {
            if admission.is_some() {
                scope.spawn(move || watch_hold(root, held_since, recovering, stopped));
            }
            let outcome = crate::gate::landing_merge::auto_train_pass(stand_down, &mut pass);
            drop(stop);
            outcome
        });
        match outcome {
            Err(line) => {
                eprintln!("auto-train: {line}");
                train_service::record_failure(root, None)?;
            }
            // Never kill a running train on rotation. It owns its journal and
            // merge leases until it returns; the next pass observes the new binding.
            Ok(Ok(())) => train_service::record_failure(root, None)?,
            Ok(Err(error))
                if error
                    .downcast_ref::<crate::pr_train::TrainJournalBusy>()
                    .is_some()
                    || error
                        .downcast_ref::<crate::admission_owner::AdmissionBusy>()
                        .is_some() =>
            {
                eprintln!("auto-train: existing train owns the journal; leaving it alone");
            }
            Ok(Err(error)) => record_pass_failure(root, error)?,
        }
        admission = None; // trace:BUG-2769 | ai:claude
        idle(root, idle_seconds, &mut admission)?;
    }
}

pub(super) fn record_pass_failure(root: &Path, error: anyhow::Error) -> Result<()> {
    let reason = format!("{error:#}");
    eprintln!("auto-train: pass failed: {reason}");
    train_service::record_failure(root, Some(&reason))
}

/// Check heartbeat and grant once a second while idle, so rotation does not
/// need to wait for the next three-minute discovery interval. Between passes
/// only, a waiting land/ship/manual train or a drain gets the claim back
/// within a second.
// trace:BUG-2703 | ai:claude
pub(super) fn idle(
    root: &Path,
    seconds: u64,
    admission: &mut Option<crate::admission_owner::Claim>,
) -> Result<()> {
    for _ in 0..seconds {
        std::thread::sleep(std::time::Duration::from_secs(1));
        train_service::check_controller(root)?;
        if admission.is_some() {
            if let Some(reason) = crate::admission_owner::yield_reason(root, ADMISSION_PURPOSE) {
                *admission = None;
                eprintln!("auto-train: admission yielded: {reason}");
            }
        }
    }
    Ok(())
}

// trace:STORY-2905 | ai:claude
#[cfg(test)]
mod story2905_admission_tests {
    use super::*;

    /// Round 2 finding 1: while the auto-train stands down (or cannot read
    /// the policy) its admission is released, so the land loop's exclusive
    /// admission succeeds; a pass that runs takes it again, and cannot while
    /// the land loop holds it.
    #[test]
    fn standing_down_releases_the_auto_train_admission() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let mut held = None;
        hold_admission(root, &Ok(None), &mut held).unwrap();
        assert!(held.is_some(), "a running pass holds admission");
        assert!(
            crate::admission_owner::acquire(root, "land run", false).is_err(),
            "the land loop waits while a train may run"
        );
        hold_admission(
            root,
            &Ok(Some(crate::gate::landing_merge::TRAIN_STAND_DOWN)),
            &mut held,
        )
        .unwrap();
        assert!(held.is_none(), "standing down releases admission");
        let land = crate::admission_owner::acquire(root, "land run", false)
            .expect("the land loop takes admission while the auto-train stands down");
        assert!(
            hold_admission(root, &Ok(None), &mut held).is_err(),
            "a pass cannot take admission the land loop holds"
        );
        drop(land);
        hold_admission(root, &Ok(None), &mut held).unwrap();
        hold_admission(root, &Err("unreadable".into()), &mut held).unwrap();
        assert!(held.is_none(), "an unknown policy releases admission too");
    }

    /// Promotion 23 review F5: a pass whose policy refresh failed (the
    /// stand-down check returns the fetch error) releases the admission it
    /// held, so the land loop is not blocked by a train that cannot run.
    // trace:STORY-2905 | ai:claude
    #[test]
    fn a_failed_policy_refresh_releases_the_admission() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        assert!(crate::test_git::git_command()
            .current_dir(root)
            .args(["init", "-q", "-b", "main"])
            .status()
            .unwrap()
            .success());
        crate::test_git::git_command()
            .current_dir(root)
            .args(["remote", "add", "origin", "/nonexistent/origin.git"])
            .status()
            .unwrap();
        let mut held = None;
        hold_admission(root, &Ok(None), &mut held).unwrap();
        assert!(held.is_some());
        let stand_down = crate::gate::landing_merge::train_stand_down(root);
        assert!(stand_down.is_err(), "{stand_down:?}");
        hold_admission(root, &stand_down, &mut held).unwrap();
        assert!(held.is_none(), "a failed refresh releases admission");
        drop(
            crate::admission_owner::acquire(root, "land run", false)
                .expect("the land loop takes admission"),
        );
    }

    /// Review round 3, finding 7: a pass that yielded says who it yielded
    /// to, and no admission refusal is worded as an unknown landing policy.
    // trace:BUG-2703 | ai:claude
    // trace:BUG-b6b1278890774ef993d2ebc327862470-465.ac6ca3ea | ai:claude
    #[test]
    fn bug2703_a_yielded_pass_is_logged_as_a_yield() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut held = None;
        hold_admission(root, &Ok(None), &mut held).unwrap();
        crate::admission_drain::fixture_drain(
            root,
            "joe (uid 1)",
            "land #4324",
            600,
            chrono::Utc::now(),
        )
        .unwrap();
        let error = hold_admission(root, &Ok(None), &mut held).unwrap_err();
        assert!(held.is_none(), "the claim is given up");
        let line = not_held_line(&error);
        assert!(
            line.starts_with(
                "not running: yielded admission to advisor loops drained by joe (uid 1)"
            ),
            "{line}"
        );
        assert!(!line.contains("landing policy"), "{line}");
        // Another holder is not a yield, and not a policy fault either.
        let temp = tempfile::tempdir().unwrap();
        let _land = crate::admission_owner::acquire(temp.path(), "land run", false).unwrap();
        let error = hold_admission(temp.path(), &Ok(None), &mut held).unwrap_err();
        let line = not_held_line(&error);
        assert!(
            line.starts_with("not running: admission is not held (admission busy: "),
            "{line}"
        );
        assert!(line.contains("purpose land run"), "{line}");
        assert!(!line.contains("landing policy"), "{line}");
    }
}

// trace:BUG-2769 | ai:claude
#[cfg(test)]
mod bug2769_idle_admission_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn claims(root: &Path) -> Vec<String> {
        crate::admission_owner::holders(root)
            .unwrap()
            .into_iter()
            .filter(|h| h.state == "claim")
            .map(|h| h.purpose)
            .collect()
    }

    fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !ready() {
            assert!(Instant::now() < deadline, "timed out: {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A local `main` with no config: the ci-gated default, so passes run.
    fn ci_gated_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &[
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@test",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "initial",
            ],
        ] {
            let status = crate::test_git::git_command()
                .current_dir(&root)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        }
        assert_eq!(
            crate::gate::landing_merge::train_stand_down(&root),
            Ok(None),
            "the fixture policy lets passes run"
        );
        (temp, root)
    }

    /// AC1-AC3 through the real worker loop: while the unclaimed pre-scan
    /// finds nothing to train, no admission is held across polls and a
    /// contending manual train is admitted at once (well inside one poll).
    /// A wanted pass takes the claim and gives it up when it returns.
    #[test]
    fn bug2769_idle_auto_train_holds_no_admission_and_admits_a_manual_train() {
        let (_temp, root) = ci_gated_repo();
        let polls = Arc::new(AtomicUsize::new(0));
        let want = Arc::new(AtomicBool::new(false));
        let held_in_pass = Arc::new(AtomicBool::new(false));
        let worker = {
            let (root, polls, want, held) = (
                root.clone(),
                polls.clone(),
                want.clone(),
                held_in_pass.clone(),
            );
            std::thread::spawn(move || {
                let pass_root = root.clone();
                let pass_want = want.clone();
                run_passes(
                    &root,
                    1,
                    move || {
                        polls.fetch_add(1, Ordering::SeqCst);
                        Ok(want.load(Ordering::SeqCst))
                    },
                    move || {
                        let mine = claims(&pass_root).iter().any(|p| p == ADMISSION_PURPOSE);
                        held.store(mine, Ordering::SeqCst);
                        pass_want.store(false, Ordering::SeqCst);
                        Ok(())
                    },
                )
            })
        };
        wait_for("three idle polls with no claim held", || {
            assert!(claims(&root).is_empty(), "idle auto-train holds admission");
            polls.load(Ordering::SeqCst) >= 3
        });
        let started = Instant::now();
        let manual = crate::admission_owner::manual_train_admission(&root, false, &[])
            .unwrap()
            .expect("the manual train takes its own claim");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        drop(manual);

        want.store(true, Ordering::SeqCst);
        wait_for("a wanted pass ran under the claim", || {
            held_in_pass.load(Ordering::SeqCst)
        });
        let after = polls.load(Ordering::SeqCst);
        wait_for("the next poll", || polls.load(Ordering::SeqCst) > after);
        assert!(
            claims(&root).is_empty(),
            "the claim was released after the pass"
        );

        crate::review_watch::stop_test_loops(&root);
        let error = worker.join().unwrap().expect_err("stopped by the fixture");
        assert!(format!("{error:#}").contains("test fixture stopped the loop"));
    }

    /// AC4 through the real worker loop: a pass that outlives the threshold
    /// is reported, holder and phase, while it still holds the claim, and
    /// `train-status`'s hold line shows it; both go when the pass returns.
    #[test]
    fn bug2769_a_long_hold_is_reported_while_it_is_held() {
        let (_temp, root) = ci_gated_repo();
        let line = line_while_held(&root, || ());
        assert!(line.contains("phase assembly"), "{line}");
    }

    /// Serializes the tests that shorten the hold threshold and restores it
    /// when one ends, pass or fail. trace:TASK-3917 | ai:claude
    struct ShortHoldWarn {
        _lock: std::sync::MutexGuard<'static, ()>,
    }
    impl Drop for ShortHoldWarn {
        fn drop(&mut self) {
            TEST_HOLD_WARN_MS.store(0, Ordering::SeqCst);
        }
    }
    fn short_hold_warn() -> ShortHoldWarn {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let lock = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        TEST_HOLD_WARN_MS.store(100, Ordering::SeqCst);
        ShortHoldWarn { _lock: lock }
    }

    /// The live hold line of a pass that keeps the claim until the line
    /// shows; `during` runs as the pass starts and its value lives with it.
    fn line_while_held<T>(root: &Path, mut during: impl FnMut() -> T + Send + 'static) -> String {
        let _short = short_hold_warn();
        let release = Arc::new(AtomicBool::new(false));
        let worker = {
            let (root, release) = (root.to_path_buf(), release.clone());
            std::thread::spawn(move || {
                run_passes(
                    &root,
                    1,
                    || Ok(true),
                    move || {
                        let _kept = during();
                        while !release.load(Ordering::SeqCst) {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        Ok(())
                    },
                )
            })
        };
        let mut line = None;
        wait_for("the live hold line", || {
            line = live_hold_line(root);
            line.is_some()
        });
        let line = line.unwrap();
        assert!(
            claims(root).iter().any(|p| p == ADMISSION_PURPOSE),
            "still held"
        );
        assert!(
            line.contains(&format!("holder pid {},", std::process::id())),
            "{line}"
        );
        release.store(true, Ordering::SeqCst);
        wait_for("the hold ends", || live_hold_line(root).is_none());
        crate::review_watch::stop_test_loops(root);
        assert!(worker.join().unwrap().is_err(), "stopped by the fixture");
        line
    }

    /// AC4's running-train phase: the train journal's run lock is held.
    // trace:TASK-3917 | ai:claude
    #[test]
    fn task3917_a_long_hold_names_the_running_train_phase() {
        let (_temp, root) = ci_gated_repo();
        let lock = root.join(".aida/train/run.lock");
        let line = line_while_held(&root, move || {
            std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
            let file = std::fs::File::create(&lock).unwrap();
            fs2::FileExt::lock_exclusive(&file).unwrap();
            file
        });
        assert!(line.contains("phase running train"), "{line}");
    }

    /// AC4's recovery phase: a journaled train awaits recovery at the claim.
    // trace:TASK-3917 | ai:claude
    #[test]
    fn task3917_a_long_hold_names_the_recovery_phase() {
        let (_temp, root) = ci_gated_repo();
        std::fs::create_dir_all(root.join(".aida/train")).unwrap();
        std::fs::write(root.join(".aida/train/retained.json"), "{}").unwrap();
        let line = line_while_held(&root, || ());
        assert!(line.contains("phase recovery"), "{line}");
    }

    fn change(id: u64, head: &str) -> forge::Change {
        forge::Change {
            id,
            head: head.into(),
            base: "main".into(),
            author: "fixture".into(),
            gitlab: false,
            overlap: None,
            behind: None,
            conflict: None,
            head_branch: Some(format!("feature-{id}")),
        }
    }

    /// AC2: idle polls judge a refused change again only when its head moves
    /// or the window passes; unreadable evidence is wanted and never kept.
    // trace:TASK-3917 | ai:claude
    #[test]
    fn task3917_idle_polls_rejudge_a_refusal_only_on_a_new_head_or_after_the_window() {
        use crate::pr_train::MemberGateError::{Refused, Unavailable};
        type Verdict = Result<std::result::Result<(), crate::pr_train::MemberGateError>>;
        let asked = std::cell::RefCell::new(Vec::new());
        let mut refuse = |c: &forge::Change| -> Verdict {
            asked.borrow_mut().push(c.id);
            Ok(Err(Refused("not ready".into())))
        };
        let mut refused = Refusals::default();
        let changes = [change(1, "a"), change(2, "b")];
        for _ in 0..3 {
            assert!(!wanted_among(&changes, &mut refused, REJUDGE, &mut refuse).unwrap());
        }
        assert_eq!(*asked.borrow(), [1, 2], "cached across idle polls");
        let moved = [change(1, "a2"), change(2, "b")];
        assert!(!wanted_among(&moved, &mut refused, REJUDGE, &mut refuse).unwrap());
        assert_eq!(*asked.borrow(), [1, 2, 1], "a new head is judged");
        assert!(!wanted_among(&moved, &mut refused, Duration::ZERO, &mut refuse).unwrap());
        assert_eq!(
            *asked.borrow(),
            [1, 2, 1, 1, 2],
            "an expired refusal is judged"
        );

        let mut refused = Refusals::default();
        let mut unreadable = |c: &forge::Change| -> Verdict {
            asked.borrow_mut().push(c.id);
            Ok(Err(Unavailable("forge down".into())))
        };
        asked.borrow_mut().clear();
        for _ in 0..2 {
            assert!(wanted_among(&changes, &mut refused, REJUDGE, &mut unreadable).unwrap());
        }
        assert_eq!(*asked.borrow(), [1, 1], "unreadable evidence is not kept");
    }

    /// BUG-2771 AC1-AC3 through the real worker loop: a manual train that
    /// gives up waiting while train A runs stays recorded; the auto-train
    /// does not start train B until that manual train has run and released,
    /// and train-status/`--check`'s line names it.
    // trace:BUG-2771 | ai:claude
    // trace:BUG-2771.AC1 | ai:claude
    // trace:BUG-2771.AC2 | ai:claude
    // trace:BUG-2771.AC3 | ai:claude
    #[test]
    fn bug2771_auto_train_yields_its_next_train_to_a_recorded_manual_waiter() {
        let (_temp, root) = ci_gated_repo();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[admission]\nwait_secs = 1\n",
        )
        .unwrap();
        assert_eq!(
            crate::gate::landing_merge::train_stand_down(&root),
            Ok(None)
        );
        let (polls, trains) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let release = Arc::new(AtomicBool::new(false));
        let worker = {
            let (root, polls, trains, release) =
                (root.clone(), polls.clone(), trains.clone(), release.clone());
            std::thread::spawn(move || {
                run_passes(
                    &root,
                    1,
                    move || {
                        polls.fetch_add(1, Ordering::SeqCst);
                        Ok(true)
                    },
                    move || {
                        trains.fetch_add(1, Ordering::SeqCst);
                        while !release.load(Ordering::SeqCst) {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        Ok(())
                    },
                )
            })
        };
        wait_for("train A runs", || trains.load(Ordering::SeqCst) == 1);
        let error = crate::admission_owner::manual_train_admission(&root, false, &[4464])
            .err()
            .expect("train A holds admission past the manual train's bound");
        assert!(format!("{error:#}").contains("gave up"), "{error:#}");
        let line = crate::admission_owner::manual_waiters(&root)
            .first()
            .expect("the manual train stays recorded")
            .describe();
        assert!(
            line.starts_with("operator train for #4464 waiting since "),
            "{line}"
        );
        assert!(line.ends_with("; auto-train yields next"), "{line}");
        assert_eq!(
            crate::admission_owner::yield_reason(&root, "legacy review-watch"),
            None,
            "review controllers coexist with a manual train"
        );

        release.store(true, Ordering::SeqCst);
        let after = polls.load(Ordering::SeqCst);
        wait_for("three polls after train A", || {
            polls.load(Ordering::SeqCst) >= after + 3
        });
        assert_eq!(trains.load(Ordering::SeqCst), 1, "train B waits");
        assert!(claims(&root).is_empty(), "the auto-train yielded admission");

        let manual = crate::admission_owner::manual_train_admission(&root, false, &[4464])
            .unwrap()
            .expect("the manual train is admitted");
        assert!(crate::admission_owner::manual_waiters(&root).is_empty());
        let during = polls.load(Ordering::SeqCst);
        wait_for("a poll while the manual train runs", || {
            polls.load(Ordering::SeqCst) > during
        });
        assert_eq!(
            trains.load(Ordering::SeqCst),
            1,
            "not beside the manual train"
        );
        drop(manual);
        wait_for("train B after the manual train released", || {
            trains.load(Ordering::SeqCst) == 2
        });

        crate::review_watch::stop_test_loops(&root);
        assert!(worker.join().unwrap().is_err(), "stopped by the fixture");
    }
}
