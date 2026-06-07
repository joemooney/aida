//! Pure decision logic for `aida queue recover <id>` — the failed-phase-1
//! recovery wizard (STORY-384).
//!
//! After a failed phase-1 implementer session (an Anthropic 529, a
//! commit-and-exit-without-PR, partial work, an external crash), recovery is a
//! mechanical sequence of git/gh/aida commands whose exact shape depends on the
//! spec's *current state* — does a lease still hold it? is there a PR? are there
//! commits ahead of `origin/main` that were never pushed? is the worktree dirty?
//! The advisor has walked an operator through this dance 5-6 times across two
//! days; the branch decisions are deterministic enough to encode.
//!
//! This module is **only the state → recommended-action decision, as a pure
//! side-effect-free function** over already-probed facts. The probing (querying
//! leases / git / PR reality) and the execution (push, PR-create, drive phases
//! 3-6 via the TASK-405 `--from-pr` path, end lease, pull) live in
//! `main.rs::handle_queue_recover` — deliberately not here. Keeping the decision
//! pure means it is exhaustively unit-testable independent of how the facts were
//! gathered, and it carries zero risk to the live recovery control flow. This
//! mirrors the `drain_resume` module's pure-decision / shell-probe split.
//!
//! trace:STORY-384 | ai:claude

/// The probed state of a spec under recovery. The caller gathers these from
/// session leases (`list_leases` + `classify_lease_state`), git (`git status
/// --porcelain`, `branch_commits_ahead_main`, `probe_branch_on_origin`), and
/// the forge (`detect_open_pr_for_spec_via_forge`, `pr_is_merged`). This struct
/// is the *whole* input to the decision — nothing else is consulted.
/// trace:STORY-384 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RecoverState {
    /// The spec is already `Completed` — the merge already promoted it. The
    /// most-final state; dominates every other signal.
    pub spec_completed: bool,
    /// A PR exists for this spec (open OR merged — `pr_merged` disambiguates).
    pub pr_exists: bool,
    /// The PR is merged. Only meaningful when `pr_exists`.
    pub pr_merged: bool,
    /// Commits on the spec's branch ahead of `origin/main` (work that exists
    /// locally / on the branch but may not have shipped).
    pub commits_ahead: u32,
    /// The branch is pushed to origin (a remote-tracking branch exists).
    pub branch_pushed: bool,
    /// The worktree has uncommitted (tracked-modified or untracked-not-ignored)
    /// changes.
    pub uncommitted_changes: bool,
    /// A lease still holds this spec's scope (active / dormant / stale — the
    /// recovery may need to end it before re-queueing).
    pub lease_held: bool,
}

/// The recommended recovery path the wizard presents as the default. Each
/// variant maps to a concrete sequence of primitives the execution step runs
/// (with confirmation on destructive ops). trace:STORY-384 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoverAction {
    /// The spec is already `Completed` — nothing to recover. Surface the state
    /// so the operator knows the lifecycle finished (and offers a lease cleanup
    /// if one leaked).
    AlreadyCompleted,
    /// The PR is already merged but the spec hasn't been promoted — run
    /// `aida pull` to auto-bump Done → Completed, and end any leaked lease.
    /// (TASK-336-shipped / pull-missed pattern.)
    AlreadyMergedPull,
    /// A PR is open and the branch is pushed — drive the remaining phases
    /// (reviewer → CI → merge → pull → build) via the TASK-405 `--from-pr`
    /// path, WITHOUT re-running the implementer. The TASK-336 pattern.
    DrivePhasesFromPr,
    /// Commits exist ahead of `origin/main`, not yet pushed, no PR — push the
    /// branch, open a PR, then drive phases 3-6. The most-common phase-1
    /// recovery (TASK-389 / TASK-413 / TASK-416 pattern).
    PushOpenPrDrive,
    /// Commits exist AND the worktree is dirty — commit the WIP first, then
    /// push + open PR + drive. (The operator may instead choose to park; the
    /// wizard offers both, but the recommended default ships the work.)
    WipCommitPushDrive,
    /// No commits ahead but the worktree is dirty — there is uncommitted work
    /// worth preserving but nothing shipped yet. WIP-commit and park for
    /// resumption (TASK-346 pattern); don't open a PR on a single WIP commit.
    WipCommitPark,
    /// No commits, clean worktree — nothing was shipped. End the lease and
    /// re-queue the spec for a fresh attempt (TASK-281 nothing-shipped pattern).
    EndAndRequeue,
}

