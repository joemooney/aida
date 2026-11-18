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

pub(super) fn run(root: &Path) -> Result<()> {
    let mut admission = None;
    loop {
        train_service::check_controller(root)?;
        // Never kill a running train on rotation. It owns its journal and merge
        // leases until it returns; the next pass observes the new binding.
        let mut pass = crate::pr_train::TrainPass::default();
        let result = (|| {
            if admission.is_none() {
                admission = Some(crate::admission_owner::acquire(
                    root,
                    "legacy auto-train",
                    true,
                )?);
            }
            crate::pr_train::advisor_train_handler(&mut pass)
        })();
        // trace:BUG-2665 | ai:codex
        let contention = result.as_ref().err().is_some_and(|error| {
            error.is::<TrainAdmissionContention>()
                || error.is::<crate::admission_owner::AdmissionBusy>()
        });
        let skipped = contention || (result.is_ok() && pass.review_blocked);
        let monitoring = train_service::record_starvation(root, skipped, pass);
        if let Err(error) = &monitoring {
            eprintln!("auto-train: starvation monitoring failed: {error:#}");
        }
        match if contention {
            monitoring
        } else {
            result.and(monitoring)
        } {
            Ok(()) if skipped => {}
            Ok(()) => train_service::record_failure(root, None)?,
            Err(error) if error.is::<crate::pr_train::TrainJournalBusy>() => {
                eprintln!("auto-train: existing train owns the journal; leaving it alone");
            }
            Err(error) => {
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
