//! `aida db` command cluster — the non-git fallback path.
//!
//! Handles `aida db <path|migrate|info|sync|merge-gate|status|block|audit-ids|
//! retire-legacy-ids|check|reconcile-status|workspace-init|export-git>`
//! for non-git-backed stores (YAML / SQLite / Postgres). For a git-backed
//! store the interesting arms (sync / merge-gate / reconcile-status) are
//! dispatched inline in the main command match against the shared two-leg
//! git-mirror + auto-bump machinery in `aida_core` — this handler is only
//! reached when the backend is NOT git-canonical, so those arms just point
//! the user at `aida --file <dir> db ...`. Extracted verbatim from `main.rs`
//! (SPIKE-78); no behavior change.

use anyhow::Result;
use colored::Colorize;

// trace:STORY-1728 | ai:claude
#[path = "db_cmd_args.rs"]
pub(crate) mod db_cmd_args;
pub(crate) use db_cmd_args::*;

use aida_core::DatabaseBackend;

use crate::cli::DbCommand;

pub(crate) fn handle_db_command(
    cmd: &DbCommand,
    requirements_path: &std::path::PathBuf,
) -> Result<()> {
    match cmd {
        DbCommand::Path => {
            // trace:FR-1-076 | ai:claude
            println!("{}", requirements_path.display());
        }
        DbCommand::Migrate {
            from,
            to,
            output,
            force,
        } => {
            // trace:REQ-0231,FR-0316 | ai:claude:high
            use aida_core::create_backend;

            let source_format = match from.to_lowercase().as_str() {
                "yaml" | "yml" => "yaml",
                "sqlite" | "db" => "sqlite",
                "postgres" | "postgresql" | "pg" => "postgres",
                _ => {
                    println!(
                        "{} Invalid source format '{}'. Use 'yaml', 'sqlite', or 'postgres'.",
                        "!".red(),
                        from
                    );
                    return Ok(());
                }
            };

            let target_format = match to.to_lowercase().as_str() {
                "yaml" | "yml" => "yaml",
                "sqlite" | "db" => "sqlite",
                "postgres" | "postgresql" | "pg" => "postgres",
                _ => {
                    println!(
                        "{} Invalid target format '{}'. Use 'yaml', 'sqlite', or 'postgres'.",
                        "!".red(),
                        to
                    );
                    return Ok(());
                }
            };

            if source_format == target_format {
                println!("{} Source and target formats are the same.", "!".yellow());
                return Ok(());
            }

            // Handle PostgreSQL migrations
            if target_format == "postgres" {
                let conn_string = output.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("PostgreSQL migration requires --output with a connection string (e.g., postgres://user:pass@host:5432/db)")
                })?;

                println!(
                    "Migrating from {} to PostgreSQL...",
                    requirements_path.display()
                );

                let source_backend = create_backend(requirements_path, None)?;
                let count = aida_core::migrate_to_postgres(source_backend.as_ref(), conn_string)?;

                println!(
                    "{} Successfully migrated {} requirements to PostgreSQL",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    count
                );
                return Ok(());
            }

            if source_format == "postgres" {
                let conn_string = requirements_path.to_string_lossy();
                if !conn_string.starts_with("postgres://")
                    && !conn_string.starts_with("postgresql://")
                {
                    println!("{} For PostgreSQL source, use --file with a connection string (e.g., postgres://user:pass@host:5432/db)", "!".red());
                    return Ok(());
                }

                let target_ext = if target_format == "yaml" {
                    "yaml"
                } else {
                    "db"
                };
                let target_path = output
                    .as_ref()
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| {
                        std::path::PathBuf::from(format!("requirements.{}", target_ext))
                    });

                if target_path.exists() && !*force {
                    println!(
                        "{} Target file '{}' already exists. Use --force to overwrite.",
                        "!".yellow(),
                        target_path.display()
                    );
                    return Ok(());
                }

                println!("Migrating from PostgreSQL to {}...", target_path.display());

                let target_backend = create_backend(&target_path, None)?;
                let count =
                    aida_core::migrate_from_postgres(&conn_string, target_backend.as_ref())?;

                println!(
                    "{} Successfully migrated {} requirements to '{}'",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    count,
                    target_path.display()
                );
                return Ok(());
            }

            // Standard YAML <-> SQLite migration
            let target_ext = if target_format == "yaml" {
                "yaml"
            } else {
                "db"
            };
            let target_path = output
                .as_ref()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| requirements_path.with_extension(target_ext));

            if target_path.exists() && !*force {
                println!(
                    "{} Target file '{}' already exists. Use --force to overwrite.",
                    "!".yellow(),
                    target_path.display()
                );
                return Ok(());
            }

            println!(
                "Migrating from {} to {}...",
                requirements_path.display(),
                target_path.display()
            );

            let count = if source_format == "yaml" {
                aida_core::migrate_yaml_to_sqlite(requirements_path, &target_path)?
            } else {
                aida_core::migrate_sqlite_to_yaml(requirements_path, &target_path)?
            };

            println!(
                "{} Successfully migrated {} requirements to '{}'",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                count,
                target_path.display()
            );
        }
        DbCommand::Info => {
            // trace:REQ-0231,FR-0316 | ai:claude:high
            use aida_core::{create_backend, BackendType};

            let backend = create_backend(requirements_path, None)?;
            let store = backend.load()?;

            println!("{}", "Database Information".bold());
            println!("{}", "─".repeat(40));
            println!("Path:        {}", requirements_path.display());
            println!("Backend:     {}", backend.backend_type());
            println!("Name:        {}", store.name);
            println!("Title:       {}", store.title);
            println!("Description: {}", store.description);
            println!();
            println!("{}", "Statistics".bold());
            println!("{}", "─".repeat(40));
            println!("Requirements: {}", store.requirements.len());
            println!("Users:        {}", store.users.len());
            println!("Features:     {}", store.features.len());
            println!("Baselines:    {}", store.baselines.len());

            match backend.backend_type() {
                BackendType::Sqlite => {
                    println!();
                    println!("{}", "Concurrency Support".bold());
                    println!("{}", "─".repeat(40));
                    println!("Store Version:  {}", store.store_version);
                    println!("WAL Mode:       Enabled (recommended for concurrent access)");
                    println!("Optimistic Locking: Supported");
                }
                BackendType::Postgres => {
                    println!();
                    println!("{}", "Concurrency Support".bold());
                    println!("{}", "─".repeat(40));
                    println!("Store Version:  {}", store.store_version);
                    println!("Connection Pool: r2d2 (max 10 connections)");
                    println!("Optimistic Locking: Supported");
                    println!("JSONB:          Native PostgreSQL JSON storage");
                }
                BackendType::Yaml => {
                    println!();
                    println!("{}", "Note".bold());
                    println!("{}", "─".repeat(40));
                    println!("Consider migrating to SQLite or PostgreSQL for concurrent access.");
                }
                BackendType::Git => {
                    println!();
                    println!("{}", "Distributed Storage".bold());
                    println!("{}", "─".repeat(40));
                    println!("Backend:        Git-backed sharded YAML");
                    println!("Object files:   objects/TYPE/NNN/SPEC-ID.yaml");
                    println!("Sync:           git push/pull");

                    // TASK-36: show block utilization so users can spot an
                    // imminent exhaustion before it bites at `aida add` time.
                    //
                    // BUG-115: remaining sums across every non-exhausted
                    // block of the type (pre-fix only used the highest-
                    // numbered block's `remaining`, so a near-empty
                    // higher block masked healthy lower ones and vice
                    // versa).
                    // trace:TASK-36 | trace:BUG-115 | ai:claude
                    let blocks_path = requirements_path.join("registry").join("blocks.yaml");
                    if let Ok(registry) = aida_core::BlockRegistry::load(&blocks_path) {
                        if !registry.blocks.is_empty() {
                            println!();
                            println!("{}", "Agreed-id blocks".bold());
                            println!("{}", "─".repeat(40));
                            use std::collections::BTreeMap;
                            let mut by_prefix: BTreeMap<String, Vec<&aida_core::AgreedIdBlock>> =
                                BTreeMap::new();
                            for b in &registry.blocks {
                                by_prefix
                                    .entry(b.type_prefix.to_uppercase())
                                    .or_default()
                                    .push(b);
                            }
                            for (prefix, blocks) in &by_prefix {
                                let pad = format!("{:<8}", format!("{}:", prefix));
                                let total: u32 =
                                    blocks.iter().map(|b| b.range_end - b.range_start + 1).sum();
                                let remaining: u32 = blocks
                                    .iter()
                                    .filter(|b| !b.is_exhausted())
                                    .map(|b| b.remaining())
                                    .sum();
                                let active_count =
                                    blocks.iter().filter(|b| !b.is_exhausted()).count();
                                if active_count == 0 {
                                    let last_end =
                                        blocks.iter().map(|b| b.range_end).max().unwrap_or(0);
                                    println!(
                                        "  {} {}  (last: {}-{}; falling back to node-aware)",
                                        pad,
                                        "EXHAUSTED".red().bold(),
                                        prefix,
                                        last_end,
                                    );
                                } else {
                                    let issued = total - remaining;
                                    println!(
                                        "  {} {}/{}  ({} remaining, {} block{})",
                                        pad,
                                        issued,
                                        total,
                                        remaining,
                                        blocks.len(),
                                        if blocks.len() == 1 { "" } else { "s" },
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        DbCommand::Sync { .. } => {
            println!(
                "{} Sync is only available for git-backed stores. Use: aida --file <dir> db sync --pull --push",
                "!".yellow()
            );
        }
        DbCommand::MergeGate => {
            println!(
                "{} Merge gate is only available for git-backed stores. Use: aida --file <dir> db merge-gate",
                "!".yellow()
            );
        }
        DbCommand::Status => {
            println!(
                "{} Status is only available for git-backed stores. Use: aida --file <dir> db status",
                "!".yellow()
            );
        }
        DbCommand::Block { .. } => {
            println!(
                "{} Block commands are only available for git-backed stores. Use: aida --file <dir> db block ...",
                "!".yellow()
            );
        }
        DbCommand::RetireLegacyIds { .. } => {
            println!(
                "{} retire-legacy-ids only applies to git-backed stores.",
                "!".yellow()
            );
        }
        // trace:TASK-80 | ai:claude
        DbCommand::Check { .. } => {
            println!(
                "{} db check only applies to git-backed stores. Run `aida init` to migrate.",
                "!".yellow()
            );
        }
        // trace:TASK-226 | ai:claude
        DbCommand::ReconcileStatus { .. } => {
            println!(
                "{} db reconcile-status only applies to git-backed stores. Run `aida init` to migrate.",
                "!".yellow()
            );
        }
        // trace:TASK-1426 | ai:claude
        DbCommand::MigrateRelatedEdges { .. } => {
            println!(
                "{} db migrate-related-edges only applies to git-backed stores.",
                "!".yellow()
            );
        }
        // trace:BUG-2017 | ai:claude
        DbCommand::AuditIds(_) => {
            anyhow::bail!("db audit-ids only applies to git-backed stores.");
        }
        DbCommand::WorkspaceInit { name, remote } => {
            let cwd = std::env::current_dir()?;
            let ws_name = name.as_deref().unwrap_or_else(|| {
                cwd.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("workspace")
            });

            println!("{}", "Initializing AIDA workspace...".bold());

            let manifest =
                aida_core::workspace::init_workspace(&cwd, ws_name, None, remote.as_deref())?;

            println!();
            println!(
                "{} Workspace '{}' initialized",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                manifest.name
            );
            println!();
            println!("  {}:", "Repos discovered".bold());
            for repo in &manifest.repos {
                println!("    {} ({})", repo.path, repo.name);
            }
            println!("  {}:", "Store".bold());
            println!("    {}/", manifest.store_path);
            println!();
            println!("  {}:", "Usage from any repo".bold());
            println!("    {}", "cd <repo> && aida list".cyan());
            println!(
                "    {}",
                "cd <repo> && aida add --title \"...\" --type functional".cyan()
            );
        }
        DbCommand::ExportGit { output } => {
            let output_path = std::path::PathBuf::from(output);

            // Load from current backend
            let source_backend = aida_core::create_backend(requirements_path, None)?;
            let store = source_backend.load()?;

            // Create target git backend
            let target = aida_core::GitBackend::new(&output_path)?;

            if !aida_core::git_ops::is_git_repo(&output_path) {
                aida_core::git_ops::init(&output_path)?;
                let git_name = aida_core::git_ops::git_config_get("user.name")
                    .unwrap_or_else(|_| "AIDA".to_string());
                let git_email = aida_core::git_ops::git_config_get("user.email")
                    .unwrap_or_else(|_| "aida@localhost".to_string());
                aida_core::git_ops::configure_user(&output_path, &git_name, &git_email)?;
            }

            // trace:STORY-1674 | ai:codex — source snapshots do not fence destination writes.
            let mut transferred = store.clone();
            transferred.loaded_objects = None;
            target.save_imported(&transferred)?;
            println!(
                "{} Exported {} requirements to git store at {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                store.requirements.len(),
                output_path.display()
            );
        }
    }

    Ok(())
}

/// Database management commands
#[derive(clap::Parser, Debug)]
#[clap(name = "db")]
#[clap(hide = true)]
pub(crate) struct DbArgs {
    #[clap(subcommand)]
    pub inner: crate::cli::DbCommand,
}

/// The push leg of `aida db sync --push`: publish `push_sha` (the commit the
/// local window captured under the store write lock).
pub(crate) fn db_sync_push_leg(
    store_path: &std::path::Path,
    branch: &str,
    mut push_sha: Option<String>,
) -> anyhow::Result<()> {
    // A clone whose push hubs exclude origin pushes the store to those hubs
    // instead and never attempts origin. The first hub is its store
    // upstream: a rejection there takes the same rebase-and-retry path as
    // origin's, rebasing onto that hub.
    // trace:STORY-B6B1278890774EF993D2EBC327862470-097 | ai:claude
    let policy = crate::store_local::push_hubs(store_path);
    let redirect = crate::store_local::redirected_store_hubs(&policy, |r| {
        aida_core::git_ops::has_remote(store_path, r)
    });
    let upstream = match &redirect {
        None => {
            println!("Pushing to origin/{}...", branch);
            "origin".to_string()
        }
        Some(hubs) => {
            let Some(first) = hubs.first() else {
                anyhow::bail!(
                    "this clone's push hubs (`aida remote push-hubs`) name no remote \
                     configured here and exclude origin; the store was not pushed"
                );
            };
            println!("Pushing to {} (this clone's push hubs)...", hubs.join(", "));
            first.clone()
        }
    };
    // One push attempt; Ok(false) is the upstream's rejection. A redirected
    // push fails outright when a later hub did not take it.
    let push_once = |sha: &str| -> anyhow::Result<bool> {
        let Some(hubs) = &redirect else {
            return aida_core::git_ops::push_exact(store_path, "origin", sha, branch);
        };
        let out = crate::store_local::push_store_to_hubs(store_path, branch, sha, hubs);
        for hub in &out.pushed {
            println!("  pushed → {hub}");
        }
        if out.first_rejected {
            return Ok(false);
        }
        if !out.failures.is_empty() {
            anyhow::bail!("store push failed: {}", out.failures.join("; "));
        }
        Ok(true)
    };
    // trace:TASK-1717 | ai:claude — push the commit captured under
    // the lock, by SHA. A rejection retakes the lock, rebases the
    // CURRENT store (keeping any commit another writer made since)
    // and pushes the new head once; a second rejection fails.
    let pushed = match push_sha.as_deref() {
        None => Err(anyhow::anyhow!("store has no commit to push")),
        Some(sha) => push_once(sha),
    };
    let origin_sha = match pushed {
        Ok(true) => {
            println!("  Push complete.");
            push_sha.clone()
        }
        Ok(false) => {
            println!("  Push rejected. Pulling and retrying...");
            let sha = aida_core::db::with_store_write_lock(store_path, || {
                aida_core::git_ops::pull_rebase(store_path, &upstream, branch).map_err(|e| {
                    anyhow::anyhow!(
                        "Pull failed: {}",
                        crate::store_pull_failure_hint(store_path, &e.to_string())
                    )
                })?;
                crate::ensure_no_spec_id_collisions_canonical(store_path)?;
                aida_core::git_ops::head_sha(store_path)
            })?;
            if !push_once(&sha)? {
                anyhow::bail!(
                    "Push rejected again after rebasing onto {upstream}/{branch}; \
                     your commits are kept locally. Re-run `aida db sync --pull --push`."
                );
            }
            println!("  Push complete after rebase.");
            push_sha = Some(sha);
            push_sha.clone()
        }
        Err(e) if redirect.is_some() => return Err(e),
        Err(e) => {
            eprintln!("  Push failed: {}", e);
            None
        }
    };
    if redirect.is_some() {
        // The hubs were the push; origin's mirror fan-out does not apply.
        return Ok(());
    }

    // TASK-1096: fan out the store push to every configured mirror
    // remote so a clone can't silently leave one hub behind — the
    // drift-prevention leg. trace:TASK-1096 | ai:claude
    // trace:TASK-1717 | ai:claude — only the commit origin accepted.
    if let Some(sha) = origin_sha {
        if let Some(project_root) = store_path.parent() {
            crate::fan_out_store_mirror_push(store_path, branch, &sha, project_root);
        }
    }
    Ok(())
}

fn run_db(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <DbArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match &ctx.store {
        None => Ok(crate::command_registry::Flow::Continue),
        // The audit reads committed trees from the store path alone, before any
        // backend opens. trace:BUG-2017 | ai:claude
        Some(crate::command_registry::StoreCtx::Distributed { store_path, .. }) => match &args {
            DbArgs {
                inner: DbCommand::AuditIds(audit),
            } => {
                crate::db_audit_ids::run(store_path, audit)?;
                Ok(crate::command_registry::Flow::Return)
            }
            _ => Ok(crate::command_registry::Flow::Continue),
        },
        Some(crate::command_registry::StoreCtx::Git {
            store_path,
            backend,
        }) => {
            use crate::git_backend_cmd::*;
            let (store_path, backend) = (*store_path, *backend);
            match &args {
                DbArgs {
                    inner: DbCommand::Path,
                } => {
                    // trace:FR-1-076 | ai:claude
                    println!("{}", store_path.display());
                }
                DbArgs {
                    inner: DbCommand::Info,
                } => {
                    let store = backend.load()?;
                    println!("{}: Git (sharded YAML)", "Backend".bold());
                    println!("{}: {}", "Path".bold(), store_path.display());
                    println!("{}: {}", "Requirements".bold(), store.requirements.len());
                    println!("{}: {}", "Users".bold(), store.users.len());

                    let file_count =
                        aida_core::object_store::list_objects(&store_path.join("objects"))
                            .map(|l| l.len())
                            .unwrap_or(0);
                    println!("{}: {}", "Object files".bold(), file_count);

                    // Show git status
                    if aida_core::git_ops::is_git_repo(store_path) {
                        let has_changes =
                            aida_core::git_ops::has_changes(store_path).unwrap_or(false);
                        let status_str = if has_changes {
                            "uncommitted changes"
                        } else {
                            "clean"
                        };
                        println!("{}: {}", "Git".bold(), status_str);
                        if let Ok(sha) = aida_core::git_ops::head_sha(store_path) {
                            println!("{}: {}", "HEAD".bold(), sha);
                        }
                    }

                    // TASK-36: surface agreed-id block utilization so users spot
                    // imminent exhaustion before it bites at `aida add` time.
                    // Aggregates across nodes (sum within each type prefix).
                    //
                    // BUG-115: remaining sums across every non-exhausted block
                    // of the type. The pre-fix code reported only the highest-
                    // numbered block's `remaining`, so a near-empty lower block
                    // looked invisible and a near-empty higher block masked
                    // healthy lower ones.
                    // trace:TASK-36 | trace:BUG-115 | ai:claude
                    let blocks_path = store_path.join("registry").join("blocks.yaml");
                    if let Ok(registry) = aida_core::BlockRegistry::load(&blocks_path) {
                        if !registry.blocks.is_empty() {
                            println!();
                            println!("{}", "Agreed-id blocks".bold());
                            use std::collections::BTreeMap;
                            let mut by_prefix: BTreeMap<String, Vec<&aida_core::AgreedIdBlock>> =
                                BTreeMap::new();
                            for b in &registry.blocks {
                                by_prefix
                                    .entry(b.type_prefix.to_uppercase())
                                    .or_default()
                                    .push(b);
                            }
                            for (prefix, blocks) in &by_prefix {
                                let pad = format!("{:<8}", format!("{}:", prefix));
                                let total: u32 =
                                    blocks.iter().map(|b| b.range_end - b.range_start + 1).sum();
                                let remaining: u32 = blocks
                                    .iter()
                                    .filter(|b| !b.is_exhausted())
                                    .map(|b| b.remaining())
                                    .sum();
                                let active_count =
                                    blocks.iter().filter(|b| !b.is_exhausted()).count();
                                if active_count == 0 {
                                    let last_end =
                                        blocks.iter().map(|b| b.range_end).max().unwrap_or(0);
                                    println!(
                                        "  {} {}  (last: {}-{}; falling back to node-aware)",
                                        pad,
                                        "EXHAUSTED".red().bold(),
                                        prefix,
                                        last_end
                                    );
                                } else {
                                    let issued = total - remaining;
                                    println!(
                                        "  {} {}/{}  ({} remaining, {} block{})",
                                        pad,
                                        issued,
                                        total,
                                        remaining,
                                        blocks.len(),
                                        if blocks.len() == 1 { "" } else { "s" }
                                    );
                                }
                            }
                        }
                    }
                }
                // Phase 1: Sync command
                DbArgs {
                    inner:
                        DbCommand::Sync {
                            pull,
                            push,
                            message,
                        },
                } => {
                    if !aida_core::git_ops::is_git_repo(store_path) {
                        anyhow::bail!("Not a git repository: {}", store_path.display());
                    }

                    // trace:TASK-1717 | ai:claude — the pending commit, the pull
                    // --rebase and its own cleanup, the collision scan and the
                    // capture of the commit to push are ONE store-writer transaction
                    // under the store write lock, so no spec edit, queue write,
                    // mailbox digest or second sync can touch the index/HEAD between
                    // them. The push, mirrors, auto-bump and maintenance run after
                    // the lock is released.
                    let window = aida_core::db::with_store_write_lock(store_path, || {
                        db_sync_local_window(store_path, *pull, *push, message.as_deref())
                    })?;
                    let branch = window.branch.clone();
                    let push_sha = window.head;

                    if let Some(conflicts) = &window.conflicts {
                        if !conflicts.is_empty() {
                            println!();
                            println!("{}", "Conflicts detected:".red().bold());
                            for conflict in conflicts {
                                println!();
                                print!("{}", conflict);
                            }
                            println!();
                            println!(
                            "Resolve with: aida edit <ID> --title/--status/... to pick the version you want."
                        );
                        }

                        // STORY-86: also scan the code repo's default
                        // branch for any spec-referencing commits that
                        // arrived (possibly via a separate `git pull`)
                        // and bump matching Done specs. `db sync --pull`
                        // is decoupled from `git pull`, so we don't know
                        // a pre_sha here — fall back to HEAD~50. The
                        // `status == Done` guard inside the helper keeps
                        // this idempotent. trace:STORY-86 | ai:claude
                        if auto_bump_enabled() {
                            if let Some(project_root) = store_path.parent() {
                                let storage = Storage::new(store_path);
                                match auto_bump_done_to_completed(
                                    project_root,
                                    store_path,
                                    None,
                                    &storage,
                                ) {
                                    Ok(flips) => print_auto_bump_summary(&flips),
                                    Err(e) => {
                                        eprintln!(
                                            "  {} auto-bump failed: {}",
                                            "Warning:".yellow().bold(),
                                            e
                                        );
                                    }
                                }
                            }
                        }
                    }

                    if *push {
                        db_sync_push_leg(store_path, &branch, push_sha)?;
                    }

                    if !pull && !push {
                        println!("Use --pull and/or --push to sync with remote.");
                        println!("  aida db sync --pull --push");
                    }

                    // TASK-1033: opportunistic store maintenance after a sync that
                    // touched the store — ensure the lowered gc.auto is set, then
                    // `git gc --auto` (a cheap no-op unless the threshold is exceeded).
                    // Best-effort; never fails the sync.
                    if *pull || *push {
                        aida_core::git_ops::opportunistic_store_gc(store_path);
                    }
                }
                // Status
                DbArgs {
                    inner: DbCommand::Status,
                } => {
                    let store = backend.load()?;

                    let total = store.requirements.len();
                    let with_agreed = store
                        .requirements
                        .iter()
                        .filter(|r| r.agreed_id.is_some())
                        .count();
                    let without_agreed = total - with_agreed;

                    println!("{}", "Distributed Store Status".bold());
                    println!("{}", "─".repeat(45));
                    println!("{:<20} {}", "Path:", store_path.display());
                    println!("{:<20} {}", "Requirements:", total);
                    println!("{:<20} {}", "With agreed ID:", with_agreed);
                    println!("{:<20} {}", "Pending merge-gate:", without_agreed);

                    if aida_core::git_ops::is_git_repo(store_path) {
                        let has_changes =
                            aida_core::git_ops::has_changes(store_path).unwrap_or(false);
                        let head =
                            aida_core::git_ops::head_sha(store_path).unwrap_or_else(|_| "?".into());
                        let branch = aida_core::git_ops::current_branch(store_path)
                            .unwrap_or_else(|_| "?".into());

                        println!("{:<20} {}", "Branch:", branch);
                        println!("{:<20} {}", "HEAD:", head);
                        println!(
                            "{:<20} {}",
                            "Working tree:",
                            if has_changes {
                                "uncommitted changes"
                            } else {
                                "clean"
                            }
                        );

                        let remote_ok =
                            aida_core::git_ops::is_remote_reachable(store_path, "origin");
                        println!(
                            "{:<20} {}",
                            "Remote:",
                            if remote_ok {
                                "reachable"
                            } else {
                                "not configured or unreachable"
                            }
                        );
                    }

                    // Show dispenser state
                    if let Ok(disp) = load_dispenser(store_path) {
                        if let Ok(state) = disp.state() {
                            let mode_str = match &state.mode {
                                aida_core::IdMode::Centralized => "centralized".to_string(),
                                aida_core::IdMode::Distributed { node_id } => {
                                    format!("distributed (node {})", node_id)
                                }
                            };
                            println!("{:<20} {}", "ID mode:", mode_str);
                            let total_dispensed: u32 = state.sequences.values().sum();
                            println!("{:<20} {}", "IDs dispensed:", total_dispensed);
                        }
                    }
                }
                // Merge gate: assign agreed IDs
                DbArgs {
                    inner: DbCommand::MergeGate,
                } => {
                    if !aida_core::git_ops::is_git_repo(store_path) {
                        anyhow::bail!("Not a git repository: {}", store_path.display());
                    }

                    // STORY-647: team RBAC guardrail — the merge gate assigns the agreed
                    // short IDs (an advisor act by default, tunable via
                    // `[team.permissions] merge_gate`). Advisor authority (TTY / live
                    // drain / advisor role) bypasses; `merge-gate` carries no `--force`
                    // flag, so a non-advisor seats the role to proceed. trace:STORY-647
                    enforce_team_gate(permissions::GatedOp::MergeGate, false)?;

                    let assignments = aida_core::git_ops::merge_gate(store_path)?;

                    if assignments.is_empty() {
                        println!("All objects already have agreed IDs.");
                    } else {
                        println!("Assigned {} agreed ID(s):", assignments.len());
                        for (node_id, agreed_id) in &assignments {
                            println!("  {} → {}", node_id, agreed_id.green().bold());
                        }
                    }
                }
                // trace:FR-2-005 | ai:claude
                DbArgs {
                    inner: DbCommand::Block { subcommand },
                } => {
                    handle_block_command(subcommand, store_path)?;
                }
                // trace:FR-1-071 | ai:claude
                DbArgs {
                    inner: DbCommand::RetireLegacyIds { dry_run },
                } => {
                    handle_retire_legacy_ids(backend, store_path, *dry_run)?;
                }
                // trace:TASK-226 | ai:claude
                DbArgs {
                    inner:
                        DbCommand::ReconcileStatus {
                            since,
                            spec,
                            dry_run,
                        },
                } => {
                    handle_db_reconcile_status(
                        store_path,
                        since.as_deref(),
                        spec.as_deref(),
                        *dry_run,
                    )?;
                }
                // trace:TASK-80 | ai:claude
                DbArgs {
                    inner: DbCommand::Check { collisions, repair },
                } => {
                    if !*collisions {
                        anyhow::bail!(
                        "`aida db check` requires a check flag. Try `--collisions` (currently the only supported audit)."
                    );
                    }
                    handle_db_check_collisions(backend, store_path, *repair)?;
                }
                // trace:BUG-2017 | ai:claude
                DbArgs {
                    inner: DbCommand::AuditIds(audit),
                } => {
                    crate::db_audit_ids::run(store_path, audit)?;
                }
                // trace:TASK-1426 | ai:claude
                DbArgs {
                    inner: DbCommand::MigrateRelatedEdges { dry_run, json },
                } => {
                    let outcome = crate::related_edge_migration::run_migration(backend, *dry_run)?;
                    if *json {
                        println!("{}", crate::cache_output::json_pretty(&outcome)?);
                    } else {
                        print!(
                            "{}",
                            crate::related_edge_migration::render_outcome(&outcome)
                        );
                    }
                }
                // Phase 3: Export to git backend
                DbArgs {
                    inner: DbCommand::ExportGit { output },
                } => {
                    let output_path = std::path::PathBuf::from(output);

                    // Load from whatever backend we're using (this handler is already git,
                    // but the export command is also available from the main CLI path)
                    let store = backend.load()?;

                    // Create the target git backend
                    let target = aida_core::GitBackend::new(&output_path)?;

                    // Initialize git repo if not already
                    if !aida_core::git_ops::is_git_repo(&output_path) {
                        aida_core::git_ops::init(&output_path)?;
                        let git_name = aida_core::git_ops::git_config_get("user.name")
                            .unwrap_or_else(|_| "AIDA".to_string());
                        let git_email = aida_core::git_ops::git_config_get("user.email")
                            .unwrap_or_else(|_| "aida@localhost".to_string());
                        aida_core::git_ops::configure_user(&output_path, &git_name, &git_email)?;
                    }

                    // trace:STORY-1674 | ai:codex — source snapshots do not fence destination writes.
                    let mut transferred = store.clone();
                    transferred.loaded_objects = None;
                    target.save_imported(&transferred)?;
                    println!(
                        "Exported {} requirements to {}",
                        store.requirements.len(),
                        output_path.display()
                    );
                }
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
                DbArgs { inner: db_cmd } => {
                    self::handle_db_command(db_cmd, &requirements_path.to_path_buf())?;
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static DB: crate::command_registry::CommandReg = crate::command_registry::CommandReg {
    name: "db",
    help_after: "feature",
    command: <DbArgs as clap::CommandFactory>::command,
    policy: db_cli_policy,
    run: crate::command_registry::Handler::Store(run_db),
};

// Per-verb policy, carried over from the legacy stakeholder and per-write
// auto-push tables. An unparseable invocation gets the fail-closed answer.
// trace:STORY-1728 | ai:claude
fn db_cli_policy(matches: &clap::ArgMatches) -> crate::command_registry::CmdPolicy {
    use crate::cli::{BlockCommand, DbCommand};
    use crate::stakeholder_role::StakeholderAction as SA;
    let args = <DbArgs as clap::FromArgMatches>::from_arg_matches(matches).ok();
    let inner = args.as_ref().map(|args| &args.inner);
    crate::command_registry::CmdPolicy {
        stakeholder: match inner {
            Some(
                DbCommand::Path
                | DbCommand::Info
                | DbCommand::Status
                | DbCommand::AuditIds(_)
                | DbCommand::Block {
                    subcommand: BlockCommand::List | BlockCommand::Status | BlockCommand::Verify,
                },
            ) => SA::Read,
            Some(DbCommand::MergeGate) => SA::MergeGate,
            _ => SA::Database,
        },
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    }
}
