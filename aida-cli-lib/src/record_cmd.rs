//! `aida record` command cluster (STORY-582) — inspect / prune the durable
//! processing-record trail stored on each requirement. Extracted verbatim from
//! `main.rs` (SPIKE-78); no behavior change. The shared `print_processing_records`
//! renderer stays in `main.rs` (reached via `crate::`).

use crate::print_processing_records;
use aida_core::{DatabaseBackend, Storage};
use anyhow::Result;
use colored::Colorize;

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
