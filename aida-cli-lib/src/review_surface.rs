// Review surface resolution and human-review eligibility.
// trace:TASK-3428 | ai:codex
use crate::pr_lookup::{change_lookup_for_branch, change_lookup_for_spec};
use crate::trace::GitLinkage;

/// trace:STORY-67 | ai:claude
/// The review surface a held spec is sitting on — what `aida review <SPEC>`
/// resolves the spec to before reviewing. NEVER asserts a closed/absent PR
/// (the TASK-715 / BUG-493 failure mode); each variant is a verified state.
/// trace:STORY-553 | ai:claude
#[derive(Debug)]
pub(crate) enum ReviewSurface {
    /// An OPEN change (PR/MR) exists for the spec's branch.
    OpenChange {
        branch: String,
        number: u64,
        url: String,
    },
    /// Work is on a feature branch with commits, but no open change.
    /// (Either never pushed a PR, or the held draft PR was closed.)
    BranchNoChange { branch: String, commits: usize },
    /// Commits reference the spec but already landed on the default branch.
    Shipped { number: Option<u64> },
    /// No commits and no trace comments reference the spec — built locally
    /// and never pushed, or not started.
    Local,
    /// BUG-1993: a bounded linkage scan timed out before it could tell where
    /// the work sits, so it may well be on a pushed branch. Never treated as
    /// `Local`: callers keep the spec visible with an "unknown" note.
    /// `reason` names the scan (`commit scan timed out`, ...).
    // trace:BUG-1993 | ai:claude
    Unknown { reason: &'static str },
}

/// BUG-1993: the `aida human` surface label for [`ReviewSurface::Unknown`].
// trace:BUG-1993 | ai:claude
pub(crate) const REVIEW_SURFACE_UNKNOWN_LABEL: &str = "unknown (scan timed out)";

/// Classify a spec's review surface from its git linkage + (optional) forge
/// change-lookup. Pure: the side-effecting `collect_git_linkage` /
/// `change_lookup_for_branch` run at the call site; this is the decision so
/// it's unit-testable. Order matters — shipped wins over branch, and only a
/// `ChangeLookup::Found` becomes `OpenChange` (a closed/absent/unreachable
/// PR must NOT be asserted as open — the TASK-715 / BUG-493 failure mode).
/// trace:STORY-553 trace:BUG-493 | ai:claude
pub(crate) fn classify_review_surface(
    linkage: &GitLinkage,
    change: Option<crate::forge::ChangeLookup>,
) -> ReviewSurface {
    classify_review_surface_forge_first(linkage, None, change)
}

/// BUG-876: resolve review surfaces forge-first. A lease/local branch is only
/// an accelerator; an open PR whose head branch or commit trailers reference
/// the spec remains the review surface after the lease is released.
// trace:BUG-876 | ai:codex
pub(crate) fn classify_review_surface_forge_first(
    linkage: &GitLinkage,
    spec_change: Option<crate::forge::ChangeLookup>,
    branch_change: Option<crate::forge::ChangeLookup>,
) -> ReviewSurface {
    if linkage.shipped {
        return ReviewSurface::Shipped {
            number: linkage.shipped_pr,
        };
    }

    if let Some(crate::forge::ChangeLookup::Found(c)) = spec_change {
        let branch = if c.branch.is_empty() {
            linkage
                .branch
                .clone()
                .unwrap_or_else(|| "unknown".to_string())
        } else {
            c.branch
        };
        return ReviewSurface::OpenChange {
            branch,
            number: c.id,
            url: c.url,
        };
    }

    match (linkage.branch.clone(), branch_change) {
        (Some(branch), Some(crate::forge::ChangeLookup::Found(c))) => ReviewSurface::OpenChange {
            branch,
            number: c.id,
            url: c.url,
        },
        (Some(branch), _) => ReviewSurface::BranchNoChange {
            branch,
            commits: linkage.commits.len(),
        },
        // BUG-1993: no branch because a scan timed out is unknown, not
        // "never pushed". trace:BUG-1993 | ai:claude
        (None, _) => match linkage.unknown_reason() {
            Some(reason) => ReviewSurface::Unknown { reason },
            None => ReviewSurface::Local,
        },
    }
}

/// BUG-881: shared forge-first review-surface resolver for human review and
/// PR-only drains. A lease/local branch is only an accelerator; a standalone
/// `queue work --from-pr` must see the same open PR that `aida review` sees
/// when the PR is discoverable by spec trailer or spec-named head branch.
// trace:BUG-881 | ai:codex
pub(crate) fn resolve_review_surface_forge_first(
    project_root: &std::path::Path,
    spec_id: &str,
    linkage: &GitLinkage,
) -> ReviewSurface {
    let spec_change = change_lookup_for_spec(project_root, spec_id);
    let branch_change = linkage
        .branch
        .as_deref()
        .map(|b| change_lookup_for_branch(project_root, b));
    classify_review_surface_forge_first(linkage, Some(spec_change), branch_change)
}

/// BUG-582: the robust invariant guarding the `aida human` reviews-awaiting
/// bucket — a finished spec can NEVER be resurrected onto the operator's seat
/// by a lingering local branch / stale review surface.
///
/// Two stale-data leaks made BUG-581 (already `Completed`, PR-1048 already
/// MERGED) surface as "needs review": (1) a spec whose status is terminal must
/// never be a review candidate regardless of any branch the Agent-tool
/// worktrees left lying around (`worktree-agent-*` branches accumulate), and
/// (2) only a genuinely-open surface (`OpenChange` / `BranchNoChange`) is a
/// review; a merged (`Shipped`) or never-pushed (`Local`) surface is not.
///
/// Centralizing both checks in one pure predicate keeps the guarantee from
/// drifting: the candidate pre-filter in `reviews_awaiting_human` is a cost
/// bound (it reads the cache, which can be stale), while THIS is the
/// load-bearing invariant — even a stale cache row that slipped through the
/// pre-filter is dropped here. Pure ⇒ unit-testable without a repo/forge.
/// trace:BUG-582 | ai:claude
pub(crate) fn spec_eligible_for_review_awaiting(
    status: aida_core::RequirementStatus,
    surface: &ReviewSurface,
) -> bool {
    // A finished (merged) or abandoned spec is never review work — a lingering
    // local branch must not put it back on the operator's seat.
    if matches!(
        status,
        aida_core::RequirementStatus::Completed | aida_core::RequirementStatus::Rejected
    ) {
        return false;
    }
    // Only a genuinely-open review surface counts. `Shipped` = the PR already
    // merged (the merged-PR signal); `Local` = never pushed. Neither is a
    // review awaiting a human. The LEGITIMATE case — Done-on-a-branch with an
    // OPEN PR not yet merged — is `OpenChange` and stays included.
    // BUG-1993: an `Unknown` surface (scan timed out) may be a real pushed
    // branch, so it stays on the seat with an "unknown" note rather than
    // silently dropping. trace:BUG-1993 | ai:claude
    matches!(
        surface,
        ReviewSurface::OpenChange { .. }
            | ReviewSurface::BranchNoChange { .. }
            | ReviewSurface::Unknown { .. }
    )
}

/// BUG-722: the requirements-store orphan branch is never a code-review target.
/// Commits that reference a spec but landed on `aida-store` during a store
/// reconcile must not register as "a branch awaiting review" — the STORY-760
/// false positive (`[branch aida-store]`). The store branch name is the
/// project-wide constant the rest of the CLI hard-codes (see the
/// `branch_exists_anywhere(.., "aida-store")` probe). Any configured store/
/// mirror ref would also belong here, but a mirror is a REMOTE, not a branch —
/// the branch name is uniformly `aida-store`.
// trace:BUG-722 | ai:claude
pub(crate) fn is_store_branch(branch: &str) -> bool {
    branch == "aida-store"
}

/// BUG-722: the branch a review surface sits on, when it has one. `Shipped`
/// (merged) and `Local` (never pushed) carry no live branch. Lets the
/// classifier apply the store-branch exclusion without re-plumbing git.
// trace:BUG-722 | ai:claude
pub(crate) fn review_surface_branch(surface: &ReviewSurface) -> Option<&str> {
    match surface {
        ReviewSurface::OpenChange { branch, .. } | ReviewSurface::BranchNoChange { branch, .. } => {
            Some(branch.as_str())
        }
        ReviewSurface::Shipped { .. } | ReviewSurface::Local | ReviewSurface::Unknown { .. } => {
            None
        }
    }
}

/// BUG-722: which `aida human` bucket a candidate spec lands in once its
/// status, view-state, and review surface are known. See
/// [`classify_human_review_bucket`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HumanReviewBucket {
    /// A genuine code-review gate — an open PR, or a pushed branch on active/
    /// Done work — the same surface `aida review <SPEC>` locates.
    ReviewsAwaiting,
    /// Loose work-in-progress: a `Draft` spec on a pushed branch with commits
    /// but no open PR. Visible under `wip-branches`, but NOT a review gate.
    WipBranch,
    /// Not the human's review work at all (finished/abandoned/deferred/archived
    /// spec, the store branch, or a merged/never-pushed surface).
    Excluded,
}

