//! The one branch write the review watcher performs. A stale head that
//! overlaps its base was parked forever once its author had been notified:
//! the authors are mostly headless lanes that have already exited, so nobody
//! merged the base and nothing was reviewed (BUG-2426). When the author holds
//! no live lease on the branch, the watcher asks the forge to merge the base
//! into the branch itself; the new head is then reviewed like any other. When
//! the forge refuses (conflicts), or the watcher cannot update the branch,
//! the change is routed to the advisor.
//!
//! This file is the only watcher module allowed to name the forge's branch
//! update; every other watcher file stays free of branch writes (see the
//! `task1993_watcher_never_signals_merges_or_runs_cargo` ratchet).
// trace:BUG-2426 | ai:claude
use super::*;

/// The watcher's branch update for one stale head: when it was requested
/// and, when the forge refused it or it could not run, why, plus the
/// advisor routing that refusal produced.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BranchUpdate {
    pub head: String,
    pub at: chrono::DateTime<chrono::Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
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

/// Whether the change's author still has a live session on its branch:
/// `Some(true)` a live lease holds the branch, `Some(false)` none does,
/// `None` the branch is unknown so liveness cannot be judged.
pub(super) type Liveness<'a> = &'a dyn Fn(&forge::Change) -> Option<bool>;
/// Ask the forge to bring the change's branch up to date with its base.
/// `Err(cause)` when the forge refused or the request could not run.
pub(super) type Updater<'a> = &'a dyn Fn(&forge::Change) -> std::result::Result<(), String>;

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
) -> std::result::Result<(), String> {
    let Some(slug) = repo.github_slug() else {
        return Err("the watcher cannot update a GitLab merge request branch".into());
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
        .ok_or_else(|| "update-branch did not run or exceeded 60 seconds".to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stderr = stderr.trim();
    Err(if update_refused_for_conflict(stderr) {
        format!("update-branch conflicts: {stderr}")
    } else {
        format!("update-branch failed ({}): {stderr}", out.status)
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
/// reason the head is not reviewed this pass, after at most one branch update
/// per head and, on refusal, one advisor routing.
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
    let outcome = match recorded {
        Some(done) => done,
        None => {
            match live(change) {
                Some(true) => {
                    return Ok(Some(format!(
                        "{STALE_NOTIFIED}; the author's session on `{}` is live and owns the merge",
                        change.head_branch.as_deref().unwrap_or("?")
                    )))
                }
                None => {
                    return Ok(Some(format!(
                        "{STALE_NOTIFIED}; the change's branch is unknown, so the watcher cannot tell whether its author is live and does not update it"
                    )))
                }
                Some(false) => {}
            }
            let refused = update(change).err().map(|e| bounded_diagnostic(&e));
            let route = refused.as_ref().map(|cause| Notice {
                message: route_message(&store.repository, &key, change, cause),
                sent: false,
            });
            let outcome = BranchUpdate {
                head: change.head.clone(),
                at: chrono::Utc::now(),
                refused,
                route,
            };
            let kept = outcome.clone();
            store.transaction(&key, |state| {
                state.entries.entry(key.clone()).or_default().update = Some(kept);
                Ok(())
            })?;
            outcome
        }
    };
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
    Ok(Some(match &outcome.refused {
        None => format!(
            "{STALE_UPDATED} at {}; the merged head is reviewed once the forge reports it",
            outcome.at.format("%Y-%m-%d %H:%M UTC")
        ),
        Some(cause) => format!(
            "{} with overlap; {cause}; routed to the advisor",
            behind_phrase(change)
        ),
    }))
}

/// Prefixes of the stale-head skip reasons, shared with the awaiting view.
pub(super) const STALE_NOTIFIED: &str = "stale overlapping head already notified";
pub(super) const STALE_UPDATED: &str = "branch update requested by the watcher";

fn behind_phrase(change: &forge::Change) -> String {
    format!("behind `{}`", change.base)
}

/// The advisor routing for a stale head the watcher could not update. A
/// deterministic ID per head makes a retry idempotent.
fn route_message(
    repository: &str,
    key: &str,
    change: &forge::Change,
    cause: &str,
) -> aida_core::mailbox::Message {
    use aida_core::mailbox::{Intent, Message, Recipient, SenderSource};
    let id = format!(
        "review-watch-{:x}",
        Sha256::digest(format!("{repository}\0{key}\0{}\0update", change.head))
    );
    let conflict = cause.starts_with("update-branch conflicts");
    let subject = if conflict {
        format!(
            "{key} {} with overlap; update-branch conflicts",
            behind_phrase(change)
        )
    } else {
        format!(
            "{key} {} with overlap; the watcher could not update it",
            behind_phrase(change)
        )
    };
    Message {
        id: id.clone(),
        thread_id: id,
        from: "review-watch".into(),
        to: Recipient::Agent("advisor".into()),
        timestamp: chrono::Utc::now().timestamp_millis(),
        in_reply_to: None,
        body: format!(
            "{key} at {} is {} with overlapping paths, and its author has no live session on the branch. The watcher asked the forge to update the branch and it did not: {cause}. Merge `{}` into the branch (resolving conflicts) and push, or close the change; the watcher reviews the new head on its next pass.",
            change.head,
            behind_phrase(change),
            change.base
        ),
        subject: Some(subject),
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
