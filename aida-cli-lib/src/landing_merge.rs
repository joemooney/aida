//! The one production merge boundary (STORY-2897 S1c). Every AIDA merge
//! path (`aida pr ship`, the drain's merge phase, the burndown wave, `aida
//! release --after-pr`, and the train) calls [`merge`] instead of
//! `Forge::merge_change`, so a new caller cannot skip the landing checks: a
//! source-scan test fails on any other production `merge_change` call.
//!
//! Immediately before the forge merge, [`merge`] re-reads the landing policy
//! from the freshly fetched default branch and refuses if it changed since
//! the caller started, re-reads the operator's floor authority, and applies
//! the merger and author rules the policy implies. Train merges pass through
//! the boundary without these checks until S1t (STORY-2905) gives each train
//! car its own authority.
// trace:STORY-2903 trace:STORY-2897 | ai:claude

use crate::forge::{ChangeRef, Forge, MergeOptions, MergeResult};
use crate::gate::ship::ShipRefusal;

/// Which merge path is merging.
// trace:STORY-2903 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Caller {
    Ship,
    Drain,
    Burndown,
    Release,
    /// The train: checked per car by S1t (STORY-2905).
    Train,
}

/// What a merge path started under, captured when it started: the landing
/// policy, and whether the operator's floor authority held.
// trace:STORY-2903 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub(crate) policy: crate::landing_policy::LandingPolicy,
    pub(crate) floor: bool,
}

impl Entry {
    /// The policy and floor authority in force now, for a path starting.
    pub(crate) fn capture(root: &std::path::Path) -> Self {
        Self {
            policy: crate::landing_policy::load(root),
            floor: crate::gate::role_authority::has_integrity_floor_authority(),
        }
    }
}

/// Why [`merge`] did not merge.
// trace:STORY-2903 | ai:claude
#[derive(Debug)]
pub(crate) enum MergeError {
    /// The landing checks refused; nothing was sent to the forge.
    Refused(ShipRefusal, String),
    /// The forge merge itself failed.
    Forge(anyhow::Error),
}

impl std::fmt::Display for MergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(_, why) => write!(f, "{why}"),
            Self::Forge(e) => write!(f, "{e:#}"),
        }
    }
}

impl MergeError {
    /// The error as the CLI reports it: a refusal keeps its exit category.
    pub(crate) fn into_anyhow(self) -> anyhow::Error {
        match self {
            Self::Refused(kind, why) => kind.error(why),
            Self::Forge(e) => e,
        }
    }
}

/// Merge `change` through `forge`, after the landing re-checks for `caller`
/// (see the module docs). `spec_ids` are the specs whose authors the author
/// rule excludes.
// trace:STORY-2903 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn merge(
    forge: &dyn Forge,
    root: &std::path::Path,
    caller: Caller,
    entry: &Entry,
    change: &ChangeRef,
    spec_ids: &[String],
    opts: &MergeOptions,
    sink: &mut dyn crate::network_retry::RetrySink,
) -> Result<MergeResult, MergeError> {
    if caller != Caller::Train {
        recheck(
            root,
            change,
            &entry.policy,
            spec_ids,
            entry.floor,
            crate::gate::role_authority::has_integrity_floor_authority,
        )
        .map_err(|(kind, why)| MergeError::Refused(kind, why))?;
    }
    forge
        .merge_change(change, opts, sink)
        .map_err(MergeError::Forge)
}

/// Whether the repository at `root` has an `origin` remote.
fn has_origin(root: &std::path::Path) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["remote", "get-url", "origin"])
        .output()
        .is_ok_and(|o| o.status.success())
}

