// trace:TASK-3428 | ai:codex
use crate::{
    for_target_requires_dispatch_authority, is_terminal_status, validate_status_input,
    RequirementStatus, RequirementType, Result,
};

/// Validate a status string against the canonical set. Accepts case-
/// insensitive matches and common spelling variants (`in-progress`,
/// `inprogress`, `in_progress`). Returns Ok with the canonical form, or
/// Err with a list-of-valid-values message. Use at the CLI layer before
/// calling `Requirement::set_status_from_str` to prevent typos like
/// `approvedxxx` from silently landing as a `custom_status`. trace:BUG-47
/// The canonical status list echoed by every "invalid status" refusal, so the
/// plain validator and its type-aware wrapper can never disagree on what the
/// CLI accepts.
// trace:TASK-1176 | ai:claude
pub(crate) const VALID_STATUS_INPUTS: &str =
    "draft, approved, planned, in-progress, done, completed, rejected, superseded, needs-attention";

/// TASK-647 (ADR-3): a status whose *production* requires advisor authority —
/// anything that places a spec into the active execution pipeline (Approved
/// and beyond). Draft / Rejected / NeedsAttention don't gate (they are not
/// "approved work" a non-advisor could smuggle past triage).
pub(crate) fn status_requires_advisor_authority(status: &RequirementStatus) -> bool {
    // TASK-739: single-sourced in the lifecycle model — the target half of the
    // advisor-authority predicate. trace:TASK-739 | ai:claude
    aida_core::lifecycle::target_requires_advisor_authority(
        aida_core::lifecycle::State::from_status(status),
    )
}

/// BUG-482: whether advancing a spec from `from` to `to` (via `aida edit
/// --status`) is an advisor-authority act keyed on the (source, target) pair.
///
/// A non-advisor may not lift an **un-triaged** (`Draft`) *or* a **punted**
/// (`NeedsAttention`) spec into the approved+ pipeline — both sources sit
/// awaiting the advisor's (or interactive human's) triage decision.
/// `forbidden_attention_transition` *permits* `NeedsAttention → Approved` as a
/// triage outcome, but *who* may make that call is still advisor authority;
/// before BUG-482 only `Draft` was a gated source, so a non-advisor could
/// self-re-approve a spec it (or the orchestrator) had just punted, bypassing
/// the triage the punt exists to request. Execution flips from a source
/// already in the pipeline (`Approved → InProgress → Done`) are NOT gated, so
/// drains are unaffected. trace:BUG-482 | ai:claude
pub fn status_advance_requires_advisor_authority(
    from: &RequirementStatus,
    to: &RequirementStatus,
) -> bool {
    // TASK-739: delegate to the lifecycle model's transition guard — the single
    // source for which (from → to) edits are advisor-authority acts. Defined
    // over the full domain (direct edits too, not only declared edges).
    // trace:TASK-739 | ai:claude
    use aida_core::lifecycle::{transition_guard, GuardKind, State};
    transition_guard(State::from_status(from), State::from_status(to))
        == GuardKind::RequiresAdvisorAuthority
}

/// BUG-1611: why `aida queue done` refuses a spec on lifecycle grounds.
/// `queue done` is an execution flip: its legal predecessors are the states a
/// spec reaches only after passing the approval gate (`Approved`, `Planned`,
/// `InProgress`, and an idempotent `Done`). Anything else is refused through
/// the existing lifecycle guard rather than a new rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueDoneLifecycleRefusal {
    /// `Draft` / `NeedsAttention`: never approved, or punted back for triage.
    /// The lifecycle guard marks `→ Done` from these as an advisor-authority
    /// act, and the caller does not hold that authority.
    NotApproved,
    /// `Rejected` / `Completed` / `Superseded`: closed. `queue done` is not a
    /// reopen verb, so it refuses whatever the caller's authority.
    Closed,
}

