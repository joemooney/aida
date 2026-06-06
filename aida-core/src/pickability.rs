//! STORY-333: the pre-pickup gate — a centralized helper that decides whether
//! a spec is *pickable* by `aida queue work` (head pickup), `aida queue next`,
//! batch drains, and cluster drains. Replaces ad-hoc "is the spec In Progress"
//! checks scattered across the queue layer with one truth, so every pickup
//! site applies the same rules.
//!
//! Three un-pickable categories:
//!
//! - **Blocked** — the spec has a `BlockedBy` relationship to a target that
//!   has not reached `Completed`. Cleared automatically when the blocker
//!   ships. If the blocker is `Rejected`, the block is *permanent* — the
//!   target will never ship, so the dependent needs re-scoping (a UX state
//!   distinct from a normal in-flight blocker).
//! - **Human-only** — the spec carries the `human_only: bool` marker, meaning
//!   it is work no agent can do (a sign-off, a physical task, a moderated
//!   user test). Never auto-unblocks; the human marks it complete by normal
//!   `aida edit --status` when they finish.
//! - **Needs triage** — the spec's status is [`RequirementStatus::NeedsAttention`].
//!   A punt parked it mid-work; an advisor or human must resolve the fork
//!   before the spec can re-enter the pickable head. Without this gate the
//!   spec still sat at the top of `aida queue list` with a `⚠` badge — visible
//!   but rendered alongside actionable items, so a dispatcher reading the
//!   head still misfired drains on it. trace:TASK-131
//!
//! Precedence when multiple apply: `HumanOnly` > `NeedsTriage` > `BlockedBy`.
//! Human-only is the durable reason (still human-only after a punt resolves
//! or a blocker clears); needs-triage outranks BlockedBy because the punt
//! itself needs deciding before the dependency math matters.
//!
//! The helper takes a `&RequirementsStore` so it can resolve `BlockedBy`
//! target uuids back to the target spec's status + display id. A dangling
//! `BlockedBy` (target not in the store) defensively reports as blocked
//! — never as accidentally pickable — and `aida doctor verify-relationships`
//! catches the dangling edge separately.
//!
//! trace:STORY-333 | ai:claude

use crate::models::{Relationship, RelationshipType, Requirement, RequirementStatus};
use crate::RequirementsStore;

/// Why a spec is un-pickable. Kept distinct from `Pickability` itself so a
/// `match Pickability::Blocked(reason)` arm has the structured detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockedReason {
    /// Has a `BlockedBy` edge to a target whose status is not yet
    /// `Completed` (but is not `Rejected` either — that's the permanent
    /// case below). Carries the target's display id for surfacing.
    UnsatisfiedBlocker {
        target_spec: String,
        target_status: RequirementStatus,
    },
    /// Has a `BlockedBy` edge to a target whose status is `Rejected` —
    /// the blocker will never ship, so the dependent needs re-scoping.
    /// A distinct variant so the UI can shout this state rather than
    /// silently skipping it. trace:STORY-333
    PermanentlyBlocked { target_spec: String },
    /// The `human_only: bool` flag is set on the spec. Never auto-clears.
    HumanOnly,
    /// Status is [`RequirementStatus::NeedsAttention`] — paused mid-work by
    /// a punt. An advisor or human must resolve the fork before the spec
    /// re-enters the pickable head. Distinct from `HumanOnly`: this is a
    /// transient state (resumes after triage), whereas `HumanOnly` is the
    /// durable nature of the work. trace:TASK-131 | ai:claude
    NeedsTriage,
}

/// The result of a pickability check. `Pickable` is the only state in which
/// the orchestrator/queue may start a phase on the spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pickability {
    Pickable,
    Blocked(BlockedReason),
}

impl Pickability {
    /// True iff the spec is pickable. Convenience for the common boolean
    /// filter check at consumer sites.
    pub fn is_pickable(&self) -> bool {
        matches!(self, Pickability::Pickable)
    }
}

