//! Stakeholder/requester role capability gates for CLI and MCP actions (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

pub(crate) fn active_stakeholder_role() -> Option<String> {
    std::env::var("AIDA_SESSION_ROLE")
        .ok()
        .map(|role| canonical_role_name(role.trim()))
        .filter(|role| role == "guest" || role == "requester")
}

// trace:TASK-1594 | ai:claude
pub(crate) fn stakeholder_refusal_message(role: &str, action: &str) -> String {
    format!(
        "The '{role}' role is a least-privilege stakeholder role; refusing {action}. Ask an advisor to groom, route, or approve it."
    )
}

pub(crate) fn stakeholder_refusal(role: &str, action: &str) -> anyhow::Error {
    anyhow::anyhow!(stakeholder_refusal_message(role, action))
}

// trace:BUG-1197 | ai:codex
// trace:TASK-1264 | ai:codex
/// Shared capability vocabulary for the CLI and MCP stakeholder envelopes.
/// Keeping this decision independent of either transport prevents a new write
/// path from silently widening guest/requester authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StakeholderAction {
    Read,
    Intake,
    EditRequirement,
    Queue,
    Database,
    MergeGate,
    Session,
    Role,
    Relationship,
    Comment,
    Type,
    Trace,
    Finding,
    Question,
    Configuration,
    BuildLoop,
    Write,
}

pub(crate) fn stakeholder_action_allowed(role: &str, action: StakeholderAction) -> bool {
    match role {
        "guest" => action == StakeholderAction::Read,
        "requester" => matches!(action, StakeholderAction::Read | StakeholderAction::Intake),
        _ => true,
    }
}

