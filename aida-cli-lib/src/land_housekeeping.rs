//! The housekeeping step of `aida land run`: work left behind by lanes that
//! died is saved, and specs left In Progress with nobody on them go back to
//! the queue.
//!
//! Nothing here deletes a worktree or a branch. A dead lane's uncommitted
//! work is written to a patch, the patch path is recorded on the spec, and
//! only then is the dead lease released. A patch that cannot be written
//! leaves the lease where it is.
// trace:STORY-2768 | ai:claude

use super::{specs_of, Pr};
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Branches the landing itself owns. Nothing on them is salvaged or reset.
const LANDING_PREFIXES: [&str; 2] = ["train/", "promote/"];
/// A spec carrying one of these is never reset unattended.
const HUMAN_TAGS: [&str; 3] = ["supervised", "operator-only", "architecture"];

/// A lease whose process is dead and whose worktree has uncommitted work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeadLane {
    pub lease: String,
    pub worktree: PathBuf,
    pub branch: String,
    /// The spec the lease is scoped to, when it names one.
    pub spec: Option<String>,
    pub agent: Option<String>,
}

/// A spec that is In Progress with no live session on this host.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Orphan {
    pub spec: String,
    /// Every id the spec answers to, for matching a change's title.
    pub ids: Vec<String>,
    pub tags: Vec<String>,
    /// Who holds it on another host, when its latest lease or claim is there.
    pub elsewhere: Option<String>,
    /// Why it may still be worked here (a subagent, a fan-out, a driver).
    pub maybe_worked: Option<String>,
    /// Branches on origin named for the spec.
    pub branches: Vec<String>,
}

/// One thing the housekeeping step does, or reports and leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step {
    /// Save the lane's work to a patch, record it, release the lease.
    Salvage(DeadLane),
    /// Send the spec back to the queue. `resume` is a branch on origin with
    /// no open change: the next pickup continues it.
    Reset {
        spec: String,
        resume: Option<String>,
    },
    /// Left for a human, with the reason.
    Attention { what: String, why: String },
    /// Left alone, with the reason.
    Skip { what: String, why: String },
}

fn landing_branch(branch: &str) -> bool {
    LANDING_PREFIXES
        .iter()
        .any(|prefix| branch.starts_with(prefix))
}

/// What the step does about `lanes` and `orphans`, given the open changes.
/// A spec an open change names is not orphaned: the change's row owns it,
/// whatever branch the change is on, a train or a promotion included.
pub(crate) fn plan(lanes: Vec<DeadLane>, orphans: Vec<Orphan>, changes: &[Pr]) -> Vec<Step> {
    let mut steps = Vec::new();
    for lane in lanes {
        if landing_branch(&lane.branch) {
            steps.push(Step::Skip {
                what: format!("the dead lane on `{}`", lane.branch),
                why: "a landing branch is never salvaged".into(),
            });
        } else {
            steps.push(Step::Salvage(lane));
        }
    }
    let named: Vec<String> = changes.iter().flat_map(specs_of).collect();
    let open_branches: Vec<&str> = changes
        .iter()
        .filter_map(|change| change.branch.as_deref())
        .collect();
    for orphan in orphans {
        let owned = orphan
            .ids
            .iter()
            .chain(std::iter::once(&orphan.spec))
            .any(|id| named.iter().any(|name| name.eq_ignore_ascii_case(id)));
        if owned {
            continue;
        }
        let what = orphan.spec.clone();
        if let Some(holder) = orphan.elsewhere {
            steps.push(Step::Skip {
                what,
                why: format!("its latest session is on another host ({holder})"),
            });
        } else if let Some(why) = orphan.maybe_worked {
            steps.push(Step::Skip { what, why });
        } else if let Some(tag) = orphan
            .tags
            .iter()
            .find(|tag| HUMAN_TAGS.contains(&tag.as_str()))
        {
            steps.push(Step::Attention {
                what,
                why: format!("In Progress with no session, and tagged `{tag}`: a human resets it"),
            });
        } else {
            let resume = orphan.branches.into_iter().find(|branch| {
                !open_branches.contains(&branch.as_str()) && !landing_branch(branch)
            });
            steps.push(Step::Reset { spec: what, resume });
        }
    }
    steps
}

