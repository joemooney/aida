//! STORY-527 slice 1: the pure pickability gate for `aida burndown plan`.
//!
//! `/aida-burndown` fans out worktree-isolated implementer subagents over a
//! ready set, with the main session integrating (see
//! `docs/aida/discipline/autonomous-burndown.md`). The non-negotiable safety
//! property is that only **bounded, unblocked, decision-free** specs are fanned
//! out — anything needing a human decision is parked, never dragged in. That is
//! what makes "never stop to ask" safe.
//!
//! This module is the side-effect-free heart of that gate: given a candidate
//! spec's already-probed facts, decide READY vs PARKED(reason). The selector
//! resolution and the graph/blocker probing live in `main.rs`; keeping the
//! verdict pure makes it exhaustively unit-testable. trace:STORY-527 | ai:claude

/// The gate's verdict for one candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Pickability {
    /// Bounded + unblocked + decision-free — safe to fan out autonomously.
    Ready,
    /// Held back, with a human-readable reason.
    Parked(String),
}

/// Already-probed facts about one candidate spec. Built in `main.rs` from the
/// store + graph; consumed by the pure [`classify`]. trace:STORY-527
#[derive(Debug, Clone)]
pub(crate) struct BurndownCandidate {
    /// Display SPEC-ID (e.g. `TASK-702`).
    pub id: String,
    /// Lowercased requirement type (`epic`, `task`, `story`, …).
    pub req_type: String,
    /// The spec's tags (used for the parking-tag check).
    pub tags: Vec<String>,
    /// True when any `BlockedBy` edge points at a not-yet-Completed spec.
    pub has_unsatisfied_blocker: bool,
    /// True when the spec carries a pending `DecisionRequest` (an open
    /// human-decision question via `aida questions`).
    pub has_pending_decision: bool,
}

/// A tag that marks a spec as not-autonomously-pickable — a human decision, a
/// deferral, or a draft-review gate. Matched case-insensitively; `deferred:` is
/// a prefix. Returns the matched tag (for the parked reason). trace:STORY-527
fn parking_tag(tags: &[String]) -> Option<String> {
    for t in tags {
        let lo = t.trim().to_ascii_lowercase();
        let parks = lo == "blocked"
            || lo == "needs-human-input"
            || lo == "needs-human"
            // A spec awaiting a human design/architecture decision or an
            // explicit operator action is NOT autonomously pickable, even
            // though it's bounded + unblocked. (Found dogfooding /aida-burndown:
            // STORY-493 needs-design-signoff + STORY-497 operator-action slipped
            // the gate.) trace:STORY-527 | ai:claude
            || lo == "needs-design-signoff"
            || lo == "needs-design"
            || lo == "operator-action"
            || lo == "review:draft-only"
            || lo.starts_with("deferred:");
        if parks {
            return Some(t.trim().to_string());
        }
    }
    None
}

/// The pickability gate. READY iff the spec is bounded (not an epic),
/// decision-free, unblocked, and not parking-tagged. Exclusions are ordered
/// cheapest/broadest first so the parked reason names the most fundamental
/// blocker. trace:STORY-527
pub(crate) fn classify(c: &BurndownCandidate) -> Pickability {
    if c.req_type.eq_ignore_ascii_case("epic") {
        return Pickability::Parked("epic — decompose into bounded specs first".to_string());
    }
    if c.has_pending_decision {
        return Pickability::Parked(
            "pending decision request — answer via `aida questions`".to_string(),
        );
    }
    if c.has_unsatisfied_blocker {
        return Pickability::Parked("blocked by an unsatisfied dependency (BlockedBy)".to_string());
    }
    if let Some(tag) = parking_tag(&c.tags) {
        return Pickability::Parked(format!("tagged `{tag}`"));
    }
    Pickability::Ready
}

/// Partition candidates into `(ready_ids, parked)` preserving input order —
/// the fan-out set and the skipped set with reasons. trace:STORY-527
pub(crate) fn partition(candidates: &[BurndownCandidate]) -> (Vec<String>, Vec<(String, String)>) {
    let mut ready = Vec::new();
    let mut parked = Vec::new();
    for c in candidates {
        match classify(c) {
            Pickability::Ready => ready.push(c.id.clone()),
            Pickability::Parked(reason) => parked.push((c.id.clone(), reason)),
        }
    }
    (ready, parked)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(
        id: &str,
        req_type: &str,
        tags: &[&str],
        blocked: bool,
        decision: bool,
    ) -> BurndownCandidate {
        BurndownCandidate {
            id: id.to_string(),
            req_type: req_type.to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            has_unsatisfied_blocker: blocked,
            has_pending_decision: decision,
        }
    }

    #[test]
    fn bounded_unblocked_decision_free_spec_is_ready() {
        assert_eq!(
            classify(&cand("TASK-1", "task", &["papercut"], false, false)),
            Pickability::Ready
        );
    }

    #[test]
    fn epic_is_parked_for_decomposition() {
        match classify(&cand("EPIC-1", "epic", &[], false, false)) {
            Pickability::Parked(r) => assert!(r.contains("decompose")),
            other => panic!("expected Parked, got {other:?}"),
        }
    }

    #[test]
    fn pending_decision_parks() {
        assert!(matches!(
            classify(&cand("STORY-1", "story", &[], false, true)),
            Pickability::Parked(_)
        ));
    }

    #[test]
    fn unsatisfied_blocker_parks() {
        assert!(matches!(
            classify(&cand("TASK-2", "task", &[], true, false)),
            Pickability::Parked(_)
        ));
    }

    #[test]
    fn parking_tags_park_case_insensitively_with_deferred_prefix() {
        for tag in [
            "blocked",
            "needs-human-input",
            "Needs-Human",
            "needs-design-signoff",
            "operator-action",
            "review:draft-only",
            "deferred:post-stability",
        ] {
            match classify(&cand("X-1", "task", &[tag], false, false)) {
                Pickability::Parked(r) => assert!(r.to_lowercase().contains(&tag.to_lowercase())),
                other => panic!("tag {tag} should park, got {other:?}"),
            }
        }
        // A benign tag does not park.
        assert_eq!(
            classify(&cand(
                "X-2",
                "task",
                &["batch:foo", "papercut"],
                false,
                false
            )),
            Pickability::Ready
        );
    }

    #[test]
    fn partition_preserves_order_and_separates() {
        let cands = vec![
            cand("A", "task", &[], false, false),             // ready
            cand("B", "epic", &[], false, false),             // parked (epic)
            cand("C", "task", &["deferred:x"], false, false), // parked (tag)
            cand("D", "story", &[], false, false),            // ready
        ];
        let (ready, parked) = partition(&cands);
        assert_eq!(ready, vec!["A".to_string(), "D".to_string()]);
        assert_eq!(
            parked.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
            vec!["B".to_string(), "C".to_string()]
        );
    }
}
