// `aida help commands` — the flat, comprehensive catalog of every command
// and subcommand, one line each. The rows are derived live from the clap
// definition (a recursive walk over `Cli::command()`), NOT hand-maintained,
// so the catalog can never drift from the real surface: a new subcommand
// appears here the moment it exists, a removed one disappears.
// trace:TASK-1098 | ai:claude

use clap::CommandFactory;
use colored::Colorize;

/// One catalog row: the full runnable invocation (`aida queue work`), the
/// first line of its clap `about`, and whether it (or an ancestor) is hidden
/// from the default `--help`.
// trace:TASK-1098 | ai:claude
pub(crate) struct CatalogRow {
    pub path: String,
    pub about: String,
    pub hidden: bool,
}

/// Collect every command and subcommand from the live clap definition,
/// depth-first, as full runnable paths, sorted so each command family's
/// subcommands read directly under it. Clap's auto-generated `help`
/// subcommands are skipped — they are navigation, not surface.
// trace:TASK-1098 | ai:claude
pub(crate) fn catalog_rows() -> Vec<CatalogRow> {
    let mut cmd = crate::cli::Cli::command();
    cmd.build();
    let mut rows = Vec::new();
    collect(&cmd, "aida", false, &mut rows);
    rows.sort_by(|a, b| a.path.cmp(&b.path));
    rows
}

// trace:TASK-1098 | ai:claude
fn collect(cmd: &clap::Command, path: &str, parent_hidden: bool, rows: &mut Vec<CatalogRow>) {
    for sub in cmd.get_subcommands() {
        // Skip clap's auto-generated `help` navigation subcommand at every
        // level; every other name is real surface.
        if sub.get_name() == "help" {
            continue;
        }
        let full = format!("{path} {}", sub.get_name());
        let about_full = sub.get_about().map(|s| s.to_string()).unwrap_or_default();
        let about = short_desc(&about_full);
        let hidden = parent_hidden || sub.is_hide_set();
        rows.push(CatalogRow {
            path: full.clone(),
            about,
            hidden,
        });
        collect(sub, &full, hidden, rows);
    }
}

/// Reduce a clap `about` paragraph to one short scannable line: first line,
/// then first sentence, then a hard cap with an ellipsis. Keeps every row of
/// the catalog one terminal line-ish so 480+ rows stay skimmable.
// trace:TASK-1098 | ai:claude
fn short_desc(about: &str) -> String {
    const MAX: usize = 88;
    let line = about.lines().next().unwrap_or("").trim();
    // Prefer the first sentence when the paragraph runs long.
    let sentence = match line.find(". ") {
        Some(idx) if idx + 1 < MAX => &line[..idx + 1],
        _ => line,
    };
    if sentence.chars().count() <= MAX {
        return sentence.to_string();
    }
    let truncated: String = sentence.chars().take(MAX - 1).collect();
    format!("{}…", truncated.trim_end())
}

/// Print the full catalog. Hidden commands are included (they run fine and
/// show up in usage telemetry — the whole point is a large catalog for
/// finding a forgotten command) but rendered dimmed with a marker so the
/// two tiers stay distinguishable.
// trace:TASK-1098 | ai:claude
pub(crate) fn print_command_catalog() {
    let rows = catalog_rows();
    let total = rows.len();
    let hidden_count = rows.iter().filter(|r| r.hidden).count();

    println!("{}", "📖 AIDA — complete command catalog".bold());
    println!(
        "{}",
        "Every command and subcommand, one line each — derived live from the CLI itself.".dimmed()
    );
    println!();

    // Pad the plain path first, THEN colorize — ANSI escape bytes would
    // otherwise count toward the field width and break column alignment.
    let width = rows.iter().map(|r| r.path.len()).max().unwrap_or(0);
    for row in &rows {
        let padded = format!("{:<width$}", row.path);
        if row.hidden {
            println!(
                "  {}  {} {}",
                padded.dimmed(),
                row.about.dimmed(),
                "· hidden".dimmed()
            );
        } else {
            println!("  {}  {}", padded.green(), row.about);
        }
    }

    println!();
    println!(
        "{} commands ({} hidden — still runnable, just kept out of `--help`).",
        total.to_string().bold(),
        hidden_count
    );
    println!(
        "Run {} for one command's options, or {} for the grouped view.",
        "`aida <command> --help`".bold(),
        "`aida help --all`".bold()
    );
}

#[cfg(test)]
#[path = "tests/help_catalog_tests.rs"]
mod help_catalog_tests;
