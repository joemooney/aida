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
fn in_place_helpers_rewrite_each_slot() {
    let _map = bind_home();
    let mut single = "STORY-HOME-001".to_string();
    canonicalize(&mut single);
    assert_eq!(single, canonical_of("001"));

    let mut opt = Some("story-home-002".to_string());
    canonicalize_opt(&mut opt);
    assert_eq!(opt.as_deref(), Some(canonical_of("002").as_str()));

    let mut none: Option<String> = None;
    canonicalize_opt(&mut none);
    assert_eq!(none, None);

    let mut list = Some("STORY-HOME-003, TASK-9,story-home-004".to_string());
    canonicalize_csv_opt(&mut list);
    assert_eq!(
        list.as_deref(),
        Some(format!("{}, TASK-9,{}", canonical_of("003"), canonical_of("004")).as_str())
    );
}

#[test]
fn queue_verbs_carry_the_canonical_id() {
    use crate::cli::QueueCommand;
    use clap::Parser;
    #[derive(Parser)]
    struct Q {
        #[clap(subcommand)]
        cmd: QueueCommand,
    }
    let _map = bind_home();
    let parse = |args: &[&str]| {
        let mut q = Q::try_parse_from(std::iter::once("queue").chain(args.iter().copied()))
            .unwrap()
            .cmd;
        crate::queue_cmd::canonicalize_queue_spec_args(&mut q);
        q
    };
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