/// BUG-1611: the lifecycle/authority decision for `aida queue done`, shared by
/// the CLI handler and the MCP `queue_done` tool so the two surfaces cannot
/// drift. `has_advisor_authority` is the caller's resolved authority (CLI:
/// [`has_advisor_authority`]; MCP: the server's role). `None` = proceed.
/// `--force` deliberately has no say here.
// trace:BUG-1611 | ai:claude
pub fn queue_done_lifecycle_refusal(
    from: &RequirementStatus,
    has_advisor_authority: bool,
) -> Option<QueueDoneLifecycleRefusal> {
    if is_terminal_status(from) {
        return Some(QueueDoneLifecycleRefusal::Closed);
    }
    if status_advance_requires_advisor_authority(from, &RequirementStatus::Done)
        && !has_advisor_authority
    {
        return Some(QueueDoneLifecycleRefusal::NotApproved);
    }
    None
}

/// BUG-1611: the operator-facing refusal line for
/// [`queue_done_lifecycle_refusal`]. Neutral guidance only: it names the
/// legitimate route (approval, or a reopen that is itself authority-gated) and
/// never suggests an environment-variable role override.
// trace:BUG-1611 | ai:claude
pub fn queue_done_lifecycle_refusal_message(
    display_id: &str,
    from: &RequirementStatus,
    refusal: QueueDoneLifecycleRefusal,
) -> String {
    match refusal {
        QueueDoneLifecycleRefusal::NotApproved => format!(
            "queue done refused: {display_id} is {from} and has not passed the approval \
             gate, so marking it done would skip triage. Ask an advisor to approve it \
             (`aida edit {display_id} --status approved`), then work it and mark it done."
        ),
        QueueDoneLifecycleRefusal::Closed => format!(
            "queue done refused: {display_id} is already {from} (closed), and marking it \
             done would reopen it. To redo the work, have an advisor reopen it \
             (`aida edit {display_id} --status approved --force`) or file a new spec."
        ),
    }
}

/// BUG-1611: whether `aida zen`'s autopilot approve-gate may flip this spec to
/// `Approved`. `[autopilot] approve = "auto"` is a policy knob in an
/// agent-writable config file, so it cannot stand in for approval authority:
/// the auto-approve runs the same (source → Approved) lifecycle guard every
/// other approval path runs.
// trace:BUG-1611 | ai:claude
pub(crate) fn zen_auto_approve_authorized(
    from: &RequirementStatus,
    has_advisor_authority: bool,
) -> bool {
    !status_advance_requires_advisor_authority(from, &RequirementStatus::Approved)
        || has_advisor_authority
}

/// A manual `aida edit <epic> --status <X>` is forbidden — an epic's status is a
/// read-only rollup of its children. This generalizes the earlier "epics cannot
/// be promoted to Approved" rule to EVERY transition; `--force` is the recovery
/// escape.
// trace:BUG-626 trace:TASK-761 | ai:claude
pub(crate) fn manual_epic_status_edit_forbidden(req_type: &RequirementType, force: bool) -> bool {
    *req_type == RequirementType::Epic && !force
}

pub(crate) fn approval_forbidden_for_type(req_type: &RequirementType) -> bool {
    // trace:TASK-761 | ai:codex
    //
    // BUG-751: `Decision` is deliberately NOT in this set. A decision spec
    // (ADR) is stateful — its documented lifecycle is proposed / accepted /
    // superseded / deprecated — and `Approved` is the sanctioned "accepted"
    // state (Draft == proposed). Gating it left an ADR with no way out of
    // Draft, while existing ADRs already sat at Approved by convention.
    // trace:BUG-751 | ai:claude
    matches!(
        req_type,
        RequirementType::Vision
            | RequirementType::Epic
            | RequirementType::Principle
            | RequirementType::Constraint
            | RequirementType::Term
    )
}

