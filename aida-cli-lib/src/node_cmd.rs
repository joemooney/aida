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

// trace:BUG-B6B1278890774EF993D2EBC327862470-026 | ai:codex
#[path = "node_inventory.rs"]
mod node_inventory;

use crate::*;

/// Handle `aida node` subcommands. Operates on the orphan-store worktree
/// at `store_path` (typically `.aida-store/`).
// trace:BUG-1561 | ai:claude
pub(crate) fn handle_node_command(cmd: &NodeCommand, store_path: &std::path::Path) -> Result<()> {
    use aida_core::node::NodeRegistry;

    let registry_path = store_path.join("registry").join("nodes.toml");
    let node_config_path = store_path.join(".aida").join("node.toml");

    // trace:BUG-2017 | ai:codex — local repair never claims a global registry/block.
    let allocation = aida_core::allocation_authority::resolve(store_path)?;
    if !allocation.authoritative {
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
                // The node role is prose, never a capitalised token an
                // operator label could share. trace:STORY-B6B1278890774EF993D2EBC327862470-098 | ai:claude
                println!(
                    "Work node {} ready (node-local allocation).",
                    binding.node_id
                );
                return Ok(());
            }
            NodeCommand::Show { id: None } => {
                if let Some(binding) = aida_core::work_binding::load(store_path)? {
                    // Keep the role/label wording from main while list continues
                    // through the complete inventory path below.
                    // trace:BUG-HOME-026 | ai:codex
                    // trace:STORY-B6B1278890774EF993D2EBC327862470-098 | ai:claude
                    println!("{}", work_node_line(store_path, &binding.node_id));
                    print_allocation_report(store_path)?;
                    return Ok(());
                }
            }
            _ => {}
        }
    }
    match cmd {
        NodeCommand::List => {
            // Listing is a union of known identities, including a worker clone's
            // opaque binding. It never repairs or allocates identity.
            // trace:BUG-B6B1278890774EF993D2EBC327862470-026 | ai:codex
            print!(
                "{}",
                node_inventory::list(store_path, allocation.authoritative)?
            );
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

            // This clone's own node still reports its delegation state when
            // the shared roster has no row for it.
            // trace:STORY-B6B1278890774EF993D2EBC327862470-095 | ai:claude
            if id.is_none() && registry.get(&target_id).is_none() {
                println!("Node {target_id} (not in the shared registry)");
                return print_allocation_report(store_path);
            }
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
            if id.is_none() {
                print_allocation_report(store_path)?;
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
        NodeCommand::Whoami(flags) => return session_misc_cmd::handle_whoami_command(flags),
        NodeCommand::Alias { cmd } => return handle_node_alias_command(cmd, store_path),
        NodeCommand::EnrollRoot { dry_run } => return handle_enroll_root(store_path, *dry_run),
        NodeCommand::Upstream { remote, clear } => {
            return handle_upstream(store_path, remote.as_deref(), *clear)
        }
    }

    Ok(())
}

/// `aida node alias …`: manage friendly node labels in
/// `registry/node_aliases.toml` (CAS push). Works without a TTY.
// trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
fn handle_node_alias_command(cmd: &NodeAliasCommand, store_path: &std::path::Path) -> Result<()> {
    use aida_core::node_alias::{self, NodeAliasMap};
    match cmd {
        NodeAliasCommand::Set { label, node } => {
            let node = match node {
                Some(n) => n.clone(),
                None => aida_core::allocation_authority::node(store_path)?,
            };
            let node = node_alias::normalize_node(&node).map_err(anyhow::Error::msg)?;
            let label = node_alias::validate_label(label).map_err(anyhow::Error::msg)?;
            // `mutate_cas` refuses a label equal to a registered node id (for
            // set and rename alike).
            let outcome =
                node_alias::mutate_cas(store_path, &format!("label node {node} {label}"), |map| {
                    map.set(&label, &node)
                })?;
            publish_alias_write(store_path, &outcome)?;
            if outcome.changed() {
                println!("Labelled node {node} {label}. TASK-{label}-NNN now names its specs.");
            } else {
                println!("Node {node} is already labelled {label}.");
            }
        }
        NodeAliasCommand::Rename { old, new } => {
            let old = node_alias::validate_label(old).map_err(anyhow::Error::msg)?;
            let new = node_alias::validate_label(new).map_err(anyhow::Error::msg)?;
            let outcome = node_alias::mutate_cas(
                store_path,
                &format!("relabel node {old} -> {new}"),
                |map| map.rename(&old, &new),
            )?;
            publish_alias_write(store_path, &outcome)?;
            if outcome.changed() {
                println!("Relabelled {old} to {new}. {old} is retired and still resolves to the same node.");
            } else {
                println!("{old} is already the current label.");
            }
        }
        NodeAliasCommand::List { json } => {
            let map = NodeAliasMap::load(store_path);
            if *json {
                println!("{}", serde_json::to_string_pretty(&map.bindings)?);
            } else if map.bindings.is_empty() {
                println!("No node labels. Add one with `aida node alias set <LABEL>`.");
            } else {
                for b in &map.bindings {
                    let state = if b.retired { "retired" } else { "current" };
                    println!("{:<8}  {}  {state}", b.label, b.node);
                }
            }
        }
        NodeAliasCommand::Show { target, json } => {
            let map = NodeAliasMap::load(store_path);
            let rows: Vec<&node_alias::NodeAliasBinding> = match node_alias::normalize_node(target)
            {
                Ok(node) => map.bindings.iter().filter(|b| b.node == node).collect(),
                Err(_) => map
                    .bindings
                    .iter()
                    .filter(|b| b.label.eq_ignore_ascii_case(target.trim()))
                    .collect(),
            };
            if rows.is_empty() {
                anyhow::bail!("no node label matches '{target}'");
            }
            if *json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                for b in rows {
                    let state = if b.retired { "retired" } else { "current" };
                    println!("{:<8}  {}  {state}", b.label, b.node);
                }
            }
        }
    }
    Ok(())
}

