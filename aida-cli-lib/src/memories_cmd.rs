// `aida memories check` command cluster, lifted verbatim out of `main.rs`
// (SPIKE-78 pure-movement extraction). The shared drift model
// (`MemoryDriftReport`/`MemoryDriftRow`/`MemoryDriftState`), the pure compute
// core (`compute_memory_drift_into`), and `project_memory_dir` stay in `main.rs`
// because `print_status_memory_drift_section` and the unit tests also consume
// them; this module reaches them via `crate::`.
// trace:SPIKE-78 | ai:claude

use anyhow::Result;
use colored::Colorize;

// trace:STORY-1728 | ai:claude
#[path = "memories_cmd_args.rs"]
pub(crate) mod memories_cmd_args;
pub(crate) use memories_cmd_args::*;

use crate::{
    compute_memory_drift_into, current_version, project_memory_dir, MemoryDriftReport,
    MemoryDriftRow, MemoryDriftState,
};

/// `aida memories check`: compare the local memory pack against the binary's
/// embedded master and report drift. Reads only — never writes. The
/// recommendation (`aida init --with-memories --refresh`) is exact and
/// paste-ready.
// trace:STORY-410 | ai:claude
pub(crate) fn handle_memories_check(verbose: bool, json: bool) -> Result<()> {
    let mem_dir = project_memory_dir()?;
    let report = compute_memory_drift_into(&mem_dir)?;

    if json {
        let rows: Vec<serde_json::Value> = report
            .rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "name": r.name,
                    "label": r.label,
                    "description": r.description,
                    "state": match r.state {
                        MemoryDriftState::Missing => "missing",
                        MemoryDriftState::Stale => "stale",
                        MemoryDriftState::UpToDate => "up-to-date",
                        MemoryDriftState::Edited => "edited",
                        MemoryDriftState::UserOwned => "user-owned",
                    },
                })
            })
            .collect();
        let out = serde_json::json!({
            "memory_dir": mem_dir.display().to_string(),
            "aida_version": current_version(),
            "master_total": report.rows.len(),
            "behind": report.behind(),
            "missing": report.missing(),
            "stale": report.stale(),
            "up_to_date": report.up_to_date(),
            "edited": report.edited(),
            "user_owned": report.user_owned(),
            "rows": rows,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    print_memory_drift(&mem_dir, &report, verbose);
    Ok(())
}

/// Render the human-readable `aida memories check` report. Default mode caps
/// each category at 5 entries; `--verbose` lists them all.
// trace:STORY-410 | ai:claude
fn print_memory_drift(mem_dir: &std::path::Path, report: &MemoryDriftReport, verbose: bool) {
    let max = if verbose { usize::MAX } else { 5 };

    println!();
    println!("  {}: {}", "Memory pack".bold(), mem_dir.display());
    println!(
        "  {} (in this aida binary): {} master memories",
        "Master pack".bold(),
        report.rows.len()
    );
    println!();

    if !mem_dir.exists() {
        println!(
            "  {} no local memory pack yet — all {} master memories are missing.",
            "Note:".yellow(),
            report.rows.len()
        );
        print_memory_drift_recommendation();
        return;
    }

    let print_category = |heading: colored::ColoredString,
                          state: MemoryDriftState,
                          with_desc: bool| {
        let rows: Vec<&MemoryDriftRow> = report.rows.iter().filter(|r| r.state == state).collect();
        if rows.is_empty() {
            return;
        }
        println!("  {} ({}):", heading, rows.len());
        for row in rows.iter().take(max) {
            if with_desc && !row.description.is_empty() {
                println!("    {} — {}", row.label.cyan(), row.description.dimmed());
            } else {
                println!("    {}", row.label.cyan());
            }
        }
        if rows.len() > max {
            println!(
                "    {} (run {} to see all)",
                format!("... {} more", rows.len() - max).dimmed(),
                "aida memories check --verbose".cyan()
            );
        }
        println!();
    };

    print_category(
        "Missing from local".yellow(),
        MemoryDriftState::Missing,
        true,
    );
    print_category(
        "Stale (newer version in master)".yellow(),
        MemoryDriftState::Stale,
        true,
    );
    print_category(
        "Edited locally (kept by --refresh)".blue(),
        MemoryDriftState::Edited,
        false,
    );
    print_category(
        "Your own (no scaffold marker; never overwritten)".dimmed(),
        MemoryDriftState::UserOwned,
        false,
    );

    println!("  {} up to date.", report.up_to_date().to_string().green());
    println!();

    if report.behind() == 0 {
        println!(
            "  {} memory pack is current with this aida binary.",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
        println!();
    } else {
        println!(
            "  {} {} behind master.",
            report.behind().to_string().yellow().bold(),
            if report.behind() == 1 {
                "memory is"
            } else {
                "memories are"
            }
        );
        print_memory_drift_recommendation();
    }
}

/// The exact, paste-ready refresh command.
// trace:STORY-410 | ai:claude
fn print_memory_drift_recommendation() {
    println!();
    println!(
        "  To pull missing/updated memories (keeps your edits):\n    {}",
        "aida init --with-memories --refresh".cyan()
    );
    println!();
}

/// Starter-memory-pack drift discovery (`aida memories check`)
// trace:STORY-410 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "memories")]
pub(crate) struct MemoriesArgs {
    #[clap(subcommand)]
    pub inner: crate::cli::MemoriesCommand,
}

fn run_memories(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <MemoriesArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match &ctx.store {
        None => {
            // STORY-410: `aida memories check` compares the local Claude Code memory
            // pack to the binary's embedded master — no store access needed, so handle
            // it before storage resolution (like init/upgrade). trace:STORY-410
            if let MemoriesArgs {
                inner: MemoriesCommand::Check { verbose, json },
            } = &args
            {
                return crate::command_registry::returned(memories_cmd::handle_memories_check(
                    *verbose, *json,
                ));
            }
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Distributed { .. }) => {
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Git {
            store_path,
            backend,
        }) => {
            use crate::git_backend_cmd::*;
            let (store_path, backend) = (*store_path, *backend);
            let _ = (store_path, backend, &args);
            crate::git_backend_cmd::exit_unsupported_on_git_backend()
        }
        Some(crate::command_registry::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            unreachable!("`aida memories` has no centralized-store form")
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static MEMORIES: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "memories",
        help_after: "init",
        command: <MemoriesArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Store(run_memories),
    };
