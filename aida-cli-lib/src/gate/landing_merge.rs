//! The one production merge boundary (STORY-2897 S1c). Every AIDA merge
//! path (`aida pr ship`, the drain's merge phase, the burndown wave, `aida
//! release --after-pr`) calls [`merge`] instead of `Forge::merge_change`, so
//! a new caller cannot skip the landing checks: a source-scan test fails on
//! any other production `merge_change` / `merge_change_guarded` call.
//!
//! Immediately before the forge merge, [`merge`] re-reads the landing policy
//! from the freshly fetched default branch and refuses if it changed since
//! the caller started, re-reads the operator's floor authority, and applies
//! the merger and author rules the policy implies, before every forge
//! transport attempt, for every caller. The live train merges through its
//! verified per-car context (STORY-2905), which re-judges each car too.
// trace:STORY-2903 trace:STORY-2897 | ai:claude
// mandate-gate

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

impl Caller {
    /// The merge hand this caller claims a PR as.
    // trace:STORY-2905 | ai:claude
    pub(crate) fn hand(self) -> crate::gate::merge_claim::Hand {
        use crate::gate::merge_claim::Hand;
        match self {
            Self::Ship => Hand::Ship,
            Self::Drain => Hand::Drain,
            Self::Burndown => Hand::Burndown,
            Self::Release => Hand::Release,
            Self::Train => Hand::Train,
        }
    }
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

/// Merge `change` through `forge`, with the landing re-checks for `caller`
/// (see the module docs) run immediately before EVERY forge transport
/// attempt, retries included, for every caller (promotion 23 review F1):
/// authority, policy or base that changed while a retry waited refuses the
/// next attempt. `spec_ids` are the specs whose authors the author rule
/// excludes. The live train merges through its verified per-car context
/// (`Forge::merge_train_change`); a train reaching this boundary is
/// rechecked like any caller, and holds no claim here because its cars
/// were claimed when they boarded.
// trace:STORY-2903 trace:STORY-2905 | ai:claude
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
    train_pins: crate::gate::merge_claim::TrainPins<'_>,
) -> Result<MergeResult, MergeError> {
    // One merge hand per PR on this host (STORY-2905): the claim is held
    // through every recheck and forge attempt. trace:STORY-2905 | ai:claude
    let _claim = match caller {
        Caller::Train => None,
        _ => Some(
            crate::gate::merge_claim::claim(
                root,
                change.id,
                opts.match_head.as_deref(),
                caller.hand(),
                Some(train_pins),
            )
            .map_err(|why| MergeError::Refused(ShipRefusal::Review, why))?,
        ),
    };
    // A train's freshness is its stacked proof's (pinned in its verified
    // context), not its head branch: it is rechecked on its base alone,
    // which in review-gated mode (where no train lands) refuses it.
    let subject = match caller {
        Caller::Train => ChangeRef {
            branch: String::new(),
            ..change.clone()
        },
        _ => change.clone(),
    };
    let mut refusal: Option<(ShipRefusal, String)> = None;
    let merged = {
        let mut guard = || -> anyhow::Result<()> {
            recheck(
                root,
                &subject,
                &entry.policy,
                spec_ids,
                entry.floor,
                crate::gate::role_authority::has_integrity_floor_authority,
            )
            .map_err(|(kind, why)| {
                let error = anyhow::anyhow!("{why}");
                refusal = Some((kind, why));
                error
            })
        };
        forge.merge_change_guarded(change, opts, sink, &mut guard)
    };
    match (merged, refusal) {
        (Ok(result), _) => Ok(result),
        (Err(_), Some((kind, why))) => Err(MergeError::Refused(kind, why)),
        (Err(e), None) => Err(MergeError::Forge(e)),
    }
}