/// A hub a committed node-alias write did not reach: why, and the command
/// that publishes it later.
// trace:BUG-B6B1278890774EF993D2EBC327862470-025 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
struct UnreachedHub {
    hub: String,
    reason: String,
    retry: String,
}

/// Publish a committed node-alias write to every configured hub: `origin`
/// (already pushed or attempted by `mutate_cas`) plus each `[store.sync]
/// mirror_remotes` entry, by exact commit. A hub that cannot be reached is a
/// warning naming it and the retry command; the command fails only when the
/// label reached no hub at all.
// trace:BUG-B6B1278890774EF993D2EBC327862470-025 | ai:claude
fn publish_alias_write(
    store_path: &std::path::Path,
    outcome: &aida_core::node_alias::MutateCasOutcome,
) -> Result<()> {
    use aida_core::git_ops::{self, PushFailure, PushOutcome};
    use aida_core::node_alias::{MutateCasOutcome, OriginPublish};
    let MutateCasOutcome::Committed {
        sha,
        branch,
        origin,
    } = outcome
    else {
        return Ok(());
    };
    let mut reached = Vec::new();
    let mut unreached = Vec::new();
    match origin {
        OriginPublish::Pushed => reached.push("origin".to_string()),
        OriginPublish::NoOrigin => {}
        OriginPublish::Unreached(stderr) => unreached.push(UnreachedHub {
            hub: "origin".into(),
            reason: crate::remote_create::git_push_failure_detail(stderr),
            retry: "aida db sync --push".into(),
        }),
    }
    let project_root = find_project_root_from(store_path.parent().unwrap_or(store_path)).ok();
    let mirrors = project_root
        .as_deref()
        .and_then(|root| crate::store_sync::read_store_sync_config(root).ok())
        .map(|cfg| cfg.mirror_remotes)
        .unwrap_or_default();
    // Once origin holds the label, mirror-sync copies it on; otherwise the
    // store push re-publishes to origin and fans out to every mirror.
    let mirror_retry = if reached.is_empty() {
        "aida db sync --push"
    } else {
        "aida remote mirror-sync"
    };
    for mirror in mirrors.iter().filter(|m| m.as_str() != "origin") {
        let miss = |reason: String, retry: &str| UnreachedHub {
            hub: mirror.clone(),
            reason,
            retry: retry.to_string(),
        };
        if !git_ops::has_remote(store_path, mirror) {
            unreached.push(miss(
                "listed in [store.sync] mirror_remotes but not a git remote in this clone".into(),
                &format!("aida remote mirror {mirror} --url <url>"),
            ));
            continue;
        }
        let missed = match git_ops::push_exact_outcome(store_path, mirror, sha, branch) {
            Ok(PushOutcome::Pushed) => None,
            Ok(PushOutcome::Failed {
                kind: PushFailure::NonFastForward,
                ..
            }) => Some(miss("the hub has diverged".into(), "aida remote reconcile")),
            Ok(PushOutcome::Failed { stderr, .. }) => Some(miss(
                crate::remote_create::git_push_failure_detail(&stderr),
                mirror_retry,
            )),
            Err(e) => Some(miss(e.to_string(), mirror_retry)),
        };
        match missed {
            None => {
                if let Some(root) = &project_root {
                    crate::doctor_root::clear_store_mirror_fanout_failure(
                        root, store_path, branch, mirror,
                    );
                }
                reached.push(mirror.clone());
            }
            Some(hub) => {
                if let Some(root) = &project_root {
                    crate::doctor_root::record_store_mirror_fanout_failure(
                        root,
                        store_path,
                        branch,
                        mirror,
                        &hub.reason,
                    );
                }
                unreached.push(hub);
            }
        }
    }
    let warn = crate::glyph(crate::glyphs::Glyph::Warning);
    for line in alias_publish_warnings(&reached, &unreached)? {
        eprintln!("{warn} {line}");
    }
    Ok(())
}

