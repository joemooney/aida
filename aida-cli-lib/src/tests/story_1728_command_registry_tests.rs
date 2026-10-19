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
    // `aida help all`'s inventory, minus its PATH-dependent extension section.
    let _ = writeln!(out, "===== help all (command groups)");
    for (group, cmds) in crate::help::command_groups() {
        let _ = writeln!(out, "{group}");
        for (name, desc) in *cmds {
            let _ = writeln!(out, "  {name:<14} {desc}");
        }
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
const LEGACY_TOP_LEVEL_COMMANDS: usize = 0;

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
    for reg in REGISTRY.iter().filter(|reg| !reg.help_after.is_empty()) {
        assert!(
            built
                .get_subcommands()
                .any(|sub| sub.get_name() == reg.help_after),
            "`{}` follows unknown command `{}`",
            reg.name,
            reg.help_after
        );
    }
    assert_eq!(
        REGISTRY
            .iter()
            .filter(|reg| reg.help_after.is_empty())
            .count(),
        1,
        "exactly one command comes first (`help_after: \"\"`)"
    );
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
    assert!(matches!(
        cli.command
            .registered::<crate::requirements_cli::ListArgs>(),
        crate::requirements_cli::ListArgs { .. }
    ));
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
        "<Cli as clap::Parser>",
        "Cli as Parser",
    ];
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut hits = Vec::new();
    for path in production_sources(&src) {
        let name = path.strip_prefix(&src).unwrap().display().to_string();
        if name == "command_registry.rs" {
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

/// Every `.rs` file below `dir`, recursively, except test modules (a
/// `tests/` directory, `tests.rs`, or `*_tests.rs`).
fn production_sources(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if path.is_dir() {
            if name != "tests" {
                out.extend(production_sources(&path));
            }
        } else if name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_tests.rs") {
            out.push(path);
        }
    }
    out
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

thread_local! {
    /// What each synthetic handler saw: `(handler, had_store)`.
    static SEEN: std::cell::RefCell<Vec<(&'static str, bool)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn synthetic_reg(run: Handler) -> &'static CommandReg {
    Box::leak(Box::new(CommandReg {
        name: "synthetic",
        help_after: "",
        command: || clap::Command::new("synthetic"),
        policy: |_| CmdPolicy {
            stakeholder: StakeholderAction::Read,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run,
    }))
}

fn synthetic_invocation(run: Handler) -> Invocation {
    let reg = synthetic_reg(run);
    Invocation {
        reg,
        matches: (reg.command)().get_matches_from(["synthetic"]),
    }
}

// Advisor condition (a): an Early handler runs exactly once, at the early
// point, with no store, and always finishes there; it can never reach a
// storage dispatcher. A store-phase handler that defers at the early point
// sees no store there and runs again only when a dispatcher hands it one.
// trace:STORY-1728 | ai:claude
#[test]
fn early_handlers_run_without_a_store_and_continue_reaches_the_store_phase() {
    let globals = try_parse_from(["aida", "status"]).unwrap().globals;
    SEEN.with(|seen| seen.borrow_mut().clear());

    let early = synthetic_invocation(Handler::Early(|_, ctx| {
        SEEN.with(|seen| seen.borrow_mut().push(("early", ctx.store.is_some())));
        Ok(())
    }));
    assert_eq!(early.run_early(&globals).unwrap(), Flow::Return);
    let store_path = std::path::Path::new("fixture-store");
    let reached_store = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let store = StoreCtx::Distributed {
            store_path,
            project_root_hint: None,
        };
        early.run_store(store, Some(&globals))
    }));
    assert!(
        reached_store.is_err(),
        "an Early handler must never run inside a storage dispatcher"
    );
    SEEN.with(|seen| assert_eq!(*seen.borrow(), vec![("early", false)]));

    SEEN.with(|seen| seen.borrow_mut().clear());
    let store = synthetic_invocation(Handler::Store(|_, ctx| {
        SEEN.with(|seen| seen.borrow_mut().push(("store", ctx.store.is_some())));
        Ok(if ctx.store.is_some() {
            Flow::Return
        } else {
            Flow::Continue
        })
    }));
    assert_eq!(store.run_early(&globals).unwrap(), Flow::Continue);
    let ctx = StoreCtx::Distributed {
        store_path,
        project_root_hint: None,
    };
    assert_eq!(store.run_store(ctx, None).unwrap(), Flow::Return);
    SEEN.with(|seen| {
        assert_eq!(*seen.borrow(), vec![("store", false), ("store", true)]);
    });
}

/// The body of `fn run()` in `lib_part1.rs`, up to its closing brace.
fn run_fn_body() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib_part1.rs");
    let text = std::fs::read_to_string(path).unwrap();
    let start = text
        .find("\npub(crate) fn run() -> Result<()> {")
        .expect("lib_part1.rs defines run()");
    let end = start + text[start..].find("\n}\n").expect("run() closes");
    text[start..end].to_string()
}