/// How a head behind its base is judged in review-gated mode.
// trace:STORY-2903 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BehindBase {
    /// Level with its base.
    Current,
    /// Behind, but the base's new commits touch none of the change's paths
    /// and the change touches no gate path: it merges as it is.
    Disjoint,
    /// The base's new commits touch these paths the change also touches.
    Overlap(Vec<String>),
    /// The change touches this gate/authority path (or the protected set
    /// could not be read: `None`), which needs full CI on the current base.
    GatePath(Option<String>),
}

/// PURE: the review-gated behind-base decision (advisor ruling 2026-11-18,
/// STORY-2903 (a)): the base-freshness overlap rule, and no stale merge of a
/// gate/authority path. `protected` is `Err` when the set is unreadable.
// trace:STORY-2903 | ai:claude
pub(crate) fn judge_behind_base(
    behind: u32,
    pr_files: &[String],
    base_files: &[String],
    protected: Result<&crate::gate::mandate_gates::ProtectedPaths, ()>,
) -> BehindBase {
    if behind == 0 {
        return BehindBase::Current;
    }
    let overlap: Vec<String> = base_files
        .iter()
        .filter(|f| pr_files.contains(f))
        .cloned()
        .collect();
    if !overlap.is_empty() {
        return BehindBase::Overlap(overlap);
    }
    match protected {
        Err(()) => BehindBase::GatePath(None),
        Ok(set) => match crate::gate::mandate_gates::touches_gates(pr_files, set) {
            Some(hit) => BehindBase::GatePath(Some(hit.path)),
            None => BehindBase::Disjoint,
        },
    }
}

/// The behind-base gate every merging path runs. In ci-gated mode it is the
/// existing gate (a head behind its base lands through the train). In
/// review-gated mode there is no train: a behind-base head merges as it is
/// when the base's new commits touch none of its paths and it touches no
/// gate path; otherwise it is refused until the branch is updated (and the
/// update reviewed).
// trace:STORY-2903 | ai:claude
pub(crate) fn behind_base_gate(
    project_root: &std::path::Path,
    ship_branch: &str,
    pr_number: u64,
    override_stale_check: bool,
) -> Result<(), (ShipRefusal, String)> {
    let policy = crate::landing_policy::load(project_root);
    if policy.mode != crate::landing_policy::LandingMode::ReviewGated {
        return crate::gate::ci_freshness::ship_behind_base_gate(
            project_root,
            ship_branch,
            pr_number,
            override_stale_check,
        );
    }
    review_gated_behind_base(
        project_root,
        ship_branch,
        pr_number,
        &crate::gate::ci_freshness::target_branch(pr_number),
    )
}

