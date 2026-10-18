//! `aida record` command cluster (STORY-582) — inspect / prune the durable
//! processing-record trail stored on each requirement. Extracted verbatim from
//! `main.rs` (SPIKE-78); no behavior change. The shared `print_processing_records`
//! renderer stays in `main.rs` (reached via `crate::`).

use crate::print_processing_records;
use aida_core::{DatabaseBackend, Storage};
use anyhow::Result;
use colored::Colorize;

// trace:STORY-1728 | ai:claude
#[path = "record_cmd_args.rs"]
pub(crate) mod record_cmd_args;
pub(crate) use record_cmd_args::*;

pub(crate) fn handle_record_command(
    cmd: &crate::cli::RecordCommand,
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
) -> Result<()> {
    use crate::cli::RecordCommand;
    match cmd {
        RecordCommand::List { spec } => {
            let store = backend.load()?;
            let mut shown = 0usize;
            for req in &store.requirements {
                if req.processing_record.is_empty() {
                    continue;
                }
                if let Some(want) = spec {
                    let hit = [req.agreed_id.as_deref(), req.spec_id.as_deref()]
                        .into_iter()
                        .flatten()
                        .any(|s| s.eq_ignore_ascii_case(want));
                    if !hit {
                        continue;
                    }
                }
                println!("{} — {}", req.display_id().cyan().bold(), req.title);
                print_processing_records(&req.processing_record);
                println!();
                shown += 1;
            }
            if shown == 0 {
                match spec {
                    Some(s) => println!("No processing records on {s}."),
                    None => println!("No processing records recorded yet."),
                }
            }
            Ok(())
        }
        RecordCommand::Prune {
            spec,
            older_than,
            apply,
        } => {
            let cutoff =
                older_than.map(|days| chrono::Utc::now() - chrono::Duration::days(days as i64));
            // Pure decision over the loaded store: how many records each
            // matched spec would lose. Reused for the dry-run report and the
            // atomic write so the two never drift.
            let store = backend.load()?;
            let mut plan: Vec<(String, usize, usize)> = Vec::new(); // (id, remove, keep)
            for req in &store.requirements {
                if req.processing_record.is_empty() {
                    continue;
                }
                if let Some(want) = spec {
                    let hit = [req.agreed_id.as_deref(), req.spec_id.as_deref()]
                        .into_iter()
                        .flatten()
                        .any(|s| s.eq_ignore_ascii_case(want));
                    if !hit {
                        continue;
                    }
                }
                let remove = req
                    .processing_record
                    .iter()
                    .filter(|r| cutoff.map(|c| r.timestamp < c).unwrap_or(true))
                    .count();
                if remove > 0 {
                    plan.push((
                        req.display_id(),
                        remove,
                        req.processing_record.len() - remove,
                    ));
                }
            }

            if plan.is_empty() {
                println!("Nothing to prune.");
                return Ok(());
            }

            let total: usize = plan.iter().map(|(_, r, _)| r).sum();
            let window = older_than
                .map(|d| format!("older than {d}d"))
                .unwrap_or_else(|| "all".to_string());
            println!(
                "{} {} processing record(s) across {} spec(s) ({}):",
                if *apply { "Pruning" } else { "Would prune" },
                total,
                plan.len(),
                window
            );
            for (id, remove, keep) in &plan {
                println!("  {id}: −{remove} (keeps {keep})");
            }

            if !*apply {
                println!("\n{}", "Dry run — pass --apply to write.".dimmed());
                return Ok(());
            }

            let storage = Storage::new(store_path);
            let spec_filter = spec.clone();
            storage.update_atomically(|s| {
                for req in &mut s.requirements {
                    if req.processing_record.is_empty() {
                        continue;
                    }
                    if let Some(want) = spec_filter.as_deref() {
                        let hit = [req.agreed_id.as_deref(), req.spec_id.as_deref()]
                            .into_iter()
                            .flatten()
                            .any(|s| s.eq_ignore_ascii_case(want));
                        if !hit {
                            continue;
                        }
                    }
                    let before = req.processing_record.len();
                    req.processing_record
                        .retain(|r| cutoff.map(|c| r.timestamp >= c).unwrap_or(false));
                    if req.processing_record.len() != before {
                        req.modified_at = chrono::Utc::now();
                    }
                }
            })?;
            println!(
                "{} pruned {} record(s).",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                total
            );
            Ok(())
        }
    }
}

/// Inspect or prune the durable per-spec processing record — the audit
/// trail of what was done + why, captured at completion time.
// trace:STORY-582 | ai:claude — plain `//` keeps the id out of `--help`.
#[derive(clap::Parser, Debug)]
#[clap(name = "record")]
#[clap(hide = true)]
pub(crate) struct RecordArgs {
    #[clap(subcommand)]
    pub inner: crate::cli::RecordCommand,
}

fn run_record(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <RecordArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match &ctx.store {
        None => Ok(crate::command_registry::Flow::Continue),
        Some(crate::command_registry::StoreCtx::Distributed { .. }) => {
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Git {
            store_path,
            backend,
        }) => {
            let (store_path, backend) = (*store_path, *backend);
            match &args {
                RecordArgs { inner: record_cmd } => {
                    // STORY-582: inspect / prune the durable processing-record trail.
                    return crate::command_registry::returned(self::handle_record_command(
                        record_cmd, &backend, store_path,
                    ));
                }
                #[allow(unreachable_patterns)]
                _ => crate::git_backend_cmd::exit_unsupported_on_git_backend(),
            }
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            match &args {
                RecordArgs { inner: _ } => {
                    // STORY-582: the processing-record trail is a git-canonical field.
                    anyhow::bail!(
                    "aida record commands are only available in git-canonical (distributed) mode. \
                     Run `aida init` (defaults to distributed) first."
                );
                }
                #[allow(unreachable_patterns)]
                _ => unreachable!("`aida record` finished before storage init"),
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static RECORD: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "record",
        help_after: "cache",
        command: <RecordArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Store(run_record),
    };
