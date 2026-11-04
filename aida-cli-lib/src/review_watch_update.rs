//! The one branch write the review watcher performs. A stale head that
//! overlaps its base was parked forever once its author had been notified:
//! the authors are mostly headless lanes that have already exited, so nobody
//! merged the base and nothing was reviewed (BUG-2426). When the author holds
//! no live lease on the branch, the watcher asks the forge to merge the base
//! into the branch itself; the new head is then reviewed like any other. When
//! the forge refuses (conflicts), the watcher cannot update the branch, the
//! author's liveness cannot be judged, or an accepted update produces no new
//! head in time, the change is routed to the advisor.
//!
//! This file is the only watcher module allowed to name the forge's branch
//! update; every other watcher file stays free of branch writes (see the
//! `task1993_watcher_never_signals_merges_or_runs_cargo` ratchet).
// trace:BUG-2426 | ai:claude
use super::*;

/// The watcher's branch update for one stale head: when it was requested
/// and, when the forge refused it or it could not run, why, plus the
/// advisor routing it produced.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BranchUpdate {
    pub head: String,
    pub at: chrono::DateTime<chrono::Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    /// A transient failure awaiting its one retry on a later pass.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub retry: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<Notice>,
}

impl BranchUpdate {
    pub(super) fn valid(&self) -> bool {
        valid_sha(&self.head)
            && self
                .refused
                .as_ref()
                .is_none_or(|r| r.len() <= DIAGNOSTIC_LIMIT)
    }
}

/// Why the forge did not update the branch. A transient failure (a timeout,
/// or a non-conflict forge error such as a network or auth flake) is retried
/// once on a later pass before the change is routed.
#[derive(Debug, Clone)]
pub(super) struct UpdateFailure {
    pub cause: String,
    pub transient: bool,
}

impl UpdateFailure {
    pub(super) fn refused(cause: impl Into<String>) -> Self {
        Self {
            cause: cause.into(),
            transient: false,
        }
    }
    pub(super) fn transient(cause: impl Into<String>) -> Self {
        Self {
            cause: cause.into(),
            transient: true,
        }
    }
}

/// Whether the change's author still has a live session on its branch:
/// `Some(true)` a live lease holds the branch, `Some(false)` none does,
/// `None` the branch is unknown so liveness cannot be judged.
pub(super) type Liveness<'a> = &'a dyn Fn(&forge::Change) -> Option<bool>;
/// Ask the forge to bring the change's branch up to date with its base.
pub(super) type Updater<'a> = &'a dyn Fn(&forge::Change) -> std::result::Result<(), UpdateFailure>;

/// How long an accepted update may go without the forge reporting a new
/// head before the change is routed to the advisor.
pub(super) const UPDATE_WAIT_MINUTES: i64 = 30;

/// The skip cause, and the advisor-mail sentence, when the forge did not
/// report the change's branch.
const UNKNOWN_BRANCH: &str = "the forge did not report the change's branch, so the watcher cannot tell whether its author is live and did not update it";

/// A live lease (live active or creator process) holds the change's branch.
pub(super) fn author_live(root: &Path, change: &forge::Change) -> Option<bool> {
    let branch = change.head_branch.as_deref()?;
    Some(crate::status_prs::live_owned_branches(root).contains(branch))
}

/// `gh pr update-branch`, pinned to the watcher's repository like discovery.
/// GitLab has no merge-based equivalent the watcher may run, so an MR is
/// routed to the advisor instead.
pub(super) fn forge_update(
    root: &Path,
    repo: &forge::Repository,
    change: &forge::Change,
) -> std::result::Result<(), UpdateFailure> {
    let Some(slug) = repo.github_slug() else {
        return Err(UpdateFailure::refused(
            "the watcher cannot update a GitLab merge request branch",
        ));
    };
    let mut command = Command::new("gh");
    command
        .args([
            "pr",
            "update-branch",
            &change.id.to_string(),
            "--repo",
            &slug,
        ])
        .current_dir(root)
        .stdin(Stdio::null());
    repo.pin_command(&mut command);
    let out = crate::command_output_with_timeout(command, std::time::Duration::from_secs(60))
        .ok_or_else(|| {
            UpdateFailure::transient("update-branch did not run or exceeded 60 seconds")
        })?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stderr = stderr.trim();
    Err(if update_refused_for_conflict(stderr) {
        UpdateFailure::refused(format!("update-branch conflicts: {stderr}"))
    } else {
        UpdateFailure::transient(format!("update-branch failed ({}): {stderr}", out.status))
    })
}