/// The landing re-checks made immediately before the forge merge call,
/// after every wait: the effective `[landing]` policy is re-read (after the
/// change's refs AND the trusted default branch are refreshed: a ship to a
/// non-default base would otherwise read a stale default-branch policy) and
/// must match the one this ship started under; the integrity-floor
/// authority is re-read (a deputy marker can expire while the ship waits)
/// and decides both the advisor-merger rule and, when the policy opts into
/// review-gated landing or the advisor merger, C2 (the session merging
/// authored none of the change). A refusal carries its category: a ref
/// that cannot be fetched is `BehindBase`; authority and policy are
/// `Review`.
// trace:STORY-2897 | ai:claude
pub(crate) fn recheck(
    project_root: &std::path::Path,
    change: &crate::forge::ChangeRef,
    entry: &crate::landing_policy::LandingPolicy,
    spec_ids: &[String],
    floor_at_entry: bool,
    floor_now: impl FnOnce() -> bool,
) -> std::result::Result<(), (crate::gate::ship::ShipRefusal, String)> {
    use crate::gate::ship::ShipRefusal;
    let subject = format!("PR-{}", change.id);
    let base = if change.base.trim().is_empty() {
        crate::pr_cmd::pr_ship_target_branch(change.id)
    } else {
        change.base.clone()
    };
    // Refresh the refs first: the trusted policy is read from the default
    // branch as it is now.
    // A repository with no `origin` has no remote to be stale against: its
    // local refs are the trusted ones. Every forge-backed project has one.
    let remote = has_origin(project_root);
    if remote {
        // A caller that knows only the PR number (the drain) has no branch
        // to fetch; the base alone carries the policy freshness.
        let branch = if change.branch.trim().is_empty() {
            base.as_str()
        } else {
            change.branch.as_str()
        };
        crate::pr_cmd::fetch_ship_refs(project_root, &base, branch)
            .map_err(|e| (ShipRefusal::BehindBase, format!("PR-{}: {e}", change.id)))?;
    }
    let default = crate::trusted_config::local_default_branch_name(project_root)
        .unwrap_or_else(|| crate::forge::default_branch_of(project_root));
    if remote && default != base {
        crate::pr_cmd::fetch_ship_refs(project_root, &default, &default).map_err(|e| {
            (
                ShipRefusal::Review,
                format!(
                    "{subject} is not merged here: the landing policy on the default branch \
                     `{default}` could not be refreshed ({e})"
                ),
            )
        })?;
    }
    let policy = crate::landing_policy::load(project_root);
    if let Some(why) = crate::landing_policy::policy_change_refusal(entry, &policy, &subject) {
        return Err((ShipRefusal::Review, why));
    }
    let floor = floor_now();
    if let Some(why) = crate::gate::ship::floor_lapse_refusal(floor_at_entry, floor, &subject) {
        return Err((ShipRefusal::Review, why));
    }
    if let Some(why) = crate::gate::ship::advisor_merge_refusal(
        policy.merger == crate::landing_policy::Merger::Advisor,
        crate::gate::seat_authority::current_seat(project_root).as_deref(),
        floor,
        &subject,
    ) {
        return Err((ShipRefusal::Review, why));
    }
    // C2: once `[landing]` opts into review-gated landing or the advisor
    // merger, whoever merges authored none of the change; the operator at a
    // terminal is the explicit exception. The defaults keep today's
    // behaviour, where an author may ship their own approved change.
    let opted_in = policy.mode == crate::landing_policy::LandingMode::ReviewGated
        || policy.merger == crate::landing_policy::Merger::Advisor;
    if opted_in && !floor {
        let (author_ids, author_branch) =
            crate::pr_cmd::ship_author_ids(project_root, change.id, spec_ids, Some(&change.branch));
        let authors = crate::gate::approval::ship_approval_policy(
            project_root,
            &author_ids,
            author_branch.as_deref(),
            &[],
        );
        if let Some(why) = crate::gate::ship::shipper_author_refusal(&authors, &subject) {
            return Err((ShipRefusal::Review, why));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/story_2897_final_recheck_tests.rs"]
mod story_2897_final_recheck_tests;

#[cfg(test)]
#[path = "tests/story_2903_merge_boundary_tests.rs"]
mod story_2903_merge_boundary_tests;
