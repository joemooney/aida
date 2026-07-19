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

    // Perform import
    let result = export::import_tree_from_file(&mut store, file, options)?;

    // Save the updated store
    storage.save(&store)?;

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
