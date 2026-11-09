// trace:TASK-3426 | ai:codex
use super::*;

// trace:TASK-3426 | ai:codex
pub(super) fn handle_queue_command(
    cmd: &QueueCommand,
    storage: &Storage,
    _store_path: &std::path::Path,
) -> Result<()> {
    let get_user = |user: &Option<String>| -> String { current_user_id(user.as_deref()) };
    match cmd {
        QueueCommand::Remove {
            id,
            user,
            global,
            r#for,
        } => {
            // Removing a route is a dispatch mutation. trace:STORY-1353 | ai:codex
            if !crate::gate::role_authority::has_dispatch_authority() {
                anyhow::bail!(
                    "removing queued work needs dispatch authority (product, advisor, or \
                     integrator role, or a live orchestrator)"
                );
            }
            let user_id = get_user(user);

            // TASK-1150: distinct-user identity guard — mutating a queue owned
            // by a genuinely-different user id than this shell's identity
            // silently crosses identities. trace:TASK-1150 | ai:claude
            identity_guard::enforce(&current_user_id(None), &user_id, "queue remove")?;

            // --global removes from ~/.aida/queue/<role>.yaml. Role from
            // --for or AIDA_SESSION_ROLE. trace:FR-1-012 | ai:claude
            if *global {
                let role = r#for
                    .clone()
                    .or_else(|| {
                        std::env::var("AIDA_SESSION_ROLE")
                            .ok()
                            .filter(|s| !s.is_empty())
                    })
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "--global requires --for <role> or an active role (AIDA_SESSION_ROLE)."
                        )
                    })?;
                // Match by spec_id OR agreed_id from the global entries (no
                // local store needed). BUG-83: prior code matched on spec_id
                // only, so `aida queue remove --global FR-1` would miss an
                // entry cached under the legacy spec_id form `FR-1-042`.
                // trace:BUG-83 | ai:claude
                // trace:BUG-1682 | ai:codex
                let entries = global_queue::load(&role)?;
                let target = entries.iter().find(|e| {
                    e.spec_id
                        .as_deref()
                        .is_some_and(|s| s.eq_ignore_ascii_case(id))
                        || e.agreed_id
                            .as_deref()
                            .is_some_and(|s| s.eq_ignore_ascii_case(id))
                        || uuid::Uuid::parse_str(id)
                            .map(|u| u == e.requirement_id)
                            .unwrap_or(false)
                });
                let Some(target) = target else {
                    anyhow::bail!("{} not found in global queue for role:{}", id, role);
                };
                let removed = global_queue::remove(
                    &role,
                    &target.requirement_id,
                    Some(&target.project_root),
                )?;
                if removed {
                    // BUG-83: prefer cached agreed_id (short form) for
                    // display when one's been assigned; fall back to
                    // spec_id. trace:BUG-83 | ai:claude
                    let display_id = target
                        .agreed_id
                        .as_deref()
                        .or(target.spec_id.as_deref())
                        .unwrap_or("???");
                    let summary = format!(
                        "{} Removed {} from global queue [role:{}, origin:{}]",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        display_id.bold(),
                        role,
                        target.project_name
                    );
                    let details = queue_destination_details(
                        display_id,
                        &format!("role:{role}"),
                        "global-role",
                        Some(&role),
                        false,
                    );
                    print_queue_mutation_destination(
                        "remove",
                        display_id,
                        target.title.as_deref(),
                        &summary,
                        &details,
                    )?;
                }
                return Ok(());
            }

            let store = storage.load()?;
            let req = if let Ok(uuid) = uuid::Uuid::parse_str(id) {
                store.requirements.iter().find(|r| r.id == uuid)
            } else {
                store.get_requirement_unambiguous(id)? // trace:TASK-1468 | ai:claude
            }
            .ok_or_else(|| not_found::requirement_not_found(id, Some(storage.path())))?;

            // BUG-529: `--for <role>` is a role FILTER on remove, mirroring
            // `queue add --for`. Resolve it the same way add does
            // (`any` → unrouted/role-blind, otherwise canonicalize), then
            // remove only the entry queued for that role so a spec queued for
            // multiple roles can be dropped from one queue without emptying
            // the others. Omitting `--for` keeps the historical role-blind
            // remove (every entry for the spec). trace:BUG-529 | ai:claude
            let remove_role: Option<String> = match r#for.as_deref() {
                None | Some("any") => None,
                Some(role) => Some(canonical_role_name(role)),
            };
            let existing_roles: Vec<String> = storage
                .queue_list(&user_id, true)?
                .iter()
                .filter(|entry| entry.requirement_id == req.id)
                .map(|entry| {
                    entry
                        .for_role
                        .as_deref()
                        .filter(|role| !role.trim().is_empty())
                        .map(canonical_role_name)
                        .unwrap_or_else(|| "any".to_string())
                })
                .collect();
            storage.queue_remove_for_role(&user_id, &req.id, remove_role.as_deref())?;
            // BUG-81: short id when present. trace:BUG-81 | ai:claude
            let display_id = req
                .agreed_id
                .as_deref()
                .or(req.spec_id.as_deref())
                .unwrap_or("???");
            let role_for_destination =
                remove_role
                    .as_deref()
                    .or_else(|| match existing_roles.as_slice() {
                        [single] => Some(single.as_str()),
                        [] => None,
                        _ => Some("all"),
                    });
            let summary = match remove_role.as_deref() {
                Some(role) => format!(
                    "{} Removed {} from queue [role:{}]",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    display_id.bold(),
                    role
                ),
                None => format!(
                    "{} Removed {} from queue",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    display_id.bold()
                ),
            };
            let details = queue_destination_details(
                display_id,
                &user_id,
                "local",
                role_for_destination,
                false,
            );
            print_queue_mutation_destination(
                "remove",
                display_id,
                Some(&req.title),
                &summary,
                &details,
            )?;
        }
        QueueCommand::Move {
            id,
            user,
            top,
            bottom,
            to,
            before,
            after,
            force,
        } => {
            // Reordering work is a dispatch mutation. trace:STORY-1353 | ai:codex
            if !crate::gate::role_authority::has_dispatch_authority() {
                anyhow::bail!(
                    "moving queued work needs dispatch authority (product, advisor, or \
                     integrator role, or a live orchestrator)"
                );
            }
            // BUG-89: route through the canonical helper so move resolves
            // user_id the same way add/list do (previously this path
            // skipped the USERNAME fallback). trace:BUG-89 | ai:claude
            // BUG-1487: `move` was the only queue verb without --user, so
            // the documented way to address another identity's queue
            // failed on the one verb that reorders it. trace:BUG-1487 | ai:claude
            let user_id = current_user_id(user.as_deref());
            let store = storage.load()?;

            let req = if let Ok(uuid) = uuid::Uuid::parse_str(id) {
                store.requirements.iter().find(|r| r.id == uuid)
            } else {
                store.get_requirement_unambiguous(id)? // trace:TASK-1468 | ai:claude
            }
            .ok_or_else(|| not_found::requirement_not_found(id, Some(storage.path())))?;

            let mut entries = storage.queue_list(&user_id, true)?;
            let current_entry_role = entries
                .iter()
                .find(|entry| entry.requirement_id == req.id)
                .and_then(|entry| entry.for_role.as_deref())
                .map(canonical_role_name);
            // BUG-249: the relative paths (--top/--bottom/--before/--after)
            // never checked the target side — queue_reorder silently
            // no-ops when the target isn't in the queue file, so the
            // command printed a `Moved` check line for a spec that wasn't in the
            // queue at all. Surface the two error states explicitly
            // before any path-specific logic runs.
            // trace:BUG-249 | ai:claude
            let move_display_id = req
                .agreed_id
                .as_deref()
                .or(req.spec_id.as_deref())
                .unwrap_or(id);
            classify_queue_move_target(req.id, move_display_id, &req.status, &entries, *force)?;
            // STORY-72: queues created before the queue_add sentinel-fix
            // can have every entry at `position: i64::MAX`, which makes
            // any --before/--after/--top math unable to produce a
            // distinct-sorting result. Detect that state and lay down
            // properly-gapped positions (preserving display order) before
            // we compute the new slot. trace:STORY-72 | ai:claude
            if entries.iter().any(|e| e.position == i64::MAX) {
                let renumber: Vec<(uuid::Uuid, i64)> = entries
                    .iter()
                    .enumerate()
                    .map(|(i, e)| (e.requirement_id, (i as i64 + 1) * 1000))
                    .collect();
                storage.queue_reorder(&user_id, &renumber)?;
                entries = storage.queue_list(&user_id, true)?;
            }
            // TASK-280: `--to <N>` absolute positioning. The slot counts
            // among the queue's live (non-terminal) entries — the same items a
            // BARE `aida queue list` numbers (a role/scope-filtered list shows a
            // subset, so the visible slot can differ — TASK-317) — so
            // Completed/Rejected entries lingering in the queue file don't throw
            // the slot number off. Only those live entries are renumbered (with a
            // fresh 1000-gap); terminal entries keep their positions and,
            // being hidden, don't affect the visible order.
            // trace:TASK-280 | ai:claude
            if let Some(requested) = to {
                let live: Vec<&aida_core::QueueEntry> = entries
                    .iter()
                    .filter(|e| {
                        store
                            .requirements
                            .iter()
                            .find(|r| r.id == e.requirement_id)
                            .map(|r| !is_terminal_status(&r.status))
                            .unwrap_or(true)
                    })
                    .collect();
                if !live.iter().any(|e| e.requirement_id == req.id) {
                    anyhow::bail!(
                        "{} is not an active item in the queue — --to positions live (non-terminal) entries",
                        id
                    );
                }
                let ids: Vec<uuid::Uuid> = live.iter().map(|e| e.requirement_id).collect();
                let (order, slot) = move_to_absolute_position(&ids, req.id, *requested);
                let renumber: Vec<(uuid::Uuid, i64)> = order
                    .iter()
                    .enumerate()
                    .map(|(i, uid)| (*uid, (i as i64 + 1) * 1000))
                    .collect();
                storage.queue_reorder(&user_id, &renumber)?;
                let display_id = req
                    .agreed_id
                    .as_deref()
                    .or(req.spec_id.as_deref())
                    .unwrap_or("???");
                if *requested != slot {
                    let summary = format!(
                        "{} Moved {} to slot {} — --to {} is out of range (queue has {} item{})",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        display_id.bold(),
                        slot,
                        requested,
                        ids.len(),
                        if ids.len() == 1 { "" } else { "s" },
                    );
                    let details = queue_destination_details(
                        display_id,
                        &user_id,
                        "local",
                        current_entry_role.as_deref(),
                        true,
                    );
                    print_queue_mutation_destination(
                        "move",
                        display_id,
                        Some(&req.title),
                        &summary,
                        &details,
                    )?;
                } else {
                    let summary = format!(
                        "{} Moved {} to slot {} in queue",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        display_id.bold(),
                        slot,
                    );
                    let details = queue_destination_details(
                        display_id,
                        &user_id,
                        "local",
                        current_entry_role.as_deref(),
                        true,
                    );
                    print_queue_mutation_destination(
                        "move",
                        display_id,
                        Some(&req.title),
                        &summary,
                        &details,
                    )?;
                }
                return Ok(());
            }
            // TASK-491: `--top` (alias `--to-top`/`--to-front`) on a spec
            // that's already at queue position 1 is a friendly no-op rather
            // than a position-renumber that prints `Moved`. "Position 1" is
            // measured among live (non-terminal) entries — the same items a
            // bare `aida queue list` numbers (a filtered list shows a subset —
            // TASK-317) — so a Completed entry lingering in the YAML file
            // doesn't mask the real head.
            // trace:TASK-491 | ai:claude
            if *top {
                let already_first = entries
                    .iter()
                    .find(|e| {
                        store
                            .requirements
                            .iter()
                            .find(|r| r.id == e.requirement_id)
                            .map(|r| !is_terminal_status(&r.status))
                            .unwrap_or(true)
                    })
                    .map(|e| e.requirement_id == req.id)
                    .unwrap_or(false);
                if already_first {
                    println!(
                        "{} {} is already at queue head",
                        "·".dimmed(),
                        move_display_id
                    );
                    return Ok(());
                }
            }
            let new_position = if *top {
                entries.first().map(|e| e.position - 1000).unwrap_or(0)
            } else if *bottom {
                entries.last().map(|e| e.position + 1000).unwrap_or(1000)
            } else if let Some(ref before_id) = before {
                let before_req = if let Ok(uuid) = uuid::Uuid::parse_str(before_id) {
                    store.requirements.iter().find(|r| r.id == uuid)
                } else {
                    store.get_requirement_unambiguous(before_id)? // trace:TASK-1468 | ai:claude
                }
                .ok_or_else(|| not_found::requirement_not_found(before_id, Some(storage.path())))?;
                if before_req.id == req.id {
                    anyhow::bail!("--before target is the same as the moved item");
                }
                entries
                    .iter()
                    .find(|e| e.requirement_id == before_req.id)
                    .ok_or_else(|| anyhow::anyhow!("{} is not in the queue", before_id))
                    .map(|e| e.position - 1)?
            } else if let Some(ref after_id) = after {
                // STORY-72: --after Y places X immediately after Y in the
                // queue. Symmetric to --before. Midpoint math against the
                // successor avoids the naive `Y.pos + 1` collision when the
                // queue happens to be densely-packed; falls back to
                // `Y.pos + 1000` when Y is at the bottom.
                // trace:STORY-72 | ai:claude
                let after_req = if let Ok(uuid) = uuid::Uuid::parse_str(after_id) {
                    store.requirements.iter().find(|r| r.id == uuid)
                } else {
                    store.get_requirement_unambiguous(after_id)? // trace:TASK-1468 | ai:claude
                }
                .ok_or_else(|| not_found::requirement_not_found(after_id, Some(storage.path())))?;
                if after_req.id == req.id {
                    anyhow::bail!("--after target is the same as the moved item");
                }
                let anchor_pos = entries
                    .iter()
                    .find(|e| e.requirement_id == after_req.id)
                    .ok_or_else(|| anyhow::anyhow!("{} is not in the queue", after_id))?
                    .position;
                // Successor is the next entry strictly after the anchor in
                // sorted-by-position order. queue_list already returns sorted.
                let successor_pos = entries
                    .iter()
                    .find(|e| e.position > anchor_pos)
                    .map(|e| e.position);
                position_after(anchor_pos, successor_pos)
            } else {
                anyhow::bail!(
                    "Specify a destination: --to-front, --to-back, --to <N>, --before <ID>, or --after <ID>"
                );
            };

            // STORY-333: warn if the new position lands this entry ahead
            // of its blocked-by target. Same shape as `queue add`.
            // trace:STORY-333 | ai:claude
            warn_if_queued_ahead_of_blocker(req, new_position, &store, storage, &user_id);

            storage.queue_reorder(&user_id, &[(req.id, new_position)])?;
            // BUG-81: short id when present. trace:BUG-81 | ai:claude
            let display_id = req
                .agreed_id
                .as_deref()
                .or(req.spec_id.as_deref())
                .unwrap_or("???");
            let summary = format!(
                "{} Moved {} in queue",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                display_id.bold()
            );
            let details = queue_destination_details(
                display_id,
                &user_id,
                "local",
                current_entry_role.as_deref(),
                true,
            );
            print_queue_mutation_destination(
                "move",
                display_id,
                Some(&req.title),
                &summary,
                &details,
            )?;
        }
        QueueCommand::Clear { user, completed } => {
            let user_id = get_user(user);
            storage.queue_clear(&user_id, *completed)?;
            if *completed {
                println!(
                    "{} Cleared completed items from queue",
                    crate::glyph(crate::glyphs::Glyph::Check).green()
                );
            } else {
                println!(
                    "{} Cleared all items from queue",
                    crate::glyph(crate::glyphs::Glyph::Check).green()
                );
            }
        }
        _ => unreachable!("dispatcher selected another queue subcommand"),
    }
    Ok(())
}
