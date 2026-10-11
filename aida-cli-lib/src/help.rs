//! Tiered help, `aida help-all`, help topics and the user-guide opener (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// The curated "Getting started" set — the core spec loop a newcomer needs to
/// be productive on day one. Kept deliberately short so the default help reads
/// as approachable, not as a 40-command wall. trace:STORY-556
// STORY-758: the first screen LEADS with the magic — `aida why` (code↔decision)
// and the plain-markdown on-ramp — then the spec core. The autonomy verbs
// (zen/ship/solo) are earned depth: still in `aida help --all` and their groups,
// just not the newcomer's first impression. trace:STORY-758 | ai:claude
pub(crate) const GETTING_STARTED: &[(&str, &str)] = &[
    (
        "why",
        "Ask your code why it exists — the decision behind any line",
    ),
    (
        "init",
        "Set up AIDA (`init --minimal` = a folder of markdown, zero setup)",
    ),
    ("show", "Show a spec — its intent, status, and links"),
    ("add", "Capture a new spec (a title + a line of why)"),
    ("list", "List specs"),
    ("search", "Search specs"),
    ("graph", "Trace impact — what a change affects"),
];

/// The full grouped command surface, organized by function. Used by both the
/// tiered help (group headings only, no command rows) and `aida help --all`
/// (every row). Dev-only / orchestrator-internal commands live in their own
/// trailing section so they stay out of the novice's eyeline. trace:STORY-556
pub(crate) fn command_groups() -> &'static [(&'static str, &'static [(&'static str, &'static str)])]
{
    &[
        (
            "Getting started",
            &[
                // STORY-758: the magic leads here too.
                (
                    "why",
                    "Ask your code why it exists — the decision behind any line",
                ),
                ("init", "Set up AIDA in the current project"),
                ("add", "Add a new requirement"),
                ("list", "List requirements"),
                ("show", "Show details for one requirement"),
                ("done", "Mark a spec done — the simple \"I finished it\""),
            ],
        ),
        (
            "Specs",
            &[
                ("edit", "Edit an existing requirement"),
                ("del", "Delete a requirement"),
                ("search", "Search requirements (case-insensitive)"),
                ("grep", "Advanced regex search across requirements"),
                ("comment", "Manage comments on requirements"),
                ("graph", "Query the cross-spec relationship graph"),
                ("rel", "Relationship management"),
                ("trace", "Code-to-requirement traceability"),
                ("archive", "Hide a requirement from default views"),
                ("unarchive", "Restore an archived requirement"),
                // trace:BUG-520 — defer/undefer were real commands missing from any group
                ("defer", "Park a spec as primed/conditional work"),
                ("undefer", "Restore a deferred spec to the active view"),
                ("lint", "EARS-style requirement quality lens"),
            ],
        ),
        (
            "Work & autonomy",
            &[
                ("queue", "Personal work queue"),
                ("backlog", "Backlog grooming views"),
                (
                    "human",
                    "Human-attention views + the unblock grooming prompt",
                ),
                ("rework", "Send a spec back for rework"),
                ("burndown", "Autonomous backlog burn-down"),
                // trace:TASK-879 | ai:claude — the solo loop alongside the other drain verbs.
                (
                    "solo",
                    "Run your project solo — groom → implement → integrate, end-to-end",
                ),
                ("findings", "Triage findings filed by drain phases"),
                ("punt", "Pause a spec at a design-fork"),
                ("questions", "The async decision inbox"),
                ("brief", "Route work to an agent via a pickup brief"),
                ("agent", "Supervised agent launcher"),
                ("triage", "Triage incoming work"),
            ],
        ),
        (
            "Git & lifecycle",
            &[
                ("fetch", "Refresh remote refs (read-only)"),
                ("pull", "Pull code + store; auto-bump done → completed"),
                ("push", "Push code + store"),
                ("rebase", "Rebase code + store"),
                ("pr", "Pull-request helpers"),
                ("review", "Send a held spec for human review"),
                ("changelog", "Refresh / generate CHANGELOG.md"),
            ],
        ),
        (
            "Planning",
            &[
                ("plan", "Verify plans + derive reusable-helper sections"),
                ("ultraplan", "Assemble a rich planner prompt from a spec"),
                ("import-plan", "Import a saved plan under docs/plans/"),
                ("goal", "Derive a machine-checkable /goal condition"),
                ("deps", "Dependency views + trace sweep"),
            ],
        ),
        (
            "Roles & sessions",
            &[
                ("role", "Per-shell role identity (advisor / implementer)"),
                ("session", "Work sessions + worktree leases"),
                ("mailbox", "Inter-agent peer↔peer messaging"),
                ("node", "Per-clone node identity"),
                ("advisor", "Live-advisor registration"),
            ],
        ),
        (
            "Project setup & maintenance",
            &[
                ("scaffold", "Scaffolding management (skills, hooks, MCP)"),
                ("config", "ID configuration (prefixes, formats, etc.)"),
                ("type", "Requirement-type management"),
                ("feature", "Feature management"),
                ("memories", "Starter memory-pack drift check"),
                ("docs", "Project documentation management"),
                ("statusline", "AIDA-aware statusline setup"),
                // trace:BUG-520 — rules/doctor/remote were real commands missing from any
                // group; release/upgrade are maintainer verbs better placed here than under
                // Git & lifecycle.
                (
                    "rules",
                    "Sync Claude Code path-gated rules from the spec graph",
                ),
                ("doctor", "Diagnose and heal multi-agent state drift"),
                ("remote", "Set up a git origin for a project that has none"),
                ("release", "Manage a release"),
                ("upgrade", "Upgrade aida to the latest release"),
                ("export", "Export requirements"),
                ("import", "Import requirements from a tree JSON file"),
            ],
        ),
        (
            "Reporting",
            &[
                ("status", "Where am I right now — unified project view"),
                ("history", "Recent activity"),
                ("report", "Report generation"),
                ("digest", "Narrative advisor report"),
                ("usage", "Inspect locally-recorded CLI usage"),
                ("metrics", "Agent-lift metrics over telemetry"),
                (
                    "criteria",
                    "Acceptance-criteria trace coverage for one spec",
                ),
                ("why", "Explain a spec's current state"),
                // trace:STORY-631 — AI WHY-comprehension, complementary to `why`.
                (
                    "intent",
                    "Plain-terms AI comprehension of why a spec exists",
                ),
                ("user-guide", "Open the user guide in the default browser"),
                // trace:STORY-600 — the CLI manual's when/why beside --help's what.
                ("manual", "Print a command's CLI-manual rationale section"),
                // trace:STORY-667 — the discoverable registry of built-in shortcuts.
                ("alias", "List AIDA's built-in shortcuts"),
            ],
        ),
        (
            "Integrations & servers",
            &[
                ("server", "Connect to or manage a remote AIDA server"),
                ("mcp-serve", "Start MCP server over stdio (for Claude Code)"),
                ("github", "GitHub Issues integration"),
                ("gitlab", "GitLab Issues integration"),
                ("jira", "Jira integration"),
            ],
        ),
        (
            "Storage & data",
            &[
                ("db", "Database management (migrate, sync, merge-gate)"),
                ("cache", "SQLite cache view (rebuild, status)"),
            ],
        ),
        (
            "Working on aida itself",
            &[
                (
                    "dev",
                    "Activate dev binary, run dev servers, install shell helpers",
                ),
                (
                    "help-all",
                    "Full inventory grouped by topic (same as `help --all`)",
                ),
            ],
        ),
    ]
}

