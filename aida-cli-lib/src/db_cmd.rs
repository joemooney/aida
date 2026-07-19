//! `aida db` command cluster — the non-git fallback path.
//!
//! Handles `aida db <path|migrate|info|sync|merge-gate|status|block|
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

            target.save(&store)?;
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
