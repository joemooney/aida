// trace:TASK-3429 | ai:codex
//! Stakeholder/requester role capability gates for CLI and MCP actions (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::{parse_requirement_type, Command, REQUESTER_INTAKE_FORBIDDEN_FIELDS};
use aida_core::RequirementType;
use anyhow::Result;

// trace:TASK-3783 | ai:codex
pub use aida_cmdkit::stakeholder_role::*;

pub fn requester_intake_type_allowed(type_name: &str) -> bool {
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
                return requester_intake_check(add);
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

#[cfg(test)]
mod stakeholder_cli_policy_tests {
    use super::*;

    fn guest_refusal(args: &[&str]) -> anyhow::Error {
        let mut cli = crate::TEST_CLI
            .try_parse_from(args)
            .expect("valid CLI command");
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
            let mut cli = crate::TEST_CLI
                .try_parse_from(args)
                .expect("valid CLI command");
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

    fn add_matches(args: &[&str]) -> clap::ArgMatches {
        let cli = crate::TEST_CLI
            .try_parse_from(args)
            .expect("valid CLI command");
        let Command::Registered(invocation) = cli.command;
        assert_eq!(invocation.reg.name, "add");
        invocation.matches
    }

    // Requester intake still lands as Draft with the `intake:requester` tag
    // (and a default change-request type) once `aida add` is a registered
    // handler: the coercion runs in the args the handler files, whatever
    // status the requester asked for. Other roles file the args unchanged.
    // trace:STORY-1728.ac394c52 | ai:claude
    #[test]
    fn requester_intake_is_coerced_by_the_add_handler() {
        let typed = add_matches(&[
            "aida", "add", "--title", "request", "--type", "bug", "--status", "approved", "--tags",
            "ui",
        ]);
        let untyped = add_matches(&["aida", "add", "--title", "request"]);
        {
            let _role = crate::test_env::EnvVarGuard::set("AIDA_SESSION_ROLE", "requester");
            let add = crate::requirements_cli::add_args_for_dispatch(&typed).unwrap();
            assert_eq!(add.status.as_deref(), Some("draft"));
            assert_eq!(add.r#type.as_deref(), Some("bug"));
            assert_eq!(add.tags.as_deref(), Some("ui,intake:requester"));
            let add = crate::requirements_cli::add_args_for_dispatch(&untyped).unwrap();
            assert_eq!(add.status.as_deref(), Some("draft"));
            assert_eq!(add.r#type.as_deref(), Some("change-request"));
            assert_eq!(add.tags.as_deref(), Some("intake:requester"));
        }
        {
            let _role = crate::test_env::EnvVarGuard::set("AIDA_SESSION_ROLE", "implementer");
            let add = crate::requirements_cli::add_args_for_dispatch(&typed).unwrap();
            assert_eq!(add.status.as_deref(), Some("approved"));
            assert_eq!(add.tags.as_deref(), Some("ui"));
        }
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/requirements_cli.rs"),
        )
        .unwrap();
        let run_add = &src[src.find("\nfn run_add(").expect("run_add")..];
        let run_add = &run_add[..run_add.find("\n}\n").expect("run_add closes")];
        assert!(
            run_add.contains("add_args_for_dispatch(matches)")
                && !run_add.contains("from_arg_matches"),
            "the add handler files only coerced args"
        );
    }

    // An `aida add` whose args cannot be decoded is refused for a requester,
    // never waved through as ordinary intake.
    // trace:STORY-1728.ac394c52 | ai:claude
    #[test]
    fn requester_add_check_fails_closed_when_args_do_not_decode() {
        let undecodable = Err(clap::Error::new(clap::error::ErrorKind::ValueValidation));
        let err = requester_intake_check(undecodable).expect_err("must refuse");
        assert!(err.to_string().contains("could not be checked"), "{err}");
        let ok = add_matches(&["aida", "add", "--title", "request", "--type", "bug"]);
        requester_intake_check(clap::FromArgMatches::from_arg_matches(&ok))
            .expect("a well-formed requester add passes");
    }

    #[test]
    fn requester_cli_rejects_every_shared_forbidden_intake_field() {
        for field in REQUESTER_INTAKE_FORBIDDEN_FIELDS {
            let flag = format!("--{field}");
            let mut cli = crate::TEST_CLI
                .try_parse_from([
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

// trace:TASK-3783 | ai:codex
use crate::requirements_cli::requester_add_args;
pub(crate) use crate::requirements_cli::{
    apply_requester_intake, coerce_requester_intake, requester_intake_check,
};
