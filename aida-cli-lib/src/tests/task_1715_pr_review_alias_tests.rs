//! `aida pr review` is `aida queue work <change> --auto-complete` under a
//! discoverable name: same arguments, same dispatch, same gates.
// trace:TASK-1715 | ai:claude

use super::*;
use crate::cmdkit::{DispatchCtx, Flow};
use crate::forge::ForgeKind;

fn parsed_review_id(argv: &[&str]) -> String {
    let cli = crate::command_registry::try_parse_from(argv.iter().copied()).unwrap();
    match cli.command.registered::<PrArgs>() {
        PrArgs {
            inner: PrCommand::Review { id },
        } => id,
        other => panic!("expected pr review, got {other:?}"),
    }
}

/// What `aida queue work <change> --auto-complete` parses to, from the full CLI.
fn direct_queue_work(change: &str) -> String {
    let cli = crate::command_registry::try_parse_from([
        "aida",
        "queue",
        "work",
        change,
        "--auto-complete",
    ])
    .unwrap();
    format!(
        "{:?}",
        cli.command.registered::<crate::queue_cmd::QueueArgs>()
    )
}

// trace:TASK-1715.ac56fdab | ai:claude
#[test]
fn pr_review_parses_number_and_explicit_change_ids() {
    for id in ["42", "PR-42", "MR-7", "pr-42"] {
        assert_eq!(parsed_review_id(&["aida", "pr", "review", id]), id);
    }
    assert!(crate::command_registry::try_parse_from(["aida", "pr", "review"]).is_err());
}

// trace:TASK-1715.ac56fdab | ai:claude
#[test]
fn bare_numbers_resolve_through_the_configured_forge() {
    assert_eq!(review_change_id("42", ForgeKind::GitHub).unwrap(), "PR-42");
    assert_eq!(review_change_id("#42", ForgeKind::GitHub).unwrap(), "PR-42");
    assert_eq!(review_change_id("7", ForgeKind::GitLab).unwrap(), "MR-7");
    assert_eq!(review_change_id(" 7 ", ForgeKind::GitLab).unwrap(), "MR-7");
}

// trace:TASK-1715.ac56fdab trace:TASK-1715.ace294ab | ai:claude
#[test]
fn explicit_change_ids_are_kept_whatever_the_forge() {
    for forge in [ForgeKind::GitHub, ForgeKind::GitLab, ForgeKind::None] {
        assert_eq!(review_change_id("PR-42", forge).unwrap(), "PR-42");
        assert_eq!(review_change_id("pr-42", forge).unwrap(), "PR-42");
        assert_eq!(review_change_id("MR-7", forge).unwrap(), "MR-7");
        assert_eq!(review_change_id("mr-7", forge).unwrap(), "MR-7");
    }
}

// trace:TASK-1715.ac56fdab trace:TASK-1715.aca3aba9 | ai:claude
#[test]
fn unresolvable_targets_are_refused() {
    let err = review_change_id("42", ForgeKind::None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("PR-42") && err.contains("MR-42"), "{err}");
    for bad in ["", "abc", "TASK-12", "+42", "PR-", "42x", "#"] {
        for forge in [ForgeKind::GitHub, ForgeKind::GitLab] {
            let err = review_change_id(bad, forge).unwrap_err().to_string();
            assert!(err.contains("PR-N") && err.contains("MR-N"), "{bad}: {err}");
        }
    }
}

// trace:TASK-1715.aca65124 trace:TASK-1715.aca3aba9 trace:TASK-1715.ace294ab | ai:claude
#[test]
fn alias_builds_the_same_queue_work_arguments_for_github_and_gitlab() {
    for (id, forge, change) in [
        ("42", ForgeKind::GitHub, "PR-42"),
        ("PR-42", ForgeKind::GitLab, "PR-42"),
        ("7", ForgeKind::GitLab, "MR-7"),
        ("MR-7", ForgeKind::GitHub, "MR-7"),
    ] {
        let resolved = review_change_id(id, forge).unwrap();
        assert_eq!(resolved, change);
        assert_eq!(
            format!("{:?}", review_queue_args(&resolved).unwrap()),
            direct_queue_work(change),
            "{id} under {forge:?}"
        );
    }
    match review_queue_args("MR-7").unwrap().inner {
        crate::cli::QueueCommand::Work { auto_complete, .. } => assert!(auto_complete.is_some()),
        other => panic!("expected queue work, got {other:?}"),
    }
}

// trace:TASK-1715.ac6152c7 trace:TASK-1715.aca3aba9 | ai:claude
#[test]
fn alias_answers_the_cross_cutting_gates_as_queue_work_does() {
    let pr = crate::command_registry::lookup("pr").unwrap();
    let queue = crate::command_registry::lookup("queue").unwrap();
    for change in ["42", "PR-42", "MR-7"] {
        let pr_matches = (pr.command)()
            .try_get_matches_from(["pr", "review", change])
            .unwrap();
        let queue_matches = (queue.command)()
            .try_get_matches_from(["queue", "work", change, "--auto-complete"])
            .unwrap();
        assert_eq!((pr.policy)(&pr_matches), (queue.policy)(&queue_matches));
    }
    // Other pr subcommands keep their own policy.
    let list = (pr.command)().try_get_matches_from(["pr", "list"]).unwrap();
    assert!(!(pr.policy)(&list).team_identity_write);
}

// trace:TASK-1715.aca65124 | ai:claude
#[test]
fn alias_defers_to_the_store_dispatcher_instead_of_running_early() {
    let pr = crate::command_registry::lookup("pr").unwrap();
    let matches = (pr.command)()
        .try_get_matches_from(["pr", "review", "PR-42"])
        .unwrap();
    let ctx = DispatchCtx {
        globals: None,
        store: None,
    };
    assert_eq!(run_pr(&matches, &ctx).unwrap(), Flow::Continue);
}

/// Every gate and refusal `queue work` applies lives behind the shared
/// dispatch, so the alias must reach it rather than re-implement any of it.
// trace:TASK-1715.aca65124 trace:TASK-1715.ac6152c7 | ai:claude
#[test]
fn alias_reuses_the_queue_dispatch_rather_than_a_second_pipeline() {
    let src = include_str!("../pr_cmd.rs");
    let start = src.find("fn run_pr(").expect("run_pr");
    let body = &src[start..start + src[start..].find("\n}\n").unwrap()];
    assert!(body.contains("crate::queue_cmd::dispatch_queue_args("));
    for forbidden in ["handle_queue_command", "Command::new", "--force"] {
        assert!(!body.contains(forbidden), "run_pr must not use {forbidden}");
    }
    let queue = include_str!("../queue_cmd/mod.rs");
    let start = queue.find("\nfn run_queue(").expect("run_queue");
    let body = &queue[start..start + queue[start..].find("\n}\n").unwrap()];
    assert!(body.contains("dispatch_queue_args(&args, ctx)"));
}

// trace:TASK-1715.ac5a12f5 | ai:claude
#[test]
fn help_names_the_pipeline_and_the_other_finish_verbs() {
    let pr = crate::command_registry::lookup("pr").unwrap();
    let mut cmd = (pr.command)();
    let help = cmd
        .find_subcommand_mut("review")
        .expect("review subcommand")
        .render_long_help()
        .to_string()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    for needle in [
        "independent review pipeline",
        "queue work <PR-N|MR-N> --auto-complete",
        "through merge only where merge is already authorized",
        "grants no extra authority",
        "`aida ship --no-merge`",
        "`aida pr ship`",
        "GitLab",
    ] {
        assert!(
            help.contains(needle),
            "pr review help lacks {needle:?}:\n{help}"
        );
    }
}
