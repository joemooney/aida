//! Transport-independent stakeholder capability policy.
// trace:TASK-3783 | ai:codex
use aida_core::mailbox_identity::canonical_role_name;

pub fn active_stakeholder_role() -> Option<String> {
    std::env::var("AIDA_SESSION_ROLE")
        .ok()
        .map(|role| canonical_role_name(role.trim()))
        .filter(|role| is_stakeholder_role(role))
}

/// Whether a canonical role name is a least-privilege stakeholder role.
// trace:STORY-1662 | ai:claude
pub fn is_stakeholder_role(role: &str) -> bool {
    role == "guest" || role == "requester"
}

// trace:TASK-1594 | ai:claude
pub fn stakeholder_refusal_message(role: &str, action: &str) -> String {
    format!(
        "The '{role}' role is a least-privilege stakeholder role; refusing {action}. Ask an advisor to groom, route, or approve it."
    )
}

pub fn stakeholder_refusal(role: &str, action: &str) -> anyhow::Error {
    anyhow::anyhow!(stakeholder_refusal_message(role, action))
}

// Preserve the capability vocabulary path without coupling cmdkit to this gate.
// trace:TASK-3425 | ai:codex
pub use crate::StakeholderAction;

pub fn stakeholder_action_allowed(role: &str, action: StakeholderAction) -> bool {
    match role {
        "guest" => action == StakeholderAction::Read,
        "requester" => matches!(action, StakeholderAction::Read | StakeholderAction::Intake),
        _ => true,
    }
}

pub fn stakeholder_action_label(action: StakeholderAction) -> &'static str {
    match action {
        StakeholderAction::Read => "reads",
        StakeholderAction::Intake => "adding requirements",
        StakeholderAction::EditRequirement => "editing requirements",
        StakeholderAction::Queue => "queue operations",
        StakeholderAction::Database => "database writes",
        StakeholderAction::MergeGate => "merge-gate writes",
        StakeholderAction::Session => "session writes",
        StakeholderAction::Role => "role writes",
        StakeholderAction::Relationship => "relationship writes",
        StakeholderAction::Comment => "comment writes",
        StakeholderAction::Type => "type writes",
        StakeholderAction::Trace => "trace writes",
        StakeholderAction::Finding => "finding writes",
        StakeholderAction::Question => "question writes",
        StakeholderAction::Configuration => "configuration writes",
        StakeholderAction::BuildLoop => "build-loop execution",
        StakeholderAction::Write => "writes",
    }
}

/// Tools that stakeholder personas may use as reads. This is deliberately
/// narrower than the MCP `read-only` profile: that profile also contains
/// command-producing helpers whose returned commands can mutate shared state.
// trace:BUG-1197 | ai:codex
pub fn stakeholder_mcp_read_allowed(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "list_requirements"
            | "show_requirement"
            | "search_requirements"
            | "query_graph"
            | "list_features"
            | "history"
            | "read_inbox"
            | "list_punts"
            | "read_punt"
            | "list_findings"
            | "list_active_leases"
            | "list_directives"
            | "list_briefs"
            | "read_brief"
            | "queue_list"
            | "queue_next"
            | "queue_progress"
            | "cache_status"
            | "schema"
            | "status_unified"
            | "usage_query"
            | "plan_verify"
            | "plan_helpers"
            | "ultraplan_assemble"
            | "goal_derive"
    )
}

pub fn add_csv_tag(existing: &mut Option<String>, tag: &str) {
    let mut tags: Vec<String> = existing
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if !tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
        tags.push(tag.to_string());
    }
    *existing = Some(tags.join(","));
}

// trace:TASK-3783 | ai:codex
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stakeholder_roles_keep_their_read_and_intake_envelopes() {
        assert!(stakeholder_action_allowed("guest", StakeholderAction::Read));
        assert!(!stakeholder_action_allowed(
            "guest",
            StakeholderAction::Intake
        ));
        assert!(stakeholder_action_allowed(
            "requester",
            StakeholderAction::Intake
        ));
        for role in ["guest", "requester"] {
            assert!(!stakeholder_action_allowed(role, StakeholderAction::Write));
            assert!(!stakeholder_action_allowed(
                role,
                StakeholderAction::MergeGate
            ));
        }
        assert!(stakeholder_mcp_read_allowed("show_requirement"));
        assert!(!stakeholder_mcp_read_allowed("edit_requirement"));
    }
}
