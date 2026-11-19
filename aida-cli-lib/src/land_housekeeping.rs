//! The housekeeping step of `aida land run`: work left behind by lanes that
//! died is saved, and specs left In Progress with nobody on them go back to
//! the queue.
//!
//! Nothing here deletes a worktree or a branch. A dead lane's uncommitted
//! work is written to a patch, the patch path is recorded on the spec, and
//! only then is the dead lease released. A patch that cannot be written
//! whole, or a path that cannot be recorded, leaves the lease where it is,
//! and the lane's spec is not sent back to the queue in that pass.
//!
//! Every fact that permits a reset is read, not assumed: what origin holds
//! is asked of origin, and a read that fails stops the step.
//!
//! Releasing a lease and resetting a spec happen only when the project turns
//! them on (`[land] housekeeping_auto_release = true`). By default the step
//! saves and records a dead lane's work once, then reports: no lease file is
//! removed, no grant is revoked, and no spec is reset or requeued.
// trace:STORY-2768 trace:TASK-3796 | ai:claude

use super::{specs_of, Pr};
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Branches the landing itself owns. Nothing on them is salvaged or reset.
const LANDING_PREFIXES: [&str; 2] = ["train/", "promote/"];
/// A spec carrying one of these is never reset unattended.
const HUMAN_TAGS: [&str; 3] = ["supervised", "operator-only", "architecture"];

/// The `[land]` key that lets the step release dead leases and reset
/// orphaned specs. Off unless set to `true`.
// trace:TASK-3796 | ai:claude
pub(crate) const AUTO_RELEASE_KEY: &str = "housekeeping_auto_release";

/// What the step may do without a human, beyond saving and recording.
// trace:TASK-3796 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unattended {
    /// The default: save and record a dead lane's work, report the rest.
    ReportOnly,
    /// Also release a dead lease once its work is recorded, and send an
    /// orphaned spec back to the queue.
    ReleaseAndReset,
}

impl Unattended {
    /// `[land] housekeeping_auto_release`: only the boolean `true` turns
    /// release and reset on; anything else, or no config, leaves them off.
    pub(crate) fn from_config(config: Option<&toml::Value>) -> Self {
        let on = config
            .and_then(|config| config.get("land")?.get(AUTO_RELEASE_KEY)?.as_bool())
            .unwrap_or(false);
        if on {
            Self::ReleaseAndReset
        } else {
            Self::ReportOnly
        }
    }

    pub(crate) fn load(root: &Path) -> Self {
        Self::from_config(super::run::project_config(root).as_ref())
    }
}

/// The text a breadcrumb for `lane` starts with, so a later pass can tell it
/// already recorded this lane's work.
// trace:TASK-3796 | ai:claude
pub(crate) fn breadcrumb_marker(lane: &DeadLane) -> String {
    format!(
        "aida land run: the lane in {} (lease {}) died with uncommitted work.",
        lane.worktree.display(),
        lane.lease
    )
}

