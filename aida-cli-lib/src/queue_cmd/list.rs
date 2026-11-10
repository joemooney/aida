// trace:TASK-3426 | ai:codex
use super::*;

// BUG-618: build the `aida queue list --json` rows from queue entries +
// cache-backed summaries. Pure (no IO) so the JSON shape
// ({spec_id,title,status,for_role}) is unit-testable without a backend. The
// queue order is preserved; entries with no matching summary are dropped.
// Display id mirrors `Requirement::display_id` (agreed_id, then spec_id,
// then "?"). trace:BUG-618 | ai:claude
pub(crate) fn queue_json_rows(
    entries: &[aida_core::QueueEntry],
    summaries: &[aida_core::RequirementSummary],
) -> Vec<serde_json::Value> {
    let by_id: std::collections::HashMap<Uuid, &aida_core::RequirementSummary> =
        summaries.iter().map(|s| (s.id, s)).collect();
    entries
        .iter()
        .filter_map(|e| {
            by_id.get(&e.requirement_id).map(|s| {
                let display_id = s
                    .agreed_id
                    .as_deref()
                    .or(s.spec_id.as_deref())
                    .unwrap_or("?");
                serde_json::json!({
                    "spec_id": display_id,
                    "title": s.title,
                    "status": s.status,
                    "for_role": e.for_role,
                })
            })
        })
        .collect()
}

/// Human-readable chip for a reviewer-routed row's actionability. Rendered
/// inline -- rows never vanish because of this label, they're annotated.
// trace:BUG-1508 | ai:claude
pub(crate) fn reviewer_state_chip(
    state: crate::gate::review_verdict::ReviewActionability,
) -> String {
    match state {
        crate::gate::review_verdict::ReviewActionability::NeedsReview => {
            format!("[{}]", "needs-review".yellow().bold())
        }
        crate::gate::review_verdict::ReviewActionability::Waiting => {
            format!("[{}]", "waiting".yellow())
        }
        crate::gate::review_verdict::ReviewActionability::AwaitingRework => {
            format!("[{}]", "awaiting-rework".blue())
        }
        crate::gate::review_verdict::ReviewActionability::Resolved => {
            format!("[{}]", "resolved".green().dimmed())
        }
    }
}

/// STORY-672: render the fleet-wide `aida queue list --all-users` view.
///
/// Aggregates every user's queue (enumerated via `storage.queue_users()`),
/// groups by user then routing role, and prints the owning user per group so a
/// coordinator can see the whole fleet's queued work in one read. Read-only.
///
/// Title/status resolution reuses the SQLite-cache `summaries` (the BUG-618
/// pattern) rather than a full `storage.load()` YAML scan, so the view stays
/// fast even with many users. Filtering mirrors the default `queue list`:
///   - role routing: `--for <role>` narrows to that role; `--all` (the
///     default here is per-role just like the single-user path, but with no
///     active session role a fleet view spans every role anyway) is honored
///     via `resolve_queue_role_filter`;
///   - terminal (Completed/Rejected) and archived specs are hidden unless
///     `--include-terminal` is passed.
// trace:STORY-672
pub(crate) fn render_all_users_queue(
    storage: &Storage,
    summaries: &[aida_core::RequirementSummary],
    role: Option<&str>,
    all: bool,
    include_terminal: bool,
) -> Result<()> {
    use std::collections::BTreeMap;

    let by_id: std::collections::HashMap<Uuid, &aida_core::RequirementSummary> =
        summaries.iter().map(|s| (s.id, s)).collect();

    // Role filter: same truth table as the per-user path. We deliberately pass
    // `session_role = None` so the fleet view is not silently narrowed to the
    // coordinator's own active role — `--for <role>` is the explicit narrow.
    // trace:STORY-672
    let (role_filter, only_unrouted) = resolve_queue_role_filter(role, all, None);

    let users = storage.queue_users()?;

    let display_id = |s: &aida_core::RequirementSummary| -> String {
        let mut id = s
            .agreed_id
            .as_deref()
            .or(s.spec_id.as_deref())
            .unwrap_or("?")
            .to_string();
        if s.archived {
            id.push_str(" (archived)");
        }
        id
    };

    // user → role-bucket → rows. BTreeMap keeps users + roles deterministically
    // ordered. The role bucket key is the displayed routing label.
    let mut grouped: BTreeMap<String, BTreeMap<String, Vec<(String, String)>>> = BTreeMap::new();
    let mut total_rows: usize = 0;
    let mut hidden_terminal: usize = 0;

    for user in &users {
        let entries = match storage.queue_list(user, include_terminal) {
            Ok(v) => v,
            Err(_) => continue,
        };
        for e in &entries {
            if !entry_matches_role_filter(
                e.for_role.as_deref(),
                role_filter.as_deref(),
                only_unrouted,
            ) {
                continue;
            }
            let summary = by_id.get(&e.requirement_id);
            // Hide terminal / archived specs unless --include-terminal.
            if !include_terminal {
                if let Some(s) = summary {
                    if s.archived {
                        continue;
                    }
                    let st = s.status.to_ascii_lowercase();
                    if st == "completed" || st == "rejected" {
                        hidden_terminal += 1;
                        continue;
                    }
                }
            }
            let (id, title) = match summary {
                Some(s) => (display_id(s), s.title.clone()),
                None => (e.requirement_id.to_string(), "(unknown spec)".to_string()),
            };
            let role_label = e.for_role.clone().unwrap_or_else(|| "unrouted".to_string());
            grouped
                .entry(user.clone())
                .or_default()
                .entry(role_label)
                .or_default()
                .push((id, title));
            total_rows += 1;
        }
    }

    println!("{}", "Fleet queue — all users (read-only)".bold().cyan());
    println!();

    if total_rows == 0 {
        println!("{}", "No queued items across the fleet.".dimmed());
        if hidden_terminal > 0 {
            println!(
                "{}",
                format!(
                    "  {} terminal item(s) hidden; pass --include-terminal to show.",
                    hidden_terminal
                )
                .dimmed()
            );
        }
        return Ok(());
    }

    for (user, roles) in &grouped {
        let user_count: usize = roles.values().map(|v| v.len()).sum();
        println!(
            "{} {}",
            user.bold().green(),
            format!("({} queued)", user_count).dimmed()
        );
        for (role_label, rows) in roles {
            println!("  {}", format!("[{}]", role_label).yellow());
            for (id, title) in rows {
                println!("    {}  {}", id.cyan(), title);
            }
        }
        println!();
    }

    let role_count: usize = grouped
        .values()
        .flat_map(|roles| roles.keys())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    println!(
        "{}",
        format!(
            "fleet: {} queued across {} role(s) / {} user(s)",
            total_rows,
            role_count,
            grouped.len()
        )
        .dimmed()
    );
    if hidden_terminal > 0 {
        println!(
            "{}",
            format!(
                "  {} terminal item(s) hidden; pass --include-terminal to show.",
                hidden_terminal
            )
            .dimmed()
        );
    }

    Ok(())
}

pub(crate) fn queue_list_json_requested(json_flag: bool, format: Option<OutputFormat>) -> bool {
    // trace:BUG-1149 | ai:codex
    json_flag || matches!(format, Some(OutputFormat::Json))
}

// Apply the queue-list filters whose source of truth is the requirement store
// before the cache-fast JSON and TOON emitters project rows. Keep these rules
// aligned with the human path below. trace:BUG-1795 | ai:codex
pub(crate) fn queue_list_matches_requested_filters(
    entry: &aida_core::QueueEntry,
    store: &aida_core::RequirementsStore,
    scope_filter: Option<&str>,
    batch_filter: Option<&str>,
    tag_filter: Option<&str>,
    tag_prefix_filter: Option<&str>,
    role_scope: Option<&(Vec<String>, Option<String>)>,
    include_terminal: bool,
) -> bool {
    let req = store
        .requirements
        .iter()
        .find(|r| r.id == entry.requirement_id);
    if let Some(req) = req {
        if req.archived || (!include_terminal && is_terminal_status(&req.status)) {
            return false;
        }
    }
    if let Some(raw) = scope_filter {
        let wants: Vec<&str> = raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if wants.iter().any(|w| w.eq_ignore_ascii_case("none")) {
            if entry.for_scope.is_some() {
                return false;
            }
        } else {
            let explicit = entry
                .for_scope
                .as_deref()
                .is_some_and(|fs| wants.iter().any(|w| w.eq_ignore_ascii_case(fs)));
            let derived = req
                .and_then(|req| derive_parent_epic_label(req, store))
                .is_some_and(|label| wants.iter().any(|w| w.eq_ignore_ascii_case(&label)));
            if !explicit && !derived {
                return false;
            }
        }
    }
    if let Some((tags, status)) = role_scope {
        if let Some(req) = req {
            if status.as_deref().is_some_and(|want| {
                !format!("{}", req.status).eq_ignore_ascii_case(want)
                    && !format!("{:?}", req.status).eq_ignore_ascii_case(want)
            }) {
                return false;
            }
            if tags.iter().any(|tag| !req.tags.iter().any(|t| t == tag)) {
                return false;
            }
        } else if !tags.is_empty() || status.is_some() {
            return false;
        }
    }
    if let Some(name) = batch_filter {
        let Some(req) = req else { return false };
        let want = format!("batch:{}", normalize_batch_name(name));
        if !req.tags.iter().any(|t| t.eq_ignore_ascii_case(&want)) {
            return false;
        }
    }
    if let Some(want) = tag_filter {
        let Some(req) = req else { return false };
        if !tag_matches_exact(&req.tags, want) {
            return false;
        }
    }
    if let Some(prefix) = tag_prefix_filter {
        let Some(req) = req else { return false };
        if !tag_matches_prefix(&req.tags, prefix) {
            return false;
        }
    }
    true
}

