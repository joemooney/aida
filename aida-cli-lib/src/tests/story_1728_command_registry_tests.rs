//! Command-registry migration guards. The golden help snapshot pins every
//! command's `--help` text and the full command catalog, so moving a command
//! from the legacy enum into the registry must be byte-identical. Regenerate
//! with `AIDA_UPDATE_SNAPSHOTS=1 cargo test -p aida-cli-lib
//! story_1728_command_registry`, then review the diff.
// trace:STORY-1728 | ai:claude

use super::*;
use crate::stakeholder_role::StakeholderAction;
use std::fmt::Write as _;

fn golden_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tests/golden/cli_help_snapshot.txt")
}

/// Every runnable command path below the root, depth-first in display order,
/// skipping clap's generated `help` subcommands.
fn collect_paths(cmd: &clap::Command, prefix: &[String], out: &mut Vec<Vec<String>>) {
    for sub in cmd.get_subcommands() {
        if sub.get_name() == "help" {
            continue;
        }
        let mut path = prefix.to_vec();
        path.push(sub.get_name().to_string());
        out.push(path.clone());
        collect_paths(sub, &path, out);
    }
}

fn render_help_snapshot() -> String {
    let mut root = built_command()
        .term_width(100)
        .color(clap::ColorChoice::Never);
    root.build();
    let mut paths = vec![Vec::new()];
    collect_paths(&root, &[], &mut paths);
    let mut out = String::new();
    for path in &paths {
        let mut argv = vec!["aida".to_string()];
        argv.extend(path.iter().cloned());
        argv.push("--help".to_string());
        let text = match root.clone().try_get_matches_from(&argv) {
            Err(err) => err.render().to_string(),
            Ok(_) => "<--help did not short-circuit>\n".to_string(),
        };
        let _ = writeln!(out, "===== aida {}", path.join(" "));
        out.push_str(&text);
    }
    let _ = writeln!(out, "===== catalog (hidden included)");
    let rows = crate::help_catalog::catalog_rows(true);
    out.push_str(&serde_json::to_string_pretty(&rows).expect("catalog serializes"));
    out.push('\n');
    out
}

// The surface depends on cargo features (`tui` adds `aida tui`); the golden
// file is recorded with the default feature set CI tests.
// trace:STORY-1728 | ai:claude
#[test]
#[cfg_attr(
    not(feature = "tui"),
    ignore = "golden help snapshot is recorded with default features"
)]
fn help_surface_matches_golden_snapshot() {
    let actual = render_help_snapshot();
    let path = golden_path();
    if std::env::var_os("AIDA_UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing snapshot {}; run with AIDA_UPDATE_SNAPSHOTS=1",
            path.display()
        )
    });
    if expected != actual {
        let first = expected
            .lines()
            .zip(actual.lines())
            .position(|(e, a)| e != a)
            .unwrap_or_else(|| expected.lines().count().min(actual.lines().count()));
        let show = |s: &str| {
            s.lines()
                .skip(first.saturating_sub(3))
                .take(12)
                .collect::<Vec<_>>()
                .join("\n")
        };
        panic!(
            "help surface differs from {} at line {}\n--- expected ---\n{}\n--- actual ---\n{}",
            path.display(),
            first + 1,
            show(&expected),
            show(&actual)
        );
    }
}

/// Top-level commands still on the legacy `Command` enum. It may only go down:
/// a new command registers in `command_registry::REGISTRY` instead, and each
/// migration slice lowers this number in the same change.
const LEGACY_TOP_LEVEL_COMMANDS: usize = 96;

// trace:STORY-1728 | ai:claude
#[test]
fn legacy_command_enum_only_shrinks() {
    let count = Cli::command().get_subcommands().count();
    assert!(
        count <= LEGACY_TOP_LEVEL_COMMANDS,
        "the legacy `Command` enum grew to {count} top-level commands \
         (ratchet {LEGACY_TOP_LEVEL_COMMANDS}); add new commands to \
         command_registry::REGISTRY instead of cli.rs"
    );
    assert_eq!(
        count, LEGACY_TOP_LEVEL_COMMANDS,
        "the legacy `Command` enum shrank to {count}: lower \
         LEGACY_TOP_LEVEL_COMMANDS to match"
    );
}

// trace:STORY-1728 | ai:claude
#[test]
fn registry_is_sorted_unique_and_self_consistent() {
    let names: Vec<&str> = REGISTRY.iter().map(|reg| reg.name).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        names, sorted,
        "REGISTRY must be sorted by name with no duplicates"
    );
    for reg in REGISTRY {
        assert_eq!(
            (reg.command)().get_name(),
            reg.name,
            "registration name must match the clap name it builds"
        );
    }
}

