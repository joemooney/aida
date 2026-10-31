//! Seat authority decisions: which role, terminal or orchestrator context
//! holds advisor, hold, dispatch and integrity-floor authority (STORY-1889).
//! The inputs (effective role, TTY probe, orchestrator detection) are
//! gathered outside the gate and read through the crate root.
// mandate-gate
// trace:STORY-1889 | ai:claude

use crate::gate::kernel_deps::authority_stdin_is_terminal;
use crate::role::{current_role_instance_is_companion, effective_role_with_roster};
use crate::worktree_gc::find_main_worktree_root;

/// TASK-647 (ADR-3): "advisor authority" — permission to produce approved+
/// specs and queue work for execution. Held by an explicit advisor role OR a
/// human at an interactive terminal (stdin is a TTY). Headless `claude -p`,
/// spawned agents, and drain/auto captures have neither, so their intake
/// lands `draft` for advisor triage. This is the hard gate (substrate-as-
/// bouncer): a soft default would be bypassed by `--status approved`.
///
/// NB: the MCP server runs non-TTY but may inherit an advisor `AIDA_SESSION_ROLE`
/// from the launching shell — it therefore does NOT use this helper and gates
/// unconditionally (see `tool_add_requirement`).
/// Pure core of [`has_advisor_authority`] (BUG-460): advisor authority is held
/// by an advisor role, an interactive (TTY) session, OR an operation that is
/// corroborated as running under a live orchestrator. The orchestrator case is
/// the fix: the drain's own auto-queue and its headless implementer/reviewer
/// phases are *workflow system actions* (advancing a spec through its
/// lifecycle), not a bare agent self-advancing — and corroboration requires a
/// token matching the live drain-state run, which a bare agent cannot forge, so
/// the TASK-647/ADR-3 gate still blocks an un-orchestrated agent.
/// trace:BUG-460 trace:TASK-647 | ai:claude
pub(crate) fn advisor_authority_from(role: &str, is_tty: bool, orchestrated: bool) -> bool {
    let _ = is_tty; // TTY is used to issue a grant, never as authority by itself.
    role == "advisor" || orchestrated
}

// trace:TASK-1594 | ai:claude
pub(crate) fn hold_authority_from(role: &str, is_tty: bool, _orchestrated: bool) -> bool {
    // ADR-66 carve-out: holds are the supervision floor and emergency brake.
    // The brake must stay reachable by a human at a TTY even when grant issuance is unhealthy.
    // Orchestrated hold authority routes through the validated grant (the `role` check).
    matches!(role, "product" | "advisor" | "operator") || is_tty
}

/// Dispatch authority permits routing already-disposed work without granting
/// the advisor's power to dispose it. Product, advisor, and integrator seats
/// may dispatch; a corroborated live orchestrator may continue its own routing.
/// Implementers and reviewers remain consumers of routed work.
// trace:STORY-1353 | ai:codex
pub(crate) fn dispatch_authority_from(role: &str, orchestrated: bool) -> bool {
    matches!(role, "product" | "advisor" | "integrator") || orchestrated
}

/// Integrity floors require a human who is present at an interactive terminal.
/// No workflow role or orchestrator corroboration can grant this authority.
// trace:STORY-1353 | ai:codex
pub(crate) fn integrity_floor_authority_from(human_present: bool) -> bool {
    human_present
}

pub(crate) fn has_advisor_authority() -> bool {
    if current_role_instance_is_companion() {
        return false;
    }
    // BUG-460: a CLI op spawned by (or under) a live --auto-complete drain
    // inherits AIDA_AUTO_COMPLETE + the run token, so `crate::orchestrator::detect`
    // corroborates it against the drain-state file. Grant it authority so
    // --no-human drains can auto-queue the review + let the phase children
    // mutate; a bare headless agent (no live orchestrator) stays gated.
    let orchestrated = find_main_worktree_root()
        .map(|root| {
            matches!(
                crate::orchestrator::detect(&root),
                crate::orchestrator::OrchestratorContext::Orchestrated
            )
        })
        .unwrap_or(false);
    // STORY-646: consult the durable per-user roster first so the guardrail
    // survives a forgotten `AIDA_SESSION_ROLE`. Non-rostered users / absent
    // store resolve identically to the pre-646 env-only behavior.
    advisor_authority_from(
        &effective_role_with_roster().0,
        authority_stdin_is_terminal(), // trace:BUG-1618 | ai:claude
        orchestrated,
    )
}

// trace:BUG-1793 | ai:antigravity
pub(crate) fn has_hold_authority() -> bool {
    if current_role_instance_is_companion() {
        return false;
    }
    let orchestrated = find_main_worktree_root()
        .map(|root| {
            matches!(
                crate::orchestrator::detect(&root),
                crate::orchestrator::OrchestratorContext::Orchestrated
            )
        })
        .unwrap_or(false);
    hold_authority_from(
        &effective_role_with_roster().0,
        authority_stdin_is_terminal(),
        orchestrated,
    )
}

// trace:STORY-1353 | ai:codex
pub(crate) fn has_dispatch_authority() -> bool {
    if current_role_instance_is_companion() {
        return false;
    }
    let orchestrated = find_main_worktree_root()
        .map(|root| {
            matches!(
                crate::orchestrator::detect(&root),
                crate::orchestrator::OrchestratorContext::Orchestrated
            )
        })
        .unwrap_or(false);
    dispatch_authority_from(&effective_role_with_roster().0, orchestrated)
}

// trace:STORY-1353 | ai:codex
pub(crate) fn has_integrity_floor_authority() -> bool {
    // mandate-gate: a process carrying a deputy job's environment
    // (AIDA_DEPUTY_JOB set) holds the floor only while the deputy's own marker
    // says the TTY policy authorized that job a terminal (signed off on the
    // spec, Q6). This binds every job and child that keeps that environment;
    // a job able to clear it (a script or --allow-exec job, which only the
    // operator can enable) is out of its reach, see
    // `kernel_deps::deputy_job_floor_authorized`.
    // trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude
    integrity_floor_authority_from(
        authority_stdin_is_terminal() // trace:BUG-1618 | ai:claude
            && crate::gate::kernel_deps::deputy_job_floor_authorized(),
    )
}