// trace:TASK-3426 | ai:codex
pub(super) fn handle_queue_command(
    cmd: &QueueCommand,
    storage: &Storage,
    store_path: &std::path::Path,
) -> Result<()> {
    let get_user = |user: &Option<String>| -> String { current_user_id(user.as_deref()) };
    match cmd {
        QueueCommand::List {
            user,
            include_completed,
            role,
            all,
            no_scope,
            global,
            local,
            // --sync is handled at the dispatch site (it needs store_path
            // access); silently consume it here. trace:STORY-78 | ai:claude
            sync: _,
            include_terminal,
            scope: scope_filter,
            tree,
            no_in_flight,
            in_flight_only,
            batch: batch_filter,
            tag: tag_filter,
            tag_prefix: tag_prefix_filter,
            by_batch,
            json,
            all_users,
            epic: epic_filter,
            no_focus,
        } => {
            let user_id = get_user(user);

            // Read-side cross-user fallback: a `--for <role>` routing lands in
            // the ROUTING user's queue file, so reading only our own file hid
            // work a peer handed us. Resolve the role of interest once here
            // (explicit `--for` wins, else the active session role) and pass it
            // to every read below; `None` keeps the historical own-file-only
            // view. No storage semantics change — see the module docs.
            // trace:BUG-774 | ai:claude
            let queue_fallback_role = queue_role_fallback::fallback_role(
                role.as_deref(),
                queue_role_fallback::session_role_env().as_deref(),
            );

            // TASK-1052: opportunistic queue self-heal. Before rendering, prune
            // dead routed entries (archived/Completed/Rejected targets) from the
            // local queue so the underlying file self-heals on read, not just
            // the view. Best-effort and silent — only terminal corpses (which
            // the list view already hides) are removed, so visible output is
            // unchanged; a hiccup never blocks the read. The `--all-users`
            // fleet view reads every queue and isn't this shell's to mutate, so
            // skip the self-heal there. trace:TASK-1052 | ai:claude
            if !*all_users {
                let _ = opportunistic_queue_gc(storage, store_path, &user_id);
            }

            // STORY-706: a persistent focus defaults the `--epic` narrowing to
            // the focused subtree, so a plain `aida queue list` under a focus
            // shows only that epic's queued work. An explicit `--epic`, the
            // `--no-focus` escape, or the fleet-wide `--all-users` view all
            // bypass it. We reuse the TASK-923 `--epic` closure wholesale —
            // focus is just its default source — and print a loud header below
            // so the scoping is never silent.
            let focus_active = epic_filter.is_none() && !*no_focus && !*all_users;
            let focus_target = if focus_active {
                find_project_root()
                    .ok()
                    .and_then(|root| crate::focus::resolve_focus(&root))
            } else {
                None
            };
            let epic_filter: Option<String> = epic_filter.clone().or_else(|| focus_target.clone());
            let epic_filter = epic_filter.as_ref();
            let focus_drove_epic = focus_target.is_some();

            // STORY-672: fleet-wide bird's-eye. `--all-users` aggregates every
            // user's queue (not just this shell's `current_user_id()`),
            // grouped by user then role with the owning user shown per row.
            // Read-only; renders its own view and returns early so it never
            // perturbs the per-identity default path below. Title resolution
            // reuses the SQLite cache summaries (BUG-618's pattern) rather than
            // a slow `storage.load_for_read()` full YAML scan. trace:STORY-672
            if *all_users {
                let backend = advance_backend(store_path)?;
                let summaries = backend.list_summaries(&queue_dead_target_summary_filter())?;
                render_all_users_queue(
                    storage,
                    &summaries,
                    role.as_deref(),
                    *all,
                    *include_terminal,
                )?;
                return Ok(());
            }
            // BUG-616: cache-fast JSON queue read for the TUI queue panel.
            // The panel only needs the user's queue head, enriched with each
            // spec's display id / title / status — emitting it straight from
            // the cache-backed `storage.queue_list` + a SQLite-cache summary
            // read avoids the full `aida status` worktree/process scan (~3s).
            // Shape matches the status-overlay `QueueItem` the TUI already
            // parses ({spec_id,title,status,for_role}). Raw queue order, pre
            // role/scope display refinement. trace:BUG-616 | ai:claude
            //
            // BUG-618: resolve titles via the SQLite cache (`list_summaries`,
            // the same path `aida list` uses, ~0.2s) instead of `storage.load_for_read()`
            // — the legacy full YAML/git scan that cost ~1s on the cockpit
            // paint. `RequirementSummary` carries id/spec_id/agreed_id/title/
            // status, everything the JSON shape needs. trace:BUG-618 | ai:claude
            if queue_list_json_requested(*json, output_format_override()) {
                let mut raw = if *global {
                    Vec::new()
                } else {
                    queue_role_fallback::queue_list_with_role_fallback(
                        storage,
                        &user_id,
                        queue_fallback_role.as_deref(),
                        *include_completed,
                    )?
                };
                let filter_store = if scope_filter.is_some()
                    || batch_filter.is_some()
                    || tag_filter.is_some()
                    || tag_prefix_filter.is_some()
                    || (!*all && !*no_scope && active_role_scope().is_some())
                {
                    Some(storage.load_for_read()?)
                } else {
                    None
                };
                if let Some(full_store) = &filter_store {
                    let role_scope = if *all || *no_scope {
                        None
                    } else {
                        active_role_scope()
                    };
                    raw.retain(|entry| {
                        queue_list_matches_requested_filters(
                            entry,
                            full_store,
                            scope_filter.as_deref(),
                            batch_filter.as_deref(),
                            tag_filter.as_deref(),
                            tag_prefix_filter.as_deref(),
                            role_scope.as_ref(),
                            *include_terminal || *include_completed,
                        )
                    });
                }
                let backend = advance_backend(store_path)?;
                let summaries = backend.list_summaries(&aida_core::ListFilter::default())?;
                let mut rows = queue_json_rows(&raw, &summaries);
                // BUG-1508 AC1: annotate reviewer-routed rows with their
                // actionability. Local-only (verdict file + git refs) and
                // only paid for when a reviewer row is actually present, so
                // the common (non-reviewer) cache-fast read is unaffected.
                // trace:BUG-1508 | ai:claude
                if raw
                    .iter()
                    .any(|e| e.for_role.as_deref() == Some("reviewer"))
                {
                    if let Ok(project_root) = find_project_root() {
                        if let Ok(full_store) = storage.load_for_read() {
                            let leases = list_leases(&project_root);
                            for entry in raw
                                .iter()
                                .filter(|e| e.for_role.as_deref() == Some("reviewer"))
                            {
                                let Some(req) = full_store
                                    .requirements
                                    .iter()
                                    .find(|r| r.id == entry.requirement_id)
                                else {
                                    continue;
                                };
                                let display_id = req
                                    .agreed_id
                                    .as_deref()
                                    .or(req.spec_id.as_deref())
                                    .unwrap_or("?");
                                let state = reviewer_row_actionability(
                                    &project_root,
                                    req,
                                    &leases,
                                    |uuid| {
                                        full_store
                                            .requirements
                                            .iter()
                                            .find(|r| r.id == uuid)
                                            .and_then(|r| {
                                                r.agreed_id.clone().or_else(|| r.spec_id.clone())
                                            })
                                    },
                                );
                                if let Some(row) = rows.iter_mut().find(|r| {
                                    r.get("spec_id").and_then(|v| v.as_str()) == Some(display_id)
                                }) {
                                    if let Some(obj) = row.as_object_mut() {
                                        obj.insert(
                                            "reviewer_state".to_string(),
                                            serde_json::Value::String(state.as_str().to_string()),
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                println!("{}", serde_json::to_string(&rows)?);
                return Ok(());
            }
            // TASK-964: AGENT-MODE token-efficient TOON render of the queue.
            // Mirrors the BUG-616 `--json` shape (raw queue order, cache-resolved
            // id/title/status/for_role) as a uniform TOON table with a count
            // header, instead of the human grouped/Done-awaiting-merge view. The
            // human TTY path drops through unchanged. trace:TASK-964
            if agent_output_mode() {
                let mut raw = if *global {
                    Vec::new()
                } else {
                    queue_role_fallback::queue_list_with_role_fallback(
                        storage,
                        &user_id,
                        queue_fallback_role.as_deref(),
                        *include_completed,
                    )?
                };
                let filter_store = if scope_filter.is_some()
                    || batch_filter.is_some()
                    || tag_filter.is_some()
                    || tag_prefix_filter.is_some()
                    || (!*all && !*no_scope && active_role_scope().is_some())
                {
                    Some(storage.load_for_read()?)
                } else {
                    None
                };
                if let Some(full_store) = &filter_store {
                    let role_scope = if *all || *no_scope {
                        None
                    } else {
                        active_role_scope()
                    };
                    raw.retain(|entry| {
                        queue_list_matches_requested_filters(
                            entry,
                            full_store,
                            scope_filter.as_deref(),
                            batch_filter.as_deref(),
                            tag_filter.as_deref(),
                            tag_prefix_filter.as_deref(),
                            role_scope.as_ref(),
                            *include_terminal || *include_completed,
                        )
                    });
                }
                // BUG-1513: `queue_list_with_role_fallback` deliberately passes
                // the CALLER'S OWN entries through unfiltered (it widens the
                // caller's own queue with peers' role-routed additions, per
                // BUG-774) — that is correct for the passive/no-`--for` view,
                // but here an explicit `--for <role>` (or the active session
                // role) is a stated request for ONLY that role's rows. Without
                // this second pass, an own-queue entry routed to a different
                // role rode along in both the printed rows and the declared
                // `count:`, so the header disagreed with the filter it claimed
                // to apply. Re-derive the same role/only-unrouted resolution
                // the human TTY view already uses and apply it here too, so
                // every row this command emits actually satisfies the filter
                // it was asked for. trace:BUG-1513 | ai:claude
                let agent_session_role = std::env::var("AIDA_SESSION_ROLE").ok();
                let (agent_role_filter, agent_only_unrouted) =
                    resolve_queue_role_filter(role.as_deref(), *all, agent_session_role.as_deref());
                let backend = advance_backend(store_path)?;
                let summaries = backend.list_summaries(&aida_core::ListFilter::default())?;
                let by_id: std::collections::HashMap<Uuid, &aida_core::RequirementSummary> =
                    summaries.iter().map(|s| (s.id, s)).collect();
                // Mirror the human default: hide archived + terminal (Completed /
                // Rejected) entries unless the caller widened with
                // `--include-terminal` / `--include-completed`. Without this the
                // raw queue (which retains Done-awaiting-merge + shipped specs)
                // balloons the agent output far past the human view. trace:TASK-964
                let show_terminal = *include_terminal || *include_completed;
                // BUG-1508 AC1/AC4/AC7: as with the BUG-616 JSON panel read,
                // only load the full store + leases when a reviewer-routed
                // row is present, so the common agent read stays cache-fast.
                // trace:BUG-1508 | ai:claude
                let reviewer_ctx = if raw
                    .iter()
                    .any(|e| e.for_role.as_deref() == Some("reviewer"))
                {
                    find_project_root().ok().and_then(|root| {
                        storage.load_for_read().ok().map(|full_store| {
                            let leases = list_leases(&root);
                            (root, full_store, leases)
                        })
                    })
                } else {
                    None
                };
                let mut reviewer_routed = 0usize;
                let mut reviewer_actionable = 0usize;
                let rows: Vec<Vec<String>> = raw
                    .iter()
                    .filter(|e| {
                        entry_matches_role_filter(
                            e.for_role.as_deref(),
                            agent_role_filter.as_deref(),
                            agent_only_unrouted,
                        )
                    })
                    .filter_map(|e| {
                        let s = by_id.get(&e.requirement_id)?;
                        if !show_terminal {
                            if s.archived {
                                return None;
                            }
                            let st = s.status.to_ascii_lowercase();
                            if st == "completed" || st == "rejected" {
                                return None;
                            }
                        }
                        // The friendly label still resolves as input, so the
                        // next-step hint built from this cell keeps working.
                        // trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
                        let id = aida_core::node_alias::display_spec_id(
                            s.agreed_id
                                .as_deref()
                                .or(s.spec_id.as_deref())
                                .unwrap_or(""),
                        );
                        let reviewer_state = if e.for_role.as_deref() == Some("reviewer") {
                            reviewer_ctx
                                .as_ref()
                                .and_then(|(root, full_store, leases)| {
                                    full_store
                                        .requirements
                                        .iter()
                                        .find(|r| r.id == e.requirement_id)
                                        .map(|req| {
                                            reviewer_row_actionability(root, req, leases, |uuid| {
                                                full_store
                                                    .requirements
                                                    .iter()
                                                    .find(|r| r.id == uuid)
                                                    .and_then(|r| {
                                                        r.agreed_id
                                                            .clone()
                                                            .or_else(|| r.spec_id.clone())
                                                    })
                                            })
                                        })
                                })
                                .map(|state| {
                                    reviewer_routed += 1;
                                    if state == crate::gate::review_verdict::ReviewActionability::NeedsReview {
                                        reviewer_actionable += 1;
                                    }
                                    state.as_str().to_string()
                                })
                                .unwrap_or_default()
                        } else {
                            String::new()
                        };
                        Some(vec![
                            id,
                            s.title.clone(),
                            toon_status_token(&s.status),
                            e.for_role.clone().unwrap_or_default(),
                            reviewer_state,
                        ])
                    })
                    .collect();
                // BUG-1513 AC6: print the resolved caller identity alongside
                // the count — the queue is keyed off shell identity (BUG-89),
                // so two callers comparing a bare count with no identity
                // attached are not comparing the same filter's output.
                println!("count: {} for_user: {}", rows.len(), user_id);
                if reviewer_routed > 0 {
                    println!(
                        "reviewer: actionable {} of {} routed",
                        reviewer_actionable, reviewer_routed
                    );
                }
                println!(
                    "{}",
                    crate::toon::table_raw(
                        "queue",
                        &["id", "title", "status", "for_role", "reviewer_state"],
                        &rows
                    )
                );
                // TASK-974 (AXI #9): next-step block — start/show the queue head
                // when non-empty, else point at the approvable backlog to fill
                // it. The first cell of each row is the spec id. trace:TASK-974
                let first_id = rows.first().and_then(|r| r.first()).map(String::as_str);
                let next = crate::help_next::queue_next(first_id);
                if let Some(block) = crate::help_next::render(&next) {
                    println!("{block}");
                }
                return Ok(());
            }
            // TASK-475: nudge when the local orphan store lags origin — a
            // multi-node user otherwise sees a silently-stale listing. Uses
            // already-known refs (no fetch); best-effort to stderr so it never
            // pollutes the listing on stdout, and skipped at 0/unknown.
            // trace:TASK-475 | ai:claude
            if !*global {
                if let Ok(root) = find_project_root() {
                    if let Some(n) = orphan_store_behind_count(&root) {
                        if n > 0 {
                            eprintln!(
                                "{} orphan store is {} commit{} behind origin/aida-store. Run `{}` or `{}` to refresh.",
                                "Note:".yellow(),
                                n,
                                if n == 1 { "" } else { "s" },
                                "aida pull".cyan(),
                                "aida queue list --sync".cyan(),
                            );
                        }
                    }
                }
            }
            // TASK-270: tolerate a redundant `batch:` prefix on `--batch`
            // (the value this very command prints), so `--batch batch:NAME`
            // and `--batch NAME` resolve identically. trace:TASK-270
            let batch_filter: Option<String> = batch_filter
                .as_deref()
                .map(|n| normalize_batch_name(n).to_string());
            let raw_entries = if *global {
                Vec::new()
            } else {
                queue_role_fallback::queue_list_with_role_fallback(
                    storage,
                    &user_id,
                    queue_fallback_role.as_deref(),
                    *include_completed,
                )?
            };
            let store = storage.load()?;

            // Determine effective role filter. BUG-87: `--for X` is
            // explicit user intent and takes precedence over `--all`,
            // which only suppresses the *default* active-role filter.
            // See `resolve_queue_role_filter` for the truth table.
            // trace:BUG-87 | ai:claude
            let session_role = std::env::var("AIDA_SESSION_ROLE").ok();
            let (role_filter, only_unrouted) =
                resolve_queue_role_filter(role.as_deref(), *all, session_role.as_deref());

            // Phase 3 scope: AND the active role's scope_tags / scope_status
            // on top of the role-routing filter. --all and --no-scope both
            // bypass; --all also bypasses role routing.
            // trace:TASK-1-021 | ai:claude
            let scope = if *all || *no_scope {
                None
            } else {
                active_role_scope()
            };

            // STORY-57: scope/session routing filter — when in a session,
            // an entry tagged for_scope=X is only visible if X matches the
            // active lease (or `--all` bypasses).
            let self_lease_for_routing: Option<SessionLease> =
                std::env::current_dir().ok().and_then(|cwd| {
                    let project_root = storage
                        .path()
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| std::path::PathBuf::from("."));
                    active_lease_for_cwd(&project_root, &cwd)
                });

            // STORY-98: load every session manifest once so the loop can
            // tag entries that another live session has planned. Cheap
            // (handful of TOML files); falls back to empty on read error.
            // trace:STORY-98 | ai:claude
            let project_root_for_manifests = storage
                .path()
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let all_manifests = session_manifest::list_all(&project_root_for_manifests);
            let viewer_session_id = self_lease_for_routing
                .as_ref()
                .map(|l| l.id.clone())
                .unwrap_or_default();

            // TASK-46: track how many entries we hid because their req
            // is in a terminal status. Surfaced as a footer hint so the
            // user knows the listing isn't the whole queue.
            // trace:TASK-46 | ai:claude
            let mut hidden_terminal_count: usize = 0;

            let entries: Vec<&aida_core::QueueEntry> = raw_entries
                .iter()
                .filter(|e| {
                    entry_matches_role_filter(
                        e.for_role.as_deref(),
                        role_filter.as_deref(),
                        only_unrouted,
                    )
                })
                .filter(|e| entry_scope_session_match(e, self_lease_for_routing.as_ref(), *all))
                .filter(|e| {
                    // TASK-46: hide Completed/Rejected entries by default.
                    // Counts them for the footer hint, so the user can
                    // discover --include-terminal. trace:TASK-46 | ai:claude
                    if *include_terminal {
                        return true;
                    }
                    let Some(req) = store.requirements.iter().find(|r| r.id == e.requirement_id)
                    else {
                        return true;
                    };
                    if is_terminal_status(&req.status) {
                        hidden_terminal_count += 1;
                        return false;
                    }
                    true
                })
                .filter(|e| {
                    // BUG-504: an archived (shelved) spec must never show in
                    // `queue list` — it looks like pending work though it's been
                    // shelved. `aida archive <id>` dequeues on the way out
                    // (BUG-492), so this is a backstop for any archived spec a
                    // stale queue row still references. trace:BUG-504
                    store
                        .requirements
                        .iter()
                        .find(|r| r.id == e.requirement_id)
                        .map(|req| !req.archived)
                        .unwrap_or(true)
                })
                .filter(|e| {
                    queue_list_matches_requested_filters(
                        e,
                        &store,
                        scope_filter.as_deref(),
                        batch_filter.as_deref(),
                        tag_filter.as_deref(),
                        tag_prefix_filter.as_deref(),
                        scope.as_ref(),
                        *include_terminal,
                    )
                })
                .collect();

            // STORY-706: row count before the focus/epic narrowing, for the
            // focus header's "showing N of M".
            let total_pre_focus = entries.len();

            // TASK-923: --epic / --parent — narrow to the epic's TRANSITIVE
            // descendant tree (epic + children + grandchildren). Resolve the
            // id to its UUID, compute the closure once via the shared
            // hierarchy walk (`epic_descendant_uuid_set`, the same one
            // `aida graph tree` uses), then run the pure
            // `filter_entries_by_descendant_set` over the already-filtered
            // (role / scope / tag) entries — so the epic filter ANDs with
            // them. `epic_label` (resolved display id) drives the focused
            // empty-result message below. trace:TASK-923 | ai:claude
            let (entries, epic_label): (Vec<&aida_core::QueueEntry>, Option<String>) =
                if let Some(epic_ref) = epic_filter {
                    let epic_req = store
                        .requirements
                        .iter()
                        .find(|r| {
                            r.spec_id
                                .as_deref()
                                .is_some_and(|s| s.eq_ignore_ascii_case(epic_ref))
                                || r.agreed_id
                                    .as_deref()
                                    .is_some_and(|s| s.eq_ignore_ascii_case(epic_ref))
                        })
                        .ok_or_else(|| {
                            anyhow::anyhow!("--epic {}: requirement not found", epic_ref)
                        })?;
                    let label = epic_req.display_id();
                    let descendants = epic_descendant_uuid_set(&store, epic_req.id);
                    (
                        filter_entries_by_descendant_set(&entries, &descendants),
                        Some(label),
                    )
                } else {
                    (entries, None)
                };

            // STORY-706: when a persistent focus (not an explicit `--epic`)
            // drove the narrowing, announce it loudly so the scoped queue view
            // is never mistaken for the whole queue.
            if focus_drove_epic {
                if let Some(label) = &epic_label {
                    println!(
                        "{}",
                        crate::focus::focus_header(label, entries.len(), total_pre_focus)
                            .cyan()
                            .bold()
                    );
                }
            }

            // STORY-333: split `entries` into pickable + blocked. Blocked
            // entries render in a sibling "Blocked" section (below
            // "Done — awaiting merge"); pickable entries continue through
            // the existing render path. Additionally collect a set of
            // entries that are queued *ahead* of their unsatisfied
            // blocked-by blocker (AC9) so the main render can decorate
            // them with a warning marker. trace:STORY-333 | ai:claude
            #[derive(Clone)]
            struct BlockedEntry<'a> {
                req: &'a aida_core::Requirement,
                reason: aida_core::pickability::BlockedReason,
            }
            let mut blocked_entries: Vec<BlockedEntry<'_>> = Vec::new();
            let entries: Vec<&aida_core::QueueEntry> = entries
                .into_iter()
                .filter(|e| {
                    let Some(req) = store.requirements.iter().find(|r| r.id == e.requirement_id)
                    else {
                        return true;
                    };
                    match queue_fresh_pickup_policy(req, &store, false, store_path.parent()) {
                        QueueFreshPickup::Pickable => true,
                        QueueFreshPickup::Archived
                        | QueueFreshPickup::Deferred
                        | QueueFreshPickup::NeedsGuidedOrOperatorSession(_)
                        | QueueFreshPickup::NeedsReleaseOperatorSession
                        | QueueFreshPickup::AwaitingMerge
                        | QueueFreshPickup::AwaitingRework
                        | QueueFreshPickup::Terminal(_) => false,
                        QueueFreshPickup::Blocked(reason) => {
                            blocked_entries.push(BlockedEntry { req, reason });
                            false
                        }
                    }
                })
                .collect();
            // Set of UUIDs (dependent specs) whose queue position is
            // ahead of a queued, unsatisfied `BlockedBy` target.
            // Computed across ALL queue entries — including ones the
            // pickability gate filtered into `blocked_entries` — because
            // the inversion is a queue-ordering concern regardless of
            // whether the dependent is currently pickable. trace:STORY-333
            let inverted_ahead_of_blocker: std::collections::HashSet<uuid::Uuid> = {
                use std::collections::HashSet;
                let mut out: HashSet<uuid::Uuid> = HashSet::new();
                let raw_position_by_uuid: std::collections::HashMap<uuid::Uuid, i64> = raw_entries
                    .iter()
                    .map(|e| (e.requirement_id, e.position))
                    .collect();
                for (uuid, my_pos) in &raw_position_by_uuid {
                    let Some(req) = store.requirements.iter().find(|r| &r.id == uuid) else {
                        continue;
                    };
                    for rel in req
                        .relationships
                        .iter()
                        .filter(|r| matches!(r.rel_type, aida_core::RelationshipType::BlockedBy))
                    {
                        let target = store.requirements.iter().find(|r| r.id == rel.target_id);
                        if let Some(t) = target {
                            if matches!(t.status, aida_core::RequirementStatus::Completed) {
                                continue;
                            }
                        }
                        let Some(blocker_pos) = raw_position_by_uuid.get(&rel.target_id) else {
                            continue;
                        };
                        if my_pos < blocker_pos {
                            out.insert(*uuid);
                        }
                    }
                }
                out
            };

            // Load global entries for the active role unless --local was passed.
            // The global queue is role-scoped (one file per role) — it only
            // makes sense when there's a routed-role filter in effect.
            // `--for any` (only_unrouted) targets entries with no role,
            // which are not stored in the per-role global queue.
            // trace:FR-1-012 BUG-87
            let global_entries: Vec<global_queue::GlobalQueueEntry> = if *local || only_unrouted {
                Vec::new()
            } else if let Some(role_name) = &role_filter {
                global_queue::load(role_name).unwrap_or_default()
            } else {
                Vec::new()
            };

            // TASK-222: compute in-flight (Done) specs once, up front, so
            // the empty-queue branch can decide whether to keep showing the
            // "Your queue is empty" line or fall through to the in-flight
            // section when there's still work awaiting merge.
            // trace:TASK-222 | ai:claude
            let in_flight_specs: Vec<&aida_core::Requirement> = if *no_in_flight {
                Vec::new()
            } else {
                let batch_want = batch_filter.as_deref().map(|n| format!("batch:{}", n));
                store
                    .requirements
                    .iter()
                    .filter(|r| r.status == RequirementStatus::Done)
                    .filter(|r| {
                        // TASK-229: when --batch NAME is in effect, also
                        // gate the in-flight section to the same batch
                        // members so the view stays focused.
                        let Some(want) = &batch_want else {
                            return true;
                        };
                        r.tags.iter().any(|t| t.eq_ignore_ascii_case(want))
                    })
                    .collect()
            };

            let pending_empty = entries.is_empty() && global_entries.is_empty();

            // TASK-805: if a `burndown run` / `queue work --auto-complete` drain
            // is actively running, lead with a banner so the queue isn't read
            // as idle pending work when a drain is mid-flight. in-flight = specs
            // a live lease is working; scheduled = the queued pickable specs the
            // drain has claimed but not yet picked up. Reads the same
            // `.aida/drain.lock` (DrainOverlay::probe → drain_lock::probe_lock)
            // the drain writes — no parallel liveness probe. Resolve the SHARED
            // main-worktree root (the drain writes its lock there), so a sibling
            // worktree reads the orchestrator's lock. trace:TASK-805
            // BUG-753: probed ONCE here and reused by the path-to-empty footer
            // below, so the footer's recommendation agrees with the banner.
            // trace:BUG-753 | ai:claude
            let drain_overlay = find_main_worktree_root()
                .ok()
                .and_then(|r| DrainOverlay::probe(&r));
            if let Some(o) = &drain_overlay {
                // Display ids of the queued pickable specs (the drain's source
                // pool — queue membership IS the advisor sign-off).
                let queued_ids: Vec<String> = entries
                    .iter()
                    .filter_map(|e| {
                        store
                            .requirements
                            .iter()
                            .find(|r| r.id == e.requirement_id)
                            .map(|r| {
                                r.agreed_id
                                    .clone()
                                    .or_else(|| r.spec_id.clone())
                                    .unwrap_or_else(|| r.id.to_string())
                            })
                    })
                    .collect();
                let scheduled: Vec<String> = {
                    let mut s: Vec<String> = queued_ids
                        .into_iter()
                        .filter(|id| !o.in_flight.contains(id))
                        .collect();
                    s.sort();
                    s.dedup();
                    s
                };
                let mut in_flight: Vec<String> = o.in_flight.iter().cloned().collect();
                in_flight.sort();
                println!("{}", drain_running_banner(o.pid, &in_flight, &scheduled));
                println!();
            }

            // Branch matrix:
            //   in_flight_only=true   → skip the regular queue render entirely
            //   pending_empty + no_in_flight=true (or no in-flight)
            //                         → classic empty-queue early return
            //   pending_empty + in-flight has items
            //                         → muted empty hint + fall through to in-flight
            if !*in_flight_only && pending_empty {
                // BUG-87: when --all is already passed, don't suggest
                // "pass --all" — the user just did. Drop the hint.
                let hint = if *all {
                    ""
                } else {
                    "; pass --all to see your full queue"
                };
                // TASK-923: when the listing was narrowed to an epic, name
                // the epic in the empty message rather than the generic
                // "queue is empty" line. trace:TASK-923 | ai:claude
                if let Some(label) = &epic_label {
                    println!("{}", format!("No queued items under {}.", label).dimmed());
                } else if only_unrouted {
                    println!("{} (no unrouted items{})", "Your queue".dimmed(), hint,);
                } else if let Some(r) = &role_filter {
                    println!(
                        "{} (no items routed to role {}{})",
                        "Your queue".dimmed(),
                        r.cyan(),
                        hint,
                    );
                } else {
                    println!("{}", "Your queue is empty.".dimmed());
                }
                // TASK-46: if the queue *looks* empty only because all
                // remaining items are terminal-status, point at the escape
                // hatch so the user isn't confused. BUG-517: only on the
                // everything view (`--all`) — on the default role-filtered
                // list it's noise. trace:TASK-46 trace:BUG-517 | ai:claude
                if hidden_terminal_count > 0 && *all {
                    println!(
                        "  ({} terminal-status entr{} hidden; pass --include-terminal to show)",
                        hidden_terminal_count,
                        if hidden_terminal_count == 1 {
                            "y"
                        } else {
                            "ies"
                        }
                    );
                }
                let other_queue_count = queue_role_fallback::other_user_queue_entry_count(
                    storage,
                    &user_id,
                    *include_completed,
                );
                if other_queue_count > 0 && !*global && !*local {
                    println!(
                        "  ({})",
                        format!(
                            "{other_queue_count} item(s) exist under other queue identities; pass --all-users to inspect them"
                        )
                        .dimmed()
                    );
                }
                // STORY-333: keep going if the Blocked section has
                // anything to surface — even when the pickable list is
                // empty. The user's queue isn't really empty if
                // un-pickable items are sitting there waiting to be
                // unblocked or triaged. trace:STORY-333 | ai:claude
                if in_flight_specs.is_empty() && blocked_entries.is_empty() {
                    return Ok(());
                }
                // fall through to in-flight + blocked sections
            }

            // TASK-222: skip the regular-queue render entirely when only
            // the in-flight section was asked for, or when the pending list
            // was empty and we already printed the muted-empty hint above.
            // trace:TASK-222 | ai:claude
            let skip_regular_render = *in_flight_only || pending_empty;

            // BUG-1508 AC1/AC4/AC7: resolve each reviewer-routed row's
            // actionability LOCALLY (verdict file + `refs/remotes/origin/*` /
            // `refs/heads/*` -- no forge call), so the depth figure used for
            // capacity decisions can say "actionable N of M routed" instead
            // of a bare routed count that reads deep even when most rows are
            // really awaiting rework, not review. Only paid for when a
            // reviewer-routed row is actually present. trace:BUG-1508 | ai:claude
            let reviewer_state_by_entry: std::collections::HashMap<
                Uuid,
                crate::gate::review_verdict::ReviewActionability,
            > = if entries
                .iter()
                .any(|e| e.for_role.as_deref() == Some("reviewer"))
            {
                match find_project_root() {
                    Ok(project_root) => {
                        let leases = list_leases(&project_root);
                        entries
                            .iter()
                            .filter(|e| e.for_role.as_deref() == Some("reviewer"))
                            .filter_map(|e| {
                                store
                                    .requirements
                                    .iter()
                                    .find(|r| r.id == e.requirement_id)
                                    .map(|req| {
                                        (
                                            e.requirement_id,
                                            reviewer_row_actionability(
                                                &project_root,
                                                req,
                                                &leases,
                                                |uuid| {
                                                    store
                                                        .requirements
                                                        .iter()
                                                        .find(|r| r.id == uuid)
                                                        .and_then(|r| {
                                                            r.agreed_id
                                                                .clone()
                                                                .or_else(|| r.spec_id.clone())
                                                        })
                                                },
                                            ),
                                        )
                                    })
                            })
                            .collect()
                    }
                    Err(_) => std::collections::HashMap::new(),
                }
            } else {
                std::collections::HashMap::new()
            };
            let reviewer_routed_count = entries
                .iter()
                .filter(|e| e.for_role.as_deref() == Some("reviewer"))
                .count();
            let reviewer_actionable_count = reviewer_state_by_entry
                .values()
                .filter(|s| **s == crate::gate::review_verdict::ReviewActionability::NeedsReview)
                .count();

            if !skip_regular_render {
                let total = entries.len() + global_entries.len();
                let title = if only_unrouted {
                    format!(
                        "My Queue · unrouted ({} item{})",
                        total,
                        if total == 1 { "" } else { "s" }
                    )
                } else {
                    match &role_filter {
                        // BUG-1508 AC4/AC7: the reviewer queue's headline
                        // number is "actionable N of M routed" -- routed
                        // rows never vanish (AC2), but the figure that
                        // decides capacity is the actionable one, and both
                        // are shown together because the gap is the signal.
                        Some(r)
                            if r.eq_ignore_ascii_case("reviewer") && reviewer_routed_count > 0 =>
                        {
                            format!(
                                "My Queue · role:{} (actionable {} of {} routed)",
                                r, reviewer_actionable_count, reviewer_routed_count,
                            )
                        }
                        Some(r) => format!(
                            "My Queue · role:{} ({} item{})",
                            r,
                            total,
                            if total == 1 { "" } else { "s" }
                        ),
                        None => format!(
                            "My Queue ({} item{})",
                            total,
                            if total == 1 { "" } else { "s" }
                        ),
                    }
                };
                println!("{}", title.bold());
                println!("{}", "─".repeat(80));

                // Local-project name for tagging local entries when global is also
                // shown (so the user can tell at a glance which is which).
                let local_project_name = global_queue::project_name_for(
                    storage.path().parent().unwrap_or(storage.path()),
                );

                // TASK-33: --tree groups local entries by their derived parent
                // EPIC so a multi-cluster queue reads as discrete sub-batches
                // instead of one long interleaved list. Globals stay flat
                // after — we don't have foreign stores loaded to derive their
                // parents. trace:TASK-33 | ai:claude
                if *tree || *by_batch {
                    use std::collections::BTreeMap;
                    let mut groups: BTreeMap<String, Vec<&aida_core::QueueEntry>> = BTreeMap::new();
                    let unscoped_key = "~unscoped".to_string();
                    for entry in &entries {
                        let Some(req) = store
                            .requirements
                            .iter()
                            .find(|r| r.id == entry.requirement_id)
                        else {
                            groups.entry(unscoped_key.clone()).or_default().push(entry);
                            continue;
                        };
                        // TASK-238: --by-batch keys groups on the `batch:*`
                        // tag; the default --tree keys on parent EPIC.
                        let key = if *by_batch {
                            batch_tag_of(&req.tags)
                                .map(|t| t.to_string())
                                .unwrap_or_else(|| unscoped_key.clone())
                        } else {
                            entry
                                .for_scope
                                .clone()
                                .or_else(|| derive_parent_epic_label(req, &store))
                                .unwrap_or_else(|| unscoped_key.clone())
                        };
                        groups.entry(key).or_default().push(entry);
                    }
                    // Sort groups: real EPICs by count desc then name; unscoped last.
                    let mut ordered: Vec<(String, Vec<&aida_core::QueueEntry>)> =
                        groups.into_iter().collect();
                    ordered.sort_by(|a, b| {
                        let a_unscoped = a.0 == unscoped_key;
                        let b_unscoped = b.0 == unscoped_key;
                        match (a_unscoped, b_unscoped) {
                            (true, false) => std::cmp::Ordering::Greater,
                            (false, true) => std::cmp::Ordering::Less,
                            _ => b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)),
                        }
                    });

                    let render_entry_inline =
                        |entry: &aida_core::QueueEntry, is_last: bool, id_col_width: usize| {
                            let req = store
                                .requirements
                                .iter()
                                .find(|r| r.id == entry.requirement_id);
                            let display_id = req
                                .and_then(|r| r.agreed_id.as_deref().or(r.spec_id.as_deref()))
                                .unwrap_or("???");
                            let title = req.map(|r| r.title.as_str()).unwrap_or("(deleted)");
                            // TASK-91: PR-N (STORY-NNN) for auto-queued review
                            // stories. trace:TASK-91 | ai:claude
                            // trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
                            let (display_id_owned, title_owned) = format_review_story_display(
                                display_id, title,
                            )
                            .unwrap_or_else(|| {
                                (
                                    aida_core::node_alias::display_spec_id(display_id),
                                    title.to_string(),
                                )
                            });
                            let status = req
                                .map(|r| format!("{}", r.status))
                                .unwrap_or_else(|| "Unknown".to_string());
                            // TASK-269: shared glyph + colour palette.
                            // trace:TASK-269 | ai:claude
                            let status_badge = req
                                .map(status_display::parked_status_badge)
                                .unwrap_or_else(|| status_display::status_badge(&status));
                            let glyph = if is_last { "└─" } else { "├─" };
                            let pad =
                                " ".repeat(id_col_width.saturating_sub(display_id_owned.len()));
                            // TASK-238: surface the tag chip here too — the
                            // grouped (--tree / --by-batch) view must show
                            // tags just like the flat view, especially
                            // --by-batch, which exists for the tags.
                            // trace:TASK-238 | ai:claude
                            let tag_chip = req
                                .and_then(|r| format_tag_chip(&r.tags))
                                .map(|c| format!("  {}", format!("[{}]", c).dimmed()))
                                .unwrap_or_default();
                            // BUG-492: flag archived-but-queued specs here too.
                            // trace:BUG-492 | ai:claude
                            let archived_chip = if req.map(|r| r.archived).unwrap_or(false) {
                                format!("  {}", "[ARCHIVED]".red().bold())
                            } else {
                                String::new()
                            };
                            // Same borrowed-work attribution as the flat view.
                            // trace:BUG-774 | ai:claude
                            let routed_chip =
                                queue_role_fallback::routed_by_other_user(entry, &user_id)
                                    .map(|owner| {
                                        format!("  {}", format!("[queued by @{}]", owner).magenta())
                                    })
                                    .unwrap_or_default();
                            let supervised_chip = req
                                .and_then(queued_supervised_mode)
                                .map(|mode| {
                                    format!(
                                        "  {}",
                                        format!(
                                            "[{} needs guided/operator session ({mode}); headless drain skips]",
                                            crate::glyph(crate::glyphs::Glyph::FlowQueuedSupervised)
                                        )
                                        .yellow()
                                    )
                                })
                                .unwrap_or_default();
                            // BUG-1508: reviewer-routed rows get an
                            // actionability chip -- needs-review /
                            // awaiting-rework / resolved.
                            let reviewer_state_chip_str = reviewer_state_by_entry
                                .get(&entry.requirement_id)
                                .map(|s| format!("  {}", reviewer_state_chip(*s)))
                                .unwrap_or_default();
                            println!(
                                "  {} {}{}  {}  [{}]{}{}{}{}{}",
                                glyph.dimmed(),
                                display_id_owned.bold(),
                                pad,
                                title_owned,
                                status_badge,
                                archived_chip,
                                routed_chip,
                                supervised_chip,
                                tag_chip,
                                reviewer_state_chip_str,
                            );
                        };

                    for (key, group) in &ordered {
                        let header = if key == &unscoped_key {
                            if *by_batch {
                                "No batch".to_string()
                            } else {
                                "Unscoped".to_string()
                            }
                        } else {
                            key.clone()
                        };
                        println!();
                        println!(
                            "{} ({} item{})",
                            header.cyan().bold(),
                            group.len(),
                            if group.len() == 1 { "" } else { "s" }
                        );
                        // TASK-91: width must account for the PR-N (STORY-NNN)
                        // expansion so the column lines up after the transform.
                        // trace:TASK-91 | ai:claude
                        let id_col_width = group
                            .iter()
                            .map(|e| {
                                let req =
                                    store.requirements.iter().find(|r| r.id == e.requirement_id);
                                let raw_id = req
                                    .and_then(|r| r.agreed_id.as_deref().or(r.spec_id.as_deref()))
                                    .unwrap_or("???");
                                let raw_title = req.map(|r| r.title.as_str()).unwrap_or("");
                                match format_review_story_display(raw_id, raw_title) {
                                    Some((expanded, _)) => expanded.len(),
                                    None => aida_core::node_alias::display_spec_id(raw_id).len(),
                                }
                            })
                            .max()
                            .unwrap_or(0);
                        for (i, entry) in group.iter().enumerate() {
                            let is_last = i + 1 == group.len();
                            render_entry_inline(entry, is_last, id_col_width);
                        }
                    }

                    if !global_entries.is_empty() {
                        println!();
                        println!(
                            "{} ({} item{})",
                            "Global queue".cyan().bold(),
                            global_entries.len(),
                            if global_entries.len() == 1 { "" } else { "s" }
                        );
                        for (i, entry) in global_entries.iter().enumerate() {
                            let glyph = if i + 1 == global_entries.len() {
                                "└─"
                            } else {
                                "├─"
                            };
                            // BUG-83: prefer cached agreed_id over spec_id to
                            // stay consistent with `aida list` / local queue
                            // after `aida db merge-gate`. trace:BUG-83 | ai:claude
                            let display_id = entry
                                .agreed_id
                                .as_deref()
                                .or(entry.spec_id.as_deref())
                                .unwrap_or("???");
                            let title = entry.title.as_deref().unwrap_or("(no cached title)");
                            // trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
                            println!(
                                "  {} {}  {}  {}",
                                glyph.dimmed(),
                                aida_core::node_alias::display_spec_id(display_id).bold(),
                                title,
                                format!("[origin:{}]", entry.project_name).dimmed(),
                            );
                        }
                    }

                    if hidden_terminal_count > 0 {
                        println!();
                        println!(
                            "{} ({} pass --include-terminal to show)",
                            format!(
                                "{} terminal-status entr{} hidden",
                                hidden_terminal_count,
                                if hidden_terminal_count == 1 {
                                    "y"
                                } else {
                                    "ies"
                                }
                            )
                            .dimmed(),
                            "Completed/Rejected;".dimmed()
                        );
                    }
                    return Ok(());
                }

                for (i, entry) in entries.iter().enumerate() {
                    let req = store
                        .requirements
                        .iter()
                        .find(|r| r.id == entry.requirement_id);
                    // BUG-81: prefer agreed_id (short form) when assigned;
                    // fall back to spec_id. Mirrors `aida list` / `aida show` /
                    // `aida search` so the queue stops drifting after
                    // `aida db merge-gate`. trace:BUG-81 | ai:claude
                    let display_id = req
                        .and_then(|r| r.agreed_id.as_deref().or(r.spec_id.as_deref()))
                        .unwrap_or("???");
                    let title = req.map(|r| r.title.as_str()).unwrap_or("(deleted)");
                    // TASK-91: PR-N (STORY-NNN) for auto-queued review stories.
                    // trace:TASK-91 | ai:claude
                    // trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
                    let (display_id_owned, title_owned) =
                        format_review_story_display(display_id, title).unwrap_or_else(|| {
                            (
                                aida_core::node_alias::display_spec_id(display_id),
                                title.to_string(),
                            )
                        });
                    let status = req
                        .map(|r| format!("{}", r.status))
                        .unwrap_or_else(|| "Unknown".to_string());
                    // TASK-269: shared glyph + colour palette.
                    // trace:TASK-269 | ai:claude
                    let status_badge = req
                        .map(status_display::parked_status_badge)
                        .unwrap_or_else(|| status_display::status_badge(&status));

                    print!(
                        "  {}. {} {}",
                        (i + 1).to_string().dimmed(),
                        display_id_owned.bold(),
                        title_owned
                    );
                    print!("  [{}]", status_badge);
                    // BUG-1508: reviewer-routed rows get an actionability
                    // chip -- needs-review / awaiting-rework / resolved.
                    // The row itself never vanishes; this only annotates it.
                    if let Some(state) = reviewer_state_by_entry.get(&entry.requirement_id) {
                        print!("  {}", reviewer_state_chip(*state));
                    }
                    // BUG-492: an archived spec that is still queued is
                    // contradictory state (`aida list` hides it, this view
                    // keeps showing it). Flag it loudly so the user can
                    // reconcile (unarchive or dequeue). trace:BUG-492
                    if req.map(|r| r.archived).unwrap_or(false) {
                        print!("  {}", "[ARCHIVED]".red().bold());
                    }
                    // TASK-1234: make queued-but-supervised work visually
                    // distinct from ordinary drainable queue rows. This mirrors
                    // the headless-drain skip policy but does not change pickup.
                    if let Some(mode) = req.and_then(queued_supervised_mode) {
                        print!(
                            "  {}",
                            format!(
                                "[{} needs guided/operator session ({mode}); headless drain skips]",
                                crate::glyph(crate::glyphs::Glyph::FlowQueuedSupervised)
                            )
                            .yellow()
                        );
                    }
                    // An entry surfaced out of ANOTHER user's queue file is
                    // borrowed work — say whose queue it came from so it is
                    // never mistaken for something this shell queued. The
                    // `(from @…)` added-by chip would repeat the same name, so
                    // this chip replaces it. trace:BUG-774 | ai:claude
                    match queue_role_fallback::routed_by_other_user(entry, &user_id) {
                        Some(owner) => {
                            print!("  {}", format!("[queued by @{}]", owner).magenta());
                        }
                        None => {
                            if entry.added_by != user_id {
                                print!("  {}", format!("(from @{})", entry.added_by).dimmed());
                            }
                        }
                    }
                    // STORY-57: inline routing tags. Show for: only when the
                    // user isn't already filtering on a specific role (avoids
                    // repeating "for:implementer" on every line in the
                    // role-filtered view). Always show scope/session — those
                    // are session-axis filters that don't get hoisted into
                    // the title bar.
                    if role_filter.is_none() {
                        if let Some(ref r) = entry.for_role {
                            print!("  {}", format!("[for:{}]", r).cyan());
                        }
                    }
                    if let Some(ref s) = entry.for_scope {
                        print!("  {}", format!("[@{}]", s).cyan());
                    } else if let Some(req) = req {
                        // TASK-44: auto-derive the parent-EPIC chip when
                        // no explicit `--scope` was set. Dimmed + `*`
                        // suffix to distinguish from explicit routing.
                        // trace:TASK-44 | ai:claude
                        if let Some(epic) = derive_parent_epic_label(req, &store) {
                            print!("  {}", format!("[@{}*]", epic).dimmed());
                        }
                    }
                    if let Some(ref s) = entry.for_session {
                        let short = &s[..s.len().min(8)];
                        print!("  {}", format!("[session:{}]", short).cyan());
                    }
                    // STORY-98: if another session's manifest plans this spec,
                    // surface a `[planned:by-<short>]` chip so concurrent
                    // sessions can see the soft claim. The viewer's own
                    // session is skipped (the chip would be redundant — the
                    // user already knows what their own /aida-pickup queued).
                    // trace:STORY-98 | ai:claude
                    if let Some(req) = req {
                        if let Some(spec_id) = req.spec_id.as_deref() {
                            if let Some(other) = session_manifest::planned_by_other(
                                &all_manifests,
                                spec_id,
                                &viewer_session_id,
                            ) {
                                let short = &other[..other.len().min(8)];
                                print!("  {}", format!("[planned:by-{}]", short).magenta());
                            }
                        }
                    }
                    // When the global queue is also being shown, tag local entries
                    // with their origin so the merge view is unambiguous.
                    if !global_entries.is_empty() {
                        print!("  {}", format!("[origin:{}]", local_project_name).dimmed());
                    }
                    // TASK-238: surface the requirement's tags inline so
                    // the batch:* convention is visible without a per-item
                    // `aida show`. trace:TASK-238 | ai:claude
                    if let Some(chip) = req.and_then(|r| format_tag_chip(&r.tags)) {
                        print!("  {}", format!("[{}]", chip).dimmed());
                    }
                    if let Some(ref note) = entry.note {
                        print!("  {}", format!("\"{}\"", note).dimmed().italic());
                    }
                    println!();
                }

                // Global entries follow the locals, numbered continuously.
                // We can't apply scope_tags / scope_status filters since we don't
                // have the foreign requirement loaded — surface them all and rely
                // on the cached spec_id/title in the entry. trace:FR-1-012
                for (idx, entry) in global_entries.iter().enumerate() {
                    let i = entries.len() + idx;
                    // BUG-83: prefer cached agreed_id; falls back to spec_id.
                    // trace:BUG-83 | ai:claude
                    let display_id = entry
                        .agreed_id
                        .as_deref()
                        .or(entry.spec_id.as_deref())
                        .unwrap_or("???");
                    let title = entry.title.as_deref().unwrap_or("(no cached title)");

                    print!(
                        "  {}. {} {}",
                        (i + 1).to_string().dimmed(),
                        display_id.bold(),
                        title
                    );
                    if entry.added_by != user_id {
                        print!("  {}", format!("(from @{})", entry.added_by).dimmed());
                    }
                    if role_filter.is_none() {
                        print!("  {}", format!("[for:{}]", entry.for_role).cyan());
                    }
                    print!("  {}", format!("[origin:{}]", entry.project_name).dimmed());
                    if let Some(ref note) = entry.note {
                        print!("  {}", format!("\"{}\"", note).dimmed().italic());
                    }
                    println!();
                }
                // TASK-46: footer hint when the default filter hid some
                // terminal-status entries. Stays silent when nothing was
                // hidden so it doesn't add noise to the common case.
                // trace:TASK-46 | ai:claude
                if hidden_terminal_count > 0 {
                    println!();
                    println!(
                        "{} ({} pass --include-terminal to show)",
                        format!(
                            "{} terminal-status entr{} hidden",
                            hidden_terminal_count,
                            if hidden_terminal_count == 1 {
                                "y"
                            } else {
                                "ies"
                            }
                        )
                        .dimmed(),
                        "Completed/Rejected;".dimmed()
                    );
                }
            } // end of `if !skip_regular_render { ... }` — trace:TASK-222

            // TASK-304: surface the ultraplan suggestion for the head row
            // (the first pickable entry) under `[ultraplan] mode =
            // "suggested"`. Mirrors the nudge `aida queue next` / `work`
            // already print, so all three pickup surfaces agree.
            // trace:TASK-304 | ai:claude
            if !skip_regular_render {
                if let Some(head) = entries.first() {
                    if let Some(req) = store
                        .requirements
                        .iter()
                        .find(|r| r.id == head.requirement_id)
                    {
                        if let Ok(root) = find_project_root() {
                            print_ultraplan_suggestion_hint(&root, req);
                        }
                    }
                }
            }

            // STORY-333: ahead-of-blocker warning. Surface every queued
            // entry (pickable OR blocked) whose blocked-by target is
            // ALSO queued at a later position — the operator queued the
            // dependent before the blocker, which is sometimes deliberate
            // but worth flagging at list time. AC9: never refuse the
            // ordering; just warn. trace:STORY-333 | ai:claude
            if !skip_regular_render && !inverted_ahead_of_blocker.is_empty() {
                let mut lines: Vec<String> = Vec::new();
                let mut seen: std::collections::HashSet<uuid::Uuid> =
                    std::collections::HashSet::new();
                for uuid in &inverted_ahead_of_blocker {
                    if !seen.insert(*uuid) {
                        continue;
                    }
                    let Some(req) = store.requirements.iter().find(|r| &r.id == uuid) else {
                        continue;
                    };
                    let dep_display = req
                        .agreed_id
                        .as_deref()
                        .or(req.spec_id.as_deref())
                        .unwrap_or("?");
                    let mut blocker_displays: Vec<String> = Vec::new();
                    for rel in req
                        .relationships
                        .iter()
                        .filter(|r| matches!(r.rel_type, aida_core::RelationshipType::BlockedBy))
                    {
                        if let Some(target) =
                            store.requirements.iter().find(|r| r.id == rel.target_id)
                        {
                            if matches!(target.status, aida_core::RequirementStatus::Completed) {
                                continue;
                            }
                            let t_display = target
                                .agreed_id
                                .as_deref()
                                .or(target.spec_id.as_deref())
                                .unwrap_or("?");
                            blocker_displays.push(t_display.to_string());
                        }
                    }
                    if blocker_displays.is_empty() {
                        continue;
                    }
                    lines.push(format!(
                        "  {}  {} queued ahead of {}, which blocks it",
                        crate::glyph(crate::glyphs::Glyph::Warning),
                        dep_display.bold(),
                        blocker_displays.join(", ").bold()
                    ));
                }
                if !lines.is_empty() {
                    println!();
                    println!("{}", "Out-of-order blockers:".yellow().bold());
                    for l in &lines {
                        println!("{}", l.yellow());
                    }
                }
            }

            // TASK-222: in-flight section. Done specs are work-in-flight
            // (branch finished, not yet merged to main). They aren't in
            // the queue anymore (queue done removed them), but they're
            // still active work — so show them in a separate section so
            // the natural "what am I waiting on" view is complete.
            // trace:TASK-222 | ai:claude
            if !in_flight_specs.is_empty() {
                if !skip_regular_render {
                    println!();
                }
                println!(
                    "{} ({} item{})",
                    "Done — awaiting merge".bright_green().bold(),
                    in_flight_specs.len(),
                    if in_flight_specs.len() == 1 { "" } else { "s" }
                );
                println!("{}", "─".repeat(80));
                // TASK-234: grouped render — bucket the Done specs by PR
                // + state (awaiting merge / stuck / awaiting commit),
                // each with a concrete `Next` action, instead of a flat
                // list under one descriptive hint. trace:TASK-234
                let project_root = storage
                    .path()
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                render_in_flight_grouped(&in_flight_specs, &store.requirements, &project_root);
                if !*in_flight_only {
                    println!(
                        "{}",
                        "  (suppress this section with `--no-in-flight`; show only this section with `--in-flight-only`)"
                            .dimmed()
                    );
                }
            } else if *in_flight_only {
                println!("{}", "No in-flight (Done-status) specs.".dimmed());
            }

            // STORY-333: Blocked section — sibling of "Done — awaiting
            // merge". Lists every queued entry that the pre-pickup gate
            // would skip, with its reason (blocked-by target +
            // target status, REJECTED → permanent, human-only). Each
            // entry is visible — never silently swallowed — and the
            // permanent-block case shouts red. trace:STORY-333 | ai:claude
            if !blocked_entries.is_empty() && !*in_flight_only {
                println!();
                println!(
                    "{} ({} item{})",
                    "Blocked".yellow().bold(),
                    blocked_entries.len(),
                    if blocked_entries.len() == 1 { "" } else { "s" }
                );
                println!("{}", "─".repeat(80));
                for be in &blocked_entries {
                    let display_id = be
                        .req
                        .agreed_id
                        .as_deref()
                        .or(be.req.spec_id.as_deref())
                        .unwrap_or("???");
                    let reason_label = aida_core::pickability::pickability_reason_label(&be.reason);
                    let label_styled = match be.reason {
                        aida_core::pickability::BlockedReason::PermanentlyBlocked { .. } => {
                            reason_label.red().bold().to_string()
                        }
                        aida_core::pickability::BlockedReason::HumanOnly => {
                            reason_label.magenta().to_string()
                        }
                        // TASK-131: needs-triage gets magenta+bold to match
                        // the `Needs Attention` status badge palette, so
                        // a punted spec reads visually as "decide something
                        // here" in both surfaces. trace:TASK-131 | ai:claude
                        aida_core::pickability::BlockedReason::NeedsTriage => {
                            reason_label.magenta().bold().to_string()
                        }
                        // trace:STORY-1642 | ai:claude — knowledge rows
                        // are reference material; dim, not alarming.
                        aida_core::pickability::BlockedReason::KnowledgeType { .. } => {
                            reason_label.dimmed().to_string()
                        }
                        aida_core::pickability::BlockedReason::UnsatisfiedBlocker { .. } => {
                            reason_label.yellow().to_string()
                        }
                    };
                    println!(
                        "  · {}  {}  —  {}",
                        display_id.bold(),
                        be.req.title,
                        label_styled,
                    );
                }
            }

            // TASK-101: base-freshness — flag any active session lease whose
            // branch has fallen behind origin/main, so the operator sees when
            // in-flight work is on a stale base *during* the session (not just
            // at the queue-work/queue-done gates). Subtle by default; only retained
            // (including legacy non-stale) leases on a non-default branch are probed, and each
            // row's `git rev-list --count` reads local refs — NO fetch here — so
            // it is best-effort (a git miss just omits that row).
            // trace:TASK-101 | ai:claude
            if !*in_flight_only {
                if let Some(root) = find_main_worktree_root()
                    .ok()
                    .or_else(|| find_project_root().ok())
                {
                    // trace:TASK-3420 | ai:codex
                    let mut stale_base_rows: Vec<(String, String, String)> = Vec::new();
                    let reader = crate::ownership::reader::LeaseReader::new(&root);
                    for l in list_leases(&root) {
                        if l.branch.is_empty() || l.branch == "main" {
                            continue;
                        }
                        if matches!(
                            reader.pickup_owner(&l, crate::ownership::reader::LegacyRule::NonStale),
                            crate::ownership::OwnerVerdict::Dead { .. }
                        ) {
                            continue;
                        }
                        let Some(behind) = commits_behind_origin_main(&root, &l.branch) else {
                            continue;
                        };
                        // Threshold 1: the queue-list surface shows any non-zero
                        // drift (the statusline uses the higher warn threshold).
                        let Some(text) = base_behind_indicator(behind, 1) else {
                            continue;
                        };
                        stale_base_rows.push((l.scope.clone(), l.branch.clone(), text));
                    }
                    if !stale_base_rows.is_empty() {
                        println!();
                        println!(
                            "{} ({} lease{})",
                            "Active leases on stale base".yellow().bold(),
                            stale_base_rows.len(),
                            if stale_base_rows.len() == 1 { "" } else { "s" }
                        );
                        for (scope, branch, text) in &stale_base_rows {
                            println!(
                                "  {}  {}  {}",
                                truncate(scope, 20).bold(),
                                truncate(branch, 24).dimmed(),
                                text.yellow(),
                            );
                        }
                    }
                }
            }

            // STORY-565: "how do I get to zero?" footer. SIGNPOSTING over the
            // SAME classifier `aida queue advance` uses — disambiguate drain
            // (`aida burndown run`, does the work) from clear (`aida queue
            // clear`, just drops them), and name the single next action for
            // each non-ready queued item. We cover the items this list actually
            // printed: the pickable `entries` plus the queued-but-`blocked_entries`
            // (both are the operator's own queue). In-flight (Done) specs are
            // already off the queue, so they're excluded. Build the OpenFacts
            // index the way `handle_queue_advance` does. Suppressed under
            // `--in-flight-only` (the regular queue render was skipped, so a
            // "how to empty the queue" footer would be off-topic).
            // trace:STORY-565
            if !*in_flight_only {
                // Collect the display-ids the list rendered, in order, deduped.
                let mut queued_ids: Vec<String> = Vec::new();
                let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
                let mut push_display = |display: String| {
                    if seen.insert(display.to_ascii_uppercase()) {
                        queued_ids.push(display);
                    }
                };
                // BUG-753: the pickable entries' display ids, kept separately —
                // these are the drain's source pool (queue membership IS the
                // sign-off), so a live drain's schedule is derived from them.
                // trace:BUG-753 | ai:claude
                let mut entry_display_ids: Vec<String> = Vec::new();
                for e in &entries {
                    if let Some(req) = store.requirements.iter().find(|r| r.id == e.requirement_id)
                    {
                        let display = req
                            .agreed_id
                            .clone()
                            .or_else(|| req.spec_id.clone())
                            .unwrap_or_else(|| req.id.to_string());
                        entry_display_ids.push(display.clone());
                        push_display(display);
                    }
                }
                for be in &blocked_entries {
                    let display = be
                        .req
                        .agreed_id
                        .clone()
                        .or_else(|| be.req.spec_id.clone())
                        .unwrap_or_else(|| be.req.id.to_string());
                    push_display(display);
                }

                if !queued_ids.is_empty() {
                    // Build the open-facts index once, keyed by UPPERCASE
                    // display id — same construction `handle_queue_advance` uses.
                    let in_flight = find_project_root()
                        .ok()
                        .map(|r| in_flight_lease_role_map(&r))
                        .unwrap_or_default();
                    let facts_by_id: std::collections::HashMap<String, burndown::OpenFacts> =
                        collect_open_facts(&store, &in_flight)
                            .into_iter()
                            .map(|f| (f.id.to_ascii_uppercase(), f))
                            .collect();

                    // BUG-772: index the store by display id so a Blocked item
                    // can resolve its own BlockedBy predecessors — the footer
                    // must know whether a block clears itself (predecessor is
                    // authorized work already moving) or is a real dead end,
                    // instead of advising `--status rejected` for every blocked
                    // spec. trace:BUG-772 | ai:claude
                    let req_by_id: std::collections::HashMap<String, &aida_core::Requirement> =
                        store
                            .requirements
                            .iter()
                            .map(|r| {
                                let display = r
                                    .agreed_id
                                    .clone()
                                    .or_else(|| r.spec_id.clone())
                                    .unwrap_or_else(|| r.id.to_string());
                                (display.to_ascii_uppercase(), r)
                            })
                            .collect();

                    let items: Vec<burndown::QueuedItem> = queued_ids
                        .into_iter()
                        .filter_map(|display| {
                            let key = display.to_ascii_uppercase();
                            let facts = facts_by_id.get(&key)?;
                            let (bucket, _reason) = burndown::explain_open(facts);
                            // Only a Blocked item needs the predecessor walk.
                            let blocker = (bucket == burndown::OpenBucket::Blocked)
                                .then(|| {
                                    req_by_id.get(&key).and_then(|req| {
                                        burndown::classify_blockers(&unsatisfied_blocker_facts(
                                            req, &store,
                                        ))
                                    })
                                })
                                .flatten();
                            Some(burndown::QueuedItem::new(display, bucket).with_blocker(blocker))
                        })
                        .collect();

                    // BUG-753: hand the footer the live drain's coverage (same
                    // probe the banner above used) so it stops recommending a
                    // `burndown run` launch the single-drain lock would refuse
                    // for specs the running drain already scheduled.
                    // BUG-765: pass the in-flight IDS (not just a count) so the
                    // per-item stalled hint keys on the same drain membership
                    // the banner renders — a spec the drain is working never
                    // reads as stalled/pick-it-back-up.
                    // trace:BUG-753 trace:BUG-765 | ai:claude
                    let drain_footer =
                        drain_overlay
                            .as_ref()
                            .map(|o| burndown::DrainFooterOverlay {
                                scheduled: entry_display_ids
                                    .iter()
                                    .filter(|id| !o.in_flight.contains(id.as_str()))
                                    .map(|id| id.to_ascii_uppercase())
                                    .collect(),
                                in_flight: o
                                    .in_flight
                                    .iter()
                                    .map(|id| id.to_ascii_uppercase())
                                    .collect(),
                            });
                    if let Some(footer) =
                        burndown::render_path_to_empty(&items, drain_footer.as_ref())
                    {
                        println!();
                        // Static framing colorized; the per-item SPEC-IDs and
                        // command snippets ride through plain.
                        for (i, line) in footer.lines().enumerate() {
                            if i == 0 {
                                println!("{}", line.bold());
                            } else {
                                println!("{}", line);
                            }
                        }
                    }
                }
            }

            // STORY-672: legibility — the per-identity scoping (your user +
            // active role) is not obvious. On the default human view, point at
            // the fleet-wide aggregate so the bird's-eye is discoverable. Kept
            // off the in-flight-only and empty paths to avoid noise.
            // trace:STORY-672
            if !skip_regular_render && !pending_empty {
                println!();
                println!(
                    "{}",
                    "Showing your queue. Pass --all-users for the fleet-wide view.".dimmed()
                );
            }
        }
        QueueCommand::Load { user } => {
            let user_id = get_user(user);
            let store = storage.load()?;
            let entries = storage.queue_list(&user_id, false)?;
            let queued_ids: HashSet<Uuid> = entries.iter().map(|e| e.requirement_id).collect();
            let project_root = find_project_root()
                .map(|p| main_worktree_root_from(&p))
                .unwrap_or_else(|_| {
                    storage
                        .path()
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| std::path::PathBuf::from("."))
                });
            print_effort_load_for_requirements(
                &project_root,
                "Queue load",
                store
                    .requirements
                    .iter()
                    .filter(|r| queued_ids.contains(&r.id)),
            );
        }
        _ => unreachable!("dispatcher selected another queue subcommand"),
    }
    Ok(())
}
