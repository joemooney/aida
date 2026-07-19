//! `aida relationship` — add/remove/list typed relationships between specs.
//!
//! Extracted verbatim from `main.rs` (SPIKE-78 pure-movement refactor). This is
//! the legacy SQLite-backend relationship path; the git-canonical `rel add`
//! lives inline in the main dispatch. Shared helpers (`rel_should_write_inverse`,
//! `parse_requirement_id`, `effective_display_status`, `is_terminal_status`)
//! stay in `main.rs` and are reached via `crate::`.
// trace:SPIKE-78 | ai:claude

use crate::*;

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
