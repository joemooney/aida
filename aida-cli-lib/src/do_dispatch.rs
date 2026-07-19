//! Pure decision logic for `aida do` universal dispatch (STORY-776).
//!
//! `aida do <spec>` reads the advisor's bless-time `execution_mode` and routes
//! to the right harness with an explicit human contract. This module holds the
//! IO-free pieces so the whole dispatch policy is unit-testable:
//!
//! - [`propose_execution_mode`] — the classify-seeded proposal heuristic the
//!   TTY micro-groom shows when a spec is ungroomed (ADR-15). Every proposal
//!   carries its reasoning line ("proposing guided: carries keystone tag") so
//!   the operator's confirm is informed rather than a reflex.
//! - [`classify_mode_override`] — the ADR-14 asymmetric `--mode` override
//!   ladder: overriding toward MORE human involvement wins one-shot with a
//!   banner; toward LESS requires `--force`; `decide` cannot be overridden
//!   past.
//! - [`human_contract`] — what each harness will ask of the human and when,
//!   printed BEFORE any harness starts.
//!
//! The IO half (store reads, TTY confirm, persist, self-invoke) lives in
//! `main.rs::run_do_drive`.
// trace:STORY-776 | ai:claude

use aida_core::ExecutionMode;

use crate::presence::is_keystone_marker_tag;

// ── 1. MODE PROPOSAL (ADR-15 seed heuristic) ──────────────────────────────

/// Already-probed facts for [`propose_execution_mode`]. Built by the caller
/// from the store + graph + lint, mirroring `zen_drive::SuitabilityInput`.
#[derive(Debug, Clone)]
pub(crate) struct ModeProposalInput<'a> {
    /// Lowercased requirement type (`task`, `story`, `bug`, …).
    pub req_type: &'a str,
    /// The spec's tags.
    pub tags: &'a [String],
    /// The spec's `human_only` flag (STORY-333) — work only a human can do.
    pub human_only: bool,
    /// True when the spec carries a pending (unanswered) decision request.
    pub has_pending_decision: bool,
    /// True when `aida lint` flagged the spec as under-specified.
    pub under_specified: bool,
}

/// A proposed execution mode plus the reasoning line that justifies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModeProposal {
    pub mode: ExecutionMode,
    /// The reason clause, WITHOUT the "proposing <mode>: " prefix — the caller
    /// formats the full line so display stays in one place.
    pub reason: String,
}

/// Propose an execution mode for an ungroomed spec, seeded from the same facts
/// the zen gate classifies on. Ordered most-binding-first: a human-only mark
/// outranks a pending decision outranks keystone class outranks the trivial
/// fast lane; the default is `drive` (autonomous through CI, human merges).
/// Pure — the caller holds the store.
// trace:STORY-776 | ai:claude
pub(crate) fn propose_execution_mode(i: &ModeProposalInput) -> ModeProposal {
    let keystone_tag = i.tags.iter().find(|t| is_keystone_marker_tag(t));
    if i.human_only {
        return ModeProposal {
            mode: ExecutionMode::Operator,
            reason: "marked human-only — this work is the operator's, not an agent's".into(),
        };
    }
    if i.has_pending_decision {
        return ModeProposal {
            mode: ExecutionMode::Decide,
            reason: "carries a pending decision request — no harness runs until it is answered"
                .into(),
        };
    }
    if i.under_specified {
        return ModeProposal {
            mode: ExecutionMode::Decide,
            reason: "under-specified (no describable behavior or acceptance) — route to clarify"
                .into(),
        };
    }
    if i.req_type.eq_ignore_ascii_case("epic") {
        // An epic is never dispatched directly (the eligibility gate refuses
        // it before any proposal), but keep the function total.
        return ModeProposal {
            mode: ExecutionMode::Operator,
            reason: "an epic is a read-only rollup — its children carry the work".into(),
        };
    }
    if let Some(tag) = keystone_tag {
        return ModeProposal {
            mode: ExecutionMode::Guided,
            reason: format!("carries `{}` tag", tag.trim()),
        };
    }
    if let Some(tag) = i.tags.iter().find(|t| {
        t.eq_ignore_ascii_case("needs-design") || t.eq_ignore_ascii_case("needs-operator-design")
    }) {
        return ModeProposal {
            mode: ExecutionMode::Guided,
            reason: format!(
                "tagged `{}` — design forks decided at the keyboard",
                tag.trim()
            ),
        };
    }
    if let Some(tag) = i.tags.iter().find(|t| {
        let lo = t.trim().to_ascii_lowercase();
        lo == "lifecycle:trivial" || lo == "lifecycle:no-review"
    }) {
        return ModeProposal {
            mode: ExecutionMode::Drain,
            reason: format!(
                "tagged `{}` — full autonomous lifecycle is sanctioned",
                tag.trim()
            ),
        };
    }
    ModeProposal {
        mode: ExecutionMode::Drive,
        reason: format!(
            "bounded {}, no supervised markers",
            i.req_type.trim().to_lowercase()
        ),
    }
}

