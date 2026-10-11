//! BUG-1944: does a spec still have live work behind it, and the targeted
//! Approved → In Progress bump every start-of-work path shares.
//!
//! An In Progress spec whose lease is gone is not necessarily abandoned: its
//! branch may still hold commits that have not reached the default branch, or
//! an open PR may be waiting on review. Moving such a spec back to Approved
//! re-offers shipped-but-unmerged work to the queue. This module gathers that
//! evidence per spec branch and decides, failing safe: when the forge cannot
//! answer, or git cannot classify a branch, the spec keeps its status.
// trace:BUG-1944 | ai:claude

use aida_core::DatabaseBackend;
use std::path::Path;

/// What one branch carrying the spec's id says about live work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BranchEvidence {
    /// The forge reports an open PR for the branch.
    OpenPr(u64),
    /// The branch carries this many commits whose content is not on the
    /// default branch.
    Unmerged(u32),
    /// The branch could not be classified (git failed or timed out, or the
    /// forge was unreachable). The string says why.
    Unknown(String),
    /// Fully merged (or empty) and no open PR.
    Idle,
}

/// Whether an automated writer may move the spec back to Approved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LiveWorkVerdict {
    /// Keep the spec In Progress; the string is the user-facing reason.
    Keep(String),
    /// No branch shows live work, so a revert is allowed.
    NoLiveWork,
}

/// Pure decision over per-branch evidence. Any open PR, unmerged commit, or
/// unclassifiable branch keeps the spec; only an all-idle (or empty) set lets
/// a revert through.
pub(crate) fn decide(evidence: &[(String, BranchEvidence)]) -> LiveWorkVerdict {
    for (branch, ev) in evidence {
        match ev {
            BranchEvidence::OpenPr(n) => {
                return LiveWorkVerdict::Keep(format!("branch `{branch}` has open PR #{n}"));
            }
            BranchEvidence::Unmerged(n) => {
                return LiveWorkVerdict::Keep(format!(
                    "branch `{branch}` has {n} unmerged commit{}",
                    if *n == 1 { "" } else { "s" }
                ));
            }
            BranchEvidence::Idle | BranchEvidence::Unknown(_) => {}
        }
    }
    if let Some((branch, BranchEvidence::Unknown(why))) = evidence
        .iter()
        .find(|(_, ev)| matches!(ev, BranchEvidence::Unknown(_)))
    {
        return LiveWorkVerdict::Keep(format!(
            "could not check branch `{branch}` ({why}); keeping it to be safe"
        ));
    }
    LiveWorkVerdict::NoLiveWork
}