pub(crate) fn stakeholder_action_label(action: StakeholderAction) -> &'static str {
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
pub(crate) fn stakeholder_mcp_read_allowed(tool_name: &str) -> bool {
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

pub(crate) fn requester_intake_type_allowed(type_name: &str) -> bool {
    parse_requirement_type(type_name)
        .map(|req_type| requester_add_type_allowed(&req_type))
        .unwrap_or(false)
}

pub(crate) fn parse_requester_add_type(raw: Option<&str>) -> Result<RequirementType> {
    Ok(raw
        .map(parse_requirement_type)
        .transpose()?
        .unwrap_or(RequirementType::ChangeRequest))
}

pub(crate) fn requester_add_type_allowed(req_type: &RequirementType) -> bool {
    matches!(
        req_type,
        RequirementType::ChangeRequest | RequirementType::Bug | RequirementType::User
    )
}

pub(crate) fn add_csv_tag(existing: &mut Option<String>, tag: &str) {
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

/// STORY-1110: hard CLI role envelope for external stakeholder personas.
/// `guest` is read-only. `requester` inherits read access plus one constrained
/// intake write: `aida add` for change-request/bug/user, always Draft and tagged
/// for advisor grooming. Everything else falls into the existing command/gate
/// machinery as a refusal, so these roles cannot become build-loop seats by
/// convention drift.
pub(crate) fn enforce_stakeholder_role_capabilities(command: &mut Command) -> Result<()> {
    let Some(role) = active_stakeholder_role() else {
        return Ok(());
    };
    enforce_stakeholder_role_capabilities_for_role(command, &role)
}

pub(crate) fn enforce_stakeholder_role_capabilities_for_role(
    command: &mut Command,
    role: &str,
) -> Result<()> {
    match role {
        "guest" => {
            let action = stakeholder_cli_action(command);
            if !stakeholder_action_allowed("guest", action) {
                return Err(stakeholder_refusal(
                    "guest",
                    stakeholder_action_label(action),
                ));
            }
        }
        "requester" => {
            // `aida add` is the requester's one intake write. The refusals run
            // here, before dispatch; the coercions (draft status, default
            // type, intake tag) run in the add handler through
            // `apply_requester_intake`, since parsed args cannot be rewritten
            // in place. trace:STORY-1728 | ai:claude
            if let Some(add) = requester_add_args(command) {
                if add.interactive {
                    return Err(stakeholder_refusal("requester", "interactive add"));
                }
                if add.queue || add.batch.is_some() || add.r#for.is_some() {
                    return Err(stakeholder_refusal("requester", "queueing intake"));
                }
                let forbidden_fields = [
                    ("parent", add.parent.is_some()),
                    ("feature", add.feature.is_some()),
                    ("owner", add.owner.is_some()),
                ];
                if REQUESTER_INTAKE_FORBIDDEN_FIELDS.iter().any(|field| {
                    forbidden_fields
                        .iter()
                        .any(|(candidate, is_set)| candidate == field && *is_set)
                }) {
                    return Err(stakeholder_refusal(
                        "requester",
                        "setting parent, feature, or owner during intake",
                    ));
                }
                let req_type = parse_requester_add_type(add.r#type.as_deref())?;
                if !requester_add_type_allowed(&req_type) {
                    return Err(stakeholder_refusal(
                        "requester",
                        "adding anything except change-request, bug, or user specs",
                    ));
                }
                return Ok(());
            }
            let action = stakeholder_cli_action(command);
            if !stakeholder_action_allowed("requester", action) {
                return Err(stakeholder_refusal(
                    "requester",
                    stakeholder_action_label(action),
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

/// The parsed `aida add` args, when `command` is `aida add`.
// trace:STORY-1728 | ai:claude
fn requester_add_args(command: &Command) -> Option<crate::requirements_cli::AddArgs> {
    let Command::Registered(invocation) = command;
    (invocation.reg.name == "add")
        .then(|| clap::FromArgMatches::from_arg_matches(&invocation.matches).ok())
        .flatten()
}

/// Requester intake lands as a Draft change-request (unless typed) tagged
/// for advisor grooming. Applied by the add handler when the active
/// stakeholder role is `requester`; the refusals ran before dispatch.
// trace:STORY-1728 | ai:claude
pub(crate) fn apply_requester_intake(add: &mut crate::requirements_cli::AddArgs) {
    if active_stakeholder_role().as_deref() != Some("requester") {
        return;
    }
    coerce_requester_intake(add);
}

pub(crate) fn coerce_requester_intake(add: &mut crate::requirements_cli::AddArgs) {
    add.status = Some("draft".to_string());
    if add.r#type.is_none() {
        add.r#type = Some("change-request".to_string());
    }
    add_csv_tag(&mut add.tags, "intake:requester");
}

#[cfg(test)]
mod stakeholder_cli_policy_tests {
    use super::*;

    fn guest_refusal(args: &[&str]) -> anyhow::Error {
        let mut cli = crate::command_registry::try_parse_from(args).expect("valid CLI command");
        enforce_stakeholder_role_capabilities_for_role(&mut cli.command, "guest")
            .expect_err("guest command must be refused")
    }

    #[test]
    fn guest_refuses_db_sync() {
        assert!(guest_refusal(&["aida", "db", "sync"])
            .to_string()
            .contains("refusing database writes"));
    }

    #[test]
    fn guest_refuses_session_start() {
        assert!(
            guest_refusal(&["aida", "session", "start", "--owns", "BUG-1197"])
                .to_string()
                .contains("refusing session writes")
        );
    }

    #[test]
    fn guest_allows_mcp_and_coordination_reads() {
        for args in [
            &["aida", "mcp-serve"][..],
            &["aida", "awaiting", "--no-ci"][..],
            &["aida", "queue", "list"][..],
            &["aida", "mailbox", "inbox", "--peek"][..],
        ] {
            let mut cli = crate::command_registry::try_parse_from(args).expect("valid CLI command");
            enforce_stakeholder_role_capabilities_for_role(&mut cli.command, "guest")
                .unwrap_or_else(|err| panic!("guest read {args:?} was refused: {err}"));
        }
    }

    #[test]
    fn guest_refuses_role_enter() {
        assert!(guest_refusal(&["aida", "role", "enter", "advisor"])
            .to_string()
            .contains("refusing role writes"));
    }

    #[test]
    fn requester_cli_rejects_every_shared_forbidden_intake_field() {
        for field in REQUESTER_INTAKE_FORBIDDEN_FIELDS {
            let flag = format!("--{field}");
            let mut cli = crate::command_registry::try_parse_from([
                "aida", "add", "--title", "request", "--type", "bug", &flag, "EPIC-5",
            ])
            .expect("valid CLI command");
            let err = enforce_stakeholder_role_capabilities_for_role(&mut cli.command, "requester")
                .expect_err("shared requester intake field must be refused");
            assert!(
                err.to_string()
                    .contains("setting parent, feature, or owner during intake"),
                "field {field}: {err}"
            );
        }
    }
}

pub(crate) fn stakeholder_cli_action(command: &Command) -> StakeholderAction {
    // Every command answers through its registration. trace:STORY-1728 | ai:claude
    let Command::Registered(invocation) = command;
    invocation.policy().stakeholder
}