// ── 2. --mode OVERRIDE LADDER (ADR-14) ────────────────────────────────────

/// The verdict for `aida do <spec> --mode <requested>` against the groomed mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverrideVerdict {
    /// The requested mode equals the groomed mode — no override in play.
    Noop,
    /// Override wins, one-shot. Carries the banner line to print.
    Allowed { banner: String },
    /// Loosening without `--force` — refused. Carries the refusal text.
    NeedsForce { refusal: String },
    /// Not overridable at all (groomed mode is `decide`). Carries the refusal.
    Refused { refusal: String },
}

/// Classify a one-shot `--mode` override per ADR-14. The ladder orders human
/// involvement drain < drive < guided < operator; tighten (toward more human)
/// is free with a banner, loosen needs `--force`, and a groomed `decide` is
/// un-overridable — the pending decision still blocks. Overrides are never
/// persisted. Pure.
// trace:STORY-776 | ai:claude
pub(crate) fn classify_mode_override(
    groomed: ExecutionMode,
    requested: ExecutionMode,
    force: bool,
) -> OverrideVerdict {
    if groomed == requested {
        return OverrideVerdict::Noop;
    }
    if groomed == ExecutionMode::Decide {
        return OverrideVerdict::Refused {
            refusal: format!(
                "groomed mode is `decide` — a pending decision blocks every harness and \
                 --mode cannot override past it (not even with --force). Resolve the \
                 decision first (`aida decide`), then re-groom or `aida do --mode {requested}`."
            ),
        };
    }
    // Requesting `decide` routes to the decision surface — always a tighten.
    let tightens = match (groomed.ladder_rank(), requested.ladder_rank()) {
        (Some(g), Some(r)) => r > g,
        (Some(_), None) => true, // requested == Decide
        // groomed == Decide handled above; unreachable, but be conservative.
        (None, _) => false,
    };
    if tightens || force {
        let mut banner = format!("overriding groomed mode {groomed} → {requested} (one-shot)");
        if !tightens {
            banner.push_str(" — --force loosened the advisor's classification");
        }
        return OverrideVerdict::Allowed { banner };
    }
    OverrideVerdict::NeedsForce {
        refusal: format!(
            "the advisor groomed this spec `{groomed}` — overriding to `{requested}` \
             LOOSENS the human contract, which a plain flag must not do. Pass \
             --mode {requested} --force to override one-shot, or re-groom the spec \
             (`aida edit <spec> --mode {requested}`) to change it durably."
        ),
    }
}

// ── 3. HUMAN CONTRACT (banner) ────────────────────────────────────────────

