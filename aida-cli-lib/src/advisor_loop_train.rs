//! Fixed train pass; authority is adopted only between journaled trains.
// trace:STORY-2691 | ai:codex
use super::*;

pub(super) fn eligible(
    change: &forge::Change,
    evidence: &dyn ready::Evidence,
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
    ensure!(
        evidence
            .hold(change)
            .map_err(|e| Unavailable(format!("{e:#}")))?
            .is_none(),
        "merge hold present"
    );
    // The authoritative gate proves carry-over under protected fast policy;
    // strict policy and invalid proofs refuse there. CI below still binds head.
    // trace:STORY-2763 | ai:codex
    let approval = evidence.train_approval(change)?;
    ensure!(
        approval.reviewed == change.head || approval.carried.is_some(),
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
    run_passes(
        root,
        180,
        || train_wanted(root),
        crate::pr_train::advisor_train_handler,
    )
}

/// Whether an automatic pass has a train to assemble: a journaled train to
/// recover, or an open change the automatic gate admits. Read without
/// admission, so a poll with nothing to train never holds it. Evidence that
/// cannot be read counts as wanted: the full pass then judges (and reports)
/// that change exactly as before.
// trace:BUG-2769 | ai:claude
fn train_wanted(root: &Path) -> Result<bool> {
    let main = crate::main_worktree_root_from(root);
    if crate::pr_train::automatic_recovery_needed(&main)? {
        return Ok(true);
    }
    let repo = forge::Repository::load(&main)?;
    let evidence = ready::Live {
        root: &main,
        repo: &repo,
    };
    for change in repo.discover(&main)? {
        match eligible(&change, &evidence) {
            Ok(()) | Err(crate::pr_train::MemberGateError::Unavailable(_)) => return Ok(true),
            Err(_) => {}
        }
    }
    Ok(false)
}

/// A claim held longer than this is logged on release with its phase.
// trace:BUG-2769 | ai:claude
const HOLD_WARN: std::time::Duration = std::time::Duration::from_secs(30);

/// The release line for a hold of `held` in `phase`, when it is long.
// trace:BUG-2769 | ai:claude
fn long_hold_line(held: std::time::Duration, phase: &str) -> Option<String> {
    (held > HOLD_WARN).then(|| {
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
        match crate::gate::landing_merge::auto_train_pass(stand_down, &mut pass) {
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
        // trace:BUG-2769 | ai:claude
        if admission.take().is_some() {
            if let Some(line) = long_hold_line(held_since.elapsed(), "train pass") {
                eprintln!("{line}");
            }
        }
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

    /// AC1-AC3 through the real worker loop: while the unclaimed pre-scan
    /// finds nothing to train, no admission is held across polls and a
    /// contending manual train is admitted at once (well inside one poll).
    /// A wanted pass takes the claim and gives it up when it returns.
    #[test]
    fn bug2769_idle_auto_train_holds_no_admission_and_admits_a_manual_train() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
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
        let manual = crate::admission_owner::manual_train_admission(&root, false)
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

    /// AC4: a hold past the threshold names the holder and the phase.
    #[test]
    fn bug2769_a_long_hold_names_the_holder_and_phase() {
        assert_eq!(long_hold_line(Duration::from_secs(5), "train pass"), None);
        let line = long_hold_line(Duration::from_secs(45), "train pass").unwrap();
        assert!(line.contains("held admission 45s"), "{line}");
        assert!(
            line.contains(&format!("holder pid {}", std::process::id())),
            "{line}"
        );
        assert!(line.contains("phase train pass"), "{line}");
    }
}
