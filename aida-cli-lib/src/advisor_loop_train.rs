//! Fixed train pass; authority is adopted only between journaled trains.
// trace:STORY-2691 | ai:codex
use super::*;

pub(super) fn eligible(change: &forge::Change, evidence: &dyn ready::Evidence) -> Result<()> {
    anyhow::ensure!(!change.gitlab, "automatic trains require GitHub");
    anyhow::ensure!(
        change
            .head_branch
            .as_deref()
            .is_some_and(|b| !b.starts_with(crate::pr_train::BRANCH_PREFIX)),
        "train branch or unknown source branch"
    );
    anyhow::ensure!(
        evidence.under_review(change).is_none(),
        "review still running"
    );
    anyhow::ensure!(evidence.hold(change)?.is_none(), "merge hold present");
    let approval = evidence.approval(change).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(
        approval.reviewed == change.head && approval.carried.is_none(),
        "approval must be recorded at the exact head"
    );
    let mut checks = evidence.ci(&change.head)?;
    anyhow::ensure!(
        checks.sha.eq_ignore_ascii_case(&change.head),
        "CI is for another head"
    );
    let required = evidence.required_checks()?;
    // The ordinary train gate also resolves forge-required policy. With no
    // configured list, preserve the conservative all-checks readiness rule.
    // Informational checks cannot delay an explicitly configured required set.
    if !required.is_empty() {
        checks.checks.retain(|c| required.contains(&c.name));
    }
    let names: Vec<&str> = required.iter().map(String::as_str).collect();
    anyhow::ensure!(
        matches!(
            crate::forge::judge_commit_checks(&checks, &names),
            crate::forge::CommitCi::Green
        ),
        "required CI is not green at the head"
    );
    anyhow::ensure!(
        evidence.head(change)? == change.head,
        "head moved during evaluation"
    );

    Ok(())
}

pub(super) fn run(root: &Path) -> Result<()> {
    loop {
        train_service::check_controller(root)?;
        // Never kill a running train on rotation. It owns its journal and merge
        // leases until it returns; the next pass observes the new binding.
        match crate::pr_train::advisor_train_handler() {
            Ok(()) => train_service::record_failure(root, None)?,
            Err(error)
                if error
                    .downcast_ref::<crate::pr_train::TrainJournalBusy>()
                    .is_some() =>
            {
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
