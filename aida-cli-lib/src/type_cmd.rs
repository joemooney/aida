//! `aida type` command cluster (custom requirement-type management).
//!
//! List the configured requirement types, add a new custom type (name, prefix,
//! and an optional description), and remove one (with an interactive confirm
//! unless `--yes`). Extracted verbatim from `main.rs` (SPIKE-78); no behavior change.
//! The `RequirementType` enum and the `add_requirement_type` store method live
//! in `aida-core`; the `glyph` / `glyphs` helpers stay in `main.rs` and are
//! reached via `crate::`.

use anyhow::Result;
use colored::Colorize;

use aida_core::Storage;

use crate::cli::TypeCommand;

pub(crate) fn handle_type_command(cmd: &TypeCommand, storage: &Storage) -> Result<()> {
    let mut store = storage.load()?;

    match cmd {
        TypeCommand::List => {
            println!("{}", "Requirement Types:".blue().bold());
            println!("{:<20} | {:<10} | Description", "Name", "Prefix");
            println!("{}", "-".repeat(60));

            for type_def in &store.id_config.requirement_types {
                println!(
                    "{:<20} | {:<10} | {}",
                    type_def.name, type_def.prefix, type_def.description
                );
            }
        }
        TypeCommand::Add {
            name,
            prefix,
            description,
        } => {
            let desc = description.clone().unwrap_or_default();
            store.add_requirement_type(name, prefix, &desc)?;
            storage.save(&store)?;
            println!(
                "{} Requirement type '{}' added with prefix '{}'.",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                name,
                prefix.to_uppercase()
            );
        }
        TypeCommand::Remove { name, yes } => {
            // Find the type
            let idx = store.id_config.requirement_types.iter().position(|t| {
                t.name.to_lowercase() == name.to_lowercase() || t.prefix == name.to_uppercase()
            });

            if let Some(idx) = idx {
                let type_def = &store.id_config.requirement_types[idx];

                if !*yes {
                    println!(
                        "About to remove type '{}' (prefix: {})",
                        type_def.name, type_def.prefix
                    );
                    // trace:STORY-809 | ai:claude
                    let card = crate::context_prompt::ContextCard {
                        decision: format!(
                            "whether to remove the custom type '{}' (prefix {})",
                            type_def.name, type_def.prefix
                        ),
                        provenance: vec![
                            "removing a type does not delete requirements already using it, but new ones cannot be filed under it".to_string(),
                        ],
                        answers: vec![
                            "y: the type definition is removed (re-add it to restore)".to_string(),
                            "n: cancel".to_string(),
                        ],
                        recommended_default: "n unless no requirements use this type".to_string(),
                    };
                    let confirm =
                        crate::context_prompt::confirm_with_context("Are you sure?", false, &card)?;
                    if !confirm {
                        println!("Removal cancelled.");
                        return Ok(());
                    }
                }

                let removed = store.id_config.requirement_types.remove(idx);
                storage.save(&store)?;
                println!(
                    "{} Requirement type '{}' removed.",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    removed.name
                );
            } else {
                println!("{} Type '{}' not found.", "!".yellow(), name);
            }
        }
    }

    Ok(())
}

/// Requirement type management commands
#[derive(clap::Parser, Debug)]
#[clap(name = "type", hide = true)]
pub(crate) struct TypeArgs {
    #[clap(subcommand)]
    pub inner: crate::cli::TypeCommand,
}

fn run_type(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <TypeArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
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
                TypeArgs { inner: type_cmd } => {
                    self::handle_type_command(type_cmd, &storage)?;
                }
                #[allow(unreachable_patterns)]
                _ => unreachable!("`aida type` finished before storage init"),
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static TYPE: crate::command_registry::CommandReg = crate::command_registry::CommandReg {
    name: "type",
    help_after: "config",
    command: <TypeArgs as clap::CommandFactory>::command,
    policy: |_| crate::command_registry::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::Type,
        per_write_auto_push: true,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::command_registry::Handler::Store(run_type),
};
