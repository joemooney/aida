//! BUG-2189: a subcommand's own `format` arg shared the id of the global
//! `--format` (OutputFormat), so clap skipped propagating the global there and
//! GlobalArgs then downcast the local value: "Mismatch between definition and
//! access of `format`" at parse time for `export`, `report ai-integration` and
//! `config format`. clap's `debug_assert` does not see this (propagation
//! silently yields to the local arg), so the tree walk below checks it directly.
//! A distinct id alone is not enough: a local `--format` long beside the
//! propagated global is ambiguous (the local silently wins, help lists two),
//! so local flags take their own long, as `digest --digest-format` does.
// trace:BUG-2189 | ai:claude

use super::*;

/// Every `<path>: <id>` (or `<path>: --<long>`) where a non-global arg reuses
/// the id or long flag of a global arg defined on an ancestor.
fn shadowed_globals(cmd: &clap::Command, path: &str, inherited: &[String], out: &mut Vec<String>) {
    let mut globals = inherited.to_vec();
    for arg in cmd.get_arguments().filter(|arg| arg.is_global_set()) {
        globals.push(arg.get_id().to_string());
        globals.extend(arg.get_long().map(|long| format!("--{long}")));
    }
    for sub in cmd.get_subcommands() {
        let sub_path = format!("{path} {}", sub.get_name());
        for arg in sub.get_arguments().filter(|arg| !arg.is_global_set()) {
            let id = arg.get_id().to_string();
            if globals.contains(&id) {
                out.push(format!("{sub_path}: {id}"));
            }
            if let Some(long) = arg.get_long().map(|long| format!("--{long}")) {
                if globals.contains(&long) {
                    out.push(format!("{sub_path}: {long}"));
                }
            }
        }
        shadowed_globals(sub, &sub_path, &globals, out);
    }
}

#[test]
fn no_subcommand_arg_shadows_a_global_arg_id() {
    let root = built_command();
    root.clone().debug_assert();
    let mut shadowed = Vec::new();
    shadowed_globals(&root, "aida", &[], &mut shadowed);
    assert!(
        shadowed.is_empty(),
        "subcommand args reuse a global arg id (clap panics decoding the global): {shadowed:?}"
    );
}

#[test]
fn shadow_walk_detects_a_local_arg_reusing_a_global_id() {
    let root = clap::Command::new("t")
        .arg(clap::Arg::new("format").long("format").global(true))
        .subcommand(
            clap::Command::new("a")
                .subcommand(clap::Command::new("b").arg(clap::Arg::new("format").long("fmt")))
                .subcommand(clap::Command::new("c").arg(clap::Arg::new("c_format").long("format"))),
        );
    let mut shadowed = Vec::new();
    shadowed_globals(&root, "t", &[], &mut shadowed);
    assert_eq!(shadowed, ["t a b: format", "t a c: --format"]);
}

#[test]
fn export_parses_its_export_format() {
    let cli = try_parse_from([
        "aida",
        "export",
        "--export-format",
        "tree",
        "--output",
        "out.json",
    ])
    .unwrap();
    assert!(cli.globals.format.is_none());
    let args = cli
        .command
        .registered::<crate::import_export_cmd::ExportArgs>();
    assert_eq!(args.format, "tree");
    assert_eq!(
        args.output.as_deref(),
        Some(std::path::Path::new("out.json"))
    );
    let args = try_parse_from(["aida", "export", "-f", "json"])
        .unwrap()
        .command
        .registered::<crate::import_export_cmd::ExportArgs>();
    assert_eq!(args.format, "json");
    let cli = try_parse_from(["aida", "export", "--output", "out.json"]).unwrap();
    assert_eq!(
        cli.command
            .registered::<crate::import_export_cmd::ExportArgs>()
            .format,
        "mapping"
    );
}

#[test]
fn report_ai_integration_parses_its_report_format() {
    let cli = try_parse_from([
        "aida",
        "report",
        "ai-integration",
        "--report-format",
        "html",
        "--output",
        "r.html",
    ])
    .unwrap();
    assert!(cli.globals.format.is_none());
    match cli
        .command
        .registered::<crate::report_cmd::ReportArgs>()
        .command
    {
        Some(crate::cli::ReportCommand::AiIntegration { format, output, .. }) => {
            assert_eq!(format, "html");
            assert_eq!(output.as_deref(), Some(std::path::Path::new("r.html")));
        }
        other => panic!("expected report ai-integration, got {other:?}"),
    }
    for (argv, expected) in [
        (&["aida", "report", "ai-integration"][..], "markdown"),
        (
            &["aida", "report", "ai-integration", "-f", "html"][..],
            "html",
        ),
    ] {
        match try_parse_from(argv)
            .unwrap()
            .command
            .registered::<crate::report_cmd::ReportArgs>()
            .command
        {
            Some(crate::cli::ReportCommand::AiIntegration { format, .. }) => {
                assert_eq!(format, expected)
            }
            other => panic!("expected report ai-integration, got {other:?}"),
        }
    }
}