/// Warning lines for the hubs a committed label missed, or an error when it
/// reached none of them (the label is then only in this clone).
// trace:BUG-B6B1278890774EF993D2EBC327862470-025 | ai:claude
fn alias_publish_warnings(reached: &[String], unreached: &[UnreachedHub]) -> Result<Vec<String>> {
    if reached.is_empty() && !unreached.is_empty() {
        let hubs: Vec<String> = unreached
            .iter()
            .map(|u| format!("{} ({})", u.hub, u.reason))
            .collect();
        anyhow::bail!(
            "the node label is committed in this clone but reached no hub: {}. Publish it with `aida db sync --push` once a hub is reachable.",
            hubs.join("; ")
        );
    }
    Ok(unreached
        .iter()
        .map(|u| {
            format!(
                "Node label not published to `{}`: {}. Retry with `{}`.",
                u.hub, u.reason, u.retry
            )
        })
        .collect())
}

/// The heading `aida node list` / `show` print for a work node: its id,
/// its current label when one is bound, and its role in lower-case prose.
// trace:STORY-B6B1278890774EF993D2EBC327862470-098 | ai:claude
pub(crate) fn work_node_line(store_path: &std::path::Path, node_id: &str) -> String {
    let aliases = aida_core::node_alias::NodeAliasMap::load(store_path);
    work_node_heading(node_id, aliases.current_label(node_id))
}

/// [`work_node_line`] from an already-resolved label.
// trace:STORY-B6B1278890774EF993D2EBC327862470-098 | ai:claude
pub(crate) fn work_node_heading(node_id: &str, label: Option<&str>) -> String {
    let label = label.map(|l| format!("\n  Label: {l}")).unwrap_or_default();
    format!("Node {node_id}{label}\n  Role:  work node (node-local allocation; no global blocks)")
}