/// Bare `aida` and `aida help`: lead with the small "Getting started" set, then
/// list the group headings so the depth is visible but not in the newcomer's
/// face. Full rows are one step away via `aida help --all`. trace:STORY-556
pub(crate) fn print_tiered_help() {
    // STORY-758: lead with the magic, matching the README front door.
    println!("{}", "AIDA — ask your codebase why".bold());
    println!(
        "{}",
        "Point at any line of code and get the decision behind it.".dimmed()
    );
    println!();
    println!("{}", "Usage: aida <command> [options]".dimmed());
    println!();

    println!("{}", "Getting started".cyan().bold());
    for (name, desc) in GETTING_STARTED {
        println!("  {:<10} {}", name.green(), desc);
    }
    // The TUI is the flagship surface (EPIC-26) — invite it from the first
    // screen, but only when it's actually compiled in so a --no-default-features
    // build doesn't list a command that errors. trace:SPIKE-66 | ai:claude
    #[cfg(feature = "tui")]
    println!(
        "  {:<10} {}",
        "tui".green(),
        "Launch the terminal UI — the at-a-glance home for your project"
    );
    println!();

    println!("{}", "More, grouped by topic".cyan().bold());
    // Skip "Getting started" (already shown above) and the dev-only group.
    // STORY-723: render the topic HEADERS bold (not command-green) — green is
    // the runnable-command colour used for the Getting-started rows above, and
    // styling a non-runnable header the same way invited `aida Specs` (which
    // errors). Bold reads as a section label, distinct from a command.
    for (group, _cmds) in command_groups()
        .iter()
        .filter(|(g, _)| *g != "Getting started" && *g != "Working on aida itself")
    {
        println!("  {}", group.bold());
    }
    println!();

    println!(
        "Run {} for the full command list, or {} for one command's options.",
        "`aida help --all`".bold(),
        "`aida <command> --help`".bold()
    );
    // STORY-758: the magic is the entry point; `aida status` is for orientation
    // once you're set up.
    println!(
        "{} — point at code, get the decision behind it.  {} for \"what's going on here?\"",
        "`aida why <file:line>`".bold(),
        "`aida status`".bold()
    );
    // trace:SPIKE-66 | ai:claude
    #[cfg(feature = "tui")]
    println!(
        "{} opens the visual home for your project — at a glance, all at once.",
        "`aida tui`".bold()
    );
}

