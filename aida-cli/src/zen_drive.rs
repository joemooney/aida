//! `aida zen <spec>` — the one-shot AUTONOMOUS implement+ship for a single
//! approved spec (STORY-721). The auto-implement counterpart to `aida ship`'s
//! HUMAN-implement finish, and the single-spec member of the ship/zen/integrate
//! taxonomy (`ship` = human-implement single spec, `zen` = auto-implement single
//! spec, `integrate` = land already-Done PRs).
//!
//! `aida zen <spec>` is a THIN wrapper. It does NOT reimplement the
//! orchestrator. It:
//!
//!   1. resolves + validates the spec — refusing a Draft (not-yet-approved)
//!      spec with clear guidance;
//!   2. relies on the existing `--auto-complete` auto-queue
//!      (`main.rs::ensure_queued_for_implementer`, STORY-246) so the operator
//!      never has to `aida queue add` first;
//!   3. drives the one spec through the EXISTING `--auto-complete`
//!      orchestrator by self-invoking `aida queue work <spec> --auto-complete
//!      --no-human <mode>` (implement → CI → review → merge → pull) — the SAME
//!      per-spec engine `aida burndown` / `aida integrate` use (ADR-7), not a
//!      fork. The review + merge phases come for free because they are already
//!      in the shared primitive. trace:TASK-1049
//!
//! Because each invocation drives a single spec in its own worktree, the
//! operator runs several `aida zen <spec>` in parallel for INDEPENDENT specs.
//! (Coupled specs that share files must use the SPIKE-70 `--single-branch`
//! sequential mode instead of parallel fire — a FOLLOW-ON, not this slice.)
//!
//! This module owns the **pure pieces** so they're unit-testable without
//! git/gh/the orchestrator: the approval-gate eligibility classification, the
//! `queue work` argv assembly, and the dry-run plan formatting. The handler
//! lives in `main.rs::run_zen_drive`.
//!
//! trace:STORY-721

use aida_core::RequirementStatus;

/// Whether `aida zen <spec>` may drive a spec, given its current status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ZenEligibility {
    /// Approved / Planned / In Progress — drive it through the orchestrator.
    Ready,
    /// Draft — not yet approved. Slice-1 refuses with guidance to approve
    /// first.
    ///
    /// TODO(STORY-721 follow-on): wire the draft→advisor auto-approve gate via
    /// the autopilot approve action [`crate::autopilot::evaluate`] (TASK-1007 /
    /// EPIC-0428) so a Draft spec routes to the advisor for an approve decision
    /// and then drives, instead of refusing here.
    NeedsApproval,
    /// Done / Completed — already shipped; nothing to drive.
    AlreadyShipped,
    /// Needs Attention — shelved on a punt; triage before driving.
    Shelved,
    /// Rejected — refuse.
    Rejected,
}

/// Classify a spec's drive-eligibility from its status. Pure: the caller reads
/// the status from the store and passes it in.
pub(crate) fn classify_eligibility(status: &RequirementStatus) -> ZenEligibility {
    match status {
        RequirementStatus::Approved
        | RequirementStatus::Planned
        | RequirementStatus::InProgress => ZenEligibility::Ready,
        RequirementStatus::Draft => ZenEligibility::NeedsApproval,
        RequirementStatus::Done | RequirementStatus::Completed => ZenEligibility::AlreadyShipped,
        RequirementStatus::NeedsAttention => ZenEligibility::Shelved,
        RequirementStatus::Rejected => ZenEligibility::Rejected,
    }
}

impl ZenEligibility {
    /// `None` when the spec may be driven; otherwise the operator-facing
    /// refusal message. (`spec` is the operator's own argument, so echoing the
    /// id is fine — no user-facing SPEC-ID-leak concern here.)
    pub(crate) fn refusal(self, spec: &str) -> Option<String> {
        match self {
            ZenEligibility::Ready => None,
            ZenEligibility::NeedsApproval => Some(format!(
                "aida zen needs an approved spec — {spec} is still a draft. \
                 Approve it first (aida edit {spec} --status approved), then re-run \
                 aida zen {spec}."
            )),
            ZenEligibility::AlreadyShipped => {
                Some(format!("{spec} is already shipped — nothing to zen."))
            }
            ZenEligibility::Shelved => Some(format!(
                "{spec} is shelved in Needs Attention — triage it (aida findings list) \
                 and resolve the punt before driving it."
            )),
            ZenEligibility::Rejected => Some(format!("{spec} was rejected — nothing to zen.")),
        }
    }
}

