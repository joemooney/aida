//! `aida defer` / `aida undefer` command cluster (STORY-584).
//!
//! The second view-level flag orthogonal to both lifecycle status and archive:
//! `defer` parks a spec as primed / conditional work (hidden from the default
//! open-work view but not filed away) and records the free-text revisit trigger
//! that distinguishes deferred (prospective) from archived (filed); `undefer`
//! is the inverse. Extracted verbatim from `main.rs` (SPIKE-78); no behavior
//! change.

use anyhow::Result;
use colored::Colorize;

use aida_core::DatabaseBackend;

use crate::not_found;
use crate::record_role_activity;

/// `aida defer <SPEC> [--until "<condition>"]` — park a spec as primed /
/// conditional work, hidden from the default open-work view. Mirrors
/// `archive_single` but sets the parallel defer view-flag and records the
/// free-text revisit trigger that distinguishes deferred (prospective) from
/// archived (filed). Defer does NOT touch the lifecycle state machine and
/// deliberately carries no terminal-status guard — the whole point is to shelf
/// live, open backlog.
// trace:STORY-584 | ai:claude
pub(crate) fn defer_single(
    id: &str,
    until: Option<&str>,
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
) -> Result<()> {
    let mut req = backend
        .get_requirement_by_spec_id(id)?
        .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;
    let display_id = req.spec_id.clone().unwrap_or_else(|| id.to_string());

    // Re-deferring an already-deferred spec is allowed — it lets the operator
    // update the revisit trigger via `--until` without an undefer round-trip.
    let already = req.deferred;
    let now = chrono::Utc::now();
    req.deferred = true;
    if already {
        // Preserve the original defer timestamp on a trigger update.
        if req.deferred_at.is_none() {
            req.deferred_at = Some(now);
        }
    } else {
        req.deferred_at = Some(now);
    }
    // Only overwrite the trigger when one is supplied, so a bare re-defer
    // keeps the existing condition.
    if let Some(cond) = until {
        req.deferred_until = Some(cond.to_string());
    }
    req.modified_at = now;
    backend.update_requirement(&req)?;
    record_role_activity(&display_id, "defer");

    // A deferred spec is parked outside executable work. Queue rows may live
    // under personal identities or shared role identities, so sweep every
    // persisted queue user instead of only the identity that ran `aida defer`.
    // trace:BUG-1099 | ai:codex
    let queue_storage = crate::Storage::new(store_path.to_path_buf());
    let removed = remove_deferred_queue_rows(&queue_storage, &req.id)?;

    let verb = if already { "Re-deferred:" } else { "Deferred:" };
    println!("{} {display_id}", verb.cyan().bold());
    if removed.is_empty() {
        println!(
            "  {} no queued rows found for {display_id}",
            "Dequeued:".dimmed()
        );
    } else {
        println!(
            "  {} removed {} queued row{} for {display_id}:",
            "Dequeued:".cyan(),
            removed.len(),
            if removed.len() == 1 { "" } else { "s" }
        );
        for row in &removed {
            println!("    - {}", row);
        }
    }
    match req.deferred_until.as_deref() {
        Some(cond) => println!("  {} {cond}", "Revisit when:".dimmed()),
        None => println!(
            "  {} no revisit trigger recorded — add one with `aida defer {display_id} --until \"<condition>\"`",
            "Note:".dimmed()
        ),
    }
    Ok(())
}

fn remove_deferred_queue_rows(
    storage: &aida_core::Storage,
    req_id: &uuid::Uuid,
) -> Result<Vec<String>> {
    let mut users = storage.queue_users().unwrap_or_default();
    users.push(crate::current_user_id(None));
    users.sort();
    users.dedup();

    let mut removed = Vec::new();
    for user in users {
        let entries = match storage.queue_list(&user, /* include_completed */ true) {
            Ok(entries) => entries,
            Err(e) => {
                eprintln!(
                    "{} could not inspect queue `{user}` while deferring ({e})",
                    "Warning:".yellow()
                );
                continue;
            }
        };
        let Some(entry) = entries.iter().find(|e| e.requirement_id == *req_id) else {
            continue;
        };
        let role = entry.for_role.as_deref().unwrap_or("unrouted").to_string();
        match storage.queue_remove(&user, req_id) {
            Ok(()) => removed.push(format!("queue `{user}` ({role})")),
            Err(e) => eprintln!(
                "{} could not remove deferred spec from queue `{user}` ({e}); row may remain.",
                "Warning:".yellow()
            ),
        }
    }
    Ok(removed)
}

/// Inverse of `aida defer` — clears the deferred flag + revisit trigger so the
/// spec reappears in the default views. Mirrors `handle_unarchive_command`.
// trace:STORY-584 | ai:claude
pub(crate) fn handle_undefer_command(
    id: &str,
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
) -> Result<()> {
    let mut req = backend
        .get_requirement_by_spec_id(id)?
        .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;
    let display_id = req.spec_id.clone().unwrap_or_else(|| id.to_string());
    // Honor-both migration: a spec deferred only via a legacy `deferred:*` tag
    // has no flag to clear — name that so the operator knows to edit the tag.
    let tag_deferred = req.tags.iter().any(|t| t.starts_with("deferred:"));
    if !req.deferred {
        if tag_deferred {
            anyhow::bail!(
                "{display_id} is hidden via a legacy `deferred:*` tag, not the deferred flag. \
                 Remove the tag with `aida edit {display_id} --remove-tag <tag>` to restore it."
            );
        }
        anyhow::bail!(
            "{display_id} is not deferred — nothing to undefer. \
             Use `aida defer {display_id}` if you meant to defer it."
        );
    }
    let now = chrono::Utc::now();
    req.deferred = false;
    req.deferred_at = None;
    req.deferred_until = None;
    req.modified_at = now;
    backend.update_requirement(&req)?;
    record_role_activity(&display_id, "undefer");
    println!("{} {display_id}", "Undeferred:".cyan().bold());
    Ok(())
}