/// The review-gated half of [`behind_base_gate`], against a known base.
// trace:STORY-2903 | ai:claude
pub(crate) fn review_gated_behind_base(
    project_root: &std::path::Path,
    ship_branch: &str,
    pr_number: u64,
    base: &str,
) -> Result<(), (ShipRefusal, String)> {
    let base = base.to_string();
    let unknown = |why: String| {
        (
            ShipRefusal::BehindBase,
            format!(
                "PR-{pr_number}: whether its head is behind {base} could not be computed ({why}); \
                 refusing rather than merging it"
            ),
        )
    };
    crate::gate::ci_freshness::fetch_refs(project_root, &base, ship_branch).map_err(unknown)?;
    let head = format!("refs/remotes/origin/{ship_branch}");
    let tip = format!("refs/remotes/origin/{base}");
    let git = |args: &[&str]| -> Result<Vec<u8>, String> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        Ok(out.stdout)
    };
    let text = |args: &[&str]| -> Result<String, (ShipRefusal, String)> {
        let out = git(args).map_err(unknown)?;
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    };
    let fork = text(&["merge-base", &head, &tip])?;
    let behind: u32 = text(&["rev-list", "--count", &format!("{fork}..{tip}")])?
        .parse()
        .map_err(|e: std::num::ParseIntError| unknown(e.to_string()))?;
    // Paths as NUL-delimited bytes (never core.quotePath's quoting), each
    // checked as UTF-8: a name that is not refuses rather than being
    // matched against the protected set in some escaped spelling.
    let paths = |args: &[&str]| -> Result<Vec<String>, (ShipRefusal, String)> {
        let out = git(args).map_err(unknown)?;
        let mut names = std::collections::BTreeSet::new();
        for raw in out.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let name = std::str::from_utf8(raw)
                .map_err(|_| unknown("a changed path is not valid UTF-8".to_string()))?;
            names.insert(name.to_string());
        }
        Ok(names.into_iter().collect())
    };
    // The change's paths: its own diff from the fork point.
    // `--no-renames`: a rename reports both its source and its destination,
    // so moving a protected file out of its prefix still touches it.
    let pr_files = paths(&["diff", "--no-renames", "--name-only", "-z", &fork, &head])?;
    // Every path any base commit since the fork touched, per commit, so an
    // edit later reverted still counts (STORY-2903 ruling S2).
    let range = format!("{fork}..{tip}");
    // `-m`: a merge commit lists what it changed too (over-reporting only
    // refuses more).
    let base_files = paths(&[
        "log",
        "-m",
        "--no-renames",
        "--name-only",
        "-z",
        "--format=",
        &range,
    ])?;
    let gates = crate::gate::mandate_gates::load_human_gates(project_root);
    let decision = judge_behind_base(
        behind,
        &pr_files,
        &base_files,
        gates.as_ref().map(|g| &g.protected).map_err(|_| ()),
    );
    let lead = format!("PR-{pr_number}'s head is {behind} commit(s) behind {base}");
    match decision {
        BehindBase::Current | BehindBase::Disjoint => Ok(()),
        BehindBase::Overlap(paths) => Err((
            ShipRefusal::BehindBase,
            format!(
                "{lead}, and those commits touch {} it also changes. Update the branch from \
                 {base}, then have the update reviewed (a delta review) before merging.",
                paths.join(", ")
            ),
        )),
        BehindBase::GatePath(path) => Err((
            ShipRefusal::BehindBase,
            format!(
                "{lead}, and it changes {}: a gate/authority change never merges behind its base. \
                 Update the branch from {base}, then have the update reviewed (a delta review).",
                path.map_or_else(
                    || "paths whose protection could not be read".to_string(),
                    |p| format!("the protected path {p}")
                )
            ),
        )),
    }
}

/// Whether this session may merge `change` under `policy`: the gate's
/// [`crate::gate::ship::merge_authority_refusal`] over this session's seat,
/// the operator's `floor` authority, and the change's authors. `aida land
/// run` asks this before it ships; the merge boundary asks it again at the
/// merge.
// trace:STORY-2903 | ai:claude
pub(crate) fn caller_may_merge(
    project_root: &std::path::Path,
    change: &ChangeRef,
    spec_ids: &[String],
    policy: &crate::landing_policy::LandingPolicy,
    floor: bool,
) -> Result<(), String> {
    match crate::gate::ship::merge_authority_refusal(
        policy.advisor_merges(),
        crate::gate::seat_authority::current_seat(project_root).as_deref(),
        floor,
        || merge_authors(project_root, change, spec_ids),
        &format!("PR-{}", change.id),
    ) {
        Some(why) => Err(why),
        None => Ok(()),
    }
}

/// The author sessions of `change` (the full authorship subject: its specs,
/// `(Refs X)` contributors and its head branch), from one checked read of
/// the authorship ledger: `Err` when it cannot be trusted (F2).
fn merge_authors(
    project_root: &std::path::Path,
    change: &ChangeRef,
    spec_ids: &[String],
) -> Result<crate::gate::ship::ShipApprovalPolicy, String> {
    let branch = Some(change.branch.as_str()).filter(|b| !b.trim().is_empty());
    let (author_ids, author_branch) =
        crate::gate::approval::ship_author_ids(project_root, change.id, spec_ids, branch);
    crate::gate::approval::merge_author_policy(project_root, &author_ids, author_branch.as_deref())
}

