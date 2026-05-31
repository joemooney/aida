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
/// The input need not be sorted, nor complete: every phase is evaluated in
/// `Phase::index` order, and a phase ABSENT from the slice is conservatively
/// treated as not-met (re-run it), because an unprobed postcondition must not
/// cause a phase to be skipped. So empty input resumes from the start
/// (`Implementer`) rather than declaring the work complete — the safe default
/// when nothing is known.
pub(crate) fn reconcile_resume_phase(postconditions: &[(Phase, bool)]) -> ResumeDecision {
    const ALL_PHASES: [Phase; 6] = [
        Phase::Implementer,
        Phase::Ci,
        Phase::Reviewer,
        Phase::Merge,
        Phase::Pull,
        Phase::Build,
    ];
    for phase in ALL_PHASES {
        let met = postconditions
            .iter()
            .find(|(p, _)| *p == phase)
            .map(|(_, m)| *m)
            .unwrap_or(false);
        if !met {
            return ResumeDecision::ResumeAt(phase);
        }
    }
    ResumeDecision::AlreadyComplete
}

/// The reconciled-from-reality facts for one crashed drain member: whether each
/// phase's *effect* is already present in the world. The caller probes these
/// from git / PR / spec state (branch exists, CI green, review verdict present,
/// PR merged, spec promoted, post-merge build ok); this module only reasons
/// over them. Keeping the probing out of here means the decision is pure and
/// exhaustively testable, and the probing can be a thin, separately-tested
/// shell. trace:STORY-492 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ResumeFacts {
    /// Implementer: a feature branch with commits referencing the spec exists.
    pub branch_exists: bool,
    /// CI: a CI run for the branch head exists and is green.
    pub ci_green: bool,
    /// Reviewer: an approving review verdict exists for the PR.
    pub reviewed: bool,
    /// Merge: the PR is merged (or the spec already sits on the default branch).
    pub pr_merged: bool,
    /// Pull: the default branch locally contains the merge AND the spec has been
    /// promoted to Completed (auto-bump landed).
    pub spec_completed: bool,
    /// Build: a post-merge build verification succeeded.
    pub build_ok: bool,
}

/// Is `phase`'s postcondition (its effect on the world) already satisfied?
/// Pure mapping from a phase to the corresponding probed fact. An unprobed /
/// false fact means "not met" → that phase will be re-run (conservative).
pub(crate) fn phase_postcondition_met(phase: Phase, facts: &ResumeFacts) -> bool {
    match phase {
        Phase::Implementer => facts.branch_exists,
        Phase::Ci => facts.ci_green,
        Phase::Reviewer => facts.reviewed,
        Phase::Merge => facts.pr_merged,
        Phase::Pull => facts.spec_completed,
        Phase::Build => facts.build_ok,
    }
}

/// What an explicit `--resume` should do for the crashed member, composed from
/// the liveness/shelve classification and the reconcile-from-reality decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResumeOutcome {
    /// Refuse: the original orchestrator may still be alive — never double-drive.
    RefuseOrchestratorAlive,
    /// Nothing to resume (no mid-flight member at the recorded state).
    NothingToResume,
    /// The member was deliberately shelved — leave it parked (triage, don't resume).
    LeaveShelved,
    /// Every phase's effect already exists — just finish/clean up, no phase re-run.
    AlreadyComplete,
    /// Re-enter the live drain at this phase (earliest whose effect is absent).
    ResumeAt(Phase),
}