impl RecoverAction {
    /// A one-line human rationale for why this action was chosen, shown beside
    /// the recommendation so the operator can sanity-check the state read.
    pub(crate) fn rationale(self) -> &'static str {
        match self {
            RecoverAction::AlreadyCompleted => {
                "spec is already Completed — the lifecycle finished; nothing to recover"
            }
            RecoverAction::AlreadyMergedPull => {
                "the PR is merged but the spec isn't promoted yet — pull auto-bumps Done → Completed"
            }
            RecoverAction::DrivePhasesFromPr => {
                "a PR is open and pushed — drive the remaining phases (reviewer → merge → pull → build)"
            }
            RecoverAction::PushOpenPrDrive => {
                "commits exist but were never pushed and no PR is open — push, open a PR, then drive phases 3-6"
            }
            RecoverAction::WipCommitPushDrive => {
                "commits AND uncommitted changes exist — commit the WIP, then push + open PR + drive"
            }
            RecoverAction::WipCommitPark => {
                "uncommitted work but nothing committed yet — commit the WIP and park for resumption"
            }
            RecoverAction::EndAndRequeue => {
                "nothing was shipped (no commits, clean worktree) — end the lease and re-queue"
            }
        }
    }
}

/// Decide the recommended recovery action from the probed state. **Pure.**
///
/// Precedence (most-final state first, so the operator sees the most accurate
/// recommendation and we never recommend driving a PR that already merged):
///
/// 1. `spec_completed` → `AlreadyCompleted` — the lifecycle is done.
/// 2. `pr_exists && pr_merged` → `AlreadyMergedPull` — merge happened, just pull.
/// 3. `pr_exists` (open) → `DrivePhasesFromPr` — engage the orchestrator on the PR.
/// 4. `commits_ahead > 0`:
///      - `uncommitted_changes` → `WipCommitPushDrive` (commit WIP, then ship)
///      - else                  → `PushOpenPrDrive`   (push + PR + drive)
/// 5. `commits_ahead == 0`:
///      - `uncommitted_changes` → `WipCommitPark` (preserve work, park)
///      - else                  → `EndAndRequeue` (nothing shipped, restart)
///
/// Note the PR branches dominate the commits/worktree branches: once a PR is
/// open, the right move is to drive it regardless of local worktree noise (the
/// branch already carries the work). The commits/worktree branches only matter
/// when no PR exists yet. trace:STORY-384 | ai:claude
pub(crate) fn recommend(state: &RecoverState) -> RecoverAction {
    if state.spec_completed {
        return RecoverAction::AlreadyCompleted;
    }
    if state.pr_exists {
        if state.pr_merged {
            return RecoverAction::AlreadyMergedPull;
        }
        return RecoverAction::DrivePhasesFromPr;
    }
    if state.commits_ahead > 0 {
        if state.uncommitted_changes {
            return RecoverAction::WipCommitPushDrive;
        }
        return RecoverAction::PushOpenPrDrive;
    }
    if state.uncommitted_changes {
        return RecoverAction::WipCommitPark;
    }
    RecoverAction::EndAndRequeue
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Convenience constructor in the field order of `RecoverState`:
    /// (completed, pr_exists, pr_merged, commits_ahead, pushed, dirty, lease).
    #[allow(clippy::too_many_arguments)]
    fn st(
        completed: bool,
        pr_exists: bool,
        pr_merged: bool,
        commits_ahead: u32,
        branch_pushed: bool,
        uncommitted_changes: bool,
        lease_held: bool,
    ) -> RecoverState {
        RecoverState {
            spec_completed: completed,
            pr_exists,
            pr_merged,
            commits_ahead,
            branch_pushed,
            uncommitted_changes,
            lease_held,
        }
    }

    #[test]
    fn completed_dominates_everything() {
        // Even with an open PR + commits + dirty worktree, Completed wins so the
        // message is the most accurate ("already shipped, nothing to do").
        assert_eq!(
            recommend(&st(true, true, false, 5, true, true, true)),
            RecoverAction::AlreadyCompleted
        );
        // …and over a merged PR.
        assert_eq!(
            recommend(&st(true, true, true, 0, true, false, false)),
            RecoverAction::AlreadyCompleted
        );
    }

    #[test]
    fn merged_pr_recommends_pull() {
        assert_eq!(
            recommend(&st(false, true, true, 0, true, false, true)),
            RecoverAction::AlreadyMergedPull
        );
        // Merged dominates local commits/worktree noise.
        assert_eq!(
            recommend(&st(false, true, true, 3, false, true, true)),
            RecoverAction::AlreadyMergedPull
        );
    }

    #[test]
    fn open_pr_drives_phases_from_pr() {
        // The TASK-336 pattern: PR open + pushed ⇒ drive 3-6 via --from-pr.
        assert_eq!(
            recommend(&st(false, true, false, 2, true, false, true)),
            RecoverAction::DrivePhasesFromPr
        );
        // An open PR dominates a dirty worktree — the branch carries the work,
        // don't be distracted by local noise.
        assert_eq!(
            recommend(&st(false, true, false, 2, true, true, true)),
            RecoverAction::DrivePhasesFromPr
        );
    }

    #[test]
    fn commits_unpushed_no_pr_pushes_and_drives() {
        // The most-common phase-1 recovery: committed but never pushed, no PR.
        assert_eq!(
            recommend(&st(false, false, false, 1, false, false, true)),
            RecoverAction::PushOpenPrDrive
        );
        // Pushed-but-still-no-PR (a push that didn't reach PR-create) takes the
        // same path — we still need to open the PR, then drive.
        assert_eq!(
            recommend(&st(false, false, false, 1, true, false, true)),
            RecoverAction::PushOpenPrDrive
        );
    }

    #[test]
    fn commits_plus_dirty_worktree_commits_wip_first() {
        assert_eq!(
            recommend(&st(false, false, false, 2, false, true, true)),
            RecoverAction::WipCommitPushDrive
        );
    }

    #[test]
    fn no_commits_dirty_worktree_parks_the_wip() {
        // TASK-346 pattern: uncommitted work, nothing committed ⇒ commit WIP +
        // park (don't open a PR on a single WIP commit).
        assert_eq!(
            recommend(&st(false, false, false, 0, false, true, true)),
            RecoverAction::WipCommitPark
        );
    }

    #[test]
    fn nothing_shipped_clean_worktree_ends_and_requeues() {
        // TASK-281 pattern: no commits, clean worktree ⇒ nothing shipped, end
        // the lease and re-queue for a fresh attempt.
        assert_eq!(
            recommend(&st(false, false, false, 0, false, false, true)),
            RecoverAction::EndAndRequeue
        );
        // Same with no lease held (already cleaned) — still re-queue.
        assert_eq!(
            recommend(&st(false, false, false, 0, false, false, false)),
            RecoverAction::EndAndRequeue
        );
    }

    #[test]
    fn default_state_is_end_and_requeue() {
        // The all-false default (fresh probe found nothing) is the
        // nothing-shipped case — the safe, non-destructive-by-default action.
        assert_eq!(
            recommend(&RecoverState::default()),
            RecoverAction::EndAndRequeue
        );
    }

    #[test]
    fn every_action_has_nonempty_rationale() {
        for action in [
            RecoverAction::AlreadyCompleted,
            RecoverAction::AlreadyMergedPull,
            RecoverAction::DrivePhasesFromPr,
            RecoverAction::PushOpenPrDrive,
            RecoverAction::WipCommitPushDrive,
            RecoverAction::WipCommitPark,
            RecoverAction::EndAndRequeue,
        ] {
            assert!(
                !action.rationale().is_empty(),
                "{action:?} has empty rationale"
            );
        }
    }

    #[test]
    fn pr_branches_dominate_commit_branches_exhaustively() {
        // For every commits/pushed/dirty combination, an OPEN PR always routes
        // to DrivePhasesFromPr — the PR state is authoritative over local state.
        for commits in [0u32, 3] {
            for pushed in [false, true] {
                for dirty in [false, true] {
                    assert_eq!(
                        recommend(&st(false, true, false, commits, pushed, dirty, true)),
                        RecoverAction::DrivePhasesFromPr,
                        "open PR must dominate (commits={commits}, pushed={pushed}, dirty={dirty})"
                    );
                }
            }
        }
    }
}