/// `gh pr update-branch` reports a base that does not merge cleanly with
/// "merge conflict" prose; any other refusal is reported as a failure.
fn update_refused_for_conflict(stderr: &str) -> bool {
    // external-prose-classifier: review_watch::update::update_refused_for_conflict
    stderr.to_lowercase().contains("conflict")
}

/// The stale-head follow-up, run before any base probe. `None` when this
/// head has no sent stale notice (the ordinary path decides). Otherwise the
/// reason the head is not reviewed this pass. Per head the watcher makes at
/// most one branch update, plus one retry of a transient failure, and at
/// most one advisor routing: when the update is refused, when the author's
/// liveness cannot be judged, or when an accepted update produces no new
/// head within [`UPDATE_WAIT_MINUTES`].
pub(super) fn follow_up(
    root: &Path,
    store: &Store,
    change: &forge::Change,
    live: Liveness<'_>,
    update: Updater<'_>,
) -> Result<Option<String>> {
    let key = change.key();
    let head = change.head.as_str();
    let recorded = store.transaction(&key, |state| {
        let Some(entry) = state.entries.get_mut(&key) else {
            return Ok(None);
        };
        if !entry.notices.get(head).is_some_and(|n| n.sent) {
            return Ok(None);
        }
        // An update recorded for an earlier head is moot.
        if entry.update.as_ref().is_some_and(|u| u.head != head) {
            entry.update = None;
        }
        Ok(Some(entry.update.clone()))
    })?;
    let Some(recorded) = recorded else {
        return Ok(None);
    };
    let now = chrono::Utc::now();
    let (outcome, changed) = match recorded {
        Some(done) if !done.retry => {
            let overdue = done.refused.is_none()
                && done.route.is_none()
                && now - done.at >= chrono::Duration::minutes(UPDATE_WAIT_MINUTES);
            if overdue {
                let situation = format!(
                    "Its author has no live session on the branch. The forge accepted the watcher's branch update at {} but has reported no new head in {UPDATE_WAIT_MINUTES} minutes.",
                    done.at.format("%Y-%m-%d %H:%M UTC")
                );
                let route = routed(
                    store,
                    &key,
                    change,
                    "update-branch produced no new head",
                    &situation,
                );
                (
                    BranchUpdate {
                        route: Some(route),
                        ..done
                    },
                    true,
                )
            } else {
                (done, false)
            }
        }
        pending => match live(change) {
            Some(true) => {
                return Ok(Some(format!(
                    "{STALE_NOTIFIED}; the author's session on `{}` is live and owns the merge",
                    change.head_branch.as_deref().unwrap_or("?")
                )))
            }
            // Liveness cannot be judged: leave the branch alone, but route
            // the change rather than park it with nobody acting.
            None => {
                let route = routed(
                    store,
                    &key,
                    change,
                    "the watcher cannot judge its author",
                    "The forge did not report the change's branch, so the watcher cannot tell whether its author is live and did not update it.",
                );
                (
                    BranchUpdate {
                        head: change.head.clone(),
                        at: now,
                        refused: Some(UNKNOWN_BRANCH.into()),
                        retry: false,
                        route: Some(route),
                    },
                    true,
                )
            }
            Some(false) => (
                attempt(store, &key, change, update, pending.is_some(), now),
                true,
            ),
        },
    };
    if changed {
        let kept = outcome.clone();
        store.transaction(&key, |state| {
            state.entries.entry(key.clone()).or_default().update = Some(kept);
            Ok(())
        })?;
    }
    if let Some(route) = outcome.route.as_ref().filter(|r| !r.sent) {
        deliver(root, route)?;
        store.transaction(&key, |state| {
            if let Some(update) = state
                .entries
                .get_mut(&key)
                .and_then(|e| e.update.as_mut())
                .filter(|u| u.head == head)
            {
                if let Some(route) = update.route.as_mut() {
                    route.sent = true;
                }
            }
            Ok(())
        })?;
        eprintln!("{key}: routed to the advisor");
    }
    Ok(Some(skip_reason(change, &outcome)))
}

