//! Operator-hold authority (STORY-1889): who may set or lift a reasoned
//! operator hold on a spec, and the refusal every pickup, launch and queue
//! path returns for a held spec. The `aida defer` / `aida undefer` commands
//! (`defer_cmd.rs`) and the queue call in.
// mandate-gate
// trace:STORY-1889 | ai:claude

use anyhow::Result;

use aida_core::Storage;

#[derive(Debug)]
pub(crate) struct OperatorHoldRefusal {
    pub(crate) spec_id: String,
    pub(crate) reason: String,
    pub(crate) trigger: String,
}

impl std::fmt::Display for OperatorHoldRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} is held ({}; revisit: {}). Run `aida undefer {}` when cleared.",
            self.spec_id, self.reason, self.trigger, self.spec_id
        )
    }
}

impl std::error::Error for OperatorHoldRefusal {}

pub(crate) fn hold_refusal(
    spec_id: &str,
    reason: Option<&str>,
    trigger: Option<&str>,
) -> anyhow::Error {
    OperatorHoldRefusal {
        spec_id: spec_id.to_string(),
        reason: reason.unwrap_or("reason not recorded").to_string(),
        trigger: trigger.unwrap_or("no revisit trigger recorded").to_string(),
    }
    .into()
}

/// Shared read-only hold gate for session creation and existing-lease re-entry.
// trace:BUG-2002 | ai:codex
pub(crate) fn enforce_reasoned_operator_hold(
    project_root: &std::path::Path,
    spec: &str,
) -> Result<()> {
    let path = project_root.join(".aida-store");
    // Storage::load creates a missing legacy file. A preflight must not do that.
    if path.exists() {
        if let Some(req) = Storage::new(path)
            .load()
            .ok()
            .and_then(|store| store.get_requirement_by_spec_id(spec).cloned())
        {
            if req.deferred && req.deferred_reason.is_some() {
                return Err(hold_refusal(
                    req.spec_id.as_deref().unwrap_or(spec),
                    req.deferred_reason.as_deref(),
                    req.deferred_until.as_deref(),
                ));
            }
        }
    }
    Ok(())
}

// trace:BUG-1793 | ai:antigravity
pub(crate) fn hold_authorized() -> bool {
    crate::gate::role_authority::has_hold_authority()
}

/// The refusal for a spec under a reasoned operator hold, if it is one: a
/// deferred spec with a recorded reason. A deferral without a reason is a
/// plain shelf, not a hold.
// trace:STORY-1889 trace:BUG-1793 | ai:claude
pub(crate) fn operator_hold_refusal(
    spec_id: &str,
    deferred: bool,
    reason: Option<&str>,
    trigger: Option<&str>,
) -> Option<anyhow::Error> {
    (deferred && reason.is_some()).then(|| hold_refusal(spec_id, reason, trigger))
}

/// Why `aida defer --reason` may not set an operator hold, if it may not.
// trace:STORY-1889 trace:BUG-1793 | ai:claude
pub(crate) fn set_hold_refusal(reason: Option<&str>) -> Option<&'static str> {
    if reason.is_some() && !hold_authorized() {
        return Some("only the product, advisor, or operator seat may set an operator hold");
    }
    if reason.is_some_and(|reason| reason.trim().is_empty()) {
        return Some("--reason must contain a non-empty reason for the operator hold");
    }
    None
}

/// Why a spec's operator hold may not be lifted or replaced, if it may not.
// trace:STORY-1889 trace:BUG-1793 | ai:claude
pub(crate) fn lift_hold_refusal(existing_reason: Option<&str>) -> Option<&'static str> {
    (existing_reason.is_some() && !hold_authorized())
        .then_some("only the product, advisor, or operator seat may lift this hold")
}