/// The merge authority of a train, judged immediately before each forge
/// merge attempt of the train change (STORY-2905 S1t-b): the boundary's
/// recheck of the train change under the landing snapshot taken at its
/// proof (fresh fetch, unchanged policy, floor lapse, the advisor-merger
/// rule), then the author rule for EACH car on its own specs, then the
/// stacked proof's base: the fetched base must still be the commit the
/// train was built and proved on. Any refusal stops the train before the
/// merge; a grant that lapsed since the train started fails here.
// trace:STORY-2905 | ai:claude
pub(crate) fn train_recheck(
    root: &std::path::Path,
    train: &ChangeRef,
    cars: &[(u64, Vec<String>)],
    entry: &Entry,
    base_ref: &str,
    base_tip: &str,
) -> Result<(), (ShipRefusal, String)> {
    recheck(
        root,
        train,
        &entry.policy,
        &[],
        entry.floor,
        crate::gate::role_authority::has_integrity_floor_authority,
    )?;
    // On the refs `recheck` just fetched, the policy is resolved
    // fail-closed: a config that cannot be read or
    // parsed is never taken as the ci-gated default (round 2 finding 2).
    let policy = crate::landing_policy::load_checked(root).map_err(|why| {
        (
            ShipRefusal::Review,
            format!("the train is not merged: its landing policy cannot be established ({why})"),
        )
    })?;
    let floor = crate::gate::role_authority::has_integrity_floor_authority();
    // Each car's author rule reads the authorship ledger through the one
    // checked read every C2 caller shares: a ledger that cannot be trusted
    // refuses the car (F2).
    for (pr, specs) in cars {
        let car = ChangeRef {
            id: *pr,
            url: String::new(),
            branch: String::new(),
            base: base_ref.to_string(),
            title: None,
        };
        caller_may_merge(root, &car, specs, &entry.policy, floor).map_err(|why| {
            (
                ShipRefusal::Review,
                format!("the train is not merged: car {why}"),
            )
        })?;
    }
    if has_origin(root) {
        let now = crate::trusted_config::rev_parse_commit(
            root,
            &format!("refs/remotes/origin/{base_ref}"),
        );
        if now.as_deref() != Some(base_tip) {
            return Err((
                ShipRefusal::BehindBase,
                format!(
                    "the train is not merged: {base_ref} moved since its stacked proof (proved on \
                     {}, now {}); rebuild the train on the current base",
                    &base_tip[..base_tip.len().min(12)],
                    now.as_deref()
                        .map_or("unreadable", |n| &n[..n.len().min(12)])
                ),
            ));
        }
    }
    Ok(())
}

/// The line the auto-train reports when it stands down (STORY-2905, advisor
/// call (c)).
pub(crate) const TRAIN_STAND_DOWN: &str = "standing down: landing.mode=review-gated";

/// The effective trusted landing mode, resolved now: `Ok(true)` when
/// review-gated. With an `origin`, the default branch is fetched first; a
/// fetch that fails, a default branch that cannot be resolved, or a config
/// that cannot be read or parsed is `Err`: a cached or defaulted policy may
/// be stale, so neither the auto-train nor `aida land run` acts on it
/// (promotion 23 review F4, F5). Both resolve it on EVERY pass, so a mode
/// change takes effect without a restart.
// trace:STORY-2903 trace:STORY-2905 | ai:claude
pub(crate) fn effective_review_gated(root: &std::path::Path) -> Result<bool, String> {
    if has_origin(root) {
        let default = crate::trusted_config::local_default_branch_name(root)
            .ok_or_else(|| "the default branch cannot be resolved".to_string())?;
        crate::gate::ci_freshness::fetch_refs(root, &default, &default)
            .map_err(|e| format!("the default branch `{default}` could not be refreshed ({e})"))?;
    }
    let policy = crate::landing_policy::load_checked(root)?;
    Ok(policy.mode == crate::landing_policy::LandingMode::ReviewGated)
}

