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

// trace:STORY-1728 | ai:claude
#[path = "node_cmd_args.rs"]
pub(crate) mod node_cmd_args;
pub(crate) use node_cmd_args::*;

use crate::*;

/// Handle `aida node` subcommands. Operates on the orphan-store worktree
/// at `store_path` (typically `.aida-store/`).
// trace:BUG-1561 | ai:claude
pub(crate) fn handle_node_command(cmd: &NodeCommand, store_path: &std::path::Path) -> Result<()> {
    use aida_core::node::NodeRegistry;

    let registry_path = store_path.join("registry").join("nodes.toml");
    let node_config_path = store_path.join(".aida").join("node.toml");

    // trace:BUG-2017 | ai:codex — local repair never claims a global registry/block.
    if !aida_core::allocation_authority::resolve(store_path)?.authoritative {
        match cmd {
            NodeCommand::Acquire {
                id,
                hostname,
                email,
                node_name,
                hijack,
                remote_only,
                ..
            } => {
                anyhow::ensure!(
                    id.is_none()
                        && hostname.is_none()
                        && email.is_none()
                        && node_name.is_none()
                        && hijack.is_none()
                        && !remote_only,
                    "WORK binding is opaque; global registration options require HOME authority"
                );
                let binding = aida_core::work_binding::bootstrap(store_path)?.ok_or_else(|| {
                    anyhow::anyhow!("allocation mode changed during WORK repair; retry")
                })?;
                println!(
                    "WORK node {} ready (node-local allocation).",
                    binding.node_id
                );
                return Ok(());
            }
            NodeCommand::List | NodeCommand::Show { id: None } => {
                if let Some(binding) = aida_core::work_binding::load(store_path)? {
                    println!(
                        "WORK node {} (node-local allocation; no global blocks).",
                        binding.node_id
                    );
                    return Ok(());
                }
            }
            _ => {}
        }
    }
    match cmd {
        NodeCommand::List => {
            let registry = NodeRegistry::load(&registry_path).unwrap_or_default();
            // trace:BUG-2017 | ai:codex — use existing surface; no transport help snapshot overlap.
            let allocation = aida_core::allocation_authority::resolve(store_path)?;
            println!(
                "Allocation: {} (clone common Git directory)",
                if allocation.authoritative {
                    "authoritative"
                } else {
                    "node-local only"
                }
            );
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

            // trace:BUG-2017 | ai:codex
            let allocation = aida_core::allocation_authority::resolve(store_path)?;
            println!(
                "Allocation: {} (clone common Git directory)",
                if allocation.authoritative {
                    "authoritative"
                } else {
                    "node-local only"
                }
            );
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
            // trace:BUG-2017 | ai:codex — legacy global registration is not WORK bootstrap.
            aida_core::allocation_authority::require_global(store_path)?;

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
                Some(TeamCommand::AllowSeat { user, seat }) => {
                    team_cmd::handle_team_allow_seat(store_path, user, seat)
                }
                Some(TeamCommand::DisallowSeat { user, seat }) => {
                    team_cmd::handle_team_disallow_seat(store_path, user, seat)
                }
            };
        }

        // trace:TASK-851 | ai:claude
        NodeCommand::Whoami => return session_misc_cmd::handle_whoami_command(),
    }

    Ok(())
}

/// Manage per-clone node identity (acquire/release node ids in the
/// shared registry, list registered clones). Each clone of an AIDA
/// project gets a unique node id; that id is the namespace for
/// node-aware spec ids (e.g. `FR-<node>-<NNN>`) until the merge gate
/// promotes them to short agreed-ids.
// trace:EPIC-1-052 | ai:claude
// trace:TASK-487 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "node")]
pub(crate) struct NodeArgs {
    #[clap(subcommand)]
    pub inner: crate::cli::NodeCommand,
}

fn run_node(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <NodeArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
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
                NodeArgs { inner: node_cmd } => {
                    return crate::command_registry::returned(self::handle_node_command(
                        node_cmd, store_path,
                    ));
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            match &args {
                NodeArgs { inner: _ } => {
                    anyhow::bail!(
                    "aida node commands are only available in git-canonical (distributed) mode. \
                     Run `aida init` (defaults to distributed) first."
                );
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static NODE: crate::command_registry::CommandReg = crate::command_registry::CommandReg {
    name: "node",
    help_after: "mailbox",
    command: <NodeArgs as clap::CommandFactory>::command,
    policy: |_| crate::command_registry::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::Write,
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::command_registry::Handler::Store(run_node),
};