/// The default autonomy mode for `aida zen <spec>` (no `--no-human` flag):
/// run the IMPLEMENTER interactively (the supervised pause point — same as
/// burndown's supervised per-spec drive) but the REVIEWER headless, as an
/// independent agent that always runs.
// trace:TASK-1049 | ai:claude
pub(crate) const DEFAULT_DRIVE_MODE: &str = "reviewer-only";

/// Assemble the `aida queue work <spec> --auto-complete --no-human <mode>`
/// argv that the handler self-invokes. Pure, so the exact set of forwarded
/// flags is pinned by unit tests.
///
/// ONE per-spec orchestration engine (ADR-7): `--auto-complete` is the single
/// home of the implement → CI → review → merge → pull lifecycle — `aida zen`,
/// `aida burndown`, and `aida integrate` all call it and differ only in scope
/// + lifetime, NEVER in the per-spec lifecycle. So zen hands the one spec to
/// the SAME full primitive burndown uses; review + merge come for free because
/// they are already in it.
///
/// The autonomy mode maps onto the orchestrator's EXISTING `--no-human` ladder
/// (no zen-specific pause behavior is invented):
///
///   * **default / `reviewer-only`** — the supervised drive: the implementer
///     pauses for the human (where burndown's supervised per-spec drive
///     pauses), but the reviewer runs HEADLESS as an independent gate, so the
///     review phase ALWAYS runs and the spec drives all the way to main.
///   * **`both`** — fully headless: the implementer runs headless too, so
///     several INDEPENDENT specs can be fired in parallel (fire-and-forget).
///     The review still gates and the merge is automatic.
///
/// TASK-1049: the previous drive shelled to `--auto-complete --zen`, whose
/// reviewer phase is an INTERACTIVE Claude session. In zen's autonomous /
/// no-TTY context that interactive reviewer never produced a verdict, so the
/// drive truncated to implement → PR → hand-off with ZERO review
/// (reviewDecision=NONE). Pointing the drive at the headless `--no-human`
/// reviewer makes the independent review run unconditionally.
// trace:TASK-1049 | ai:claude
pub(crate) fn drive_args(spec: &str, no_human: Option<&str>, no_pull: bool) -> Vec<String> {
    let mode = no_human.unwrap_or(DEFAULT_DRIVE_MODE);
    let mut args = vec![
        "queue".to_string(),
        "work".to_string(),
        spec.to_string(),
        "--auto-complete".to_string(),
        "--no-human".to_string(),
        mode.to_string(),
    ];
    if no_pull {
        args.push("--no-pull".to_string());
    }
    args
}