// Advisor condition (a), dispatcher side: `run()` reaches the early dispatch
// point before it resolves or opens any store, so no Early handler runs with
// storage, the cache or the graph initialised; and each storage stage has
// exactly one dispatch site, so a `Flow::Continue` from the early point is
// served by one store dispatch (legacy or git-backend, with the distributed
// pre-backend stage only a pass-through seam in front of the latter).
// trace:STORY-1728 | ai:claude
#[test]
fn run_dispatches_early_handlers_before_any_store_is_opened() {
    const STORE_INIT: &[&str] = &[
        "requirements_path",
        "Storage::new(",
        "run_on_distributed_store(",
        "handle_git_backend_command(",
        "CachedGitBackend",
        "find_project_root(",
        "open_store",
        "ensure_cache",
    ];
    let body = run_fn_body();
    let early = body
        .find("invocation.run_early(")
        .expect("run() has the early dispatch point");
    let prelude: Vec<&str> = body[..early]
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .filter(|line| STORE_INIT.iter().any(|pat| line.contains(pat)))
        .collect();
    assert!(
        prelude.is_empty(),
        "run() touches storage before the early dispatch point:\n{}",
        prelude.join("\n")
    );
    assert_eq!(
        body.matches("invocation.run_early(").count(),
        1,
        "run() has exactly one early dispatch point"
    );

    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sites = Vec::new();
    for entry in std::fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if !name.ends_with(".rs") || name == "command_registry.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for (_, line) in production_lines(&text) {
            if line.contains("invocation.run_early(") && name != "lib_part1.rs" {
                sites.push(format!("{name}: early"));
            }
            if line.contains("invocation.run_store(") {
                sites.push(name.clone());
            }
        }
    }
    sites.sort();
    assert_eq!(
        sites,
        ["git_backend_cmd.rs", "lib_part1.rs", "store_sync.rs"],
        "one store dispatch per stage: git-backend, legacy, distributed"
    );
    let store_sync =
        std::fs::read_to_string(src.join("store_sync.rs")).expect("store_sync.rs reads");
    let seam = store_sync
        .find("invocation.run_store(")
        .expect("distributed dispatch site");
    let tail = &store_sync[seam..];
    let next_fn = tail.find("\n}\n").expect("run_on_distributed_store closes");
    assert!(
        tail[..next_fn].contains("handle_git_backend_command(store_path, command)"),
        "a Continue at the distributed seam falls through to the git-backend dispatcher"
    );
}

/// `is_write_command` on main before the registry (origin/main aaff1236ad,
/// status_awaiting.rs): the commands the team-identity guard refuses under a
/// shared `default` identity when `AIDA_TEAM_REQUIRE_USER=1`. Every verb of
/// these commands counted; every other command was a read.
const MAIN_TEAM_IDENTITY_WRITES: &[&str] = &[
    "add",
    "archive",
    "brief",
    "comment",
    "defer",
    "do",
    "edit",
    "queue",
    "session",
    "unarchive",
    "undefer",
];

// Advisor condition (c), two-seat 5(b) identity: `team_identity_write` comes
// from each command's registration, and for every registered command and
// every one of its verbs it equals main's `is_write_command`. Any divergence
// changes which commands write under team identity, so it fails here.
// trace:STORY-1728 | ai:claude
#[test]
fn team_identity_write_matches_mains_is_write_command_for_every_verb() {
    let mut checked = 0;
    let mut diverged = Vec::new();
    for reg in REGISTRY {
        let want = MAIN_TEAM_IDENTITY_WRITES.contains(&reg.name);
        let mut paths = vec![Vec::new()];
        collect_paths(&(reg.command)(), &[], &mut paths);
        for path in paths {
            let mut argv = vec![reg.name.to_string()];
            argv.extend(path.iter().cloned());
            // Required arguments are absent; policies answer for an
            // unparseable invocation too (fail-closed).
            let matches = (reg.command)()
                .ignore_errors(true)
                .try_get_matches_from(&argv)
                .unwrap_or_else(|err| panic!("`aida {}`: {err}", argv.join(" ")));
            checked += 1;
            if (reg.policy)(&matches).team_identity_write != want {
                diverged.push(format!("aida {} (main: {want})", argv.join(" ")));
            }
        }
    }
    assert!(
        diverged.is_empty(),
        "team_identity_write diverges from main's is_write_command:\n{}",
        diverged.join("\n")
    );
    assert!(checked > REGISTRY.len(), "verbs were enumerated");
    for name in MAIN_TEAM_IDENTITY_WRITES {
        assert!(
            lookup(name).is_some(),
            "`{name}` is registered, so the parity above covered it"
        );
    }

    // The guard itself reads the registry answer for real argv.
    for (argv, want) in [
        (vec!["aida", "queue", "list"], true),
        (vec!["aida", "comment", "add", "TASK-1", "note"], true),
        (vec!["aida", "edit", "TASK-1", "--status", "approved"], true),
        (vec!["aida", "session", "leases"], true),
        (vec!["aida", "show", "TASK-1"], false),
        (vec!["aida", "rework", "TASK-1"], false),
        (vec!["aida", "status"], false),
        (vec!["aida", "team"], false),
    ] {
        let cli = try_parse_from(&argv).unwrap_or_else(|err| panic!("{argv:?}: {err}"));
        assert_eq!(
            crate::status_awaiting::is_write_command(&cli.command),
            want,
            "{argv:?}"
        );
    }
}