// BUG-751: type-aware wrapper over `validate_status_input`. The ADR lifecycle
// verb `accepted` is not a status in the enum; for decision-class specs it is
// an input alias for `Approved` (approved == accepted for ADRs), so a recorded
// acceptance can be applied with either verb. For every other type — and every
// other input — this delegates to the canonical validator, except that a
// non-decision given `accepted` gets a refusal naming the correct verb.
// trace:BUG-751 | ai:claude
pub(crate) fn validate_status_input_for_type(
    raw: &str,
    req_type: &RequirementType,
) -> Result<&'static str, String> {
    if raw.trim().eq_ignore_ascii_case("accepted") {
        if *req_type == RequirementType::Decision {
            return Ok("Approved");
        }
        return Err(format!(
            "invalid status `{}` — `accepted` applies to decision specs (where it \
             records as `approved`); expected one of: {}",
            raw, VALID_STATUS_INPUTS
        ));
    }
    validate_status_input(raw)
}

/// TASK-130: resolve the `human_only` marker for a freshly-added spec from its
/// type plus the explicit `--human-only` / `--no-human-only` flags.
///
/// A Spike is, by definition, time-boxed research driven by operator judgment
/// (candidate selection, experimental design, rubric evaluation, report
/// writing) — not heads-down implementation. So a Spike defaults to
/// `human_only = true`, which the orchestrator's pre-pickup gate honors by
/// skipping it rather than handing it to an implementer agent that would have
/// to bail or fabricate work. Every other type defaults to `false`.
///
/// The flags always win, and they are mutually exclusive at the CLI layer
/// (`conflicts_with`), so at most one is set:
/// - `--human-only`     → `true`  (opt any type in)
/// - `--no-human-only`  → `false` (opt a Spike back out into auto-pickup)
/// - neither            → the per-type default above
///
/// trace:TASK-130 | ai:claude
pub(crate) fn resolve_human_only(
    req_type: &RequirementType,
    human_only: bool,
    no_human_only: bool,
) -> bool {
    if human_only {
        return true;
    }
    if no_human_only {
        return false;
    }
    matches!(req_type, RequirementType::Spike)
}

/// TASK-754: why `aida add --queue` would refuse to enqueue the freshly-filed
/// spec. Pure decision over the new spec's *final* status (post-intake-gate)
/// plus whether the intake gate downgraded the requested Approved to Draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueAtFilingRefusal {
    /// The advisor-authority gate downgraded Approved → Draft (AC5): a
    /// non-advisor / non-TTY session filed but cannot queue — same hard gate
    /// `aida queue add` enforces.
    Downgraded,
    /// The spec's status is simply not enqueueable (AC2): only Approved is
    /// groomable, matching `aida backlog groom`'s policy.
    NotApproved,
}

// trace:TASK-754 trace:BUG-631 | ai:claude
/// `None` ⇒ enqueue is allowed. `Some(reason)` ⇒ refuse with the matching
/// message. AC2 + AC5 collapse to one rule: the new spec must end up Approved,
/// and a downgrade (the authority gate having fired) is reported distinctly so
/// the operator knows to re-run as the advisor rather than just fix the status.
///
/// The `route` (the `--for` target, before defaulting) carves out the
/// request-for-review case. Routing a freshly-filed draft `--for advisor`
/// (or `--for human`/`--for reviewer`) is a REQUEST for review/triage, open to
/// any role — so it is never refused here regardless of status, mirroring the
/// standalone `aida queue add --for advisor`, which enqueues any non-terminal
/// status. Only execution-dispatch routes carry the Approved-only / authority
/// gate.
pub(crate) fn queue_at_filing_refusal(
    final_status: &RequirementStatus,
    intake_downgraded: bool,
    route: Option<&str>,
) -> Option<QueueAtFilingRefusal> {
    // BUG-631: a request-for-review route bypasses the dispatch gate entirely.
    if !for_target_requires_dispatch_authority(route) {
        return None;
    }
    if matches!(final_status, RequirementStatus::Approved) {
        return None;
    }
    if intake_downgraded {
        Some(QueueAtFilingRefusal::Downgraded)
    } else {
        Some(QueueAtFilingRefusal::NotApproved)
    }
}
