//! BUG-2221 slice 1: one pristine clap tree per process. Every pre-dispatch
//! consumer — alias guarding, extension shadowing, the parse, the JSON
//! capability gate and its reparse — clones the same tree instead of
//! re-deriving it; dispatch precedence is unchanged. Structural (build
//! counter), not timing.
// trace:BUG-2221 | ai:claude
use super::*;

#[test]
fn show_builds_one_pristine_tree_and_preserves_dispatch() {
    // Initialize (some earlier test in this process may already have).
    let _ = built_command();
    let builds = pristine_tree_builds();
    assert!(builds >= 1);

    // The full pre-dispatch sequence for `aida show <id> --format json`.
    let names = crate::user_alias::real_subcommand_names();
    let shadowed = crate::external_cmd::shadowed_names();
    let mut argv: Vec<String> = ["aida", "show", "TASK-1", "--format", "json"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let cli = try_parse_from(argv.clone()).expect("show parses");
    crate::enforce_json_format_capability(&mut argv).expect("show supports JSON");
    let reparsed = try_parse_from(argv.clone()).expect("capability reparse parses");
    for _ in 0..3 {
        let _ = built_command();
    }
    assert_eq!(
        pristine_tree_builds(),
        builds,
        "alias guard, shadowing, parse, capability gate and reparse must all \
         reuse the one pristine tree"
    );

    // Precedence preserved: real commands (and their clap aliases) stay
    // un-shadowable by aliases and externals; `show` still dispatches.
    assert!(names.contains("show"));
    assert!(shadowed.contains("show") && shadowed.contains("help"));
    assert!(
        crate::external_cmd::dispatch_name(&argv, &shadowed).is_none(),
        "a builtin never dispatches to an external"
    );
    assert_eq!(cli.command.name(), "show");
    assert_eq!(reparsed.command.name(), "show");

    // Callers mutate only their own clone: a mutated copy leaves the
    // pristine tree untouched.
    let mut mutated = built_command();
    mutated = mutated.subcommand(clap::Command::new("bug-2221-probe"));
    assert!(mutated.find_subcommand("bug-2221-probe").is_some());
    assert!(built_command().find_subcommand("bug-2221-probe").is_none());
}
