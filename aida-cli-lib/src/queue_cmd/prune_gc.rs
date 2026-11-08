// trace:TASK-3426 | ai:codex
use super::*;

/// The error `aida queue prune` bails with when no predicate flag is passed.
/// Names every dead-queue-pruning verb and the entry class each removes so an
/// operator can pick the right one without opening three separate --help
/// pages. Shared with the unit test so the surface stays honest.
// trace:TASK-1063 | ai:claude
pub(crate) fn queue_prune_no_predicate_message() -> String {
    "no prune predicate specified. Available pruning verbs:\n  \
     --orphaned         remove entries whose backing spec was DELETED\n  \
     --merged           remove auto-queued reviewer entries whose PR already merged\n  \
     aida queue gc      remove entries whose spec is archived / Completed / Rejected\n\
     \nCombine --orphaned --merged to sweep both; add --dry-run to preview."
        .to_string()
}

/// Handle queue commands
///
/// The queue entries whose backing spec is a terminal corpse — archived,
/// Completed, or Rejected. These linger in the queue file after the work
/// shipped; the front-door view (STORY-723) hides them but the file itself
/// still carries them, so a queue-GC sweep removes them here. Pure over
/// (entries, summaries) so it's cache-fast and unit-testable. The missing-spec
/// case (an entry whose spec isn't in `summaries`) is the `queue prune
/// --orphaned` predicate's job, NOT GC's — such an entry is LEFT alone. An
/// optional `for_role` narrows to one routed role.
// trace:TASK-1052 | ai:claude
pub(crate) fn dead_queue_entries<'a>(
    entries: &'a [aida_core::models::QueueEntry],
    summaries: &[aida_core::RequirementSummary],
    for_role: Option<&str>,
) -> Vec<&'a aida_core::models::QueueEntry> {
    let by_id: std::collections::HashMap<uuid::Uuid, &aida_core::RequirementSummary> =
        summaries.iter().map(|s| (s.id, s)).collect();
    entries
        .iter()
        .filter(|e| match for_role {
            None => true,
            Some(want) => e
                .for_role
                .as_deref()
                .is_some_and(|have| want.eq_ignore_ascii_case(have)),
        })
        .filter(|e| match by_id.get(&e.requirement_id) {
            // Dead = the spec still exists AND is archived or terminal
            // (Completed / Rejected). Done is NOT terminal (work on a branch,
            // not yet merged), so a Done entry survives.
            Some(s) => s.archived || is_terminal_status_str(&s.status),
            None => false,
        })
        .collect()
}

/// BUG-1512: which of `summaries` look like auto-queued review rows ("Review
/// PR-N: ...") and aren't already flagged dead by the target-spec rule above.
/// A routed review entry's liveness must not hinge on the review STORY's own
/// status — that status only says whether a reviewer closed it, not whether
/// the PR it reviews is still open. Pure — no forge, no network — so it's
/// unit-testable on its own; the caller resolves each returned PR number's
/// merged-or-closed state (via `review_pr_is_merged_or_closed`, or a test
/// double) and feeds the resolved ids to [`merged_pr_review_entries`].
// trace:BUG-1512 | ai:claude
pub(crate) fn review_summaries_pending_merge_check<'a>(
    summaries: &'a [aida_core::RequirementSummary],
    already_dead: &std::collections::HashSet<uuid::Uuid>,
) -> Vec<(&'a aida_core::RequirementSummary, u64)> {
    summaries
        .iter()
        .filter(|s| !already_dead.contains(&s.id))
        .filter_map(|s| parse_review_story_pr_number(&s.title).map(|pr| (s, pr)))
        .collect()
}

