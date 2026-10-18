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

/// Sync Claude Code path-gated rules from the spec graph. For each
/// active spec (In Progress or Done) with spec-id markers in the code,
/// emit `.claude/rules/aida-specs/<SPEC-ID>.md` with a `paths:` glob
/// matching the marked files. Claude Code's runtime loads the rule
/// on-demand when the implementer reads or edits one of those files —
/// so the spec's scope + acceptance land in context exactly when
/// load-bearing.
// trace:SPIKE-31 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "rules")]
pub(crate) struct RulesArgs {
    #[clap(subcommand)]
    pub inner: crate::cli::RulesCommand,
}

fn run_rules(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <RulesArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match &ctx.store {
        None => Ok(crate::command_registry::Flow::Continue),
        Some(crate::command_registry::StoreCtx::Distributed { .. }) => {
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Git {
            store_path,
            backend,
        }) => {
            use crate::git_backend_cmd::*;
            let (store_path, backend) = (*store_path, *backend);
            match &args {
                RulesArgs { inner: rules_cmd } => {
                    // trace:SPIKE-31 | ai:claude
                    return crate::command_registry::returned(self::handle_rules_command(
                        rules_cmd, backend,
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
                RulesArgs { inner: _ } => {
                    // `aida rules` reads the cache projection — git-canonical only.
                    // trace:SPIKE-31 | ai:claude
                    anyhow::bail!(
                        "aida rules requires the distributed git-canonical store \
                     (run `aida init` to migrate, or this project is on the \
                     deprecated --centralized backend)"
                    );
                }
                #[allow(unreachable_patterns)]
                _ => unreachable!("`aida rules` finished before storage init"),
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static RULES: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "rules",
        help_after: "docs",
        command: <RulesArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Store(run_rules),
    };