/// Run the forge update once. A first transient failure is kept for one
/// retry on a later pass; a refusal, or a transient failure on the retry, is
/// routed to the advisor.
fn attempt(
    store: &Store,
    key: &str,
    change: &forge::Change,
    update: Updater<'_>,
    retrying: bool,
    now: chrono::DateTime<chrono::Utc>,
) -> BranchUpdate {
    let mut outcome = BranchUpdate {
        head: change.head.clone(),
        at: now,
        refused: None,
        retry: false,
        route: None,
    };
    let Err(failure) = update(change) else {
        return outcome;
    };
    let cause = bounded_diagnostic(&failure.cause);
    if failure.transient && !retrying {
        outcome.refused = Some(cause);
        outcome.retry = true;
        return outcome;
    }
    let mut situation = format!(
        "Its author has no live session on the branch. The watcher asked the forge to update the branch and it did not: {cause}."
    );
    if failure.transient {
        situation.push_str(
            " The cause may be transient (a timeout, network or forge auth error): the update failed on two separate passes.",
        );
    }
    let tail = if cause.starts_with("update-branch conflicts") {
        "update-branch conflicts"
    } else {
        "the watcher could not update it"
    };
    outcome.route = Some(routed(store, key, change, tail, &situation));
    outcome.refused = Some(cause);
    outcome
}

/// Why the head is skipped this pass, for stderr and `aida awaiting`.
fn skip_reason(change: &forge::Change, outcome: &BranchUpdate) -> String {
    let at = outcome.at.format("%Y-%m-%d %H:%M UTC");
    match (&outcome.refused, &outcome.route) {
        (Some(cause), _) if outcome.retry => format!(
            "{} with overlap; {cause}; retried once on the next pass",
            behind_phrase(change)
        ),
        (Some(cause), _) => format!(
            "{} with overlap; {cause}; routed to the advisor",
            behind_phrase(change)
        ),
        (None, Some(_)) => format!(
            "{STALE_UPDATED} at {at}; no new head after {UPDATE_WAIT_MINUTES} minutes; routed to the advisor"
        ),
        (None, None) => format!(
            "{STALE_UPDATED} at {at}; the merged head is reviewed once the forge reports it"
        ),
    }
}

/// Prefixes of the stale-head skip reasons, shared with the awaiting view.
pub(super) const STALE_NOTIFIED: &str = "stale overlapping head already notified";
pub(super) const STALE_UPDATED: &str = "branch update requested by the watcher";

fn behind_phrase(change: &forge::Change) -> String {
    format!("behind `{}`", change.base)
}

/// An unsent advisor routing for this head.
fn routed(
    store: &Store,
    key: &str,
    change: &forge::Change,
    subject_tail: &str,
    situation: &str,
) -> Notice {
    Notice {
        message: route_message(&store.repository, key, change, subject_tail, situation),
        sent: false,
    }
}

/// The advisor routing for a stale head the watcher could not get updated.
/// A deterministic ID per head makes a retry idempotent; each head is routed
/// at most once.
fn route_message(
    repository: &str,
    key: &str,
    change: &forge::Change,
    subject_tail: &str,
    situation: &str,
) -> aida_core::mailbox::Message {
    use aida_core::mailbox::{Intent, Message, Recipient, SenderSource};
    let id = format!(
        "review-watch-{:x}",
        Sha256::digest(format!("{repository}\0{key}\0{}\0update", change.head))
    );
    Message {
        id: id.clone(),
        thread_id: id,
        from: "review-watch".into(),
        to: Recipient::Agent("advisor".into()),
        timestamp: chrono::Utc::now().timestamp_millis(),
        in_reply_to: None,
        body: format!(
            "{key} at {} is {} with overlapping paths. {situation} Merge `{}` into the branch (resolving conflicts) and push, or close the change; the watcher reviews the new head on its next pass.",
            change.head,
            behind_phrase(change),
            change.base
        ),
        subject: Some(format!(
            "{key} {} with overlap; {subject_tail}",
            behind_phrase(change)
        )),
        urgent: false,
        intent: Intent::Request,
        retracted: false,
        deleted: false,
        archived: false,
        from_source: SenderSource::Explicit,
        from_role: None,
        relayed_from: None,
    }
}
