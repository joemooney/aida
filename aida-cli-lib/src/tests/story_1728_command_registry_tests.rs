//! Command-registry migration guards. The golden help snapshot pins every
//! command's `--help` text and the full command catalog, so moving a command
//! from the legacy enum into the registry must be byte-identical. Regenerate
//! with `AIDA_UPDATE_SNAPSHOTS=1 cargo test -p aida-cli-lib
//! story_1728_command_registry`, then review the diff.
// trace:STORY-1728 | ai:claude

use super::*;
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