/// BUG-1512: the routed review entries whose PR already merged or closed —
/// additive to `dead_queue_entries`'s target-spec rule, never a replacement
/// for it (an entry the target-spec rule already caught is excluded via
/// `already_dead` so the two conditions don't double-count). `merged_pr_ids`
/// is the set of requirement ids whose PR was confirmed merged/closed (see
/// `review_summaries_pending_merge_check` for how the caller builds the
/// candidate list). Pure over its inputs, mirroring `dead_queue_entries`'s
/// shape so both are unit-testable without a forge/network dependency.
///
/// DECISION (BUG-1512 AC3): the review STORY's own status is left untouched
/// here — still Draft (or whatever it was), just unrouted. It is
/// deliberately NOT bumped to Completed (that would assert a review that
/// never happened) and this fix does not invent a new terminal
/// "closed-unreviewed" status either, since a bare Draft-and-unrouted spec
/// already reads as "nobody signed off on this" and a new status is a
/// bigger surface change than this entry-level bug calls for.
// trace:BUG-1512 | ai:claude
pub(crate) fn merged_pr_review_entries<'a>(
    entries: &'a [aida_core::models::QueueEntry],
    merged_pr_ids: &std::collections::HashSet<uuid::Uuid>,
    already_dead: &std::collections::HashSet<uuid::Uuid>,
    for_role: Option<&str>,
) -> Vec<&'a aida_core::models::QueueEntry> {
    entries
        .iter()
        .filter(|e| match for_role {
            None => true,
            Some(want) => e
                .for_role
                .as_deref()
                .is_some_and(|have| want.eq_ignore_ascii_case(have)),
        })
        .filter(|e| !already_dead.contains(&e.requirement_id))
        .filter(|e| merged_pr_ids.contains(&e.requirement_id))
        .collect()
}

/// BUG-1512 AC1: a routed review's reason for existing ends when its PR
/// either MERGED or was CLOSED without merging — a declined PR needs no more
/// review either, so both are terminal for this purpose. Mirrors
/// `pr_is_merged_with_sink`'s shape (same `None` = "cannot confirm, don't
/// collect" contract) but widens the accepted state. `None` on any forge
/// failure (gh missing/unreachable, no forge configured), never a silent
/// collect.
// trace:BUG-1512 | ai:claude
// trace:STORY-2401 | ai:codex
pub(crate) fn review_pr_is_merged_or_closed(
    project_root: &std::path::Path,
    pr: u32,
    sink: &mut dyn network_retry::RetrySink,
) -> Option<bool> {
    crate::forge_confirmation::metadata_for(
        project_root,
        crate::forge::forge_for(project_root).as_ref(),
        pr as u64,
        sink,
        |m| m.state != crate::forge::ChangeState::Open,
    )
    .known()
    .map(|m| {
        matches!(
            m.state,
            crate::forge::ChangeState::Merged | crate::forge::ChangeState::Closed
        )
    })
}

/// BUG-772: the UNSATISFIED `BlockedBy` predecessors of `req`, as the
/// path-to-empty footer's plain-data facts — display id + status, with `None`
/// status marking a dangling edge (target no longer in the store). A Completed
/// predecessor is a satisfied edge and is omitted, matching
/// `aida_core::pickability::blocked_by_incomplete`. The verdict over these facts
/// is the pure `burndown::classify_blockers`; this is just the store walk.
// trace:BUG-772 | ai:claude
pub(crate) fn unsatisfied_blocker_facts(
    req: &aida_core::Requirement,
    store: &aida_core::RequirementsStore,
) -> Vec<burndown::BlockerFact> {
    req.relationships
        .iter()
        .filter(|r| matches!(r.rel_type, aida_core::RelationshipType::BlockedBy))
        .filter_map(|rel| {
            match store.requirements.iter().find(|r| r.id == rel.target_id) {
                Some(target)
                    if matches!(target.status, aida_core::RequirementStatus::Completed) =>
                {
                    None // satisfied edge
                }
                Some(target) => Some(burndown::BlockerFact {
                    id: target
                        .agreed_id
                        .clone()
                        .or_else(|| target.spec_id.clone())
                        .unwrap_or_else(|| target.id.to_string()),
                    status: Some(format!("{:?}", target.status)),
                }),
                // Dangling edge — a blocker we can't resolve. Defensively a
                // dead end (never silently "it'll clear on its own").
                None => Some(burndown::BlockerFact {
                    id: "a deleted spec".to_string(),
                    status: None,
                }),
            }
        })
        .collect()
}