/// BUG-722: classify what `aida human` bucket a candidate spec belongs to.
/// Pure ⇒ every false-positive class is unit-testable without a repo/forge.
/// Layers three gates on top of the BUG-582 status/surface invariant, in
/// precedence order:
///  1. **view-state** — a deferred or archived spec is hidden from the human's
///     review seat exactly as `aida list` hides it from the default view
///     (STORY-441 archived, STORY-584 deferred);
///  2. **status/surface** — the BUG-582 invariant: a Completed/Rejected spec or
///     a merged (`Shipped`) / never-pushed (`Local`) surface is never review
///     work, even with a lingering local branch;
///  3. **branch-type** — the `aida-store` requirements-store orphan branch is
///     never a code-review target (the STORY-760 false positive);
/// then splits the survivors: a `Draft` spec whose only surface is a pushed
/// branch with no PR is loose WIP (`wip-branches`), not a review gate — nothing
/// awaits a human until there's a PR or a Done claim (operator decision,
/// 2026-07-12). Everything else is a genuine review.
// trace:BUG-722 | ai:claude
pub(crate) fn classify_human_review_bucket(
    status: aida_core::RequirementStatus,
    archived: bool,
    deferred: bool,
    surface: &ReviewSurface,
) -> HumanReviewBucket {
    // Gate 1 — view-state: a filed-away or primed-conditional spec is not the
    // human's review work, even with a live branch (mirrors `aida list`).
    if archived || deferred {
        return HumanReviewBucket::Excluded;
    }
    // Gate 2 — the BUG-582 status/surface invariant.
    if !spec_eligible_for_review_awaiting(status.clone(), surface) {
        return HumanReviewBucket::Excluded;
    }
    // Gate 3 — branch-type: the requirements-store orphan branch is never a
    // code-review target.
    if review_surface_branch(surface).is_some_and(is_store_branch) {
        return HumanReviewBucket::Excluded;
    }
    // Draft + pushed branch + no open PR ⇒ loose WIP, not a review gate.
    // BUG-1993: a Draft whose surface is unknown (scan timed out) is listed
    // as WIP too — visible, but not asserted as a review gate.
    // trace:BUG-1993 | ai:claude
    if matches!(status, aida_core::RequirementStatus::Draft)
        && matches!(
            surface,
            ReviewSurface::BranchNoChange { .. } | ReviewSurface::Unknown { .. }
        )
    {
        return HumanReviewBucket::WipBranch;
    }
    HumanReviewBucket::ReviewsAwaiting
}

/// BUG-722: mirror the deferred view-state predicate `aida list` applies — the
/// `deferred` flag OR any legacy `deferred:*` parking tag (STORY-584's
/// honor-both rule). Keeps the `aida human` review seat hidden for exactly the
/// specs the default list hides.
// trace:BUG-722 | ai:claude
pub(crate) fn requirement_is_deferred(req: &aida_core::Requirement) -> bool {
    req.deferred || req.tags.iter().any(|t| t.starts_with("deferred:"))
}
