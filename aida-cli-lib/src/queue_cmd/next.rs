// trace:TASK-3426 | ai:codex
use super::*;

// trace:TASK-3426 | ai:codex
pub(super) fn handle_queue_command(
    cmd: &QueueCommand,
    storage: &Storage,
    store_path: &std::path::Path,
) -> Result<()> {
    let get_user = |user: &Option<String>| -> String { current_user_id(user.as_deref()) };
    match cmd {
        // trace:EPIC-1-001 | ai:claude
        QueueCommand::Next {
            role,
            all,
            user,
            no_scope,
            global,
            local,
        } => {
            let user_id = get_user(user);
            let session_role = std::env::var("AIDA_SESSION_ROLE").ok();
            let queue_fallback_role =
                queue_role_fallback::fallback_role(role.as_deref(), session_role.as_deref());
            let raw_entries = if *global {
                Vec::new()
            } else {
                queue_role_fallback::queue_list_with_role_fallback(
                    storage,
                    &user_id,
                    queue_fallback_role.as_deref(),
                    /* include_completed */ false,
                )?
            };
            let store = storage.load()?;

            // Same role-filter logic as queue list (BUG-87).
            // `--for X` takes precedence over `--all`. trace:BUG-87 | ai:claude
            let (role_filter, only_unrouted) =
                resolve_queue_role_filter(role.as_deref(), *all, session_role.as_deref());

            // Phase 3 scope filter (see queue list).
            // trace:TASK-1-021 | ai:claude
            let scope = if *all || *no_scope {
                None
            } else {
                active_role_scope()
            };

            // STORY-48: skip queue items whose target spec is owned by
            // another active session. Honors `[session].enforcement`:
            //   off   → no filtering
            //   warn  → entry is filtered out, but a stderr note explains
            //           why so the user isn't confused by an empty queue
            //   block → entry is filtered out silently (consistent with
            //           a hard "those specs aren't yours" stance)
            // trace:STORY-48 | ai:claude
            let project_root_for_leases = find_project_root().ok();
            let leases = project_root_for_leases
                .as_ref()
                .map(|p| list_leases(p))
                .unwrap_or_default();
            let self_lease = if leases.is_empty() {
                None
            } else {
                std::env::current_dir().ok().and_then(|cwd| {
                    project_root_for_leases
                        .as_ref()
                        .and_then(|root| active_lease_for_cwd(root, &cwd))
                })
            };
            let enforcement_mode = project_root_for_leases
                .as_ref()
                .map(|p| session_enforcement(p))
                .unwrap_or(SessionEnforcement::Warn);
            let lease_filter_active =
                !leases.is_empty() && enforcement_mode != SessionEnforcement::Off;
            // Unknown ownership retains the assignment except for legacy pickup compatibility.
            // trace:TASK-3420 | ai:codex
            let mut skipped_for_lease: Vec<(String, String)> = Vec::new();
            // STORY-333: track specs skipped by the pre-pickup gate so
            // `queue next` can hint why (without a hint, an un-pickable
            // head looks like "queue is empty"). trace:STORY-333 | ai:claude
            let mut skipped_unpickable: Vec<(String, String)> = Vec::new();

            // BUG-1082: once an implementer session is inside a leased worktree,
            // the lease scope is the assignment. A later queue mutation must not
            // let a headless worker re-poll the queue and drift to a different
            // spec. Pin `queue next` to the leased spec before consulting the
            // ambient queue head.
            // trace:BUG-1082 | ai:codex
            let leased_head =
                leased_implementer_queue_head(&raw_entries, &store, &user_id, self_lease.as_ref());

            let lease_reader = project_root_for_leases
                .as_ref()
                .map(|root| crate::ownership::reader::LeaseReader::new(root));
            let next_entry = leased_head.or_else(|| {
                raw_entries
                    .iter()
                    .filter(|e| {
                        entry_matches_role_filter(
                            e.for_role.as_deref(),
                            role_filter.as_deref(),
                            only_unrouted,
                        )
                    })
                    .filter(|e| {
                        // STORY-57: scope/session routing — only show items
                        // targeted at this session (or unrouted on that axis).
                        // --all bypasses; consistent with queue list.
                        entry_scope_session_match(e, self_lease.as_ref(), *all)
                    })
                    .filter(|e| {
                        // TASK-46: never surface terminal-status entries
                        // as "next" — they aren't actionable. Unlike
                        // `queue list` we don't offer an --include-terminal
                        // override here because the whole point of `next`
                        // is "what should I pick up", and the answer is
                        // never "this Completed thing". trace:TASK-46 | ai:claude
                        //
                        // NeedsAttention is gated by `pickability` below (one
                        // step down in this chain) so it surfaces as a
                        // skipped-with-reason entry rather than disappearing
                        // silently. trace:STORY-332 trace:TASK-131
                        let Some(req) =
                            store.requirements.iter().find(|r| r.id == e.requirement_id)
                        else {
                            return true;
                        };
                        !is_terminal_status(&req.status)
                    })
                    .filter(|e| {
                        // STORY-333: pre-pickup gate. A spec that is blocked-by
                        // an unsatisfied blocker, or marked human-only, is never
                        // the right "next" to pick up. Record the skip so the
                        // user sees why instead of an empty-queue silence.
                        // trace:STORY-333 | ai:claude
                        let Some(req) =
                            store.requirements.iter().find(|r| r.id == e.requirement_id)
                        else {
                            return true;
                        };
                        match queue_fresh_pickup_policy(req, &store, false, store_path.parent()) {
                            QueueFreshPickup::Pickable => true,
                            other => {
                                let display = req
                                    .agreed_id
                                    .clone()
                                    .or_else(|| req.spec_id.clone())
                                    .unwrap_or_else(|| "?".to_string());
                                let reason = queue_fresh_pickup_reason_label(&other)
                                    .unwrap_or_else(|| "not pickable".to_string());
                                skipped_unpickable.push((display, reason));
                                false
                            }
                        }
                    })
                    .filter(|e| {
                        if !lease_filter_active {
                            return true;
                        }
                        let Some(req) =
                            store.requirements.iter().find(|r| r.id == e.requirement_id)
                        else {
                            return true;
                        };
                        let owner = lease_owning_spec(
                            &leases,
                            self_lease.as_ref(),
                            req.id,
                            req.spec_id.as_deref(),
                            &store,
                            |lease| {
                                lease_reader
                                    .as_ref()
                                    .is_none_or(|reader| reader.retained(lease))
                            },
                        );
                        match owner {
                            None => true,
                            Some(o) => {
                                skipped_for_lease.push((
                                    req.spec_id.clone().unwrap_or_else(|| "?".into()),
                                    o.scope.clone(),
                                ));
                                false
                            }
                        }
                    })
                    .filter(|e| {
                        let Some((scope_tags, scope_status)) = &scope else {
                            return true;
                        };
                        let Some(req) =
                            store.requirements.iter().find(|r| r.id == e.requirement_id)
                        else {
                            return true;
                        };
                        if let Some(want) = scope_status {
                            if !format!("{}", req.status).eq_ignore_ascii_case(want)
                                && !format!("{:?}", req.status).eq_ignore_ascii_case(want)
                            {
                                return false;
                            }
                        }
                        for tag in scope_tags {
                            if !req.tags.iter().any(|t| t == tag) {
                                return false;
                            }
                        }
                        true
                    })
                    .min_by_key(|e| e.position)
                    .cloned()
            });

            // Local wins on tiebreak — the FR specifies that local-context
            // work takes precedence. Only fall through to global when local
            // is empty (or --global was passed). `--for any` (only_unrouted)
            // never falls through — global queues are per-role.
            // trace:FR-1-012 BUG-87
            let global_next: Option<global_queue::GlobalQueueEntry> =
                if *local || only_unrouted || next_entry.is_some() {
                    None
                } else if let Some(role_name) = &role_filter {
                    let entries = global_queue::load(role_name).unwrap_or_default();
                    entries.into_iter().min_by_key(|e| e.position)
                } else {
                    None
                };

            if next_entry.is_none() && global_next.is_none() {
                // STORY-63: scope fallback. If the personal+global queues
                // are both empty AND we're inside a session lease, surface
                // the EPIC's approved children — `aida session start
                // --owns EPIC-X` should make picking work feel automatic,
                // not require pre-queueing every story.
                //
                // Rules:
                //   - resolve self_lease.scope to a Requirement (path-glob
                //     scopes don't qualify and fall through to the empty
                //     message)
                //   - if any child of that scope is already InProgress,
                //     don't auto-pick a parallel one — the session is
                //     already busy, even if the user got here looking for
                //     "what's next." Better to surface the in-flight item
                //     than start a second one
                //   - candidates: status=Approved, plus the active role's
                //     scope filter (tags/status) when set, same as the
                //     existing queue path
                //   - sort: priority High → Medium → Low, then created_at
                //     oldest-first as tiebreak
                // trace:STORY-63 | ai:claude
                if let Some(self_l) = self_lease.as_ref() {
                    if let Some(picked) = scope_fallback_pick(&store, self_l, scope.as_ref()) {
                        let approved_count = picked.approved_count;
                        let pick = picked.pick;
                        // BUG-81: short id when assigned. trace:BUG-81 | ai:claude
                        let pick_display_id = pick
                            .agreed_id
                            .as_deref()
                            .or(pick.spec_id.as_deref())
                            .unwrap_or("?");
                        println!(
                            "{} {} has {} approved child(ren); picking {} {}",
                            "Queue empty —".dimmed(),
                            self_l.scope.cyan().bold(),
                            approved_count,
                            pick_display_id.green().bold(),
                            "(scope fallback)".dimmed(),
                        );
                        println!();
                        println!("  {}: {}", "Title".bold(), pick.title);
                        println!("  {}: {}", "Status".bold(), pick.status);
                        println!("  {}: {}", "Priority".bold(), pick.priority);
                        if !pick.tags.is_empty() {
                            let mut tags: Vec<&String> = pick.tags.iter().collect();
                            tags.sort();
                            let tags_str = tags
                                .iter()
                                .map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join(", ");
                            println!("  {}: {}", "Tags".bold(), tags_str);
                        }
                        println!();
                        println!("{}", "Suggested:".dimmed());
                        let id_for_cmd = pick_display_id;
                        println!(
                            "  aida show {}  &&  aida edit {} --status in-progress",
                            id_for_cmd, id_for_cmd
                        );
                        return Ok(());
                    }
                }

                let scope = if only_unrouted {
                    format!(" for {}", "unrouted items".cyan())
                } else {
                    match &role_filter {
                        Some(r) => format!(" for role {}", r.cyan()),
                        None => String::new(),
                    }
                };
                println!("{} Queue is empty{}.", "Nothing to do —".dimmed(), scope);
                // STORY-48: if the queue would have had items but they were
                // all owned by other sessions, name them — otherwise the
                // user sees an empty queue with no idea why.
                if !skipped_for_lease.is_empty() {
                    eprintln!();
                    eprintln!(
                        "{} {} item(s) skipped (owned by other sessions):",
                        "Note:".yellow().bold(),
                        skipped_for_lease.len()
                    );
                    for (spec, scope) in &skipped_for_lease {
                        eprintln!("  · {}  →  scope {}", spec.cyan(), scope.cyan());
                    }
                }
                // STORY-333: same shape for the pickability gate — name the
                // un-pickable specs and their reasons so the user sees why
                // the queue head was skipped.
                // trace:STORY-333 | ai:claude
                if !skipped_unpickable.is_empty() {
                    eprintln!();
                    eprintln!(
                        "{} {} un-pickable item(s) skipped (run `aida queue list` to see the Blocked section):",
                        "Note:".yellow().bold(),
                        skipped_unpickable.len()
                    );
                    for (spec, reason) in &skipped_unpickable {
                        eprintln!("  · {}  —  {}", spec.cyan(), reason.dimmed());
                    }
                }
                // STORY-63: nudge toward `aida list --status approved` when
                // even the scope fallback came up empty, so the user has a
                // concrete next step rather than a dead-end.
                if self_lease.is_some() {
                    println!(
                        "  ({})",
                        "scope has no approved+ready children either — try `aida list --status approved`".dimmed()
                    );
                } else {
                    // BUG-684: the old hint pointed at `aida role enter dialog` —
                    // `dialog` is the DEPRECATED alias for `advisor`, AND the
                    // advisor seat is the wrong next step for a default
                    // implementer with an empty queue. Mirror the sibling hint
                    // above: fill the queue from the approved backlog, or drive a
                    // scope directly. trace:BUG-684 | ai:claude
                    println!(
                        "  ({})",
                        "no items queued — try `aida list --status approved` or `aida queue work <scope>`".dimmed()
                    );
                }
                return Ok(());
            }
            // STORY-48: surface non-fatal skips before rendering the next
            // item — useful when the active queue contains a mix of
            // in-scope and out-of-scope items.
            if !skipped_for_lease.is_empty() {
                eprintln!(
                    "{} {} other-session item(s) skipped (run `aida session leases` to see who)",
                    "Note:".dimmed(),
                    skipped_for_lease.len()
                );
            }
            // STORY-333: un-pickable skips surfaced before the next-item
            // render so the user sees what was bypassed.
            // trace:STORY-333 | ai:claude
            if !skipped_unpickable.is_empty() {
                eprintln!(
                    "{} {} un-pickable item(s) skipped: {}",
                    "Note:".dimmed(),
                    skipped_unpickable.len(),
                    skipped_unpickable
                        .iter()
                        .map(|(s, r)| format!("{} ({})", s, r))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }

            // If only the global has an item, render it and return.
            if let Some(entry) = global_next {
                // BUG-83: prefer cached agreed_id; falls back to spec_id.
                // The `aida show` suggestion uses the same display id since
                // the resolver accepts both forms. trace:BUG-83 | ai:claude
                let display_id = entry
                    .agreed_id
                    .as_deref()
                    .or(entry.spec_id.as_deref())
                    .unwrap_or("???");
                let title = entry.title.as_deref().unwrap_or("(no cached title)");
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
                println!("{}", "Next up:".bold());
                println!(
                    "  {}: {}",
                    display_id_owned.green().bold(),
                    title_owned.bold()
                );
                println!(
                    "  {} {}",
                    format!("[role:{}]", entry.for_role).cyan(),
                    format!("[origin:{}]", entry.project_name).dimmed()
                );
                if let Some(ref note) = entry.note {
                    println!("  Note: {}", note.italic());
                }
                println!();
                println!("{}", "Suggested:".dimmed());
                // `aida show` still resolves via the canonical id; show
                // both forms so the user knows their hand-off targets.
                println!(
                    "  cd {}  &&  aida show {}",
                    entry.project_root.display().to_string().cyan(),
                    display_id
                );
                return Ok(());
            }

            match next_entry {
                None => {
                    return Ok(());
                }
                Some(entry) => {
                    let req = store
                        .requirements
                        .iter()
                        .find(|r| r.id == entry.requirement_id);
                    // BUG-81: prefer short id; mirrors `aida list` / queue
                    // list rendering. trace:BUG-81 | ai:claude
                    let spec_id = req
                        .and_then(|r| r.agreed_id.as_deref().or(r.spec_id.as_deref()))
                        .unwrap_or("???");
                    let title = req.map(|r| r.title.as_str()).unwrap_or("(deleted)");
                    // TASK-91: PR-N (STORY-NNN) for auto-queued review
                    // stories. trace:TASK-91 | ai:claude
                    // trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
                    let (display_id_owned, title_owned) =
                        format_review_story_display(spec_id, title).unwrap_or_else(|| {
                            (
                                aida_core::node_alias::display_spec_id(spec_id),
                                title.to_string(),
                            )
                        });
                    let status = req
                        .map(|r| format!("{}", r.status))
                        .unwrap_or_else(|| "Unknown".to_string());
                    let priority = req
                        .map(|r| format!("{}", r.priority))
                        .unwrap_or_else(|| "?".to_string());
                    let owner = req.map(|r| r.owner.as_str()).unwrap_or("");
                    let description = req.map(|r| r.description.as_str()).unwrap_or("");

                    println!("{}", "Next up:".bold());
                    println!(
                        "  {}: {}",
                        display_id_owned.green().bold(),
                        title_owned.bold()
                    );
                    println!(
                        "  Status: {}  ·  Priority: {}{}",
                        status,
                        priority,
                        if owner.is_empty() {
                            String::new()
                        } else {
                            format!("  ·  Owner: {}", owner)
                        }
                    );
                    if let Some(ref r) = entry.for_role {
                        println!("  Routed for: {}", r.cyan());
                    }
                    if let Some(ref note) = entry.note {
                        println!("  Note: {}", note.italic());
                    }
                    // BUG-775: the last review's verdict, front and centre.
                    // An implementer picking this spec up must not have to go
                    // hunting through comments to learn a reviewer already
                    // rejected the branch — that is how a rejected commit got
                    // merged and marked done. trace:BUG-775 | ai:claude
                    if let Ok(root) = find_project_root() {
                        // BUG-2403: an unselectable review says so instead
                        // of reading as no verdict. trace:BUG-2403 | ai:claude
                        let selection = crate::gate::review_alias::select_verdict(
                            &root,
                            &[&display_id_owned, spec_id],
                        );
                        if let Some(reason) = selection.unavailable_reason() {
                            println!(
                                "  {} {}",
                                "Review verdict:".bold(),
                                format!("cannot be selected ({reason})").yellow().bold()
                            );
                        }
                        if let Some(v) = selection.fail_closed() {
                            let line = crate::review_verdict_display::verdict_notice_line(&v);
                            if v.kind.requires_changes() {
                                println!("  {} {}", "Review verdict:".bold(), line.yellow().bold());
                                println!(
                                    "  {} address it and commit — `aida queue done` will refuse \
                                     while the branch sits at the reviewed commit.",
                                    crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed()
                                );
                            } else {
                                println!("  {} {}", "Review verdict:".bold(), line);
                            }
                        }
                    }
                    if !description.is_empty() {
                        println!();
                        println!("{}", "Description (first 10 lines):".dimmed());
                        for line in description.lines().take(10) {
                            println!("  {}", line);
                        }
                        if description.lines().count() > 10 {
                            println!("  {}", "…".dimmed());
                        }
                    }
                    println!();
                    println!("{}", "Suggested commands:".dimmed());
                    println!("  {} {}    full details", "aida show".cyan(), spec_id);
                    println!(
                        "  {} {} --status in-progress     mark in-progress",
                        "aida edit".cyan(),
                        spec_id
                    );
                    println!(
                        "  {} {}    when finished (marks complete + dequeues)",
                        "aida queue done".cyan(),
                        spec_id
                    );
                    // TASK-304: surface the ultraplan suggestion for a chunky
                    // head spec under `[ultraplan] mode = "suggested"`. This
                    // is the surface `/aida-pickup` renders via `aida queue
                    // next`. trace:TASK-304 | ai:claude
                    if let Some(r) = req {
                        if let Ok(root) = find_project_root() {
                            print_ultraplan_suggestion_hint(&root, r);
                        }
                    }
                }
            }
        }
        _ => unreachable!("dispatcher selected another queue subcommand"),
    }
    Ok(())
}
