//! Epic status as a read-only rollup of its children.
//!
//! An EPIC's status is NOT a manually-set field — it is a projection of the
//! statuses of its children (the same child subtree `aida graph --tree` rolls
//! up). Manual epic status edits drift from reality in both directions: a
//! childless epic can read In Progress (false "active"), and an epic whose
//! children are shipping can read Draft (false "not started"). This module is
//! the single source of the derived status; every display surface (`aida list`
//! via the cache projection, `aida show`, `aida why`, `aida status`) routes
//! through it so the cache-backed and full-store paths agree, and the edit
//! guard in `aida-cli` rejects manual epic status sets so the field can't drift.
//!
//! The derivation reuses [`graph_walk::child_status_rollup`] — the SAME walk +
//! rollup the `--tree` render prints — so the derived status is always
//! consistent with the rollup numbers an operator sees.
//!
//! trace:BUG-626 | ai:claude

use crate::graph_walk::{child_status_rollup, StatusRollup};
use crate::models::{RequirementStatus, RequirementType, RequirementsStore};
use uuid::Uuid;

/// Derive an epic's effective status from the rollup of its children.
///
/// Precedence (evaluated top to bottom; `total` is the child count):
///
/// 1. **No children** (`total == 0`) -> `Draft` — nothing has started; the epic
///    still needs decomposition. (`aida why` already says "decompose — an epic
///    with no children yet"; this makes the status agree.)
/// 2. **All non-Rejected children Completed** -> `Completed`. A Rejected child
///    is RESOLVED, not open — it can never transition again, so it must not
///    hold the epic in a perpetual "in progress" it can't leave (the BUG-764
///    stuck state: an epic whose last open child completed via the `aida pull`
///    auto-bump but that also carried a rejected child read In Progress
///    forever).
/// 3. **All non-Rejected children Done or Completed** (some on a branch, not
///    all merged) -> `Done`.
/// 4. **At least one child In Progress, OR a mix of done-and-not-done work** ->
///    `InProgress` — the epic is actively moving.
/// 5. **At least one child shelved (NeedsAttention) and none In Progress** ->
///    `NeedsAttention` — a child is blocked / needs a decision. (There is no
///    `Blocked` status variant; `NeedsAttention` is its closest analogue.)
/// 6. **Only remaining (Draft/Approved/Planned) children, none in progress** ->
///    `Draft` — queued but not started.
/// 7. **Only Rejected children** (every other bucket empty) -> `None`: we do not
///    auto-reject an epic. The caller keeps the epic's stored status so a human
///    can make that call (and `--force` recovery still works).
///
/// Returns `None` only in case (7); every other case yields a derived status.
// trace:BUG-626 | ai:claude
pub fn derive_epic_status_from_rollup(r: &StatusRollup) -> Option<RequirementStatus> {
    // (1) Childless epic — nothing started.
    if r.total == 0 {
        return Some(RequirementStatus::Draft);
    }

    // Rejected children are resolved (terminal), not open: exclude them from
    // the finished-vs-open denominator so an epic with zero OPEN children can
    // reach Completed/Done. Before this, a completed+rejected mix fell through
    // to the `any_finished` arm and derived InProgress forever — no child
    // transition could ever move it again. trace:BUG-764 | ai:claude
    let unrejected = r.total - r.rejected;

    // (2) Every non-rejected child Completed.
    if unrejected > 0 && r.completed == unrejected {
        return Some(RequirementStatus::Completed);
    }

    // (3) Every non-rejected child Done or Completed — work finished on
    // branches, not all merged yet.
    if unrejected > 0 && r.done + r.completed == unrejected {
        return Some(RequirementStatus::Done);
    }

    // (4) Actively moving: a child is in progress, OR some work is already
    // done/completed while other children are not (a partially-shipped epic is
    // unambiguously "in progress").
    let any_finished = r.done + r.completed > 0;
    if r.in_progress > 0 || any_finished {
        return Some(RequirementStatus::InProgress);
    }

    // From here: no in-progress and nothing finished — the epic hasn't started.

    // (5) A child is shelved and nothing is moving — surface the stall.
    if r.shelved > 0 {
        return Some(RequirementStatus::NeedsAttention);
    }

    // (6) Only queued work remains (Draft/Approved/Planned children).
    if r.remaining > 0 {
        return Some(RequirementStatus::Draft);
    }

    // (7) Only Rejected children left (total > 0 but every non-rejected bucket is
    // empty). Don't auto-reject the epic; let the caller keep the stored status.
    None
}

