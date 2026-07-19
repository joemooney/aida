//! `aida rules` command handler, lifted out of `main.rs` (SPIKE-78).
//!
//! Pure movement: the rule-projection engine lives in `crate::rules_sync`;
//! this module just maps the `RulesCommand::Sync` variant to a trace-graph
//! scan and calls `rules_sync::sync` / `sync_review_md`.
// trace:SPIKE-31 | ai:claude

use std::collections::HashSet;

use anyhow::Result;
use colored::Colorize;

use crate::cli;
use crate::find_project_root;
use crate::rules_sync;
use crate::scan_trace_graph;

/// SPIKE-31 entry point: reconcile `.claude/rules/aida-specs/` against the
/// spec graph. Surfaces a small text report (or dry-run preview) — the
/// substrate side of the substrate-as-bouncer compose move.
// trace:SPIKE-31 | ai:claude
pub(crate) fn handle_rules_command(
    cmd: &cli::RulesCommand,
    backend: &aida_core::CachedGitBackend,
) -> Result<()> {
    match cmd {
        cli::RulesCommand::Sync { dry_run, review_md } => {
            let project_root = find_project_root()?;
            // Pull every spec id that has a trace comment by passing the
            // full set of known ids and letting scan_trace_graph filter.
            use aida_core::DatabaseBackend;
            let store = backend.load()?;
            let mut wanted: HashSet<String> = HashSet::new();
            for r in &store.requirements {
                if let Some(id) = r.agreed_id.as_deref() {
                    wanted.insert(id.to_string());
                }
                if let Some(id) = r.spec_id.as_deref() {
                    wanted.insert(id.to_string());
                }
            }
            let raw = scan_trace_graph(&project_root, &wanted);
            let trace_graph: std::collections::HashMap<String, Vec<rules_sync::TracedFile>> = raw
                .into_iter()
                .map(|(k, hits)| {
                    let files = hits
                        .into_iter()
                        .map(|h| rules_sync::TracedFile {
                            path: h.file,
                            symbol: h.symbol,
                        })
                        .collect();
                    (k, files)
                })
                .collect();

            let mut report = rules_sync::sync(&project_root, backend, &trace_graph, *dry_run)?;

            // SPIKE-35: emit REVIEW.md alongside .claude/rules/ when
            // the operator passes --review-md. trace:SPIKE-35 | ai:claude
            if *review_md {
                let r = rules_sync::sync_review_md(&project_root, backend, &trace_graph, *dry_run)?;
                report.review_md = Some(r);
            }

            let prefix = if *dry_run {
                "→ dry-run:".cyan().to_string()
            } else {
                crate::glyph(crate::glyphs::Glyph::Check)
                    .green()
                    .to_string()
            };
            println!(
                "{} {} written, {} unchanged, {} removed",
                prefix,
                report.written.len(),
                report.unchanged.len(),
                report.removed.len(),
            );
            for path in &report.written {
                let rel = path.strip_prefix(&project_root).unwrap_or(path);
                println!("  {} {}", "write".green(), rel.display());
            }
            for path in &report.removed {
                let rel = path.strip_prefix(&project_root).unwrap_or(path);
                println!("  {} {}", "remove".yellow(), rel.display());
            }
            if !report.skipped_no_traces.is_empty() {
                println!(
                    "  {} {} active spec(s) had no trace comments",
                    "skipped:".dimmed(),
                    report.skipped_no_traces.len(),
                );
            }
            if let Some(rmd) = &report.review_md {
                let rel = rmd.path.strip_prefix(&project_root).unwrap_or(&rmd.path);
                if rmd.unchanged {
                    println!(
                        "  {} {} ({} spec(s) included)",
                        "unchanged".dimmed(),
                        rel.display(),
                        rmd.specs_included.len(),
                    );
                } else if rmd.written {
                    let verb = if *dry_run { "write" } else { "wrote" };
                    println!(
                        "  {} {} ({} spec(s) included)",
                        verb.green(),
                        rel.display(),
                        rmd.specs_included.len(),
                    );
                }
            }
        }
    }
    Ok(())
}
