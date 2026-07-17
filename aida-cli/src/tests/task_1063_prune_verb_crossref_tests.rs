use clap::{CommandFactory, Parser};

/// Render the --help for a nested `queue <sub>` subcommand.
fn queue_sub_help(sub: &str) -> String {
    let mut cli = crate::cli::Cli::command();
    let queue = cli
        .get_subcommands_mut()
        .find(|c| c.get_name() == "queue")
        .expect("queue subcommand exists");
    let cmd = queue
        .get_subcommands_mut()
        .find(|c| c.get_name() == sub)
        .unwrap_or_else(|| panic!("queue {sub} subcommand exists"));
    cmd.render_long_help().to_string()
}

#[test]
fn queue_prune_help_names_sibling_pruning_verbs() {
    let help = queue_sub_help("prune");
    // Names its own two predicates …
    assert!(
        help.contains("--orphaned"),
        "prune --help must name --orphaned:\n{help}"
    );
    assert!(
        help.contains("--merged"),
        "prune --help must name --merged:\n{help}"
    );
    // … and cross-references the sibling `queue gc` verb.
    assert!(
        help.contains("queue gc"),
        "prune --help must cross-reference `aida queue gc`:\n{help}"
    );
    // States which entry class each predicate targets.
    assert!(
        help.to_lowercase().contains("delete"),
        "prune --help must say --orphaned targets DELETED specs:\n{help}"
    );
    assert!(
        help.to_lowercase().contains("merged"),
        "prune --help must say --merged targets shipped/merged rows:\n{help}"
    );
}

#[test]
fn queue_gc_help_cross_references_prune() {
    let help = queue_sub_help("gc");
    assert!(
        help.contains("queue prune"),
        "gc --help must cross-reference `aida queue prune`:\n{help}"
    );
}

#[test]
fn queue_prune_no_predicate_error_names_the_verbs() {
    // Parse-level: `aida queue prune` (no predicate) is accepted by clap —
    // the guard is a runtime bail, not a clap constraint.
    let parsed = crate::cli::Cli::try_parse_from(["aida", "queue", "prune"]);
    assert!(
        parsed.is_ok(),
        "aida queue prune (no flag) must parse; the guard is a runtime bail"
    );
    // The guard message is the surface an operator sees — assert it names
    // every dead-queue-pruning verb + what each removes.
    let msg = crate::queue_prune_no_predicate_message();
    for frag in ["--orphaned", "--merged", "queue gc", "DELETED"] {
        assert!(
            msg.contains(frag),
            "prune-no-predicate error must name `{frag}`:\n{msg}"
        );
    }
}
