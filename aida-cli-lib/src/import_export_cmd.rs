//! `aida export` / `aida import` command cluster (requirement-tree
//! export/import).
//!
//! JSON tree serialization plus import with skip/rename/replace conflict
//! strategies. Extracted verbatim from `main.rs` (SPIKE-78); no behavior
//! change.

use anyhow::Result;
use colored::Colorize;

use aida_core::export;
use aida_core::Storage;

use crate::get_default_author;

pub(crate) fn handle_export_command(
    storage: &Storage,
    format: &str,
    output: Option<&std::path::Path>,
    id: Option<&str>,
) -> Result<()> {
    // Load requirements
    let store = storage.load()?;

    // The default format is `mapping`, but the documented export -> import
    // round-trip needs `--format tree`. Passing `--id` while leaving the
    // format at its default is almost always "I meant a tree export" — warn
    // rather than silently emit the wrong shape. trace:TASK-778
    if id.is_some() && format != "tree" {
        eprintln!(
            "{}: --id is set but --format is `{}`. The export -> import \
             round-trip needs `--format tree`; --id is ignored for `{}`.",
            "Note".yellow(),
            format,
            format
        );
    }

    match format {
        "mapping" => {
            let output_path = output
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from(".requirements-mapping.yaml"));
            export::generate_mapping_file(&store, &output_path)?;
        }
        "json" => {
            let output_path = output
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from("requirements.json"));
            export::export_json(&store, &output_path)?;
        }
        "spec" | "requirements" => {
            let output_path = output
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from("REQUIREMENTS.md"));
            export::export_requirements_spec(&store, &output_path)?;
        }
        "impl" | "implementation" => {
            let output_path = output
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from("IMPLEMENTATION.md"));
            export::export_implementation_records(&store, &output_path)?;
        }
        "tree" => {
            let root_id = id.ok_or_else(|| {
                anyhow::anyhow!("Tree export requires --id to specify the root requirement")
            })?;
            let output_path = output
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from("tree-export.json"));
            export::export_tree_to_file(&store, root_id, &output_path)?;
            println!(
                "{}: Exported requirement tree to {}",
                "Success".green(),
                output_path.display()
            );
        }
        _ => {
            anyhow::bail!(
                "Unknown export format: {}. Supported formats: mapping, json, spec, impl, tree",
                format
            );
        }
    }

    Ok(())
}

pub(crate) fn handle_import_command(
    storage: &Storage,
    file: &std::path::Path,
    parent_id: Option<&str>,
    on_conflict: &str,
) -> Result<()> {
    use export::{ConflictStrategy, TreeImportOptions};

    // Parse conflict strategy
    let conflict_strategy = match on_conflict.to_lowercase().as_str() {
        "skip" => ConflictStrategy::Skip,
        "rename" => ConflictStrategy::Rename,
        "replace" => ConflictStrategy::Replace,
        _ => {
            anyhow::bail!(
                "Unknown conflict strategy: {}. Supported: skip, rename, replace",
                on_conflict
            );
        }
    };

    // Load current store
    let mut store = storage.load()?;

    // Setup import options
    let options = TreeImportOptions {
        parent_id: parent_id.map(|s| s.to_string()),
        conflict_strategy,
        created_by: Some(get_default_author()),
    };

    let old_ids: std::collections::HashSet<_> = store.requirements.iter().map(|r| r.id).collect();
    // Perform import
    let result = export::import_tree_from_file(&mut store, file, options)?;

    // Save the updated store
    // New specs are filed now, but their copied comments remain historical.
    // trace:STORY-1674 | ai:codex
    let filing = aida_core::provenance::capture();
    for req in &mut store.requirements {
        if !old_ids.contains(&req.id) {
            aida_core::provenance::stamp_with(req, &filing);
        }
    }
    storage.save_imported(&store)?;

    // Print results
    println!("{}: Import completed", "Success".green());
    println!("  Imported: {} requirements", result.imported_count);
    println!("  Skipped:  {} requirements", result.skipped_count);

    if !result.unresolved_refs.is_empty() {
        println!(
            "  {}",
            format!(
                "Unresolved external references: {}",
                result.unresolved_refs.len()
            )
            .yellow()
        );
        for ext_ref in &result.unresolved_refs {
            if let Some(ref spec_id) = ext_ref.original_target_spec_id {
                println!(
                    "    - {} -> {} ({})",
                    spec_id, ext_ref.original_target_uuid, ext_ref.rel_type
                );
            } else {
                println!(
                    "    - {} ({})",
                    ext_ref.original_target_uuid, ext_ref.rel_type
                );
            }
        }
    }

    Ok(())
}

/// Export requirements to different formats
#[derive(clap::Parser, Debug)]
#[clap(name = "export")]
#[clap(hide = true)]
pub(crate) struct ExportArgs {
    /// Output format: mapping, json, spec, impl, tree. The default is
    /// `mapping`; the export -> import round-trip needs `--format tree`.
    // trace:TASK-778
    #[clap(long, short = 'f', default_value = "mapping")]
    pub format: String,

    /// Output file path
    #[clap(long, short = 'o')]
    pub output: Option<std::path::PathBuf>,

    /// Requirement ID (UUID or SPEC-ID) for tree export - exports this requirement and all descendants
    #[clap(long)]
    pub id: Option<String>,
}

fn run_export(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <ExportArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
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
            let _ = (store_path, backend, &args);
            crate::git_backend_cmd::exit_unsupported_on_git_backend()
        }
        Some(crate::command_registry::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            match &args {
                ExportArgs { format, output, id } => {
                    self::handle_export_command(
                        &storage,
                        format,
                        output.as_deref(),
                        id.as_deref(),
                    )?;
                }
                #[allow(unreachable_patterns)]
                _ => unreachable!("`aida export` finished before storage init"),
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static EXPORT: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "export",
        help_after: "regen",
        command: <ExportArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Store(run_export),
    };

/// Import requirements from a tree JSON file
#[derive(clap::Parser, Debug)]
#[clap(name = "import")]
#[clap(hide = true)]
pub(crate) struct ImportArgs {
    /// Path to the tree JSON file to import
    pub file: std::path::PathBuf,

    /// Parent requirement ID (UUID or SPEC-ID) to attach imported tree under
    #[clap(long)]
    pub parent: Option<String>,

    /// Conflict strategy: skip, rename, replace (default: skip)
    #[clap(long, default_value = "skip")]
    pub on_conflict: String,
}

fn run_import(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <ImportArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
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
            let _ = (store_path, backend, &args);
            crate::git_backend_cmd::exit_unsupported_on_git_backend()
        }
        Some(crate::command_registry::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            match &args {
                ImportArgs {
                    file,
                    parent,
                    on_conflict,
                } => {
                    self::handle_import_command(&storage, file, parent.as_deref(), on_conflict)?;
                }
                #[allow(unreachable_patterns)]
                _ => unreachable!("`aida import` finished before storage init"),
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static IMPORT: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "import",
        help_after: "export",
        command: <ImportArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: true,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Store(run_import),
    };