/// What this harness will ask of the human and when — printed BEFORE any
/// harness starts, so the contract is explicit rather than discovered.
// trace:STORY-776 | ai:claude
pub(crate) fn human_contract(mode: ExecutionMode) -> &'static str {
    match mode {
        ExecutionMode::Drain => {
            "drain: fully autonomous — implement → CI → review → merge → pull. \
             Nothing is asked of you; you'll see the merged result. Failures park \
             the spec NeedsAttention for your triage."
        }
        ExecutionMode::Drive => {
            "drive: autonomous through CI — implement → PR → CI green, then STOP. \
             You review and merge the PR; nothing merges without you."
        }
        ExecutionMode::Guided => {
            "guided: interactive keystone session — you will answer 2-4 recorded \
             forks up front (each becomes an ADR), then review one PR. Never \
             auto-merges."
        }
        ExecutionMode::Operator => {
            "operator: this work is yours — aida prints the checklist and stops. \
             No agent runs."
        }
        ExecutionMode::Decide => {
            "decide: a pending decision blocks this spec — no harness runs until \
             you answer it."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_input<'a>(req_type: &'a str, tags: &'a [String]) -> ModeProposalInput<'a> {
        ModeProposalInput {
            req_type,
            tags,
            human_only: false,
            has_pending_decision: false,
            under_specified: false,
        }
    }

    #[test]
    fn proposal_keystone_tag_forces_guided_with_named_tag_reason() {
        let tags = vec!["keystone".to_string(), "dx".to_string()];
        let p = propose_execution_mode(&base_input("story", &tags));
        assert_eq!(p.mode, ExecutionMode::Guided);
        assert!(
            p.reason.contains("`keystone`"),
            "reason names the tag: {}",
            p.reason
        );
    }

    #[test]
    fn proposal_bounded_bug_defaults_to_drive() {
        let tags = vec!["papercut".to_string()];
        let p = propose_execution_mode(&base_input("bug", &tags));
        assert_eq!(p.mode, ExecutionMode::Drive);
        assert!(p.reason.contains("bounded bug"), "{}", p.reason);
    }

    #[test]
    fn proposal_precedence_human_only_then_decision_then_keystone() {
        let tags = vec!["keystone".to_string()];
        let mut i = base_input("task", &tags);
        i.human_only = true;
        i.has_pending_decision = true;
        assert_eq!(propose_execution_mode(&i).mode, ExecutionMode::Operator);
        i.human_only = false;
        assert_eq!(propose_execution_mode(&i).mode, ExecutionMode::Decide);
        i.has_pending_decision = false;
        assert_eq!(propose_execution_mode(&i).mode, ExecutionMode::Guided);
    }

    #[test]
    fn proposal_under_specified_routes_to_decide() {
        let tags = vec![];
        let mut i = base_input("task", &tags);
        i.under_specified = true;
        let p = propose_execution_mode(&i);
        assert_eq!(p.mode, ExecutionMode::Decide);
        assert!(p.reason.contains("clarify"), "{}", p.reason);
    }

    #[test]
    fn proposal_trivial_lifecycle_tag_sanctions_drain() {
        let tags = vec!["lifecycle:trivial".to_string()];
        let p = propose_execution_mode(&base_input("task", &tags));
        assert_eq!(p.mode, ExecutionMode::Drain);
    }

    #[test]
    fn proposal_needs_design_routes_guided() {
        let tags = vec!["needs-design".to_string()];
        let p = propose_execution_mode(&base_input("story", &tags));
        assert_eq!(p.mode, ExecutionMode::Guided);
    }

    #[test]
    fn override_tighten_is_free_with_banner() {
        let v = classify_mode_override(ExecutionMode::Drive, ExecutionMode::Guided, false);
        match v {
            OverrideVerdict::Allowed { banner } => {
                assert!(banner.contains("drive → guided"), "{banner}");
                assert!(banner.contains("one-shot"), "{banner}");
            }
            other => panic!("expected Allowed, got {other:?}"),
        }
    }

    #[test]
    fn override_loosen_needs_force() {
        let v = classify_mode_override(ExecutionMode::Guided, ExecutionMode::Drain, false);
        assert!(matches!(v, OverrideVerdict::NeedsForce { .. }), "{v:?}");
        let v = classify_mode_override(ExecutionMode::Guided, ExecutionMode::Drain, true);
        match v {
            OverrideVerdict::Allowed { banner } => {
                assert!(banner.contains("--force"), "{banner}")
            }
            other => panic!("expected Allowed with force, got {other:?}"),
        }
    }

    #[test]
    fn override_decide_is_unoverridable_even_with_force() {
        for force in [false, true] {
            let v = classify_mode_override(ExecutionMode::Decide, ExecutionMode::Drain, force);
            assert!(
                matches!(v, OverrideVerdict::Refused { .. }),
                "force={force}: {v:?}"
            );
        }
    }

    #[test]
    fn override_requesting_decide_is_a_tighten() {
        let v = classify_mode_override(ExecutionMode::Drain, ExecutionMode::Decide, false);
        assert!(matches!(v, OverrideVerdict::Allowed { .. }), "{v:?}");
    }

    #[test]
    fn override_same_mode_is_noop() {
        let v = classify_mode_override(ExecutionMode::Drive, ExecutionMode::Drive, false);
        assert_eq!(v, OverrideVerdict::Noop);
    }

    #[test]
    fn every_mode_has_a_human_contract() {
        for mode in ExecutionMode::ALL {
            assert!(!human_contract(mode).is_empty());
        }
    }
}
