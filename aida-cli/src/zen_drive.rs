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
//!   3. drives the one spec through the EXISTING `--auto-complete --zen`
//!      orchestrator by self-invoking `aida queue work <spec> --auto-complete
//!      --zen` (implement → CI → review → merge → pull) — the same machinery,
//!      not a fork.
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

/// Assemble the `aida queue work <spec> --auto-complete --zen [...]` argv that
/// the handler self-invokes. Pure, so the exact set of forwarded flags is
/// pinned by unit tests. The orchestrator (the `--auto-complete --zen` drive)
/// is the single home of the implement → CI → review → merge → pull sequence;
/// this just hands it the one spec.
pub(crate) fn drive_args(spec: &str, no_human: Option<&str>, no_pull: bool) -> Vec<String> {
    let mut args = vec![
        "queue".to_string(),
        "work".to_string(),
        spec.to_string(),
        "--auto-complete".to_string(),
        "--zen".to_string(),
    ];
    if let Some(mode) = no_human {
        args.push("--no-human".to_string());
        args.push(mode.to_string());
    }
    if no_pull {
        args.push("--no-pull".to_string());
    }
    args
}

/// Render the `--dry-run` plan: a one-line summary plus the per-phase steps the
/// `--auto-complete --zen` drive will run. Mirrors the actual drive so the
/// preview is faithful. `already_queued` toggles the first step between
/// auto-queue and skip.
pub(crate) fn format_zen_plan(spec: &str, already_queued: bool, no_human: Option<&str>) -> String {
    let mode = match no_human {
        Some(m) => format!("headless (--no-human {m})"),
        None => "advisor-on-standby (--zen)".to_string(),
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

    #[test]
    fn drive_args_minimal_is_queue_work_auto_complete_zen() {
        assert_eq!(
            drive_args("STORY-721", None, false),
            vec!["queue", "work", "STORY-721", "--auto-complete", "--zen"]
        );
    }

    #[test]
    fn drive_args_forwards_no_human_mode_and_no_pull() {
        assert_eq!(
            drive_args("BUG-9", Some("both"), true),
            vec![
                "queue",
                "work",
                "BUG-9",
                "--auto-complete",
                "--zen",
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
        assert!(plan.contains("advisor-on-standby"));
    }

    #[test]
    fn plan_skips_queue_step_when_already_queued_and_shows_headless_mode() {
        let plan = format_zen_plan("BUG-9", true, Some("both"));
        assert!(plan.contains("skip queue-add"));
        assert!(plan.contains("headless (--no-human both)"));
    }
}