/// Whether the auto-train may run a pass: `Ok(None)` it may (ci-gated);
/// `Ok(Some(line))` it stands down (review-gated, where the land loop is
/// the one merge hand); `Err(why)` the policy cannot be established
/// ([`effective_review_gated`]), and the pass does not run either (never a
/// ci-gated default under uncertainty). The caller then releases its
/// admission.
// trace:STORY-2905 | ai:claude
pub(crate) fn train_stand_down(root: &std::path::Path) -> Result<Option<&'static str>, String> {
    Ok(effective_review_gated(root)?.then_some(TRAIN_STAND_DOWN))
}

/// The stand-down line the status surfaces show, if any: the stand-down,
/// or why the auto-train is not running.
// trace:STORY-2905 | ai:claude
pub(crate) fn train_stand_down_line(root: &std::path::Path) -> Option<String> {
    match train_stand_down(root) {
        Ok(line) => line.map(str::to_string),
        Err(why) => Some(format!(
            "not running: the landing policy is unknown ({why})"
        )),
    }
}

/// The auto-train's one decision per pass (STORY-2905): run `handler` only
/// when `stand_down` says the policy allows it. Returns what the pass did,
/// for the service log and status.
// trace:STORY-2905 | ai:claude
pub(crate) fn auto_train_pass(
    stand_down: Result<Option<&'static str>, String>,
    handler: impl FnOnce() -> anyhow::Result<()>,
) -> Result<anyhow::Result<()>, String> {
    match stand_down {
        Ok(None) => Ok(handler()),
        Ok(Some(line)) => Err(line.to_string()),
        Err(why) => Err(format!(
            "not running: the landing policy is unknown ({why})"
        )),
    }
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
        crate::gate::ci_freshness::target_branch(change.id)
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
        crate::gate::ci_freshness::fetch_refs(project_root, &base, branch)
            .map_err(|e| (ShipRefusal::BehindBase, format!("PR-{}: {e}", change.id)))?;
    }
    let default = crate::trusted_config::local_default_branch_name(project_root)
        .unwrap_or_else(|| crate::forge::default_branch_of(project_root));
    if remote && default != base {
        crate::gate::ci_freshness::fetch_refs(project_root, &default, &default).map_err(|e| {
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
    // Base freshness, judged here against the refs just fetched, for every
    // caller (STORY-2903 ruling (a); finding F1): in review-gated mode a head
    // behind its base merges only when the base's commits since touch none
    // of its paths and it touches no protected path.
    if policy.mode == crate::landing_policy::LandingMode::ReviewGated {
        let branch = change.branch.trim();
        if branch.is_empty() {
            return Err((
                ShipRefusal::BehindBase,
                format!(
                    "{subject}: its head branch is unknown, so whether it is behind {base} cannot \
                     be judged; refusing rather than merging it"
                ),
            ));
        }
        review_gated_behind_base(project_root, branch, change.id, &base)?;
    }
    let floor = floor_now();
    if let Some(why) = crate::gate::ship::floor_lapse_refusal(floor_at_entry, floor, &subject) {
        return Err((ShipRefusal::Review, why));
    }
    if let Err(why) = caller_may_merge(project_root, change, spec_ids, &policy, floor) {
        return Err((ShipRefusal::Review, why));
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/story_2897_final_recheck_tests.rs"]
mod story_2897_final_recheck_tests;

#[cfg(test)]
#[path = "tests/story_2903_merge_boundary_tests.rs"]
mod story_2903_merge_boundary_tests;

#[cfg(test)]
#[path = "tests/story_2903_review_gated_base_tests.rs"]
mod story_2903_review_gated_base_tests;

#[cfg(test)]
#[path = "tests/story_2905_train_recheck_tests.rs"]
mod story_2905_train_recheck_tests;