pub(crate) fn print_help_all() {
    let groups = command_groups();

    println!(
        "{}",
        "AIDA — full command inventory (run `aida <command> --help` for details)".bold()
    );
    println!();
    for (group, cmds) in groups {
        println!("{}", group.cyan().bold());
        for (name, desc) in *cmds {
            println!("  {:<14} {}", name.green(), desc);
        }
        println!();
    }
    println!(
        "Bare `aida` / `aida help` shows the curated Getting-started view. {}",
        "Tip:".bold()
    );
    println!("  - `aida help <topic>` expands a single group (e.g. `aida help queue`)");
    // trace:TASK-1098 | ai:claude
    println!("  - `aida help commands` lists EVERY command and subcommand, one line each");
    println!("  - `aida <topic> --help` works for any command, even hidden ones");
    println!("  - `aida status` is the best entry point for \"what's going on here?\"");
    println!("  - `aida dev shell-init --install` to wire up the `aida` shell wrapper");
}

/// Resolve a user-supplied `<topic>` to one command group. Matches a group name
/// case-insensitively; falls back to a unique case-insensitive prefix so
/// `aida help git` resolves "Git & lifecycle". Returns `Err(Vec)` (the valid
/// topic names) when nothing matches or the prefix is ambiguous.
/// trace:TASK-861 | ai:claude
#[allow(clippy::type_complexity)]
pub(crate) fn resolve_help_topic(
    topic: &str,
) -> Result<(&'static str, &'static [(&'static str, &'static str)]), Vec<&'static str>> {
    let groups = command_groups();
    let needle = topic.trim().to_lowercase();

    // Exact (case-insensitive) name match wins outright.
    if let Some((g, cmds)) = groups.iter().find(|(g, _)| g.to_lowercase() == needle) {
        return Ok((*g, *cmds));
    }

    // Otherwise accept a UNIQUE case-insensitive prefix.
    let matches: Vec<&(&str, &[(&str, &str)])> = groups
        .iter()
        .filter(|(g, _)| g.to_lowercase().starts_with(&needle))
        .collect();
    if matches.len() == 1 {
        let (g, cmds) = matches[0];
        return Ok((*g, *cmds));
    }

    Err(groups.iter().map(|(g, _)| *g).collect())
}