/// Whether the AUTHORITATIVE object behind a queue entry is still dead
/// (archived or terminal). `get_requirement` on the cached backend reads the
/// spec's YAML object, not the cache row, so a spec reopened after the cache
/// snapshot was taken reads as live. A missing or unreadable object is not
/// provably dead, so the entry is kept (deleted specs are `queue prune
/// --orphaned`'s job).
// trace:BUG-1664 | ai:claude
pub(crate) fn queue_target_still_dead(
    backend: &aida_core::CachedGitBackend,
    requirement_id: &uuid::Uuid,
) -> bool {
    match backend.get_requirement(requirement_id) {
        Ok(Some(req)) => req.archived || is_terminal_status_str(&format!("{:?}", req.status)),
        _ => false,
    }
}

/// Split the cache-selected dead queue entries into those whose authoritative
/// spec is still dead and a count of those that no longer are. The candidates
/// come from the cache projection, which a read may serve from a stale
/// snapshot (a live foreign cache writer skips the catch-up), so a destructive
/// sweep re-checks every candidate before removing it.
// trace:BUG-1664 | ai:claude
pub(crate) fn revalidate_dead_queue_entries<'a>(
    backend: &aida_core::CachedGitBackend,
    candidates: Vec<&'a aida_core::models::QueueEntry>,
) -> (Vec<&'a aida_core::models::QueueEntry>, usize) {
    let total = candidates.len();
    let still_dead: Vec<_> = candidates
        .into_iter()
        .filter(|e| queue_target_still_dead(backend, &e.requirement_id))
        .collect();
    let skipped = total - still_dead.len();
    (still_dead, skipped)
}

