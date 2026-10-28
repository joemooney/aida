//! `aida adoption`: inspect, refresh, bootstrap, inventory and remove the
//! local projection of the shared adoption plane. Every subcommand works
//! on the current worktree and never writes a product-branch file.
// trace:STORY-1741 | ai:claude

#[path = "adoption_cmd_args.rs"]
pub(crate) mod adoption_cmd_args;
pub(crate) use adoption_cmd_args::*;

use aida_core::adoption_migration::{self, Proposal};
use aida_core::adoption_projection::{self, AdoptionState, Outcome, State};
use anyhow::Result;
use std::path::Path;

fn run_adoption(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> Result<()> {
    let args = <AdoptionArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let cwd = std::env::current_dir()?;
    match args.inner {
        AdoptionCommand::Status => status(&cwd),
        AdoptionCommand::Refresh => refresh(&cwd),
        AdoptionCommand::Bootstrap { revision } => {
            let outcome = adoption_projection::bootstrap(&cwd, &revision)?;
            println!(
                "Verified: this worktree's projection holds adoption plane revision {}{}.",
                outcome.commit,
                if outcome.written {
                    ""
                } else {
                    " (already projected)"
                }
            );
            Ok(())
        }
        AdoptionCommand::Inventory => inventory(&cwd),
        AdoptionCommand::Uninstall { dry_run } => uninstall(&cwd, dry_run),
    }
}

fn status(cwd: &Path) -> Result<()> {
    if let AdoptionState::Unknown(error) = adoption_projection::adoption_state(cwd) {
        anyhow::bail!("Could not read the adoption plane: {error}");
    }
    let kept = |kept: Option<adoption_projection::KeptConfig>| {
        kept.map(|k| format!("\nNote: {}.", k.note()))
            .unwrap_or_default()
    };
    match adoption_projection::inspect(cwd)? {
        State::NotAdopted => println!(
            "Not adopted: this clone has no adoption/settings.toml on an aida-store plane."
        ),
        State::NotProjected { plane } => println!(
            "Adopted, not yet projected here. The plane is at {plane}; run \
             `aida adoption refresh`."
        ),
        State::Current {
            commit,
            config_kept,
        } => println!(
            "Current: the projection holds plane revision {commit}.{}",
            kept(config_kept)
        ),
        State::Stale {
            projected,
            plane,
            config_kept,
        } => println!(
            "Stale: the projection holds {projected}, the plane is at {plane}; run \
             `aida adoption refresh`.{}",
            kept(config_kept)
        ),
        State::Damaged { commit, paths } => println!(
            "Damaged: projected files from {commit} were edited, removed or left mid-refresh: \
             {}. Move any change you need to the plane or your user config, delete the \
             files, then run `aida adoption refresh`.",
            paths.join(", ")
        ),
    }
    Ok(())
}

fn refresh(cwd: &Path) -> Result<()> {
    match adoption_projection::sync(cwd)? {
        None => println!("Not adopted: nothing to project."),
        Some(Outcome {
            commit,
            written,
            config_kept,
        }) => {
            if written {
                println!("Projected adoption plane revision {commit}.");
            } else {
                println!("Already current at adoption plane revision {commit}.");
            }
            if let Some(kept) = config_kept {
                println!("Note: {}.", kept.note());
            }
        }
    }
    Ok(())
}

fn inventory(cwd: &Path) -> Result<()> {
    let inv = adoption_migration::inventory(cwd)?;
    let Some(commit) = &inv.commit else {
        println!("The product branch has no commits; nothing is tracked.");
        return Ok(());
    };
    if inv.entries.is_empty() {
        println!("No AIDA files are tracked at {commit}; nothing to migrate.");
        return Ok(());
    }
    println!("AIDA files tracked at {commit}:");
    for proposal in [
        Proposal::Remove,
        Proposal::RemoveAfterPublishing,
        Proposal::Review,
        Proposal::Keep,
    ] {
        let rows: Vec<_> = inv
            .entries
            .iter()
            .filter(|e| e.proposal == proposal)
            .collect();
        if rows.is_empty() {
            continue;
        }
        println!();
        println!("{} ({}):", capitalized(proposal.label()), rows.len());
        for e in rows {
            println!("  {} [{}]: {}", e.path, e.class.label(), e.detail);
        }
    }
    if let Some(split) = inv.config.as_ref().filter(|s| !s.settings.is_empty()) {
        println!();
        println!(
            "Proposed {} for the aida-store plane:",
            aida_core::adoption_plane::SETTINGS_PATH
        );
        for line in split.settings_toml()?.lines() {
            println!("  {line}");
        }
    }
    println!();
    println!(
        "Dry run: nothing was written. Removing tracked files is a separate change to the \
         product branch, reviewed like any other."
    );
    Ok(())
}

fn uninstall(cwd: &Path, dry_run: bool) -> Result<()> {
    let removal = adoption_projection::uninstall(cwd, dry_run)?;
    let hooks = aida_core::git_ops::remove_shipped_aida_hooks(cwd, dry_run)?;
    let verb = if dry_run { "Would remove" } else { "Removed" };
    for path in &removal.removed {
        println!("{verb} {path}");
    }
    for path in &hooks.removed {
        println!("{verb} the AIDA git hook {}", path.display());
    }
    for (path, why) in &removal.kept {
        println!("Kept {path}: {why}");
    }
    for (path, why) in &hooks.kept {
        println!("Kept the git hook {}: {why}", path.display());
    }
    if removal.removed.is_empty() && hooks.removed.is_empty() {
        println!("Nothing to remove: no adoption projection or AIDA git hook here.");
    }
    println!(
        "The aida-store branch and worktree, the adoption locator, this clone's node \
         identity and every product file are left as they are. While the clone stays \
         adopted, `aida agent` and `aida adoption refresh` project it again."
    );
    if dry_run {
        println!("Dry run: nothing was removed.");
    }
    Ok(())
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

// trace:STORY-1741 | ai:claude
pub(crate) static ADOPTION: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "adoption",
        help_after: "init",
        command: <AdoptionArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Early(run_adoption),
    };
