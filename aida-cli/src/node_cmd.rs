//! `aida node` command cluster (node-identity management).
//!
//! Node acquire / list / show / release / set-owner / set-name — the
//! per-clone node-identity registry surface, plus the `node team` /
//! `node whoami` identity-introspection aliases. Extracted verbatim from
//! `main.rs` (SPIKE-78); no behavior change. The id *allocation* machinery
//! (the dispenser, node registry, block-claim) lives in `aida_core::node` /
//! `aida_core::git_ops` and stays there, reached via `aida_core::`. The
//! CLI-side helpers this handler leans on — `resolve_node_name`,
//! `auto_allocate_initial_blocks`, `read_id_format_policy`,
//! `read_id_counter_scope`, `current_user_id`, `truncate` — are shared with
//! `aida init` (which also acquires a node) and stay in `main.rs`, reached
//! via `crate::`.

use anyhow::Result;
use colored::Colorize;

use crate::*;

pub(crate) fn handle_node_command(cmd: &NodeCommand, store_path: &std::path::Path) -> Result<()> {
    use aida_core::node::NodeRegistry;

    let registry_path = store_path.join("registry").join("nodes.toml");
    let node_config_path = store_path.join(".aida").join("node.toml");

    match cmd {
        NodeCommand::List => {
            let registry = NodeRegistry::load(&registry_path).unwrap_or_default();
            let current_id = if node_config_path.exists() {
                aida_core::NodeConfig::load(&node_config_path)
                    .ok()
                    .map(|c| c.node_id)
            } else {
                None
            };

            if registry.nodes.is_empty() {
                println!("No nodes registered yet. Run `aida node acquire` to claim id 1.");
                return Ok(());
            }

            println!(
                "{:<2}  {:<6}  {:<8}  {:<28}  {:<22}  Registered",
                "", "Node", "User", "Email", "Hostname"
            );
            println!("{}", "─".repeat(100));
            for n in &registry.nodes {
                let marker = if current_id.as_deref() == Some(n.id.as_str()) {
                    "*"
                } else {
                    " "
                };
                let email = n.email.clone().unwrap_or_else(|| "-".into());
                let when = n
                    .registered
                    .with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string();
                println!(
                    "{:<2}  {:<6}  {:<8}  {:<28}  {:<22}  {}",
                    marker,
                    n.id,
                    n.user_id,
                    truncate(&email, 28),
                    truncate(&n.hostname, 22),
                    when
                );
            }
        }

        NodeCommand::Show { id } => {
            let registry = NodeRegistry::load(&registry_path).unwrap_or_default();
            let target_id: String = match id {
                Some(i) => i.clone(),
                None => {
                    if !node_config_path.exists() {
                        anyhow::bail!(
                            "No current node — `.aida/node.toml` does not exist. \
                             Run `aida node acquire` to claim a node id, or pass an id explicitly."
                        );
                    }
                    aida_core::NodeConfig::load(&node_config_path)?.node_id
                }
            };

            let entry = registry.get(&target_id).ok_or_else(|| {
                anyhow::anyhow!("Node {} is not in the shared registry", target_id)
            })?;

            println!("Node {}", entry.id);
            println!("  User ID:    {}", entry.user_id);
            println!("  Hostname:   {}", entry.hostname);
            println!("  Email:      {}", entry.email.as_deref().unwrap_or("-"));
            println!(
                "  Registered: {}",
                entry
                    .registered
                    .with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M:%S %Z")
            );
            if node_config_path.exists() {
                let local = aida_core::NodeConfig::load(&node_config_path)?;
                if local.node_id == entry.id {
                    println!("  Active on this clone: yes (.aida/node.toml matches)");
                }
            }
        }

        NodeCommand::Acquire {
            id: requested_id,
            hostname: hn_override,
            email: email_override,
            node_name,
            force,
            yes,
            hijack,
            remote_only,
        } => {
            // FR-265 remote-only path: backfill a registry entry for some
            // OTHER (legacy) clone and push, without touching this clone's
            // identity. Requires explicit --id/--hostname/--email since the
            // entry isn't about the local clone. trace:FR-265 | ai:claude
            if *remote_only {
                let id = requested_id.clone().ok_or_else(|| {
                    anyhow::anyhow!("--remote-only requires --id <NODE_ID> (explicit; nothing is inferred from this clone)")
                })?;
                let hn = hn_override.clone().ok_or_else(|| {
                    anyhow::anyhow!("--remote-only requires --hostname <HOST> (explicit; nothing is inferred from this clone)")
                })?;
                let email = email_override.clone().ok_or_else(|| {
                    anyhow::anyhow!("--remote-only requires --email <EMAIL> (explicit; nothing is inferred from this clone)")
                })?;
                if let Err(msg) = aida_core::node::validate_node_id(&id) {
                    anyhow::bail!("Invalid node id: {}", msg);
                }
                let user_id = 1;
                println!(
                    "Backfilling registry entry for node {} (hostname={}, email={}) — local identity untouched...",
                    id, hn, email
                );
                let registered = aida_core::git_ops::register_node_remote_only(
                    store_path, id, user_id, &hn, email,
                )?;
                println!(
                    "{} Backfilled node id {} into registry/nodes.toml. This clone's identity ({}) is unchanged.",
                    "".green().bold(),
                    registered,
                    if node_config_path.exists() {
                        aida_core::NodeConfig::load(&node_config_path)
                            .map(|c| c.node_id)
                            .unwrap_or_else(|_| "-".to_string())
                    } else {
                        "none".to_string()
                    }
                );
                return Ok(());
            }

            // STORY-43 hijack path: re-claim an existing node id.
            if let Some(target_id) = hijack {
                let hn = hn_override.clone().unwrap_or_else(hostname);
                let email = email_override
                    .clone()
                    .or_else(|| aida_core::git_ops::git_config_get("user.email").ok());
                let user_id = 1;
                println!(
                    "Hijacking node id '{}' for this clone (hostname={}, email={})...",
                    target_id,
                    hn,
                    email.as_deref().unwrap_or("-")
                );
                let outcome = aida_core::git_ops::hijack_node(
                    store_path,
                    target_id,
                    user_id,
                    &hn,
                    email.clone(),
                )?;
                match &outcome {
                    aida_core::git_ops::HijackOutcome::MarkedInPlace { marker_path } => {
                        println!(
                            "{} Hijacked node id '{}'. Marker dropped at {}",
                            "".green().bold(),
                            target_id,
                            marker_path.display()
                        );
                        println!("  Next `aida` invocation in the old clone will warn the user.");
                    }
                    aida_core::git_ops::HijackOutcome::Reattributed { reason } => {
                        println!(
                            "{} Hijacked node id '{}' (re-attributed: {}).",
                            "".green().bold(),
                            target_id,
                            reason
                        );
                    }
                }
                return Ok(());
            }

            if node_config_path.exists() && !*force {
                let existing = aida_core::NodeConfig::load(&node_config_path)?;
                anyhow::bail!(
                    "This clone already has node id {} (registered {}). \
                     Pass --force to re-acquire (will allocate a new id).",
                    existing.node_id,
                    existing
                        .registered_at
                        .with_timezone(&chrono::Local)
                        .format("%Y-%m-%d")
                );
            }

            let hn = hn_override.clone().unwrap_or_else(hostname);
            let email = email_override
                .clone()
                .or_else(|| aida_core::git_ops::git_config_get("user.email").ok());

            // user_id resolution: for now we use a placeholder of 1 if we
            // can't find a UserRegistry entry. Phase 1 deliberately doesn't
            // touch user identity — that's out of scope. The email stamp
            // in the node entry is the actual identity carrier.
            let user_id = 1;

            // STORY-42 collision UX: when the user requested a specific id
            // (e.g., `--id JM`), probe the registry first. If it's taken,
            // suggest `JM2` and prompt — or auto-accept under `--yes`.
            // trace:STORY-42 | ai:claude
            let effective_id: Option<String> = match requested_id {
                Some(req) => {
                    use aida_core::git_ops::{suggest_free_node_id, NodeIdProbe};
                    match suggest_free_node_id(store_path, req)? {
                        NodeIdProbe::Free => Some(req.clone()),
                        NodeIdProbe::Taken { suggested } => {
                            if *yes {
                                println!(
                                    "Node id '{}' is taken — auto-accepting suggested '{}' (--yes).",
                                    req, suggested
                                );
                                Some(suggested)
                            } else if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                                anyhow::bail!(
                                    "Node id '{}' is taken. Try `aida node acquire --id {}` \
                                     (or pass --yes to auto-accept the suggestion).",
                                    req,
                                    suggested
                                );
                            } else {
                                use std::io::Write;
                                println!("Node id '{}' is already taken in this store.", req);
                                print!("Use suggested '{}' instead? [Y/n/<other-id>] ", suggested);
                                std::io::stdout().flush()?;
                                let mut answer = String::new();
                                std::io::stdin().read_line(&mut answer)?;
                                let answer = answer.trim();
                                let chosen = if answer.is_empty()
                                    || answer.eq_ignore_ascii_case("y")
                                    || answer.eq_ignore_ascii_case("yes")
                                {
                                    suggested
                                } else if answer.eq_ignore_ascii_case("n")
                                    || answer.eq_ignore_ascii_case("no")
                                {
                                    println!("Aborted.");
                                    return Ok(());
                                } else {
                                    answer.to_string()
                                };
                                if let Err(msg) = aida_core::node::validate_node_id(&chosen) {
                                    anyhow::bail!("Invalid node id: {}", msg);
                                }
                                Some(chosen)
                            }
                        }
                    }
                }
                None => None,
            };

            let id_label = match &effective_id {
                Some(n) => format!("id={}", n),
                None => "next available id".to_string(),
            };
            println!(
                "Acquiring node ({}; hostname={}, email={}, user_id={})...",
                id_label,
                hn,
                email.as_deref().unwrap_or("-"),
                user_id
            );

            // STORY-652: owner $USER + friendly node name. The seq for the
            // default-name preview is the effective id when known, else the
            // registry's predicted next id; accepting the default passes None
            // so core stamps the actually-assigned id. trace:STORY-652
            let owner_user = current_user_id(None);
            let predicted_seq = effective_id.clone().unwrap_or_else(|| {
                aida_core::node::NodeRegistry::load(&store_path.join("registry").join("nodes.toml"))
                    .map(|r| r.next_node_id())
                    .unwrap_or_else(|_| "1".to_string())
            });
            let resolved_name =
                resolve_node_name(node_name.as_deref(), &hn, &owner_user, &predicted_seq)?;
            let default_name = aida_core::node::default_node_name(&hn, &owner_user, &predicted_seq);
            let identity = aida_core::git_ops::NodeIdentity {
                name: if resolved_name == default_name {
                    None
                } else {
                    Some(resolved_name)
                },
                user: Some(owner_user.clone()),
            };
            let new_id = aida_core::git_ops::register_node_full_identity(
                store_path,
                effective_id,
                user_id,
                &hn,
                email.clone(),
                identity,
            )?;

            println!(
                "{} Acquired node id {} for this clone. Per-clone identity at {}",
                "".green().bold(),
                new_id,
                node_config_path.display()
            );

            // Phase 3: auto-allocate initial blocks (FR + BUG + TASK +
            // EPIC + STORY + SPIKE) when the project's id_format policy
            // uses blocks. Skipped for `node-aware-only` since that
            // policy never dispenses from blocks.
            // trace:EPIC-1-052 Phase 3 | ai:claude
            // trace:FR-1-073 | ai:claude
            let project_dir = std::env::current_dir().unwrap_or_default();
            let id_policy = read_id_format_policy(&project_dir);
            if id_policy.uses_blocks() {
                match auto_allocate_initial_blocks(store_path, &new_id, &hn, email.as_deref()) {
                    Ok(blocks) if !blocks.is_empty() => {
                        println!(
                            "  Auto-allocated {} block{}: {}",
                            blocks.len(),
                            if blocks.len() == 1 { "" } else { "s" },
                            blocks.join(", ")
                        );
                    }
                    Ok(_) => {
                        // every type already had a block — nothing to do
                    }
                    Err(e) => {
                        eprintln!(
                            "{} Could not auto-allocate initial blocks: {}. \
                             Run `aida db block claim --type <T> --size 100` per type to retry.",
                            "Warning:".yellow().bold(),
                            e
                        );
                    }
                }
            } else {
                println!(
                    "  id_format policy is `{}` — skipping block allocation.",
                    id_policy.as_str()
                );
            }
        }

        NodeCommand::SetOwner { id, user } => {
            // Backfill the owner field on a legacy node entry (+ local
            // node.toml if current). trace:STORY-654 | ai:claude
            aida_core::git_ops::set_node_identity_field(
                store_path,
                id,
                aida_core::git_ops::NodeIdentityField::Owner,
                user,
            )?;
            let touched_local = node_config_path.exists()
                && aida_core::NodeConfig::load(&node_config_path)
                    .map(|c| c.node_id == *id)
                    .unwrap_or(false);
            let local_note = if touched_local {
                " (also updated this clone's .aida/node.toml)"
            } else {
                ""
            };
            println!(
                "{} set owner for node {}: {}{}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                id.bold(),
                user.cyan(),
                local_note.dimmed()
            );
        }

        NodeCommand::SetName { id, name } => {
            // Backfill the friendly name on a legacy node entry (+ local
            // node.toml if current). trace:STORY-654 | ai:claude
            aida_core::git_ops::set_node_identity_field(
                store_path,
                id,
                aida_core::git_ops::NodeIdentityField::Name,
                name,
            )?;
            let touched_local = node_config_path.exists()
                && aida_core::NodeConfig::load(&node_config_path)
                    .map(|c| c.node_id == *id)
                    .unwrap_or(false);
            let local_note = if touched_local {
                " (also updated this clone's .aida/node.toml)"
            } else {
                ""
            };
            println!(
                "{} set name for node {}: {}{}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                id.bold(),
                name.cyan(),
                local_note.dimmed()
            );
        }

        NodeCommand::Release { id, yes } => {
            let registry = NodeRegistry::load(&registry_path).unwrap_or_default();
            let entry = registry
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("Node {} is not in the shared registry", id))?;

            println!("About to release node {}:", entry.id);
            println!("  Hostname: {}", entry.hostname);
            println!("  Email:    {}", entry.email.as_deref().unwrap_or("-"));
            println!(
                "  Note: any IDs already issued by this node remain valid. \
                 The node id is NOT recycled."
            );

            if !*yes {
                use std::io::Write;
                print!("Continue? [y/N] ");
                std::io::stdout().flush()?;
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                    println!("Aborted.");
                    return Ok(());
                }
            }

            let removed = aida_core::git_ops::unregister_node(store_path, id)?;
            if removed {
                println!("{} Node {} removed from registry.", "".green(), id);
            } else {
                println!("Node {} was not in the registry (already removed?).", id);
            }
        }

        // TASK-851: identity introspection under the `node` namespace. Shares
        // the same handlers as the top-level `aida team` / `aida whoami`.
        // trace:TASK-851 | ai:claude
        NodeCommand::Team { json, cmd } => {
            return match cmd {
                None => team_cmd::handle_team_command(store_path, *json),
                Some(TeamCommand::SetRole { user, role }) => {
                    team_cmd::handle_team_set_role(store_path, user, role)
                }
                Some(TeamCommand::MyRole { json }) => {
                    team_cmd::handle_team_my_role(store_path, *json)
                }
                Some(TeamCommand::UnsetRole { user }) => {
                    team_cmd::handle_team_unset_role(store_path, user)
                }
            };
        }

        // trace:TASK-851 | ai:claude
        NodeCommand::Whoami => return session_misc_cmd::handle_whoami_command(),
    }

    Ok(())
}