/// Derive an epic's effective status from the live store. Returns the rollup
/// status for an epic with the precedence above. Returns `None` when:
/// - `epic_id` doesn't resolve, or the resolved spec is not an Epic (callers
///   should only display the stored status for non-epics), or
/// - the epic's only children are Rejected (case 7 above — keep stored status).
// trace:BUG-626 | ai:claude
pub fn derive_epic_status(store: &RequirementsStore, epic_id: Uuid) -> Option<RequirementStatus> {
    let req = store.get_requirement_by_id(&epic_id)?;
    if req.req_type != RequirementType::Epic {
        return None;
    }
    let rollup = child_status_rollup(store, epic_id);
    derive_epic_status_from_rollup(&rollup)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rollup(
        total: usize,
        completed: usize,
        done: usize,
        in_progress: usize,
        remaining: usize,
        shelved: usize,
        rejected: usize,
    ) -> StatusRollup {
        StatusRollup {
            total,
            completed,
            done,
            in_progress,
            remaining,
            shelved,
            rejected,
        }
    }

    #[test]
    fn childless_epic_is_draft() {
        assert_eq!(
            derive_epic_status_from_rollup(&rollup(0, 0, 0, 0, 0, 0, 0)),
            Some(RequirementStatus::Draft)
        );
    }

    #[test]
    fn one_in_progress_child_is_in_progress() {
        // 3 children: 1 in progress, 2 remaining.
        assert_eq!(
            derive_epic_status_from_rollup(&rollup(3, 0, 0, 1, 2, 0, 0)),
            Some(RequirementStatus::InProgress)
        );
    }

    #[test]
    fn all_children_completed_is_completed() {
        assert_eq!(
            derive_epic_status_from_rollup(&rollup(4, 4, 0, 0, 0, 0, 0)),
            Some(RequirementStatus::Completed)
        );
    }

    #[test]
    fn all_children_done_or_completed_is_done() {
        // 3 children: 1 completed, 2 done — finished on branches, not all merged.
        assert_eq!(
            derive_epic_status_from_rollup(&rollup(3, 1, 2, 0, 0, 0, 0)),
            Some(RequirementStatus::Done)
        );
    }

    #[test]
    fn mix_of_done_and_not_done_is_in_progress() {
        // 3 children: 1 done, 2 remaining (queued) — partially shipped => moving.
        assert_eq!(
            derive_epic_status_from_rollup(&rollup(3, 0, 1, 0, 2, 0, 0)),
            Some(RequirementStatus::InProgress)
        );
    }

    #[test]
    fn shelved_child_with_nothing_moving_is_needs_attention() {
        // 2 children: 1 shelved, 1 remaining, none in progress, none finished.
        assert_eq!(
            derive_epic_status_from_rollup(&rollup(2, 0, 0, 0, 1, 1, 0)),
            Some(RequirementStatus::NeedsAttention)
        );
    }

    #[test]
    fn shelved_child_with_work_in_progress_is_in_progress() {
        // A shelved child does not mask active work — in-progress wins.
        assert_eq!(
            derive_epic_status_from_rollup(&rollup(3, 0, 0, 1, 1, 1, 0)),
            Some(RequirementStatus::InProgress)
        );
    }

    #[test]
    fn only_remaining_children_is_draft() {
        // 3 children all Draft/Approved/Planned, none started.
        assert_eq!(
            derive_epic_status_from_rollup(&rollup(3, 0, 0, 0, 3, 0, 0)),
            Some(RequirementStatus::Draft)
        );
    }

    #[test]
    fn only_rejected_children_keeps_stored_status() {
        // Every child rejected — don't auto-reject the epic; caller keeps stored.
        assert_eq!(
            derive_epic_status_from_rollup(&rollup(2, 0, 0, 0, 0, 0, 2)),
            None
        );
    }

    #[test]
    fn derive_over_store_walks_children_and_skips_non_epics() {
        use crate::models::{Relationship, RelationshipType, Requirement};

        fn req(status: RequirementStatus, req_type: RequirementType) -> Requirement {
            let mut r = Requirement::new("t".into(), "d".into());
            r.status = status;
            r.req_type = req_type;
            r
        }
        fn link(from: &mut Requirement, rel_type: RelationshipType, target: Uuid) {
            from.relationships.push(Relationship {
                rel_type,
                target_id: target,
                created_at: None,
                created_by: None,
            });
        }

        // EPIC stored Draft with one In-Progress child -> derived InProgress.
        let mut epic = req(RequirementStatus::Draft, RequirementType::Epic);
        let mut child = req(RequirementStatus::InProgress, RequirementType::Story);
        link(&mut epic, RelationshipType::Parent, child.id);
        link(&mut child, RelationshipType::Child, epic.id);
        let (epic_id, child_id) = (epic.id, child.id);

        let mut store = RequirementsStore::new();
        store.requirements.extend([epic, child]);

        assert_eq!(
            derive_epic_status(&store, epic_id),
            Some(RequirementStatus::InProgress)
        );
        // A non-epic id yields None — callers display the stored status.
        assert_eq!(derive_epic_status(&store, child_id), None);
        // An unknown id yields None.
        assert_eq!(derive_epic_status(&store, Uuid::new_v4()), None);
    }

    // BUG-764: rejected children are RESOLVED, not open — they must not hold
    // the epic at InProgress forever once every open child has finished. This
    // was the stuck state: an epic whose last open child completed (e.g. via
    // the `aida pull` auto-bump) but that also carried a rejected child derived
    // InProgress with no child transition left that could ever move it.
    #[test]
    fn rejected_plus_completed_rolls_up_completed() {
        // total=3, completed=2, rejected=1 — zero open children => Completed.
        assert_eq!(
            derive_epic_status_from_rollup(&rollup(3, 2, 0, 0, 0, 0, 1)),
            Some(RequirementStatus::Completed)
        );
    }

    // BUG-764: same shape with a Done (finished-on-branch, unmerged) child —
    // zero open children => Done, not InProgress.
    #[test]
    fn rejected_plus_done_rolls_up_done() {
        // total=3, completed=1, done=1, rejected=1.
        assert_eq!(
            derive_epic_status_from_rollup(&rollup(3, 1, 1, 0, 0, 0, 1)),
            Some(RequirementStatus::Done)
        );
    }

    // BUG-764 guard: excluding rejected children from the denominator must NOT
    // close an epic that still has genuinely open work.
    #[test]
    fn rejected_child_does_not_mask_open_work() {
        // completed + rejected + one still-queued child => InProgress (moving).
        assert_eq!(
            derive_epic_status_from_rollup(&rollup(3, 1, 0, 0, 1, 0, 1)),
            Some(RequirementStatus::InProgress)
        );
        // rejected + queued only (nothing finished, nothing moving) => Draft.
        assert_eq!(
            derive_epic_status_from_rollup(&rollup(2, 0, 0, 0, 1, 0, 1)),
            Some(RequirementStatus::Draft)
        );
        // rejected + in-progress => InProgress.
        assert_eq!(
            derive_epic_status_from_rollup(&rollup(2, 0, 0, 1, 0, 0, 1)),
            Some(RequirementStatus::InProgress)
        );
    }
}