// Every top-level name resolves to exactly one owner, so a command cannot be
// both legacy and registered, and every registration's help anchor exists.
// trace:STORY-1728 | ai:claude
#[test]
fn every_top_level_name_has_exactly_one_owner() {
    let legacy: Vec<String> = Cli::command()
        .get_subcommands()
        .map(|sub| sub.get_name().to_string())
        .collect();
    let built = built_command();
    for sub in built.get_subcommands() {
        let name = sub.get_name();
        let owners = legacy.iter().filter(|n| *n == name).count()
            + REGISTRY.iter().filter(|reg| reg.name == name).count();
        assert_eq!(owners, 1, "`aida {name}` has {owners} owners");
    }
    for reg in REGISTRY {
        assert!(
            built
                .get_subcommands()
                .any(|sub| sub.get_name() == reg.help_after),
            "`{}` follows unknown command `{}`",
            reg.name,
            reg.help_after
        );
    }
}

// Every registered command's policy answers for its bare invocation, so the
// gates in `run()` never see an unclassified registered command.
// trace:STORY-1728 | ai:claude
#[test]
fn every_registered_command_parses_through_the_registry_path() {
    for (argv, name) in [
        (vec!["aida", "away"], "away"),
        (vec!["aida", "home"], "home"),
        (vec!["aida", "_bg-fetch", "fixture-store"], "_bg-fetch"),
    ] {
        let cli = try_parse_from(&argv).unwrap_or_else(|err| panic!("{argv:?}: {err}"));
        let Command::Registered(invocation) = &cli.command else {
            panic!("{argv:?} parsed as legacy {:?}", cli.command);
        };
        assert_eq!(invocation.reg.name, name);
        let _ = invocation.policy();
    }
    // Every registration is reachable by name; `--help` short-circuits any
    // required arguments.
    for reg in REGISTRY {
        let err = try_parse_from(["aida", reg.name, "--help"]).expect_err("help exits");
        assert_eq!(
            err.kind(),
            clap::error::ErrorKind::DisplayHelp,
            "`aida {} --help`",
            reg.name
        );
    }
}

// The presence verbs and the fetch worker keep their legacy policy answers.
// trace:STORY-1728 | ai:claude
#[test]
fn migrated_commands_keep_their_legacy_policy() {
    for argv in [
        vec!["aida", "away"],
        vec!["aida", "home"],
        vec!["aida", "_bg-fetch", "fixture-store"],
    ] {
        let cli = try_parse_from(&argv).unwrap();
        assert_eq!(
            crate::stakeholder_cli_action(&cli.command),
            StakeholderAction::Write,
            "{argv:?}"
        );
        assert!(!crate::command_triggers_per_write_auto_push(&cli.command));
        assert!(!crate::role::role_restore_prompt_should_skip(&cli.command));
    }
}

// trace:STORY-1728 | ai:claude
#[test]
fn bg_fetch_handler_receives_its_positional() {
    let cli = try_parse_from(["aida", "_bg-fetch", "fixture-store"]).unwrap();
    let Command::Registered(invocation) = &cli.command else {
        panic!("expected registered");
    };
    let args = <crate::session_misc_cmd::BgFetchArgs as FromArgMatches>::from_arg_matches(
        &invocation.matches,
    )
    .unwrap();
    assert_eq!(args.store_path, std::path::PathBuf::from("fixture-store"));
    assert!(try_parse_from(["aida", "_bg-fetch"]).is_err());
}

// Top-level options reach both dispatch paths, before or after the command.
// trace:STORY-1728 | ai:claude
#[test]
fn global_args_reach_registered_and_legacy_commands() {
    use crate::cli::OutputFormat;
    for argv in [
        vec!["aida", "--format", "toon", "away"],
        vec!["aida", "away", "--format", "toon"],
    ] {
        let cli = try_parse_from(&argv).unwrap();
        assert!(matches!(cli.command, Command::Registered(_)), "{argv:?}");
        assert!(
            matches!(cli.globals.format, Some(OutputFormat::Toon)),
            "{argv:?}"
        );
    }
    let cli = try_parse_from(["aida", "-p", "demo", "--format", "json", "list"]).unwrap();
    assert!(matches!(cli.command, Command::List { .. }));
    assert!(matches!(cli.globals.format, Some(OutputFormat::Json)));
    assert_eq!(cli.globals.project.as_deref(), Some("demo"));
    let cli = try_parse_from(["aida", "--file", "r.yaml", "home"]).unwrap();
    assert_eq!(cli.globals.file.as_deref(), Some("r.yaml"));
}

