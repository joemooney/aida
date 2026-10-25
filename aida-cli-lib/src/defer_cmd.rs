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

use aida_core::{DatabaseBackend, Storage};

use crate::not_found;
use crate::record_role_activity;

// STORY-1889: operator-hold authority lives in the protected gate module.
// Transitional re-export; the final STORY-1889 PR repoints the callers.
// trace:STORY-1889 | ai:claude
pub(crate) use crate::gate::operator_hold::{
    enforce_reasoned_operator_hold, hold_refusal, OperatorHoldRefusal,
};
use crate::gate::operator_hold::{lift_hold_refusal, set_hold_refusal};

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
    reason: Option<&str>,
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
) -> Result<()> {
    if let Some(refusal) = set_hold_refusal(reason) {
        anyhow::bail!(refusal);
    }
    let mut req = backend
        .get_requirement_unambiguous(id)? // trace:BUG-1535 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;
    let display_id = req.spec_id.clone().unwrap_or_else(|| id.to_string());
    if let Some(refusal) = lift_hold_refusal(req.deferred_reason.as_deref()) {
        anyhow::bail!(refusal);
    }

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
    if let Some(reason) = reason.map(str::trim).filter(|reason| !reason.is_empty()) {
        req.deferred_reason = Some(reason.to_string());
        req.tags.insert("operator-held".to_string());
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
    match req.deferred_reason.as_deref() {
        Some(reason) => println!("  {} {reason}", "Hold reason:".dimmed()),
        None => println!(
            "  {} record one with `aida defer {display_id} --reason \"<reason>\"`",
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
        .get_requirement_unambiguous(id)? // trace:BUG-1535 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;
    let display_id = req.spec_id.clone().unwrap_or_else(|| id.to_string());
    if let Some(refusal) = lift_hold_refusal(req.deferred_reason.as_deref()) {
        anyhow::bail!(refusal);
    }
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
    req.deferred_reason = None;
    req.tags.remove("operator-held");
    req.modified_at = now;
    backend.update_requirement(&req)?;
    record_role_activity(&display_id, "undefer");
    println!("{} {display_id}", "Undeferred:".cyan().bold());
    Ok(())
}

/// Defer a spec — park it as primed/conditional work, hidden from the
/// default open-work view but not filed away the way archive is.
///
/// Defer is a view-level flag distinct from status (it does not touch the
/// lifecycle state machine). Use `--reason` to record why this is an
/// operator hold and `--until` to record the revisit trigger —
/// the free-text condition that brings the spec back (e.g.
/// `--until "when a slice verb ships"`). That trigger is the one thing
/// distinguishing deferred (prospective, primed) from archived
/// (retrospective, filed). Deferred rows are hidden from `aida list`,
/// `aida search`, and `aida history` by default; surface them with
/// `--deferred` (only) or `--all` (union).
// trace:STORY-584 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "defer")]
pub(crate) struct DeferArgs {
    /// SPEC-ID or UUID to defer.
    pub id: String,

    /// The revisit trigger — the condition that brings this spec back
    /// (free text). Stored alongside the spec and shown in the deferred
    /// view so you can scan what is primed and what returns each item.
    // trace:STORY-584 | ai:claude
    // trace:BUG-1294 | ai:claude
    #[clap(long, value_name = "CONDITION", allow_hyphen_values = true)]
    pub until: Option<String>,

    /// Why the work is being held. Required for a durable operator hold;
    /// legacy deferrals may omit it.
    #[clap(long, value_name = "REASON", allow_hyphen_values = true)]
    pub reason: Option<String>,
}

fn run_defer(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <DeferArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
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
                DeferArgs { id, until, reason } => {
                    // STORY-584: park a spec on the primed/conditional shelf, hidden
                    // from the default open-work view. trace:STORY-584 | ai:claude
                    self::defer_single(
                        id,
                        until.as_deref(),
                        reason.as_deref(),
                        backend,
                        store_path,
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
                DeferArgs { .. } => {
                    // STORY-584: defer uses the git-canonical write path.
                    anyhow::bail!(
                        "aida defer/undefer requires the distributed git-canonical store \
                     (run `aida init` to migrate, or this project is on the \
                     deprecated --centralized backend)"
                    );
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static DEFER: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "defer",
        help_after: "unarchive",
        command: <DeferArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: true,
            skip_role_restore_prompt: false,
            team_identity_write: true,
        },
        run: crate::command_registry::Handler::Store(run_defer),
    };

/// Inverse of `aida defer` — clears the deferred flag (and its revisit
/// trigger) so the spec reappears in default `aida list` / `aida history`
/// / `aida search` views.
// trace:STORY-584 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "undefer")]
pub(crate) struct UndeferArgs {
    /// SPEC-ID or UUID to undefer.
    pub id: String,
}

fn run_undefer(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <UndeferArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
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
                UndeferArgs { id } => {
                    // STORY-584: inverse of `aida defer`. trace:STORY-584 | ai:claude
                    self::handle_undefer_command(id, backend, store_path)?;
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
                UndeferArgs { .. } => {
                    // STORY-584: defer uses the git-canonical write path.
                    anyhow::bail!(
                        "aida defer/undefer requires the distributed git-canonical store \
                     (run `aida init` to migrate, or this project is on the \
                     deprecated --centralized backend)"
                    );
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static UNDEFER: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "undefer",
        help_after: "defer",
        command: <UndeferArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: true,
            skip_role_restore_prompt: false,
            team_identity_write: true,
        },
        run: crate::command_registry::Handler::Store(run_undefer),
    };
