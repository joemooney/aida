//! Spec-ID arguments typed as a friendly node alias resolve to the canonical
//! id; everything else passes through untouched.
// trace:BUG-B6B1278890774EF993D2EBC327862470-015 | ai:claude

use super::*;

const NODE: &str = "fedcba9876543210fedcba9876543210";

fn bind_home() -> aida_core::node_alias::ThreadMapGuard {
    let mut map = aida_core::node_alias::NodeAliasMap::default();
    map.set("HOME", NODE).unwrap();
    aida_core::node_alias::install_for_thread(map)
}

fn canonical_of(seq: &str) -> String {
    format!("STORY-{NODE}-{seq}").to_ascii_uppercase()
}

#[test]
fn alias_argument_becomes_canonical_any_case() {
    let _map = bind_home();
    assert_eq!(canonical("STORY-HOME-080"), canonical_of("080"));
    assert_eq!(canonical("story-home-080"), canonical_of("080"));
}

#[test]
fn non_alias_arguments_pass_through_unchanged() {
    let _map = bind_home();
    for id in [
        "STORY-NOPE-080",
        "TASK-1607",
        "PR-14",
        "batch:alias-fix",
        "next3",
        "0193a5b0-7c1d-7000-8000-000000000000",
    ] {
        assert_eq!(canonical(id), id, "{id} must stay as typed");
    }
}

#[test]
fn value_parsers_resolve_single_and_csv_ids() {
    let _map = bind_home();
    assert_eq!(spec_id("STORY-HOME-001").unwrap(), canonical_of("001"));
    assert_eq!(
        spec_id_csv("STORY-HOME-003, TASK-9,story-home-004").unwrap(),
        format!("{}, TASK-9,{}", canonical_of("003"), canonical_of("004"))
    );
}

#[test]
fn queue_verbs_carry_the_canonical_id() {
    use crate::cli::QueueCommand;
    let _map = bind_home();
    let parse = |args: &[&str]| parse_sub::<QueueCommand>(args);
    let want = canonical_of("080");
    match parse(&["add", "STORY-HOME-080"]) {
        QueueCommand::Add { id, .. } => assert_eq!(id, want),
        other => panic!("{other:?}"),
    }
    match parse(&["remove", "story-home-080"]) {
        QueueCommand::Remove { id, .. } => assert_eq!(id, want),
        other => panic!("{other:?}"),
    }
    match parse(&["done", "STORY-HOME-080"]) {
        QueueCommand::Done { id, .. } => assert_eq!(id, want),
        other => panic!("{other:?}"),
    }
    match parse(&["work", "STORY-HOME-080", "--dry-run"]) {
        QueueCommand::Work { id, .. } => assert_eq!(id.as_deref(), Some(want.as_str())),
        other => panic!("{other:?}"),
    }
    match parse(&["work", "STORY-NOPE-080", "--dry-run"]) {
        QueueCommand::Work { id, .. } => assert_eq!(id.as_deref(), Some("STORY-NOPE-080")),
        other => panic!("{other:?}"),
    }
}

/// Parse `args` as subcommand `C` (argv[0] is a placeholder).
fn parse_sub<C: clap::Subcommand + std::fmt::Debug>(args: &[&str]) -> C {
    use clap::Parser;
    #[derive(Parser)]
    struct W<C: clap::Subcommand> {
        #[clap(subcommand)]
        cmd: C,
    }
    W::<C>::try_parse_from(std::iter::once("aida").chain(args.iter().copied()))
        .unwrap_or_else(|e| panic!("{args:?}: {e}"))
        .cmd
}