/// Decide whether `req` is currently pickable by the orchestrator/queue.
///
/// Order of checks (precedence matters for the surfaced reason):
///
/// 1. `human_only` first — a human-only spec stays un-pickable even when
///    its blocker clears, so we report that as the durable reason.
/// 2. `NeedsAttention` status — a punted spec is paused awaiting triage.
///    Gated here (not just at the `aida queue next` site) so `queue list`
///    surfaces it in its Blocked section instead of inline at the head,
///    where a dispatcher misreads it as drainable. trace:TASK-131 | ai:claude
/// 3. `BlockedBy` edges — walked once; first `PermanentlyBlocked`
///    (Rejected target) wins over `UnsatisfiedBlocker` (still-in-flight
///    target) so the UI surfaces the louder failure mode.
/// 4. Otherwise pickable.
pub fn pickability(req: &Requirement, store: &RequirementsStore) -> Pickability {
    if req.human_only {
        return Pickability::Blocked(BlockedReason::HumanOnly);
    }

    if matches!(req.status, RequirementStatus::NeedsAttention) {
        return Pickability::Blocked(BlockedReason::NeedsTriage);
    }

    let blocked_by_edges: Vec<&Relationship> = req
        .relationships
        .iter()
        .filter(|r| matches!(r.rel_type, RelationshipType::BlockedBy))
        .collect();

    if blocked_by_edges.is_empty() {
        return Pickability::Pickable;
    }

    // First pass: any Rejected target wins — surface the loudest signal.
    for rel in &blocked_by_edges {
        match store.requirements.iter().find(|r| r.id == rel.target_id) {
            Some(target) if matches!(target.status, RequirementStatus::Rejected) => {
                return Pickability::Blocked(BlockedReason::PermanentlyBlocked {
                    target_spec: target_display_id(target),
                });
            }
            _ => {}
        }
    }

    // Second pass: any non-Completed target → still blocked. A dangling
    // edge (target not in the store) reports as a blocker too — defensive:
    // an unresolvable blocker is not accidentally pickable. The dangling
    // case is also surfaced by `aida doctor verify-relationships`.
    for rel in &blocked_by_edges {
        match store.requirements.iter().find(|r| r.id == rel.target_id) {
            Some(target) if !matches!(target.status, RequirementStatus::Completed) => {
                return Pickability::Blocked(BlockedReason::UnsatisfiedBlocker {
                    target_spec: target_display_id(target),
                    target_status: target.status.clone(),
                });
            }
            None => {
                return Pickability::Blocked(BlockedReason::UnsatisfiedBlocker {
                    target_spec: format!("(unknown:{})", rel.target_id),
                    target_status: RequirementStatus::Draft,
                });
            }
            _ => {} // Completed target — this edge is satisfied.
        }
    }

    // All BlockedBy targets are Completed → unblocked.
    Pickability::Pickable
}

/// TASK-670: is `req` blocked purely by the dependency graph — i.e. it carries
/// a `BlockedBy` edge to a target that is NOT Completed (an in-progress, draft,
/// rejected, or dangling/unknown blocker)?
///
/// This is the *work-routing* "blocked behind a blocker" axis used by the
/// `aida list --blocked` leading ⊘ glyph. Unlike [`pickability`], it
/// deliberately ignores `human_only` and `NeedsAttention` — those are already
/// surfaced on the status axis (the status glyph `⚠`), and TASK-670's overlay
/// must not duplicate status. trace:TASK-670 | ai:claude
pub fn blocked_by_incomplete(req: &Requirement, store: &RequirementsStore) -> bool {
    req.relationships
        .iter()
        .filter(|r| matches!(r.rel_type, RelationshipType::BlockedBy))
        .any(|rel| {
            match store.requirements.iter().find(|r| r.id == rel.target_id) {
                // Resolvable blocker that hasn't reached Completed → blocked.
                Some(target) => !matches!(target.status, RequirementStatus::Completed),
                // Dangling edge: a blocker we can't resolve is treated as
                // unsatisfied (defensive — don't silently un-block).
                None => true,
            }
        })
}

/// Render a `BlockedReason` as a single line suitable for the
/// `aida queue list` Blocked section, `aida queue next` skip hints, and
/// the head-pickup banner. The label leads with the *reason kind*, then
/// the relevant target detail.
pub fn pickability_reason_label(reason: &BlockedReason) -> String {
    match reason {
        BlockedReason::HumanOnly => "human-only".to_string(),
        BlockedReason::NeedsTriage => "needs-triage".to_string(),
        BlockedReason::PermanentlyBlocked { target_spec } => {
            format!("blocked-by {} (REJECTED — needs re-scoping)", target_spec)
        }
        BlockedReason::UnsatisfiedBlocker {
            target_spec,
            target_status,
        } => format!("blocked-by {} ({})", target_spec, target_status),
    }
}