/// Local and `origin/` refs whose branch name carries `spec_id`, grouped by
/// branch name: `(branch, refs)`. A project root that is not a git checkout
/// has no branches (`Ok(empty)`); a checkout whose refs git cannot list is an
/// `Err`, which callers must treat as "unknown", never as "no branches".
pub(crate) fn spec_branch_refs(
    project_root: &Path,
    spec_id: &str,
) -> Result<Vec<(String, Vec<String>)>, String> {
    if !project_root.join(".git").exists() {
        return Ok(Vec::new());
    }
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes/origin",
        ])
        .output()
        .map_err(|e| format!("git could not run: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git could not list branches: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
    for full in String::from_utf8_lossy(&out.stdout).lines() {
        let (branch, short_ref) = if let Some(b) = full.strip_prefix("refs/heads/") {
            (b.to_string(), b.to_string())
        } else if let Some(b) = full.strip_prefix("refs/remotes/origin/") {
            if b == "HEAD" {
                continue;
            }
            (b.to_string(), format!("origin/{b}"))
        } else {
            continue;
        };
        let matches = crate::worktree_lease::spec_id_from_branch(&branch)
            .is_some_and(|id| id.eq_ignore_ascii_case(spec_id));
        if !matches {
            continue;
        }
        match grouped.iter_mut().find(|(b, _)| *b == branch) {
            Some((_, refs)) => refs.push(short_ref),
            None => grouped.push((branch, vec![short_ref])),
        }
    }
    Ok(grouped)
}

/// Per-ref ceiling for the squash-aware unmerged-commit probe, so one
/// anciently-forked branch cannot stall the caller.
const PROBE_SLICE: std::time::Duration = std::time::Duration::from_secs(10);

/// Classify one branch: local git first (unmerged commits need no forge),
/// then the forge for an open PR.
fn branch_evidence(
    project_root: &Path,
    default_ref: Option<&str>,
    branch: &str,
    refs: &[String],
    open_pr: &dyn Fn(&str) -> crate::PrLookup,
) -> BranchEvidence {
    let Some(default_ref) = default_ref else {
        return BranchEvidence::Unknown("no default branch to compare against".to_string());
    };
    let mut unknown = None;
    for r in refs {
        match crate::branch_unshipped_patch_count_probe(
            project_root,
            default_ref,
            r,
            Some(PROBE_SLICE),
        ) {
            crate::PatchCountProbe::Counted(0) => {}
            crate::PatchCountProbe::Counted(n) => return BranchEvidence::Unmerged(n),
            crate::PatchCountProbe::NoSignal => {
                unknown = Some(format!("git could not compare `{r}`"));
            }
            crate::PatchCountProbe::TimedOut => {
                unknown = Some(format!("comparing `{r}` timed out"));
            }
        }
    }
    match open_pr(branch) {
        crate::PrLookup::Found(pr) => BranchEvidence::OpenPr(pr.number),
        crate::PrLookup::NoOpenPr => match unknown {
            Some(why) => BranchEvidence::Unknown(why),
            None => BranchEvidence::Idle,
        },
        crate::PrLookup::GhMissing => {
            BranchEvidence::Unknown("the forge CLI is not installed".to_string())
        }
        crate::PrLookup::GhFailed(e) | crate::PrLookup::GhUnreachable(e) => {
            BranchEvidence::Unknown(format!("the forge did not answer: {e}"))
        }
    }
}

/// [`spec_live_work`] with an injectable PR lookup (tests).
pub(crate) fn spec_live_work_with(
    project_root: &Path,
    spec_id: &str,
    open_pr: &dyn Fn(&str) -> crate::PrLookup,
) -> LiveWorkVerdict {
    let branches = match spec_branch_refs(project_root, spec_id) {
        Ok(branches) => branches,
        Err(why) => return LiveWorkVerdict::Keep(format!("{why}; keeping it to be safe")),
    };
    if branches.is_empty() {
        return LiveWorkVerdict::NoLiveWork;
    }
    let default_ref = crate::resolve_default_branch_ref(project_root);
    let evidence: Vec<(String, BranchEvidence)> = branches
        .iter()
        .map(|(branch, refs)| {
            let ev = branch_evidence(project_root, default_ref.as_deref(), branch, refs, open_pr);
            (branch.clone(), ev)
        })
        .collect();
    decide(&evidence)
}

/// Does `spec_id` still have live work (an open PR, or commits not yet on the
/// default branch) on any branch carrying its id?
pub(crate) fn spec_live_work(project_root: &Path, spec_id: &str) -> LiveWorkVerdict {
    let started = std::time::Instant::now();
    spec_live_work_with(project_root, spec_id, &|branch| {
        bounded_pr_lookup(started, FORGE_BUDGET, || {
            crate::detect_open_pr_for_branch(project_root, branch)
        })
    })
}

/// Total forge time one live-work check may spend across all spec branches.
/// Each lookup is also capped at [`crate::FORGE_CLI_CALL_TIMEOUT`].
const FORGE_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

/// One forge lookup under the scoped per-call ceiling: a lookup killed at the
/// ceiling comes back `GhUnreachable`, and once `budget` (measured from
/// `started`) is spent no further call is made. Both read as "unknown", so
/// the spec is kept, and a slow forge cannot stall the caller (the doctor
/// holds the loaded store while it heals).
pub(crate) fn bounded_pr_lookup(
    started: std::time::Instant,
    budget: std::time::Duration,
    lookup: impl FnOnce() -> crate::PrLookup,
) -> crate::PrLookup {
    if started.elapsed() >= budget {
        return crate::PrLookup::GhUnreachable("forge lookup budget spent".to_string());
    }
    crate::with_forge_lookup_timeout(crate::FORGE_CLI_CALL_TIMEOUT, lookup)
}

/// [`crate::try_auto_queue_pr_review_unbumped`], then — when the PR is open
/// (a review story was filed or already exists) — move the specs it carries
/// (branch spec and covered specs) Approved → In Progress. Every PR-open
/// path goes through here: `aida pr auto-queue-review`, the session-end
/// backup, and the orphan sweep.
pub(crate) fn try_auto_queue_pr_review(
    project_root: &Path,
    branch: &str,
    session_id: &str,
    origin: crate::AutoQueueOrigin,
) -> crate::AutoQueueOutcome {
    let outcome =
        crate::try_auto_queue_pr_review_unbumped(project_root, branch, session_id, origin);
    if matches!(
        outcome.status,
        crate::AutoQueueStatus::Filed | crate::AutoQueueStatus::AlreadyExists
    ) {
        bump_pr_specs(project_root, branch, &outcome.covered_specs);
    }
    outcome
}

/// Bump every spec an open PR carries. The store lives in the main checkout,
/// so a worktree root falls back to it.
pub(crate) fn bump_pr_specs(project_root: &Path, branch: &str, covered_specs: &[String]) {
    let store_owner = if project_root.join(".aida-store").is_dir() {
        project_root.to_path_buf()
    } else {
        match crate::find_main_worktree_root() {
            Ok(root) => root,
            Err(_) => return,
        }
    };
    for spec in crate::pr_cmd::pr_open_bump_targets(None, branch, covered_specs) {
        bump_spec_in_progress(&store_owner, &spec, aida_core::conflict::PR_OPEN_AUTHOR);
    }
}

/// Targeted Approved → In Progress bump for one spec, through the
/// cache-backed single-spec write. Idempotent: any other status is left
/// alone. Best-effort: a failure is reported and returns false.
pub(crate) fn bump_spec_in_progress(project_root: &Path, spec_id: &str, author: &str) -> bool {
    let store_root = project_root.join(".aida-store");
    if !store_root.is_dir() {
        return false;
    }
    let result = (|| -> anyhow::Result<bool> {
        let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_root);
        let backend = aida_core::CachedGitBackend::open(&store_root, &cache_path)?;
        let Some(req) = backend.get_requirement_unambiguous(spec_id)? else {
            return Ok(false);
        };
        // The status check runs on the copy read under the store lock, so a
        // concurrent writer cannot be overwritten (BUG-1637).
        crate::status_write_race_seam(&store_root);
        let mut bumped = false;
        backend.update_spec_atomically(&req, |r| {
            bumped = crate::approved_to_in_progress_bump(r, author);
        })?;
        Ok(bumped)
    })();
    match result {
        Ok(bumped) => bumped,
        Err(e) => {
            eprintln!(
                "warning: couldn't move `{spec_id}` to In Progress: {e} — `aida doctor` will heal"
            );
            false
        }
    }
}

#[cfg(test)]
#[path = "tests/live_work_tests.rs"]
mod tests;