/// `aida node show`'s delegation section: this clone's role and upstream,
/// the store's root, every pool with its derived free space, this node's
/// blocks with whether each is spendable (and why not), and any lineage
/// conflict.
// trace:STORY-B6B1278890774EF993D2EBC327862470-095 | ai:claude
fn print_allocation_report(store_path: &std::path::Path) -> Result<()> {
    use aida_core::allocation_lineage::{
        check_invariants, free_ranges, AllocationRole, LineageView, PoolRegistry, RootRecord,
    };
    let mode = aida_core::allocation_authority::resolve(store_path)?;
    let node = aida_core::allocation_authority::node(store_path).ok();
    let pools = PoolRegistry::load(store_path)?;
    let blocks =
        aida_core::node::BlockRegistry::load(&store_path.join("registry").join("blocks.yaml"))?;
    let root = RootRecord::load(store_path)?;
    println!();
    print_store_mode(store_path)?;
    println!("Block delegation (this clone)");
    println!(
        "  Role:      {}",
        match mode.role {
            AllocationRole::Root => "root",
            AllocationRole::Node => "node",
        }
    );
    println!(
        "  Upstream:  {}",
        match (&mode.upstream, mode.role) {
            (Some(remote), _) => remote.clone(),
            (None, AllocationRole::Root) => "none (the root has no upstream)".into(),
            (None, AllocationRole::Node) => {
                "none declared (`aida node upstream <remote>`)".into()
            }
        }
    );
    println!(
        "  Root:      {}",
        match &root {
            Some(r) => format!("node {} (epoch {})", r.holder_node, r.epoch),
            None => "not enrolled (`aida node enroll-root` on the primary clone)".into(),
        }
    );
    // The clone-local role is not authority on its own: the store's root
    // record names the one root. Refusing on this mismatch is slice 2.
    if let (AllocationRole::Root, Some(r)) = (mode.role, &root) {
        if node.as_deref() != Some(r.holder_node.as_str()) {
            println!(
                "  {} Mismatch: this clone is marked root, but the store's root is node {}{}",
                "!".yellow().bold(),
                r.holder_node,
                node.as_deref()
                    .map(|n| format!(" (this clone is node {n})"))
                    .unwrap_or_default(),
            );
        }
    }
    if pools.pools.is_empty() {
        println!("  Pools:     none");
    } else {
        println!("  Pools:");
        for p in &pools.pools {
            let free: u64 = free_ranges(p, &pools.pools, &blocks.blocks)
                .iter()
                .map(|(start, end)| u64::from(end - start) + 1)
                .sum();
            println!(
                "    {:<12} {} {}-{}  holder {}  home {}  parent {}  free {free}",
                p.pool_id,
                p.type_prefix,
                p.range_start,
                p.range_end,
                p.holder_node,
                p.home_ref,
                p.parent_pool_id.as_deref().unwrap_or("-"),
            );
        }
    }
    match &node {
        None => println!("  Blocks:    this clone has no node binding"),
        Some(node) => aida_core::allocation_authority::with_clone_lineage_view(
            store_path,
            node,
            |view: &LineageView<'_>| {
                let mine: Vec<_> = blocks
                    .blocks
                    .iter()
                    .filter(|b| &b.node_id == node)
                    .collect();
                if mine.is_empty() {
                    println!("  Blocks:    none held by node {node}");
                } else {
                    println!("  Blocks:");
                    for b in mine {
                        // The root keeps dispensing its blocks recorded before lineage.
                        let status = if mode.authoritative && b.pool_id.is_none() {
                            if b.is_exhausted() {
                                "exhausted".to_string()
                            } else {
                                "in use by the root (no lineage recorded yet)".to_string()
                            }
                        } else {
                            match view.block_status(b) {
                                Ok(()) => "spendable".to_string(),
                                Err(why) => format!("not spendable: {why}"),
                            }
                        };
                        println!(
                            "    {} {}-{}  next {}  pool {}  {status}",
                            b.type_prefix,
                            b.range_start,
                            b.range_end,
                            b.next,
                            b.pool_id.as_deref().unwrap_or("-"),
                        );
                    }
                }
            },
        )?,
    }
    let conflicts = check_invariants(&pools.pools, &blocks.blocks, root.as_ref());
    if !conflicts.is_empty() {
        println!("  Conflicts (none are resolved automatically):");
        for c in conflicts {
            println!("    {c}");
        }
    }
    Ok(())
}

/// List the single-hub pools enroll-root turns (or would turn) into root
/// pools, with how many blocks and dispensed IDs each keeps.
// trace:STORY-B6B1278890774EF993D2EBC327862470-164 | ai:claude
fn print_materialized(
    pools: &[aida_core::allocation_lineage::PoolRecord],
    blocks: &[aida_core::AgreedIdBlock],
    dry_run: bool,
) {
    if pools.is_empty() {
        return;
    }
    println!(
        "{} the single-hub pools as root pools (same ids; no block or ID is renumbered):",
        if dry_run { "Would record" } else { "Recorded" }
    );
    for p in pools {
        let held: Vec<_> = blocks
            .iter()
            .filter(|b| b.pool_id.as_deref() == Some(p.pool_id.as_str()))
            .collect();
        let issued: u64 = held
            .iter()
            .map(|b| u64::from(b.next.saturating_sub(b.range_start)))
            .sum();
        println!(
            "  {}  {} blocks, {issued} IDs issued, home {}",
            p.pool_id,
            held.len(),
            p.home_ref
        );
    }
}