/// TASK-1052: opportunistic queue self-heal on read. Drops dead routed entries
/// (archived/Completed/Rejected targets) from the user's local queue so the
/// underlying queue stays clean, not just the view. Cheap — reuses the
/// cache-backed summaries; best-effort, swallowing every error so a `queue
/// list` read never fails on a GC hiccup. Returns the count pruned.
// trace:TASK-1052 | ai:claude
pub(crate) fn opportunistic_queue_gc(
    storage: &Storage,
    store_path: &std::path::Path,
    user_id: &str,
) -> usize {
    // include_completed=true so terminal corpses are actually visible to the
    // sweep (the backend keeps them in the file regardless of this flag, but
    // be explicit). An empty queue → nothing to do.
    let entries = match storage.queue_list(user_id, /* include_completed */ true) {
        Ok(e) if !e.is_empty() => e,
        _ => return 0,
    };
    let Ok(backend) = advance_backend(store_path) else {
        return 0;
    };
    // cache-tolerant-read: selection only — each candidate's target spec YAML
    // object is re-read inside the store write lock, right before the removal
    // (`queue_remove_many_if`).
    // trace:BUG-1671 | ai:claude
    let summaries = match backend.list_summaries(&queue_dead_target_summary_filter()) {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let candidates = dead_queue_entries(&entries, &summaries, None);
    let dead: Vec<uuid::Uuid> = revalidate_dead_queue_entries(&backend, candidates)
        .0
        .iter()
        .map(|e| e.requirement_id)
        .collect();
    if dead.is_empty() {
        return 0;
    }
    crate::sweep_test_hook::fire(store_path);
    // BUG-1671: the pass above runs outside the store lock, so a reopen could
    // land between it and the queue write. Re-check each candidate under the
    // lock the reopen itself needs. trace:BUG-1671 | ai:claude
    storage
        .queue_remove_many_if(user_id, &dead, &|id| queue_target_still_dead(&backend, id))
        .map(|removed| removed.len())
        .unwrap_or(0)
}

// trace:TASK-3426 | ai:codex
pub(super) fn handle_queue_command(
    cmd: &QueueCommand,
    storage: &Storage,
    store_path: &std::path::Path,
) -> Result<()> {
    let get_user = |user: &Option<String>| -> String { current_user_id(user.as_deref()) };
    match cmd {
        QueueCommand::Prune {
            orphaned,
            merged,
            dry_run,
            user,
            r#for,
        } => {
            if !*orphaned && !*merged {
                // trace:TASK-1063 | ai:claude — name every pruning verb + the
                // entry class each removes so the operator can pick without
                // reading three separate --help pages.
                anyhow::bail!("{}", queue_prune_no_predicate_message());
            }
            let user_id = get_user(user);
            let entries = storage.queue_list(&user_id, /* include_completed */ false)?;
            let store = storage.load()?;
            let existing_ids: std::collections::HashSet<uuid::Uuid> =
                store.requirements.iter().map(|r| r.id).collect();

            let role_matches = |e: &&aida_core::models::QueueEntry| match (
                r#for.as_deref(),
                e.for_role.as_deref(),
            ) {
                (None, _) => true,
                (Some(want), Some(have)) => want.eq_ignore_ascii_case(have),
                (Some(_), None) => false,
            };

            // Collect prune targets from whichever predicates were passed,
            // deduped by requirement id (a row can match more than one).
            // trace:TASK-593 | ai:claude
            let mut seen: std::collections::HashSet<uuid::Uuid> = std::collections::HashSet::new();
            let mut orphans: Vec<&aida_core::models::QueueEntry> = Vec::new();

            if *orphaned {
                for e in entries
                    .iter()
                    .filter(|e| !existing_ids.contains(&e.requirement_id))
                    .filter(role_matches)
                {
                    if seen.insert(e.requirement_id) {
                        orphans.push(e);
                    }
                }
            }

            let _confirmation_sweep = crate::forge_confirmation::Sweep::new();
            if *merged {
                // TASK-593: a review row ("Review PR-N: …") whose PR has merged
                // outside the reviewer's `aida queue done` flow lingers because
                // its backing spec is often still non-terminal (the orphaned
                // predicate only catches deleted specs). Match review rows by
                // title, parse the PR number, and confirm merge state via gh.
                let by_id: std::collections::HashMap<uuid::Uuid, &aida_core::Requirement> =
                    store.requirements.iter().map(|r| (r.id, r)).collect();
                let project_root = find_project_root()?;
                let mut sink = network_retry::NoopSink;
                for e in entries.iter().filter(role_matches) {
                    if seen.contains(&e.requirement_id) {
                        continue;
                    }
                    let Some(req) = by_id.get(&e.requirement_id) else {
                        continue; // missing spec → that's the --orphaned case
                    };
                    let Some(pr) = parse_review_story_pr_number(&req.title) else {
                        continue; // not an auto-queued review row
                    };
                    if pr_is_merged_with_sink(&project_root, pr as u32, &mut sink) == Some(true)
                        && seen.insert(e.requirement_id)
                    {
                        orphans.push(e);
                    }
                }
            }

            // Predicate-neutral noun for the result messages.
            let noun = if *orphaned && *merged {
                "stale"
            } else if *merged {
                "merged-PR"
            } else {
                "orphan"
            };

            if orphans.is_empty() {
                println!(
                    "{} No {noun} queue entries found{}",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    if r#for.is_some() {
                        format!(" for role {}", r#for.as_deref().unwrap())
                    } else {
                        String::new()
                    }
                );
            } else {
                let n = orphans.len();
                println!(
                    "{} {} {noun} queue entr{} ({})",
                    if *dry_run {
                        crate::glyph(crate::glyphs::Glyph::InfoAlt).yellow()
                    } else {
                        crate::glyph(crate::glyphs::Glyph::Cross).yellow()
                    },
                    n.to_string().bold(),
                    if n == 1 { "y" } else { "ies" },
                    if *dry_run { "would remove" } else { "removing" },
                );
                for e in &orphans {
                    let note = e
                        .note
                        .as_deref()
                        .map(|n| format!(" — {}", n.dimmed()))
                        .unwrap_or_default();
                    let role = e
                        .for_role
                        .as_deref()
                        .map(|r| format!(" [for:{r}]"))
                        .unwrap_or_default();
                    println!(
                        "  pos {:2}  {} {}{}",
                        e.position,
                        e.requirement_id.to_string().dimmed(),
                        role,
                        note,
                    );
                }
                if *dry_run {
                    println!();
                    println!("  {}", "Re-run without --dry-run to remove.".dimmed());
                } else {
                    for e in &orphans {
                        storage.queue_remove(&user_id, &e.requirement_id)?;
                    }
                    println!(
                        "{} Removed {} {noun} queue entr{}",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        n.to_string().bold(),
                        if n == 1 { "y" } else { "ies" },
                    );
                }
            }
        }
        QueueCommand::Gc {
            user,
            r#for,
            dry_run,
        } => {
            let user_id = get_user(user);
            let entries = storage.queue_list(&user_id, /* include_completed */ true)?;
            let gc_backend = advance_backend(store_path)?;
            // cache-tolerant-read: selection only — each candidate's YAML
            // object is re-read inside the store write lock, right before the
            // removal.
            // trace:BUG-1671 | ai:claude
            let summaries = gc_backend.list_summaries(&queue_dead_target_summary_filter())?;
            // BUG-1664: the summaries may be a stale snapshot; keep only the
            // candidates whose authoritative spec is still dead.
            // trace:BUG-1664 | ai:claude
            let (dead, revived) = revalidate_dead_queue_entries(
                &gc_backend,
                dead_queue_entries(&entries, &summaries, r#for.as_deref()),
            );
            if revived > 0 {
                println!(
                    "  {}",
                    format!(
                        "kept {revived} entr{} whose spec changed since the cached view \
                         (no longer closed or archived)",
                        if revived == 1 { "y" } else { "ies" }
                    )
                    .dimmed()
                );
            }
            let dead_ids: std::collections::HashSet<Uuid> =
                dead.iter().map(|e| e.requirement_id).collect();

            // BUG-1512: resolve the merge state of any candidate review rows
            // not already caught above. This is the one network/gh-backed
            // step in the sweep; everything upstream and downstream of it
            // (the candidate selection and the removal) is pure and covered
            // by unit tests.
            let _confirmation_sweep = crate::forge_confirmation::Sweep::new();
            let review_candidates = review_summaries_pending_merge_check(&summaries, &dead_ids);
            let mut merged_pr_by_id: std::collections::HashMap<Uuid, u64> =
                std::collections::HashMap::new();
            if !review_candidates.is_empty() {
                let project_root = find_project_root()?;
                let mut sink = network_retry::NoopSink;
                for (s, pr) in review_candidates {
                    if review_pr_is_merged_or_closed(&project_root, pr as u32, &mut sink)
                        == Some(true)
                    {
                        merged_pr_by_id.insert(s.id, pr);
                    }
                }
            }
            let merged_pr_ids: std::collections::HashSet<Uuid> =
                merged_pr_by_id.keys().copied().collect();
            let merged_dead =
                merged_pr_review_entries(&entries, &merged_pr_ids, &dead_ids, r#for.as_deref());

            let total = dead.len() + merged_dead.len();
            if total == 0 {
                println!(
                    "{} No dead queue entries found{}",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    if r#for.is_some() {
                        format!(" for role {}", r#for.as_deref().unwrap())
                    } else {
                        String::new()
                    }
                );
            } else {
                println!(
                    "{} {} dead queue entr{} ({})",
                    if *dry_run {
                        crate::glyph(crate::glyphs::Glyph::InfoAlt).yellow()
                    } else {
                        crate::glyph(crate::glyphs::Glyph::Cross).yellow()
                    },
                    total.to_string().bold(),
                    if total == 1 { "y" } else { "ies" },
                    if *dry_run { "would remove" } else { "removing" },
                );
                let by_id: std::collections::HashMap<Uuid, &aida_core::RequirementSummary> =
                    summaries.iter().map(|s| (s.id, s)).collect();
                for e in &dead {
                    let label = by_id
                        .get(&e.requirement_id)
                        .map(|s| {
                            let id = s
                                .agreed_id
                                .as_deref()
                                .or(s.spec_id.as_deref())
                                .unwrap_or("?");
                            format!("{} [{}]", id, s.status)
                        })
                        .unwrap_or_else(|| e.requirement_id.to_string());
                    let role = e
                        .for_role
                        .as_deref()
                        .map(|r| format!(" [for:{r}]"))
                        .unwrap_or_default();
                    println!("  pos {:2}  {}{}", e.position, label.dimmed(), role);
                }
                // BUG-1512: a distinct "why" per row — this class survives
                // the target-spec check above (the story is still Draft) but
                // its PR already merged, so the review is unrouted, not
                // completed (see the decision note on `merged_pr_review_entries`).
                for e in &merged_dead {
                    let id = by_id
                        .get(&e.requirement_id)
                        .and_then(|s| s.agreed_id.as_deref().or(s.spec_id.as_deref()))
                        .unwrap_or("?");
                    let pr = merged_pr_by_id
                        .get(&e.requirement_id)
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "?".to_string());
                    let role = e
                        .for_role
                        .as_deref()
                        .map(|r| format!(" [for:{r}]"))
                        .unwrap_or_default();
                    println!(
                        "  pos {:2}  {}{}",
                        e.position,
                        format!("{id} [PR #{pr} merged/closed — review never closed]").dimmed(),
                        role,
                    );
                }
                if *dry_run {
                    println!();
                    println!("  {}", "Re-run without --dry-run to remove.".dimmed());
                } else {
                    // for_role None → bulk remove-by-spec (one commit). With a
                    // role filter, drop only the matching-role entry per spec so
                    // a sibling entry routed to another role survives.
                    let all_dead: Vec<&aida_core::models::QueueEntry> = dead
                        .iter()
                        .copied()
                        .chain(merged_dead.iter().copied())
                        .collect();
                    // BUG-1671: the revalidation above ran outside the store
                    // lock, so re-take the decision under it, right before the
                    // removal. Only the target-spec rule's candidates are
                    // re-read: a BUG-1512 review row is dead because its PR
                    // merged, which no store write can revive.
                    // trace:BUG-1671 | ai:claude
                    let still_dead = |id: &Uuid| {
                        !dead_ids.contains(id) || queue_target_still_dead(&gc_backend, id)
                    };
                    crate::sweep_test_hook::fire(store_path);
                    let removed = if r#for.is_none() {
                        let ids: Vec<Uuid> = all_dead.iter().map(|e| e.requirement_id).collect();
                        storage
                            .queue_remove_many_if(&user_id, &ids, &still_dead)?
                            .len()
                    } else {
                        let mut removed = 0usize;
                        for e in &all_dead {
                            if storage.queue_remove_for_role_if(
                                &user_id,
                                &e.requirement_id,
                                r#for.as_deref(),
                                &still_dead,
                            )? {
                                removed += 1;
                            }
                        }
                        removed
                    };
                    println!(
                        "{} Removed {} dead queue entr{}",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        removed.to_string().bold(),
                        if removed == 1 { "y" } else { "ies" },
                    );
                }
            }
        }
        _ => unreachable!("dispatcher selected another queue subcommand"),
    }
    Ok(())
}