fn target_display_id(target: &Requirement) -> String {
    target
        .agreed_id
        .clone()
        .or_else(|| target.spec_id.clone())
        .unwrap_or_else(|| target.id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Relationship, Requirement, RequirementStatus};
    use crate::RequirementsStore;
    use uuid::Uuid;

    fn make_req(spec_id: &str, status: RequirementStatus) -> Requirement {
        let mut r = Requirement::new(format!("title for {spec_id}"), String::new());
        r.spec_id = Some(spec_id.to_string());
        r.status = status;
        r
    }

    fn store_with(reqs: Vec<Requirement>) -> RequirementsStore {
        let mut s = RequirementsStore::default();
        s.requirements = reqs;
        s
    }

    fn add_blocked_by(req: &mut Requirement, target_id: Uuid) {
        req.relationships.push(Relationship {
            rel_type: RelationshipType::BlockedBy,
            target_id,
            created_at: None,
            created_by: None,
        });
    }

    #[test]
    fn pickability_pickable_when_no_blockers_no_human_only() {
        let a = make_req("STORY-1", RequirementStatus::Approved);
        let store = store_with(vec![a.clone()]);
        assert_eq!(pickability(&a, &store), Pickability::Pickable);
    }

    #[test]
    fn pickability_blocked_by_in_progress_target_reports_unsatisfied() {
        let blocker = make_req("STORY-B", RequirementStatus::InProgress);
        let mut dependent = make_req("STORY-A", RequirementStatus::Approved);
        add_blocked_by(&mut dependent, blocker.id);
        let store = store_with(vec![blocker, dependent.clone()]);
        match pickability(&dependent, &store) {
            Pickability::Blocked(BlockedReason::UnsatisfiedBlocker {
                target_spec,
                target_status,
            }) => {
                assert_eq!(target_spec, "STORY-B");
                assert_eq!(target_status, RequirementStatus::InProgress);
            }
            other => panic!("expected UnsatisfiedBlocker, got {other:?}"),
        }
    }

    #[test]
    fn pickability_unblocks_when_blocker_reaches_completed() {
        let blocker = make_req("STORY-B", RequirementStatus::Completed);
        let mut dependent = make_req("STORY-A", RequirementStatus::Approved);
        add_blocked_by(&mut dependent, blocker.id);
        let store = store_with(vec![blocker, dependent.clone()]);
        assert_eq!(pickability(&dependent, &store), Pickability::Pickable);
    }

    /// TASK-670: `blocked_by_incomplete` is the graph-only blocked axis for the
    /// `aida list --blocked` ⊘ glyph — true for an incomplete/dangling blocker,
    /// false once every blocker is Completed, and (unlike `pickability`) it
    /// ignores human_only / NeedsAttention so it never duplicates the status
    /// axis. trace:TASK-670 | ai:claude
    #[test]
    fn blocked_by_incomplete_axis() {
        // No edges → not blocked.
        let lone = make_req("STORY-X", RequirementStatus::Approved);
        let store = store_with(vec![lone.clone()]);
        assert!(!blocked_by_incomplete(&lone, &store));

        // Incomplete blocker → blocked.
        let blocker = make_req("STORY-B", RequirementStatus::InProgress);
        let mut dep = make_req("STORY-A", RequirementStatus::Approved);
        add_blocked_by(&mut dep, blocker.id);
        let store = store_with(vec![blocker, dep.clone()]);
        assert!(blocked_by_incomplete(&dep, &store));

        // Blocker Completed → not blocked.
        let done = make_req("STORY-B", RequirementStatus::Completed);
        let mut dep2 = make_req("STORY-A", RequirementStatus::Approved);
        add_blocked_by(&mut dep2, done.id);
        let store = store_with(vec![done, dep2.clone()]);
        assert!(!blocked_by_incomplete(&dep2, &store));

        // Dangling blocker (target absent) → blocked (defensive).
        let mut dep3 = make_req("STORY-A", RequirementStatus::Approved);
        add_blocked_by(&mut dep3, Uuid::new_v4());
        let store = store_with(vec![dep3.clone()]);
        assert!(blocked_by_incomplete(&dep3, &store));

        // human_only / NeedsAttention with no BlockedBy edge → NOT graph-blocked
        // (those belong to the status axis, not this overlay).
        let mut na = make_req("STORY-N", RequirementStatus::NeedsAttention);
        na.human_only = true;
        let store = store_with(vec![na.clone()]);
        assert!(!blocked_by_incomplete(&na, &store));
    }

    #[test]
    fn pickability_blocked_by_rejected_target_reports_permanent() {
        let blocker = make_req("STORY-B", RequirementStatus::Rejected);
        let mut dependent = make_req("STORY-A", RequirementStatus::Approved);
        add_blocked_by(&mut dependent, blocker.id);
        let store = store_with(vec![blocker, dependent.clone()]);
        match pickability(&dependent, &store) {
            Pickability::Blocked(BlockedReason::PermanentlyBlocked { target_spec }) => {
                assert_eq!(target_spec, "STORY-B");
            }
            other => panic!("expected PermanentlyBlocked, got {other:?}"),
        }
    }

    #[test]
    fn pickability_human_only_reports_human_only() {
        let mut h = make_req("TASK-H", RequirementStatus::Approved);
        h.human_only = true;
        let store = store_with(vec![h.clone()]);
        assert_eq!(
            pickability(&h, &store),
            Pickability::Blocked(BlockedReason::HumanOnly)
        );
    }

    #[test]
    fn pickability_human_only_takes_precedence_over_blocked() {
        // Human-only + blocked → reported as human-only (the durable reason).
        let blocker = make_req("STORY-B", RequirementStatus::InProgress);
        let mut dependent = make_req("STORY-A", RequirementStatus::Approved);
        dependent.human_only = true;
        add_blocked_by(&mut dependent, blocker.id);
        let store = store_with(vec![blocker, dependent.clone()]);
        assert_eq!(
            pickability(&dependent, &store),
            Pickability::Blocked(BlockedReason::HumanOnly)
        );
    }

    #[test]
    fn pickability_dangling_blocked_by_target_treated_as_blocked() {
        // Defensive: an unresolvable blocker must not accidentally be pickable.
        let mut dependent = make_req("STORY-A", RequirementStatus::Approved);
        add_blocked_by(&mut dependent, Uuid::now_v7());
        let store = store_with(vec![dependent.clone()]);
        match pickability(&dependent, &store) {
            Pickability::Blocked(BlockedReason::UnsatisfiedBlocker { target_spec, .. }) => {
                assert!(target_spec.starts_with("(unknown:"));
            }
            other => panic!("expected UnsatisfiedBlocker (dangling), got {other:?}"),
        }
    }

    #[test]
    fn pickability_rejected_blocker_wins_over_in_progress_blocker() {
        // Two BlockedBy edges, one Rejected + one InProgress → surface the
        // louder Rejected signal.
        let b_rejected = make_req("STORY-R", RequirementStatus::Rejected);
        let b_inprog = make_req("STORY-I", RequirementStatus::InProgress);
        let mut dependent = make_req("STORY-A", RequirementStatus::Approved);
        add_blocked_by(&mut dependent, b_rejected.id);
        add_blocked_by(&mut dependent, b_inprog.id);
        let store = store_with(vec![b_rejected, b_inprog, dependent.clone()]);
        match pickability(&dependent, &store) {
            Pickability::Blocked(BlockedReason::PermanentlyBlocked { target_spec }) => {
                assert_eq!(target_spec, "STORY-R");
            }
            other => panic!("expected PermanentlyBlocked, got {other:?}"),
        }
    }

    #[test]
    fn pickability_label_renders_each_reason() {
        assert_eq!(
            pickability_reason_label(&BlockedReason::HumanOnly),
            "human-only"
        );
        assert_eq!(
            pickability_reason_label(&BlockedReason::NeedsTriage),
            "needs-triage"
        );
        assert_eq!(
            pickability_reason_label(&BlockedReason::PermanentlyBlocked {
                target_spec: "STORY-B".into()
            }),
            "blocked-by STORY-B (REJECTED — needs re-scoping)"
        );
        assert_eq!(
            pickability_reason_label(&BlockedReason::UnsatisfiedBlocker {
                target_spec: "STORY-B".into(),
                target_status: RequirementStatus::InProgress,
            }),
            "blocked-by STORY-B (In Progress)"
        );
    }

    // TASK-131: NeedsAttention specs must surface in the Blocked section,
    // not inline at the head of `aida queue list`. Until this gate moved
    // into pickability, a dispatcher reading the numbered head still
    // saw shelved specs at positions #1-3 and misfired drains on them.
    // trace:TASK-131 | ai:claude
    #[test]
    fn pickability_needs_attention_reports_needs_triage() {
        let r = make_req("TASK-X", RequirementStatus::NeedsAttention);
        let store = store_with(vec![r.clone()]);
        assert_eq!(
            pickability(&r, &store),
            Pickability::Blocked(BlockedReason::NeedsTriage)
        );
    }

    #[test]
    fn pickability_needs_attention_precedes_blocked_by() {
        // A NeedsAttention spec with an unsatisfied blocker still reports
        // NeedsTriage — the punt itself needs deciding before the
        // dependency math matters. trace:TASK-131
        let blocker = make_req("STORY-B", RequirementStatus::InProgress);
        let mut dependent = make_req("STORY-A", RequirementStatus::NeedsAttention);
        add_blocked_by(&mut dependent, blocker.id);
        let store = store_with(vec![blocker, dependent.clone()]);
        assert_eq!(
            pickability(&dependent, &store),
            Pickability::Blocked(BlockedReason::NeedsTriage)
        );
    }

    #[test]
    fn pickability_human_only_takes_precedence_over_needs_attention() {
        // Human-only is the durable reason — even if the spec is also
        // NeedsAttention (e.g. a human punted on a human-only spec),
        // the surfaced reason stays human-only. trace:TASK-131
        let mut r = make_req("TASK-H", RequirementStatus::NeedsAttention);
        r.human_only = true;
        let store = store_with(vec![r.clone()]);
        assert_eq!(
            pickability(&r, &store),
            Pickability::Blocked(BlockedReason::HumanOnly)
        );
    }
}
