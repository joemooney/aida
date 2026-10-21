//! `aida relationship` — add/remove/list typed relationships between specs.
//!
//! Extracted verbatim from `main.rs` (SPIKE-78 pure-movement refactor). This is
//! the legacy SQLite-backend relationship path; the git-canonical `rel add`
//! lives inline in the main dispatch. Shared helpers (`rel_should_write_inverse`,
//! `parse_requirement_id`, `effective_display_status`, `is_terminal_status`)
//! stay in `main.rs` and are reached via `crate::`.
// trace:SPIKE-78 | ai:claude

use crate::*;

// trace:STORY-1728 | ai:claude
#[path = "relationship_cmd_args.rs"]
pub(crate) mod relationship_cmd_args;
pub(crate) use relationship_cmd_args::*;

pub(crate) fn handle_relationship_command(
    cmd: &RelationshipCommand,
    storage: &Storage,
) -> Result<()> {
    match cmd {
        RelationshipCommand::Add {
            from_pos,
            to_pos,
            from_flag,
            to_flag,
            r#type,
            bidirectional,
            force_parent,
        } => {
            let from = from_pos
                .as_deref()
                .or(from_flag.as_deref())
                .ok_or_else(|| anyhow::anyhow!("missing FROM (positional or --from)"))?;
            let to = to_pos
                .as_deref()
                .or(to_flag.as_deref())
                .ok_or_else(|| anyhow::anyhow!("missing TO (positional or --to)"))?;
            add_relationship(storage, from, to, r#type, *bidirectional, *force_parent)?;
        }
        RelationshipCommand::Remove {
            from_pos,
            to_pos,
            from_flag,
            to_flag,
            r#type,
            bidirectional,
        } => {
            let from = from_pos
                .as_deref()
                .or(from_flag.as_deref())
                .ok_or_else(|| anyhow::anyhow!("missing FROM (positional or --from)"))?;
            let to = to_pos
                .as_deref()
                .or(to_flag.as_deref())
                .ok_or_else(|| anyhow::anyhow!("missing TO (positional or --to)"))?;
            remove_relationship(storage, from, to, r#type, *bidirectional)?;
        }
        RelationshipCommand::List { id, source, .. } => {
            // Legacy SQLite path doesn't implement the global/incoming/--type
            // filters from TASK-65 — they require iterating every requirement
            // and the legacy backend isn't the target of new work. Falls back
            // to the point-query when an id (positional or --source) is given;
            // refuses cleanly otherwise. trace:TASK-65 | ai:claude
            let id_str = id.as_deref().or(source.as_deref()).ok_or_else(|| {
                anyhow::anyhow!(
                    "legacy SQLite backend supports only `aida rel list <ID>`. \
                     Global / --target / --dangling listings require the git-canonical \
                     store — run `aida init` (or `aida db export-git` to migrate)."
                )
            })?;
            list_relationships(storage, id_str)?;
        }
    }
    Ok(())
}

fn add_relationship(
    storage: &Storage,
    from_str: &str,
    to_str: &str,
    rel_type_str: &str,
    bidirectional: bool,
    force_parent: bool,
) -> Result<()> {
    // Load requirements
    let mut store = storage.load()?;

    // Parse source and target IDs
    let from_id = parse_requirement_id(from_str, &store)?;
    let to_id = parse_requirement_id(to_str, &store)?;

    // Parse relationship type
    let rel_type = RelationshipType::from_str(rel_type_str);

    // Get requirement info for display (clone the data we need)
    let from_req = store
        .get_requirement_by_id(&from_id)
        .ok_or_else(|| anyhow::anyhow!("Source requirement not found"))?;
    let to_req = store
        .get_requirement_by_id(&to_id)
        .ok_or_else(|| anyhow::anyhow!("Target requirement not found"))?;

    let from_spec = from_req
        .spec_id
        .clone()
        .unwrap_or_else(|| "N/A".to_string());
    let from_title = from_req.title.clone();
    let to_spec = to_req.spec_id.clone().unwrap_or_else(|| "N/A".to_string());
    let to_title = to_req.title.clone();

    // BUG-64: terminal-status guard on the parent end of a parent/child
    // edge. Same logic as the git-canonical path. trace:BUG-64 | ai:claude
    //
    // BUG-628: read the parent's EFFECTIVE (display) status, not its stored
    // status. For an epic the displayed status is the read-only rollup of its
    // children (`effective_display_status` → `derive_epic_status`), so a guard
    // keyed off the raw stored status disagreed with what `aida show` / the
    // cache report — e.g. an epic deriving to Draft but carrying a stale stored
    // `Completed` was wrongly guarded as "is Completed". Unifying the guard onto
    // the derived value makes the guard and the display agree.
    // trace:BUG-628 | ai:claude
    if !force_parent {
        let parent_for_guard = match &rel_type {
            RelationshipType::Child => Some(&to_req),
            RelationshipType::Parent => Some(&from_req),
            _ => None,
        };
        if let Some(p) = parent_for_guard {
            let p: &aida_core::models::Requirement = p;
            let effective = effective_display_status(&store, p);
            if is_terminal_status(&effective) {
                anyhow::bail!(
                    "parent {} is {} — adding new children to a closed parent is usually a mistake. \
                     Pass `--force-parent` to override.",
                    p.spec_id.as_deref().unwrap_or("?"),
                    effective,
                );
            }
        }
    }

    // TASK-679: dedup — `add_relationship` errors hard on a pre-existing edge;
    // make a repeated `rel add` a friendly no-op instead. trace:TASK-679 | ai:claude
    let already_exists = from_req
        .relationships
        .iter()
        .any(|r| r.rel_type == rel_type && r.target_id == to_id);
    if already_exists {
        println!(
            "{} {} {} {} ({})",
            "Relationship already exists (no change):".yellow(),
            from_spec,
            "->".blue(),
            to_spec,
            rel_type.to_string().cyan()
        );
        return Ok(());
    }

    // TASK-679: parent/child edges are canonically bidirectional (match
    // `aida add --parent`), so write the reciprocal even without the flag.
    // `add_relationship` itself dedups the inverse end. trace:TASK-679 | ai:claude
    let write_inverse = rel_should_write_inverse(&rel_type, bidirectional);

    // Add the relationship
    store.add_relationship(&from_id, rel_type.clone(), &to_id, write_inverse)?;

    // Save
    storage.save(&store)?;

    println!("{}", "Relationship added successfully!".green());
    println!(
        "  {} ({}) {} {} ({})",
        from_spec,
        from_title,
        "->".blue(),
        to_spec,
        to_title
    );
    println!("  Relationship: {}", rel_type.to_string().cyan());

    if write_inverse {
        if let Some(inverse) = rel_type.inverse() {
            println!("  {} (bidirectional)", inverse.to_string().cyan());
        }
    }

    Ok(())
}

fn remove_relationship(
    storage: &Storage,
    from_str: &str,
    to_str: &str,
    rel_type_str: &str,
    bidirectional: bool,
) -> Result<()> {
    // Load requirements
    let mut store = storage.load()?;

    // Parse source and target IDs
    let from_id = parse_requirement_id(from_str, &store)?;
    let to_id = parse_requirement_id(to_str, &store)?;

    // Parse relationship type
    let rel_type = RelationshipType::from_str(rel_type_str);

    // Remove the relationship
    store.remove_relationship(&from_id, &rel_type, &to_id, bidirectional)?;

    // Save
    storage.save(&store)?;

    println!("{}", "Relationship removed successfully!".green());
    println!("  Relationship: {}", rel_type.to_string().cyan());

    if bidirectional {
        if let Some(inverse) = rel_type.inverse() {
            println!("  {} (bidirectional)", inverse.to_string().cyan());
        }
    }

    Ok(())
}

fn list_relationships(storage: &Storage, id_str: &str) -> Result<()> {
    // Load requirements
    let store = storage.load()?;

    // Parse ID
    let id = parse_requirement_id(id_str, &store)?;

    // Get requirement
    let req = store
        .get_requirement_by_id(&id)
        .ok_or_else(|| anyhow::anyhow!("Requirement not found"))?;

    println!("{}: {}", "Requirement".blue(), req.title);
    if let Some(spec_id) = &req.spec_id {
        println!("{}: {}", "SPEC-ID".blue(), spec_id);
    }
    println!("{}: {}", "UUID".blue(), req.id);
    println!();

    if req.relationships.is_empty() {
        println!("{}", "No relationships found.".yellow());
        return Ok(());
    }

    println!("{}:", "Relationships".green());
    for relationship in &req.relationships {
        let target = store.get_requirement_by_id(&relationship.target_id);
        if let Some(target_req) = target {
            let target_spec = target_req.spec_id.as_deref().unwrap_or("N/A");

            // Format the relationship description based on type
            let description = match &relationship.rel_type {
                RelationshipType::Parent => "is parent of".to_string(),
                RelationshipType::Child => "is child of".to_string(),
                RelationshipType::Duplicate => "is duplicate of".to_string(),
                RelationshipType::Verifies => "verifies".to_string(),
                RelationshipType::VerifiedBy => "is verified by".to_string(),
                RelationshipType::References => "references".to_string(),
                // trace:STORY-333 | ai:claude
                RelationshipType::BlockedBy => "is blocked by".to_string(),
                RelationshipType::Blocks => "blocks".to_string(),
                // trace:TASK-1176 | ai:claude
                RelationshipType::SupersededBy => "is superseded by".to_string(),
                RelationshipType::Supersedes => "supersedes".to_string(),
                RelationshipType::Custom(name) => name.clone(),
            };

            // BUG-53: tag rejected targets so a dangling-looking edge is
            // recognizable as still-resolvable rather than removed.
            // trace:BUG-53 | ai:claude
            if matches!(target_req.status, RequirementStatus::Rejected) {
                println!(
                    "  {} {} {} ({}) - {}",
                    description.cyan(),
                    target_spec.yellow(),
                    "[REJECTED]".red().bold(),
                    target_req.id.to_string().dimmed(),
                    target_req.title
                );
            } else {
                println!(
                    "  {} {} ({}) - {}",
                    description.cyan(),
                    target_spec.yellow(),
                    target_req.id.to_string().dimmed(),
                    target_req.title
                );
            }
        } else {
            // BUG-53: shorten dangling target uuid + flag it as removed so
            // the line reads as a tombstone rather than a phantom.
            // trace:BUG-53 | ai:claude
            let uuid_str = relationship.target_id.to_string();
            let short = &uuid_str[..uuid_str.len().min(8)];
            println!(
                "  {} {} {}",
                relationship.rel_type.to_string().cyan(),
                short.dimmed(),
                "(removed — run `aida doctor verify-relationships --repair` to clean up)".red()
            );
        }
    }

    Ok(())
}

/// Relationship management commands
#[derive(clap::Parser, Debug)]
#[clap(name = "rel")]
#[clap(hide = true)]
pub(crate) struct RelArgs {
    #[clap(subcommand)]
    pub inner: crate::cli::RelationshipCommand,
}

fn run_rel(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <RelArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
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
                // Phase 2: Relationship commands
                RelArgs {
                    inner:
                        RelationshipCommand::Add {
                            from_pos,
                            to_pos,
                            from_flag,
                            to_flag,
                            r#type,
                            bidirectional,
                            force_parent,
                        },
                } => {
                    let from = from_pos
                        .as_deref()
                        .or(from_flag.as_deref())
                        .ok_or_else(|| anyhow::anyhow!("missing FROM (positional or --from)"))?;
                    let to = to_pos
                        .as_deref()
                        .or(to_flag.as_deref())
                        .ok_or_else(|| anyhow::anyhow!("missing TO (positional or --to)"))?;
                    // trace:BUG-790 | ai:codex
                    reject_cross_store_spec_ref(from)?;
                    reject_cross_store_spec_ref(to)?;
                    let mut from_req = backend
                        .get_requirement_unambiguous(from)?
                        .ok_or_else(|| not_found::requirement_not_found(from, Some(store_path)))?;

                    let to_req = backend
                        .get_requirement_unambiguous(to)?
                        .ok_or_else(|| not_found::requirement_not_found(to, Some(store_path)))?;

                    // BUG-1471: normalize the natural `related` spelling to the
                    // traversable standard References edge before anything is written.
                    // Other aliases and custom names retain the core parser's behavior.
                    // trace:BUG-1471 | ai:codex
                    let rel_type = cli_relationship_type(r#type);

                    // TASK-887: a `--type` that isn't a standard relationship type lands
                    // as a `Custom` edge, which the graph traversals (blocked-by,
                    // blocks, tree, impact) silently won't follow. Don't hard-
                    // reject (custom types can be intentional), but surface a note —
                    // with a did-you-mean when it looks like a typo of a standard type —
                    // so a fat-fingered `blockedby`/`depends` doesn't create an
                    // invisible edge. trace:TASK-887 | ai:claude
                    if let RelationshipType::Custom(ref custom) = rel_type {
                        match nearest_standard_rel_type(custom) {
                            Some(near) => eprintln!(
                                "{} '{}' isn't a standard relationship type (did you mean '{}'?); \
                             created as a custom edge — graph traversals won't follow it.",
                                "note:".yellow().bold(),
                                custom,
                                near,
                            ),
                            None => eprintln!(
                                "{} '{}' isn't a standard relationship type ({}); \
                             created as a custom edge — graph traversals won't follow it.",
                                "note:".yellow().bold(),
                                custom,
                                STANDARD_REL_TYPES.join(", "),
                            ),
                        }
                    }

                    // BUG-64: same terminal-status guard as `aida add --parent`,
                    // applied here when the user is hand-rolling a parent edge
                    // via `aida rel add X Y --type child` (X is child of Y, so Y
                    // is the parent being checked) or its inverse `--type parent`
                    // (X is parent of Y). `--force-parent` bypasses for backfill
                    // cases. trace:BUG-64 | ai:claude
                    if !*force_parent {
                        let parent_for_guard = match &rel_type {
                            RelationshipType::Child => Some(&to_req),
                            RelationshipType::Parent => Some(&from_req),
                            _ => None,
                        };
                        if let Some(p) = parent_for_guard {
                            if is_terminal_status(&p.status) {
                                anyhow::bail!(
                                "parent {} is {} — adding new children to a closed parent is usually a mistake. \
                                 Pass `--force-parent` to override.",
                                p.spec_id.as_deref().unwrap_or("?"),
                                p.status,
                            );
                            }
                        }
                    }
                    if let Some(warning) =
                        relationship_terminal_ambiguity_warning(&rel_type, &from_req, &to_req)
                    {
                        // trace:BUG-790 | ai:codex
                        eprintln!("{} {}", "warning:".yellow().bold(), warning);
                    }

                    // TASK-679: a parent/child edge is canonically BIDIRECTIONAL so it
                    // matches `aida add --parent` (which writes the parent's `Parent`
                    // edge AND the child's reciprocal `Child` edge). The old `rel add`
                    // wrote only the source-side edge, leaving the other spec's `show`
                    // blind to the link (spec point #5). Force the reciprocal for
                    // parent/child even without an explicit `--bidirectional`; other
                    // edge types keep the opt-in flag. trace:TASK-679 | ai:claude
                    let write_inverse = rel_should_write_inverse(&rel_type, *bidirectional);

                    // TASK-679: dedup. The old git-canonical path pushed blindly, so a
                    // repeated `rel add` (EPIC-37 had the same target twice) accumulated
                    // duplicate edges. Skip the push when an identical (rel_type,
                    // target) edge already exists on the source. trace:TASK-679 | ai:claude
                    let edge_exists = from_req
                        .relationships
                        .iter()
                        .any(|r| r.rel_type == rel_type && r.target_id == to_req.id);
                    if edge_exists {
                        println!(
                            "Relationship already exists: {} --[{:?}]--> {} (no change)",
                            from, rel_type, to
                        );
                    } else {
                        let rel = aida_core::models::Relationship {
                            rel_type: rel_type.clone(),
                            target_id: to_req.id,
                            created_at: Some(chrono::Utc::now()),
                            created_by: Some(get_default_author()),
                        };

                        from_req.relationships.push(rel);
                        from_req.modified_at = chrono::Utc::now();
                        backend.update_requirement(&from_req)?;
                        println!("Added relationship: {} --[{:?}]--> {}", from, rel_type, to);
                    }

                    if write_inverse {
                        let mut to_req = backend.get_requirement_unambiguous(to)?.unwrap();
                        // STORY-333: use the canonical inverse helper so new typed
                        // variants (BlockedBy ↔ Blocks) compose automatically.
                        // trace:STORY-333 | ai:claude
                        let inverse_type = rel_type.inverse().unwrap_or_else(|| rel_type.clone());
                        // TASK-679: dedup the inverse end too. trace:TASK-679 | ai:claude
                        let inverse_exists = to_req
                            .relationships
                            .iter()
                            .any(|r| r.rel_type == inverse_type && r.target_id == from_req.id);
                        if inverse_exists {
                            println!(
                                "Inverse already exists: {} --[{:?}]--> {} (no change)",
                                to, inverse_type, from
                            );
                        } else {
                            let inv_rel = aida_core::models::Relationship {
                                rel_type: inverse_type.clone(),
                                target_id: from_req.id,
                                created_at: Some(chrono::Utc::now()),
                                created_by: Some(get_default_author()),
                            };
                            to_req.relationships.push(inv_rel);
                            to_req.modified_at = chrono::Utc::now();
                            backend.update_requirement(&to_req)?;
                            println!("Added inverse: {} --[{:?}]--> {}", to, inverse_type, from);
                        }
                    }
                }
                RelArgs {
                    inner:
                        RelationshipCommand::Remove {
                            from_pos,
                            to_pos,
                            from_flag,
                            to_flag,
                            r#type,
                            bidirectional,
                        },
                } => {
                    let from = from_pos
                        .as_deref()
                        .or(from_flag.as_deref())
                        .ok_or_else(|| anyhow::anyhow!("missing FROM (positional or --from)"))?;
                    let to = to_pos
                        .as_deref()
                        .or(to_flag.as_deref())
                        .ok_or_else(|| anyhow::anyhow!("missing TO (positional or --to)"))?;
                    let mut from_req = backend
                        .get_requirement_unambiguous(from)?
                        .ok_or_else(|| not_found::requirement_not_found(from, Some(store_path)))?;

                    // Look up target UUID
                    let to_req = backend
                        .get_requirement_unambiguous(to)?
                        .ok_or_else(|| not_found::requirement_not_found(to, Some(store_path)))?;

                    // TASK-1426: honor `--type`. This arm used to drop EVERY edge to
                    // the target whatever type was asked for, so removing a stale
                    // custom `related` edge also removed a good `references` edge
                    // beside it. `related` resolves through the same alias as
                    // `rel add`, and the references family also matches stored
                    // legacy custom `related` spellings. trace:TASK-1426 | ai:claude
                    let requested = cli_relationship_type(r#type);
                    let before = from_req.relationships.len();
                    from_req.relationships.retain(|r| {
                        !(r.target_id == to_req.id && rel_remove_matches(&r.rel_type, &requested))
                    });
                    let removed = before - from_req.relationships.len();

                    if removed > 0 {
                        from_req.modified_at = chrono::Utc::now();
                        backend.update_requirement(&from_req)?;
                        println!(
                            "Removed {} relationship(s) from {} to {}",
                            removed, from, to
                        );
                    } else {
                        println!(
                            "No {} relationship found from {} to {}",
                            requested, from, to
                        );
                    }

                    // Mirror `rel add`'s `rel_should_write_inverse` — the parent/child
                    // pair (and any future pair added to that rule) is canonically
                    // bidirectional, so remove drops both reciprocal edges even
                    // without an explicit `--bidirectional`. Every other type keeps
                    // the opt-in flag, matching `rel add`. trace:BUG-1602 | ai:claude
                    if rel_should_write_inverse(&requested, *bidirectional) {
                        let inverse = requested.inverse().unwrap_or_else(|| requested.clone());
                        let mut to_req = backend.get_requirement(&to_req.id)?.ok_or_else(|| {
                            not_found::requirement_not_found(to, Some(store_path))
                        })?;
                        let before = to_req.relationships.len();
                        to_req.relationships.retain(|r| {
                            !(r.target_id == from_req.id
                                && rel_remove_matches(&r.rel_type, &inverse))
                        });
                        let removed_inverse = before - to_req.relationships.len();
                        if removed_inverse > 0 {
                            to_req.modified_at = chrono::Utc::now();
                            backend.update_requirement(&to_req)?;
                            println!(
                                "Removed {} inverse relationship(s) from {} to {}",
                                removed_inverse, to, from
                            );
                        }
                    }
                }
                RelArgs {
                    inner:
                        RelationshipCommand::List {
                            id,
                            source,
                            target,
                            r#type,
                            dangling,
                            all,
                            limit,
                        },
                } => {
                    // trace:TASK-65 | ai:claude
                    // Three modes:
                    //   - target=Some                → incoming edges
                    //   - id|source=Some             → outgoing edges (legacy point query)
                    //   - none of the above          → global listing
                    // --type / --dangling / --all compose across all three modes.
                    let source_ref = source.as_deref().or(id.as_deref());
                    handle_rel_list_modern(
                        backend,
                        store_path,
                        source_ref,
                        target.as_deref(),
                        r#type.as_deref(),
                        *dangling,
                        *all,
                        *limit,
                    )?;
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
                RelArgs { inner: rel_cmd } => {
                    self::handle_relationship_command(rel_cmd, &storage)?;
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static REL: crate::command_registry::CommandReg = crate::command_registry::CommandReg {
    name: "rel",
    help_after: "upgrade",
    command: <RelArgs as clap::CommandFactory>::command,
    policy: |_| crate::command_registry::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::Relationship,
        per_write_auto_push: true,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::command_registry::Handler::Store(run_rel),
};
