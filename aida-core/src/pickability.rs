//! STORY-333: the pre-pickup gate — a centralized helper that decides whether
//! a spec is *pickable* by `aida queue work` (head pickup), `aida queue next`,
//! batch drains, and cluster drains. Replaces ad-hoc "is the spec In Progress"
//! checks scattered across the queue layer with one truth, so every pickup
//! site applies the same rules.
//!
//! Two un-pickable categories:
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
//!
//! When both apply, `HumanOnly` takes precedence — a human-only spec is
//! still human-only when its blocker clears.
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
/// 2. `BlockedBy` edges — walked once; first `PermanentlyBlocked`
///    (Rejected target) wins over `UnsatisfiedBlocker` (still-in-flight
///    target) so the UI surfaces the louder failure mode.
/// 3. Otherwise pickable.
pub fn pickability(req: &Requirement, store: &RequirementsStore) -> Pickability {
    if req.human_only {
        return Pickability::Blocked(BlockedReason::HumanOnly);
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

/// Render a `BlockedReason` as a single line suitable for the
/// `aida queue list` Blocked section, `aida queue next` skip hints, and
/// the head-pickup banner. The label leads with the *reason kind*, then
/// the relevant target detail.
pub fn pickability_reason_label(reason: &BlockedReason) -> String {
    match reason {
        BlockedReason::HumanOnly => "human-only".to_string(),
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
}