/// `Mode:` and `Hubs:` lines: single-hub (no root needed), multi-hub with
/// its root, or multi-hub with no root (claims refused), plus mirrors and
/// any mirror-and-hub config error.
// trace:STORY-B6B1278890774EF993D2EBC327862470-164 | ai:claude
fn print_store_mode(store_path: &std::path::Path) -> Result<()> {
    use aida_core::allocation_authority::{store_mode, StoreMode, MULTI_HUB_NEEDS_ROOT};
    let hubs = aida_core::store_hubs::committed(store_path)?;
    println!("Store allocation");
    let line = match store_mode(store_path)? {
        StoreMode::SingleHub { hub, remote } => format!(
            "single-hub (hub {hub}{})",
            match remote {
                Some(r) => format!(" → this clone reaches it as `{r}`"),
                None => " → no remote of this clone reaches it".into(),
            }
        ),
        StoreMode::Rooted { root, hubs } if hubs.len() > 1 => {
            format!("multi-hub (root node {root}): {}", hubs.join(", "))
        }
        StoreMode::Rooted { root, .. } => format!("rooted (root node {root})"),
        StoreMode::MultiHubNoRoot { hubs } => format!(
            "multi-hub, NO ROOT: claims refused ({}); {MULTI_HUB_NEEDS_ROOT}",
            hubs.join(", ")
        ),
        StoreMode::Undeclared => {
            let pending = aida_core::store_hubs::working(store_path)
                .map(|w| !w.hubs.is_empty())
                .unwrap_or(false);
            if pending {
                "no hub committed yet ([store.hubs] is in .aida/config.toml but not on the default branch; commit it)".into()
            } else {
                "no hub declared (declare it under [store.hubs] in .aida/config.toml)".into()
            }
        }
    };
    println!("  Mode:      {line}");
    for hub in &hubs.hubs {
        println!("  Hub:       {}  {}", hub.name, hub.urls.join(", "));
    }
    if !hubs.mirror_remotes.is_empty() {
        println!(
            "  Mirrors:   {} (never hubs)",
            hubs.mirror_remotes.join(", ")
        );
    }
    for problem in aida_core::store_hubs::config_conflicts(store_path, &hubs) {
        println!("  {} {problem}", "!".yellow().bold());
    }
    Ok(())
}

/// `aida node enroll-root`: record this clone's node as the store's one
/// allocation root, then mark the clone itself.
// trace:STORY-B6B1278890774EF993D2EBC327862470-095 | ai:claude
fn handle_enroll_root(store_path: &std::path::Path, dry_run: bool) -> Result<()> {
    use aida_core::allocation_lineage::{self as lineage, AllocationRole, EnrollOutcome};
    let node = aida_core::allocation_authority::node(store_path)?;
    let mode = aida_core::allocation_authority::resolve(store_path)?;
    if dry_run {
        match lineage::enroll_preflight(store_path, &node)? {
            EnrollOutcome::Enrolled => println!(
                "Would record node {node} as the allocation root of this store and mark this clone as root."
            ),
            EnrollOutcome::AlreadyRoot => {
                println!("Node {node} is already the allocation root; would mark this clone as root.")
            }
        }
        if lineage::enroll_preflight(store_path, &node)? == EnrollOutcome::Enrolled {
            let pools = lineage::PoolRegistry::load(store_path)?;
            let blocks = aida_core::node::BlockRegistry::load(
                &store_path.join("registry").join("blocks.yaml"),
            )?;
            let to_materialize = lineage::pools_to_materialize(&pools.pools, &blocks.blocks, &node);
            print_materialized(&to_materialize, &blocks.blocks, true);
        }
        if let Some(upstream) = &mode.upstream {
            println!("Would drop this clone's upstream `{upstream}` (the root has none).");
        }
        println!("Nothing written (dry run).");
        return Ok(());
    }
    let pools = lineage::PoolRegistry::load(store_path)?;
    let blocks =
        aida_core::node::BlockRegistry::load(&store_path.join("registry").join("blocks.yaml"))?;
    let to_materialize = lineage::pools_to_materialize(&pools.pools, &blocks.blocks, &node);
    let outcome = lineage::enroll_root(store_path, &node)?;
    if outcome == EnrollOutcome::Enrolled {
        print_materialized(&to_materialize, &blocks.blocks, false);
    }
    aida_core::allocation_authority::write_mode(store_path, AllocationRole::Root, None)?;
    match outcome {
        EnrollOutcome::Enrolled => println!(
            "{} Node {node} is now the allocation root of this store.",
            "".green().bold()
        ),
        EnrollOutcome::AlreadyRoot => {
            println!("Node {node} is already the allocation root; this clone is marked as root.")
        }
    }
    Ok(())
}

