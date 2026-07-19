//! `aida assign` / `aida unassign` command pair (STORY-639).
//!
//! `assign` sets the durable assignee on a spec and routes it into that user's
//! work queue; `unassign` clears the assignee and, by default, leaves the spec
//! in the (former) assignee's queue (`--from-queue` also removes it). Extracted
//! verbatim from `main.rs` (SPIKE-78); no behavior change.

use anyhow::Result;
use colored::Colorize;

use aida_core::{DatabaseBackend, Storage};

use crate::not_found;
use crate::{current_user_id, hostname, record_role_activity, send_notification};

/// `aida assign <SPEC> --to <user>` — set the durable assignee on a spec and
/// route it into that user's work queue so it surfaces in their
/// `aida queue list`. Idempotent: re-assigning to the same user is a no-op on
/// both the assignee field and the queue (no duplicate queue entry).
///
/// Assignment is distinct from `owner` (the creator/author): owner records who
/// filed the spec and drives contributions analytics; assignee is mutable
/// work-division metadata.
// trace:STORY-639 | ai:claude
pub(crate) fn handle_assign_command(
    id: &str,
    to: &str,
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
) -> Result<()> {
    let mut req = backend
        .get_requirement_by_spec_id(id)?
        .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;
    let display_id = req.spec_id.clone().unwrap_or_else(|| id.to_string());
    let target = to.trim();
    if target.is_empty() {
        anyhow::bail!("--to requires a non-empty username");
    }

    // trace:TASK-951 | ai:claude — fold case so re-assigning `joe` to a spec
    // already assigned to `Joe` is recognised as a no-op. The stored assignee
    // keeps its original casing (we only re-stamp it when the identity actually
    // changes, below).
    let already_assigned_here = req.assignee.as_deref().is_some_and(|a| {
        aida_core::node::canonical_user_id(a) == aida_core::node::canonical_user_id(target)
    });
    if !already_assigned_here {
        let now = chrono::Utc::now();
        req.assignee = Some(target.to_string());
        req.modified_at = now;
        backend.update_requirement(&req)?;
    }
    record_role_activity(&display_id, "assign");

    // Route into the target user's queue (idempotent — skip if already there).
    let storage = Storage::new(store_path.to_path_buf());
    let existing = storage.queue_list(target, false).unwrap_or_default();
    let already_queued = existing.iter().any(|e| e.requirement_id == req.id);
    if !already_queued {
        let this_machine = hostname();
        let entry = aida_core::QueueEntry {
            user_id: target.to_string(),
            requirement_id: req.id,
            position: i64::MAX, // sentinel: queue_add auto-assigns max+1000
            added_by: current_user_id(None),
            note: None,
            added_at: chrono::Utc::now(),
            for_role: None,
            for_scope: None,
            for_session: None,
            added_by_machine: Some(this_machine),
        };
        if let Err(e) = storage.queue_add(entry) {
            eprintln!(
                "{} assigned {display_id} but could not add it to {target}'s queue: {e}",
                "warning:".yellow().bold()
            );
        }
    }

    println!(
        "{} {display_id} {} {}",
        "Assigned:".cyan().bold(),
        "→".dimmed(),
        target.bold()
    );
    if already_assigned_here && already_queued {
        println!("  {}", "(already assigned and queued — no change)".dimmed());
    } else {
        println!("  {} {target}'s queue", "Queued to:".dimmed());
    }

    // STORY-644: notify the assignee via the mailbox (best-effort; STORY-643
    // auto-sync carries it to their clone on the next pull). Skip self-assigns
    // — no point messaging yourself — and skip an idempotent re-assign that
    // changed nothing. trace:STORY-644 | ai:claude
    let assigner = current_user_id(None);
    // trace:TASK-951 | ai:claude — self-assign detection folds case (`Joe`
    // assigning to `joe` is still a self-assign).
    let is_self_assign =
        aida_core::node::canonical_user_id(&assigner) == aida_core::node::canonical_user_id(target);
    if !is_self_assign && !(already_assigned_here && already_queued) {
        let title = req.title.clone();
        send_notification(
            store_path,
            &assigner,
            target,
            format!("You were assigned {display_id}: {title}"),
        );
    }
    Ok(())
}

/// `aida unassign <SPEC>` — clear the assignee. By default the spec stays in
/// the (former) assignee's queue, since the queue is the now-doing list rather
/// than the assignment of record; `--from-queue` also removes it.
// trace:STORY-639 | ai:claude
pub(crate) fn handle_unassign_command(
    id: &str,
    from_queue: bool,
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
) -> Result<()> {
    let mut req = backend
        .get_requirement_by_spec_id(id)?
        .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;
    let display_id = req.spec_id.clone().unwrap_or_else(|| id.to_string());
    let previous = req.assignee.clone();
    if previous.is_none() {
        anyhow::bail!("{display_id} is not assigned — nothing to unassign.");
    }
    let now = chrono::Utc::now();
    req.assignee = None;
    req.modified_at = now;
    backend.update_requirement(&req)?;
    record_role_activity(&display_id, "unassign");

    println!("{} {display_id}", "Unassigned:".cyan().bold());
    if from_queue {
        if let Some(prev_user) = previous.as_deref() {
            let storage = Storage::new(store_path.to_path_buf());
            if let Err(e) = storage.queue_remove(prev_user, &req.id) {
                eprintln!(
                    "{} cleared the assignee but could not remove {display_id} from {prev_user}'s queue: {e}",
                    "warning:".yellow().bold()
                );
            } else {
                println!("  {} {prev_user}'s queue", "Removed from:".dimmed());
            }
        }
    } else if let Some(prev_user) = previous.as_deref() {
        println!(
            "  {} still in {prev_user}'s queue (pass `--from-queue` to remove it)",
            "Note:".dimmed()
        );
    }
    Ok(())
}