#[test]
fn config_format_parses_positional_format() {
    let cli = try_parse_from(["aida", "config", "format", "two"]).unwrap();
    assert!(cli.globals.format.is_none());
    match cli
        .command
        .registered::<crate::config_cmd::ConfigArgs>()
        .inner
    {
        crate::cli::ConfigCommand::Format { format } => assert_eq!(format, "two"),
        other => panic!("expected config format, got {other:?}"),
    }
}

#[test]
fn global_format_applies_on_either_side_of_each_command() {
    for argv in [
        &["aida", "--format", "json", "export", "-f", "tree"][..],
        &["aida", "export", "-f", "tree", "--format", "json"][..],
    ] {
        let cli = try_parse_from(argv).unwrap();
        assert!(matches!(
            cli.globals.format,
            Some(crate::cli::OutputFormat::Json)
        ));
        assert_eq!(
            cli.command
                .registered::<crate::import_export_cmd::ExportArgs>()
                .format,
            "tree"
        );
    }
    for argv in [
        &["aida", "report", "ai-integration", "--format", "toon"][..],
        &["aida", "config", "format", "single", "--format", "toon"][..],
    ] {
        let cli = try_parse_from(argv).unwrap();
        assert!(matches!(
            cli.globals.format,
            Some(crate::cli::OutputFormat::Toon)
        ));
    }
}

/// Run the command's handler at the early dispatch point (no store opened).
fn dispatch_early(argv: &[&str]) -> Result<Flow> {
    let cli = try_parse_from(argv).unwrap();
    let Command::Registered(invocation) = &cli.command;
    invocation.run_early(&cli.globals)
}

/// The former local `--format` spelling now parses as the global output
/// format; each command refuses it, naming its own option, instead of
/// silently emitting its default format.
#[test]
fn old_format_spelling_is_refused_naming_the_new_option() {
    for (argv, expected) in [
        (
            &["aida", "export", "--format", "json"][..],
            "use --export-format json (or -f json)",
        ),
        (
            &["aida", "--format", "toon", "export", "-o", "out.json"][..],
            "use --export-format <mapping|json|spec|impl|tree>",
        ),
        (
            &["aida", "report", "ai-integration", "--format", "json"][..],
            "use --report-format markdown|html (or -f)",
        ),
        (
            &["aida", "config", "format", "two", "--format", "human"][..],
            "aida config format single|two",
        ),
    ] {
        let error = dispatch_early(argv).unwrap_err().to_string();
        assert!(error.contains(expected), "{argv:?}: {error}");
        assert!(error.contains("--format"), "{argv:?}: {error}");
        assert!(!error.contains("BUG-"), "{argv:?}: {error}");
    }
}

#[test]
fn commands_without_the_global_format_still_dispatch() {
    for argv in [
        &["aida", "export", "-f", "json"][..],
        &[
            "aida",
            "report",
            "ai-integration",
            "--report-format",
            "html",
        ][..],
        &["aida", "config", "format", "two"][..],
        // Other report/config subcommands keep accepting the global flag.
        &["aida", "--format", "json", "config", "show"][..],
    ] {
        assert!(
            matches!(dispatch_early(argv).unwrap(), Flow::Continue),
            "{argv:?}"
        );
    }
}

/// `--format json` reaches the JSON-capability gate before any handler; the
/// old spelling there also names the renamed option, while a JSON request
/// from the environment alone, or on another command, keeps the generic error.
#[test]
fn json_gate_names_the_renamed_option_for_the_old_spelling() {
    let gate = |argv: &[&str]| {
        let mut argv: Vec<String> = argv.iter().map(|arg| arg.to_string()).collect();
        crate::json_capability_gate(&mut argv)
            .unwrap_err()
            .to_string()
    };
    for (argv, expected) in [
        (
            &["aida", "export", "--format", "json"][..],
            "use --export-format json (or -f json)",
        ),
        (
            &["aida", "--format", "json", "report", "ai-integration"][..],
            "use --report-format markdown|html (or -f)",
        ),
        (
            &["aida", "config", "format", "two", "--format", "json"][..],
            "aida config format single|two",
        ),
    ] {
        let error = gate(argv);
        assert!(error.contains(expected), "{argv:?}: {error}");
        assert!(!error.contains("BUG-"), "{argv:?}: {error}");
    }
    for argv in [
        &["aida", "export", "-f", "tree"][..],
        &["aida", "config", "numbering", "global", "--format", "json"][..],
    ] {
        assert!(gate(argv).contains("has no JSON projection"), "{argv:?}");
    }
}