/// `aida node upstream`: show, set, or clear the one remote this clone
/// takes ID blocks from.
// trace:STORY-B6B1278890774EF993D2EBC327862470-095 | ai:claude
fn handle_upstream(store_path: &std::path::Path, remote: Option<&str>, clear: bool) -> Result<()> {
    use aida_core::allocation_authority::{self as authority, AllocationRole};
    let mode = authority::resolve(store_path)?;
    let Some(remote) = remote else {
        if clear {
            anyhow::ensure!(
                !mode.authoritative,
                "this clone is the allocation root; the root has no upstream"
            );
            authority::write_mode(store_path, AllocationRole::Node, None)?;
            println!("Cleared the upstream; this clone issues node-local IDs.");
            return Ok(());
        }
        match (&mode.upstream, mode.authoritative) {
            (Some(upstream), _) => println!("{upstream}"),
            (None, true) => println!("none (this clone is the allocation root)"),
            (None, false) => println!("none (this clone issues node-local IDs)"),
        }
        return Ok(());
    };
    anyhow::ensure!(
        !mode.authoritative,
        "this clone is the allocation root; the root has no upstream"
    );
    anyhow::ensure!(
        authority::valid_remote_name(remote),
        "`{remote}` is not a plain git remote name"
    );
    anyhow::ensure!(
        aida_core::git_ops::has_remote(store_path, remote),
        "`{remote}` is not a git remote of this clone (see `git remote -v`)"
    );
    let node = authority::node(store_path)?;
    if aida_core::allocation_lineage::RootRecord::load(store_path)?
        .is_some_and(|r| r.holder_node == node)
    {
        anyhow::bail!(
            "node {node} is this store's allocation root; declaring an upstream for it would make a cycle"
        );
    }
    refuse_mirror_upstream(store_path, remote)?;
    authority::write_mode(store_path, AllocationRole::Node, Some(remote))?;
    println!("Upstream set to `{remote}`. This clone takes ID blocks only from that remote; until it holds one, it issues node-local IDs.");
    Ok(())
}

/// Refuse `remote` as an upstream when it is a store mirror. Fails closed:
/// when the project root or its config cannot be read, the mirror list is
/// unknown, so the remote is refused rather than accepted.
// trace:STORY-B6B1278890774EF993D2EBC327862470-095 | ai:claude
fn refuse_mirror_upstream(store_path: &std::path::Path, remote: &str) -> Result<()> {
    use anyhow::Context as _;
    let project_root = find_project_root_from(store_path.parent().unwrap_or(store_path))
        .with_context(|| {
            format!(
                "cannot find the project for store {}; refusing to set an upstream without checking [store.sync] mirror_remotes",
                store_path.display()
            )
        })?;
    let config = crate::config_init::config_path_for_project(&project_root);
    match std::fs::read_to_string(&config) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => anyhow::bail!(
            "cannot read {}: {e}; refusing to set an upstream without checking [store.sync] mirror_remotes",
            config.display()
        ),
    }
    let sync = crate::store_sync::read_store_sync_config(&project_root)?;
    anyhow::ensure!(
        !sync.mirror_remotes.iter().any(|m| m == remote),
        "`{remote}` is a store mirror ([store.sync] mirror_remotes); a mirror copies records and never grants blocks"
    );
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

#[cfg(test)]
#[path = "tests/story_095_upstream_tests.rs"]
mod story_095_upstream_tests;

#[cfg(test)]
mod alias_publish_tests {
    // trace:BUG-B6B1278890774EF993D2EBC327862470-025 | ai:claude
    use super::{alias_publish_warnings, UnreachedHub};

    fn github_down() -> UnreachedHub {
        UnreachedHub {
            hub: "origin".into(),
            reason: "fatal: unable to access: Could not resolve host: github.com".into(),
            retry: "aida db sync --push".into(),
        }
    }

    #[test]
    fn a_label_on_one_hub_warns_about_the_other_and_succeeds() {
        let lines = alias_publish_warnings(&["gitlab".into()], &[github_down()]).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("`origin`"), "{}", lines[0]);
        assert!(lines[0].contains("Could not resolve host"), "{}", lines[0]);
        assert!(lines[0].contains("`aida db sync --push`"), "{}", lines[0]);
    }

    #[test]
    fn a_label_on_no_hub_fails_naming_each() {
        let err = alias_publish_warnings(&[], &[github_down()]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("committed in this clone"), "{msg}");
        assert!(msg.contains("origin (fatal"), "{msg}");
    }

    #[test]
    fn every_hub_reached_or_solo_is_silent() {
        assert!(alias_publish_warnings(&["origin".into()], &[])
            .unwrap()
            .is_empty());
        assert!(alias_publish_warnings(&[], &[]).unwrap().is_empty());
    }
}
