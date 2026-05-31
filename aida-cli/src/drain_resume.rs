//! Pure decision logic for resumable drain checkpointing (STORY-491, slice 1).
//!
//! When `aida queue work --auto-complete` crashes mid-drain, we want to *resume*
//! at the right phase rather than restart the spec from phase 1. That requires
//! two decisions: (a) is this crashed/parked drain *safely* resumable, and
//! (b) which phase do we re-enter at. This module is **only those two
//! decisions, as pure side-effect-free functions** over already-probed facts.
//!
//! The probing (querying git / PR / spec-status reality, PID liveness) and the
//! live re-entry into the orchestrator phase loop are SEPARATE, sign-off-gated
//! slices — deliberately not here. Keeping the decision logic pure means it is
//! exhaustively unit-testable and robust regardless of how the facts are
//! gathered, and it carries zero risk to the live drain control flow.
//!
//! The central safety invariant: **double-drive is catastrophic** (two
//! processes driving the same spec), so the orchestrator-alive guard is checked
//! first and any doubt about liveness must resolve to "alive" at the call site.
//!
//! trace:STORY-491 | ai:claude

// Slice 1 is the pure decision logic; its callers land in slice 2 (the probing
// + live re-entry wiring, sign-off-gated). Until then these items have only
// test callers, so scope a dead-code allow to this module rather than leave
// warnings for the next session to triage. trace:STORY-491
#![allow(dead_code)]

use crate::auto_complete::Phase;

/// Whether a crashed or parked drain member may be auto-resumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Resumability {
    /// The orchestrator process is still alive — NEVER resume (double-drive).
    OrchestratorAlive,
    /// The member was deliberately shelved (a `FailureReason` is recorded) —
    /// leave it parked in `NeedsAttention` (EPIC-28 semantics), don't resume.
    Shelved,
    /// No member was mid-flight at the recorded state — nothing to resume.
    NotInFlight,
    /// Crashed mid-phase with a dead orchestrator — safe to reconcile + resume.
    ResumableCrash,
}

/// Classify resumability from drain-state facts. Pure: the caller probes the
/// facts (PID liveness, whether a member was current, whether its state is
/// `in-phase-N`, whether the requirement carries a `FailureReason`); this only
/// decides. The `orchestrator_alive` guard is evaluated FIRST because
/// double-driving a spec is the worst outcome — the caller must pass `true`
/// on any uncertainty about liveness.
pub(crate) fn classify_resumability(
    orchestrator_alive: bool,
    member_in_flight: bool,
    member_state_in_phase: bool,
    has_failure_reason: bool,
) -> Resumability {
    if orchestrator_alive {
        return Resumability::OrchestratorAlive;
    }
    if !member_in_flight {
        return Resumability::NotInFlight;
    }
    if has_failure_reason {
        return Resumability::Shelved;
    }
    if member_state_in_phase {
        Resumability::ResumableCrash
    } else {
        Resumability::NotInFlight
    }
}

/// The phase to re-enter, or that the member's work is already complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResumeDecision {
    /// Every phase's postcondition is already satisfied — nothing to redo.
    AlreadyComplete,
    /// Re-enter the drain at this phase: the earliest whose effect is absent.
    ResumeAt(Phase),
}

/// Given each phase's "postcondition already met?" flag, return the earliest
/// phase whose effect is NOT yet present — the resume point. This is the
/// reconcile-from-reality core: the caller computes each flag from the actual
/// world (branch exists? PR merged? spec promoted?), and this picks where to
/// re-enter so phases whose effects already exist are skipped (idempotent
/// resume).
///
/// The input need not be sorted — it is ordered by `Phase::index` here. A phase
/// missing from the slice is conservatively treated as not-met (re-run it),
/// because an unknown postcondition must not cause a phase to be skipped.
pub(crate) fn reconcile_resume_phase(postconditions: &[(Phase, bool)]) -> ResumeDecision {
    let mut ordered: Vec<(Phase, bool)> = postconditions.to_vec();
    ordered.sort_by_key(|(phase, _)| phase.index());
    for (phase, met) in ordered {
        if !met {
            return ResumeDecision::ResumeAt(phase);
        }
    }
    ResumeDecision::AlreadyComplete
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orchestrator_alive_never_resumes() {
        // The double-drive guard wins over every other signal.
        assert_eq!(
            classify_resumability(true, true, true, false),
            Resumability::OrchestratorAlive
        );
    }

    #[test]
    fn shelved_member_is_not_resumed() {
        // Dead orchestrator + in-flight, but a FailureReason ⇒ deliberately
        // shelved (EPIC-28), leave parked.
        assert_eq!(
            classify_resumability(false, true, true, true),
            Resumability::Shelved
        );
    }

    #[test]
    fn crashed_mid_phase_is_resumable() {
        assert_eq!(
            classify_resumability(false, true, true, false),
            Resumability::ResumableCrash
        );
    }

    #[test]
    fn nothing_in_flight_is_not_resumable() {
        assert_eq!(
            classify_resumability(false, false, false, false),
            Resumability::NotInFlight
        );
        // In-flight flag set but state not in-phase (e.g. between members).
        assert_eq!(
            classify_resumability(false, true, false, false),
            Resumability::NotInFlight
        );
    }

    #[test]
    fn reconcile_returns_first_unmet_phase() {
        // Implementer + CI done, reviewer not ⇒ resume at reviewer.
        let pcs = [
            (Phase::Implementer, true),
            (Phase::Ci, true),
            (Phase::Reviewer, false),
            (Phase::Merge, false),
        ];
        assert_eq!(
            reconcile_resume_phase(&pcs),
            ResumeDecision::ResumeAt(Phase::Reviewer)
        );
    }

    #[test]
    fn reconcile_all_met_is_already_complete() {
        let pcs = [
            (Phase::Implementer, true),
            (Phase::Ci, true),
            (Phase::Reviewer, true),
            (Phase::Merge, true),
            (Phase::Pull, true),
            (Phase::Build, true),
        ];
        assert_eq!(
            reconcile_resume_phase(&pcs),
            ResumeDecision::AlreadyComplete
        );
    }

    #[test]
    fn reconcile_sorts_unordered_input_by_phase_index() {
        // Merge "met" but CI not, passed out of order ⇒ still resumes at CI
        // (the earliest unmet), not Merge. Guards against a "merge completed
        // before crash but CI somehow unmet" misread re-entering too late.
        let pcs = [
            (Phase::Merge, true),
            (Phase::Ci, false),
            (Phase::Implementer, true),
        ];
        assert_eq!(
            reconcile_resume_phase(&pcs),
            ResumeDecision::ResumeAt(Phase::Ci)
        );
    }

    #[test]
    fn reconcile_treats_missing_phase_as_already_complete_only_when_all_present_met() {
        // Empty input ⇒ nothing unmet ⇒ AlreadyComplete (caller supplies the
        // full set; an empty set means "no phases to check").
        assert_eq!(reconcile_resume_phase(&[]), ResumeDecision::AlreadyComplete);
    }
}