// A migrated built-in still shadows an `aida-<name>` extension on PATH.
// trace:STORY-1728 | ai:claude
#[test]
fn registered_commands_shadow_path_extensions() {
    let names = crate::user_alias::real_subcommand_names();
    for reg in REGISTRY {
        assert!(
            names.contains(reg.name),
            "`{}` is not a real subcommand",
            reg.name
        );
    }
    let shadowed = crate::external_cmd::shadowed_names();
    let argv = vec!["aida".to_string(), "away".to_string()];
    assert_eq!(crate::external_cmd::dispatch_name(&argv, &shadowed), None);

    let dir = tempfile::tempdir().unwrap();
    let stub = dir.path().join("aida-away");
    crate::test_exec::write_executable(&stub, "#!/bin/sh\nexit 0\n");
    let installed = crate::external_cmd::discover_in(Some(dir.path().as_os_str()), &shadowed);
    assert!(
        installed.iter().all(|ext| ext.name != "away"),
        "a PATH `aida-away` must not be offered over the built-in"
    );
}

/// Production code reaches the clap tree only through this module, so no
/// consumer can miss a registered command. Test modules may still use the
/// derived `Cli` parser directly.
// trace:STORY-1728 | ai:claude
#[test]
fn production_code_uses_only_the_built_command_tree() {
    const FORBIDDEN: &[&str] = &[
        "Cli::command(",
        "Cli::parse(",
        "Cli::parse_from(",
        "Cli::try_parse",
        "Cli::from_arg_matches",
        "Cli as clap::CommandFactory",
        "Cli as CommandFactory",
    ];
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut hits = Vec::new();
    for entry in std::fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if !name.ends_with(".rs") || name == "command_registry.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for (line_no, line) in production_lines(&text) {
            if FORBIDDEN.iter().any(|pat| line.contains(pat)) {
                hits.push(format!("{name}:{line_no}: {}", line.trim()));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "use crate::command_registry::{{built_command, parse_from, try_parse_from}}:\n{}",
        hits.join("\n")
    );
}

/// Non-comment lines outside inline `#[cfg(test)] mod … { … }` blocks.
fn production_lines(text: &str) -> Vec<(usize, &str)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim() == "#[cfg(test)]" {
            if let Some(next) = lines.get(i + 1) {
                if next.trim_start().starts_with("mod ") && next.trim_end().ends_with('{') {
                    let indent = next.len() - next.trim_start().len();
                    let close = format!("{}}}", " ".repeat(indent));
                    i += 2;
                    while i < lines.len() && lines[i].trim_end() != close {
                        i += 1;
                    }
                    i += 1;
                    continue;
                }
            }
        }
        if !line.trim_start().starts_with("//") {
            out.push((i + 1, line));
        }
        i += 1;
    }
    out
}

// Commands whose read-only verbs stay open to guest stakeholders keep the
// legacy per-verb answers once registered.
// trace:STORY-1728 | ai:claude
#[test]
fn per_verb_stakeholder_policy_matches_the_legacy_table() {
    use StakeholderAction::*;
    for (argv, want) in [
        (vec!["aida", "plan", "verify", "p.md"], Read),
        (vec!["aida", "plan", "verify", "p.md", "--fix"], Write),
        (vec!["aida", "plan", "helpers", "TASK-1"], Read),
        (vec!["aida", "role", "list"], Read),
        (vec!["aida", "role", "active"], Read),
        (vec!["aida", "role", "enter", "advisor"], Role),
        (vec!["aida", "session", "leases"], Read),
        (vec!["aida", "session", "end"], Session),
        (vec!["aida", "worker", "directives"], Read),
        (vec!["aida", "worker", "gc"], Write),
        (vec!["aida", "punts", "list"], Read),
        (vec!["aida", "punts", "analyze"], Write),
    ] {
        let cli = try_parse_from(&argv).unwrap_or_else(|err| panic!("{argv:?}: {err}"));
        assert_eq!(
            crate::stakeholder_cli_action(&cli.command),
            want,
            "{argv:?}"
        );
    }
    let cli = try_parse_from(["aida", "role", "list"]).unwrap();
    assert!(crate::role::role_restore_prompt_should_skip(&cli.command));
}