// The team-identity guard's exemptions keep main's classification: `aida
// team` and the machine-readable `aida status --json` stay silent.
// trace:STORY-1728 | ai:claude
#[test]
fn team_identity_guard_exemptions_match_main() {
    let team = parse_command(["aida", "team"]);
    assert_eq!(team.name(), "team");
    let status = parse_command(["aida", "status", "--json"]);
    assert!(status
        .args::<crate::status_cmd::StatusArgs>()
        .is_some_and(|args| args.json));
    let plain = parse_command(["aida", "status"]);
    assert!(!plain
        .args::<crate::status_cmd::StatusArgs>()
        .is_some_and(|args| args.json));
    assert!(team.args::<crate::status_cmd::StatusArgs>().is_none());
}

fn ordering_reg(name: &'static str, help_after: &'static str) -> &'static CommandReg {
    Box::leak(Box::new(CommandReg {
        name,
        help_after,
        command: || clap::Command::new("ordering"),
        policy: |_| CmdPolicy {
            stakeholder: StakeholderAction::Read,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: Handler::Store(|_, _| Ok(Flow::Return)),
    }))
}

// A help_after cycle among registered commands is rejected by name, never
// appended silently at the end of help.
// trace:STORY-1728 | ai:claude
#[test]
fn help_after_cycle_is_rejected() {
    let a = ordering_reg("cyc-a", "cyc-b");
    let b = ordering_reg("cyc-b", "cyc-a");
    let err = help_order(&["status"], &[a, b]).expect_err("a cycle never resolves");
    assert!(
        err.contains("`cyc-a` (help_after `cyc-b`)")
            && err.contains("`cyc-b` (help_after `cyc-a`)"),
        "{err}"
    );
    let unknown = ordering_reg("orphan", "no-such-command");
    let err = help_order(&["status"], &[unknown]).expect_err("an unknown anchor is an error");
    assert!(
        err.contains("`orphan` (help_after `no-such-command`)"),
        "{err}"
    );
}

// Entries sharing one anchor land right after it in name order, whatever the
// input order, and chained anchors resolve across rounds.
// trace:STORY-1728 | ai:claude
#[test]
fn shared_help_after_anchor_places_entries_in_name_order() {
    let alpha = ordering_reg("alpha", "list");
    let beta = ordering_reg("beta", "list");
    let gamma = ordering_reg("gamma", "list");
    let child = ordering_reg("child", "beta");
    let want = vec!["add", "list", "alpha", "beta", "child", "gamma", "show"];
    for regs in [
        [alpha, beta, gamma, child],
        [gamma, child, alpha, beta],
        [child, beta, gamma, alpha],
    ] {
        assert_eq!(
            help_order(&["add", "list", "show"], &regs).unwrap(),
            want,
            "input order {:?}",
            regs.iter().map(|reg| reg.name).collect::<Vec<_>>()
        );
    }
    // `help_after: ""` is the first command, and those siblings sort too.
    let first = ordering_reg("first", "");
    let front = ordering_reg("front", "");
    assert_eq!(
        help_order(&["add", "list"], &[front, alpha, first]).unwrap(),
        vec!["first", "front", "add", "list", "alpha"]
    );
    assert!(
        help_order(&built_legacy_names(), REGISTRY).is_ok(),
        "every real registration resolves"
    );
}

fn built_legacy_names() -> Vec<&'static str> {
    Cli::command()
        .get_subcommands()
        .map(|sub| &*Box::leak(sub.get_name().to_string().into_boxed_str()))
        .collect()
}