/// The acts of the housekeeping step, behind one seam so tests count them.
pub(crate) trait Keeper {
    /// Write the lane's uncommitted work to a patch. `Ok(None)` when its
    /// worktree has nothing uncommitted any more.
    fn salvage(&mut self, lane: &DeadLane) -> Result<Option<PathBuf>>;
    /// Record `text` on the spec.
    fn note(&mut self, spec: &str, text: &str) -> Result<()>;
    /// Release the dead lease. The worktree and its branch stay.
    fn release(&mut self, lane: &DeadLane) -> Result<()>;
    /// Send the spec back to the implementer queue.
    fn reset(&mut self, spec: &str) -> Result<()>;
}

/// Carry out `steps`, or on a dry run say what would be done. Returns one
/// line per step.
pub(crate) fn keep(steps: &[Step], keeper: &mut dyn Keeper, dry_run: bool) -> Vec<String> {
    let mut notes = Vec::new();
    for step in steps {
        match step {
            Step::Skip { what, why } => notes.push(format!("{what}: left alone, {why}")),
            Step::Attention { what, why } => notes.push(format!("{what}: needs a human, {why}")),
            Step::Salvage(lane) => {
                let what = format!(
                    "dead lane {} in {}",
                    lane.spec.as_deref().unwrap_or(&lane.branch),
                    lane.worktree.display()
                );
                if dry_run {
                    notes.push(format!(
                        "{what}: would save its uncommitted work to a patch and release its lease (dry run)"
                    ));
                    continue;
                }
                let patch = match keeper.salvage(lane) {
                    Ok(Some(patch)) if patch.is_file() => patch,
                    Ok(Some(patch)) => {
                        notes.push(format!(
                            "{what}: the patch {} is not there; the lease is kept",
                            patch.display()
                        ));
                        continue;
                    }
                    Ok(None) => {
                        notes.push(format!(
                            "{what}: nothing uncommitted is left to save; the lease is kept for the reaper"
                        ));
                        continue;
                    }
                    Err(error) => {
                        notes.push(format!(
                            "{what}: its work could not be saved, so the lease is kept: {error:#}"
                        ));
                        continue;
                    }
                };
                // The patch is safe on disk; a failed record is reported and
                // does not keep a dead lease that would be salvaged again.
                let recorded = match &lane.spec {
                    Some(spec) => match keeper.note(
                        spec,
                        &format!(
                            "aida land run: the lane in {} died with uncommitted work. It is saved in {} (apply with `git apply`). The worktree and its branch `{}` were left as they are.",
                            lane.worktree.display(),
                            patch.display(),
                            lane.branch
                        ),
                    ) {
                        Ok(()) => format!("recorded on {spec}"),
                        Err(error) => format!("not recorded on {spec} ({error:#})"),
                    },
                    None => "no spec to record it on".into(),
                };
                match keeper.release(lane) {
                    Ok(()) => notes.push(format!(
                        "{what}: saved to {} ({recorded}); lease released, worktree left alone",
                        patch.display()
                    )),
                    Err(error) => notes.push(format!(
                        "{what}: saved to {} ({recorded}); the lease was not released: {error:#}",
                        patch.display()
                    )),
                }
            }
            Step::Reset { spec, resume } => {
                let how = match resume {
                    Some(branch) => format!("its branch `{branch}` on origin has no open change, so the next pickup continues it"),
                    None => "no branch of it is on origin".into(),
                };
                if dry_run {
                    notes.push(format!(
                        "{spec}: In Progress with no session; would go back to the queue ({how}) (dry run)"
                    ));
                    continue;
                }
                if let Some(branch) = resume {
                    if let Err(error) = keeper.note(
                        spec,
                        &format!(
                            "aida land run: In Progress with no session and no open change, so it went back to the queue. Unfinished work is on origin as `{branch}`: resume it with `aida queue work {spec} --branch {branch}`; it is not a fresh start."
                        ),
                    ) {
                        notes.push(format!(
                            "{spec}: its branch `{branch}` could not be recorded on it, so it is not reset: {error:#}"
                        ));
                        continue;
                    }
                }
                match keeper.reset(spec) {
                    Ok(()) => notes.push(format!(
                        "{spec}: In Progress with no session; sent back to the queue ({how})"
                    )),
                    Err(error) => notes.push(format!("{spec}: not reset: {error:#}")),
                }
            }
        }
    }
    notes
}