/// Render the `--dry-run` plan: a one-line summary plus the per-phase steps the
/// `--auto-complete --no-human <mode>` drive will run. Mirrors the actual drive
/// so the preview is faithful. `already_queued` toggles the first step between
/// auto-queue and skip.
pub(crate) fn format_zen_plan(spec: &str, already_queued: bool, no_human: Option<&str>) -> String {
    let mode = match no_human.unwrap_or(DEFAULT_DRIVE_MODE) {
        "both" => "headless implementer + reviewer (--no-human both)".to_string(),
        // The supervised default: interactive implementer, independent headless
        // reviewer. trace:TASK-1049
        _ => "supervised implementer + independent reviewer (--no-human reviewer-only)".to_string(),
    };
    let mut out = String::new();
    out.push_str(&format!(
        "would zen-drive {spec}: queue -> implement -> CI -> review -> merge -> pull\n"
    ));
    out.push_str(&format!("  mode: {mode}\n"));
    let queue_step = if already_queued {
        "already queued for the implementer — skip queue-add"
    } else {
        "queue the spec for the implementer (auto-queue)"
    };
    let steps = [
        queue_step,
        "implement (autonomous orchestrator phase 1)",
        "wait for CI green",
        "review",
        "squash-merge the PR",
        "aida pull (Done -> Completed auto-bump)",
    ];
    for (i, s) in steps.iter().enumerate() {
        out.push_str(&format!("  {}. {}\n", i + 1, s));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approved_and_beyond_are_ready() {
        assert_eq!(
            classify_eligibility(&RequirementStatus::Approved),
            ZenEligibility::Ready
        );
        assert_eq!(
            classify_eligibility(&RequirementStatus::Planned),
            ZenEligibility::Ready
        );
        assert_eq!(
            classify_eligibility(&RequirementStatus::InProgress),
            ZenEligibility::Ready
        );
    }

    #[test]
    fn draft_needs_approval_and_refuses_with_guidance() {
        let e = classify_eligibility(&RequirementStatus::Draft);
        assert_eq!(e, ZenEligibility::NeedsApproval);
        let msg = e.refusal("STORY-721").expect("draft must refuse");
        assert!(msg.contains("needs an approved spec"));
        assert!(msg.contains("aida edit STORY-721 --status approved"));
    }

    #[test]
    fn terminal_states_refuse() {
        assert!(classify_eligibility(&RequirementStatus::Done)
            .refusal("X-1")
            .unwrap()
            .contains("already shipped"));
        assert!(classify_eligibility(&RequirementStatus::Completed)
            .refusal("X-1")
            .unwrap()
            .contains("already shipped"));
        assert!(classify_eligibility(&RequirementStatus::Rejected)
            .refusal("X-1")
            .unwrap()
            .contains("rejected"));
        assert!(classify_eligibility(&RequirementStatus::NeedsAttention)
            .refusal("X-1")
            .unwrap()
            .contains("Needs Attention"));
    }

    #[test]
    fn ready_has_no_refusal() {
        assert_eq!(ZenEligibility::Ready.refusal("X-1"), None);
    }

    // TASK-1049: the default drive must invoke the FULL shared per-spec
    // primitive (`--auto-complete`) with the reviewer running headless
    // (`--no-human reviewer-only`) — NOT the old truncated `--auto-complete
    // --zen` whose interactive reviewer no-showed in zen's autonomous context.
    #[test]
    fn drive_args_default_invokes_full_primitive_with_independent_reviewer() {
        assert_eq!(
            drive_args("STORY-721", None, false),
            vec![
                "queue",
                "work",
                "STORY-721",
                "--auto-complete",
                "--no-human",
                "reviewer-only"
            ]
        );
    }

    // The drive must reach the full lifecycle (review + merge), so it must NOT
    // emit a phase-truncating `--auto-complete=through-ci` / `through-merge`
    // variant, and must NOT fall back to the supervised `--zen` reviewer.
    #[test]
    fn drive_args_is_not_truncated_and_runs_the_reviewer() {
        for no_human in [None, Some("reviewer-only"), Some("both")] {
            let args = drive_args("BUG-9", no_human, false);
            // Full lifecycle primitive — bare `--auto-complete` is variant Full
            // (implement -> CI -> review -> merge -> pull -> build).
            assert!(args.iter().any(|a| a == "--auto-complete"));
            assert!(
                !args.iter().any(|a| a.starts_with("--auto-complete=")
                    || a == "through-ci"
                    || a == "through-merge"),
                "zen must drive the full lifecycle, not a truncated variant: {args:?}"
            );
            // The reviewer phase runs as an INDEPENDENT headless agent in every
            // mode — never the interactive `--zen` reviewer that skipped review.
            assert!(args.iter().any(|a| a == "--no-human"));
            assert!(!args.iter().any(|a| a == "--zen"));
        }
    }

    #[test]
    fn drive_args_forwards_no_human_both_and_no_pull() {
        assert_eq!(
            drive_args("BUG-9", Some("both"), true),
            vec![
                "queue",
                "work",
                "BUG-9",
                "--auto-complete",
                "--no-human",
                "both",
                "--no-pull"
            ]
        );
    }

    #[test]
    fn plan_lists_every_phase_and_auto_queue_step() {
        let plan = format_zen_plan("STORY-721", false, None);
        assert!(plan.contains("would zen-drive STORY-721"));
        assert!(plan.contains("queue -> implement -> CI -> review -> merge -> pull"));
        assert!(plan.contains("auto-queue"));
        assert!(plan.contains("squash-merge the PR"));
        assert!(plan.contains("auto-bump"));
        // The default is the supervised drive with an independent reviewer.
        assert!(plan.contains("independent reviewer"));
    }

    #[test]
    fn plan_skips_queue_step_when_already_queued_and_shows_headless_mode() {
        let plan = format_zen_plan("BUG-9", true, Some("both"));
        assert!(plan.contains("skip queue-add"));
        assert!(plan.contains("headless implementer + reviewer (--no-human both)"));
    }
}