#[test]
fn worktree_add_and_enter_target_the_canonical_id() {
    use crate::cli::WorktreeCommand;
    let _map = bind_home();
    let want = canonical_of("080");
    match parse_sub(&["add", "STORY-HOME-080"]) {
        WorktreeCommand::Add { target, .. } => assert_eq!(target, want),
        other => panic!("{other:?}"),
    }
    match parse_sub(&["enter", "story-home-080"]) {
        WorktreeCommand::Enter { target, .. } => assert_eq!(target, want),
        other => panic!("{other:?}"),
    }
    // Alias and canonical forms parse to the same target, so the same
    // branch and lease; an unknown label and an epic pass through.
    match parse_sub(&["add", &want]) {
        WorktreeCommand::Add { target, .. } => assert_eq!(target, want),
        other => panic!("{other:?}"),
    }
    for kept in ["STORY-NOPE-080", "EPIC-7"] {
        match parse_sub(&["add", kept]) {
            WorktreeCommand::Add { target, .. } => assert_eq!(target, kept),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn session_verbs_carry_the_canonical_id() {
    use crate::cli::{SessionCommand, SessionManifestCommand};
    let _map = bind_home();
    let parse = |args: &[&str]| parse_sub::<SessionCommand>(args);
    let want = canonical_of("080");
    match parse(&["end", "--spec", "STORY-HOME-080"]) {
        SessionCommand::End { spec, .. } => assert_eq!(spec.as_deref(), Some(want.as_str())),
        other => panic!("{other:?}"),
    }
    match parse(&["start", "--owns", "story-home-080"]) {
        SessionCommand::Start { owns, .. } => assert_eq!(owns, want),
        other => panic!("{other:?}"),
    }
    match parse(&["start", "--owns", "src/scaffolding/**"]) {
        SessionCommand::Start { owns, .. } => assert_eq!(owns, "src/scaffolding/**"),
        other => panic!("{other:?}"),
    }
    match parse(&["manifest", "write", "--items", "STORY-HOME-080,TASK-9"]) {
        SessionCommand::Manifest {
            cmd: SessionManifestCommand::Write { items, .. },
        } => assert_eq!(items, format!("{want},TASK-9")),
        other => panic!("{other:?}"),
    }
    match parse(&["manifest", "mark-started", "STORY-HOME-080"]) {
        SessionCommand::Manifest {
            cmd: SessionManifestCommand::MarkStarted { spec_id, .. },
        } => assert_eq!(spec_id, want),
        other => panic!("{other:?}"),
    }
}

/// `aida review record <SPEC>` keys the verdict under the spec it parses to;
/// an alias must key it under the canonical id. Same for the other review
/// spec arguments.
#[test]
fn review_spec_arguments_parse_to_the_canonical_id() {
    use crate::cli::ReviewCommand;
    let _map = bind_home();
    let want = canonical_of("080");
    match parse_sub(&["record", "STORY-HOME-080", "--verdict", "approved"]) {
        ReviewCommand::Record { spec, .. } => assert_eq!(spec, want),
        other => panic!("{other:?}"),
    }
    match parse_sub(&["verdict", "story-home-080"]) {
        ReviewCommand::Verdict { spec, .. } => assert_eq!(spec, want),
        other => panic!("{other:?}"),
    }
    match parse_sub(&["claim", "--pr", "3", "--spec", "STORY-HOME-080"]) {
        ReviewCommand::Claim { spec, .. } => assert_eq!(spec.as_deref(), Some(want.as_str())),
        other => panic!("{other:?}"),
    }
    match parse_sub(&["prompt", "--specs", "STORY-HOME-080,TASK-9"]) {
        ReviewCommand::Prompt { specs, .. } => {
            assert_eq!(specs, Some(format!("{want},TASK-9")))
        }
        other => panic!("{other:?}"),
    }
    match parse_sub(&["record", "STORY-NOPE-080", "--verdict", "approved"]) {
        ReviewCommand::Record { spec, .. } => assert_eq!(spec, "STORY-NOPE-080"),
        other => panic!("{other:?}"),
    }
}

/// Spot-check the other audited spec arguments that use the value parser.
#[test]
fn audited_spec_arguments_parse_to_the_canonical_id() {
    use crate::cli::{PuntsCommand, UsageCommand};
    let _map = bind_home();
    let want = canonical_of("080");
    match parse_sub(&["resolve", "STORY-HOME-080", "--answer", "a"]) {
        PuntsCommand::Resolve { id, .. } => assert_eq!(id, want),
        other => panic!("{other:?}"),
    }
    match parse_sub(&["list", "--spec", "story-home-080"]) {
        PuntsCommand::List { spec, .. } => assert_eq!(spec.as_deref(), Some(want.as_str())),
        other => panic!("{other:?}"),
    }
    match parse_sub(&["show", "STORY-HOME-080"]) {
        UsageCommand::Show { spec, .. } => assert_eq!(spec, want),
        other => panic!("{other:?}"),
    }
}

/// Every remaining audited spec argument routed through the value parser —
/// one case per argument, so a dropped `value_parser` fails here.
#[test]
fn every_routed_spec_argument_parses_to_the_canonical_id() {
    use crate::cli::*;
    let _map = bind_home();
    let want = canonical_of("080");
    let want_opt = Some(want.as_str());
    let a = "STORY-HOME-080";

    // `aida review <SPEC>` (top-level) through the real registry.
    let review = crate::command_registry::try_parse_from(["aida", "review", a])
        .unwrap()
        .command
        .registered::<crate::review_cmd::ReviewArgs>();
    assert_eq!(review.spec.as_deref(), want_opt);

    match parse_sub(&["run", "well-formed", a]) {
        GateCommand::Run { spec, .. } => assert_eq!(spec, want),
        other => panic!("{other:?}"),
    }
    for args in [
        &[
            "request", "take", "7", "--branch", "b", "--spec", a, "--brief", "x.md",
        ][..],
        &[
            "request", "impl", "--name", "n", "--branch", "b", "--spec", a, "--brief", "x.md",
        ][..],
    ] {
        match parse_sub(args) {
            DispatchAction::Request {
                verb:
                    DispatchRequestVerb::Take { spec, .. } | DispatchRequestVerb::Impl { spec, .. },
                ..
            } => assert_eq!(spec, want, "{args:?}"),
            other => panic!("{other:?}"),
        }
    }
    match parse_sub(&["start", a]) {
        DrainCommand::Start { selector, .. } => assert_eq!(selector.as_deref(), want_opt),
        other => panic!("{other:?}"),
    }
    for kept in ["next", "next3", "batch:alias-fix"] {
        match parse_sub(&["start", kept]) {
            DrainCommand::Start { selector, .. } => assert_eq!(selector.as_deref(), Some(kept)),
            other => panic!("{other:?}"),
        }
    }
    match parse_sub(&["resume", a]) {
        DrainCommand::Resume { spec, .. } => assert_eq!(spec, want),
        other => panic!("{other:?}"),
    }
    match parse_sub(&[
        "add",
        a,
        "--decision",
        "merge",
        "--verdict",
        "ok",
        "--authorized-by",
        "joe",
        "--independent",
        "yes",
    ]) {
        ApprovalCommand::Add { spec, .. } => assert_eq!(spec, want),
    }
    for args in [
        &["answer", a, "1"][..],
        &["decide", a, "1"][..],
        &["review", a][..],
    ] {
        match parse_sub(args) {
            HumanCommand::Answer { spec, .. }
            | HumanCommand::Decide { spec, .. }
            | HumanCommand::Review { spec, .. } => assert_eq!(spec, want, "{args:?}"),
            other => panic!("{other:?}"),
        }
    }
    for args in [&["helpers", a][..], &["scan", a][..]] {
        match parse_sub(args) {
            PlanCommand::Helpers { spec, .. } | PlanCommand::Scan { spec, .. } => {
                assert_eq!(spec, want, "{args:?}")
            }
            other => panic!("{other:?}"),
        }
    }
    match parse_sub(&["promote", a]) {
        PlanCommand::Promote { spec, .. } => assert_eq!(spec.as_deref(), want_opt),
        other => panic!("{other:?}"),
    }
    for args in [
        &["ask", a, "-q", "which?", "-c", "x|y|noop"][..],
        &["remedy", a][..],
    ] {
        match parse_sub(args) {
            QuestionsCommand::Ask { spec, .. } | QuestionsCommand::Remedy { spec, .. } => {
                assert_eq!(spec, want, "{args:?}")
            }
            other => panic!("{other:?}"),
        }
    }
    match parse_sub(&["clarify", a, "TASK-9"]) {
        QuestionsCommand::Clarify { specs, .. } => {
            assert_eq!(specs, vec![want.clone(), "TASK-9".to_string()])
        }
        other => panic!("{other:?}"),
    }
    match parse_sub(&["answer", a]) {
        QuestionsCommand::Answer { spec, .. } => assert_eq!(spec.as_deref(), want_opt),
        other => panic!("{other:?}"),
    }
    for args in [
        &["analyze", "--specs", "STORY-HOME-080,TASK-9"][..],
        &["groom", "--specs", "STORY-HOME-080,TASK-9"][..],
    ] {
        match parse_sub(args) {
            BacklogCommand::Analyze { specs, .. } | BacklogCommand::Groom { specs, .. } => {
                assert_eq!(specs, Some(format!("{want},TASK-9")), "{args:?}")
            }
            other => panic!("{other:?}"),
        }
    }
    for args in [&["list", a][..], &["prune", "--spec", a][..]] {
        match parse_sub(args) {
            RecordCommand::List { spec, .. } | RecordCommand::Prune { spec, .. } => {
                assert_eq!(spec.as_deref(), want_opt, "{args:?}")
            }
        }
    }
    for args in [
        &[
            "register",
            "42",
            "--type",
            "claude",
            "--role",
            "implementer",
            "--spec",
            a,
        ][..],
        &["new", "claude", "--spec", a][..],
        &["new", "codex", "--spec", a][..],
        &["new", "antigravity", "--spec", a][..],
    ] {
        match parse_sub(args) {
            AgentCommand::Register { spec, .. }
            | AgentCommand::New {
                command:
                    Some(
                        AgentNewCommand::Claude { spec, .. }
                        | AgentNewCommand::Codex { spec, .. }
                        | AgentNewCommand::Antigravity { spec, .. },
                    ),
                ..
            } => assert_eq!(spec.as_deref(), want_opt, "{args:?}"),
            other => panic!("{other:?}"),
        }
    }
}

/// `aida pr auto-queue-review` files the review story against the ids its
/// commit trailers name; a `(TYPE-<LABEL>-NNN)` trailer names the canonical
/// id, and alias and canonical trailers dedup to one entry.
#[test]
fn review_coverage_trailers_resolve_to_the_canonical_id() {
    let _map = bind_home();
    let want = canonical_of("080");
    let messages = vec![
        "[AI:claude] fix(x): one (STORY-HOME-080)".to_string(),
        format!("[AI:claude] fix(x): two ({want})"),
        "[AI:claude] feat(x): slice (Refs STORY-HOME-081)\n\nTouches (STORY-HOME-082)".to_string(),
    ];
    let cov = crate::review_gates::review_coverage_from_commits(&messages);
    assert_eq!(cov.delivered, vec![want]);
    assert_eq!(cov.slice_refs, vec![canonical_of("081")]);
    assert_eq!(cov.referenced, vec![canonical_of("082")]);
}

/// Argument ids / value names that name a spec. Compared case-insensitively.
const SPEC_ARG_NAMES: &[&str] = &[
    "spec",
    "specs",
    "spec_id",
    "owns",
    "req",
    "req_id",
    "epic",
    "epic_or_spec",
];

/// Spec-named arguments that are deliberately not spec ids, with the reason.
/// The two `--scope` entries are not matched by name today; they are listed
/// so a rename to a spec-like value name keeps them deliberate.
const NOT_A_SPEC_ID: &[(&str, &str)] = &[
    (
        "questions sweep --scope",
        "a status keyword (backlog, approved, all, ...)",
    ),
    ("triage --scope", "a subsystem name"),
    ("burndown readiness --specs", "a count of expected specs"),
];

/// Ratchet: every argument in the real command tree whose id or value name
/// names a spec resolves an alias through [`spec_id`] / [`spec_id_csv`].
/// Checked by behavior — the arg's value parser must turn `STORY-HOME-080`
/// into the canonical id — so a new or missed spec arg fails here.
#[test]
fn every_spec_named_argument_in_the_tree_resolves_aliases() {
    let _map = bind_home();
    let want = canonical_of("080");
    let root = crate::command_registry::built_command();
    let mut failures = Vec::new();
    walk_spec_args(&root, "", &want, &mut failures);
    assert!(
        failures.is_empty(),
        "spec arguments that keep an alias as typed (add `value_parser = \
         crate::spec_arg::spec_id` or list them in NOT_A_SPEC_ID):\n{}",
        failures.join("\n")
    );
}

fn names_a_spec(arg: &clap::Arg) -> bool {
    let id = arg.get_id().as_str().replace('-', "_");
    let value_names = arg.get_value_names().unwrap_or_default();
    std::iter::once(id)
        .chain(value_names.iter().map(|v| v.as_str().replace('-', "_")))
        .any(|name| SPEC_ARG_NAMES.iter().any(|s| s.eq_ignore_ascii_case(&name)))
}

/// What `arg`'s own value parser makes of `STORY-HOME-080`, probed through a
/// one-argument command (the parser's direct entry point is crate-private to
/// clap). `None` when it fails or is not a `String`.
fn parses_to(arg: &clap::Arg) -> Option<String> {
    let probe = clap::Arg::new("v")
        .required(true)
        .value_parser(arg.get_value_parser().clone());
    clap::Command::new("probe")
        .arg(probe)
        .try_get_matches_from(["probe", "STORY-HOME-080"])
        .ok()?
        .try_get_one::<String>("v")
        .ok()
        .flatten()
        .cloned()
}

fn walk_spec_args(cmd: &clap::Command, path: &str, want: &str, failures: &mut Vec<String>) {
    for arg in cmd.get_arguments() {
        if !names_a_spec(arg) || !arg.get_action().takes_values() {
            continue;
        }
        let label = match arg.get_long() {
            Some(long) => format!("{path} --{long}"),
            None => format!("{path} <{}>", arg.get_id()),
        };
        if NOT_A_SPEC_ID.iter().any(|(l, _)| *l == label) {
            continue;
        }
        if parses_to(arg).as_deref() != Some(want) {
            failures.push(label);
        }
    }
    for sub in cmd.get_subcommands() {
        let sub_path = format!("{path} {}", sub.get_name());
        walk_spec_args(sub, sub_path.trim_start(), want, failures);
    }
}