/// A lease whose process is dead and whose worktree has uncommitted work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeadLane {
    pub lease: String,
    pub worktree: PathBuf,
    pub branch: String,
    /// The spec the lease is scoped to, when it names one.
    pub spec: Option<String>,
    pub agent: Option<String>,
    /// Set when the lane's worktree could not be read (its dispatch state is
    /// Unknown): nothing is salvaged or released, the lane is reported, and
    /// its spec is kept, never reset.
    pub unread: Option<String>,
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
    /// A commit on the integration branch that main does not have names the
    /// spec: its work is merged there. A promotion squashes, so those commits
    /// never reach main: this stays true after the promotion too.
    pub awaits_promotion: bool,
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
        /// Every id the spec answers to, for matching a dead lane's scope.
        ids: Vec<String>,
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
    // The scopes of lanes whose worktree could not be read: their specs are
    // kept, since what is in the worktree is not known.
    // trace:TASK-3796 | ai:claude
    let mut unread_scopes: Vec<(String, String)> = Vec::new();
    for lane in lanes {
        if let Some(why) = &lane.unread {
            let what = format!(
                "the lane {} in {}",
                lane.spec.as_deref().unwrap_or(&lane.branch),
                lane.worktree.display()
            );
            if let Some(spec) = &lane.spec {
                unread_scopes.push((spec.clone(), lane.worktree.display().to_string()));
            }
            steps.push(Step::Attention {
                what,
                why: format!(
                    "its worktree could not be read ({why}); nothing was salvaged or released and its spec is kept"
                ),
            });
        } else if landing_branch(&lane.branch) {
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
        let unread = unread_scopes.iter().find(|(scope, _)| {
            orphan
                .ids
                .iter()
                .chain(std::iter::once(&orphan.spec))
                .any(|id| id.eq_ignore_ascii_case(scope))
        });
        if let Some((_, worktree)) = unread {
            steps.push(Step::Attention {
                what,
                why: format!(
                    "In Progress with no session, but its lane's worktree {worktree} could not be read; not reset"
                ),
            });
        } else if orphan.awaits_promotion {
            steps.push(Step::Skip {
                what,
                why:
                    "integration carries its work (promoted by squash, or it waits for a promotion)"
                        .into(),
            });
        } else if let Some(holder) = orphan.elsewhere {
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
            steps.push(Step::Reset {
                spec: what,
                ids: orphan.ids,
                resume,
            });
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
    /// Whether this lane's work was already saved and recorded, by this run
    /// or by a breadcrumb starting with `marker` on its spec. Asked only when
    /// the lease is kept, so a lane is not saved again every pass.
    fn already_saved(&mut self, lane: &DeadLane, marker: &str) -> bool;
    /// Release the dead lease. The worktree and its branch stay.
    fn release(&mut self, lane: &DeadLane) -> Result<()>;
    /// Send the spec back to the implementer queue.
    fn reset(&mut self, spec: &str) -> Result<()>;
}

/// Carry out `steps`, or on a dry run say what would be done. Returns one
/// line per step. Under [`Unattended::ReportOnly`] no lease is released and
/// no spec is reset: `release` and `reset` are never called.
// trace:STORY-2768 trace:TASK-3796 | ai:claude
pub(crate) fn keep(
    steps: &[Step],
    keeper: &mut dyn Keeper,
    dry_run: bool,
    unattended: Unattended,
) -> Vec<String> {
    let releases = unattended == Unattended::ReleaseAndReset;
    const OFF: &str = "automatic release and reset are off ([land] housekeeping_auto_release)";
    let mut notes = Vec::new();
    // The scopes of dead lanes whose work this pass did not save and record.
    let mut unsaved_scopes: Vec<String> = Vec::new();
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
                let marker = breadcrumb_marker(lane);
                if dry_run {
                    notes.push(if releases {
                        format!(
                            "{what}: would save its uncommitted work to a patch and release its lease (dry run)"
                        )
                    } else {
                        format!(
                            "{what}: would save its uncommitted work to a patch and keep its lease, {OFF} (dry run)"
                        )
                    });
                    continue;
                }
                // The lease stays, so the lane is here again next pass: its
                // work is saved and recorded once, not every pass.
                if !releases && keeper.already_saved(lane, &marker) {
                    notes.push(format!(
                        "{what}: its work is already saved and recorded; the lease is kept, {OFF}"
                    ));
                    continue;
                }
                // Until the work is saved and its place recorded, the lane's
                // spec is not sent back to the queue by a later step.
                let mut unsaved = |notes: &mut Vec<String>, note: String| {
                    notes.push(note);
                    unsaved_scopes.extend(lane.spec.clone());
                };
                let patch = match keeper.salvage(lane) {
                    Ok(Some(patch)) if patch.is_file() => patch,
                    Ok(Some(patch)) => {
                        unsaved(
                            &mut notes,
                            format!(
                                "{what}: the patch {} is not there; the lease is kept",
                                patch.display()
                            ),
                        );
                        continue;
                    }
                    Ok(None) => {
                        notes.push(format!(
                            "{what}: nothing uncommitted is left to save; the lease is kept for the reaper"
                        ));
                        continue;
                    }
                    Err(error) => {
                        unsaved(
                            &mut notes,
                            format!(
                                "{what}: its work could not be saved, so the lease is kept: {error:#}"
                            ),
                        );
                        continue;
                    }
                };
                // The lease is what leads the next pickup to the saved work
                // until the spec says where it is: no record, no release.
                let Some(spec) = &lane.spec else {
                    notes.push(format!(
                        "{what}: saved to {}, but its lease names no spec to record that on; the lease is kept and a human releases it",
                        patch.display()
                    ));
                    continue;
                };
                let kept = if releases {
                    ""
                } else {
                    " Its lease was kept: automatic release is off."
                };
                if let Err(error) = keeper.note(
                    spec,
                    &format!(
                        "{marker} It is saved in {} (apply with `git apply`). The worktree and its branch `{}` were left as they are.{kept}",
                        patch.display(),
                        lane.branch
                    ),
                ) {
                    unsaved(
                        &mut notes,
                        format!(
                            "{what}: saved to {}, but that could not be recorded on {spec}, so the lease is kept: {error:#}",
                            patch.display()
                        ),
                    );
                    continue;
                }
                if !releases {
                    notes.push(format!(
                        "{what}: saved to {} (recorded on {spec}); the lease is kept and the worktree left alone, {OFF}",
                        patch.display()
                    ));
                    continue;
                }
                match keeper.release(lane) {
                    Ok(()) => notes.push(format!(
                        "{what}: saved to {} (recorded on {spec}); lease released, worktree left alone",
                        patch.display()
                    )),
                    Err(error) => notes.push(format!(
                        "{what}: saved to {} (recorded on {spec}); the lease was not released: {error:#}",
                        patch.display()
                    )),
                }
            }
            Step::Reset { spec, ids, resume } => {
                // Its dead lane's work is not saved (or not findable): a
                // reset now would start it fresh over that work.
                if let Some(scope) = unsaved_scopes.iter().find(|scope| {
                    std::iter::once(spec)
                        .chain(ids)
                        .any(|id| id.eq_ignore_ascii_case(scope))
                }) {
                    notes.push(format!(
                        "{spec}: not reset, because the work of its dead lane ({scope}) was not saved and recorded in this pass"
                    ));
                    continue;
                }
                let how = match resume {
                    Some(branch) => format!("its branch `{branch}` on origin has no open change, so the next pickup continues it"),
                    None => "no branch of it is on origin".into(),
                };
                if !releases {
                    let dry = if dry_run { " (dry run)" } else { "" };
                    notes.push(format!(
                        "{spec}: In Progress with no session ({how}); not sent back to the queue, {OFF}{dry}"
                    ));
                    continue;
                }
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

/// Whether one of `comments`, or a reply to one, starts with `marker`.
// trace:TASK-3796 | ai:claude
pub(crate) fn recorded(comments: &[aida_core::models::Comment], marker: &str) -> bool {
    comments
        .iter()
        .any(|comment| comment.content.starts_with(marker) || recorded(&comment.replies, marker))
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
    let (rows, orphans) = crate::gather_running_work(root);
    let lanes = lanes_of(&rows);
    if orphans.is_empty() {
        return Ok((lanes, Vec::new()));
    }
    let store = crate::integration_cmd::load_store(root)?;
    let host = crate::coordination::hostname();
    let leases = crate::leases::list_leases(root);
    // Asked of origin now; a read that fails stops the step, so a missing
    // answer is never read as "no branch" or "nothing merged".
    let origin = origin_evidence(root)?;
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
            let elsewhere = held_elsewhere(&ids, &leases, foreign, &host);
            let maybe_worked = worked_by(
                orphan.likely_fanout,
                orphan.possibly_subagent,
                orphan.driver.as_ref(),
            );
            Orphan {
                awaits_promotion: named_in(&ids, &origin.unpromoted),
                branches: spec_branches(&ids, &origin.branches),
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

/// The lanes of `rows` the step acts on or reports: a dead lease with
/// uncommitted work, and a lease that is not live whose worktree could not
/// be read (Unknown). The second is never salvaged or released; it keeps its
/// spec from being reset.
// trace:STORY-2768 trace:TASK-3796 | ai:claude
pub(crate) fn lanes_of(rows: &[crate::ps_integrate::PsRow]) -> Vec<DeadLane> {
    use crate::dispatch_health_ps::DispatchState;
    rows.iter()
        .filter_map(|row| {
            let dispatch = row.dispatch.as_ref()?;
            let unread = match dispatch.state {
                DispatchState::Salvageable => None,
                DispatchState::Unknown if row.state != aida_core::liveness::LeaseState::Live => {
                    Some(dispatch.hint.clone().unwrap_or_else(|| {
                        "its process and worktree state are unknown".to_string()
                    }))
                }
                _ => return None,
            };
            Some(DeadLane {
                lease: row.lease.id.clone(),
                worktree: row.lease.worktree_path.clone(),
                branch: row.lease.branch.clone(),
                spec: row.spec.clone(),
                agent: row.role.clone().or_else(|| row.lease_role.clone()),
                unread,
            })
        })
        .collect()
}

/// The host that holds a spec with these `ids`, when it is not `host`: the
/// newest lease scoped to the spec says where it was last worked, and a live
/// claim another host took on the spec says it still is. A claim's `scope`
/// is the spec; its `node_id` is only the clone that holds it.
// trace:STORY-2768 | ai:claude
pub(crate) fn held_elsewhere(
    ids: &[String],
    leases: &[crate::leases::SessionLease],
    foreign: &[crate::coordination::Claim],
    host: &str,
) -> Option<String> {
    let names = |text: &str| ids.iter().any(|id| id.eq_ignore_ascii_case(text.trim()));
    leases
        .iter()
        .filter(|lease| names(&lease.scope))
        .max_by_key(|lease| lease.started_at)
        .filter(|lease| lease.hostname != host)
        .map(|lease| lease.hostname.clone())
        .or_else(|| {
            foreign
                .iter()
                .find(|claim| claim.host != host && names(&claim.scope))
                .map(|claim| claim.host.clone())
        })
}

/// What origin holds, read from origin in this pass.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct OriginEvidence {
    /// Every branch on origin.
    pub branches: Vec<String>,
    /// The subject of every commit the integration branch has and main does
    /// not. Empty when origin has no integration branch.
    pub unpromoted: Vec<String>,
}

/// Where this step keeps the two branch tips it compares. Outside the
/// landing pass's own ref space, which that pass prunes.
const KEEP_REF_PREFIX: &str = "refs/aida/keep/";

/// Ask origin for its branches and for what integration carries that main
/// does not. Every read is bounded, and any that fails is an error: the
/// caller then resets nothing. The history is read whole, with no cap.
// trace:STORY-2768 | ai:claude
pub(crate) fn origin_evidence(root: &Path) -> Result<OriginEvidence> {
    use anyhow::Context as _;
    let listing = super::origin_git(root, &["ls-remote", "--heads", "--", "origin"], 60)
        .context("origin's branches could not be listed")?;
    let mut branches = Vec::new();
    for line in listing.lines() {
        let name = line
            .split_once('\t')
            .and_then(|(_, name)| name.strip_prefix("refs/heads/"))
            .with_context(|| {
                format!("origin's branch listing has a line that is not a branch: {line}")
            })?;
        branches.push(name.to_string());
    }
    let main = super::MAIN_BRANCH;
    let integration = super::INTEGRATION_BRANCH;
    let has = |name: &str| branches.iter().any(|branch| branch == name);
    if !has(integration) {
        // Origin answered, and it has no integration branch: nothing is
        // merged and waiting.
        return Ok(OriginEvidence {
            branches,
            unpromoted: Vec::new(),
        });
    }
    anyhow::ensure!(
        has(main),
        "origin has `{integration}` but no `{main}` to compare it with"
    );
    let keep = |name: &str| format!("{KEEP_REF_PREFIX}{name}");
    super::origin_git(
        root,
        &[
            "fetch",
            "--no-write-fetch-head",
            "--no-tags",
            "--",
            "origin",
            &format!("+refs/heads/{main}:{}", keep(main)),
            &format!("+refs/heads/{integration}:{}", keep(integration)),
        ],
        120,
    )
    .with_context(|| format!("origin's `{main}` and `{integration}` could not be fetched"))?;
    let range = format!("{}..{}", keep(main), keep(integration));
    let subjects = super::git_text(root, &["log", "--format=%s", &range]).with_context(|| {
        format!("what `{integration}` carries beyond `{main}` could not be read")
    })?;
    Ok(OriginEvidence {
        branches,
        unpromoted: subjects.lines().map(String::from).collect(),
    })
}

/// Whether one of `subjects` names one of `ids` as a whole token (`BUG-25`
/// is not named by `BUG-255`).
// trace:STORY-2768 | ai:claude
fn named_in(ids: &[String], subjects: &[String]) -> bool {
    subjects.iter().any(|subject| {
        subject
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .any(|token| ids.iter().any(|id| id.eq_ignore_ascii_case(token)))
    })
}

/// Why an In-Progress spec with no live session of its own may still be
/// worked, so it is left alone: `None` for a true orphan. Only a live relay,
/// drain or agent is a driver; an idle or merely queued spec has none and is
/// exactly what this step resets.
// trace:STORY-2768 | ai:claude
fn worked_by(
    likely_fanout: bool,
    possibly_subagent: bool,
    driver: Option<&aida_core::driver_registry::DerivedDriver>,
) -> Option<String> {
    if likely_fanout {
        Some("a live fan-out names it".to_string())
    } else if possibly_subagent {
        Some("a subagent may be working in its worktree".to_string())
    } else {
        driver
            .filter(|driver| driver.is_driven())
            .map(|driver| format!("{} is working it", driver.label()))
    }
}

#[cfg(test)]
#[path = "tests/story_2768_land_housekeeping_tests.rs"]
mod tests;