/// `aida help <topic>`: expand one command group — its commands + descriptions —
/// the middle granularity between bare `aida help` (group names only) and
/// `aida help --all` (every group). An unknown/ambiguous topic errors with the
/// list of valid topic names rather than silently falling through.
/// trace:TASK-861 | ai:claude
pub(crate) fn print_help_topic(topic: &str) -> Result<()> {
    match resolve_help_topic(topic) {
        Ok((group, cmds)) => {
            println!("{}", group.cyan().bold());
            for (name, desc) in cmds {
                println!("  {:<14} {}", name.green(), desc);
            }
            println!();
            println!(
                "Run {} for one command's options, or {} for every group.",
                "`aida <command> --help`".bold(),
                "`aida help --all`".bold()
            );
            Ok(())
        }
        Err(valid) => {
            let project_root = find_main_worktree_root()
                .or_else(|_| find_project_root())
                .ok();
            if help_catalog::print_semantic_help_topic(topic, project_root.as_deref()) {
                return Ok(());
            }
            eprintln!();
            eprintln!("Unknown help topic: {}", topic.bold());
            eprintln!();
            eprintln!("Valid topics (try `aida help <topic>`):");
            for name in valid {
                eprintln!("  {}", name.green());
            }
            eprintln!();
            // trace:TASK-1098 | ai:claude
            eprintln!(
                "Or `aida help` for the curated view, {} for everything, {} for the flat catalog.",
                "`aida help --all`".bold(),
                "`aida help commands`".bold()
            );
            anyhow::bail!("unknown help topic: {topic}");
        }
    }
}

#[cfg(test)]
#[path = "tests/help_grouping_tests.rs"]
mod help_grouping_tests;

pub(crate) fn open_user_guide(dark_mode: bool) -> Result<()> {
    // Get the path to the docs directory relative to the executable
    let exe_path = aida_exe_path();

    // Try multiple possible locations for the docs
    let possible_paths = [
        // Relative to executable (for installed binaries)
        exe_path.parent().unwrap().join("../docs"),
        exe_path.parent().unwrap().join("../../docs"),
        // Development paths
        exe_path.parent().unwrap().join("../../../docs"),
        exe_path.parent().unwrap().join("../../../../docs"),
        // Current directory
        std::env::current_dir().unwrap_or_default().join("docs"),
        // Project root (when running from project directory)
        std::path::PathBuf::from("docs"),
    ];

    let filename = if dark_mode {
        "user-guide-dark.html"
    } else {
        "user-guide.html"
    };

    // Find the first path that exists
    let doc_path = possible_paths
        .iter()
        .map(|p| p.join(filename))
        .find(|p| p.exists());

    match doc_path {
        Some(path) => {
            let path_str = path
                .canonicalize()
                .unwrap_or(path.clone())
                .to_string_lossy()
                .to_string();

            // Convert to file:// URL
            let url = format!("file://{}", path_str);

            println!(
                "Opening user guide{}...",
                if dark_mode { " (dark mode)" } else { "" }
            );

            // Try to open in browser using platform-specific commands
            #[cfg(target_os = "linux")]
            {
                std::process::Command::new("xdg-open")
                    .arg(&url)
                    .spawn()
                    .context("Failed to open browser. Try opening manually: {}")?;
            }

            #[cfg(target_os = "macos")]
            {
                std::process::Command::new("open")
                    .arg(&url)
                    .spawn()
                    .context("Failed to open browser")?;
            }

            #[cfg(target_os = "windows")]
            {
                std::process::Command::new("cmd")
                    .args(["/C", "start", &url])
                    .spawn()
                    .context("Failed to open browser")?;
            }

            println!("{}", "User guide opened in browser".green());
            Ok(())
        }
        None => {
            println!("{}", "User guide not found.".yellow());
            println!("Expected location: docs/{}", filename);
            println!("\nTo generate the documentation, run:");
            println!("  ./helper/generate-docs.sh");
            anyhow::bail!("User guide not found")
        }
    }
}