/// The branches of `remote_branches` named for a spec with these `ids`: the
/// lowercased id, alone or followed by `-`.
pub(crate) fn spec_branches(ids: &[String], remote_branches: &[String]) -> Vec<String> {
    remote_branches
        .iter()
        .filter(|branch| {
            ids.iter().any(|id| {
                let id = id.to_ascii_lowercase();
                branch
                    .strip_prefix(id.as_str())
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'))
            })
        })
        .cloned()
        .collect()
}

/// What is on this machine now: the dead lanes with uncommitted work and the
/// In-Progress specs with no live session. `foreign` are the live claims of
/// other clones and hosts.
pub(crate) fn survey(
    root: &Path,
    foreign: &[crate::coordination::Claim],
) -> Result<(Vec<DeadLane>, Vec<Orphan>)> {
    use crate::dispatch_health_ps::DispatchState;
    let (rows, orphans) = crate::gather_running_work(root);
    let lanes = rows
        .iter()
        .filter(|row| {
            row.dispatch
                .as_ref()
                .is_some_and(|dispatch| dispatch.state == DispatchState::Salvageable)
        })
        .map(|row| DeadLane {
            lease: row.lease.id.clone(),
            worktree: row.lease.worktree_path.clone(),
            branch: row.lease.branch.clone(),
            spec: row.spec.clone(),
            agent: row.role.clone().or_else(|| row.lease_role.clone()),
        })
        .collect();
    if orphans.is_empty() {
        return Ok((lanes, Vec::new()));
    }
    let store = crate::integration_cmd::load_store(root)?;
    let host = crate::coordination::hostname();
    let leases = crate::leases::list_leases(root);
    let remote_branches: Vec<String> = super::git_text(
        root,
        &[
            "for-each-ref",
            "--format=%(refname:lstrip=3)",
            "refs/remotes/origin",
        ],
    )?
    .lines()
    .map(String::from)
    .collect();
    let orphans = orphans
        .into_iter()
        .map(|orphan| {
            let spec = crate::integration_cmd::resolve_trailer(&store, &orphan.spec)
                .and_then(|uuid| store.requirements.iter().find(|r| r.id == uuid));
            let mut ids = vec![orphan.spec.clone()];
            let mut tags = Vec::new();
            if let Some(spec) = spec {
                ids.extend(spec.spec_id.clone());
                ids.extend(spec.agreed_id.clone());
                ids.push(spec.id.to_string());
                tags = spec.tags.iter().cloned().collect();
                tags.sort();
            }
            let names = |text: &str| ids.iter().any(|id| id.eq_ignore_ascii_case(text));
            // The newest lease scoped to the spec says where it was last
            // worked; a live claim from another host says it still is.
            let elsewhere = leases
                .iter()
                .filter(|lease| names(&lease.scope))
                .max_by_key(|lease| lease.started_at)
                .filter(|lease| lease.hostname != host)
                .map(|lease| lease.hostname.clone())
                .or_else(|| {
                    foreign
                        .iter()
                        .find(|claim| claim.host != host && names(&claim.node_id))
                        .map(|claim| claim.host.clone())
                });
            let maybe_worked = if orphan.likely_fanout {
                Some("a live fan-out names it".to_string())
            } else if orphan.possibly_subagent {
                Some("a subagent may be working in its worktree".to_string())
            } else {
                orphan
                    .driver
                    .as_ref()
                    .map(|driver| format!("it has a driver ({driver:?})"))
            };
            Orphan {
                branches: spec_branches(&ids, &remote_branches),
                spec: orphan.spec,
                ids,
                tags,
                elsewhere,
                maybe_worked,
            }
        })
        .collect();
    Ok((lanes, orphans))
}

#[cfg(test)]
#[path = "tests/story_2768_land_housekeeping_tests.rs"]
mod tests;
