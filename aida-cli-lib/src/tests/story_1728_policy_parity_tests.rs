//! Policy parity against main's legacy tables. The fixture holds, for every
//! command path and every single-flag variant of it that parses, the three
//! answers main's tables gave before the commands moved into the registry:
//! stakeholder action, per-write auto-push and the role-restore skip. It was
//! captured from origin/main a27fe3fc76 (the last tree where those tables
//! classified the migrated commands) with `AIDA_CAPTURE_MAIN_POLICY=1` and must
//! never be regenerated from this branch: the registry is what it checks.
// trace:STORY-1728 | ai:claude

use super::*;

fn fixture_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/tests/golden/story_1728_main_policy.tsv")
}

/// Value candidates for a required or probed argument, tried in order until
/// clap accepts one.
fn candidates(arg: &clap::Arg) -> Vec<String> {
    let mut out: Vec<String> = arg
        .get_possible_values()
        .iter()
        .filter(|value| !value.is_hide_set())
        .map(|value| value.get_name().to_string())
        .take(1)
        .collect();
    out.extend(["TASK-1", "1", "x.md", "a=b"].map(String::from));
    out
}

fn takes_value(arg: &clap::Arg) -> bool {
    matches!(
        arg.get_action(),
        clap::ArgAction::Set | clap::ArgAction::Append
    )
}

fn spell(arg: &clap::Arg, value: &str, argv: &mut Vec<String>) {
    if arg.is_positional() {
        argv.push(value.to_string());
        return;
    }
    let flag = match (arg.get_long(), arg.get_short()) {
        (Some(long), _) => format!("--{long}"),
        (None, Some(short)) => format!("-{short}"),
        (None, None) => return,
    };
    argv.push(flag);
    if takes_value(arg) {
        argv.push(value.to_string());
    }
}

/// An argv for `path` with every required argument filled, plus `probe` when
/// given; `None` when no candidate values parse (a probe that conflicts with
/// a required argument, or a node that needs a subcommand). A rejected value
/// moves only the argument clap names to its next candidate.
fn synthesize(node: &clap::Command, path: &[String], probe: Option<&clap::Arg>) -> Option<String> {
    let mut args: Vec<&clap::Arg> = node
        .get_arguments()
        .filter(|arg| !arg.is_global_set())
        .filter(|arg| arg.is_required_set() || probe.is_some_and(|p| p.get_id() == arg.get_id()))
        .collect();
    // Options first, then positionals in index order.
    args.sort_by_key(|arg| (arg.is_positional(), arg.get_index().unwrap_or(0)));
    let mut picks = vec![0usize; args.len()];
    loop {
        let mut argv = vec!["aida".to_string()];
        argv.extend(path.iter().cloned());
        for (arg, pick) in args.iter().zip(&picks) {
            spell(arg, &candidates(arg)[*pick], &mut argv);
        }
        let parsed = std::panic::catch_unwind(|| crate::command_registry::try_parse_from(&argv));
        let err = match parsed {
            Ok(Ok(_)) => return Some(argv[1..].join(" ")),
            Ok(Err(err)) => err,
            Err(_) => {
                eprintln!("parse panics: {}", argv.join(" "));
                return None;
            }
        };
        let named = err.get(clap::error::ContextKind::InvalidArg)?.to_string();
        let at = args.iter().position(|arg| arg.to_string() == named)?;
        picks[at] += 1;
        if picks[at] == candidates(args[at]).len() {
            return None;
        }
    }
}

fn answers(argv: &str) -> String {
    let mut full = vec!["aida"];
    full.extend(argv.split(' '));
    let cli = crate::command_registry::try_parse_from(&full).expect("synthesized argv parses");
    format!(
        "{:?}\t{}\t{}",
        crate::stakeholder_role::stakeholder_cli_action(&cli.command),
        crate::command_triggers_per_write_auto_push(&cli.command),
        crate::role::role_restore_prompt_should_skip(&cli.command),
    )
}

/// `argv\tstakeholder\tauto_push\tskip_role_restore` for every command path
/// and every single-flag variant of it that parses, sorted.
fn policy_rows() -> Vec<String> {
    let mut root = crate::command_registry::built_command();
    root.build();
    let mut paths = Vec::new();
    collect_paths(&root, &[], &mut paths);
    let mut rows = std::collections::BTreeSet::new();
    for path in paths {
        let mut node = &root;
        for name in &path {
            node = node.find_subcommand(name).expect("collected path exists");
        }
        let base = synthesize(node, &path, None);
        if base.is_none() && !node.is_subcommand_required_set() {
            eprintln!("no parseable argv: aida {}", path.join(" "));
        }
        let mut argvs = vec![base];
        for arg in node.get_arguments() {
            if arg.is_global_set() || arg.is_required_set() || arg.is_positional() {
                continue;
            }
            if matches!(arg.get_id().as_str(), "help" | "version") {
                continue;
            }
            argvs.push(synthesize(node, &path, Some(arg)));
        }
        for argv in argvs.into_iter().flatten() {
            rows.insert(format!("{argv}\t{}", answers(&argv)));
        }
    }
    rows.into_iter().collect()
}

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

// F2 (advisor round 1 on #2751): every command and verb, and every
// single-flag variant that parses, answers stakeholder action, per-write
// auto-push and the role-restore skip exactly as main's legacy tables did.
// trace:STORY-1728.ac394c52 | ai:claude
#[test]
fn registered_policy_matches_mains_legacy_tables_for_every_verb() {
    if std::env::var("AIDA_CAPTURE_MAIN_POLICY").as_deref() == Ok("1") {
        let mut text = policy_rows().join("\n");
        text.push('\n');
        std::fs::write(fixture_path(), text).unwrap();
        return;
    }
    let want: std::collections::BTreeMap<String, String> = std::fs::read_to_string(fixture_path())
        .expect("main policy fixture exists")
        .lines()
        .map(|line| {
            let (argv, answer) = line.split_once('\t').expect("argv\\tanswers");
            (argv.to_string(), answer.to_string())
        })
        .collect();
    assert!(want.len() > 600, "fixture covers the command surface");
    let mut diverged = Vec::new();
    let mut checked = 0;
    for (argv, answer) in &want {
        let mut full = vec!["aida"];
        full.extend(argv.split(' '));
        // Main's argv must still parse here (the golden help snapshot pins
        // the surface), so a parse failure is itself a divergence.
        if crate::command_registry::try_parse_from(&full).is_err() {
            diverged.push(format!("aida {argv}: no longer parses"));
            continue;
        }
        checked += 1;
        let got = answers(argv);
        if &got != answer {
            diverged.push(format!("aida {argv}\n    main: {answer}\n    here: {got}"));
        }
    }
    assert!(
        diverged.is_empty(),
        "{} of {} invocations diverge from main's legacy policy tables \
         (stakeholder\\tauto_push\\tskip_role_restore):\n{}",
        diverged.len(),
        want.len(),
        diverged.join("\n")
    );
    assert_eq!(checked, want.len());
}