/// Decide the explicit-resume outcome. Pure: the caller supplies the liveness +
/// shelve facts (probed via the same PID-liveness gate the orchestrator uses to
/// auto-release dormant leases) and the per-phase reality `facts`. The
/// orchestrator-alive guard dominates — a `--resume` must refuse on any doubt
/// that the original drive is dead, because double-driving a spec (two
/// processes merging the same PR / writing the same worktree) is the one
/// unrecoverable failure mode. trace:STORY-492 | ai:claude
pub(crate) fn resume_plan(
    orchestrator_alive: bool,
    member_in_flight: bool,
    member_state_in_phase: bool,
    has_failure_reason: bool,
    facts: &ResumeFacts,
) -> ResumeOutcome {
    match classify_resumability(
        orchestrator_alive,
        member_in_flight,
        member_state_in_phase,
        has_failure_reason,
    ) {
        Resumability::OrchestratorAlive => ResumeOutcome::RefuseOrchestratorAlive,
        Resumability::NotInFlight => ResumeOutcome::NothingToResume,
        Resumability::Shelved => ResumeOutcome::LeaveShelved,
        Resumability::ResumableCrash => {
            const ALL_PHASES: [Phase; 6] = [
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build,
            ];
            let postconditions: Vec<(Phase, bool)> = ALL_PHASES
                .iter()
                .map(|&p| (p, phase_postcondition_met(p, facts)))
                .collect();
            match reconcile_resume_phase(&postconditions) {
                ResumeDecision::AlreadyComplete => ResumeOutcome::AlreadyComplete,
                ResumeDecision::ResumeAt(phase) => ResumeOutcome::ResumeAt(phase),
            }
        }
    }
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
    fn reconcile_treats_missing_phase_as_not_met() {
        // Empty input ⇒ every phase is unprobed ⇒ conservatively not-met ⇒
        // resume from the start, NOT AlreadyComplete (the safe default when
        // nothing is known). trace:STORY-491 (review finding)
        assert_eq!(
            reconcile_resume_phase(&[]),
            ResumeDecision::ResumeAt(Phase::Implementer)
        );
        // A later phase met but an earlier one MISSING ⇒ resume at the missing
        // (not-met) earlier phase, never skip it.
        assert_eq!(
            reconcile_resume_phase(&[(Phase::Build, true)]),
            ResumeDecision::ResumeAt(Phase::Implementer)
        );
    }

    // --- slice 2: resume_plan composition (STORY-492) ---

    fn facts(
        branch: bool,
        ci: bool,
        review: bool,
        merged: bool,
        completed: bool,
        build: bool,
    ) -> ResumeFacts {
        ResumeFacts {
            branch_exists: branch,
            ci_green: ci,
            reviewed: review,
            pr_merged: merged,
            spec_completed: completed,
            build_ok: build,
        }
    }

    #[test]
    fn phase_postcondition_maps_each_phase_to_its_fact() {
        let f = facts(true, false, true, false, true, false);
        assert!(phase_postcondition_met(Phase::Implementer, &f));
        assert!(!phase_postcondition_met(Phase::Ci, &f));
        assert!(phase_postcondition_met(Phase::Reviewer, &f));
        assert!(!phase_postcondition_met(Phase::Merge, &f));
        assert!(phase_postcondition_met(Phase::Pull, &f));
        assert!(!phase_postcondition_met(Phase::Build, &f));
    }

    #[test]
    fn resume_plan_refuses_when_orchestrator_alive() {
        // The double-drive guard dominates regardless of how complete the
        // world looks. This is the catastrophic-risk gate.
        let out = resume_plan(
            true,
            true,
            true,
            false,
            &facts(true, true, true, true, true, true),
        );
        assert_eq!(out, ResumeOutcome::RefuseOrchestratorAlive);
    }

    #[test]
    fn resume_plan_leaves_shelved_member_parked() {
        let out = resume_plan(
            false,
            true,
            true,
            true,
            &facts(true, false, false, false, false, false),
        );
        assert_eq!(out, ResumeOutcome::LeaveShelved);
    }

    #[test]
    fn resume_plan_nothing_to_resume_when_not_in_flight() {
        let out = resume_plan(false, false, false, false, &ResumeFacts::default());
        assert_eq!(out, ResumeOutcome::NothingToResume);
    }

    #[test]
    fn resume_plan_reenters_at_first_unmet_phase() {
        // Crashed after merge but before pull (spec not yet Completed) ⇒
        // resume at Pull, skipping the already-merged PR (idempotent).
        let out = resume_plan(
            false,
            true,
            true,
            false,
            &facts(true, true, true, true, false, false),
        );
        assert_eq!(out, ResumeOutcome::ResumeAt(Phase::Pull));
    }

    #[test]
    fn resume_plan_already_complete_when_every_effect_present() {
        let out = resume_plan(
            false,
            true,
            true,
            false,
            &facts(true, true, true, true, true, true),
        );
        assert_eq!(out, ResumeOutcome::AlreadyComplete);
    }

    #[test]
    fn resume_plan_merge_done_before_crash_is_not_redone() {
        // The checkpoint might say "phase 4 (merge)", but if the PR actually
        // merged before the crash (pr_merged=true) reconcile skips merge and
        // resumes at Pull — never re-merging.
        let out = resume_plan(
            false,
            true,
            true,
            false,
            &facts(true, true, true, true, false, false),
        );
        assert_eq!(out, ResumeOutcome::ResumeAt(Phase::Pull));
    }
}
