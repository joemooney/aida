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
// trace:STORY-2905 | ai:claude
pub(super) fn hold_admission(
    root: &Path,
    stand_down: &std::result::Result<Option<&'static str>, String>,
    held: &mut Option<crate::admission_owner::Claim>,
) -> Result<()> {
    if !matches!(stand_down, Ok(None)) {
        *held = None;
        return Ok(());
    }
    if held.is_none() {
        *held = Some(crate::admission_owner::acquire(
            root,
            "legacy auto-train",
            true,
        )?);
    }
    Ok(())
}

pub(super) fn run(root: &Path) -> Result<()> {
    let mut admission: Option<crate::admission_owner::Claim> = None;
    loop {
        train_service::check_controller(root)?;
        // trace:STORY-2856 | ai:codex
        if let Err(error) = crate::ci_startup_monitor::check(root) {
            eprintln!("auto-train: startup monitor unavailable: {error:#}");
        }
        let mut stand_down = crate::gate::landing_merge::train_stand_down(root);
        if let Err(error) = hold_admission(root, &stand_down, &mut admission) {
            stand_down = Err(format!("admission is held elsewhere ({error:#})"));
        }
        // In review-gated mode the land loop is the one merge hand, and a
        // policy that cannot be read runs nothing: the pass decision is
        // landing_merge::auto_train_pass. trace:STORY-2905 | ai:claude
        match crate::gate::landing_merge::auto_train_pass(
            stand_down,
            crate::pr_train::advisor_train_handler,
        ) {
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
            Ok(Err(error)) => {
                let reason = format!("{error:#}");
                eprintln!("auto-train: pass failed: {reason}");
                train_service::record_failure(root, Some(&reason))?;
            }
        }
        // Check heartbeat and grant once a second while idle, so rotation does
        // not need to wait for the next three-minute discovery interval.
        for _ in 0..180 {
            std::thread::sleep(std::time::Duration::from_secs(1));
            train_service::check_controller(root)?;
        }
    }
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
}
