//! `aida queue` command cluster — `handle_queue_command` and the
//! advance/progress/rework/work/recover/integrate handlers plus their
//! queue-local helpers, extracted from `lib.rs` (SPIKE-78 / STORY-771;
//! pure movement, no behavior change). Shared queue/store helpers stay in
//! `lib.rs`, reached via `crate::`.
// trace:STORY-771 | ai:claude

use crate::*;

/// STORY-566: `aida queue advance` — a ROUTER over the queue. Walks each queued
/// spec in order, classifies it via `burndown::explain_open`, and dispatches to
/// the EXISTING flow for that bucket (review / `queue work [--zen]` / decision /
/// approve / reject) so the operator processes the queue one item at a time to a
/// real resolution. It reimplements none of those flows — it shells out to the
/// same binary (or, for review/mutations, calls in-process) and continues the
/// walk regardless of any sub-step's outcome. `--yes` auto-takes ONLY the
/// unambiguous autonomous step (drain a ready spec, approve a groomed draft) and
/// skips everything that needs a human. trace:STORY-566 | ai:claude
pub(crate) fn handle_queue_advance(
    storage: &Storage,
    id: Option<&str>,
    yes: bool,
    user: Option<&str>,
) -> Result<()> {
    use std::io::IsTerminal;

    let user_id = current_user_id(user);
    let store_path = storage.path().to_path_buf();
    let interactive = !yes && std::io::stdin().is_terminal() && std::io::stdout().is_terminal();

    let entries = storage.queue_list(&user_id, /* include_completed */ false)?;
    let store = storage.load()?;

    // Build the open-facts index once (every open spec → its classification
    // facts), keyed by display id (UPPERCASE for case-insensitive lookup).
    let project_root = find_project_root().ok();
    let in_flight = project_root
        .as_ref()
        .map(|r| in_flight_lease_role_map(r))
        .unwrap_or_default();
    let facts_by_id: std::collections::HashMap<String, burndown::OpenFacts> =
        collect_open_facts(&store, &in_flight)
            .into_iter()
            .map(|f| (f.id.to_ascii_uppercase(), f))
            .collect();

    // Resolve the walk set: a single id, or the whole queue in order. A
    // single-id request that isn't queued still advances (resolve it from the
    // store) so `queue advance <id>` works on any open spec.
    struct Item {
        display: String,
        title: String,
    }
    let resolve_display = |req: &aida_core::Requirement| -> String {
        req.agreed_id
            .clone()
            .or_else(|| req.spec_id.clone())
            .unwrap_or_else(|| req.id.to_string())
    };
    let mut items: Vec<Item> = Vec::new();
    if let Some(target) = id {
        let req = if let Ok(uuid) = uuid::Uuid::parse_str(target) {
            store.requirements.iter().find(|r| r.id == uuid)
        } else {
            store.get_requirement_by_spec_id(target)
        }
        .ok_or_else(|| not_found::requirement_not_found(target, Some(storage.path())))?;
        items.push(Item {
            display: resolve_display(req),
            title: req.title.clone(),
        });
    } else {
        for e in &entries {
            if let Some(req) = store.requirements.iter().find(|r| r.id == e.requirement_id) {
                items.push(Item {
                    display: resolve_display(req),
                    title: req.title.clone(),
                });
            }
        }
    }

    if items.is_empty() {
        println!("Queue empty — nothing to advance.");
        return Ok(());
    }

    for item in &items {
        // Classify the item from its open facts. A terminal spec has no facts;
        // report and skip it.
        let Some(facts) = facts_by_id.get(&item.display.to_ascii_uppercase()) else {
            println!(
                "\n{} {} — {}\n  {} already terminal (completed/rejected) — nothing to advance.",
                "›".dimmed(),
                item.display.bold(),
                item.title,
                crate::glyph(crate::glyphs::Glyph::Check).green(),
            );
            continue;
        };
        let (bucket, reason) = burndown::explain_open(facts);
        // A Planned spec buckets as Actionable — its next step is to DRAIN it
        // (`queue work <id>` picks up a Planned spec directly; it doesn't apply
        // the burndown's default `--status approved` filter). No backward
        // Planned→Approved flip. trace:STORY-566 | ai:claude
        let action = burndown::advance_action(bucket);

        println!(
            "\n{} {} — {}",
            "›".cyan().bold(),
            item.display.bold(),
            item.title
        );
        println!("  {} {}  ({})", "bucket".dimmed(), bucket.key(), reason);

        // --yes / non-interactive: take ONLY the unambiguous autonomous step;
        // skip anything needing a human, printing what each needs.
        if !interactive {
            if action.is_autonomous() {
                advance_dispatch(action, &item.display, &store_path)?;
            } else {
                println!(
                    "  {} needs a human — skipped. {}",
                    crate::glyph(crate::glyphs::Glyph::SubArrow).yellow(),
                    burndown::advance_action_label(bucket)
                );
            }
            continue;
        }

        // Interactive: offer the bucket-appropriate next action + Skip + Quit.
        let primary = burndown::advance_action_label(bucket);
        let mut options: Vec<&str> = Vec::new();
        if action != burndown::AdvanceAction::None {
            options.push(primary);
        }
        // A draft/planned spec can ALSO be approved-then-drained; the primary
        // already is Approve for those, so no extra option needed.
        options.push("Skip");
        options.push("Quit");

        let choice = match inquire::Select::new(
            "Advance this item:",
            options.iter().map(|s| s.to_string()).collect(),
        )
        .prompt()
        {
            Ok(c) => c,
            // Esc / Ctrl-C / cancel → treat as Quit (stop the walk).
            Err(_) => {
                println!(
                    "  {} stopping the walk.",
                    crate::glyph(crate::glyphs::Glyph::Check).green()
                );
                break;
            }
        };

        if choice == "Quit" {
            println!(
                "  {} stopping the walk.",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
            break;
        }
        if choice == "Skip" {
            println!(
                "  {} skipped.",
                crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed()
            );
            continue;
        }
        // The only remaining option is the bucket's primary action.
        advance_dispatch(action, &item.display, &store_path)?;
    }

    Ok(())
}

/// STORY-566: in-process construction of the cached git backend for the
/// advance router's review + status mutations. Mirrors the
/// `file_reviewer_verdict_unavailable_finding` pattern. trace:STORY-566
pub(crate) fn advance_backend(store_path: &std::path::Path) -> Result<aida_core::CachedGitBackend> {
    let dispenser = load_dispenser(store_path)?;
    let inner = aida_core::GitBackend::new(store_path)?.with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(store_path);
    aida_core::CachedGitBackend::with_inner(inner, &cache_path)
}

/// STORY-566: dispatch ONE advance action to the existing flow. Review and the
/// status mutations (approve / reject / drop the `review:draft-only` tag) run
/// in-process via the cached backend; the build/drain verbs shell out to the
/// same binary so they stay interactive. A failed/abandoned sub-step just leaves
/// the item unprocessed — the caller continues the walk. trace:STORY-566
pub(crate) fn advance_dispatch(
    action: burndown::AdvanceAction,
    display: &str,
    store_path: &std::path::Path,
) -> Result<()> {
    use burndown::AdvanceAction;

    let aida = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("aida"));
    let project_root = store_path.parent().map(|p| p.to_path_buf());

    match action {
        AdvanceAction::Review => {
            // In-process review (handles the closed-PR reopen offer itself,
            // AC-4/AC-5). On the operator's confirmation that it was approved,
            // offer to drop `review:draft-only` so it drains.
            let backend = advance_backend(store_path)?;
            if let Err(e) = handle_review_spec(
                &backend, store_path, display, /* no_agent */ false,
                /* allow_stale_base */ false,
            ) {
                eprintln!(
                    "  {} review of {} did not complete: {}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    display,
                    e
                );
                return Ok(());
            }
            if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                let approved = inquire::Confirm::new(&format!(
                    "Was {display} approved? (drop `review:draft-only` so it drains)"
                ))
                .with_default(false)
                .prompt()
                .unwrap_or(false);
                if approved {
                    let mut req = backend
                        .get_requirement_by_spec_id(display)?
                        .ok_or_else(|| anyhow::anyhow!("could not reload {display}"))?;
                    let before = req.tags.len();
                    req.tags
                        .retain(|t| !t.trim().eq_ignore_ascii_case("review:draft-only"));
                    if req.tags.len() != before {
                        req.record_change(
                            current_user_id(None),
                            vec![aida_core::Requirement::field_change(
                                "tags",
                                "review:draft-only".to_string(),
                                "(removed)".to_string(),
                            )],
                        );
                        req.modified_at = chrono::Utc::now();
                        backend.update_requirement(&req)?;
                        println!(
                            "  {} dropped `review:draft-only` from {} — it can now drain.",
                            crate::glyph(crate::glyphs::Glyph::Check).green(),
                            display.bold()
                        );
                    } else {
                        println!(
                            "  {} {} had no `review:draft-only` tag.",
                            crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                            display
                        );
                    }
                }
            }
        }
        AdvanceAction::SupervisedBuild => {
            println!(
                "  {} building at the keyboard (queue work --zen)…",
                "→".green()
            );
            let mut cmd = std::process::Command::new(&aida);
            if let Some(root) = &project_root {
                cmd.current_dir(root);
            }
            let status = cmd.args(["queue", "work", display, "--zen"]).status();
            advance_report_status(status, display);
        }
        AdvanceAction::Decision => {
            println!(
                "  {} this item needs a human decision. Answer it with: {}",
                crate::glyph(crate::glyphs::Glyph::SubArrow).yellow(),
                "aida questions".cyan()
            );
        }
        AdvanceAction::Drain => {
            println!("  {} draining now (queue work)…", "→".green());
            let mut cmd = std::process::Command::new(&aida);
            if let Some(root) = &project_root {
                cmd.current_dir(root);
            }
            let status = cmd.args(["queue", "work", display]).status();
            advance_report_status(status, display);
            println!(
                "  {} tip: {} drains every ready item at once.",
                crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                "aida burndown run".cyan()
            );
        }
        AdvanceAction::Approve => {
            let backend = advance_backend(store_path)?;
            let mut req = backend
                .get_requirement_by_spec_id(display)?
                .ok_or_else(|| not_found::requirement_not_found(display, Some(store_path)))?;
            // Same advisor-authority gate as `aida edit` / `aida queue add`:
            // approving is an advisor call. Closes the `--yes` side-door where a
            // non-advisor / non-TTY run could auto-approve drafts. trace:STORY-566
            let new_status = RequirementStatus::Approved;
            if status_advance_requires_advisor_authority(&req.status, &new_status)
                && !has_advisor_authority()
            {
                println!(
                    "  {} approving {} needs the advisor role (or an interactive terminal). \
                     Re-run as advisor: `AIDA_SESSION_ROLE=advisor aida queue advance {}`.",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    display.bold(),
                    display
                );
                return Ok(());
            }
            let old = req.status.to_string();
            req.set_status_from_str("Approved");
            req.record_change(
                current_user_id(None),
                vec![aida_core::Requirement::field_change(
                    "status",
                    old,
                    "Approved".to_string(),
                )],
            );
            req.modified_at = chrono::Utc::now();
            backend.update_requirement(&req)?;
            println!(
                "  {} approved {} — it's now drainable.",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                display.bold()
            );
        }
        AdvanceAction::Reject => {
            // Offer Reject (resolve it out) or leave it parked.
            let do_reject = if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                inquire::Confirm::new(&format!("Reject {display} (resolve it out)?"))
                    .with_default(false)
                    .prompt()
                    .unwrap_or(false)
            } else {
                false
            };
            if do_reject {
                let backend = advance_backend(store_path)?;
                let mut req = backend
                    .get_requirement_by_spec_id(display)?
                    .ok_or_else(|| not_found::requirement_not_found(display, Some(store_path)))?;
                let new_status = RequirementStatus::Rejected;
                if status_advance_requires_advisor_authority(&req.status, &new_status)
                    && !has_advisor_authority()
                {
                    println!(
                        "  {} rejecting {} needs the advisor role. \
                         Re-run as advisor: `AIDA_SESSION_ROLE=advisor`.",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                        display.bold()
                    );
                    return Ok(());
                }
                let old = req.status.to_string();
                req.set_status_from_str("Rejected");
                req.record_change(
                    current_user_id(None),
                    vec![aida_core::Requirement::field_change(
                        "status",
                        old,
                        "Rejected".to_string(),
                    )],
                );
                req.modified_at = chrono::Utc::now();
                backend.update_requirement(&req)?;
                println!(
                    "  {} rejected {}.",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    display.bold()
                );
            } else {
                println!(
                    "  {} left parked.",
                    crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed()
                );
            }
        }
        AdvanceAction::Close => {
            // BUG-543: a fully-delivered epic (all children Completed). Offer to
            // close it (status → Completed) — operator-confirmed, never silent.
            let do_close = if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                inquire::Confirm::new(&format!(
                    "Close {display}? (all children completed → status completed)"
                ))
                .with_default(true)
                .prompt()
                .unwrap_or(false)
            } else {
                false
            };
            if !do_close {
                println!(
                    "  {} left open.",
                    crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed()
                );
                return Ok(());
            }
            let backend = advance_backend(store_path)?;
            let mut req = backend
                .get_requirement_by_spec_id(display)?
                .ok_or_else(|| not_found::requirement_not_found(display, Some(store_path)))?;
            // Completing is an advisor-authority act (same gate as approve/
            // reject); an interactive operator clears it via the TTY branch of
            // `has_advisor_authority`. trace:BUG-543
            let new_status = RequirementStatus::Completed;
            if status_advance_requires_advisor_authority(&req.status, &new_status)
                && !has_advisor_authority()
            {
                println!(
                    "  {} closing {} needs the advisor role (or an interactive terminal). \
                     Re-run as advisor: `AIDA_SESSION_ROLE=advisor`.",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    display.bold()
                );
                return Ok(());
            }
            let old = req.status.to_string();
            req.set_status_from_str("Completed");
            req.record_change(
                current_user_id(None),
                vec![aida_core::Requirement::field_change(
                    "status",
                    old,
                    "Completed".to_string(),
                )],
            );
            req.modified_at = chrono::Utc::now();
            backend.update_requirement(&req)?;
            println!(
                "  {} closed {} — all children were completed.",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                display.bold()
            );
        }
        AdvanceAction::None => {
            println!(
                "  {} nothing to do — it resolves through normal flow.",
                crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed()
            );
        }
    }
    Ok(())
}

/// STORY-566: report the exit status of a shelled-out advance sub-step without
/// failing the walk. trace:STORY-566 | ai:claude
pub(crate) fn advance_report_status(
    status: std::io::Result<std::process::ExitStatus>,
    display: &str,
) {
    match status {
        Ok(s) if s.success() => {}
        Ok(_) => println!(
            "  {} {} did not complete — it stays in the queue.",
            crate::glyph(crate::glyphs::Glyph::SubArrow).yellow(),
            display
        ),
        Err(e) => eprintln!(
            "  {} could not launch the sub-step for {}: {}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            display,
            e
        ),
    }
}

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
        s.agreed_id
            .as_deref()
            .or(s.spec_id.as_deref())
            .unwrap_or("?")
            .to_string()
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
    let summaries = match advance_backend(store_path)
        .and_then(|b| b.list_summaries(&aida_core::ListFilter::default()))
    {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let dead: Vec<uuid::Uuid> = dead_queue_entries(&entries, &summaries, None)
        .iter()
        .map(|e| e.requirement_id)
        .collect();
    if dead.is_empty() {
        return 0;
    }
    storage
        .queue_remove_many(user_id, &dead)
        .map(|removed| removed.len())
        .unwrap_or(0)
}

/// `store_path` is the orphan-store path; the `--json` fast path opens a
/// cache-backed backend from it to resolve titles via the SQLite cache rather
/// than the legacy full YAML load.
// trace:BUG-618 | ai:claude
pub(crate) fn handle_queue_command(
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
            // a slow `storage.load()` full YAML scan. trace:STORY-672
            if *all_users {
                let backend = advance_backend(store_path)?;
                let summaries = backend.list_summaries(&aida_core::ListFilter::default())?;
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
            // the same path `aida list` uses, ~0.2s) instead of `storage.load()`
            // — the legacy full YAML/git scan that cost ~1s on the cockpit
            // paint. `RequirementSummary` carries id/spec_id/agreed_id/title/
            // status, everything the JSON shape needs. trace:BUG-618 | ai:claude
            if *json {
                let raw = if *global {
                    Vec::new()
                } else {
                    storage.queue_list(&user_id, *include_completed)?
                };
                let backend = advance_backend(store_path)?;
                let summaries = backend.list_summaries(&aida_core::ListFilter::default())?;
                let rows = queue_json_rows(&raw, &summaries);
                println!("{}", serde_json::to_string(&rows)?);
                return Ok(());
            }
            // TASK-964: AGENT-MODE token-efficient TOON render of the queue.
            // Mirrors the BUG-616 `--json` shape (raw queue order, cache-resolved
            // id/title/status/for_role) as a uniform TOON table with a count
            // header, instead of the human grouped/Done-awaiting-merge view. The
            // human TTY path drops through unchanged. trace:TASK-964
            if agent_output_mode() {
                let raw = if *global {
                    Vec::new()
                } else {
                    storage.queue_list(&user_id, *include_completed)?
                };
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
                let rows: Vec<Vec<String>> = raw
                    .iter()
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
                        let id = s
                            .agreed_id
                            .as_deref()
                            .or(s.spec_id.as_deref())
                            .unwrap_or("")
                            .to_string();
                        Some(vec![
                            id,
                            s.title.clone(),
                            toon_status_token(&s.status),
                            e.for_role.clone().unwrap_or_default(),
                        ])
                    })
                    .collect();
                println!("count: {}", rows.len());
                println!(
                    "{}",
                    crate::toon::table_raw("queue", &["id", "title", "status", "for_role"], &rows)
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
                storage.queue_list(&user_id, *include_completed)?
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

            // TASK-52: parse --scope <CSV>. `none` (case-insensitive) is
            // a sentinel meaning "entries with no for_scope set". Any
            // other token (or comma-separated list) matches against
            // explicit `for_scope` AND the auto-derived parent EPIC
            // label from TASK-44 (so the displayed chip and the filter
            // agree). trace:TASK-52 | ai:claude
            #[derive(Debug, Clone)]
            enum ScopeFilterKind {
                Match(Vec<String>),
                NoScope,
            }
            let scope_filter_parsed: Option<ScopeFilterKind> = scope_filter.as_deref().map(|raw| {
                let parts: Vec<String> = raw
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if parts.iter().any(|p| p.eq_ignore_ascii_case("none")) {
                    ScopeFilterKind::NoScope
                } else {
                    ScopeFilterKind::Match(parts)
                }
            });
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
                    // TASK-52: --scope filter. Matches explicit
                    // for_scope first, then the auto-derived parent
                    // EPIC label so the displayed chip and the filter
                    // agree. trace:TASK-52 | ai:claude
                    let Some(ref kind) = scope_filter_parsed else {
                        return true;
                    };
                    match kind {
                        ScopeFilterKind::NoScope => e.for_scope.is_none(),
                        ScopeFilterKind::Match(wants) => {
                            if let Some(ref fs) = e.for_scope {
                                if wants.iter().any(|w| w.eq_ignore_ascii_case(fs)) {
                                    return true;
                                }
                            }
                            // Fall through to derived parent-EPIC label.
                            let Some(req) =
                                store.requirements.iter().find(|r| r.id == e.requirement_id)
                            else {
                                return false;
                            };
                            if let Some(derived) = derive_parent_epic_label(req, &store) {
                                wants.iter().any(|w| w.eq_ignore_ascii_case(&derived))
                            } else {
                                false
                            }
                        }
                    }
                })
                .filter(|e| {
                    let Some((scope_tags, scope_status)) = &scope else {
                        return true;
                    };
                    let Some(req) = store.requirements.iter().find(|r| r.id == e.requirement_id)
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
                .filter(|e| {
                    // TASK-229: --batch NAME filter. Match entries whose
                    // requirement carries the `batch:NAME` tag.
                    // Case-insensitive match. trace:TASK-229 | ai:claude
                    let Some(name) = batch_filter.as_deref() else {
                        return true;
                    };
                    let want = format!("batch:{}", name);
                    let Some(req) = store.requirements.iter().find(|r| r.id == e.requirement_id)
                    else {
                        return false;
                    };
                    req.tags.iter().any(|t| t.eq_ignore_ascii_case(&want))
                })
                .filter(|e| {
                    // TASK-238: --tag exact-match filter (case-insensitive).
                    let Some(want) = tag_filter.as_deref() else {
                        return true;
                    };
                    store
                        .requirements
                        .iter()
                        .find(|r| r.id == e.requirement_id)
                        .map(|req| tag_matches_exact(&req.tags, want))
                        .unwrap_or(false)
                })
                .filter(|e| {
                    // TASK-238: --tag-prefix filter (case-insensitive).
                    let Some(prefix) = tag_prefix_filter.as_deref() else {
                        return true;
                    };
                    store
                        .requirements
                        .iter()
                        .find(|r| r.id == e.requirement_id)
                        .map(|req| tag_matches_prefix(&req.tags, prefix))
                        .unwrap_or(false)
                })
                .collect();

            // STORY-706: row count before the focus/epic narrowing, for the
            // focus header's "showing N of M".
            let total_pre_focus = entries.len();

            // TASK-923: --epic / --parent — narrow to the epic's TRANSITIVE
            // descendant tree (epic + children + grandchildren). Resolve the
            // id to its UUID, compute the closure once via the shared
            // hierarchy walk (`epic_descendant_uuid_set`, the same one
            // `aida graph --tree` uses), then run the pure
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
                    match aida_core::pickability::pickability(req, &store) {
                        aida_core::pickability::Pickability::Pickable => true,
                        aida_core::pickability::Pickability::Blocked(reason) => {
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
                            let (display_id_owned, title_owned) =
                                format_review_story_display(display_id, title)
                                    .unwrap_or_else(|| (display_id.to_string(), title.to_string()));
                            let status = req
                                .map(|r| format!("{}", r.status))
                                .unwrap_or_else(|| "Unknown".to_string());
                            // TASK-269: shared glyph + colour palette.
                            // trace:TASK-269 | ai:claude
                            let status_badge = status_display::status_badge(&status);
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
                            println!(
                                "  {} {}{}  {}  [{}]{}{}",
                                glyph.dimmed(),
                                display_id_owned.bold(),
                                pad,
                                title_owned,
                                status_badge,
                                archived_chip,
                                tag_chip,
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
                                    None => raw_id.len(),
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
                            println!(
                                "  {} {}  {}  {}",
                                glyph.dimmed(),
                                display_id.bold(),
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
                    let (display_id_owned, title_owned) =
                        format_review_story_display(display_id, title)
                            .unwrap_or_else(|| (display_id.to_string(), title.to_string()));
                    let status = req
                        .map(|r| format!("{}", r.status))
                        .unwrap_or_else(|| "Unknown".to_string());
                    // TASK-269: shared glyph + colour palette.
                    // trace:TASK-269 | ai:claude
                    let status_badge = status_display::status_badge(&status);

                    print!(
                        "  {}. {} {}",
                        (i + 1).to_string().dimmed(),
                        display_id_owned.bold(),
                        title_owned
                    );
                    print!("  [{}]", status_badge);
                    // BUG-492: an archived spec that is still queued is
                    // contradictory state (`aida list` hides it, this view
                    // keeps showing it). Flag it loudly so the user can
                    // reconcile (unarchive or dequeue). trace:BUG-492
                    if req.map(|r| r.archived).unwrap_or(false) {
                        print!("  {}", "[ARCHIVED]".red().bold());
                    }
                    if entry.added_by != user_id {
                        print!("  {}", format!("(from @{})", entry.added_by).dimmed());
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
            // at the queue-work/queue-done gates). Subtle by default; only live
            // (non-stale) leases on a non-default branch are probed, and each
            // row's `git rev-list --count` reads local refs — NO fetch here — so
            // it's cheap + best-effort (a git miss just omits that row).
            // trace:TASK-101 | ai:claude
            if !*in_flight_only {
                if let Some(root) = find_main_worktree_root()
                    .ok()
                    .or_else(|| find_project_root().ok())
                {
                    let now = chrono::Utc::now();
                    let live = process_probe::probe_live_claude_sessions();
                    let mut stale_base_rows: Vec<(String, String, String)> = Vec::new();
                    for l in list_leases(&root) {
                        if matches!(lease_state_for(&l, &live, now), LeaseState::Stale) {
                            continue;
                        }
                        if l.branch.is_empty() || l.branch == "main" {
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

                    let items: Vec<burndown::QueuedItem> = queued_ids
                        .into_iter()
                        .filter_map(|display| {
                            let facts = facts_by_id.get(&display.to_ascii_uppercase())?;
                            let (bucket, _reason) = burndown::explain_open(facts);
                            Some(burndown::QueuedItem {
                                id: display,
                                bucket,
                            })
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
        QueueCommand::Add {
            id,
            top,
            bottom: _,
            user,
            note,
            r#for,
            scope,
            for_session,
            no_scope,
            global,
            force,
        } => {
            // TASK-647 (ADR-3): queue-for-work is an advisor-authority act —
            // it commits a spec to the execution pipeline. A non-advisor,
            // non-TTY caller (headless agent, drain capture) is refused; they
            // file drafts and let the advisor triage + queue. Internal Rust
            // callers (orchestrator) use `storage.queue_add()` directly and
            // bypass this CLI gate. trace:TASK-647 | ai:claude
            //
            // BUG-631: scope the gate to DISPATCH-for-execution targets only.
            // Routing a draft `--for advisor` (or `--for human`/`--for reviewer`)
            // is a REQUEST for review/triage — open to any role, since it does
            // not dispatch execution and the advisor/reviewer still decides.
            // Only execution-dispatch routes (implementer, unknown/custom roles,
            // and the unrouted `--for any`/no-`--for` cases) stay gated.
            // trace:BUG-631 | ai:claude
            if for_target_requires_dispatch_authority(r#for.as_deref()) && !has_advisor_authority()
            {
                anyhow::bail!(
                    "queuing work for execution needs advisor authority (advisor role or an \
                     interactive session). File the spec for advisor triage instead, or run \
                     as the advisor. (Routing for review — `--for advisor`/`--for human`/\
                     `--for reviewer` — needs no advisor authority.)"
                );
            }
            // BUG-498: queuing work is advisor-style — nudge the operator to
            // seat the advisor role if they're acting via an env prefix.
            maybe_hint_advisor_seat();
            let user_id = get_user(user);

            // TASK-1150: distinct-user identity guard. Adding to a queue keyed
            // by a genuinely-different user id than this shell's identity (e.g.
            // `--user user-b` from a `user-a` shell — not just a case variant)
            // silently crosses identities. Surface it (warn by default, refuse
            // when the operator opts in). No-op on the common same-identity add.
            // trace:TASK-1150 | ai:claude
            identity_guard::enforce(&current_user_id(None), &user_id, "queue add")?;

            // BUG-634: avoid a full-store scan (`storage.load()` parses every
            // YAML) on the queue write path. For the distributed (directory)
            // store, open a cache-backed backend for targeted single-spec
            // lookups; the deprecated centralized (.db) path keeps the full
            // load. trace:BUG-634 | ai:claude
            let targeted_backend: Option<aida_core::CachedGitBackend> = if store_path.is_dir() {
                let cache_path = aida_core::CachedGitBackend::default_cache_path(store_path);
                aida_core::CachedGitBackend::open(store_path, &cache_path).ok()
            } else {
                None
            };

            // Default routing: when no --for is given but the active session
            // has a role (AIDA_SESSION_ROLE), route to that role automatically.
            // Without this, `queue add X` produced an unrouted entry that
            // `queue next` (filtered to active role by default) wouldn't show
            // — surprising "queue is empty" right after queueing something.
            // Pass `--for any` to keep the unrouted behavior explicitly.
            // trace:BUG-18 | ai:claude
            let r#for: Option<String> = match r#for.as_deref() {
                Some("any") => None,
                // TASK-586 / TASK-747: canonicalize the route target on add so
                // `dialog`→`advisor` and `Human`/`HUMAN`→`human` route
                // consistently and surface together downstream.
                Some(role) => Some(canonical_role_name(role)),
                None => std::env::var("AIDA_SESSION_ROLE")
                    .ok()
                    .filter(|s| !s.is_empty()),
            };

            // STORY-57: default scope routing. When adding inside a session
            // worktree without --scope or --no-scope, fill `for_scope` with
            // the active lease's scope so concurrent sessions sharing a
            // role don't see each other's work. --for-session is more
            // specific than --scope and overrides it for filtering, but we
            // keep both fields in the entry — the consumer side ANDs them.
            // trace:STORY-57 | ai:claude
            let active_lease_for_routing: Option<SessionLease> =
                std::env::current_dir().ok().and_then(|cwd| {
                    let project_root = storage
                        .path()
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| std::path::PathBuf::from("."));
                    active_lease_for_cwd(&project_root, &cwd)
                });
            let for_scope_routing: Option<String> = queue_add_for_scope_routing(
                *no_scope,
                scope.as_deref(),
                for_session.as_deref(),
                active_lease_for_routing.as_ref(),
            );
            let for_session_routing: Option<String> = for_session.clone();

            // Resolve requirement ID + the blocker-warning graph subset. The
            // distributed path resolves both via targeted lookups; the
            // centralized path falls back to a single full load. trace:BUG-634
            let (req_owned, warn_store): (aida_core::Requirement, aida_core::RequirementsStore) =
                if let Some(backend) = targeted_backend.as_ref() {
                    let resolved = if let Ok(uuid) = uuid::Uuid::parse_str(id) {
                        backend.get_requirement(&uuid)?
                    } else {
                        backend.get_requirement_by_spec_id(id)?
                    }
                    .ok_or_else(|| not_found::requirement_not_found(id, Some(storage.path())))?;
                    let warn_store = build_queue_warn_subset(backend, &resolved, storage, &user_id);
                    (resolved, warn_store)
                } else {
                    let store = storage.load()?;
                    let resolved = if let Ok(uuid) = uuid::Uuid::parse_str(id) {
                        store.requirements.iter().find(|r| r.id == uuid).cloned()
                    } else {
                        store.get_requirement_by_spec_id(id).cloned()
                    }
                    .ok_or_else(|| not_found::requirement_not_found(id, Some(storage.path())))?;
                    (resolved, store)
                };
            let req = &req_owned;

            // TASK-45: refuse to queue a Completed/Rejected req unless
            // --force. The intermediate state ("Completed item in
            // queue") is harmless but confuses agents and races with
            // bulk reclassification. The error message points at the
            // escape hatch. trace:TASK-45 | ai:claude
            if is_terminal_status(&req.status) && !*force {
                // BUG-81: surface the short id when one's been assigned.
                // trace:BUG-81 | ai:claude
                let display_id = req
                    .agreed_id
                    .as_deref()
                    .or(req.spec_id.as_deref())
                    .unwrap_or("?");
                // BUG-671: override flag on the FIRST line so agent mode (which
                // shows only the first line as the error summary) surfaces it.
                // trace:BUG-671 | ai:claude
                anyhow::bail!(
                    "{} is {} — re-queueing closed work is usually a mistake; pass --force \
                     to override.\n  Otherwise, file a new requirement that supersedes {}.",
                    display_id,
                    req.status,
                    display_id
                );
            }

            let position = if *top {
                let entries = storage.queue_list(&user_id, true)?;
                entries.first().map(|e| e.position - 1000).unwrap_or(1000)
            } else {
                i64::MAX // sentinel: queue_add auto-assigns max+1000
            };

            let spec_id = req.spec_id.as_deref().unwrap_or("???");
            // BUG-81: short id for user-facing prints; `spec_id` stays
            // canonical for the activity-log key (consistency with edit/
            // show/comment events). trace:BUG-81 | ai:claude
            let display_id = req
                .agreed_id
                .as_deref()
                .or(req.spec_id.as_deref())
                .unwrap_or("???");

            // --global routes to ~/.aida/queue/<role>.yaml. The role comes
            // from --for, falling back to the active role. Refuse silently
            // if neither is set — global queues only make sense role-scoped.
            // trace:FR-1-012 | ai:claude
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
                        "--global requires --for <role> or an active role (AIDA_SESSION_ROLE). \
                         The global queue is keyed by role."
                    )
                    })?;
                let project_root = storage
                    .path()
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                let project_root = project_root.canonicalize().unwrap_or(project_root);
                let project_name = global_queue::project_name_for(&project_root);
                let position = if *top {
                    let existing = global_queue::load(&role).unwrap_or_default();
                    existing.first().map(|e| e.position - 1000).unwrap_or(1000)
                } else {
                    i64::MAX
                };
                // Resolve i64::MAX to actual max+1000 inline (the local queue
                // path delegates this to the backend; we do it here ourselves).
                let position = if position == i64::MAX {
                    let existing = global_queue::load(&role).unwrap_or_default();
                    existing.iter().map(|e| e.position).max().unwrap_or(0) + 1000
                } else {
                    position
                };
                let gentry = global_queue::GlobalQueueEntry {
                    requirement_id: req.id,
                    project_root,
                    project_name: project_name.clone(),
                    spec_id: req.spec_id.clone(),
                    // BUG-83: cache agreed_id too so the global queue
                    // can render the short id once one's been assigned.
                    // trace:BUG-83 | ai:claude
                    agreed_id: req.agreed_id.clone(),
                    title: Some(req.title.clone()),
                    position,
                    added_by: user_id.clone(),
                    added_at: chrono::Utc::now(),
                    note: note.clone(),
                    for_role: role.clone(),
                };
                global_queue::add(&role, gentry)?;
                // BUG-65: bump role activity on queue-add so statusline
                // tracks "started caring about this spec" alongside the
                // existing edit/show/comment events.
                // trace:BUG-65 | ai:claude
                record_role_activity(spec_id, "queue-add");
                println!(
                    "{} Added {} ({}) to {} {}",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    display_id.bold(),
                    req.title,
                    "global queue".cyan(),
                    format!("[role:{}, origin:{}]", role, project_name).dimmed()
                );
                return Ok(());
            }

            // STORY-333: warn — never refuse — if this placement puts
            // the new entry ahead of a queued blocker. Operator may have
            // a reason (e.g. branching from the blocker's branch
            // intentionally), so the warning is informational only.
            // trace:STORY-333 | ai:claude
            warn_if_queued_ahead_of_blocker(req, position, &warn_store, storage, &user_id);

            // TASK-618: warn on the silent cross-machine collision hazard.
            // When the queue user_id is the BUG-89 "default" fallback and a
            // remote default.yaml already carries entries from a DIFFERENT
            // machine, two clones are about to write the same orphan-branch
            // file → merge conflict on the next sync rebase. Stamp this
            // clone's fingerprint so the next add can make the same check.
            // trace:TASK-618 | ai:claude
            let this_machine = hostname();
            if let Ok(existing) = storage.queue_list(&user_id, true) {
                if let Some(other) = default_queue_collision_fingerprint(
                    &user_id,
                    &this_machine,
                    existing.iter().map(|e| e.added_by_machine.as_deref()),
                ) {
                    eprintln!(
                        "{} This queue is using the shared '{}' user id and already has \
                         entries from another machine ('{}'). Two machines writing the \
                         same queue can collide on the next sync. Set a distinct user id \
                         per machine (export AIDA_USER=<name>) to keep queues separate.",
                        "warning:".yellow().bold(),
                        "default".bold(),
                        other.bold(),
                    );
                }
            }

            let entry = aida_core::QueueEntry {
                user_id: user_id.clone(),
                requirement_id: req.id,
                position,
                added_by: user_id.clone(),
                note: note.clone(),
                added_at: chrono::Utc::now(),
                for_role: r#for.clone(),
                for_scope: for_scope_routing.clone(),
                for_session: for_session_routing.clone(),
                added_by_machine: Some(this_machine),
            };
            storage.queue_add(entry)?;
            // BUG-65: bump role activity on queue-add so statusline tracks
            // "started caring about this spec" alongside edit/show/comment.
            // trace:BUG-65 | ai:claude
            record_role_activity(spec_id, "queue-add");

            // trace:STORY-57 | ai:claude
            let mut routing_parts: Vec<String> = Vec::new();
            if let Some(r) = &r#for {
                routing_parts.push(format!("for:{}", r));
            }
            if let Some(s) = &for_scope_routing {
                routing_parts.push(format!("@{}", s));
            }
            if let Some(s) = &for_session_routing {
                routing_parts.push(format!("session:{}", &s[..s.len().min(8)]));
            }
            let routing = if routing_parts.is_empty() {
                String::new()
            } else {
                format!(" [{}]", routing_parts.join(" ").cyan())
            };
            println!(
                "{} Added {} ({}) to queue{}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                display_id.bold(),
                req.title,
                routing
            );
        }
        QueueCommand::Remove {
            id,
            user,
            global,
            r#for,
        } => {
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
                let entries = global_queue::load(&role).unwrap_or_default();
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
                    println!(
                        "{} Removed {} from global queue [role:{}, origin:{}]",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        display_id.bold(),
                        role,
                        target.project_name
                    );
                }
                return Ok(());
            }

            let store = storage.load()?;
            let req = if let Ok(uuid) = uuid::Uuid::parse_str(id) {
                store.requirements.iter().find(|r| r.id == uuid)
            } else {
                store.get_requirement_by_spec_id(id)
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
            storage.queue_remove_for_role(&user_id, &req.id, remove_role.as_deref())?;
            // BUG-81: short id when present. trace:BUG-81 | ai:claude
            let display_id = req
                .agreed_id
                .as_deref()
                .or(req.spec_id.as_deref())
                .unwrap_or("???");
            match remove_role.as_deref() {
                Some(role) => println!(
                    "{} Removed {} from queue [role:{}]",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    display_id.bold(),
                    role
                ),
                None => println!(
                    "{} Removed {} from queue",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    display_id.bold()
                ),
            }
        }
        QueueCommand::Move {
            id,
            top,
            bottom,
            to,
            before,
            after,
            force,
        } => {
            // BUG-89: route through the canonical helper so move resolves
            // user_id the same way add/list do (previously this path
            // skipped the USERNAME fallback). trace:BUG-89 | ai:claude
            let user_id = current_user_id(None);
            let store = storage.load()?;

            let req = if let Ok(uuid) = uuid::Uuid::parse_str(id) {
                store.requirements.iter().find(|r| r.id == uuid)
            } else {
                store.get_requirement_by_spec_id(id)
            }
            .ok_or_else(|| not_found::requirement_not_found(id, Some(storage.path())))?;

            let mut entries = storage.queue_list(&user_id, true)?;
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
                    println!(
                        "{} Moved {} to slot {} — --to {} is out of range (queue has {} item{})",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        display_id.bold(),
                        slot,
                        requested,
                        ids.len(),
                        if ids.len() == 1 { "" } else { "s" },
                    );
                } else {
                    println!(
                        "{} Moved {} to slot {} in queue",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        display_id.bold(),
                        slot,
                    );
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
                    store.get_requirement_by_spec_id(before_id)
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
                    store.get_requirement_by_spec_id(after_id)
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
            println!(
                "{} Moved {} in queue",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                display_id.bold()
            );
        }
        // trace:STORY-566 | ai:claude
        QueueCommand::Advance { id, yes, user } => {
            handle_queue_advance(storage, id.as_deref(), *yes, user.as_deref())?
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
        // Prune queue entries matching a predicate: `--orphaned` (spec deleted)
        // and/or `--merged` (shipped reviewer row). The sibling `queue gc`
        // handles the archived/terminal-but-present class.
        // trace:TASK-537 | ai:claude
        // trace:TASK-1063 | ai:claude
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
        // TASK-1052: queue-GC — sweep dead routed entries (target spec
        // archived / Completed / Rejected) and report the count. The explicit
        // companion to the opportunistic self-heal that runs on `queue list`.
        // trace:TASK-1052 | ai:claude
        QueueCommand::Gc {
            user,
            r#for,
            dry_run,
        } => {
            let user_id = get_user(user);
            let entries = storage.queue_list(&user_id, /* include_completed */ true)?;
            let summaries =
                advance_backend(store_path)?.list_summaries(&aida_core::ListFilter::default())?;
            let dead = dead_queue_entries(&entries, &summaries, r#for.as_deref());

            if dead.is_empty() {
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
                let n = dead.len();
                println!(
                    "{} {} dead queue entr{} ({})",
                    if *dry_run {
                        crate::glyph(crate::glyphs::Glyph::InfoAlt).yellow()
                    } else {
                        crate::glyph(crate::glyphs::Glyph::Cross).yellow()
                    },
                    n.to_string().bold(),
                    if n == 1 { "y" } else { "ies" },
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
                if *dry_run {
                    println!();
                    println!("  {}", "Re-run without --dry-run to remove.".dimmed());
                } else {
                    // for_role None → bulk remove-by-spec (one commit). With a
                    // role filter, drop only the matching-role entry per spec so
                    // a sibling entry routed to another role survives.
                    let removed = if r#for.is_none() {
                        let ids: Vec<Uuid> = dead.iter().map(|e| e.requirement_id).collect();
                        storage.queue_remove_many(&user_id, &ids)?.len()
                    } else {
                        for e in &dead {
                            storage.queue_remove_for_role(
                                &user_id,
                                &e.requirement_id,
                                r#for.as_deref(),
                            )?;
                        }
                        dead.len()
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
            let raw_entries = if *global {
                Vec::new()
            } else {
                storage.queue_list(&user_id, /* include_completed */ false)?
            };
            let store = storage.load()?;

            // Same role-filter logic as queue list (BUG-87).
            // `--for X` takes precedence over `--all`. trace:BUG-87 | ai:claude
            let session_role = std::env::var("AIDA_SESSION_ROLE").ok();
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
            // BUG-637: probe liveness once so the queue filter only hides specs a
            // LIVE foreign session is working — a crashed/stale claim must not keep
            // a spec out of the queue forever (no crash-deadlock). Skip the probe
            // entirely when the filter is inactive.
            // trace:BUG-637 | ai:claude
            let lease_live_now = chrono::Utc::now();
            let lease_live_sessions = if lease_filter_active {
                process_probe::probe_live_claude_sessions()
            } else {
                Vec::new()
            };
            let mut skipped_for_lease: Vec<(String, String)> = Vec::new();
            // STORY-333: track specs skipped by the pre-pickup gate so
            // `queue next` can hint why (without a hint, an un-pickable
            // head looks like "queue is empty"). trace:STORY-333 | ai:claude
            let mut skipped_unpickable: Vec<(String, String)> = Vec::new();

            let next_entry = raw_entries
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
                    let Some(req) = store.requirements.iter().find(|r| r.id == e.requirement_id)
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
                    let Some(req) = store.requirements.iter().find(|r| r.id == e.requirement_id)
                    else {
                        return true;
                    };
                    match aida_core::pickability::pickability(req, &store) {
                        aida_core::pickability::Pickability::Pickable => true,
                        aida_core::pickability::Pickability::Blocked(reason) => {
                            let display = req
                                .agreed_id
                                .clone()
                                .or_else(|| req.spec_id.clone())
                                .unwrap_or_else(|| "?".to_string());
                            skipped_unpickable.push((
                                display,
                                aida_core::pickability::pickability_reason_label(&reason),
                            ));
                            false
                        }
                    }
                })
                .filter(|e| {
                    if !lease_filter_active {
                        return true;
                    }
                    let Some(req) = store.requirements.iter().find(|r| r.id == e.requirement_id)
                    else {
                        return true;
                    };
                    let owner = lease_owning_spec(
                        &leases,
                        self_lease.as_ref(),
                        req.id,
                        req.spec_id.as_deref(),
                        &store,
                        lease_is_live(&lease_live_sessions, lease_live_now),
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
                    let Some(req) = store.requirements.iter().find(|r| r.id == e.requirement_id)
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
                .min_by_key(|e| e.position);

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
                let (display_id_owned, title_owned) =
                    format_review_story_display(display_id, title)
                        .unwrap_or_else(|| (display_id.to_string(), title.to_string()));
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
                    let (display_id_owned, title_owned) =
                        format_review_story_display(spec_id, title)
                            .unwrap_or_else(|| (spec_id.to_string(), title.to_string()));
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
        // trace:EPIC-1-001 | ai:claude
        QueueCommand::Done {
            id,
            user,
            yes,
            force,
            skip_pr_check,
            interface_cli,
            interface_mcp,
            interface_tui,
            interface_other,
            no_interface_change,
            test_plan,
            no_test_plan,
        } => {
            let user_id = get_user(user);
            let store = storage.load()?;

            let req = if let Ok(uuid) = uuid::Uuid::parse_str(id) {
                store.requirements.iter().find(|r| r.id == uuid)
            } else {
                store.get_requirement_by_spec_id(id)
            }
            .ok_or_else(|| not_found::requirement_not_found(id, Some(storage.path())))?;

            let spec_id = req.spec_id.as_deref().unwrap_or("???");
            // BUG-81: display short id when one's been assigned; canonical
            // spec_id stays for activity-log / manifest matching (those
            // keys were written with spec_id form). trace:BUG-81 | ai:claude
            let display_id = req
                .agreed_id
                .as_deref()
                .or(req.spec_id.as_deref())
                .unwrap_or("???");

            // BUG-269: refuse `queue done` when the branch carries
            // committed-but-unshipped work with no open PR. Without this
            // gate, a `--zen` or interactive session that forgot to run
            // `/aida-pr` leaves the spec Done locally and unmergeable —
            // the orchestrator (or a watching human) has no way to advance
            // it. The BUG-232 hint nudges; this *enforces*.
            //
            // Skip when `--force` or `--skip-pr-check` (BUG-285: explicit
            // opt-in for the rare case where the spec shipped via a
            // different already-merged branch) and when the BUG-232 hint
            // detector itself can't decide (gh missing / unauthenticated /
            // network failure) — proceeding is the safe default when we
            // can't prove "no PR." Likewise skip when commits_ahead
            // resolution fails (None) so a missing origin/main never blocks
            // a legitimate done. `--yes` does NOT bypass — it's the
            // confirmation-skip for the prompt below, not a gate override.
            // trace:BUG-269 BUG-285 | ai:claude
            let bypass_pr_check =
                workflow_hints::queue_done_should_bypass_pr_check(*yes, *force, *skip_pr_check);
            if !bypass_pr_check {
                // BUG-360 / TASK-500: the gate's decision tree is now a pure
                // `queue_done_precheck_diagnose` (workflow_hints) with the
                // git/gh lookups injected as closures, so every skip path,
                // the refusal, and the proceed are unit-testable in isolation.
                // Each silent-skip path still emits a diagnostic warning so a
                // future bypass leaves a trace in the headless log / cast /
                // stderr — the operator can identify WHICH condition failed
                // without re-instrumenting. The closure mapping ChangeLookup →
                // PrState keeps the gate forge-routed (STORY-516).
                // trace:TASK-500 BUG-360 STORY-516 | ai:claude
                let diagnose = workflow_hints::queue_done_precheck_diagnose(
                    display_id,
                    find_project_root(),
                    |root| current_branch_at(root),
                    |root, branch| branch_commits_ahead_main(root, branch),
                    |root, branch| match change_lookup_for_branch(root, branch) {
                        crate::forge::ChangeLookup::Found(c) => workflow_hints::PrState::Open(c.id),
                        crate::forge::ChangeLookup::NoChange => workflow_hints::PrState::Absent,
                        crate::forge::ChangeLookup::CliMissing
                        | crate::forge::ChangeLookup::CliFailed(_)
                        | crate::forge::ChangeLookup::Unreachable(_) => {
                            workflow_hints::PrState::Unknown
                        }
                    },
                );
                match diagnose {
                    workflow_hints::QueueDoneGateDiagnose::Refuse(lines) => {
                        for line in &lines {
                            eprintln!("{}", line);
                        }
                        std::process::exit(1);
                    }
                    workflow_hints::QueueDoneGateDiagnose::SilentSkip { warning_line, .. } => {
                        eprintln!("{}", warning_line);
                    }
                    workflow_hints::QueueDoneGateDiagnose::Proceed => {}
                }
            } else {
                // BUG-285: an opt-in bypass should NEVER be silent — the
                // headless drain's tee + the user's terminal both need a
                // record that the gate was deliberately skipped, otherwise
                // a routine bypass looks identical to the happy path. Use
                // a `warning:` prefix (matches the rest of the CLI) so the
                // line stands out in scrollback and parses as a soft error
                // for any tooling watching stderr.
                // trace:BUG-285 | ai:claude
                let flag = if *skip_pr_check {
                    "--skip-pr-check"
                } else {
                    "--force"
                };
                eprintln!(
                    "{} bypassing PR check for {} via {} — proceeding without an open PR.",
                    "warning:".yellow().bold(),
                    display_id,
                    flag
                );
            }

            // STORY-469 Guard 1: validate trailer spec-IDs before flipping the
            // spec to Done. Catch a hallucinated / typo'd / since-rejected
            // `(SPEC-ID)` trailer on this branch's commits BEFORE the spec is
            // marked Done (and before the branch's commits — already authored —
            // ride a later merge into shared history). Reuses the STORY-498
            // gate's pure validator + store resolver, client-side. `--force`
            // bypasses (matching its existing "I know what I'm doing" role for
            // the PR check above). trace:STORY-469 | ai:claude
            if let Ok(project_root) = find_project_root() {
                run_client_trailer_guard(&project_root, "queue done", *force);
            }

            // BUG-671: `queue done` is non-destructive and reversible (marks
            // Done + dequeues). With no human to answer the prompt
            // (`non_interactive_confirm`), the old `read_line` gate hit EOF,
            // failed the 'y' check, and CANCELLED — marking nothing Done while
            // an agent capturing stdout (the "Cancelled" notice went to stderr)
            // believed it succeeded. AUTO-CONFIRM instead: the agent explicitly
            // invoked `done`. Only a real interactive non-`--yes` run still
            // prompts. trace:BUG-671 | ai:claude
            if !yes && !non_interactive_confirm() {
                eprintln!(
                    "Mark {} ({}) as done and remove from queue?",
                    display_id.bold(),
                    req.title
                );
                eprintln!("Type 'y' to confirm:");
                let mut answer = String::new();
                if std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut answer).is_err() {
                    eprintln!("Cancelled.");
                    return Ok(());
                }
                if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
                    eprintln!("Cancelled. Requirement and queue untouched.");
                    return Ok(());
                }
            }

            // BUG-684: `queue done` promotes ANY status straight to Done — a
            // Draft/Approved/Planned spec (never started) flips to Done with a
            // green check, so a typo'd id silently corrupts state. This is
            // asymmetric with the strong re-open guard that gates Done → *. Warn
            // (non-blocking) when marking done a spec that was never In Progress
            // so the mistake is VISIBLE, then proceed — a legitimate "I finished,
            // don't nag" agent path exists, so we don't hard-block. Statuses
            // that HAVE been worked (InProgress, NeedsAttention) and the
            // already-terminal ones (Done/Completed) don't trip the warning.
            // trace:BUG-684 | ai:claude
            if matches!(
                req.status,
                RequirementStatus::Draft | RequirementStatus::Approved | RequirementStatus::Planned
            ) {
                eprintln!(
                    "{} {} was never In Progress (was {}) — marking done anyway.",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    display_id.bold(),
                    req.status
                );
            }

            // Update status to Done via update_atomically — works across
            // SQLite and git-canonical modes. set_status_from_str also
            // clears any stale custom_status so the canonical enum value
            // actually takes effect (BUG-1-025).
            // trace:BUG-1-025 | ai:claude
            //
            // STORY-86: `queue done` flips to **Done** (work finished on
            // a branch), not Completed. The auto-bump scan in
            // `aida pull` / `aida db sync --pull` advances Done →
            // Completed once a referencing commit lands on the default
            // branch. Existing call sites that say "completed" in their
            // UI strings have been retargeted to "done" here; the
            // `Completed` semantics ("shipped on main") are preserved.
            // trace:STORY-86 | ai:claude
            //
            // STORY-81 Part 2: also stamp `implementation_info` so the
            // req permanently records who completed it and when, with
            // the AI source-tool when known. This survives the queue
            // entry's deletion (which happens right below) so post-
            // merge `aida show` still surfaces the completion context.
            // `completed_at` / `completion_sha` are intentionally LEFT
            // UNSET here — those are stamped by the STORY-86 auto-bump.
            // trace:STORY-81 | ai:claude
            let req_id = req.id;
            let now = chrono::Utc::now();
            let completer = get_default_author();
            let source_tool = std::env::var("AIDA_AI_TOOL").ok().filter(|s| !s.is_empty());
            storage.update_atomically(|s| {
                if let Some(r) = s.requirements.iter_mut().find(|r| r.id == req_id) {
                    r.set_status_from_str("Done");
                    r.modified_at = now;
                    // Don't clobber prior `summary` / `risk_notes` /
                    // `test_coverage_notes` if the user / `/aida-pr`
                    // skill already populated them. We only set the
                    // fields the queue-done path can know.
                    let info = r
                        .implementation_info
                        .get_or_insert_with(aida_core::ImplementationInfo::default);
                    info.implemented = true;
                    info.implemented_at.get_or_insert(now);
                    if info.implemented_by.is_none() {
                        info.implemented_by = Some(completer.clone());
                    }
                    if let Some(ref tool) = source_tool {
                        info.source_tool.get_or_insert_with(|| tool.clone());
                    }
                }
            })?;
            storage.queue_remove(&user_id, &req_id)?;
            // BUG-65: queue done bypasses Command::Edit (sets status via
            // update_atomically directly), so the role activity log used
            // to miss it entirely — leaving statusline @SPEC stuck on the
            // last show/comment after every shipped spec. Bump explicitly
            // here so the most recently shipped spec wins.
            // trace:BUG-65 | ai:claude
            record_role_activity(spec_id, "done");

            // STORY-98: same reason as the BUG-65 hookup above — `queue
            // done` bypasses Command::Edit, so the manifest-flip path
            // there doesn't fire. Mirror it explicitly so `aida session
            // show --plan` flips the Done check at completion time. STORY-86:
            // pass "Done" since that's the canonical status now (manifest
            // treats Done + Completed equivalently — both check the item
            // off the planned cluster).
            // trace:STORY-98 STORY-86 | ai:claude
            update_manifest_for_status(spec_id, "Done");

            println!(
                "{} {} marked done and removed from queue.",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                display_id.bold()
            );
            println!(
                "  ({})",
                "run `aida queue next` to see what's next".dimmed()
            );

            // BUG-673: next-step breadcrumb after `queue done`. The agent chain
            // is add -> approve -> queue list -> queue work -> queue done ->
            // `aida pull`, but the `next[]` block dropped out exactly here, so
            // an agent that just marked a spec Done was never told the next move
            // is `aida pull` (which fires the Done -> Completed auto-bump once
            // the PR merges) and stranded the spec at Done. Emit the lifecycle
            // block on both surfaces: the TOON `next[]` block in agent mode, the
            // `Next:` block on the human TTY (same idiom `aida show` uses).
            // trace:BUG-673 | ai:claude
            {
                let next = crate::help_next::queue_done_next(display_id);
                let rendered = if agent_output_mode() {
                    crate::help_next::render(&next)
                } else {
                    crate::help_next::render_human(&next)
                };
                if let Some(block) = rendered {
                    println!("{block}");
                }
            }

            // TASK-96: offer to file the plan's Followups bullets as child
            // TASKs. Interactive prompt unless `--yes` (then file all).
            // Best-effort — a failure here never blocks `queue done`.
            // trace:TASK-96 | ai:claude
            if let Ok(project_root) = find_project_root() {
                if let Err(e) =
                    extract_plan_followups(storage, &project_root, spec_id, display_id, !yes)
                {
                    eprintln!(
                        "{} followup extraction skipped: {}",
                        "Warning:".yellow().bold(),
                        e
                    );
                }
            }

            // STORY-542: capture the spec's user-facing interface changes (the
            // deterministic Layer-1 source for `aida digest --audience
            // operator`). Flags win and skip the prompt; otherwise at a TTY
            // (and unless --yes / --no-interface-change) ask per surface.
            // Best-effort — a failure here never blocks `queue done`.
            // trace:STORY-542 | ai:claude
            if let Err(e) = capture_interface_changes(
                storage,
                req_id,
                display_id,
                interface_cli,
                interface_mcp,
                interface_tui,
                interface_other,
                *no_interface_change,
                /* interactive = */ !yes,
            ) {
                eprintln!(
                    "{} interface-change capture skipped: {}",
                    "Warning:".yellow().bold(),
                    e
                );
            }

            // STORY-698: capture the verification steps the builder actually
            // ran — the implementation audit trail surfaced in the PR body.
            // Stored in implementation_info.test_coverage_notes (no new field).
            // `--test-plan` flags win and skip the prompt; otherwise at a TTY
            // (and unless --yes / --no-test-plan) ask. Best-effort — a failure
            // here never blocks `queue done`. trace:STORY-698 | ai:claude
            if let Err(e) = capture_test_plan(
                storage,
                req_id,
                display_id,
                test_plan,
                *no_test_plan,
                /* interactive = */ !yes,
            ) {
                eprintln!(
                    "{} verification-step capture skipped: {}",
                    "Warning:".yellow().bold(),
                    e
                );
            }

            // STORY-106: workflow hint when the queue is now empty for the
            // active role+scope. Best-effort: any state-detection failure
            // skips the hint silently rather than failing the command.
            // trace:STORY-106 | ai:claude
            maybe_hint_after_queue_drain(storage, &user_id);

            // STORY-700: passive first-run chain — advancing past the first
            // `queue done` to the review → merge → `aida pull` hint. Only fires
            // when the arc sits at exactly the spec-filed step; no-op otherwise.
            // trace:STORY-700 | ai:claude
            if let Ok(project_root) = find_project_root() {
                first_run::after_work_done(&project_root);
            }

            // BUG-378: substrate-as-bouncer for the scratchpad-drift ceiling.
            // An agent about to declare "all done" gets told here, before it
            // exits, if new work is queued in the brief surface for its type.
            // trace:BUG-378 | ai:claude
            if let Ok(project_root) = find_project_root() {
                warn_pending_briefs_for_running_agent(&project_root);
            }
        }
        // STORY-42: one-shot queue pickup → session start → claude launch.
        // trace:STORY-42 | ai:claude
        QueueCommand::Work {
            id,
            count,
            permission_mode,
            sandbox,
            no_launch,
            plan_only,
            guided,
            with_plan,
            role,
            no_pull,
            type_filter,
            branch,
            path,
            stack,
            base,
            force_base,
            steal,
            force_claim,
            force,
            batch,
            batches,
            single_branch,
            sequential,
            dry_run,
            resume,
            fresh,
            list_sessions,
            session_id,
            vendor,
            auto_complete,
            drain,
            json,
            max,
            max_failures,
            max_tokens,
            max_iterations,
            max_runtime,
            no_progress_minutes,
            phase_ceiling_minutes,
            resume_drain,
            drain_id,
            resume_dry_run,
            from_pr,
            no_human,
            escalate_blocks,
            escalate_defaults,
            zen,
            pause_always,
            quiet,
            no_tee_headless,
            user,
            calibrate,
            no_calibrate,
            allow_stale_base,
            allow_intermediate_only,
            no_auto_rebase,
            complexity,
            assist_est,
            effort,
            panes,
            strict,
        } => {
            let user_id = get_user(user);
            // TASK-1120: opt-in pane hosting. Export the requested host so every
            // implementer this drain spawns (single / --batch / nextN) reads it
            // at its run_implementer spawn point. Faithful-launcher: an absent
            // flag leaves the env untouched → byte-identical background spawn.
            if let Some(host) = panes.as_deref() {
                std::env::set_var(pane_host::HOST_ENV, host);
            }
            // TASK-1116: an explicit `--vendor`/`--agent` ALSO routes the
            // headless `--auto-complete` implementer (and any headless
            // orchestrator phase), not just the interactive host — so it is no
            // longer a silent no-op on an autonomous drain. Installed as the
            // top-precedence override BEFORE the orchestrator spawns the
            // `aida queue work <spec> --no-human` implementer child, which
            // inherits the choice via `AIDA_HEADLESS_VENDOR`. A recognized
            // token (claude/codex) installs; an unrecognized value is left to
            // the interactive host path's existing handling below. trace:TASK-1116
            if let Some(raw) = vendor.as_deref() {
                let _ = session::install_headless_vendor_override(raw);
            }
            // STORY-761: resolve the interactive host vendor — explicit
            // `--vendor` flag > the uniform `[agents] vendor` knob
            // (agents.toml, project over global) > `claude`. Shadowed so
            // everything downstream reads the resolved value.
            let vendor: String = vendor.clone().unwrap_or_else(|| {
                find_project_root()
                    .ok()
                    .and_then(|root| aida_core::agents_config::resolve_default_vendor(&root))
                    .unwrap_or_else(|| "claude".to_string())
            });
            // STORY-717: focus-scope drift guard at the queue-work work-start
            // moment. When an explicit spec `id` is named and the worktree has
            // a focus set, refuse/nudge if the spec is outside the focus
            // subtree per `[focus] out_of_scope`. `--force` always overrides.
            // The `next` / `nextN` / `batch:NAME` keywords don't resolve to a
            // spec, so the guard naturally no-ops for them. trace:STORY-717
            if let Some(spec_arg) = id.as_deref() {
                if let Ok(project_root) = find_project_root() {
                    focus_scope_guard_for_spec(&project_root, spec_arg, *force)?;
                }
            }
            // TASK-578: expand the `--drain` discoverability alias into the
            // underlying `--auto-complete --no-human=both --max <queue-size>`
            // state before anything downstream reads those flags. `--drain` only
            // sets state — every guard, dispatch, and orchestrator path below
            // behaves exactly as if the operator had typed the long form. The
            // drivable queue size feeds the `--max` default (the bare-queue
            // drain N); a best-effort `0` falls back to 99 inside the resolver.
            // Explicit flags always win. trace:TASK-578 | ai:claude
            let drain_queue_size = if *drain {
                drivable_queued_count(storage, &user_id).unwrap_or(0)
            } else {
                0
            };
            let drain_resolution = resolve_drain_alias(
                *drain,
                auto_complete.as_deref(),
                no_human.as_deref(),
                *max,
                drain_queue_size,
            );
            // Shadow the raw flags with their drain-resolved values. `max` is
            // only re-applied on routes where it is meaningful (the bare-queue
            // nextN drain and a batch drain); on the single-spec path the
            // existing `--max` rejection still applies, so keep `max` itself the
            // operator-supplied value there. trace:TASK-578 | ai:claude
            let auto_complete = &drain_resolution.auto_complete;
            let no_human = &drain_resolution.no_human;
            // TASK-966: assemble the hard budget caps once, up front, so a bad
            // `--max-runtime` value bails before any drain work starts. The caps
            // are threaded into the multi-spec drain loops (nextN / batch /
            // batches); they stop the drain cleanly at a spec boundary and
            // compose with `--max-failures` + the goal condition (whichever
            // fires first). trace:TASK-966 | ai:claude
            let drain_caps = {
                let max_runtime = match max_runtime.as_deref() {
                    Some(s) => match drain_caps::parse_duration(s) {
                        Some(d) => Some(d),
                        None => anyhow::bail!(
                            "could not parse --max-runtime value {:?} — use minutes \
                             (e.g. `90`) or a suffixed/compound form like `90s`, `45m`, \
                             `2h`, `1h30m`",
                            s
                        ),
                    },
                    None => None,
                };
                drain_caps::DrainCaps {
                    max_tokens: *max_tokens,
                    max_iterations: *max_iterations,
                    max_runtime,
                }
            };
            // TASK-560: reject --resume + --auto-complete with a message that
            // explains the conflict and names both recovery paths, instead of
            // clap's terse "cannot be used with". trace:TASK-560 | ai:claude
            if let Some(msg) =
                resume_autocomplete_conflict_message(resume.is_some(), auto_complete.is_some())
            {
                anyhow::bail!(msg);
            }
            // STORY-647: team RBAC guardrail — starting an autonomous drain
            // (`--auto-complete`) is an advisor-gated op by default (tunable via
            // `[team.permissions] drain_start`). A live-orchestrator re-entry
            // (`--resume-drain`, and the phase children the orchestrator spawns)
            // holds advisor authority via `has_advisor_authority()`'s
            // orchestrator carve-out, so the gate bypasses those and only the
            // FRESH launch is checked. Advisor authority (TTY / advisor role)
            // bypasses; there is no `--force` on this path, so a non-advisor
            // seats the role. trace:STORY-647 | ai:claude
            if auto_complete.is_some() && !*resume_drain {
                enforce_team_gate(permissions::GatedOp::DrainStart, false)?;
            }
            // TASK-307: propagate the headless-tee flag the same way
            // `--zen` propagates via `AIDA_ZEN` — set the env var once at
            // the top and every downstream child (the direct headless
            // launch, the orchestrator's spawned `aida queue work` phase
            // children) reads it from `TeeOptions::from_env_and_flag`. The
            // env var is the canonical signal; the flag is sugar for setting
            // it. A user-set `AIDA_TEE_HEADLESS=0` in the shell is honored
            // (we never scrub a value we didn't set). trace:TASK-307
            if *no_tee_headless {
                std::env::set_var("AIDA_TEE_HEADLESS", "0");
            }
            // STORY-287: resolve the three-mode autonomy ladder up front,
            // before either the `--auto-complete` orchestrator or the plain
            // `handle_queue_work` path runs. `--zen` works purely through
            // the `AIDA_ZEN` env var — skill templates auto-resolve their
            // `kind:confirmation` prompts when it is set. Setting it here
            // means every downstream child inherits it: the direct
            // `exec_claude`, and the orchestrator's spawned `aida queue
            // work` phase subprocesses (which re-exec `claude`). Same
            // shape as the `AIDA_SESSION_ROLE` propagation below.
            // Precedence: `--no-human` > `--zen` > default — when
            // `--no-human` is effective we actively clear `AIDA_ZEN` so a
            // stale flag (or an inherited env var) never reaches a headless
            // session. trace:STORY-287 | ai:claude
            // TASK-1060 / ADR-10: resolve the autonomy mode ONCE here and carry
            // the typed value; in-process reads (the pre-flight banner below)
            // consult it instead of re-reading the bare `AIDA_ZEN` env var. The
            // env var is still SET below strictly as the cross-process transport
            // to spawned phase children / skill templates. trace:ADR-10 trace:TASK-1060
            let autonomy = resolve_autonomy_mode(*zen, no_human.is_some());
            match autonomy {
                AutonomyMode::Zen => {
                    std::env::set_var(zen::ZEN_ENV, "1");
                    // STORY-564: propagate `--pause-always` to the launched
                    // session so `aida zen finish` pauses at the grab-next/stop
                    // checkpoint instead of auto-exiting on a clean finish. A
                    // leak only ever adds a pause (the safe direction), so —
                    // unlike the zen-intent token — this needs no corroboration.
                    // trace:STORY-564 | ai:claude
                    if *pause_always {
                        std::env::set_var(zen::ZEN_PAUSE_ALWAYS_ENV, "1");
                    }
                    // BUG-237: mint this invocation's zen-intent token. It is
                    // the provenance anchor for `AIDA_ZEN` — the session lease
                    // records it (standalone path), and (under
                    // `--auto-complete`) `run_auto_complete` reads its
                    // presence and records the `zen` flag on the drain-state
                    // file as the orchestrator-path corroboration anchor
                    // (TASK-336). A leaked `AIDA_ZEN=1` carries no token, so
                    // `aida zen status` corroborates it away rather than
                    // auto-resolving a confirmation prompt the user never
                    // authorized. trace:BUG-237 trace:TASK-336 | ai:claude
                    std::env::set_var(zen::ZEN_TOKEN_ENV, uuid::Uuid::now_v7().to_string());
                }
                AutonomyMode::NoHuman => {
                    if *zen {
                        eprintln!(
                            "  {} --zen and --no-human both set; --no-human wins (it is the \
                             stronger autonomy mode — nobody is reachable to consult)",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                        );
                    }
                    std::env::remove_var(zen::ZEN_ENV);
                    std::env::remove_var(zen::ZEN_TOKEN_ENV);
                }
                AutonomyMode::Default => {
                    // BUG-237: scrub any inherited / leaked zen-intent token
                    // so a non-`--zen` `aida queue work` never records zen
                    // provenance — neither in its session lease nor (for an
                    // `--auto-complete` orchestrator) in its run marker.
                    // `AIDA_ZEN` itself is left intact: an orchestrated phase
                    // child legitimately inherits it, and corroboration — not
                    // scrubbing — is the safety net. trace:BUG-237 | ai:claude
                    std::env::remove_var(zen::ZEN_TOKEN_ENV);
                }
            }
            // STORY-564: `--pause-always` only governs the standalone-`--zen`
            // finish; flag it as a no-op when `--zen` isn't effective so a
            // mistyped invocation isn't silently ignored. trace:STORY-564
            if *pause_always && !matches!(autonomy, AutonomyMode::Zen) {
                eprintln!(
                    "  {} --pause-always has no effect without --zen (it governs the \
                     standalone --zen finish checkpoint only)",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                );
            }
            // TASK-270: accept `batch:NAME` as a positional id (equivalent
            // to `--batch NAME`) and strip a redundant `batch:` prefix off
            // the `--batch` flag value. `batch:` is the literal tag printed
            // by `aida queue list`; first-users reflexively copy that whole
            // token back as an identifier. trace:TASK-270 | ai:claude
            let (effective_id, effective_batch) =
                resolve_queue_work_batch(id.as_deref(), batch.as_deref());
            let effective_batches = if let Some(raw) = batches.as_deref() {
                Some(parse_batch_chain(raw)?)
            } else if auto_complete.is_some()
                && effective_batch.map(|b| b.contains(',')).unwrap_or(false)
            {
                Some(parse_batch_chain(effective_batch.unwrap())?)
            } else {
                None
            };
            if auto_complete.is_none() && effective_batch.map(|b| b.contains(',')).unwrap_or(false)
            {
                anyhow::bail!(
                    "comma-separated batches require `--auto-complete` — use \
                     `aida queue work --batch A,B --auto-complete` or `--batches A,B`"
                );
            }
            // TASK-322: a batch drain (--batch NAME / batch:NAME / --batches) and
            // a `nextN` head pickup are two different targets;
            // resolve_queue_work_batch discards the positional when a batch is
            // set, which silently swallowed the `nextN`. Catch the collision
            // explicitly, mirroring the --batch+--type and --max+nextN guards.
            // trace:TASK-322 | ai:claude
            if (effective_batch.is_some() || effective_batches.is_some())
                && id.as_deref().map(is_next_keyword_id).unwrap_or(false)
            {
                anyhow::bail!(
                    "pick one: a batch drain (`--batch NAME`) or a head drain (`nextN`) — not both"
                );
            }
            // TASK-270: clap rejects `--batch` alongside `--type`; the
            // positional `batch:NAME` form bypasses that rule, so enforce
            // the same conflict here rather than silently dropping
            // `--type`. trace:TASK-270 | ai:claude
            if (effective_batch.is_some() || effective_batches.is_some()) && type_filter.is_some() {
                anyhow::bail!(
                    "`--type` does not apply to batch pickup — drop it, or pass an EPIC/STORY id for a typed cluster"
                );
            }
            // TASK-293: resolve the `next` / `nextN` keyword. `next` (N=1) is
            // an explicit alias for no-arg head pickup; `nextN` / `next N`
            // (N>1) is the drain-N-from-head form. A `next*` positional is
            // not a spec-id, so clear `effective_id` — every head-pickup path
            // below keys off `None`. trace:TASK-293 | ai:claude
            let next_kw = parse_next_keyword(effective_id, count.as_deref())?;
            let mut effective_id: Option<&str> = match next_kw {
                NextKeyword::Count(_) => None,
                NextKeyword::NotNext => effective_id,
            };
            // trace:TASK-518 | ai:antigravity
            let resolved_spec_id = if let Some(s) = effective_id {
                if let Some(pr) = parse_pr_arg(s) {
                    // STORY-501/BUG-440: when a "Review PR-N" story is queued,
                    // DON'T resolve PR→backing-spec here. The TASK-518 resolution
                    // (resolve to the implemented spec) is right for a human who
                    // wants to keep working the spec, but it runs BEFORE
                    // resolve_queue_work_plan's review-story pickup (TASK-85) and
                    // so defeated it: `queue work PR-N` became `queue work
                    // <spec>` → an implementer pickup that OWNS + re-implements
                    // the spec (the shared root of the resume reviewer
                    // re-implementing, BUG-436, BUG-438, BUG-440). Leaving "PR-N"
                    // when a review story exists lets TASK-85 route to the
                    // reviewer (/aida-review, PR-scoped lease — no spec
                    // ownership). Resolve to the backing spec only when there is
                    // no review story to pick up. trace:STORY-501 | ai:claude
                    let review_queued = parse_review_scope(s)
                        .map(|(forge, n)| queued_review_story_for_pr(storage, &user_id, forge, n))
                        .unwrap_or(false);
                    if review_queued {
                        None
                    } else {
                        let store = storage.load()?;
                        let project_root = storage.path().parent().ok_or_else(|| {
                            anyhow::anyhow!("cannot derive project root from store path")
                        })?;
                        let resolved = resolve_pr_to_spec(project_root, pr, &store)?;
                        println!("working on {} (backs {})", resolved, s);
                        Some(resolved)
                    }
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(ref res) = resolved_spec_id {
                effective_id = Some(res.as_str());
            }
            // STORY-246: `--auto-complete` drives the full
            // implementer→CI→reviewer→merge→pull→build lifecycle. It is a
            // sibling orchestrator, not a decoration of the exec path —
            // `handle_auto_complete` spawns each phase, waits, and never
            // returns (it always terminates the process with a phase-keyed
            // exit code), so the `queue work` logic below is reached only
            // when `--auto-complete` is absent. trace:STORY-246 | ai:claude
            if auto_complete.is_some() {
                let mode = auto_complete.as_deref().unwrap_or("full");
                let variant = auto_complete::AutoCompleteVariant::parse(mode)
                    .map_err(|e| anyhow::anyhow!(e))?;
                // STORY-561: operator-presence advisory default (consumers a+b).
                // When the operator is AWAY and passes no explicit
                // --no-human / --escalate-* flag, presence fills the drain-mode
                // default per `[presence] away_drain` — the autonomy ladder
                // keying on a presence STATE instead of per-command flags.
                // Explicit flags ALWAYS win, and the kickoff scope-ack +
                // escalate validation below still apply (advisory only —
                // acceptance #4/#5). Presence is only ever `away` here in a
                // non-TTY context (the interactive-TTY auto-flip preempts to
                // home, main.rs:~922), so this sets the default for unattended
                // drains; an interactive operator is already `home`.
                // trace:STORY-561 | ai:claude
                let presence_cfg = {
                    let cfg_path = storage
                        .path()
                        .parent()
                        .map(config_path_for_project)
                        .unwrap_or_else(|| std::path::PathBuf::from(".aida/config.toml"));
                    presence::read_presence_config(&cfg_path)
                };
                let drain_res = presence::resolve_drain_mode(
                    no_human.as_deref(),
                    *escalate_blocks,
                    *escalate_defaults,
                    presence::current_presence(chrono::Utc::now()),
                    &presence_cfg,
                );
                if drain_res.presence_applied {
                    eprintln!(
                        "  {} operator {} → drain defaulting to {}{} (per [presence] away_drain; override with --no-human / --escalate-*)",
                        crate::glyph(crate::glyphs::Glyph::Away),
                        "away".yellow().bold(),
                        drain_res
                            .no_human
                            .as_deref()
                            .map(|s| format!("--no-human={s}"))
                            .unwrap_or_else(|| "interactive".to_string()),
                        if drain_res.no_human.as_deref() == Some("both") {
                            if drain_res.escalate_defaults {
                                " --escalate-defaults"
                            } else {
                                " --escalate-blocks"
                            }
                        } else {
                            ""
                        },
                    );
                }
                // STORY-263 / STORY-276: `--no-human[=MODE]` runs the
                // orchestrator's phases headless (`claude -p`). `reviewer-only`
                // wires the reviewer (phase 3); `both` additionally runs the
                // implementer (phase 1) headless, with the `/aida-punt` safety
                // net for design-forks (STORY-276). The slug is the explicit
                // flag's value, or the presence-supplied default (STORY-561).
                let no_human_mode = match drain_res.no_human.as_deref() {
                    Some(v) => {
                        Some(auto_complete::NoHumanMode::parse(v).map_err(|e| anyhow::anyhow!(e))?)
                    }
                    None => None,
                };
                let aida_headless_env = std::env::var("AIDA_HEADLESS")
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false);
                let requested_no_human_mode = no_human_mode;
                let no_human_mode = non_tty_interactive_implementer_preflight(
                    no_human_mode,
                    std::io::IsTerminal::is_terminal(&std::io::stdin()),
                    std::io::IsTerminal::is_terminal(&std::io::stdout()),
                    aida_headless_env,
                )?;
                if no_human_mode == Some(auto_complete::NoHumanMode::Both)
                    && requested_no_human_mode != Some(auto_complete::NoHumanMode::Both)
                    && aida_headless_env
                {
                    std::env::set_var("AIDA_NO_HUMAN_ACKNOWLEDGED", "1");
                    eprintln!(
                        "  {} AIDA_HEADLESS=1 detected — running the implementer headless",
                        crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
                    );
                }
                // TASK-306 / STORY-276: pre-launch gate. Both modes print the
                // loud scope banner (wording per mode) and require a one-time
                // acknowledgement. This dispatch arm runs exactly once per
                // `aida queue work --auto-complete` invocation, so the banner
                // appears once per kickoff even when a batch / `nextN` drain
                // loops `run_auto_complete` internally. trace:TASK-306, STORY-276
                if let Some(mode) = no_human_mode {
                    no_human_kickoff_gate(mode)?;
                }
                // STORY-306: resolve how an advisor escalation is handled.
                // `--escalate-blocks` (the default — pause, don't guess) and
                // `--escalate-defaults` are `--no-human=both`-only: the
                // advisor tier exists only in a fully-headless drain. Reject
                // the flag anywhere else so it never silently no-ops.
                // trace:STORY-306 | ai:claude
                if (*escalate_blocks || *escalate_defaults)
                    && no_human_mode != Some(auto_complete::NoHumanMode::Both)
                {
                    anyhow::bail!(
                        "--escalate-blocks / --escalate-defaults only apply to a \
                         fully-headless drain — pair them with `--no-human=both`"
                    );
                }
                // STORY-561: the escalate default folds in presence — explicit
                // --escalate-* flags win (validated above), else the
                // presence-supplied `away_drain` advice (defaults vs park).
                // `drain_res.escalate_defaults` == `*escalate_defaults` whenever
                // presence supplied nothing, so the non-away path is unchanged.
                let escalate_mode =
                    auto_complete::EscalateMode::from_flags(drain_res.escalate_defaults);
                // STORY-347: per-drain calibration override propagates via an
                // env var, so it composes with `--batch` / `nextN` / single-
                // spec drains without threading through three signatures.
                // `run_advisor` reads `AIDA_CALIBRATE` as `1` (force on) or `0`
                // (force off); absence falls through to `[advisor]
                // calibration_mode` in `.aida/config.toml`. Only meaningful
                // under `--no-human=both` where the advisor tier runs.
                // trace:STORY-347 | ai:claude
                if *calibrate || *no_calibrate {
                    if no_human_mode != Some(auto_complete::NoHumanMode::Both) {
                        anyhow::bail!(
                            "--calibrate / --no-calibrate only apply when the advisor tier runs — \
                             pair with `--no-human=both`"
                        );
                    }
                    std::env::set_var("AIDA_CALIBRATE", if *calibrate { "1" } else { "0" });
                } else {
                    std::env::remove_var("AIDA_CALIBRATE");
                }
                // BUG-420: map the watchdog flags to env vars so the resolved
                // `DrainTuning` (read in `RealPhaseDriver::new`) picks them up
                // across single-spec / batch / nextN drains without threading
                // through every handler signature — the same pattern
                // `AIDA_CALIBRATE` uses just above. trace:BUG-420 | ai:claude
                if let Some(m) = no_progress_minutes {
                    std::env::set_var("AIDA_NO_PROGRESS_MINUTES", m.to_string());
                }
                if let Some(m) = phase_ceiling_minutes {
                    std::env::set_var("AIDA_PHASE_CEILING_MINUTES", m.to_string());
                }
                // TASK-480: propagate the intermediate-only opt-out to the
                // orchestrator phase-3 reviewer pre-flight via an env var
                // (same pattern as the BUG-420 watchdog flags) so we don't
                // thread the bool through every batch/nextN/auto-complete
                // handler signature. The single-spec `handle_queue_work`
                // path below also receives it explicitly.
                // trace:TASK-480 | ai:claude
                if *allow_intermediate_only {
                    std::env::set_var("AIDA_ALLOW_INTERMEDIATE_ONLY", "1");
                }
                // STORY-265 slice 3: `--with-plan` runs a PLAN PRELUDE (plan
                // session → Approved→Planned promote) before the drain's phase
                // 1. Propagate it via an env var — the same pattern AIDA_CALIBRATE
                // / AIDA_ALLOW_INTERMEDIATE_ONLY use — so it composes across the
                // single-spec / batch / nextN drain routes without threading a
                // new bool through every handler signature. `run_auto_complete`
                // reads it once and runs the prelude before driving the phases;
                // the Phase enum is untouched (the prelude is NOT a renumbered
                // phase). trace:STORY-265 | ai:claude
                if *with_plan {
                    std::env::set_var("AIDA_WITH_PLAN", "1");
                }
                // BUG-538: take the global drain lock before dispatching to ANY
                // of the auto-complete routes (single / batch / batches / nextN /
                // --drain / --from-pr / --resume-drain). This drain integrates on
                // main; a second one (here or via `aida burndown run`) would
                // double-drive the tree. The guard is held for the rest of this
                // arm — the route handlers terminate via `std::process::exit`, so
                // Drop won't fire, but a dead pid is stale-reclaimed by the next
                // launch (see drain_lock module docs). The `--resume-dry-run`
                // preview drives nothing, so it skips the lock. A real
                // `--resume-drain` re-entry correctly reclaims the crashed
                // drain's now-dead lock (or refuses if its pid is somehow alive).
                // trace:BUG-538 | ai:claude
                let _drain_guard = if *resume_dry_run {
                    None
                } else {
                    let drain_lock_command = {
                        let mut c = String::from("queue work --auto-complete");
                        if let Some(b) = effective_batches.as_deref() {
                            c.push_str(&format!(" --batches {}", b.join(",")));
                        } else if let Some(b) = effective_batch {
                            c.push_str(&format!(" --batch {b}"));
                        } else if let NextKeyword::Count(n) = next_kw {
                            if n > 1 {
                                c.push_str(&format!(" next{n}"));
                            }
                        } else if let Some(s) = effective_id {
                            c.push(' ');
                            c.push_str(s);
                        }
                        c
                    };
                    let project_root = find_main_worktree_root()?;
                    Some(drain_lock::acquire_drain_lock(
                        &project_root,
                        &drain_lock_command,
                    )?)
                };
                // BUG-660: prevent the host from sleeping for the duration of an
                // unattended drive — a lidded/idle laptop must not suspend
                // mid-drain. Best-effort (a missing caffeinate / systemd-inhibit
                // degrades to a no-op) and scoped to the drive's lifetime: the
                // held child is pid-tied so it auto-releases on the route
                // handlers' `std::process::exit` (which skips `Drop`). The
                // `--resume-dry-run` preview drives nothing, so it stays
                // unarmed alongside the lock above. trace:BUG-660 | ai:claude
                let _sleep_inhibitor = if *resume_dry_run {
                    None
                } else {
                    let inhibitor =
                        drive_robustness::SleepInhibitor::for_drive("aida unattended drive");
                    if let Some(tool) = inhibitor.tool() {
                        eprintln!(
                            "{} sleep-prevention active for this drive (via {})",
                            crate::glyph(crate::glyphs::Glyph::Check).green(),
                            tool
                        );
                    }
                    Some(inhibitor)
                };
                // STORY-492: `--resume-drain` is its own entry — read the crashed
                // drain-state, gate on PID-liveness, reconcile from reality, and
                // re-enter (or `--dry-run` preview). It never returns. Routed
                // before the batch/single dispatch since it replaces them.
                // trace:STORY-492 | ai:claude
                if *resume_drain {
                    handle_drain_resume(
                        storage,
                        &user_id,
                        drain_id.as_deref(),
                        *resume_dry_run,
                        *json,
                        permission_mode.as_deref(),
                        no_human_mode,
                        escalate_mode,
                        *steal,
                        *force_claim,
                        *allow_stale_base,
                        *no_auto_rebase,
                    );
                }
                // TASK-1003 / SPIKE-70: `--single-branch` drives a coupled batch
                // on ONE shared branch in ONE worktree → ONE cluster PR
                // (commit-per-member, Implementer+CI only between members,
                // halt-on-failure). Routed BEFORE the per-member-PR batch drains
                // below since it replaces them. Requires exactly one `--batch`.
                // trace:TASK-1003 | ai:claude
                if *single_branch {
                    let Some(batch_name) = effective_batch.filter(|b| !b.is_empty()) else {
                        anyhow::bail!(
                            "--single-branch drives ONE coupled batch on one shared branch — \
                             pair it with `--batch NAME` (e.g. `aida queue work --batch NAME \
                             --auto-complete --single-branch`)"
                        );
                    };
                    if effective_batches.is_some() {
                        anyhow::bail!(
                            "--single-branch drives ONE shared branch — use a single \
                             `--batch NAME`, not `--batches`"
                        );
                    }
                    handle_auto_complete_single_branch(
                        storage,
                        &user_id,
                        batch_name,
                        variant,
                        *json,
                        role.as_deref(),
                        *max,
                    );
                }
                // TASK-1005 / SPIKE-70: `--sequential` NAMES + guards the existing
                // per-member-PR batch drain as a first-class coupled-ordered mode:
                // members run ONE AT A TIME (concurrency pinned to 1), each its own
                // PR off freshly-pulled main, with shelve-and-continue on a member
                // failure. It does NOT change the engine — it requires a batch and
                // then falls through to the same `handle_auto_complete_batch[es]`
                // dispatch below, which `drain_batch` already drives sequentially.
                // `conflicts_with = "single_branch"` is enforced by clap.
                // trace:TASK-1005 | ai:claude
                if *sequential {
                    let has_batch = effective_batch.is_some_and(|b| !b.is_empty())
                        || effective_batches.is_some();
                    if !has_batch {
                        anyhow::bail!(
                            "--sequential drives a batch one member at a time — pair it with \
                             `--batch NAME` or `--batches A,B,C` (e.g. `aida queue work \
                             --batch NAME --auto-complete --sequential`)"
                        );
                    }
                    if !*json {
                        eprintln!(
                            "Sequential drain: members run one at a time (concurrency {SEQUENTIAL_DRAIN_CONCURRENCY}); \
                             each member is its own PR off freshly-pulled main, and a member \
                             failure shelves that member and continues with the rest."
                        );
                    }
                    // Fall through to the batch / batches dispatch below — it IS
                    // the sequential one-member-at-a-time engine.
                }
                // TASK-285: `--batch NAME --auto-complete` drains the whole
                // batch — one full lifecycle per member, advancing the head
                // after each — instead of one session per re-invocation.
                // `--max` bounds the drain. trace:TASK-285 | ai:claude
                if let Some(batch_names) = effective_batches.as_deref() {
                    handle_auto_complete_batches(
                        storage,
                        &user_id,
                        batch_names,
                        variant,
                        *json,
                        permission_mode.as_deref(),
                        role.as_deref(),
                        *max,
                        *max_failures,
                        no_human_mode,
                        escalate_mode,
                        *steal,
                        *force_claim,
                        *allow_stale_base,
                        *no_auto_rebase,
                        &drain_caps,
                    );
                }
                if let Some(batch_name) = effective_batch {
                    if batch_name.is_empty() {
                        anyhow::bail!(
                            "batch name is empty — pass `--batch NAME` or `aida queue work batch:NAME`"
                        );
                    }
                    handle_auto_complete_batch(
                        storage,
                        &user_id,
                        batch_name,
                        variant,
                        *json,
                        permission_mode.as_deref(),
                        role.as_deref(),
                        *max,
                        *max_failures,
                        no_human_mode,
                        escalate_mode,
                        *steal,
                        *force_claim,
                        *allow_stale_base,
                        *no_auto_rebase,
                        &drain_caps,
                    );
                }
                // TASK-293: `nextN --auto-complete` drains N specs from the
                // queue head sequentially — one full lifecycle each, advancing
                // the head after every ship. `next` / `next1` carries N=1 and
                // falls through to the single-head path below. trace:TASK-293
                if let NextKeyword::Count(n) = next_kw {
                    if n > 1 {
                        if max.is_some() {
                            anyhow::bail!(
                                "`next{n}` already bounds the drain to {n} specs — drop `--max`"
                            );
                        }
                        handle_auto_complete_next_n(
                            storage,
                            &user_id,
                            n,
                            variant,
                            *json,
                            permission_mode.as_deref(),
                            no_human_mode,
                            escalate_mode,
                            *steal,
                            *force_claim,
                            *allow_stale_base,
                            *no_auto_rebase,
                            *max_failures,
                            &drain_caps,
                        );
                    }
                }
                // TASK-578: a bare `--drain` (no batch, no positional spec, no
                // `nextN` keyword) drains the whole drivable queue. It expands to
                // a `nextN` drain where N is the resolved drain `--max` (the
                // queue size, an explicit `--max`, or the 99 fallback). Routing
                // through the existing nextN orchestrator gives us the
                // skip-undrivable + per-member-lifecycle behaviour for free.
                // trace:TASK-578 | ai:claude
                if *drain && effective_id.is_none() {
                    let n = drain_resolution.max.unwrap_or(99).max(1);
                    handle_auto_complete_next_n(
                        storage,
                        &user_id,
                        n,
                        variant,
                        *json,
                        permission_mode.as_deref(),
                        no_human_mode,
                        escalate_mode,
                        *steal,
                        *force_claim,
                        *allow_stale_base,
                        *no_auto_rebase,
                        *max_failures,
                        &drain_caps,
                    );
                }
                // `--max` only bounds a batch drain — reject it on the
                // single-spec path so the flag never silently no-ops.
                // trace:TASK-285 | ai:claude
                if max.is_some() {
                    anyhow::bail!(
                        "`--max` bounds a batch drain — pair it with `--batch NAME` \
                         (e.g. `aida queue work --batch NAME --auto-complete --max 3`)"
                    );
                }
                // TASK-292: with no positional SPEC, inherit the no-arg
                // `aida queue work` "pick the queue head" semantics instead of
                // demanding an explicit id — `--auto-complete` composes with
                // head pickup the same way the interactive form already does.
                // trace:TASK-292 | ai:claude
                let spec = match effective_id {
                    Some(s) => s.to_string(),
                    None => resolve_auto_complete_head(storage, &user_id)?,
                };
                // TASK-405: `--from-pr` — implementation shipped OUTSIDE the
                // orchestrator (a PR is already open). Drive phases 3-6 only,
                // skipping the implementer. Routed here on the single-spec path
                // (batch / nextN drains exit above). The shared `--resume-dry-run`
                // flag previews the plan. trace:TASK-405 | ai:claude
                if *from_pr {
                    handle_from_pr(
                        storage,
                        &user_id,
                        &spec,
                        variant,
                        *resume_dry_run,
                        *json,
                        permission_mode.as_deref(),
                        no_human_mode,
                        escalate_mode,
                        *steal,
                        *force_claim,
                        *allow_stale_base,
                        *no_auto_rebase,
                    );
                }
                handle_auto_complete(
                    storage,
                    &user_id,
                    &spec,
                    variant,
                    *json,
                    permission_mode.as_deref(),
                    no_human_mode,
                    escalate_mode,
                    *steal,
                    *force_claim,
                    *allow_stale_base,
                    *no_auto_rebase,
                );
            }
            // TASK-293: a multi-spec `nextN` has no coherent single
            // interactive session — draining N unrelated head items into one
            // branch/PR is wrong, so it requires the orchestrator. `next`
            // (N=1) already had `effective_id` cleared to None above and
            // falls through as a plain head pickup. trace:TASK-293 | ai:claude
            if let NextKeyword::Count(n) = next_kw {
                if n > 1 {
                    anyhow::bail!(
                        "`next{n}` drains {n} specs sequentially — it needs `--auto-complete`:\n  \
                         aida queue work next{n} --auto-complete\n\
                         For a single interactive pickup use `aida queue work next` (or just `aida queue work`)."
                    );
                }
            }
            // TASK-229: --batch NAME picks the head of the items tagged
            // `batch:NAME`. --dry-run lists the batch instead of acting.
            // The standard handle_queue_work path takes a spec id; we
            // resolve the batch head here and pass it through. Each
            // invocation drains one item (head-pickup loop semantics —
            // re-run after each session exits to advance).
            // trace:TASK-229 | ai:claude
            let resolved_id: Option<String> = if let Some(name) = effective_batch {
                if name.is_empty() {
                    anyhow::bail!(
                        "batch name is empty — pass `--batch NAME` or `aida queue work batch:NAME`"
                    );
                }
                let want = format!("batch:{}", name);
                // trace:TASK-285 | ai:claude — shared with the
                // `--auto-complete` batch drain.
                let members = resolve_batch_members(storage, &user_id, name, role.as_deref())?;
                if *dry_run {
                    if members.is_empty() {
                        println!(
                            "(no queued items tagged `{}` — add the tag via `aida edit <id> --tags {}`)",
                            want.cyan(),
                            want
                        );
                    } else {
                        println!(
                            "{} `{}` ({} item{}, pickup order):",
                            "Batch".bold(),
                            want.cyan(),
                            members.len(),
                            if members.len() == 1 { "" } else { "s" }
                        );
                        for (i, (_, display_id, title, status)) in members.iter().enumerate() {
                            println!(
                                "  {:>2}. {} [{}] {}",
                                i + 1,
                                display_id.bold(),
                                format!("{}", status).dimmed(),
                                title
                            );
                        }
                        println!();
                        println!(
                            "  {}",
                            format!("(run `aida queue work --batch {}` to pick up the head; repeat per item)", name).dimmed()
                        );
                    }
                    return Ok(());
                }
                let Some(head) = members.into_iter().next() else {
                    anyhow::bail!(
                        "no queued items tagged `{}` — tag members via `aida edit <id> --tags {}` first",
                        want,
                        want
                    );
                };
                Some(head.1)
            } else {
                // TASK-1053: a single-spec `--dry-run` is no longer a
                // dead-end — it flows into handle_queue_work, which prints
                // the resolved plan and returns before any side effect.
                // trace:TASK-1053 | ai:claude
                effective_id.map(|s| s.to_string())
            };
            handle_queue_work(
                storage,
                &user_id,
                resolved_id.as_deref().or(effective_id),
                permission_mode.as_deref(),
                *sandbox,
                *no_launch,
                role.as_deref(),
                *no_pull,
                type_filter.as_deref(),
                branch.as_deref(),
                path.as_deref(),
                // STORY-248: stacked-branch base resolution. Mutually
                // exclusive at the CLI level — `--stack` auto-picks the
                // freshest un-merged implementer lease, `--base BRANCH`
                // takes a name. Threaded through into `session_start` via
                // its existing `base` parameter. trace:STORY-248 | ai:claude
                *stack,
                base.as_deref(),
                *force_base,
                *steal,
                *force_claim,
                resume.as_deref(),
                *fresh,
                *list_sessions,
                session_id.as_deref(),
                // TASK-895: which vendor CLI hosts the interactive session
                // (resolved above: flag > [agents] vendor > claude).
                // trace:TASK-895 STORY-761 | ai:claude
                &vendor,
                // STORY-263: presence of `--no-human` (any value) launches
                // this session headless. The orchestrator appends a bare
                // `--no-human` to its reviewer subprocess.
                no_human.is_some(),
                // TASK-1060 / ADR-10: the typed autonomy resolved once above.
                autonomy,
                // TASK-272: the resolved `--batch NAME` (None for a plain
                // or item-mode pickup) — recorded on the session manifest.
                effective_batch,
                // BUG-226: suppress the standalone reviewer summary.
                *quiet,
                // STORY-281: opt out of the reviewer pre-flight stale-base
                // refusal. trace:STORY-281 | ai:claude
                *allow_stale_base,
                // TASK-480: opt out of the reviewer pre-flight
                // intermediate-only refusal. trace:TASK-480 | ai:claude
                *allow_intermediate_only,
                // STORY-439: pickup-time calibration capture. Each value
                // writes to .aida/complexity-calibration/<SPEC>.yaml AND
                // stamps a tag on the spec.
                *complexity,
                *assist_est,
                *effort,
                *strict,
                // STORY-265: plan-only mode — launch /aida-plan in `plan`
                // permission mode instead of /aida-pickup implement.
                *plan_only,
                // STORY-735: guided keystone mode — launch
                // /aida-guided-implement (structured decision dialog) instead
                // of /aida-pickup. trace:STORY-735 | ai:claude
                *guided,
                // TASK-1053: single-spec dry-run — print the resolved plan
                // (session id, worktree, branch, role, skill, lease) and
                // return before any side effect. trace:TASK-1053 | ai:claude
                *dry_run,
            )?;
        }
        // TASK-232: progress view across the buckets a draining session
        // moves items through (Shipped / In flight / Working now /
        // Remaining). trace:TASK-232 | ai:claude
        QueueCommand::Progress {
            session,
            batch,
            since,
            verbose,
        } => {
            handle_queue_progress(
                storage,
                session.as_deref(),
                // TASK-270: strip a redundant `batch:` prefix off `--batch`
                // so `--batch batch:NAME` == `--batch NAME`. trace:TASK-270
                batch.as_deref().map(normalize_batch_name),
                since.as_deref(),
                *verbose,
            )?;
        }
        // TASK-218: encapsulate the implementer → reviewer → fixup
        // three-command recovery sequence (`aida edit --status` +
        // `aida queue add` + optional `aida queue work`) behind a single
        // verb. Smart status table flips per the current status; `--work`
        // chains the session launch. trace:TASK-218 | ai:claude
        QueueCommand::Rework {
            id,
            work,
            r#for,
            status,
            reason,
            resume,
            force,
            steal,
            permission_mode,
            no_pull,
            user,
        } => {
            handle_queue_rework(
                storage,
                id,
                *work,
                r#for.as_deref(),
                status.as_deref(),
                reason.as_deref(),
                *resume,
                *force,
                *steal,
                permission_mode.as_deref(),
                *no_pull,
                user.as_deref(),
            )?;
        }
        // STORY-384: failed-phase-1 recovery wizard — inspect state, recommend a
        // recovery path, step through it. A front-end over existing primitives.
        // trace:STORY-384 | ai:claude
        QueueCommand::Recover {
            id,
            dry_run,
            auto,
            user,
        } => {
            let user_id = current_user_id(user.as_deref());
            handle_queue_recover(storage, &user_id, id, *dry_run, *auto)?;
        }
        QueueCommand::Integrate {
            dry_run,
            once: _,
            watch,
            interval,
            max,
            rebase,
            strategy,
            focus,
            idle_minutes,
            force,
            user,
        } => {
            // STORY-647: team RBAC guardrail — integrating ready PRs into the
            // default branch is an advisor-gated op by default (tunable via
            // `[team.permissions] integrate`). Checked before any work, including
            // dry-run, so the gate is what the harness probes. `--force` /
            // advisor authority bypass. trace:STORY-647 | ai:claude
            enforce_team_gate(permissions::GatedOp::Integrate, *force)?;
            let user_id = current_user_id(user.as_deref());
            handle_queue_integrate(
                storage,
                &user_id,
                *dry_run,
                *watch,
                *interval,
                *max,
                *rebase,
                *strategy,
                focus.clone(),
                *idle_minutes,
            )?;
        }
    }
    Ok(())
}

/// TASK-218: smart status-transition table for `aida queue rework`.
/// Returns `Some(target)` when the spec's current status maps to a flip
/// per the table below, or `None` when the status is already in a
/// reasonable rework state (just queue-add it, don't flip).
///
///   Draft      → None         (unusual; preserve)
///   Approved   → None         (ready to queue as-is)
///   Planned    → InProgress
///   InProgress → None         (already there; caller warns unless --force)
///   Done       → InProgress   (typical PR-review-found-issues case)
///   Completed  → InProgress   (requires --force at caller)
///   Rejected   → Approved     (requires --force at caller)
///
/// Pure function — separated from `handle_queue_rework` so the table
/// can be unit-tested without spinning up a storage backend.
/// trace:TASK-218 | ai:claude
pub(crate) fn rework_smart_target(current: &RequirementStatus) -> Option<RequirementStatus> {
    match current {
        RequirementStatus::Draft => None,
        RequirementStatus::Approved => None,
        RequirementStatus::Planned => Some(RequirementStatus::InProgress),
        RequirementStatus::InProgress => None,
        RequirementStatus::Done => Some(RequirementStatus::InProgress),
        RequirementStatus::Completed => Some(RequirementStatus::InProgress),
        RequirementStatus::Rejected => Some(RequirementStatus::Approved),
        // STORY-332: reworking a punted spec resumes the paused work.
        RequirementStatus::NeedsAttention => Some(RequirementStatus::InProgress),
    }
}

/// TASK-232: `aida queue progress` — show what a session has shipped so
/// far alongside what remains, bucketed into Shipped / In flight /
/// Working now / Remaining. Resolves the spec set from a session manifest
/// (default or `--session ID`), a `batch:NAME` tag (`--batch`), or a
/// modified-since timestamp (`--since`). Status comes from the live
/// store; manifest timestamps are used only to compute the source set,
/// not to classify status (which avoids drift — see
/// `session_manifest::classify_item` for the precedent).
/// trace:TASK-232 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProgressBucket {
    Shipped,
    InFlight,
    WorkingNow,
    Remaining,
}

pub(crate) fn classify_progress_bucket(status: &aida_core::RequirementStatus) -> ProgressBucket {
    use aida_core::RequirementStatus;
    match status {
        RequirementStatus::Completed | RequirementStatus::Rejected => ProgressBucket::Shipped,
        RequirementStatus::Done => ProgressBucket::InFlight,
        RequirementStatus::InProgress => ProgressBucket::WorkingNow,
        // STORY-332: a punted spec is paused pending triage — still work the
        // batch must land, so it buckets with the not-yet-done Remaining set
        // (its warning status glyph carries the "needs a decision" signal).
        RequirementStatus::NeedsAttention => ProgressBucket::Remaining,
        _ => ProgressBucket::Remaining,
    }
}

/// A spec is "shelved" when parked in `NeedsAttention` (a punt/escalation the
/// drain could not auto-resolve). Surfaced as a distinct drain-legibility
/// callout in `aida queue progress` — the "M shelved" half of the
/// "N draining / M shelved" view a parallel autonomous drain needs. Bucketing
/// is deliberately UNCHANGED (STORY-332): a shelved spec still counts in
/// `Remaining` as work the batch must land; this only adds a visible count so
/// the parked work is legible at a glance instead of hiding inside Remaining.
/// trace:STORY-490 | ai:claude
pub(crate) fn status_is_shelved(status: &aida_core::RequirementStatus) -> bool {
    matches!(status, aida_core::RequirementStatus::NeedsAttention)
}

/// Parse a `--since` value as either an RFC3339 timestamp or a relative
/// `<N>{d,h,m}` expression (e.g. `2d`, `12h`, `45m`). Returns the
/// resulting absolute UTC timestamp.
pub(crate) fn parse_since_arg(raw: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        anyhow::bail!("--since cannot be empty");
    }
    // Try RFC3339 first.
    if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(trimmed) {
        return Ok(ts.with_timezone(&chrono::Utc));
    }
    // Relative form: <number><unit>, unit ∈ {d,h,m}
    // BUG-100: peel the last CHAR rather than the last BYTE so multi-byte
    // trailing units don't crash the process.
    let (num_str, unit) = split_last_char(trimmed);
    let n: i64 = num_str.parse().map_err(|_| {
        anyhow::anyhow!(
            "invalid --since value `{}` (try `2d`, `12h`, or RFC3339)",
            raw
        )
    })?;
    let now = chrono::Utc::now();
    let delta = match unit {
        "d" => chrono::Duration::days(n),
        "h" => chrono::Duration::hours(n),
        "m" => chrono::Duration::minutes(n),
        _ => anyhow::bail!("invalid --since unit `{}` — use d/h/m or RFC3339", unit),
    };
    Ok(now - delta)
}

pub(crate) fn handle_queue_progress(
    storage: &Storage,
    session: Option<&str>,
    batch: Option<&str>,
    since: Option<&str>,
    verbose: bool,
) -> Result<()> {
    let project_root = find_project_root()?;
    let store = storage.load()?;

    // Resolve the spec set. Exactly one of {session, batch, since} drives
    // the source; default is "manifest of the cwd / most-recent lease".
    enum Source {
        Manifest {
            session_id: String,
            scope: Option<String>,
            specs: Vec<String>,
        },
        Batch {
            name: String,
            specs: Vec<String>,
        },
        Since {
            cutoff: chrono::DateTime<chrono::Utc>,
            specs: Vec<String>,
        },
    }

    let source: Source = if let Some(name) = batch {
        let tag = format!("batch:{}", name);
        let specs: Vec<String> = store
            .requirements
            .iter()
            .filter(|r| r.tags.iter().any(|t| t.eq_ignore_ascii_case(&tag)))
            .map(|r| r.display_id())
            .collect();
        if specs.is_empty() {
            println!(
                "{} (no requirements tagged `batch:{}` — tag members via `aida edit --tags`)",
                "Batch progress:".bold(),
                name.cyan()
            );
            return Ok(());
        }
        Source::Batch {
            name: name.to_string(),
            specs,
        }
    } else if let Some(raw) = since {
        let cutoff = parse_since_arg(raw)?;
        let specs: Vec<String> = store
            .requirements
            .iter()
            .filter(|r| r.modified_at >= cutoff)
            .map(|r| r.display_id())
            .collect();
        if specs.is_empty() {
            println!(
                "{} (no requirements modified since {})",
                "Progress:".bold(),
                cutoff
                    .with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M %Z")
            );
            return Ok(());
        }
        Source::Since { cutoff, specs }
    } else {
        // Manifest-driven: explicit --session, else cwd-lease, else most-recent.
        let leases = list_leases(&project_root);
        let lease = if let Some(q) = session {
            Some(find_lease_by_id_prefix(q, &leases)?)
        } else {
            std::env::current_dir()
                .ok()
                .and_then(|cwd| {
                    let canon = cwd.canonicalize().unwrap_or(cwd);
                    leases
                        .iter()
                        .find(|&l| lease_covers_cwd(l, &canon))
                        .cloned()
                })
                .or_else(|| leases.last().cloned())
        };
        let Some(lease) = lease else {
            println!(
                "{} (no active sessions; pass --batch NAME or --since 2d for a non-session view)",
                "Progress:".bold()
            );
            return Ok(());
        };
        let manifest_path = session_manifest::manifest_path(&project_root, &lease.id);
        if !manifest_path.exists() {
            println!(
                "{} session {} ({}) has no planned-cluster manifest yet.",
                "Progress:".bold(),
                (&lease.id[..lease.id.len().min(8)]).yellow(),
                lease.scope.cyan(),
            );
            println!(
                "  {} {}",
                "tip:".dimmed(),
                "manifests are written by `/aida-pickup` when it confirms a multi-item cluster"
                    .dimmed()
            );
            return Ok(());
        }
        let manifest = session_manifest::load(&manifest_path)?;
        let specs: Vec<String> = manifest.items.iter().map(|it| it.spec_id.clone()).collect();
        Source::Manifest {
            session_id: lease.id.clone(),
            scope: Some(lease.scope.clone()),
            specs,
        }
    };

    // Resolve each spec_id (or agreed_id) to its live requirement.
    let (specs_in_source, header_lines): (&Vec<String>, Vec<String>) = match &source {
        Source::Manifest {
            session_id,
            scope,
            specs,
        } => {
            let short = &session_id[..session_id.len().min(8)];
            let mut lines = vec![format!(
                "{} session {}{}",
                "Progress:".bold(),
                short.yellow(),
                match scope {
                    Some(s) => format!(" · {}", s.cyan()),
                    None => String::new(),
                }
            )];
            lines.push(format!(
                "  {} {} spec{} from manifest",
                "source:".dimmed(),
                specs.len(),
                if specs.len() == 1 { "" } else { "s" }
            ));
            (specs, lines)
        }
        Source::Batch { name, specs } => {
            let mut lines = vec![format!("{} batch:{}", "Progress:".bold(), name.cyan())];
            lines.push(format!(
                "  {} {} tagged spec{}",
                "source:".dimmed(),
                specs.len(),
                if specs.len() == 1 { "" } else { "s" }
            ));
            (specs, lines)
        }
        Source::Since { cutoff, specs } => {
            let mut lines = vec![format!(
                "{} since {}",
                "Progress:".bold(),
                cutoff
                    .with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M %Z")
                    .to_string()
                    .cyan()
            )];
            lines.push(format!(
                "  {} {} modified spec{}",
                "source:".dimmed(),
                specs.len(),
                if specs.len() == 1 { "" } else { "s" }
            ));
            (specs, lines)
        }
    };

    for line in &header_lines {
        println!("{}", line);
    }
    println!();

    // Bucket each spec by its current status.
    let mut buckets: std::collections::BTreeMap<&str, Vec<(String, String)>> =
        std::collections::BTreeMap::new();
    // Insert bucket keys in display order via a Vec.
    let bucket_order = [
        ("Shipped", ProgressBucket::Shipped),
        ("In flight", ProgressBucket::InFlight),
        ("Working now", ProgressBucket::WorkingNow),
        ("Remaining", ProgressBucket::Remaining),
    ];
    for (label, _) in &bucket_order {
        buckets.insert(*label, Vec::new());
    }

    let mut unresolved: Vec<String> = Vec::new();
    let mut shelved_count = 0usize;
    for spec in specs_in_source {
        // Match by spec_id, agreed_id, or both. We accept agreed_id-form
        // ids when the manifest was written before merge-gate ran.
        let req = store.requirements.iter().find(|r| r.matches_id(spec));
        let Some(req) = req else {
            unresolved.push(spec.clone());
            continue;
        };
        // Tally shelved (NeedsAttention) specs for the drain-legibility
        // callout below — independent of bucketing. trace:STORY-490
        if status_is_shelved(&req.status) {
            shelved_count += 1;
        }
        let bucket = classify_progress_bucket(&req.status);
        let display_id = req.display_id();
        let label = bucket_order
            .iter()
            .find(|(_, b)| *b == bucket)
            .map(|(l, _)| *l)
            .unwrap_or("Remaining");
        buckets
            .get_mut(label)
            .expect("bucket initialized")
            .push((display_id, req.title.clone()));
    }

    let bucket_count = |label: &str| -> usize { buckets.get(label).map(|v| v.len()).unwrap_or(0) };

    let bucket_color = |label: &str, n: usize| -> colored::ColoredString {
        match label {
            "Shipped" => n.to_string().green().bold(),
            "In flight" => n.to_string().bright_yellow().bold(),
            "Working now" => n.to_string().cyan().bold(),
            "Remaining" => n.to_string().dimmed(),
            _ => n.to_string().normal(),
        }
    };

    for (label, _) in &bucket_order {
        let items = &buckets[label];
        if items.is_empty() {
            continue;
        }
        let suffix = match *label {
            "In flight" => " — Done, awaiting merge",
            "Working now" => " — InProgress",
            "Remaining" => " — queued",
            _ => "",
        };
        println!(
            "{} ({} item{}{})",
            label.bold(),
            bucket_color(label, items.len()),
            if items.len() == 1 { "" } else { "s" },
            suffix.dimmed()
        );
        let limit = if verbose || *label != "Remaining" {
            items.len()
        } else {
            items.len().min(8)
        };
        for (display_id, title) in items.iter().take(limit) {
            println!("  {}  {}", display_id.bold(), title);
        }
        if limit < items.len() {
            println!(
                "  {} {} more (pass --verbose to expand)",
                "…".dimmed(),
                (items.len() - limit).to_string().dimmed()
            );
        }
        println!();
    }

    // Net summary.
    let shipped = bucket_count("Shipped");
    let in_flight = bucket_count("In flight");
    let working = bucket_count("Working now");
    let remaining = bucket_count("Remaining");
    let total = shipped + in_flight + working + remaining + unresolved.len();
    let terminal = shipped; // only Completed/Rejected count as "reached terminal status"
    println!(
        "{} {} of {} reached terminal; {} in flight; {} working; {} remaining{}.",
        "Net:".bold(),
        terminal,
        total,
        in_flight,
        working,
        remaining,
        if unresolved.is_empty() {
            String::new()
        } else {
            format!(" ({} unresolved)", unresolved.len())
        }
    );

    // Drain-legibility callout: surface parked (shelved) work distinctly so a
    // parallel autonomous drain's "M shelved, needs a decision" is visible at a
    // glance. Shelved specs still count in Remaining above (STORY-332); this is
    // an additive signal, not a re-bucketing. trace:STORY-490 | ai:claude
    if shelved_count > 0 {
        println!(
            "{} {} shelved (NeedsAttention — needs a decision; triage with {})",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
            shelved_count.to_string().yellow().bold(),
            "aida findings list".cyan()
        );
    }

    if !unresolved.is_empty() && verbose {
        println!();
        println!(
            "{} (manifest references that don't resolve in the live store):",
            "Unresolved".dimmed()
        );
        for spec in &unresolved {
            println!("  {}", spec.dimmed());
        }
    }

    Ok(())
}

#[cfg(test)]
#[path = "tests/queue_progress_tests.rs"]
mod queue_progress_tests;

/// trace:BUG-225 | ai:claude
#[cfg(test)]
#[path = "tests/headless_hint_tests.rs"]
mod headless_hint_tests;

/// TASK-218: shared implementation backing both `aida queue rework SPEC`
/// and the top-level `aida rework SPEC` alias. Encapsulates the three-
/// command rework sequence (status flip + queue add + optional session
/// launch) so the implementer → reviewer → fixup loop is one verb.
/// trace:TASK-218 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_queue_rework(
    storage: &Storage,
    id: &str,
    work: bool,
    for_role: Option<&str>,
    status_override: Option<&str>,
    reason: Option<&str>,
    resume: bool,
    force: bool,
    steal: bool,
    permission_mode: Option<&str>,
    no_pull: bool,
    user: Option<&str>,
) -> Result<()> {
    // `--resume` chains through to `aida queue work --resume` (TASK-112,
    // shipped). It implies `--work` — there is nothing to resume without
    // launching a session. trace:BUG-236 | ai:claude
    let work = work || resume;

    let user_id = current_user_id(user);
    let store = storage.load()?;

    // Resolve the requirement (UUID first, then SPEC-ID).
    let req = if let Ok(uuid) = uuid::Uuid::parse_str(id) {
        store.requirements.iter().find(|r| r.id == uuid)
    } else {
        store.get_requirement_by_spec_id(id)
    }
    .ok_or_else(|| not_found::requirement_not_found(id, Some(storage.path())))?;

    let req_id = req.id;
    let spec_id = req.spec_id.clone().unwrap_or_else(|| "???".to_string());
    let display_id = req
        .agreed_id
        .clone()
        .or_else(|| req.spec_id.clone())
        .unwrap_or_else(|| "???".to_string());
    let title = req.title.clone();
    let current_status = req.status.clone();

    // Smart target-status resolution. `--status` always wins; otherwise
    // pick per the table in TASK-218's spec. See `rework_smart_target`.
    let smart_target = rework_smart_target(&current_status);
    let target_status: Option<RequirementStatus> = match status_override {
        Some(s) => Some(parse_status(s)?),
        None => smart_target,
    };

    // Guards. Terminal status (Completed/Rejected) + already-InProgress
    // both require --force. We surface the spec id in the error so the
    // user can copy the exact `--force` invocation.
    if matches!(
        current_status,
        RequirementStatus::Completed | RequirementStatus::Rejected
    ) && !force
    {
        // BUG-671: override flag on the FIRST line so agent mode (first-line
        // error summary only) surfaces it. trace:BUG-671 | ai:claude
        anyhow::bail!(
            "{} is {} — re-opening closed work is usually a mistake; pass --force to \
             override.\n  Otherwise, file a new requirement that supersedes {}.",
            display_id,
            current_status,
            display_id
        );
    }
    if matches!(current_status, RequirementStatus::InProgress) && !force {
        eprintln!(
            "  {} {} is already In Progress — re-queueing without status \
             flip. Pass `--force` to silence this warning.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
            display_id
        );
    }

    // Status flip (if any). update_atomically works for both SQLite and
    // git-canonical paths; queue done uses the same approach.
    if let Some(ref new_status) = target_status {
        if new_status != &current_status {
            let new_status = new_status.clone();
            let now = chrono::Utc::now();
            storage.update_atomically(|s| {
                if let Some(r) = s.requirements.iter_mut().find(|r| r.id == req_id) {
                    r.set_status_from_str(&format!("{:?}", new_status));
                    r.modified_at = now;
                }
            })?;
            record_role_activity(&spec_id, "rework");
            update_manifest_for_status(&spec_id, &format!("{:?}", new_status));
            println!(
                "{} {} status: {} → {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                display_id.bold(),
                current_status.to_string().dimmed(),
                new_status.to_string().cyan(),
            );
        }
    }

    // Optional audit comment. Mirrors `aida comment add` path so the
    // entry shows up in `aida show <spec>` history.
    if let Some(reason_text) = reason {
        let author = get_default_author();
        let comment = aida_core::Comment::new(author, reason_text.to_string());
        storage.update_atomically(|s| {
            if let Some(r) = s.requirements.iter_mut().find(|r| r.id == req_id) {
                r.add_comment(comment);
            }
        })?;
        println!(
            "  {} reason captured as comment ({} chars)",
            "·".dimmed(),
            reason_text.chars().count()
        );
    }

    // Route resolution: --for wins, else the active role, else error if
    // queueing requires a role (we let the queue_add path stay unrouted
    // — that's a legitimate state too, matching `aida queue add` default
    // when no role is active).
    let for_role_resolved: Option<String> = match for_role {
        Some("any") => None,
        Some(role) => Some(role.to_string()),
        None => std::env::var("AIDA_SESSION_ROLE")
            .ok()
            .filter(|s| !s.is_empty()),
    };

    // Queue add. queue_add upserts by requirement_id (replaces same-spec
    // entries), so re-running rework is idempotent on the queue side.
    let entry = aida_core::QueueEntry {
        user_id: user_id.clone(),
        requirement_id: req_id,
        position: i64::MAX, // backend resolves to existing_max + 1000
        added_by: user_id.clone(),
        note: reason.map(|r| r.to_string()),
        added_at: chrono::Utc::now(),
        for_role: for_role_resolved.clone(),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    };
    storage.queue_add(entry)?;
    record_role_activity(&spec_id, "queue-add");
    let routing = match &for_role_resolved {
        Some(r) => format!(" [for:{}]", r).cyan().to_string(),
        None => String::new(),
    };
    println!(
        "{} Queued {} ({}){}",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        display_id.bold(),
        title,
        routing
    );

    // Optional --work chain. We don't run this in a sub-process — call
    // `handle_queue_work` directly with the same storage handle so any
    // failure surfaces here. trace:TASK-218 | ai:claude
    if work {
        handle_queue_work(
            storage,
            &user_id,
            Some(&spec_id),
            permission_mode,
            /* sandbox */ false,
            /* no_launch */ false,
            for_role_resolved.as_deref(),
            no_pull,
            /* type_filter */ None,
            /* branch_override */ None,
            /* path_override */ None,
            /* stack */ false,
            /* base */ None,
            /* force_base */ false,
            steal,
            /* force_claim */ false,
            // Bare `--resume` → resume the scope's most recent recorded
            // claude session (`resolve_queue_work_launch` fails clean when
            // there is none). trace:BUG-236 | ai:claude
            if resume { Some("") } else { None },
            /* fresh */ false,
            /* list_sessions */ false,
            /* session_id */ None,
            // STORY-761: the convenience chain honors the uniform
            // `[agents] vendor` knob instead of hard-coding claude.
            &find_project_root()
                .ok()
                .and_then(|root| aida_core::agents_config::resolve_default_vendor(&root))
                .unwrap_or_else(|| "claude".to_string()),
            /* no_human */ false,
            // TASK-1060: the `queue add --work` convenience chain has no `--zen`.
            /* autonomy */
            AutonomyMode::Default,
            /* batch_name */ None,
            /* quiet */ false,
            /* allow_stale_base */ false,
            /* allow_intermediate_only */ false,
            /* complexity */ None,
            /* assist_est */ None,
            /* effort */ None,
            /* strict */ false,
            /* plan_only */ false,
            /* guided */ false,
            /* dry_run */ false,
        )?;
    } else {
        println!(
            "  ({})",
            format!(
                "run `aida queue work {}` to start a session for this spec",
                display_id
            )
            .dimmed()
        );
    }

    Ok(())
}

/// STORY-42: resolved pickup plan — which queue entries we're working,
/// in which mode, against which scope/branch, with which skill.
/// trace:STORY-42 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct QueueWorkPlan {
    /// "head" (no arg), "item" (single queued entry), or "cluster"
    /// (parent scope draining children).
    pub(crate) mode: QueueWorkMode,
    /// Pre-resolved view of each queued entry we're picking up, in
    /// queue order. Carries the display SPEC-ID and current status
    /// at plan time so manifest writing doesn't need a second store
    /// load (the second load races against `aida db sync --pull` /
    /// concurrent edits and can return a different requirements set
    /// from a sibling worktree's perspective).
    pub(crate) entries: Vec<QueueWorkEntry>,
    /// Resolved scope string for `--owns` (e.g. "EPIC-20", "PR-11",
    /// "BUG-82"). Always non-empty.
    pub(crate) scope: String,
    /// Forge + PR number when scope is a PR-N / MR-N. Lets the caller
    /// route to the review session flow without re-parsing.
    pub(crate) review_target: Option<(ReviewForge, u64)>,
    /// Display SPEC-ID of the "anchor" requirement (the first entry
    /// for head/item modes; the parent for cluster mode). Surfaced in
    /// human output so the user can confirm we resolved the right thing.
    pub(crate) anchor_display: String,
    /// Title of the anchor requirement (e.g., "Review PR-11: …" or the
    /// real spec title). Used to detect Review-PR shape for skill
    /// routing without re-querying.
    pub(crate) anchor_title: String,
}

/// STORY-42: per-entry resolved view used by manifest writing and role
/// tally. trace:STORY-42 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct QueueWorkEntry {
    /// The underlying queue entry (carries position, for_role,
    /// for_scope so the role tally and ordering work).
    pub(crate) queue: aida_core::QueueEntry,
    /// Display SPEC-ID for the manifest (agreed_id → spec_id → "?").
    pub(crate) spec_id: String,
    /// Status at plan time, formatted as the manifest field expects
    /// ("Approved", "In Progress", …).
    pub(crate) status_at_plan: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueWorkMode {
    Head,
    Item,
    Cluster,
}

/// STORY-42: resolve the user's queue-work argument into a concrete plan.
///
///   - `arg = None`            → head-pickup mode: top item from the
///     active role's queue (with the same
///     role + scope filters `queue next` uses).
///   - `arg` matches a queued  → item-pickup mode: just that entry.
///     entry's spec_id/agreed_id/uuid
///   - `arg` resolves to a req → cluster-pickup mode: every queued entry
///     that isn't queued but   whose for_scope == <id> OR whose req's
///     has queued children    derived parent EPIC == <id>.
///
/// `type_filter` only applies in cluster mode (filters drained children
/// by req type, case-insensitive). trace:STORY-42 | ai:claude
/// TASK-560: `--auto-complete` and `--resume` are mutually exclusive
/// (auto-complete drives a FRESH implementer→CI→reviewer→merge pipeline;
/// --resume continues an EXISTING session — the two can't both own the run).
/// Returns the helpful rejection message when both are set, else `None`.
/// Pure so the message (the WHY + both recovery paths) is unit-testable
/// without running the launcher. trace:TASK-560 | ai:claude
pub(crate) fn resume_autocomplete_conflict_message(
    resume: bool,
    auto_complete: bool,
) -> Option<String> {
    if resume && auto_complete {
        Some(
            "--auto-complete cannot be used with --resume.\n\
             auto-complete drives a FRESH implementer→CI→reviewer→merge→pull→build \
             pipeline; --resume continues an EXISTING session — the two can't both \
             own the run.\n\n  \
             To continue the existing session:    re-run with --resume alone.\n  \
             To start a fresh orchestrator drain:  end the existing session first \
             (`aida session end <id>`), then re-run with --auto-complete."
                .to_string(),
        )
    } else {
        None
    }
}

pub(crate) fn resolve_queue_work_plan(
    storage: &Storage,
    user_id: &str,
    arg: Option<&str>,
    type_filter: Option<&str>,
    strict: bool,
    // TASK-1053: under a dry-run, the convenience auto-queue of an explicit
    // Approved-but-unqueued spec must NOT persist — we synthesize the queue
    // entry in-memory so the plan resolves identically to a real pickup while
    // mutating nothing. trace:TASK-1053 | ai:claude
    dry_run: bool,
) -> Result<QueueWorkPlan> {
    let mut entries = storage.queue_list(user_id, /* include_completed */ false)?;
    let store = storage.load()?;

    if let Some(arg_str) = arg {
        if let Some(req) = store.requirements.iter().find(|r| spec_matches(r, arg_str)) {
            let is_queued = entries.iter().any(|e| e.requirement_id == req.id);
            if !is_queued && req.status == RequirementStatus::Approved && !strict {
                let role = std::env::var("AIDA_SESSION_ROLE")
                    .unwrap_or_else(|_| "implementer".to_string());
                let entry = aida_core::QueueEntry {
                    user_id: user_id.to_string(),
                    requirement_id: req.id,
                    position: i64::MAX,
                    added_by: user_id.to_string(),
                    note: None,
                    added_at: chrono::Utc::now(),
                    for_role: Some(role.clone()),
                    for_scope: None,
                    for_session: None,
                    added_by_machine: None,
                };
                if dry_run {
                    // TASK-1053: preview only — push the synthesized entry so the
                    // Item-pickup match below resolves the same plan a real pickup
                    // would, but persist nothing (no queue_add, no role-activity,
                    // no "queued …" line). trace:TASK-1053 | ai:claude
                    entries.push(entry);
                } else {
                    storage.queue_add(entry)?;
                    let display_id = req
                        .agreed_id
                        .as_deref()
                        .or(req.spec_id.as_deref())
                        .unwrap_or(arg_str);
                    record_role_activity(display_id, "queue-add");
                    println!("queued {} for role:{}", display_id, role);
                    // Reload entries to include the auto-queued entry.
                    // trace:TASK-547 | ai:antigravity
                    entries = storage.queue_list(user_id, /* include_completed */ false)?;
                }
            }
        }
    }

    // Head-pickup: pick the top item for the active role, honoring the
    // same filters as `queue next` (role routing + active-role scope +
    // terminal-status skip). We don't re-implement that whole filter
    // chain here; queue work is best-effort and the user can pass an
    // explicit id when the head heuristic picks the wrong thing.
    if arg.is_none() {
        let role_filter: Option<String> = std::env::var("AIDA_SESSION_ROLE")
            .ok()
            .filter(|s| !s.is_empty());
        // STORY-333: collect un-pickable specs (blocked-by / human-only)
        // skipped during head resolution so the kickoff banner can name
        // them. Silent skipping looks like "queue is empty" — we surface
        // the reason instead. trace:STORY-333 | ai:claude
        let mut skipped_unpickable: Vec<(String, String)> = Vec::new();
        let head = entries
            .iter()
            .filter(|e| match &role_filter {
                Some(r) => e.for_role.as_deref() == Some(r.as_str()),
                None => true,
            })
            .filter(|e| {
                let Some(req) = store.requirements.iter().find(|r| r.id == e.requirement_id) else {
                    return true;
                };
                !is_terminal_status(&req.status)
            })
            .filter(|e| {
                // STORY-333: pre-pickup gate. Skip un-pickable specs so the
                // orchestrator never spawns a doomed phase-1 implementer on
                // them. Reasons are recorded for the banner.
                let Some(req) = store.requirements.iter().find(|r| r.id == e.requirement_id) else {
                    return true;
                };
                match aida_core::pickability::pickability(req, &store) {
                    aida_core::pickability::Pickability::Pickable => true,
                    aida_core::pickability::Pickability::Blocked(reason) => {
                        let display = req
                            .agreed_id
                            .clone()
                            .or_else(|| req.spec_id.clone())
                            .unwrap_or_else(|| "?".to_string());
                        skipped_unpickable.push((
                            display,
                            aida_core::pickability::pickability_reason_label(&reason),
                        ));
                        false
                    }
                }
            })
            .min_by_key(|e| e.position)
            .cloned()
            .ok_or_else(|| {
                let suffix = if skipped_unpickable.is_empty() {
                    String::new()
                } else {
                    let listed = skipped_unpickable
                        .iter()
                        .map(|(s, r)| format!("{} ({})", s, r))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!(
                        "\n  Skipped {} un-pickable spec(s): {}\n  \
                         (Run `aida queue list` to see the Blocked section.)",
                        skipped_unpickable.len(),
                        listed
                    )
                };
                // BUG-465: on a FRESH project (no real, non-META specs yet) the
                // queue is legitimately empty — guide the new user's first
                // action instead of dead-ending with a bare error. EPIC-37:
                // fresh init -> first `queue work` must be boring and correct.
                // trace:BUG-465 | ai:claude
                let has_real_specs = store
                    .requirements
                    .iter()
                    .any(|r| !matches!(r.req_type, aida_core::RequirementType::Meta));
                // STORY-737 (delight #5): an empty queue on day one is the
                // EXPECTED state, not a failure. On the HUMAN path, render a
                // soft, forward-pointing signpost (info glyph — NOT a red
                // `Error:`) and return the `SoftSignpostShown` sentinel so the
                // top-level handler suppresses the error render (exit still
                // non-zero so scripts keep gating). Agent mode keeps the
                // structured error it parses. trace:STORY-737 | ai:claude
                if !agent_output_mode() {
                    if !has_real_specs && skipped_unpickable.is_empty() {
                        eprintln!(
                            "{} Your queue is empty — looks like a fresh project. Get started:\n  \
                             1. File a spec:  aida add --title \"<what you're building>\" --type task --status approved\n  \
                             2. Queue it:     aida queue add <SPEC-ID>   (the id printed by step 1)\n  \
                             3. Work it:      aida queue work\n  \
                             Browse anytime with `aida list`.",
                            crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
                        );
                    } else {
                        eprintln!(
                            "{} Nothing queued yet — that's the expected day-one state. \
                             Approve a draft and queue it in one step: \
                             `aida add \"<what you're building>\" --queue`, \
                             or queue an existing spec: `aida queue add <id>`.{}",
                            crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                            suffix
                        );
                    }
                    return anyhow::Error::new(SoftSignpostShown);
                }
                if !has_real_specs && skipped_unpickable.is_empty() {
                    anyhow::anyhow!(
                        "Your queue is empty — looks like a fresh project. Get started:\n  \
                         1. File a spec:  aida add --title \"<what you're building>\" --type task --status approved\n  \
                         2. Queue it:     aida queue add <SPEC-ID>   (the id printed by step 1)\n  \
                         3. Work it:      aida queue work\n  \
                         Browse anytime with `aida list`."
                    )
                } else {
                    anyhow::anyhow!(
                        "queue is empty for {}; pass an id explicitly or run `aida queue list`{}",
                        role_filter.as_deref().unwrap_or("any role"),
                        suffix
                    )
                }
            })?;
        if !skipped_unpickable.is_empty() {
            let listed = skipped_unpickable
                .iter()
                .map(|(s, r)| format!("{} ({})", s, r))
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!(
                "  {} skipped {} un-pickable spec(s) ahead of head: {}",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                skipped_unpickable.len(),
                listed
            );
        }
        let req = store
            .requirements
            .iter()
            .find(|r| r.id == head.requirement_id)
            .ok_or_else(|| anyhow::anyhow!("queue head's requirement is missing from the store"))?;
        let (scope, review_target) = derive_scope_from_entry(&head, req);
        let anchor_display = req
            .agreed_id
            .clone()
            .or_else(|| req.spec_id.clone())
            .unwrap_or_else(|| "?".to_string());
        let resolved = build_resolved_entry(head, req);
        return Ok(QueueWorkPlan {
            mode: QueueWorkMode::Head,
            entries: vec![resolved],
            scope,
            review_target,
            anchor_display,
            anchor_title: req.title.clone(),
        });
    }

    let arg = arg.unwrap();

    // Item-pickup: look for a queue entry whose req matches the arg by
    // uuid / spec_id / agreed_id (case-insensitive).
    let matched_item: Option<aida_core::QueueEntry> = entries
        .iter()
        .find(|e| {
            let Some(req) = store.requirements.iter().find(|r| r.id == e.requirement_id) else {
                return false;
            };
            spec_matches(req, arg)
        })
        .cloned();

    if let Some(entry) = matched_item {
        let req = store
            .requirements
            .iter()
            .find(|r| r.id == entry.requirement_id)
            .unwrap();
        let (scope, review_target) = derive_scope_from_entry(&entry, req);
        let anchor_display = req
            .agreed_id
            .clone()
            .or_else(|| req.spec_id.clone())
            .unwrap_or_else(|| arg.to_string());
        let resolved = build_resolved_entry(entry, req);
        return Ok(QueueWorkPlan {
            mode: QueueWorkMode::Item,
            entries: vec![resolved],
            scope,
            review_target,
            anchor_display,
            anchor_title: req.title.clone(),
        });
    }

    // TASK-85: PR-N / MR-N pickup by review-story title. The auto-queued
    // review stories live in the queue with titles like "Review PR-14:
    // ...", but their spec_id (e.g., STORY-103) isn't memorable. Let
    // `aida queue work PR-14` find the corresponding queue entry by
    // title prefix so the user doesn't have to look up the story id.
    // Composes with STORY-66/STORY-90 (auto-queue at PR-create) which
    // produce these stories. trace:TASK-85 | ai:claude
    if let Some((forge, n)) = parse_review_scope(arg) {
        let matches: Vec<aida_core::QueueEntry> = entries
            .iter()
            .filter(|e| {
                let Some(req) = store.requirements.iter().find(|r| r.id == e.requirement_id) else {
                    return false;
                };
                if is_terminal_status(&req.status) {
                    return false;
                }
                review_title_matches(&req.title, forge, n)
            })
            .cloned()
            .collect();
        match matches.len() {
            0 => {
                let label = format_review_label(forge, n);
                anyhow::bail!(
                    "no queued review story for {} — check `gh pr view {}` (or `glab mr view {}`) and run `aida pr auto-queue-review --branch <branch>` if needed",
                    label,
                    n,
                    n
                );
            }
            1 => {
                let entry = matches.into_iter().next().unwrap();
                let req = store
                    .requirements
                    .iter()
                    .find(|r| r.id == entry.requirement_id)
                    .unwrap();
                // PR review pickup: scope + review_target come straight from
                // the (forge, n) parse_review_scope just gave us. We don't
                // delegate to derive_scope_from_entry because its title-based
                // path is case-sensitive on `strip_prefix("Review ")` while
                // review_title_matches is case-insensitive — a lowercase
                // "review pr-14: …" title would pass the matcher but fall
                // through to the spec_id fallback, losing the PR scope and
                // the --pr N reviewer-skill route. trace:TASK-85 | ai:claude
                let scope = format_review_label(forge, n);
                let review_target = Some((forge, n));
                let anchor_display = req
                    .agreed_id
                    .clone()
                    .or_else(|| req.spec_id.clone())
                    .unwrap_or_else(|| arg.to_string());
                let resolved = build_resolved_entry(entry, req);
                return Ok(QueueWorkPlan {
                    mode: QueueWorkMode::Item,
                    entries: vec![resolved],
                    scope,
                    review_target,
                    anchor_display,
                    anchor_title: req.title.clone(),
                });
            }
            _ => {
                let label = format_review_label(forge, n);
                let mut msg = format!(
                    "{} matches {} queued review stories — pass the specific spec_id instead:",
                    label,
                    matches.len()
                );
                for e in &matches {
                    if let Some(req) = store.requirements.iter().find(|r| r.id == e.requirement_id)
                    {
                        let id = req
                            .agreed_id
                            .as_deref()
                            .or(req.spec_id.as_deref())
                            .unwrap_or("?");
                        msg.push_str(&format!("\n  · {} — {}", id, req.title));
                    }
                }
                anyhow::bail!(msg);
            }
        }
    }

    // Cluster-pickup: arg names a req that isn't queued itself but has
    // queued children. Find them via for_scope match OR derived parent
    // EPIC match. The arg must resolve to a req in the store.
    let anchor_req = store
        .requirements
        .iter()
        .find(|r| spec_matches(r, arg))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "`{}` doesn't match any queued entry or known requirement",
                arg
            )
        })?;
    let anchor_id_upper = anchor_req
        .agreed_id
        .as_deref()
        .or(anchor_req.spec_id.as_deref())
        .map(|s| s.to_uppercase())
        .unwrap_or_default();
    let type_filter_lower = type_filter.map(|s| s.to_ascii_lowercase());

    let cluster: Vec<aida_core::QueueEntry> = entries
        .iter()
        .filter(|e| {
            let Some(req) = store.requirements.iter().find(|r| r.id == e.requirement_id) else {
                return false;
            };
            // Skip terminal-status reqs (they shouldn't ever be in the
            // queue but defensive).
            if is_terminal_status(&req.status) {
                return false;
            }
            // STORY-333: cluster drains must skip un-pickable members so
            // the orchestrator never spawns phase 1 on a blocked-by /
            // human-only spec. Same gate as head pickup + batch drain.
            // trace:STORY-333 | ai:claude
            match aida_core::pickability::pickability(req, &store) {
                aida_core::pickability::Pickability::Pickable => {}
                aida_core::pickability::Pickability::Blocked(reason) => {
                    eprintln!(
                        "  {} cluster {} — skipping un-pickable {} ({})",
                        crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                        anchor_id_upper,
                        req.display_id(),
                        aida_core::pickability::pickability_reason_label(&reason),
                    );
                    return false;
                }
            }
            // Match by explicit for_scope.
            let scope_match = e
                .for_scope
                .as_deref()
                .map(|s| s.eq_ignore_ascii_case(&anchor_id_upper))
                .unwrap_or(false);
            // Match by derived parent EPIC (when anchor is itself an
            // EPIC; otherwise the function returns None so this branch
            // is harmless).
            let parent_match = derive_parent_epic_label(req, &store)
                .map(|p| p.eq_ignore_ascii_case(&anchor_id_upper))
                .unwrap_or(false);
            if !(scope_match || parent_match) {
                return false;
            }
            // Type filter (cluster only).
            if let Some(want) = &type_filter_lower {
                let actual = format!("{:?}", req.req_type).to_ascii_lowercase();
                if actual != *want {
                    return false;
                }
            }
            true
        })
        .cloned()
        .collect();

    if cluster.is_empty() {
        // TASK-217: status-aware recovery hint. The user typed an id that
        // resolves to a known spec but isn't queued (and has no queued
        // children). Branch on the anchor's current status to suggest the
        // recovery the user most likely wants — re-queueing after a
        // Done→rework cycle, promoting Planned to Approved, etc. — instead
        // of the previous opaque "no queued children match" hint.
        // trace:TASK-217 | ai:claude
        let role = std::env::var("AIDA_SESSION_ROLE")
            .ok()
            .filter(|s| !s.is_empty());
        let display_id = anchor_req
            .agreed_id
            .as_deref()
            .or(anchor_req.spec_id.as_deref())
            .unwrap_or(&anchor_id_upper);
        anyhow::bail!(format_queue_work_not_queued_error(
            display_id,
            anchor_req,
            role.as_deref(),
        ));
    }

    // Sort cluster by queue position so the manifest reflects intent.
    let mut cluster_sorted = cluster;
    cluster_sorted.sort_by_key(|e| e.position);

    // Pre-resolve each entry's display id + status so manifest writing
    // doesn't need a second store load.
    let resolved_entries: Vec<QueueWorkEntry> = cluster_sorted
        .into_iter()
        .filter_map(|entry| {
            let req = store
                .requirements
                .iter()
                .find(|r| r.id == entry.requirement_id)?;
            Some(build_resolved_entry(entry, req))
        })
        .collect();

    let scope = anchor_id_upper.clone();
    let review_target = parse_review_scope(&scope);
    Ok(QueueWorkPlan {
        mode: QueueWorkMode::Cluster,
        entries: resolved_entries,
        scope,
        review_target,
        anchor_display: anchor_id_upper,
        anchor_title: anchor_req.title.clone(),
    })
}

/// STORY-42: zip a queue entry with its current requirement state so
/// later steps (role tally, manifest write) don't need to re-look-up
/// against a potentially-different Storage handle.
/// trace:STORY-42 | ai:claude
pub(crate) fn build_resolved_entry(
    queue: aida_core::QueueEntry,
    req: &aida_core::Requirement,
) -> QueueWorkEntry {
    let spec_id = req
        .agreed_id
        .clone()
        .or_else(|| req.spec_id.clone())
        .unwrap_or_else(|| "?".to_string());
    let status_at_plan = format!("{}", req.status);
    QueueWorkEntry {
        queue,
        spec_id,
        status_at_plan,
    }
}

/// TASK-217: build a status-aware recovery hint when `aida queue work <id>`
/// resolves a spec but finds no queue entry. The current status of the
/// resolved spec tells us which recovery path the user most likely wants.
/// trace:TASK-217 | ai:claude
pub(crate) fn format_queue_work_not_queued_error(
    display_id: &str,
    anchor_req: &aida_core::Requirement,
    role: Option<&str>,
) -> String {
    use RequirementType::*;
    let role_display = role.unwrap_or("<role>");
    let is_container = matches!(anchor_req.req_type, Epic | Folder | Sprint);
    if is_container {
        return format!(
            "`{display_id}` has no queued children. (Status: {status})\n  \
             Inspect the cluster: `aida queue list --tree` or `aida list --parent {display_id}`\n  \
             To queue more work under {display_id}: `aida queue add <child-id> --for {role_display}`",
            status = anchor_req.status,
        );
    }
    match anchor_req.status {
        RequirementStatus::Draft | RequirementStatus::Approved => format!(
            "`{display_id}` isn't queued. Status is {status}.\n  \
             To queue and start: `aida queue add {display_id} --for {role_display}` then `aida queue work {display_id}`",
            status = anchor_req.status,
        ),
        RequirementStatus::Planned => format!(
            "`{display_id}` isn't queued. Status is Planned (still in planning).\n  \
             To work it: `aida edit {display_id} --status approved` then `aida queue add {display_id} --for {role_display}`"
        ),
        RequirementStatus::InProgress => format!(
            "`{display_id}` isn't queued, but status is In Progress.\n  \
             The lease may have been lost. Inspect with `aida queue list --all`.\n  \
             To re-queue: `aida queue add {display_id} --for {role_display}`"
        ),
        // The suggested command must run verbatim for *any* Done spec —
        // including one with no recorded claude session — so it stays
        // `--work` (a fresh session), not `--work --resume` (which needs a
        // prior session and bounces when there isn't one). trace:BUG-236
        // TASK-240: a Done spec usually means "PR open, awaiting merge". If the
        // user authored that PR they may want to merge it NOW rather than wait —
        // point them at the merge path. We can't name the PR number here (no gh
        // lookup in this pure builder), so route via `aida show` which prints the
        // PR linkage, keeping every suggested command honest/runnable.
        // No --delete-branch in the merge suggestion — a worktree may still
        // hold the branch — and `;` so the auto-bump pull cannot be dropped.
        // trace:TASK-240 trace:BUG-758 | ai:claude
        RequirementStatus::Done => format!(
            "`{display_id}` isn't queued. Status is Done (work finished on a branch).\n  \
             If review found issues and more commits are needed: `aida queue rework {display_id} --work`\n  \
             If the PR is yours and you want to merge now (CI green): find it with `aida show {display_id}`, then `gh pr merge <PR> --squash; aida pull`.\n  \
             Otherwise nothing to do — auto-bump fires when the PR merges."
        ),
        RequirementStatus::Completed => format!(
            "`{display_id}` is Completed (already shipped). Nothing to work on.\n  \
             To re-open: `aida edit {display_id} --status in-progress --force`"
        ),
        RequirementStatus::Rejected => format!(
            "`{display_id}` is Rejected. Pick a different spec, or re-open with `aida edit {display_id} --status approved --force`."
        ),
        // STORY-332: a punted spec is paused awaiting triage — it should be
        // resolved by a human/advisor, not silently re-queued.
        RequirementStatus::NeedsAttention => format!(
            "`{display_id}` is paused (Needs Attention) — an agent punted a design-fork it couldn't resolve.\n  \
             Review the reason with `aida show {display_id}`, then triage:\n  \
             resume with `aida edit {display_id} --status in-progress`, or drop it with `--status rejected`."
        ),
    }
}

/// TASK-630: the held-state re-entry decision, isolated so it is unit-testable
/// without a Storage handle, a worktree, or a launcher.
///
/// A deliberate *push-branch, hold-PR* finish (BUG-250) leaves the spec **Done**
/// + dequeued with a persisted hold marker at `.aida/pr-holds/<spec>.json`. The
///   plain `aida queue work <spec>` recovery hints (rework / wait-for-merge) don't
///   fit that state — the operator is neither reworking nor waiting for a merge;
///   they're re-entering the dormant implementer worktree to run the manual gate
///   and *then* open the deferred PR. `--resume` against such a spec should flow
///   through the verb's normal lease/pull/worktree bookkeeping instead of bailing.
///
/// The decision is deliberately narrow: re-entry is allowed **only** when the
/// operator explicitly asked to resume (`resume == true`), the spec is in the
/// Done state (the state a deliberate hold parks it in), AND a hold marker is
/// present. A queued / Completed / NeedsAttention spec, or a Done spec with no
/// hold marker, is left to the existing recovery hints — a held re-entry is a
/// distinct state, not a blanket "resume any Done spec". trace:TASK-630 | ai:claude
pub(crate) fn held_resume_reentry_allowed(
    resume: bool,
    status: &RequirementStatus,
    hold_marker_present: bool,
) -> bool {
    resume && hold_marker_present && *status == RequirementStatus::Done
}

/// TASK-630: build the Item-mode plan for a held-spec `--resume` re-entry.
///
/// A held spec is Done + dequeued, so there is no real `QueueEntry` to anchor
/// the plan on. We synthesise one (scoped to the spec's own id, matching the
/// implementer worktree the held session lives in) so the rest of
/// `handle_queue_work` — lease bookkeeping, worktree resolution, the
/// `QueueWorkLaunch::Resume` re-entry — runs unchanged. trace:TASK-630 | ai:claude
pub(crate) fn held_resume_plan(req: &aida_core::Requirement, user_id: &str) -> QueueWorkPlan {
    let synthetic = aida_core::QueueEntry {
        user_id: user_id.to_string(),
        requirement_id: req.id,
        position: 0,
        added_by: user_id.to_string(),
        note: None,
        added_at: chrono::Utc::now(),
        for_role: Some("implementer".to_string()),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    };
    let (scope, review_target) = derive_scope_from_entry(&synthetic, req);
    let anchor_display = req
        .agreed_id
        .clone()
        .or_else(|| req.spec_id.clone())
        .unwrap_or_else(|| scope.clone());
    let resolved = build_resolved_entry(synthetic, req);
    QueueWorkPlan {
        mode: QueueWorkMode::Item,
        entries: vec![resolved],
        scope,
        review_target,
        anchor_display,
        anchor_title: req.title.clone(),
    }
}

/// STORY-42: case-insensitive match against a requirement's uuid,
/// spec_id, or agreed_id. trace:STORY-42 | ai:claude
pub(crate) fn spec_matches(req: &aida_core::Requirement, query: &str) -> bool {
    if let Ok(uuid) = uuid::Uuid::parse_str(query) {
        return req.id == uuid;
    }
    let q = query.to_ascii_uppercase();
    if let Some(s) = &req.spec_id {
        if s.to_ascii_uppercase() == q {
            return true;
        }
    }
    if let Some(a) = &req.agreed_id {
        if a.to_ascii_uppercase() == q {
            return true;
        }
    }
    false
}

/// STORY-42: pick the scope string for a single queued entry.
/// Preference order:
///   1. PR-N / MR-N parsed from "Review PR-N: …" title → "PR-N"
///   2. entry.for_scope (explicit STORY-57 tag)
///   3. fall back to the req's own display id
///      Also returns the parsed review target (if any) so the caller can
///      thread it into session_start without re-parsing.
///      trace:STORY-42 | ai:claude
// BUG-431 #1: a child story no longer inherits its parent epic's scope (the
// removed step 3). Session scope is derived purely from the entry's
// `for_scope`, the review-title shape, and the req's own id — so same-epic
// siblings drained together each get an independent scope / worktree / branch
// instead of contending for one epic scope. The store param is gone with the
// parent lookup. trace:BUG-431
pub(crate) fn derive_scope_from_entry(
    entry: &aida_core::QueueEntry,
    req: &aida_core::Requirement,
) -> (String, Option<(ReviewForge, u64)>) {
    // Review-PR shape — title carries the PR ref; e.g. "Review PR-11: …".
    if let Some(rest) = req.title.strip_prefix("Review ") {
        let pr_token = rest.split([':', ' ']).next().unwrap_or("");
        if let Some(target) = parse_review_scope(pr_token) {
            return (pr_token.to_uppercase(), Some(target));
        }
    }
    if let Some(s) = &entry.for_scope {
        // BUG-739: `harness-worktree` is a generic Claude harness lease, not a
        // work scope. Older queue entries may have been auto-stamped with it;
        // ignore that legacy value so `queue work` falls back to the spec id
        // instead of trying to start inside the shared harness checkout.
        // trace:BUG-739 | ai:codex
        if s.eq_ignore_ascii_case(worktree_lease::HARNESS_WORKTREE_SCOPE) {
            return derive_scope_from_req_id(req);
        }
        let target = parse_review_scope(s);
        return (s.to_uppercase(), target);
    }
    derive_scope_from_req_id(req)
}

pub(crate) fn derive_scope_from_req_id(
    req: &aida_core::Requirement,
) -> (String, Option<(ReviewForge, u64)>) {
    // BUG-431 #1: a child story's session scopes to the STORY'S OWN id, not
    // its parent epic. Previously this fell back to `derive_parent_epic_label`,
    // so every same-epic story under one drain (`aida queue work next N`) tried
    // to own the SAME epic scope — the worktree (`proj-epic-11`) and branch
    // (`epic-11`) collided, the first story's lease blocked all siblings at
    // phase 1 ("scope EPIC-11 is owned by lease …"), and the epic-leading PR
    // title backed two specs (epic + child), breaking the headless reviewer.
    // Scoping to the story gives each sibling its own scope / worktree / branch
    // so they drain without contention. An operator who genuinely wants an
    // epic-wide session still gets it via an explicit `for_scope` (above) or
    // `aida queue work EPIC-N`. The parent-epic label is still used for
    // *display* clustering elsewhere (planned-cluster manifest, queue
    // grouping) — that is separate from the lease scope and unaffected.
    // trace:BUG-431 | ai:claude
    let fallback = req
        .agreed_id
        .clone()
        .or_else(|| req.spec_id.clone())
        .unwrap_or_else(|| "scope".to_string());
    let target = parse_review_scope(&fallback);
    (fallback, target)
}

pub(crate) fn queue_add_for_scope_routing(
    no_scope: bool,
    explicit_scope: Option<&str>,
    for_session: Option<&str>,
    active_lease: Option<&SessionLease>,
) -> Option<String> {
    if no_scope {
        None
    } else if let Some(s) = explicit_scope {
        Some(s.to_string())
    } else if for_session.is_some() {
        // --for-session is the more specific axis; don't also auto-add a scope
        // filter unless the user asked for it.
        None
    } else {
        // BUG-739: the generic harness-worktree lease only means "this shell is
        // in a shared Claude harness checkout"; it must not become queue
        // routing state. Explicit `--scope harness-worktree` remains honored by
        // the explicit_scope branch above.
        // trace:BUG-739 | ai:codex
        active_lease
            .filter(|l| {
                !l.scope
                    .eq_ignore_ascii_case(worktree_lease::HARNESS_WORKTREE_SCOPE)
            })
            .map(|l| l.scope.clone())
    }
}

/// STORY-42: pick the role for a resolved queue-work plan.
/// trace:STORY-42 | ai:claude
pub(crate) fn infer_queue_work_role(
    plan: &QueueWorkPlan,
    override_role: Option<&str>,
) -> (String, &'static str, Option<Vec<String>>) {
    if let Some(r) = override_role {
        return (r.to_string(), "--role flag", None);
    }
    // Tally for_role across entries. None counts as "unrouted".
    let mut tally: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for e in &plan.entries {
        if let Some(r) = &e.queue.for_role {
            *tally.entry(r.to_ascii_lowercase()).or_default() += 1;
        }
    }
    let total_routed: usize = tally.values().sum();
    let mut warnings: Vec<String> = Vec::new();
    let chosen = if let Some((best, count)) = tally
        .iter()
        .max_by_key(|(_, c)| **c)
        .map(|(k, c)| (k.clone(), *c))
    {
        if count < total_routed {
            warnings.push(format!(
                "cluster has {} item{} routed to other role(s); using majority `{}` ({}/{})",
                total_routed - count,
                if total_routed - count == 1 { "" } else { "s" },
                best,
                count,
                total_routed
            ));
        }
        (best, "cluster-derived")
    } else {
        // No entry has for_role set — fall through to scope default
        // (PR-N → reviewer; else implementer) then env.
        let scope_default = if plan.review_target.is_some() {
            "reviewer"
        } else {
            "implementer"
        };
        let env_role = std::env::var("AIDA_SESSION_ROLE")
            .ok()
            .filter(|s| !s.is_empty());
        match env_role {
            Some(r) if !r.eq_ignore_ascii_case(scope_default) => {
                warnings.push(format!(
                    "no for_role on queued items; scope default `{}` overrides shell role `{}` (pass --role {} to keep shell role)",
                    scope_default, r, r
                ));
                (scope_default.to_string(), "scope-default")
            }
            _ => (scope_default.to_string(), "scope-default"),
        }
    };
    let warns = if warnings.is_empty() {
        None
    } else {
        Some(warnings)
    };
    (chosen.0, chosen.1, warns)
}

/// STORY-42: build the initial-prompt string fed to `claude <prompt>`.
/// Routes by role:
///   - reviewer → `/aida-review --pr N` (when scope parses as PR-N/MR-N)
///     else `/aida-review`
///   - implementer / other → `/aida-pickup [<ITEM-ID> | --auto-first]`
///     Both forms skip the skill's Step 2 confirm — the
///     argument IS the consent signal. Item mode focuses a
///     single id (the operator typed it, so the SPEC-ID is
///     the commitment — TASK-548 generalised TASK-86's
///     argument-as-consent rule to cover this case).
///     Cluster / head mode passes `--auto-first` to ride the
///     queue-work pre-flight summary as its consent point.
///     Only a bare `/aida-pickup` from the conversation (no
///     argument) still pauses to confirm.
///     trace:TASK-86 trace:TASK-548 | ai:claude
///     trace:STORY-42 | ai:claude
pub(crate) fn derive_queue_work_prompt(
    plan: &QueueWorkPlan,
    role: &str,
    plan_only: bool,
    guided: bool,
) -> String {
    let role_lower = role.to_ascii_lowercase();
    if role_lower == "reviewer" {
        if let Some((_, n)) = plan.review_target {
            return format!("/aida-review --pr {}", n);
        }
        return "/aida-review".to_string();
    }
    // STORY-735: guided keystone mode runs the structured decision-dialog
    // skill instead of pickup. It is interactive-only and conflicts with the
    // autonomous drain at the CLI; the spec id is the anchor it dialogs over.
    // trace:STORY-735 | ai:claude
    if guided {
        return format!("/aida-guided-implement {}", plan.anchor_display);
    }
    // STORY-265: plan-only mode runs the planning skill instead of pickup —
    // produce a docs/plans/ file, no code. (Reviewer handled above; plan-only
    // is an implementer-side mode.)
    if plan_only {
        if plan.mode == QueueWorkMode::Item {
            return format!("/aida-plan {}", plan.anchor_display);
        }
        return "/aida-plan".to_string();
    }
    // Implementer (and unknown roles): /aida-pickup with optional focus.
    if plan.mode == QueueWorkMode::Item {
        return format!("/aida-pickup {}", plan.anchor_display);
    }
    // Cluster / Head: drain-intent already confirmed by the queue work
    // pre-flight; pass --auto-first so the skill skips its own confirm.
    "/aida-pickup --auto-first".to_string()
}

/// BUG-225: render the copy-pasteable `claude` command line for a
/// headless launch deferred by `--no-launch`. Built from
/// `session::claude_headless_args` — the exact argv `exec_claude_headless`
/// feeds to `claude` — so the printed hint can never drift from the real
/// launch (correct flag order, `--session-id` included, prompt last).
/// STORY-278: prefix `AIDA_HEADLESS=1` so the copy-paste hint also sets
/// the env var `exec_claude_headless` puts on the child via `.env(...)`.
/// trace:BUG-225 | ai:claude
pub(crate) fn headless_launch_hint(prompt: &str, session_id: &str, contained: bool) -> String {
    let argv = session::claude_headless_args_with_posture(prompt, session_id, contained);
    format!("AIDA_HEADLESS=1 claude {}", shell_join_display(&argv))
}

pub(crate) fn claude_posture_display(permission_mode: Option<&str>, contained: bool) -> String {
    if contained {
        "contained sandbox".to_string()
    } else {
        permission_mode
            .map(|m| format!("permission-mode {}", m))
            .unwrap_or_else(|| "native permission posture".to_string())
    }
}

/// STORY-42: the orchestrator. Resolves the plan, optionally pulls,
/// runs session_start in non-launch mode (so we can write the manifest
/// from the freshly minted lease), then either prints next-steps or
/// chdirs + execs claude with the skill prompt.
/// trace:STORY-42 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_queue_work(
    storage: &Storage,
    user_id: &str,
    arg: Option<&str>,
    permission_mode: Option<&str>,
    sandbox: bool,
    no_launch: bool,
    role_override: Option<&str>,
    no_pull: bool,
    type_filter: Option<&str>,
    branch_override: Option<&str>,
    path_override: Option<&str>,
    // STORY-248: stacked-branch base resolution. `stack` auto-detects the
    // freshest un-merged in-flight implementer lease; `base` takes an
    // explicit branch name. CLI clap-level enforces they're mutually
    // exclusive. `force_base` opts out of the merged-base safety check.
    // trace:STORY-248 | ai:claude
    stack: bool,
    base: Option<&str>,
    force_base: bool,
    steal: bool,
    force_claim: bool,
    resume: Option<&str>,
    fresh: bool,
    list_sessions: bool,
    session_id: Option<&str>,
    // TASK-895: which vendor CLI hosts the interactive session — `claude`
    // (default) or `codex`. The AIDA TUI passes `codex` to host a Codex tab.
    // Only affects the interactive (non-headless) launch path; the headless
    // drain resolves its own vendor via STORY-683. trace:TASK-895 | ai:claude
    vendor: &str,
    no_human: bool,
    // TASK-1060 / ADR-10: the autonomy mode, resolved ONCE at dispatch and
    // threaded in as a typed value so in-process logic (the pre-flight banner)
    // never re-reads the bare `AIDA_ZEN` env var. trace:TASK-1060 | ai:claude
    autonomy: AutonomyMode,
    // TASK-272: the batch this pickup belongs to, when resolved from
    // `aida queue work --batch NAME`. Recorded on the session manifest so
    // /aida-pickup can detect batch context. `None` for a plain pickup.
    batch_name: Option<&str>,
    // BUG-226: suppress the end-of-command summary for a standalone
    // reviewer run (`--quiet`). Ignored for non-reviewer / orchestrator
    // launches, which never print one.
    quiet: bool,
    // STORY-281: opt out of the reviewer pre-flight stale-base refusal.
    // Only meaningful when scope resolves to a PR + role is reviewer;
    // ignored on every other pickup. trace:STORY-281 | ai:claude
    allow_stale_base: bool,
    // TASK-480: opt out of the reviewer pre-flight intermediate-only
    // refusal. Same scoping as allow_stale_base — only meaningful for a
    // reviewer pickup of a GitHub PR. trace:TASK-480 | ai:claude
    allow_intermediate_only: bool,
    // STORY-439: pickup-time complexity + assistance estimate. Each
    // value writes a slot to `.aida/complexity-calibration/<SPEC>.yaml`
    // AND stamps a `complexity:<level>` / `estimated-assistance:<level>`
    // tag on the spec for tag-based queries. trace:STORY-439 | ai:claude
    complexity: Option<complexity_calibration::ComplexityLevel>,
    assist_est: Option<complexity_calibration::AssistanceLevel>,
    // STORY-451: post-plan/pickup effort estimate. Captured as the plan
    // touchpoint and stamped as `effort:plan:<bucket>`.
    effort: Option<effort_calibration::EffortBucket>,
    // trace:TASK-547 | ai:antigravity
    strict: bool,
    // STORY-265: plan-only mode — run /aida-plan (not /aida-pickup) in `plan`
    // permission mode so the session writes a docs/plans/ file without
    // touching code; promote Approved -> Planned afterward via
    // `aida plan promote`. trace:STORY-265 | ai:claude
    plan_only: bool,
    // STORY-735: guided keystone-implementation mode — launch
    // /aida-guided-implement (a structured decision dialog) instead of
    // /aida-pickup. Interactive-only; the spec's major architectural forks
    // are decided up front and recorded as ADRs before any code.
    // trace:STORY-735 | ai:claude
    guided: bool,
    // TASK-1053: single-spec preview. Resolve the full plan (scope, role,
    // skill, branch, worktree path, session id, lease) and print it, then
    // return WITHOUT creating a worktree, taking a lease, or launching a
    // session. The `--batch` form is handled at the dispatch site (it
    // returns there before reaching this function). trace:TASK-1053 | ai:claude
    dry_run: bool,
) -> Result<()> {
    // STORY-132: validate a caller-minted --session-id up front — before
    // any side effect — so a malformed id fails clean with a clear
    // message rather than surfacing deep in session setup.
    if let Some(sid) = session_id {
        uuid::Uuid::parse_str(sid)
            .with_context(|| format!("--session-id `{}` is not a valid UUID", sid))?;
    }
    // TASK-630: a deliberate PR-hold (BUG-250) parks the spec Done + dequeued
    // with a persisted marker at `.aida/pr-holds/<spec>.json`. Normal queue
    // resolution then bails ("isn't queued. Status is Done") — wrong for a
    // re-entry. When the operator explicitly `--resume`s such a spec, recognise
    // the hold as a re-enterable state and synthesise an Item-mode plan so the
    // rest of the verb (lease/pull/worktree + QueueWorkLaunch::Resume) runs
    // unchanged. We try the normal resolution first; only on its failure do we
    // consult the marker, so a still-queued or non-held spec keeps its existing
    // behaviour exactly. trace:TASK-630 | ai:claude
    let plan = match resolve_queue_work_plan(storage, user_id, arg, type_filter, strict, dry_run) {
        Ok(plan) => plan,
        Err(e) => match arg.filter(|_| resume.is_some()) {
            Some(arg_str) => {
                let store = storage.load()?;
                let held = store
                    .requirements
                    .iter()
                    .find(|r| spec_matches(r, arg_str))
                    .and_then(|req| {
                        let main_root = find_main_worktree_root().ok()?;
                        let display = req
                            .agreed_id
                            .as_deref()
                            .or(req.spec_id.as_deref())
                            .unwrap_or(arg_str);
                        let marker = punt::hold_signal_path(&main_root, display);
                        let present = punt::read_hold_signal(&marker).is_some();
                        held_resume_reentry_allowed(true, &req.status, present)
                            .then(|| held_resume_plan(req, user_id))
                    });
                match held {
                    Some(plan) => plan,
                    None => return Err(e),
                }
            }
            None => return Err(e),
        },
    };

    // TASK-304: on a no-arg head pickup, surface the ultraplan suggestion
    // for a chunky head spec under `[ultraplan] mode = "suggested"`. Only
    // the no-arg form (the operator hasn't already chosen a spec) gets the
    // nudge; an explicit `aida queue work SPEC` is already a deliberate
    // choice. Best-effort and read-only — never perturbs the pickup.
    // trace:TASK-304 | ai:claude
    if arg.is_none() && !list_sessions {
        if let Ok(root) = find_project_root() {
            if let Ok(store) = storage.load() {
                if let Some(req) = store.get_requirement_by_spec_id(&plan.anchor_display) {
                    print_ultraplan_suggestion_hint(&root, req);
                }
            }
        }
    }

    // TASK-516: warn before picking up a spec whose imported plan is still
    // awaiting master review (tagged `plan-review:pending`). The plan is
    // archived but NOT yet canonical — the master hasn't signed off, so
    // riding it into the implementer session as the brief would treat an
    // unreviewed plan as load-bearing. Best-effort, read-only; never blocks
    // the pickup (this minimal slice warns, it doesn't gate).
    // trace:TASK-516 | ai:claude
    if !list_sessions {
        if let Ok(store) = storage.load() {
            if let Some(req) = store.get_requirement_by_spec_id(&plan.anchor_display) {
                if let Some(msg) = plan_review_warning(&req.tags, &plan.anchor_display) {
                    eprintln!("{msg}");
                }
            }
        }
    }

    // STORY-439: capture pickup-time complexity + assistance estimate
    // ASAP after plan resolution — we know the anchor spec, the project
    // root is reachable via `find_project_root`, and the capture is a
    // local FS write that won't perturb the rest of the pickup flow.
    // Best-effort: a write error logs and continues. Cluster mode uses
    // the anchor (the parent scope) as the captured spec; the
    // operator's estimate is for the cluster as a whole.
    // trace:STORY-439 | ai:claude
    if (complexity.is_some() || assist_est.is_some()) && !list_sessions && !dry_run {
        if let Ok(project_root) = find_project_root() {
            let main_root = main_worktree_root_from(&project_root);
            let spec = plan.anchor_display.as_str();
            if let Err(e) =
                complexity_calibration::upsert_pickup(&main_root, spec, complexity, assist_est)
            {
                eprintln!(
                    "  {} could not record pickup calibration for {spec}: {e}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
            }
            // Stamp the tags on the spec so existing tag tooling works
            // on the new dimension. Best-effort — a load/save failure
            // doesn't fail the pickup.
            apply_calibration_tags(storage, spec, complexity, assist_est);
        }
    }
    if effort.is_some() && !list_sessions && !dry_run {
        if let Ok(project_root) = find_project_root() {
            let main_root = main_worktree_root_from(&project_root);
            let spec = plan.anchor_display.as_str();
            if let Err(e) = effort_calibration::upsert_plan(
                &main_root,
                spec,
                effort,
                Some(current_user_id(None)),
            ) {
                eprintln!(
                    "  {} could not record plan effort for {spec}: {e}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
            }
            apply_effort_tag(
                storage,
                spec,
                effort_calibration::EffortTouchpoint::Plan,
                effort,
            );
        }
    }

    // TASK-112: `--list-sessions` is a pure read — print the recorded
    // claude conversations for this scope and exit before any side
    // effects (no worktree, no lease, no pull).
    if list_sessions {
        return print_scope_sessions(&plan.anchor_display);
    }

    // TASK-112: decide cold-launch-vs-resume up front, *before*
    // session_start mints a worktree — so a bad `--resume <id>` (or a
    // bare `--resume` with no prior session) fails clean with nothing to
    // unwind. `None` when `--no-launch` (setup-only, no conversation).
    let launch: Option<QueueWorkLaunch> = if no_launch {
        None
    } else {
        Some(resolve_queue_work_launch(
            &plan.anchor_display,
            resume,
            fresh,
            session_id,
        )?)
    };

    let (role, role_origin, warnings) = infer_queue_work_role(&plan, role_override);

    // Permission mode resolution (TASK-83 → TASK-84). Order:
    //   1. --permission-mode flag (explicit override always wins)
    //   2. AIDA_PERMISSION_MODE env var (process-level opt-out)
    //   3. .aida/config.toml [behavior] permission_mode (project policy)
    //   4. AIDA-managed worktree default → `bypassPermissions` (TASK-84:
    //      worktree is sandboxed git state; the prompt flood was eating
    //      autonomous overnight runs and circuit breakers stay in place)
    //   5. Non-AIDA default → `acceptEdits` (safe default for foreign cwd)
    // The pass-through to `claude --permission-mode` is unvalidated, so
    // users can pick `auto`, `plan`, etc. trace:STORY-42, TASK-83, TASK-84
    // | ai:claude
    let env_mode = std::env::var("AIDA_PERMISSION_MODE")
        .ok()
        .filter(|s| !s.is_empty());
    let project_root_for_config = find_main_worktree_root().ok();
    let config_mode = project_root_for_config
        .as_deref()
        .and_then(read_behavior_permission_mode);
    // STORY-495: the interactive worktree default is now faithful (native).
    // The uniform `[agents] bypass` knob is the single opt-in that restores
    // bypass posture — read it from user-base + project agents.toml.
    let bypass_knob = project_root_for_config
        .as_deref()
        .map(|r| load_agents_bypass(r).unwrap_or(false))
        .unwrap_or(false);
    let contained_knob = project_root_for_config
        .as_deref()
        .map(|r| load_agents_contained(r).unwrap_or(false))
        .unwrap_or(false);
    let contained_env = std::env::var("AIDA_CONTAINED")
        .ok()
        .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes"));
    if sandbox && permission_mode.is_some() {
        anyhow::bail!(
            "--sandbox and --permission-mode are mutually exclusive Claude launch postures"
        );
    }
    let contained = !plan_only && (sandbox || contained_env || contained_knob);
    if contained && bypass_knob {
        anyhow::bail!("[agents] bypass and contained mode are mutually exclusive launch postures");
    }
    if contained {
        std::env::set_var("AIDA_CONTAINED", "1");
    }
    let (permission_mode, permission_mode_origin) = if contained {
        (Some("dontAsk".to_string()), "contained sandbox")
    } else {
        resolve_queue_work_permission_mode(
            permission_mode,
            env_mode.as_deref(),
            config_mode.as_deref(),
            bypass_knob,
            plan_only,
        )
    };
    if permission_mode.is_none() && !contained {
        maybe_show_faithful_launcher_notice();
    }

    let prompt = derive_queue_work_prompt(&plan, &role, plan_only, guided);

    // STORY-281: reviewer pre-flight stale-base check. Fires only when
    // the resolved scope is a GitHub PR AND the inferred role is the
    // reviewer — every other pickup (implementer, dialog, architect,
    // GitLab MR) skips this branch. The check is also no-op'd when the
    // pickup is `--no-launch` (no reviewer session about to run) and
    // when the user passed `--list-sessions` (already exited above).
    //
    // Behaviour mirrors the orchestrator's phase-3 pre-flight:
    //   Current        → silent proceed
    //   StaleNoOverlap → warning, proceed
    //   StaleOverlap   → refuse (anyhow::bail!) unless allow_stale_base
    //   Err (gh / fetch) → warning, proceed (never block on transient infra)
    //
    // trace:STORY-281 | ai:claude
    if !no_launch && role == "reviewer" {
        if let Some((ReviewForge::GitHub, pr_n)) = plan.review_target {
            if let Some(root) = project_root_for_config.as_deref() {
                match preflight_stale_base_check(root, pr_n) {
                    Ok(pr_rebase::StaleBaseOutcome::Current) => {}
                    Ok(pr_rebase::StaleBaseOutcome::StaleNoOverlap { behind }) => {
                        eprintln!(
                            "  {} {}",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                            pr_rebase::stale_base_warn_message(pr_n, behind).yellow()
                        );
                    }
                    Ok(pr_rebase::StaleBaseOutcome::StaleOverlap {
                        behind,
                        overlap_files,
                        ..
                    }) => {
                        let msg = pr_rebase::stale_base_block_message(pr_n, behind, &overlap_files);
                        if allow_stale_base {
                            eprintln!(
                                "  {} stale-base + overlap detected; \
                                 `--allow-stale-base` is set, proceeding.\n{}",
                                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                                msg.yellow()
                            );
                        } else {
                            anyhow::bail!(msg);
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "  {} pre-flight stale-base check for PR-{pr_n} failed \
                             ({e}); proceeding with reviewer",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                        );
                    }
                }
            }
        }
    }

    // TASK-480: reviewer pre-flight intermediate-only check. Sibling
    // substrate-as-bouncer gate to the STORY-281 stale-base refusal
    // above — same scoping (reviewer + GitHub PR + launching). Refuses a
    // PR whose diff is exclusively intermediate/generated paths (build
    // outputs, gitignored files, lockfiles with no source change)
    // because such a fix is not reproducible. The check is a
    // PROGRAMMATIC GATE here, not skill-template instruction text
    // (BUG-280-class lesson). Fails open on infra error.
    //
    //   Clean                   → silent proceed
    //   SourcePlusIntermediate  → warning, proceed (flag-but-allow)
    //   IntermediateOnly        → refuse unless --allow-intermediate-only
    //
    // trace:TASK-480 | ai:claude
    if !no_launch && role == "reviewer" {
        if let Some((ReviewForge::GitHub, pr_n)) = plan.review_target {
            if let Some(root) = project_root_for_config.as_deref() {
                let allow = allow_intermediate_only
                    || std::env::var("AIDA_ALLOW_INTERMEDIATE_ONLY")
                        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                        .unwrap_or(false);
                match preflight_intermediate_only_check(root, pr_n) {
                    Ok(pr_rebase::IntermediateOnlyOutcome::Clean) => {}
                    Ok(pr_rebase::IntermediateOnlyOutcome::SourcePlusIntermediate {
                        intermediate,
                    }) => {
                        eprintln!(
                            "  {} {}",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                            pr_rebase::intermediate_only_warn_message(pr_n, &intermediate).yellow()
                        );
                    }
                    Ok(pr_rebase::IntermediateOnlyOutcome::IntermediateOnly { intermediate }) => {
                        let msg = pr_rebase::intermediate_only_block_message(pr_n, &intermediate);
                        if allow {
                            eprintln!(
                                "  {} intermediate-only diff detected; \
                                 `--allow-intermediate-only` is set, proceeding.\n{}",
                                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                                msg.yellow()
                            );
                        } else {
                            anyhow::bail!(msg);
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "  {} pre-flight intermediate-only check for PR-{pr_n} failed \
                             ({e}); proceeding with reviewer",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                        );
                    }
                }
            }
        }
    }

    // Pre-flight summary so the user sees what we resolved before any
    // side effects fire (worktree create, lease, exec). Goes to stderr.
    eprintln!();
    eprintln!(
        "{} queue work {} mode",
        crate::glyph(crate::glyphs::Glyph::FlowActive)
            .green()
            .bold(),
        format!("({:?})", plan.mode).to_lowercase().cyan()
    );
    let line = |label: &str, value: String| {
        // Pad label to 8 chars so anchor/scope/role/mode/skill/cluster
        // line up vertically. The bold() call doesn't affect width as
        // seen by humans (terminal renders the same column).
        eprintln!("  {:<8} {}", format!("{}:", label).bold(), value);
    };
    line(
        "anchor",
        format!(
            "{}  {}",
            plan.anchor_display.cyan(),
            plan.anchor_title.dimmed()
        ),
    );
    line("scope", plan.scope.cyan().to_string());
    line(
        "role",
        format!("{} {}", role.cyan(), format!("({})", role_origin).dimmed()),
    );
    line(
        "mode",
        format!(
            "{} {}",
            permission_mode.as_deref().unwrap_or("native").cyan(),
            format!("({})", permission_mode_origin).dimmed()
        ),
    );
    line("skill", prompt.cyan().to_string());
    // STORY-287: surface the `--zen` autonomy mode in the pre-flight so the
    // user sees the flag took. ADR-10 / TASK-1060: consult the typed `autonomy`
    // value (resolved ONCE at dispatch and threaded in) instead of re-reading
    // the bare `AIDA_ZEN` env var — the env var stays as the cross-process
    // transport to phase children, not the in-process source of truth. This
    // also subsumes the old TASK-327 value-semantics guard (`AIDA_ZEN=0`/`false`
    // must not enable zen): `resolve_autonomy_mode` already handles it.
    // The full three-mode pre-launch banner is TASK-306's job.
    // trace:STORY-287 trace:ADR-10 trace:TASK-1060 | ai:claude
    if autonomy.is_zen() {
        // BUG-232: `--zen`'s end-of-session behavior differs by whether an
        // orchestrator is present to hand off to. With one, `--auto-complete`
        // drives PR → CI → review → merge; without one, `--zen` auto-opens
        // the PR at end-of-session then pauses on the grab-next/stop fork.
        // Corroborate via `orchestrator::detect` — BUG-233: never key off
        // the bare `AIDA_AUTO_COMPLETE` var, a stray value would lie here.
        // trace:BUG-232 | ai:claude
        let orch_root = project_root_for_config
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let (mode, note) = if orchestrator::detect(&orch_root).is_orchestrated() {
            (
                "zen + auto-complete",
                "(orchestrator drives end-of-session: PR → CI → review → merge)",
            )
        } else {
            (
                "zen",
                "(no orchestrator — confirmations auto-resolve; at end-of-session the PR \
                 auto-opens, then grab-next/stop pauses)",
            )
        };
        line("autonomy", format!("{} {}", mode.cyan(), note.dimmed()));
    }
    // TASK-112: surface a resume in the pre-flight summary.
    if let Some(QueueWorkLaunch::Resume(id)) = &launch {
        line(
            "resume",
            format!(
                "{} {}",
                id[..id.len().min(8)].cyan(),
                "(continuing prior conversation)".dimmed()
            ),
        );
    }
    if plan.entries.len() > 1 {
        line(
            "cluster",
            format!(
                "{} item{}",
                plan.entries.len().to_string().cyan(),
                if plan.entries.len() == 1 { "" } else { "s" }
            ),
        );
    }
    if let Some(warns) = &warnings {
        for w in warns {
            eprintln!(
                "  {} {}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                w.yellow()
            );
        }
    }

    // BUG-233: if this session carries `AIDA_AUTO_COMPLETE` without a live
    // corroboration token, surface it once — informationally. It is NOT a
    // leak to hunt (BUG-233's corrected diagnosis): the session simply runs
    // interactive. A corroborated orchestrator child stays silent here.
    // trace:BUG-233 | ai:claude
    {
        let root_for_orch = project_root_for_config
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        if let Some(note) = orchestrator::detect(&root_for_orch).informational_note() {
            eprintln!(
                "  {} {}",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                note.dimmed()
            );
        }
    }

    // TASK-1053: single-spec dry-run preview. The plan is now fully resolved —
    // the pre-flight summary above already printed anchor/scope/role/mode/skill;
    // here we add the branch, worktree path, session id, and the lease the
    // pickup WOULD create, then return. We sit BEFORE the first side effect
    // (the scope-conflict sweep, the pre-pickup pull, and session_start that
    // follow), so a dry run creates no worktree, takes no lease, writes no
    // manifest, and launches no claude. The branch + worktree derivation
    // mirror `session_start` exactly so the preview matches a real pickup.
    // The `--batch` dry-run is a separate path that returns at the dispatch
    // site before this function is even called. trace:TASK-1053 | ai:claude
    if dry_run {
        let dry_line = |label: &str, value: String| {
            eprintln!("  {:<8} {}", format!("{}:", label).bold(), value);
        };
        let project_root = find_main_worktree_root()?;
        let slug = slugify(&plan.scope);
        let branch = if let Some(b) = branch_override {
            b.to_string()
        } else if let Some((forge, n)) = plan.review_target {
            forge.local_branch_for(n)
        } else {
            // Best-effort: on the rare all-candidates-taken error, fall back to
            // the bare slug rather than failing a read-only preview.
            resolve_session_branch(&project_root, &slug, "auto").unwrap_or_else(|_| slug.clone())
        };
        let repo_name = project_root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("project");
        let worktree_path = match path_override {
            Some(p) => std::path::PathBuf::from(p),
            None => project_root
                .parent()
                .map(|parent| parent.join(format!("{}-{}", repo_name, slug)))
                .unwrap_or_else(|| std::path::PathBuf::from(format!("{}-{}", repo_name, slug))),
        };
        let session_render = match &launch {
            Some(l) => format!(
                "{} {}",
                l.session_id().cyan(),
                "(claude session id it would launch / resume)".dimmed()
            ),
            None => "(deferred — --no-launch)".dimmed().to_string(),
        };
        dry_line("branch", branch.cyan().to_string());
        dry_line(
            "worktree",
            worktree_path.display().to_string().cyan().to_string(),
        );
        dry_line("session", session_render);
        dry_line(
            "lease",
            format!(
                "{} {}",
                "would be created".cyan(),
                format!("(owner {}, scope {}, role {})", user_id, plan.scope, role).dimmed()
            ),
        );
        eprintln!();
        eprintln!(
            "{} dry run — nothing created (no worktree, no lease, no session). \
             Re-run without --dry-run to pick up.",
            crate::glyph(crate::glyphs::Glyph::Check).green().bold()
        );
        return Ok(());
    }

    // TASK-81: scope-conflict pre-flight. If any active lease already
    // owns plan.scope, `session_start` would refuse with "scope X already
    // owned by session Y". Surface that here with a queue-work-flavored
    // message that names --steal as the override, and when --steal is
    // passed, end EACH holding session cleanly first so session_start
    // sees a free scope. Re-list + re-detect after every end so we drain
    // the rare-but-possible multi-lease-same-scope state (manual lease
    // restores, partial session_end crashes, concurrent writes) — the
    // helper itself returns only the freshest, but session_start's guard
    // bails on ANY same-scope lease, so we must sweep until clean.
    // Uncommitted-changes guards in session_end still apply — we don't
    // escalate to --force on the user's behalf because that would
    // silently discard their in-flight work; the message points at
    // `aida session end --force` for the user-driven path.
    //
    // BUG-307 extension: before refusing or applying --steal, check
    // whether the conflict lease is **dormant** (process dead, mtime
    // stale, no live claude in the worktree). If it is AND the worktree
    // is clean, auto-release it transparently — the dominant friction
    // class for unsupervised drains is stale-lease state from PREVIOUS
    // stalls, every recovered failure leaves a lease behind. If the
    // worktree carries uncommitted changes we still refuse, but with a
    // loss-risk-aware message instead of the generic "pass --steal".
    // trace:TASK-81 trace:BUG-307 | ai:claude
    {
        let project_root_for_conflict = find_main_worktree_root()?;
        let orch_config = orchestrator::OrchestratorConfig::load(&project_root_for_conflict);
        // Safety cap: in practice this resolves in 1 iteration for the
        // canonical case, 2-3 for the multi-lease state. A bound prevents
        // a hypothetical infinite loop if session_end ever returns Ok
        // without actually removing the lease (defense-in-depth).
        let mut remaining = 16usize;
        while let Some(conflict) =
            find_scope_lease_conflict(&list_leases(&project_root_for_conflict), &plan.scope)
        {
            // BUG-307: classify before deciding to refuse. The auto-release
            // path only fires when every liveness signal says the lease is
            // truly dormant; a live or freshly-minted lease falls through to
            // the existing --steal/refuse logic unchanged.
            // trace:BUG-307 | ai:claude
            match auto_release_decision_for_lease(
                &project_root_for_conflict,
                &conflict,
                &orch_config,
            ) {
                orchestrator::AutoReleaseDecision::SafelyDormant {
                    process_dead,
                    mtime_age_secs,
                    worktree_missing,
                } => {
                    let cause = if worktree_missing {
                        format!(
                            "worktree missing, lease {}",
                            humanize_secs_short(mtime_age_secs)
                        )
                    } else if process_dead {
                        format!(
                            "process dead {} ago, worktree clean",
                            humanize_secs_short(mtime_age_secs)
                        )
                    } else {
                        format!(
                            "mtime {} old, worktree clean",
                            humanize_secs_short(mtime_age_secs)
                        )
                    };
                    eprintln!(
                        "  {} released stale lease {} ({})",
                        crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                        (&conflict.id[..conflict.id.len().min(8)]).yellow(),
                        cause.dimmed()
                    );
                    // force_cleanup_lease removes the lease file first, then
                    // attempts `git worktree remove --force`. A `false` return
                    // means the worktree leg failed (likely already gone or
                    // permissions); the lease file is still removed and the
                    // loop's `find_scope_lease_conflict` re-check will confirm
                    // the conflict has cleared. force_cleanup_lease prints its
                    // own Warning so the operator sees the worktree-remove
                    // failure without a hard bail.
                    let _ = force_cleanup_lease(&project_root_for_conflict, &conflict);
                    remaining -= 1;
                    if remaining == 0 {
                        anyhow::bail!(
                            "auto-release sweep gave up after 16 iterations on scope `{}` — \
                             the lease store may be corrupt; inspect `.aida/sessions/`",
                            plan.scope
                        );
                    }
                    continue;
                }
                orchestrator::AutoReleaseDecision::DormantDirty { dirty_entries } => {
                    // TASK-402 (friction #3 + #4): a dormant lease with a dirty
                    // worktree is the canonical resume-after-failure state — the
                    // process died mid-implementation (orphaned) and the
                    // unfinished work lives uncommitted in that exact worktree.
                    // The bare message ("commit/stash, or --force to discard")
                    // omits the one option that keeps the WIP: resume into the
                    // existing worktree. `--steal` would remove the worktree the
                    // WIP lives in, so name it as the destructive path, not the
                    // default. trace:TASK-402 | ai:claude
                    let resume_hint = if resume.is_some() {
                        format!(
                            " You passed --resume: keep this worktree and re-attach \
                             to the recorded session — re-run with \
                             `aida queue work {} --resume <session-uuid>` \
                             (--list-sessions shows the uuid).",
                            plan.scope
                        )
                    } else {
                        format!(
                            " To keep the work, resume into this worktree: \
                             `aida queue work {} --resume` (bare = most-recent session). \
                             Only commit/stash or `aida session end {} --force` (discards) \
                             if you intend to abandon it.",
                            plan.scope,
                            &conflict.id[..conflict.id.len().min(8)]
                        )
                    };
                    anyhow::bail!(
                        "lease {} for scope `{}` looks orphaned (dormant process, \
                         worktree has {} uncommitted change(s) at {}).{}",
                        &conflict.id[..conflict.id.len().min(8)],
                        plan.scope,
                        dirty_entries,
                        conflict.worktree_path.display(),
                        resume_hint
                    );
                }
                orchestrator::AutoReleaseDecision::Live => {
                    // Fall through to the existing --steal/refuse logic.
                }
            }

            if !steal {
                // TASK-402 (friction #4): the lease is live (the auto-release
                // sweep above handles dormant/orphaned leases). Warn that
                // `--steal` ends the session AND removes its worktree — so if
                // there is unfinished work in that worktree, `--resume` (which
                // re-attaches in place) is the work-preserving path, not
                // `--steal`. trace:TASK-402 | ai:claude
                anyhow::bail!(
                    "scope `{}` is owned by lease {} ({}, worktree: {}) — \
                     to take over: `--steal` ends that session and removes its \
                     worktree (work there is lost unless committed), \
                     or `--resume` re-attaches to it in place (keeps the worktree), \
                     or `aida session end {}` manually.",
                    plan.scope,
                    &conflict.id[..conflict.id.len().min(8)],
                    conflict.role.as_deref().unwrap_or("(unset)"),
                    conflict.worktree_path.display(),
                    &conflict.id[..conflict.id.len().min(8)]
                );
            }
            eprintln!(
                "  {} {} — ending it first (--steal)",
                "⟲".cyan().bold(),
                format!(
                    "scope `{}` owned by lease {}",
                    plan.scope,
                    &conflict.id[..conflict.id.len().min(8)]
                )
                .cyan()
            );
            let short_id = &conflict.id[..conflict.id.len().min(8)];
            session_end(
                Some(&conflict.id),
                /* spec */ None,
                /* branch */ None,
                /* yes */ true,
                /* force */ false,
                /* purge_cc */ false,
                /* wait_ci */ false,
                /* watch_ci */ false,
                // --steal is a recovery path for stuck leases; skip the
                // CI probe so we don't ask the user to wait on CI for a
                // session they're actively stealing. trace:TASK-111
                /* skip_ci */
                true,
                /* return_to_pool */ false,
                // --steal reclaims a stuck lease's tree; let the default apply
                // (a pooled tree is returned + reset clean, not deleted).
                /* remove */
                false,
            )
            // BUG-311: collapse the internal session_end error into the
            // primary message so it surfaces specifically as "--steal could
            // not end lease X: <actual reason>" — not the canned "pass
            // --steal" loop and not a context line that buries the cause
            // under a "Caused by:" chain. Anyhow's `{:#}` chains the cause
            // inline on one line for the same effect. trace:BUG-311
            .map_err(|e| {
                anyhow::anyhow!(
                    "--steal could not end lease {}: {:#} \
                     (resolve manually with `aida session end {} --force` to discard, \
                     or commit/stash the worktree's changes first, then re-run)",
                    short_id,
                    e,
                    short_id,
                )
            })?;
            remaining -= 1;
            if remaining == 0 {
                anyhow::bail!(
                    "--steal sweep gave up after 16 iterations on scope `{}` — \
                     the lease store may be corrupt; inspect `.aida/sessions/`",
                    plan.scope
                );
            }
        }
    }

    // TASK-619: capture the anchor spec's status from the *local* (pre-pull)
    // view so we can detect whether the pull below reveals that another
    // machine claimed the spec under us. Best-effort + read-only — a load
    // failure just yields None, which makes the guard a no-op.
    // trace:TASK-619 | ai:claude
    let status_before_pull: Option<RequirementStatus> = storage.load().ok().and_then(|store| {
        store
            .get_requirement_by_spec_id(&plan.scope)
            .map(|r| r.status.clone())
    });

    // Pull the orphan store before resolving the worktree — keeps the
    // queue + req view fresh for the new session. Best-effort: a
    // failed pull (offline, divergent) prints a note and continues
    // with local view rather than aborting the whole pickup.
    if !no_pull {
        if let Err(e) = run_aida_db_sync_pull(storage.path()) {
            eprintln!(
                "  {} {}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                format!("pre-pickup pull failed; proceeding with local view: {}", e).yellow()
            );
        }
    }

    // TASK-619: cross-machine duplicate-pickup guard. Session leases are
    // machine-local (.aida/sessions/, gitignored), so the only shared
    // "someone is on this" signal across machines is the git-canonical
    // status flip — which is eventually-consistent. Now that we've pulled
    // the latest store, re-read the anchor spec's status and refuse if it
    // was claimed/shipped elsewhere in the window between planning this
    // pickup and now. The existing BUG-379 preflight in `session_start`
    // catches stuck-InProgress-without-lease; this guard specifically names
    // the cross-machine dup-pickup case with a recovery hint and respects
    // --force-claim / orchestrator-corroboration / review sessions.
    // trace:TASK-619 | ai:claude
    if !no_pull {
        let status_after_pull: Option<RequirementStatus> = storage.load().ok().and_then(|store| {
            store
                .get_requirement_by_spec_id(&plan.scope)
                .map(|r| r.status.clone())
        });
        let orchestrator_corroborated = {
            let root = project_root_for_config
                .clone()
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            orchestrator::detect(&root).is_orchestrated()
        };
        let is_review_session = plan.review_target.is_some()
            || role == "reviewer"
            || std::env::var("AIDA_REVIEW_VERDICT_FILE").is_ok();
        if let DupPickupDecision::Refuse(msg) = dup_pickup_recheck(
            &plan.anchor_display,
            status_before_pull.as_ref(),
            status_after_pull.as_ref(),
            force_claim,
            orchestrator_corroborated,
            is_review_session,
        ) {
            anyhow::bail!("{}", msg);
        }
    }

    // Set AIDA_SESSION_ROLE for the exec'd claude (and for any in-process
    // logic the rest of this command runs against). session_start reads
    // it to record the lease's role field when --role isn't passed; we
    // pass --role anyway, but the env var is what the claude SessionStart
    // hook sees on launch.
    std::env::set_var("AIDA_SESSION_ROLE", &role);

    // STORY-248: resolve the stacked base BEFORE session_start so the
    // worktree is forked from the right place and the SHA capture has a
    // real base to record. Both `--stack` and `--base` flow through the
    // same `session_start(base: Option<&str>)` parameter.
    // trace:STORY-248 | ai:claude
    let project_root_for_base = find_main_worktree_root()?;
    let cwd_for_base = std::env::current_dir().unwrap_or_else(|_| project_root_for_base.clone());
    let resolved_base = resolve_stack_base(
        &project_root_for_base,
        &cwd_for_base,
        stack,
        base,
        force_base,
    )?;
    if let Some(b) = resolved_base.as_deref() {
        eprintln!(
            "  {} base: {} {}",
            crate::glyph(crate::glyphs::Glyph::FlowActive).cyan().bold(),
            b.cyan(),
            if stack {
                "(detected via --stack)".dimmed().to_string()
            } else {
                "(--base)".dimmed().to_string()
            }
        );
    }

    // session_start handles worktree creation, lease persistence, env
    // shim, conflict detection. We always pass launch=false so we can
    // (a) write the cluster manifest from the new lease and (b) emit
    // a queue-work-specific launch summary before exec.
    session_start(
        &plan.scope,
        branch_override,
        resolved_base.as_deref(),
        /* reuse_branch */ false,
        path_override,
        /* forge_override */ None,
        /* branch_style */ "auto",
        /* launch */ false,
        /* launch_title */ None,
        /* launch_name */ None,
        // STORY-495: inert here (launch=false); the real launch below threads
        // the resolved `permission_mode` into the exec call directly.
        /* permission_mode */
        permission_mode.as_deref(),
        /* launch_contained */
        false,
        /* role */ Some(role.clone()),
        /* force_claim */ force_claim,
        // STORY-714: config-driven ([worktree_pool] enabled). The pool is used
        // only for non-stacked spawns (resolved_base is None unless --stack /
        // --base), so stacked work keeps a fresh `git worktree add`.
        /* use_pool */
        None,
    )?;

    // Look up the lease we just minted: by scope, by owner=us, freshest.
    let project_root = find_main_worktree_root()?;
    let lease = list_leases(&project_root)
        .into_iter()
        .filter(|l| l.scope.eq_ignore_ascii_case(&plan.scope))
        .max_by_key(|l| l.started_at)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "session_start completed but no lease for scope `{}` is visible — try `aida session leases`",
                plan.scope
            )
        })?;

    // TASK-99: warn (don't auto-pull) when the base the new worktree forked
    // from is behind origin/main. Closes the visibility half of the
    // 2026-05-13 stale-base pain cheaply: the operator sees the drift at
    // pickup and can `aida rebase` before the session accumulates work on a
    // stale base. We deliberately do NOT auto-pull here — that risks
    // surprising the worktree; the orchestrator drain (fresh main per-phase)
    // and the rebase verb own divergence handling. Best-effort + silent on
    // missing data (no origin/main → fresh clone / offline).
    // trace:TASK-99 | ai:claude
    {
        let base_ref = resolved_base.as_deref().unwrap_or("main");
        if let Some(behind) = commits_behind_origin_main(&project_root, base_ref) {
            if let Some(msg) = behind_origin_warning(behind, "main") {
                eprintln!(
                    "  {} {}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                    msg.yellow()
                );
            }
        }
    }

    // STORY-248: register the stacked-branch entry in `.aida/stacks.json`
    // so `aida pull --auto`'s cascade can find it when the parent merges.
    // session_start records the parent fields on the lease itself; we
    // mirror them into the dedicated graph file because the cascade
    // needs to consult them AFTER the lease has been removed by
    // `aida queue done`. Best-effort — a write failure logs but doesn't
    // fail the pickup. trace:STORY-248 | ai:claude
    if let (Some(parent), Some(sha)) = (
        lease.parent_branch.as_deref(),
        lease.parent_branch_sha.as_deref(),
    ) {
        let mut graph = stacks::load(&project_root);
        stacks::add(
            &mut graph,
            stacks::StackEntry {
                branch: lease.branch.clone(),
                parent_branch: parent.to_string(),
                parent_branch_sha: sha.to_string(),
                spec_id: Some(plan.scope.clone()),
                created_at: chrono::Utc::now(),
            },
        );
        if let Err(e) = stacks::save(&project_root, &graph) {
            eprintln!(
                "  {} {}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                format!(
                    "stack graph save failed: {} (cascade may miss this branch)",
                    e
                )
                .yellow()
            );
        } else {
            eprintln!(
                "  {} stacked: {} → {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                lease.branch.cyan(),
                parent.cyan()
            );
        }
    }

    // Cluster manifest: pre-populate items so /aida-pickup can walk
    // them top-down. Skip for head/item modes (single item → no plan
    // needed beyond the queue head). trace:STORY-98 | ai:claude
    //
    // TASK-95: discover the plan brief for the anchor spec from any
    // owning docs/plans/ file. Cluster mode always writes a manifest, so
    // it just gains the brief; head/item modes write a manifest only
    // when a plan file exists (no plan file → today's no-op behavior).
    // trace:TASK-95 | ai:claude
    let plan_context = discover_plan_context(&project_root, &plan.anchor_display);
    // TASK-112: the claude session id to record in the manifest — the
    // UUID minted for a fresh launch, or the id being resumed.
    // STORY-132: a caller-minted `--session-id` is recorded even under
    // `--no-launch` (the TUI sets up a session then hosts the launch
    // itself), so the manifest carries the id either way.
    let claude_session_id: Option<String> = match (&launch, session_id) {
        (Some(l), _) => Some(l.session_id().to_string()),
        // --no-launch + caller-minted id: record it so the manifest
        // carries it (already validated as a UUID at function entry).
        (None, Some(sid)) => Some(sid.to_string()),
        // BUG-225: --no-launch --no-human defers a headless launch, and
        // `claude -p` requires a `--session-id`. Mint one when the caller
        // didn't supply it so the manifest carries the id and the printed
        // hint is a complete, round-trippable command.
        (None, None) if no_human => Some(uuid::Uuid::now_v7().to_string()),
        (None, None) => None,
    };
    // Write the manifest when there are planned cluster items, a plan
    // brief was discovered, there's a claude session id to record so
    // a later `--resume` can find this conversation, or this is a batch
    // pickup whose batch marker /aida-pickup needs.
    // trace:STORY-98, TASK-95, TASK-112, TASK-272 | ai:claude
    if plan.mode == QueueWorkMode::Cluster
        || plan_context.is_some()
        || claude_session_id.is_some()
        || batch_name.is_some()
    {
        write_queue_work_manifest(
            &project_root,
            &lease,
            &plan,
            plan_context.clone(),
            claude_session_id.clone(),
            batch_name,
        )?;
        if plan.mode == QueueWorkMode::Cluster {
            eprintln!(
                "  {} wrote manifest with {} planned item(s)",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                plan.entries.len()
            );
        }
        if let Some(ctx) = &plan_context {
            eprintln!(
                "  {} attached plan brief from {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                ctx.plan_file.cyan()
            );
        }
    }

    if no_launch {
        eprintln!();
        eprintln!(
            "{} setup complete; launch deferred (`--no-launch`).",
            crate::glyph(crate::glyphs::Glyph::Check).green().bold()
        );
        eprintln!(
            "  {}",
            format!("cd {}", lease.worktree_path.display()).cyan()
        );
        eprintln!(
            "  {}    {}",
            "source .aida/session-env.sh".cyan(),
            "# share parent's cargo target/".dimmed()
        );
        if no_human {
            // STORY-263: mirror the headless launch the non-`--no-launch`
            // path would have run. BUG-225: render it from
            // `claude_headless_args` (via `headless_launch_hint`) so the
            // copy-pasteable command can't drift from `exec_claude_headless`
            // — same flag order, `--session-id` included, prompt last.
            // STORY-278: helper also prefixes `AIDA_HEADLESS=1` to match
            // the env `exec_claude_headless` sets, so the copy-pasted hint
            // launches an equivalent process.
            let sid = claude_session_id.as_deref().unwrap_or_default();
            eprintln!("  {}", headless_launch_hint(&prompt, sid, contained).cyan());
        } else {
            let mut args = session::claude_session_args(
                permission_mode.as_deref(),
                None,
                Some(&prompt),
                None,
                contained,
            );
            if contained && !args.iter().any(|arg| arg == "--permission-mode") {
                args.splice(
                    0..0,
                    ["--permission-mode".to_string(), "dontAsk".to_string()],
                );
            }
            eprintln!(
                "  {}",
                format!("claude {}", shell_join_display(&args)).cyan()
            );
        }
        // BUG-673: next-step breadcrumb after a `queue work` pickup. The lease
        // is taken and the worktree is set up (only the launch is deferred), so
        // this IS a genuine pickup — the natural next move is to finish the work
        // (`aida queue done <id>`). Closes the gap where the next[] block
        // dropped out at `queue work`. Emit the TOON `next[]` block in agent
        // mode, the human `Next:` block otherwise. trace:BUG-673 | ai:claude
        {
            let next = crate::help_next::queue_work_next(&plan.anchor_display);
            let rendered = if agent_output_mode() {
                crate::help_next::render(&next)
            } else {
                crate::help_next::render_human(&next)
            };
            if let Some(block) = rendered {
                println!("{block}");
            }
        }
        return Ok(());
    }

    // Chdir + source session-env shim + exec claude with the skill prompt.
    // Mirrors session_start's launch path (TASK-63 env sourcing).
    let env_shim = lease.worktree_path.join(".aida").join("session-env.sh");
    let applied_vars: Vec<String> = match std::fs::read_to_string(&env_shim) {
        Ok(body) => apply_session_env_to_process(&body),
        Err(_) => Vec::new(),
    };
    if !applied_vars.is_empty() {
        eprintln!(
            "  {} sourced .aida/session-env.sh ({})",
            crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
            applied_vars.join(", ").dimmed()
        );
    }
    std::env::set_current_dir(&lease.worktree_path)
        .with_context(|| format!("failed to chdir into {}", lease.worktree_path.display()))?;

    // TASK-112: exec — resume the prior conversation, or cold-launch
    // with the minted session id so this conversation is itself
    // resumable later. trace:TASK-112 | ai:claude
    let launch = launch.expect("launch decision is set when !no_launch");

    // BUG-226: a standalone `aida queue work <PR-N> --role reviewer` — a
    // reviewer session on a PR scope NOT spawned by the `--auto-complete`
    // orchestrator. The orchestrator sets `AIDA_REVIEW_VERDICT_FILE` on
    // its phase-3 child, so its absence is the standalone signal. For a
    // standalone run, point the `/aida-review` skill at a verdict file and
    // route the launch through `run_standalone_reviewer` (spawn + wait +
    // end-of-command summary) instead of `exec`'ing claude — so the
    // command no longer exits silently to the shell prompt.
    // trace:BUG-226 | ai:claude
    let standalone_reviewer: Option<(u64, std::path::PathBuf)> = if role
        .eq_ignore_ascii_case("reviewer")
        && std::env::var("AIDA_REVIEW_VERDICT_FILE")
            .ok()
            .filter(|s| !s.is_empty())
            .is_none()
    {
        plan.review_target.map(|(_, n)| {
            (
                n,
                project_root
                    .join(".aida")
                    .join("review-verdicts")
                    .join(format!("PR-{n}.json")),
            )
        })
    } else {
        None
    };
    if let Some((_, verdict_path)) = &standalone_reviewer {
        if let Some(dir) = verdict_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Clear any stale verdict so the summary can't read a prior run's.
        let _ = std::fs::remove_file(verdict_path);
        // The `/aida-review` skill writes the verdict here — it keys off
        // this env var. Setting it for standalone runs too is what makes
        // the skill "always write the verdict file" (BUG-226 acceptance).
        std::env::set_var("AIDA_REVIEW_VERDICT_FILE", verdict_path);
    }

    eprintln!();
    if let Some((pr, verdict_path)) = standalone_reviewer {
        return run_standalone_reviewer(
            &project_root,
            pr,
            launch,
            &prompt,
            permission_mode.as_deref(),
            &plan.scope,
            &lease.branch,
            &role,
            &lease.worktree_path,
            no_human,
            quiet,
            &verdict_path,
            contained,
        );
    }
    // TASK-895: a Codex tab hosts a fresh interactive Codex session. Codex has
    // no caller-minted session id / AIDA-addressable resume, and the interactive
    // tab launch is never `--no-human` (the headless drain resolves its own
    // vendor via STORY-683), so this branch handles only the interactive Codex
    // launch and leaves the entire Claude `match launch` below byte-identical.
    // trace:TASK-895 | ai:claude
    if vendor.eq_ignore_ascii_case("codex") && !no_human {
        // BUG-743: queue/do's interactive Codex path used to ignore the
        // STORY-495 `[agents] bypass` resolver and launch bare `codex
        // /aida-pickup`, leaving operators in prompt-per-command posture even
        // after opting the supervised fleet into bypass. Map the resolved
        // uniform bypass posture to Codex's actual flag here; `None` and every
        // non-bypass Claude permission mode keep Codex native.
        let codex_bypass = permission_mode.as_deref() == Some("bypassPermissions");
        eprintln!(
            "{} {}",
            crate::glyph(crate::glyphs::Glyph::FlowActive)
                .green()
                .bold(),
            format!(
                "launching codex in {} ({}, prompt `{}`)",
                lease.worktree_path.display(),
                if codex_bypass { "bypass" } else { "native" },
                prompt
            )
            .cyan()
        );
        return session::exec_codex_session(&prompt, codex_bypass);
    }
    match launch {
        QueueWorkLaunch::Resume(id) => {
            if no_human {
                let log_path = project_root
                    .join(".aida")
                    .join("headless-logs")
                    .join(format!("{}-{}.jsonl", lease.branch, id));
                eprintln!(
                    "{} {}",
                    crate::glyph(crate::glyphs::Glyph::FlowActive)
                        .green()
                        .bold(),
                    format!(
                        "resuming claude headless session {} in {} (claude -p, {}, prompt `{}`)",
                        &id[..id.len().min(8)],
                        lease.worktree_path.display(),
                        claude_posture_display(permission_mode.as_deref(), contained),
                        prompt
                    )
                    .cyan()
                );
                eprintln!(
                    "  {} headless output → {}",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    log_path.display().to_string().dimmed()
                );
                // BUG-342: no-human resumes must use the same structural
                // AskUserQuestion denial as fresh headless launches. Plain
                // `claude --resume` bypasses `claude_headless_resume_args`.
                // trace:BUG-342 | ai:codex
                let tee_opts =
                    headless_tee::TeeOptions::from_env_and_flag(false).with_label(&lease.branch);
                let status = session::spawn_claude_headless_resume(
                    &prompt,
                    &id,
                    &log_path,
                    &lease.worktree_path,
                    &tee_opts,
                    contained,
                )?;
                std::process::exit(status.code().unwrap_or(1));
            }
            eprintln!(
                "{} {}",
                crate::glyph(crate::glyphs::Glyph::FlowActive)
                    .green()
                    .bold(),
                format!(
                    "resuming claude session {} in {} ({})",
                    &id[..id.len().min(8)],
                    lease.worktree_path.display(),
                    claude_posture_display(permission_mode.as_deref(), contained)
                )
                .cyan()
            );
            session::exec_claude_resume(&id, permission_mode.as_deref(), contained)
        }
        QueueWorkLaunch::Fresh(id) => {
            let name = session::derive_session_name(&plan.scope, &lease.branch, &role);
            if no_human {
                // STORY-263: headless launch — `claude -p`, single-turn,
                // exits on its own (no Ctrl+D). `bypassPermissions` is forced
                // (SPIKE-7 Q2 — `acceptEdits` leaves Bash gated); the
                // stream-json output goes to a log file the watchdog
                // (TASK-298) can tail. trace:STORY-263 | ai:claude
                let log_path = project_root
                    .join(".aida")
                    .join("headless-logs")
                    .join(format!("{}-{}.jsonl", lease.branch, id));
                // BUG-705: the banner names the RESOLVED headless vendor —
                // before, it always said claude even when the drain was
                // routed to codex, hiding the unrouted-exec bug.
                let headless_vendor = crate::session::resolve_headless_vendor(
                    &find_project_root().unwrap_or_else(|_| std::path::PathBuf::from(".")),
                );
                let launch_detail = match headless_vendor {
                    crate::session::HeadlessVendor::Claude => format!(
                        "claude -p, {}",
                        claude_posture_display(permission_mode.as_deref(), contained)
                    ),
                    crate::session::HeadlessVendor::Codex => "codex exec".to_string(),
                    // trace:TASK-1048 | ai:claude
                    crate::session::HeadlessVendor::Agy => "agy -p".to_string(),
                };
                eprintln!(
                    "{} {}",
                    crate::glyph(crate::glyphs::Glyph::FlowActive)
                        .green()
                        .bold(),
                    format!(
                        "launching {} headless in {} ({}, prompt `{}`)",
                        headless_vendor.as_str(),
                        lease.worktree_path.display(),
                        launch_detail,
                        prompt
                    )
                    .cyan()
                );
                eprintln!(
                    "  {} headless output → {}",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    log_path.display().to_string().dimmed()
                );
                // TASK-307: tee high-signal events so the operator can
                // follow the headless run without opening a second terminal
                // to tail the JSONL. Disable with `--no-tee-headless` or
                // `AIDA_TEE_HEADLESS=0`; failure events stream regardless.
                // trace:TASK-307 | ai:claude
                let tee_opts =
                    headless_tee::TeeOptions::from_env_and_flag(false).with_label(&lease.branch);
                return session::exec_claude_headless(
                    &prompt,
                    &id,
                    &log_path,
                    &tee_opts,
                    contained,
                    Some(&lease.id),
                );
            }
            eprintln!(
                "{} {}",
                crate::glyph(crate::glyphs::Glyph::FlowActive)
                    .green()
                    .bold(),
                format!(
                    "launching claude in {} ({}, prompt `{}`)",
                    lease.worktree_path.display(),
                    claude_posture_display(permission_mode.as_deref(), contained),
                    prompt
                )
                .cyan()
            );
            session::exec_claude_with_session(
                permission_mode.as_deref(),
                name.as_deref(),
                &prompt,
                &id,
                contained,
            )
        }
    }
}

/// BUG-226: drive a standalone `aida queue work <PR-N> --role reviewer`
/// launch — spawn `claude` (not `exec`) so this process survives to read
/// the verdict file + headless JSONL log and print an end-of-command
/// summary. The bug was the silent exit to the shell prompt: a headless
/// reviewer ran to completion, posted a real review, and left no terminal
/// trace of pass/fail, cost, or where the artifacts landed.
///
/// The `--auto-complete` orchestrator's phase-3 reviewer does NOT come
/// here — it sets `AIDA_REVIEW_VERDICT_FILE`, which clears the standalone
/// signal — so the orchestrator keeps owning its own progress output.
/// trace:BUG-226 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_standalone_reviewer(
    project_root: &std::path::Path,
    pr: u64,
    launch: QueueWorkLaunch,
    prompt: &str,
    // STORY-495: `None` → faithful native launch (no `--permission-mode`).
    permission_mode: Option<&str>,
    scope: &str,
    branch: &str,
    role: &str,
    worktree: &std::path::Path,
    no_human: bool,
    quiet: bool,
    verdict_path: &std::path::Path,
    contained: bool,
) -> Result<()> {
    // Spawn claude, wait, and capture the headless JSONL log path (None
    // for an interactive review — there is no stream-json log).
    let (status, log_path): (std::process::ExitStatus, Option<std::path::PathBuf>) = match launch {
        QueueWorkLaunch::Resume(id) => {
            if no_human {
                let log_path = project_root
                    .join(".aida")
                    .join("headless-logs")
                    .join(format!("{branch}-{id}.jsonl"));
                eprintln!(
                    "{} {}",
                    crate::glyph(crate::glyphs::Glyph::FlowActive)
                        .green()
                        .bold(),
                    format!(
                        "resuming claude headless reviewer session {} in {} (claude -p, {})",
                        &id[..id.len().min(8)],
                        worktree.display(),
                        claude_posture_display(permission_mode, contained)
                    )
                    .cyan()
                );
                eprintln!(
                    "  {} headless output → {}",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    log_path.display().to_string().dimmed()
                );
                // BUG-342: a no-human reviewer resume is still unattended;
                // route it through the shared headless resume argv so
                // AskUserQuestion is structurally denied.
                // trace:BUG-342 | ai:codex
                let tee_opts =
                    headless_tee::TeeOptions::from_env_and_flag(false).with_label("reviewer");
                let status = session::spawn_claude_headless_resume(
                    prompt, &id, &log_path, worktree, &tee_opts, contained,
                )?;
                (status, Some(log_path))
            } else {
                eprintln!(
                    "{} {}",
                    crate::glyph(crate::glyphs::Glyph::FlowActive)
                        .green()
                        .bold(),
                    format!(
                        "resuming claude reviewer session {} in {} ({})",
                        &id[..id.len().min(8)],
                        worktree.display(),
                        claude_posture_display(permission_mode, contained)
                    )
                    .cyan()
                );
                (
                    session::spawn_claude_resume(&id, permission_mode, contained)?,
                    None,
                )
            }
        }
        QueueWorkLaunch::Fresh(id) => {
            if no_human {
                let log_path = project_root
                    .join(".aida")
                    .join("headless-logs")
                    .join(format!("{branch}-{id}.jsonl"));
                eprintln!(
                    "{} {}",
                    crate::glyph(crate::glyphs::Glyph::FlowActive)
                        .green()
                        .bold(),
                    format!(
                        "launching claude headless reviewer in {} (claude -p, {})",
                        worktree.display(),
                        claude_posture_display(permission_mode, contained)
                    )
                    .cyan()
                );
                eprintln!(
                    "  {} headless output → {}",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    log_path.display().to_string().dimmed()
                );
                // TASK-307: tee high-signal events for the standalone
                // reviewer too. Label = "reviewer" so a future concurrent
                // batch can disambiguate `│ [headless:reviewer]` lines.
                // trace:TASK-307 | ai:claude
                let tee_opts =
                    headless_tee::TeeOptions::from_env_and_flag(false).with_label("reviewer");
                let status =
                    session::spawn_claude_headless(prompt, &id, &log_path, &tee_opts, contained)?;
                (status, Some(log_path))
            } else {
                let name = session::derive_session_name(scope, branch, role);
                eprintln!(
                    "{} {}",
                    crate::glyph(crate::glyphs::Glyph::FlowActive)
                        .green()
                        .bold(),
                    format!(
                        "launching claude reviewer in {} ({}, prompt `{}`)",
                        worktree.display(),
                        claude_posture_display(permission_mode, contained),
                        prompt
                    )
                    .cyan()
                );
                let status = session::spawn_claude_session(
                    permission_mode,
                    name.as_deref(),
                    prompt,
                    &id,
                    contained,
                )?;
                (status, None)
            }
        }
    };

    // STORY-439: tag-along reviewer-side calibration capture. Resolve
    // every spec the PR credits (via the existing title / branch / body
    // precedence used by the squash-subject repair) and write a review
    // slot per spec. Best-effort; never blocks the summary print.
    // trace:STORY-439 | ai:claude
    if let Ok(meta) = fetch_pr_ship_metadata_via_gh(project_root, pr) {
        for spec in pr_ship::derive_squash_subject_spec_ids(&meta.title, branch, &meta.body) {
            capture_review_calibration_for_spec(project_root, verdict_path, &spec);
        }
    }

    // End-of-command summary, assembled from the verdict file the
    // `/aida-review` skill wrote and (headless only) the JSONL log.
    // `--quiet` suppresses it for scripted consumers that read the
    // verdict file directly. trace:BUG-226 | ai:claude
    if !quiet {
        let verdict_json = std::fs::read_to_string(verdict_path).ok();
        let result_event = log_path
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| reviewer_summary::parse_result_event(&s));
        let summary = reviewer_summary::format_reviewer_summary(
            pr,
            verdict_json.as_deref(),
            result_event.as_ref(),
            verdict_path,
            log_path.as_deref(),
            status.code(),
        );
        println!();
        println!("{summary}");
    }

    // Propagate claude's exit code so a scripted consumer can branch on
    // it; a clean exit returns `Ok(())` so `main` records telemetry.
    match status.code() {
        Some(0) | None => Ok(()),
        Some(code) => std::process::exit(code),
    }
}

// ===========================================================================
// STORY-246: `aida queue work --auto-complete` — full lifecycle orchestrator.
// The phase-sequencing logic lives in `auto_complete.rs`; this is the real
// `PhaseDriver` — it spawns Claude sessions, polls CI, and shells out to
// `gh` / `aida` / `cargo`. trace:STORY-246 | ai:claude
// ===========================================================================

/// Partial view of a `SessionLease` TOML — just the fields the orchestrator
/// needs to discover after spawning a session. trace:STORY-246 | ai:claude
#[derive(serde::Deserialize)]
pub(crate) struct LeasePeek {
    pub(crate) id: String,
    pub(crate) branch: String,
    /// Optional for older leases and minimal test fixtures. Used only by the
    /// phase watchdog's local descendant scan.
    // trace:BUG-749 | ai:codex
    #[serde(default)]
    pub(crate) creator_pid: Option<u32>,
    /// Worktree path — re-read by the BUG-223 branch-swap reconciliation to
    /// recover the live branch when `/aida-pr` swapped it mid-phase.
    /// `#[serde(default)]` so a lease (or hand-written test fixture) without
    /// the field still parses. trace:BUG-223 | ai:claude
    #[serde(default)]
    pub(crate) worktree_path: std::path::PathBuf,
}

/// Statuses the `--auto-complete` orchestrator can drive from scratch
/// (TASK-292). The orchestrator runs a full implementer → CI → reviewer →
/// merge lifecycle starting at phase 1, so it can only begin a spec that
/// hasn't started: Draft / Approved / Planned. In Progress and Done are
/// mid-flight (someone is on it / it sits on a branch awaiting merge);
/// Completed and Rejected are terminal. trace:TASK-292 | ai:claude
pub(crate) fn auto_complete_head_drivable(status: &RequirementStatus) -> bool {
    matches!(
        status,
        RequirementStatus::Draft | RequirementStatus::Approved | RequirementStatus::Planned
    )
}

/// Pure pickup-order resolution for the `aida queue work --auto-complete` head
/// (TASK-292): given queued `(display_id, status)` pairs already in pickup
/// order, return the first orchestrator-drivable spec together with the items
/// skipped to reach it, or `Err(skipped)` when none is drivable. Split out of
/// [`resolve_auto_complete_head`] so the skip / empty-queue logic is
/// unit-testable without a storage fixture. trace:TASK-292 | ai:claude
#[allow(clippy::type_complexity)]
pub(crate) fn pick_auto_complete_head(
    candidates: &[(String, RequirementStatus)],
) -> std::result::Result<(String, Vec<(String, RequirementStatus)>), Vec<(String, RequirementStatus)>>
{
    let mut skipped: Vec<(String, RequirementStatus)> = Vec::new();
    for (id, status) in candidates {
        if auto_complete_head_drivable(status) {
            return Ok((id.clone(), skipped));
        }
        skipped.push((id.clone(), status.clone()));
    }
    Err(skipped)
}

/// Build the `(display_id, status)` candidate list for the active role's
/// queue in pickup order (queue position ascending, same as `aida queue
/// next`) — the shared input to [`pick_auto_complete_head`] for both
/// single-head pickup (TASK-292) and the `nextN` drain (TASK-293).
/// trace:TASK-292 TASK-293 | ai:claude
pub(crate) fn auto_complete_head_candidates(
    storage: &Storage,
    user_id: &str,
) -> Result<Vec<(String, RequirementStatus)>> {
    let entries = storage.queue_list(user_id, /* include_completed */ false)?;
    let store = storage.load()?;
    let role_filter: Option<String> = std::env::var("AIDA_SESSION_ROLE")
        .ok()
        .filter(|s| !s.is_empty());

    let mut ordered: Vec<&aida_core::QueueEntry> = entries
        .iter()
        .filter(|e| match &role_filter {
            Some(r) => e.for_role.as_deref() == Some(r.as_str()),
            None => true,
        })
        .collect();
    ordered.sort_by_key(|e| e.position);

    Ok(ordered
        .iter()
        .filter_map(|e| {
            store
                .requirements
                .iter()
                .find(|r| r.id == e.requirement_id)
                .map(|r| (r.display_id(), r.status.clone()))
        })
        .collect())
}

/// Resolve the queue head for `aida queue work --auto-complete` invoked with no
/// positional SPEC id (TASK-292) — the natural composition of the no-arg "pick
/// the head" semantics with `--auto-complete`. Walks the active role's queue in
/// pickup order (queue position ascending, same as `aida queue next`) and
/// returns the first item the orchestrator can drive from scratch, skipping —
/// with a note — any item already In Progress / Done / terminal. Errors when
/// nothing drivable remains. trace:TASK-292 | ai:claude
pub(crate) fn resolve_auto_complete_head(storage: &Storage, user_id: &str) -> Result<String> {
    let role_label = std::env::var("AIDA_SESSION_ROLE")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "any role".to_string());
    let candidates = auto_complete_head_candidates(storage, user_id)?;

    match pick_auto_complete_head(&candidates) {
        Ok((spec, skipped)) => {
            // Acceptance criterion: name each item skipped to reach the
            // drivable head so the pickup is never silently surprising.
            for (id, status) in &skipped {
                eprintln!(
                    "  {} skipping {} ({}) — the orchestrator drives a full \
                     lifecycle from scratch and cannot resume it",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    id,
                    status
                );
            }
            Ok(spec)
        }
        Err(skipped) if skipped.is_empty() => {
            anyhow::bail!("queue is empty for {role_label}; nothing to drive")
        }
        Err(skipped) => {
            // The queue has items, but every one is in-flight or terminal —
            // name the first few so it's clear *why* there's nothing to
            // drive, without dumping a long stale list.
            const SHOWN: usize = 5;
            let detail = skipped
                .iter()
                .take(SHOWN)
                .map(|(id, status)| format!("{id} ({status})"))
                .collect::<Vec<_>>()
                .join(", ");
            let more = skipped.len().saturating_sub(SHOWN);
            let suffix = if more > 0 {
                format!(", +{more} more")
            } else {
                String::new()
            };
            anyhow::bail!(
                "no drivable item in the queue for {role_label}; nothing to drive — \
                 {} queued item{} in-flight or terminal: {detail}{suffix}",
                skipped.len(),
                if skipped.len() == 1 { "" } else { "s" },
            )
        }
    }
}

/// STORY-384: `aida queue recover <id>` — the failed-phase-1 recovery wizard.
///
/// Inspects the spec's recovery-relevant state (lease, branch, worktree, PR),
/// derives the recommended recovery path via the pure
/// [`queue_recover::recommend`], prints the inspection + recommendation, and —
/// unless `--dry-run` — steps through the recovery, confirming destructive ops
/// (push, PR-create, session end) unless `--auto`.
///
/// A FRONT-END over existing primitives: the probes reuse the same lease/PR/git
/// helpers the orchestrator and `aida session leases` use; the execution shells
/// out to `aida` subcommands (`queue work --from-pr`, `pull`, `session end`,
/// `queue add`) and `git` / `gh` rather than reimplementing them.
/// trace:STORY-384 | ai:claude
pub(crate) fn handle_queue_recover(
    storage: &Storage,
    user_id: &str,
    spec_query: &str,
    dry_run: bool,
    auto: bool,
) -> Result<()> {
    let project_root = find_project_root()?;

    // Resolve the spec id + status from the store (canonical SPEC-ID, status
    // for display). A spec we can't find still gets a best-effort recovery
    // using the query string verbatim, but we warn.
    let store = storage.load().ok();
    let (spec, status_label, spec_completed) = match store.as_ref().and_then(|s| {
        s.requirements.iter().find(|r| {
            r.spec_id.as_deref() == Some(spec_query)
                || r.agreed_id.as_deref() == Some(spec_query)
                || r.id.to_string() == spec_query
        })
    }) {
        Some(req) => {
            let id = req
                .agreed_id
                .clone()
                .or_else(|| req.spec_id.clone())
                .unwrap_or_else(|| spec_query.to_string());
            (
                id,
                req.status.to_string(),
                req.status == RequirementStatus::Completed,
            )
        }
        None => {
            eprintln!(
                "{} no spec matched `{}` in the store — probing git/PR state by the id verbatim.",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                spec_query
            );
            (spec_query.to_string(), "<unknown>".to_string(), false)
        }
    };

    // --- Inspection: probe the world for this spec. ---
    // Reuse the orchestrator's PR/branch/CI/reviewed/merged probe (the same
    // helper `--from-pr` and `--resume-drain` use). `member: None` is the
    // standalone-PR case it already handles. trace:STORY-384 | ai:claude
    let (facts, pr_branch, pr) = probe_resume_facts(&project_root, storage, &spec, None);

    // Lease state — does a session still hold this spec's scope?
    let leases = list_leases(&project_root);
    let lease = find_lease_by_spec(&spec, &leases).ok();
    let now = chrono::Utc::now();
    let live = process_probe::probe_live_claude_sessions();
    // BUG-511: review-verb leases classify by creator PID, not worktree.
    let lease_state = lease.as_ref().map(|l| lease_state_for(l, &live, now));

    // The branch we inspect for commits-ahead / dirty: prefer the lease's
    // branch (where a phase-1 implementer committed), else the PR head branch.
    let branch = lease
        .as_ref()
        .map(|l| l.branch.clone())
        .or_else(|| pr_branch.clone());

    // Commits ahead of origin/main on the branch (work that exists but may not
    // have shipped). Probe against the lease worktree if present, else root.
    let probe_repo = lease
        .as_ref()
        .map(|l| l.worktree_path.clone())
        .filter(|p| p.exists())
        .unwrap_or_else(|| project_root.clone());
    let commits_ahead = branch
        .as_deref()
        .and_then(|b| branch_commits_ahead_main(&probe_repo, b))
        .unwrap_or(0);

    // Branch pushed? A PR implies a pushed branch; otherwise probe origin.
    let branch_pushed = pr.is_some()
        || branch
            .as_deref()
            .map(|b| {
                matches!(
                    probe_branch_on_origin(&project_root, b),
                    BranchOriginProbe::Present
                )
            })
            .unwrap_or(false);

    // Worktree dirty? Only meaningful when a lease worktree exists.
    let dirty_entries = lease
        .as_ref()
        .filter(|l| l.worktree_path.exists())
        .map(|l| worktree_dirty_entries(&l.worktree_path))
        .unwrap_or_default();
    let uncommitted_changes = !dirty_entries.is_empty();

    let state = queue_recover::RecoverState {
        spec_completed: spec_completed || facts.spec_completed,
        pr_exists: pr.is_some(),
        pr_merged: facts.pr_merged,
        commits_ahead,
        branch_pushed,
        uncommitted_changes,
        lease_held: lease.is_some(),
    };

    // --- Print the inspection result. ---
    println!("🔎 recovery inspection for {}", spec.bold());
    println!("  {:<14} {}", "status".dimmed(), status_label);
    match (&lease, lease_state) {
        (Some(l), Some(st)) => println!(
            "  {:<14} {} {} (id {}, branch {})",
            "lease".dimmed(),
            st.glyph(),
            st.label(),
            &l.id[..l.id.len().min(8)],
            l.branch
        ),
        _ => println!("  {:<14} none", "lease".dimmed()),
    }
    match &branch {
        Some(b) => println!(
            "  {:<14} {} — {} commit(s) ahead of origin/main, {}",
            "branch".dimmed(),
            b,
            commits_ahead,
            if branch_pushed {
                "pushed"
            } else {
                "NOT pushed"
            }
        ),
        None => println!("  {:<14} none", "branch".dimmed()),
    }
    if uncommitted_changes {
        println!(
            "  {:<14} {} uncommitted change(s)",
            "worktree".dimmed(),
            dirty_entries.len()
        );
        for e in dirty_entries.iter().take(3) {
            println!("                   {}", e.dimmed());
        }
        if dirty_entries.len() > 3 {
            println!(
                "                   {} … and {} more",
                "".dimmed(),
                dirty_entries.len() - 3
            );
        }
    } else {
        println!("  {:<14} clean", "worktree".dimmed());
    }
    match pr {
        Some(n) if facts.pr_merged => println!("  {:<14} PR-{} (merged)", "pr".dimmed(), n),
        Some(n) => println!("  {:<14} PR-{} (open)", "pr".dimmed(), n),
        None => println!("  {:<14} none", "pr".dimmed()),
    }
    println!();

    // --- Recommendation. ---
    let action = queue_recover::recommend(&state);
    println!(
        "{} recommended: {}",
        "→".cyan().bold(),
        recover_action_label(action).bold()
    );
    println!("  {} {}", "rationale:".dimmed(), action.rationale());
    println!();

    if dry_run {
        println!(
            "{} --dry-run — not executing. Drop it to run the recovery.",
            crate::glyph(crate::glyphs::Glyph::Info).cyan()
        );
        return Ok(());
    }

    // --- Execution. ---
    let aida = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("aida"));
    // Helper: run an `aida` subcommand from the project root, surfacing its
    // exit code. Non-zero leaves the state visible for manual resume.
    let run_aida = |args: &[&str]| -> std::io::Result<std::process::ExitStatus> {
        println!(
            "  {} aida {}",
            crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
            args.join(" ")
        );
        std::process::Command::new(&aida)
            .current_dir(&project_root)
            .args(args)
            .status()
    };
    let run_git =
        |args: &[&str], cwd: &std::path::Path| -> std::io::Result<std::process::ExitStatus> {
            println!(
                "  {} git {}",
                crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
                args.join(" ")
            );
            std::process::Command::new("git")
                .arg("-C")
                .arg(cwd)
                .args(args)
                .status()
        };
    // Confirm a destructive op unless --auto. Non-interactive (no TTY) defaults
    // to NO so a scripted run without --auto never silently ships.
    let confirm = |what: &str| -> bool {
        if auto {
            return true;
        }
        prompt_yes_no(&format!("  {} {} [y/N] ", "?".yellow(), what), false).unwrap_or(false)
    };

    match action {
        queue_recover::RecoverAction::AlreadyCompleted => {
            println!(
                "{} `{}` is already Completed — nothing to recover.",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                spec
            );
            if state.lease_held && confirm("end the leaked lease for this spec?") {
                let _ = run_aida(&["session", "end", &spec]);
            }
        }
        queue_recover::RecoverAction::AlreadyMergedPull => {
            println!(
                "{} the PR merged — pulling to auto-bump `{}` Done → Completed.",
                "↩".cyan().bold(),
                spec
            );
            let _ = run_aida(&["pull"]);
            if state.lease_held && confirm("end the leaked lease for this spec?") {
                let _ = run_aida(&["session", "end", &spec]);
            }
        }
        queue_recover::RecoverAction::DrivePhasesFromPr => {
            if !confirm("drive phases 3-6 on the open PR (reviewer → merge → pull → build)?")
            {
                println!(
                    "{} skipped — state left intact.",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan()
                );
                return Ok(());
            }
            // Reuse the TASK-405 PR-only orchestrator path.
            let _ = run_aida(&["queue", "work", &spec, "--auto-complete", "--from-pr"]);
        }
        queue_recover::RecoverAction::PushOpenPrDrive => {
            let Some(b) = branch.as_deref() else {
                eprintln!(
                    "{} no branch to push for `{}` — cannot recover automatically.",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    spec
                );
                return Ok(());
            };
            if !confirm(&format!("push `{b}`, open a PR, then drive phases 3-6?")) {
                println!(
                    "{} skipped — state left intact.",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan()
                );
                return Ok(());
            }
            let push_st = run_git(&["push", "-u", "origin", b], &probe_repo)?;
            if !push_st.success() {
                eprintln!(
                    "{} push failed — resolve manually, then re-run `aida queue recover {}`.",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    spec
                );
                return Ok(());
            }
            // Open the change through the forge — title/body derived from the
            // branch commits (the forge-routed equivalent of `gh pr create
            // --fill`), so a GitLab/pure-git repo opens its own change.
            // trace:TASK-961 trace:STORY-621 | ai:claude
            if recover_open_change(&probe_repo, b) {
                // Now drive phases 3-6 on the freshly-opened change.
                let _ = run_aida(&["queue", "work", &spec, "--auto-complete", "--from-pr"]);
            } else {
                eprintln!(
                    "{} opening the {} failed — open it manually, then run \
                     `aida queue recover {}` again (it will take the drive-from-PR path).",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    crate::forge::resolve_forge_kind(&probe_repo).change_noun(),
                    spec
                );
            }
        }
        queue_recover::RecoverAction::WipCommitPushDrive => {
            if !confirm("commit the WIP, then push + open PR + drive phases 3-6?") {
                println!(
                    "{} skipped — commit/stash the WIP yourself, then re-run.",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan()
                );
                return Ok(());
            }
            let _ = run_git(&["add", "-A"], &probe_repo)?;
            let _ = run_git(
                &["commit", "-m", &format!("wip: recover {spec}")],
                &probe_repo,
            )?;
            let Some(b) = branch.as_deref() else {
                eprintln!(
                    "{} no branch to push.",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold()
                );
                return Ok(());
            };
            let push_st = run_git(&["push", "-u", "origin", b], &probe_repo)?;
            if !push_st.success() {
                eprintln!(
                    "{} push failed — resolve manually.",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold()
                );
                return Ok(());
            }
            // Forge-routed change open — title/body derived from the branch
            // commits. trace:TASK-961 trace:STORY-621 | ai:claude
            if recover_open_change(&probe_repo, b) {
                let _ = run_aida(&["queue", "work", &spec, "--auto-complete", "--from-pr"]);
            } else {
                eprintln!(
                    "{} opening the {} failed — open it manually, then re-run recover.",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    crate::forge::resolve_forge_kind(&probe_repo).change_noun()
                );
            }
        }
        queue_recover::RecoverAction::WipCommitPark => {
            println!(
                "{} preserving uncommitted work as a WIP commit (parked for resumption).",
                crate::glyph(crate::glyphs::Glyph::Info).cyan()
            );
            if !confirm("commit the WIP on the branch and park (no PR)?") {
                println!(
                    "{} skipped — state left intact.",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan()
                );
                return Ok(());
            }
            let _ = run_git(&["add", "-A"], &probe_repo)?;
            let _ = run_git(
                &["commit", "-m", &format!("wip: recover {spec} (parked)")],
                &probe_repo,
            )?;
            println!(
                "{} WIP committed. Resume later with `aida queue work {} --resume`.",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                spec
            );
        }
        queue_recover::RecoverAction::EndAndRequeue => {
            println!(
                "{} nothing was shipped — ending the lease and re-queueing `{}` for a fresh attempt.",
                "↩".cyan().bold(),
                spec
            );
            if !confirm("end the session and re-queue the spec?") {
                println!(
                    "{} skipped — state left intact.",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan()
                );
                return Ok(());
            }
            if state.lease_held {
                let _ = run_aida(&["session", "end", &spec]);
            }
            // Re-queue for the spec's owner. queue add is advisor-gated in
            // non-TTY contexts, so route the role explicitly.
            let _ = run_aida(&["queue", "add", &spec, "--user", user_id]);
        }
    }

    Ok(())
}

/// TASK-836: probe the richer pre-merge facts for ONE ready integration
/// candidate — CI rollup, RequestChanges (local verdict OR forge decision), and
/// mergeability — and normalize them into the pure [`integrate::PrIntegrationState`]
/// the pre-merge gate decides on. All the messy `gh`-string interpretation lives
/// here, at the probe boundary; the decision ([`integrate::classify_integration_action`])
/// stays pure + unit-tested.
///
/// Conservative: anything we cannot tell degrades to the optimistic value
/// (CI `None`, no RequestChanges, `Unknown` mergeable) — the `--from-pr` drive
/// re-gates CI + merge before the irreversible step, so "couldn't tell" never
/// ships something unsafe; it just doesn't pre-empt a park. trace:TASK-836
pub(crate) fn probe_pr_integration_state(
    project_root: &std::path::Path,
    spec_id: &str,
    branch: Option<&str>,
    snapshot: &OpenPrSnapshot,
) -> integrate::PrIntegrationState {
    // The forge row for this PR (keyed by head branch).
    let item = branch.and_then(|b| snapshot.by_branch.get(b));

    // CI: prefer the snapshot rollup ("pass"/"fail"/"pending"/"?"), normalized.
    let ci = match item.and_then(|i| i.ci_rollup.as_deref()) {
        Some("pass") => integrate::CiState::Passing,
        Some("fail") => integrate::CiState::Failing,
        Some("pending") => integrate::CiState::Running,
        _ => integrate::CiState::None,
    };

    // Mergeability: the forge's `mergeable` field — MERGEABLE / CONFLICTING /
    // UNKNOWN. "UNKNOWN" means GitHub hasn't computed it yet; treated optimistic.
    // A behind-base branch presents as CONFLICTING only on a real conflict, so
    // we can't distinguish BehindBase from the snapshot alone — the caller's
    // --rebase step covers the behind-base case, so mapping non-conflict to
    // Unknown here is sufficient. trace:TASK-836
    let mergeable = match item.and_then(|i| i.mergeable.as_deref()) {
        Some("MERGEABLE") => integrate::MergeableState::Mergeable,
        Some("CONFLICTING") => integrate::MergeableState::Conflicting,
        _ => integrate::MergeableState::Unknown,
    };

    // RequestChanges from the forge's reviewDecision (CHANGES_REQUESTED).
    let forge_request_changes = item
        .and_then(|i| i.review_decision.as_deref())
        .map(|d| d.eq_ignore_ascii_case("CHANGES_REQUESTED"))
        .unwrap_or(false);

    // RequestChanges from the LOCAL review-verdict file (the orchestrator's own
    // reviewer writes `.aida/review-verdicts/PR-N.json`). Either source is a
    // hard stop — never merge over a pending RequestChanges. trace:TASK-836
    let local_request_changes = item
        .map(|i| i.number)
        .map(|n| {
            let path = project_root
                .join(".aida")
                .join("review-verdicts")
                .join(format!("PR-{n}.json"));
            matches!(
                read_verdict_file(&path),
                Ok(auto_complete::ReviewerOutcome::Verdict(
                    auto_complete::Verdict::RequestChanges
                ))
            )
        })
        .unwrap_or(false);

    let _ = spec_id; // spec id is the caller's message prefix, not a probe input.
    integrate::PrIntegrationState {
        ci,
        request_changes_pending: forge_request_changes || local_request_changes,
        mergeable,
    }
}

/// STORY-520: `aida queue integrate` — the thin integrator watch-loop.
///
/// The consumer half of a producer/consumer split: parallel implementers
/// produce PRs + flip specs to Done; this single serial loop consumes that
/// signal (Done + open PR) and drives the back-end merge phases on each in turn.
///
/// A FRONT-END over existing primitives, not new mechanism: the ready-set query
/// reuses the store load + the forge PR/merged probes (`probe_resume_facts`),
/// the membership decision is the pure [`integrate::classify_candidate`], and
/// each ready spec is driven by shelling out to the SAME TASK-405 PR-only path
/// (`aida queue work <id> --auto-complete --from-pr`) the resume/recover flows
/// use. The serial-merge invariant (one merge authority over `main`) is
/// preserved by driving each spec to completion before the next.
/// trace:STORY-520 | ai:claude
// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
/// TASK-1036: build the focus subtree as a set of display SPEC-IDs — the focus
/// root (`focus_ref`, a spec/agreed/uuid form) plus its transitive descendants
/// (the cache's `descendant_ids` closure, TASK-955, which INCLUDES the root),
/// mapped to the same display-id form the integrator keys candidates on. Used to
/// scope the integrator `--watch` candidate scan and the event-wake filter.
///
/// Best-effort: an unresolvable store / focus spec / cache error yields `None`,
/// which the caller treats as "no scoping" (whole-project scan) rather than
/// blocking — a focus that no longer resolves must never wedge the integrator.
// trace:TASK-1036 | ai:claude
pub(crate) fn build_focus_display_subtree(
    project_root: &std::path::Path,
    focus_ref: &str,
    store: &aida_core::RequirementsStore,
) -> Option<std::collections::HashSet<String>> {
    let store_path = detect_distributed_store_from(project_root)?;
    let backend = advance_backend(&store_path).ok()?;
    let focus_req = backend
        .get_requirement_by_spec_id(focus_ref)
        .ok()
        .flatten()?;
    let uuid_subtree = backend.descendant_ids(&focus_req.id).ok()?;
    let display: std::collections::HashSet<String> = store
        .requirements
        .iter()
        .filter(|r| uuid_subtree.contains(&r.id))
        .map(|r| r.display_id())
        .collect();
    Some(display)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_queue_integrate(
    storage: &Storage,
    _user_id: &str,
    dry_run: bool,
    watch: bool,
    interval: u64,
    max: usize,
    rebase: bool,
    strategy: Option<integrate::IntegrateStrategy>,
    // TASK-1036: scope the candidate scan to a focus subtree. `--focus <id>`
    // overrides; else the per-worktree `aida focus` marker / `AIDA_FOCUS`.
    focus_override: Option<String>,
    // TASK-1036: idle backstop for the event-driven `--watch` loop, in minutes.
    // `None` falls back to `--interval` seconds (the old blind-timer cadence).
    idle_minutes: Option<u64>,
) -> Result<()> {
    let project_root = find_project_root()?;

    // TASK-812: take the global drain lock so an integrate run (especially
    // `--watch`, which a solo operator leaves running) can't double-drive the
    // tree against a `burndown run` / `queue work --auto-complete` / a second
    // `integrate` — they ALL merge into the shared default branch, so there can
    // be only one merge authority at a time. Held for the whole run, including
    // the `--watch` loop; a crashed watcher's lock is stale-reclaimed by the
    // next launch (BUG-538). `--dry-run` drives nothing, so it skips the lock.
    // The guard frees on return (this fn returns Result, never `process::exit`).
    // trace:TASK-812 | ai:claude
    let _drain_guard = if dry_run {
        None
    } else {
        Some(drain_lock::acquire_drain_lock(
            &project_root,
            "queue integrate",
        )?)
    };

    // TASK-691: resolve the accumulation strategy — the --strategy flag wins,
    // else the project default (`[integrate] strategy` in .aida/config.toml),
    // else `per-item`. trace:TASK-691 | ai:claude
    let strategy = strategy
        .or_else(|| {
            std::fs::read_to_string(project_root.join(".aida").join("config.toml"))
                .ok()
                .and_then(|c| integrate::integrate_strategy_from_config(&c))
        })
        .unwrap_or(integrate::IntegrateStrategy::PerItem);

    // STORY-335: only `per-item` is built; refuse `one-branch`/`stacked` cleanly
    // (with a pointer) before doing any probing or acting. trace:STORY-335
    if let Some(msg) = integrate::strategy_unsupported_message(strategy) {
        anyhow::bail!(msg);
    }

    let aida = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("aida"));

    // TASK-1036: resolve the active focus once — `--focus <id>` wins, else the
    // per-worktree `aida focus` marker / `AIDA_FOCUS` (STORY-706). The per-pass
    // subtree is re-resolved inside the loop (cache-backed, cheap) so newly-filed
    // descendants are picked up. trace:TASK-1036 | ai:claude
    let active_focus = focus_override.or_else(|| crate::focus::resolve_focus(&project_root));

    // TASK-1036: in `--watch`, wake on real drain events instead of a blind
    // timer. Seed the event cursor at the stream's CURRENT end so a stale backlog
    // from a prior drain never re-fires an old wake (the advisor_watch precedent).
    // The idle backstop is the documented timer fallback: `--idle-minutes` if
    // given, else the legacy `--interval` seconds. trace:TASK-1036 | ai:claude
    let events_path = crate::events::events_path(&project_root);
    let mut event_offset: u64 = std::fs::metadata(&events_path)
        .map(|m| m.len())
        .unwrap_or(0);
    let idle_backstop_secs = idle_minutes
        .map(|m| m.saturating_mul(60))
        .unwrap_or(interval);

    // TASK-1050 / BUG-650 (slice 2b): give `aida integrate` its OWN checkout so
    // it never contends the advisor's harness-worktree lease — the gate that
    // unblocks UNATTENDED `--watch`. (1) acquire/reuse a dedicated warm-pool
    // worktree pinned to the default branch under a distinct integrator lease
    // scope; (2) refuse to drive from — or relocate the drives out of — a shared
    // checkout a live non-integrator session holds (option-c). The merge drives
    // below run IN this dedicated checkout. `--dry-run` drives nothing, so it
    // skips both. The global drain lock taken above is untouched: it stays the
    // one merge authority repo-wide. trace:TASK-1050 trace:BUG-650 | ai:claude
    let integrator_checkout: Option<std::path::PathBuf> = if dry_run {
        None
    } else {
        let dedicated = integrate_checkout::ensure_integrator_checkout(&project_root)?;
        let cwd = std::env::current_dir().unwrap_or_else(|_| project_root.clone());
        integrate_checkout::guard_not_shared_checkout(&project_root, &cwd, &dedicated)?;
        Some(dedicated)
    };
    // The cwd for the spawned child drives: the dedicated integrator checkout
    // when we have one, else the project root (dry-run never spawns a real
    // drive). trace:TASK-1050 | ai:claude
    let drive_cwd: &std::path::Path = integrator_checkout
        .as_deref()
        .unwrap_or(project_root.as_path());

    let mut integrated_total: usize = 0;
    let mut pass: usize = 0;
    // TASK-836: track whether any member was parked (shelvable scenario) or made
    // to wait (CI running) across the whole run, so the exit code mirrors the
    // resilient-drain contract: exit 2 when anything was parked/skipped so a
    // wrapping script triages instead of treating the run as a clean success.
    // trace:TASK-836 | ai:claude
    let mut any_parked_or_waited = false;

    loop {
        pass += 1;

        // --- Probe: build the candidate set from store status + forge facts. ---
        // Reuse `probe_resume_facts` (the same PR/merged probe `--from-pr` and
        // `--resume-drain` use) so the integrator sees exactly the reality the
        // drive path will. trace:STORY-520 | ai:claude
        let store = storage.load()?;

        // TASK-1036: re-resolve the focus subtree this pass (cache-backed, cheap)
        // — the display-id set of the focus root + its transitive descendants —
        // so an out-of-scope Done spec is never probed or driven. `None` = no
        // focus set → whole-project scan, exactly as before. trace:TASK-1036
        let focus_subtree: Option<std::collections::HashSet<String>> = active_focus
            .as_deref()
            .and_then(|focus_ref| build_focus_display_subtree(&project_root, focus_ref, &store));
        if watch {
            if let (Some(focus_ref), Some(subtree)) = (active_focus.as_deref(), &focus_subtree) {
                println!(
                    "  {} focus {} — scanning {} spec(s) in subtree",
                    crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
                    focus_ref,
                    subtree.len()
                );
            }
        }

        let mut candidates: Vec<integrate::IntegrationCandidate> = Vec::new();
        // STORY-335: keep each candidate's PR branch (for the dry-run forecast)
        // and PR number (for the --rebase step) per ready member. trace:STORY-335
        let mut branches: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        let mut pr_numbers: std::collections::HashMap<String, Option<u32>> =
            std::collections::HashMap::new();
        for req in &store.requirements {
            if req.status != RequirementStatus::Done {
                continue;
            }
            let id = req
                .agreed_id
                .as_deref()
                .or(req.spec_id.as_deref())
                .unwrap_or("?")
                .to_string();
            // TASK-1036: focus-scope the scan BEFORE the (forge) probe, so an
            // out-of-scope Done spec is never probed, reported, or driven. Pure
            // `integrate::in_focus_scope` over the display-id subtree set.
            // trace:TASK-1036 | ai:claude
            if let Some(subtree) = &focus_subtree {
                if !integrate::in_focus_scope(&id, subtree) {
                    continue;
                }
            }
            // Classify the PR lookup as conclusive-or-not BEFORE probing the
            // richer facts, so a flaky gh never gets read as "no PR".
            let lookup = detect_open_pr_for_spec_via_forge(&project_root, &id);
            let inconclusive = matches!(
                lookup,
                PrLookup::GhMissing | PrLookup::GhFailed(_) | PrLookup::GhUnreachable(_)
            );
            let (facts, branch, pr) = probe_resume_facts(&project_root, storage, &id, None);
            branches.insert(id.clone(), branch);
            pr_numbers.insert(id.clone(), pr);
            candidates.push(integrate::IntegrationCandidate {
                id,
                is_done: true,
                has_open_pr: pr.is_some(),
                pr_merged: facts.pr_merged,
                pr_lookup_inconclusive: inconclusive,
                // TASK-813: keystone work the integrator must NOT auto-merge.
                held_for_human: req.tags.iter().any(|t| {
                    let t = t.trim();
                    t.eq_ignore_ascii_case("supervised")
                        || t.eq_ignore_ascii_case("review:draft-only")
                }),
            });
        }

        let ready = integrate::ready_for_integration(&candidates);

        // TASK-836: one forge snapshot per pass (CI rollup + mergeable +
        // reviewDecision keyed by branch) feeds the pre-merge scenario gate
        // below. Empty when gh is missing/failing — the gate then degrades to
        // the optimistic "Merge" verdict, and the --from-pr drive re-gates the
        // irreversible step. trace:TASK-836 | ai:claude
        let mut pr_snapshot = if dry_run || !ready.is_empty() {
            collect_open_prs(&project_root)
        } else {
            OpenPrSnapshot::default()
        };

        // --- Report the pass. ---
        if watch {
            println!(
                "{} integrator pass {} — {} Done spec(s), {} ready for integration",
                crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold(),
                pass,
                candidates.len(),
                ready.len()
            );
        } else {
            println!(
                "{} {} Done spec(s); {} ready for integration",
                crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold(),
                candidates.len(),
                ready.len()
            );
        }
        for c in &candidates {
            match integrate::classify_candidate(c) {
                integrate::CandidateVerdict::Integrate => {
                    println!("  {} {} — open PR, ready to merge", "→".green(), c.id);
                }
                integrate::CandidateVerdict::SkipNoPr => {
                    println!("  {} {} — Done but no open PR (skip)", "·".dimmed(), c.id);
                }
                integrate::CandidateVerdict::SkipAlreadyMerged => {
                    println!(
                        "  {} {} — PR already merged; `aida pull` will promote it (skip)",
                        "·".dimmed(),
                        c.id
                    );
                }
                integrate::CandidateVerdict::SkipProbeInconclusive => {
                    println!(
                        "  {} {} — PR probe inconclusive (gh missing/auth/network); skipping, not guessing",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                        c.id
                    );
                }
                integrate::CandidateVerdict::SkipHeldForHuman => {
                    println!(
                        "  {} {} — keystone (supervised / review:draft-only): parked for your review, not auto-merged",
                        "⏸".yellow(),
                        c.id
                    );
                }
                // Non-Done specs were filtered out before classification.
                integrate::CandidateVerdict::SkipNotDone => {}
            }
        }

        // --- Act: drive each ready spec, serially, through the PR-only path. ---
        let mut ready_ids: Vec<String> = ready.iter().map(|c| c.id.clone()).collect();

        // TASK-841: `--strategy stacked` — order + gate the ready set by the
        // recorded stack graph. A member stacked behind another still-open branch
        // is DEFERRED this pass (merging it now would drag the parent's
        // un-squashed commits in under the wrong PR); only the mergeable
        // bottom-of-stack layer is driven. Completion stays per-commit (each
        // member auto-bumps when its own commit lands), so no special barrier is
        // needed — just this ordering. The drive's `aida pull` then cascade-
        // rebases the next layer's worktree (STORY-248); a later `--watch` pass
        // (or re-run) picks it up. trace:TASK-841 | ai:claude
        if matches!(strategy, integrate::IntegrateStrategy::Stacked) && !ready_ids.is_empty() {
            let mut graph = stacks::load(&project_root);
            let default_short = detect_default_branch_ref(&project_root)
                .as_deref()
                .map(|s| s.strip_prefix("origin/").unwrap_or(s).to_string())
                .unwrap_or_else(|| "main".to_string());
            let plan =
                integrate::plan_stacked_integration(&ready_ids, &branches, &graph, &default_short);
            // TASK-1080: stack-aware promotion. A deferred member whose parent
            // branch is GONE on origin (squash-merged + deleted) is promoted in
            // place: its ORIGIN PR branch is rebased with the 3-arg
            // `git rebase --onto <default> <recorded fork SHA>` + force-pushed
            // with lease — composing the one `pr rebase` machinery (temp
            // worktree, BUG-640 patch-id guard, lease-anchored push) via
            // `--onto-parent` — then merged by the normal drive THIS pass. The
            // STORY-248 cascade only rebases the child's local worktree; this
            // closes the origin-PR half so a 2-deep stack drains without a
            // manual `/aida-rebase`. The decision per member is the pure
            // `integrate::classify_stacked_promotion`. trace:TASK-1080 | ai:claude
            let mut promoted: Vec<String> = Vec::new();
            let mut still_deferred = 0usize;
            for d in &plan.deferred {
                if let Some(parent_spec) = &d.blocked_on_spec {
                    // Parent is in THIS pass's ready set — it merges below; the
                    // next pass (or --watch wake) promotes this child.
                    println!(
                        "  {} {} — stacked behind {} (branch `{}`); deferring until it lands",
                        "⏸".yellow(),
                        d.id,
                        parent_spec,
                        d.blocked_on_branch
                    );
                    still_deferred += 1;
                    continue;
                }
                let child_branch = branches.get(&d.id).and_then(|b| b.clone());
                let recorded_sha = child_branch
                    .as_deref()
                    .and_then(|b| graph.get(b))
                    .map(|e| e.parent_branch_sha.clone());
                let parent_gone = remote_branch_gone(&project_root, &d.blocked_on_branch);
                let pr_num = pr_numbers.get(&d.id).and_then(|p| *p);
                match integrate::classify_stacked_promotion(
                    recorded_sha.as_deref(),
                    parent_gone,
                    pr_num.is_some(),
                ) {
                    integrate::StackedPromotion::Promote { parent_sha } => {
                        let pr = pr_num.expect("Promote implies a resolved PR number");
                        if dry_run {
                            println!(
                                "  {} {} — parent `{}` merged+deleted; [dry-run] would rebase PR #{} onto {} (stack-aware) and force-push-with-lease",
                                "↻".cyan(),
                                d.id,
                                d.blocked_on_branch,
                                pr,
                                default_short
                            );
                            promoted.push(d.id.clone());
                            continue;
                        }
                        println!(
                            "  {} {} — parent `{}` merged+deleted; promoting: stack-aware rebase of PR #{} onto {}…",
                            "↻".cyan(),
                            d.id,
                            d.blocked_on_branch,
                            pr,
                            default_short
                        );
                        let status = std::process::Command::new(&aida)
                            .current_dir(drive_cwd)
                            .args(integrate::promotion_rebase_args(pr, &parent_sha))
                            .status();
                        match status {
                            Ok(s) if s.success() => {
                                // The child now forks straight from the default
                                // branch — drop its stack entry so no later pass
                                // or cascade replays the consumed fork record.
                                // Dependents (grandchildren) keep their own
                                // records: their recorded fork SHA is still an
                                // ancestor of THEIR untouched PR head, so their
                                // eventual promotion stays correct.
                                if let Some(b) = child_branch.as_deref() {
                                    stacks::remove(&mut graph, b);
                                    if let Err(e) = stacks::save(&project_root, &graph) {
                                        eprintln!(
                                            "  {} could not update .aida/stacks.json: {e}",
                                            "Note:".dimmed()
                                        );
                                    }
                                }
                                println!(
                                    "  {} {} promoted — PR #{} rebased + force-pushed; merging this pass",
                                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                                    d.id,
                                    pr
                                );
                                promoted.push(d.id.clone());
                            }
                            Ok(_) => {
                                // Shelvable failure — park + continue (resilient-
                                // drain contract). Never corrupts: `pr rebase`
                                // aborts a conflicted rebase and cleans its temp
                                // worktree; nothing was pushed.
                                println!(
                                    "  {} {} — stack-aware rebase failed (conflict?); parking and continuing",
                                    "⏸".yellow(),
                                    d.id
                                );
                                if let Err(e) = shelve_spec_on_failure(
                                    &project_root,
                                    &d.id,
                                    "integrate",
                                    0,
                                    "stacked-rebase",
                                    &format!(
                                        "stack-aware rebase of PR #{pr} onto {default_short} failed (likely a conflict replaying the stacked commits)"
                                    ),
                                    &format!(
                                        "resolve with `aida pr rebase {pr} --onto-parent {parent_sha}` (or add `--interactive`), then re-run `aida queue integrate --strategy stacked`"
                                    ),
                                ) {
                                    eprintln!(
                                        "  {} could not park {}: {e}",
                                        "Note:".dimmed(),
                                        d.id
                                    );
                                }
                                any_parked_or_waited = true; // trace:TASK-836
                            }
                            Err(e) => {
                                println!(
                                    "  {} {} — could not launch the stack-aware rebase ({e}); deferring",
                                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                                    d.id
                                );
                                still_deferred += 1;
                            }
                        }
                    }
                    integrate::StackedPromotion::ParentStillOpen => {
                        println!(
                            "  {} {} — stacked behind `{}` (parent not merged yet); deferring until it lands",
                            "⏸".yellow(),
                            d.id,
                            d.blocked_on_branch
                        );
                        still_deferred += 1;
                    }
                    integrate::StackedPromotion::ProbeInconclusive => {
                        println!(
                            "  {} {} — stacked behind `{}`; couldn't verify the parent branch on origin (offline?); deferring, not guessing",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                            d.id,
                            d.blocked_on_branch
                        );
                        still_deferred += 1;
                    }
                    integrate::StackedPromotion::ChurnedNoEntry => {
                        // Churn guard: the cascade removed the stack entry after
                        // this pass's plan was computed — defer and let the next
                        // re-plan classify it (it will read as mergeable).
                        println!(
                            "  {} {} — its stack record vanished mid-pass; deferring (next pass re-plans)",
                            "·".dimmed(),
                            d.id
                        );
                        still_deferred += 1;
                    }
                    integrate::StackedPromotion::NoPrNumber => {
                        println!(
                            "  {} {} — parent `{}` merged but no PR number resolved; run `/aida-rebase` in its worktree, then re-run",
                            "⏸".yellow(),
                            d.id,
                            d.blocked_on_branch
                        );
                        still_deferred += 1;
                    }
                }
            }
            if still_deferred > 0 {
                // Mirror the resilient-drain contract: something was held back, so
                // the run exits 2 and a wrapping loop (or `--watch`) re-checks
                // after the bottom layer merges. trace:TASK-836 | ai:claude
                any_parked_or_waited = true;
            }
            ready_ids = plan.mergeable;
            ready_ids.extend(promoted.iter().cloned());
            // A promoted child's snapshot row predates the force-push — refresh
            // so the TASK-836 pre-merge gate judges the NEW head, not the stale
            // stacked one. trace:TASK-1080 | ai:claude
            if !promoted.is_empty() && !dry_run {
                pr_snapshot = collect_open_prs(&project_root);
            }
        }

        // STORY-335: dry-run rebase-conflict forecast. Each ready member's PR
        // branch is checked (read-only, via `git merge-tree`) against current
        // main — surfacing which WILL conflict before any merge is attempted.
        // Read-only first slice: the act path below is unchanged (no rebase step
        // yet). trace:STORY-335 | ai:claude
        if dry_run && !ready_ids.is_empty() {
            let base = detect_default_branch_ref(&project_root);
            let rows: Vec<integrate::ForecastRow> = ready_ids
                .iter()
                .map(|id| {
                    let forecast = match (&base, branches.get(id).and_then(|b| b.clone())) {
                        (Some(base_ref), Some(branch)) => {
                            forecast_rebase_onto(&project_root, base_ref, &branch)
                        }
                        (None, _) => integrate::RebaseForecast::Unknown(
                            "no default branch (origin/main) detected".to_string(),
                        ),
                        (_, None) => integrate::RebaseForecast::Unknown(
                            "no PR branch resolved for spec".to_string(),
                        ),
                    };
                    integrate::ForecastRow {
                        id: id.clone(),
                        forecast,
                    }
                })
                .collect();
            let base_label = base.as_deref().unwrap_or("main");
            println!(
                "\n{} Rebase forecast (each PR branch onto {}, in order):",
                crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold(),
                base_label
            );
            for r in &rows {
                match &r.forecast {
                    integrate::RebaseForecast::Clean => {
                        println!(
                            "    {} {} — clean",
                            crate::glyph(crate::glyphs::Glyph::Check).green(),
                            r.id
                        );
                    }
                    integrate::RebaseForecast::Conflict(files) => {
                        let detail = if files.is_empty() {
                            String::new()
                        } else {
                            format!(": {}", files.join(", "))
                        };
                        println!(
                            "    {} {} — conflict{}",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                            r.id,
                            detail
                        );
                    }
                    integrate::RebaseForecast::Unknown(why) => {
                        println!("    {} {} — unknown ({})", "?".dimmed(), r.id, why);
                    }
                }
            }
            let s = integrate::summarize_forecast(&rows);
            if s.conflict > 0 {
                println!(
                    "  {} {} of {} will conflict — resolve {} first.",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                    s.conflict,
                    rows.len(),
                    s.conflicting_ids.join(", ")
                );
            } else if s.unknown > 0 {
                println!(
                    "  {} {} clean, {} indeterminate — re-check those before integrating.",
                    crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
                    s.clean,
                    s.unknown
                );
            } else {
                println!(
                    "  {} all {} forecast clean.",
                    crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                    s.clean
                );
            }
            println!(
                "  {} forecast checks each branch against current {} independently; a member that\n    only conflicts with an earlier un-landed batch member won't show here yet (follow-up).",
                "·".dimmed(),
                base_label
            );
        }

        // TASK-842: recognize the full set of specs each ready PR will complete,
        // and dedupe by PR number across the ready set so a multi-spec cluster PR
        // — surfaced once per member spec by the Done-spec scan — is driven ONCE,
        // not N times. The merge-trailer → auto-bump path still completes every
        // trailered spec; this is recognition + dedupe + reporting only. The
        // dedupe itself is the pure `integrate::dedupe_pr_completions` over
        // (pr_number, trailered-spec-ids) rows, so it's unit-tested independently.
        // trace:TASK-842 | ai:claude
        let completion_rows: Vec<(u64, Vec<String>)> = ready_ids
            .iter()
            .filter_map(|id| {
                let pr_num = pr_numbers.get(id).and_then(|p| *p)? as u64;
                // The PR's trailered spec set, from the snapshot title (AIDA PR
                // titles carry the `(SPEC-ID)` trailers); fall back to the spec
                // the scan keyed on when the title can't be read.
                let spec_ids = branches
                    .get(id)
                    .and_then(|b| b.as_deref())
                    .and_then(|b| pr_snapshot.by_branch.get(b))
                    .map(|it| extract_spec_ids_from_commit(&it.title))
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| vec![id.clone()]);
                Some((pr_num, spec_ids))
            })
            .collect();
        let pr_completions = integrate::dedupe_pr_completions(&completion_rows);
        let multi_spec_pr: std::collections::HashMap<u64, integrate::PrCompletion> = pr_completions
            .into_iter()
            .filter(|c| c.spec_ids.len() > 1)
            .map(|c| (c.number, c))
            .collect();
        // Track which PR numbers we've already driven this pass — so a multi-spec
        // PR reached again via a sibling spec isn't re-driven. trace:TASK-842
        let mut integrated_pr_numbers: std::collections::HashSet<u32> =
            std::collections::HashSet::new();

        for id in &ready_ids {
            if max != 0 && integrated_total >= max {
                println!(
                    "{} reached --max {} this run; stopping.",
                    crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
                    max
                );
                return Ok(());
            }
            // TASK-836: pre-merge scenario gate. Probe the richer PR facts (CI
            // state, RequestChanges, mergeability) and decide handle-vs-park
            // BEFORE driving the merge. Reuses the resilient-drain
            // park-and-continue contract: a shelvable scenario (CI red,
            // RequestChanges, conflict) parks the spec with one legible line and
            // the loop continues; CI-running waits (re-decided next --watch
            // pass). trace:TASK-836 | ai:claude
            let branch = branches.get(id).and_then(|b| b.clone());
            let pr_state =
                probe_pr_integration_state(&project_root, id, branch.as_deref(), &pr_snapshot);
            match integrate::classify_integration_action(&pr_state) {
                integrate::IntegrationAction::Park(reason) => {
                    println!("  {} {} — {}", "⏸".yellow(), id, reason.message());
                    any_parked_or_waited = true;
                    continue;
                }
                integrate::IntegrationAction::WaitCi => {
                    println!(
                        "  {} {} — CI still running; skipping this pass (will re-check)",
                        crate::glyph(crate::glyphs::Glyph::Hourglass).yellow(),
                        id
                    );
                    any_parked_or_waited = true;
                    continue;
                }
                integrate::IntegrationAction::Merge => {}
            }

            // TASK-843: a spec may have MORE than one open PR (a reopened /
            // duplicate). Before driving, list every open PR referencing the spec
            // and apply the pure newest-canonical policy: pick the newest
            // mergeable PR, report the rest as ignored-this-pass, or PARK when
            // none are clean. When only one PR is found this is a no-op (the
            // common case). The forge list is best-effort — an empty result (gh
            // missing/failing, or no title/body match) falls back to the existing
            // single-PR drive path. trace:TASK-843 | ai:claude
            let multi_prs = all_open_prs_for_spec_via_forge(&project_root, id);
            if multi_prs.len() > 1 {
                // Mergeability per candidate from the pass snapshot (CONFLICTING
                // → not mergeable; anything else admissible — the --from-pr drive
                // re-gates the irreversible merge).
                let candidates: Vec<integrate::PrCandidate> = multi_prs
                    .iter()
                    .map(|(number, branch)| {
                        let mergeable = pr_snapshot
                            .by_branch
                            .get(branch)
                            .and_then(|it| it.mergeable.as_deref())
                            .map(|m| !m.eq_ignore_ascii_case("CONFLICTING"))
                            .unwrap_or(true);
                        integrate::PrCandidate {
                            number: *number,
                            mergeable,
                        }
                    })
                    .collect();
                match integrate::select_canonical_pr(&candidates) {
                    integrate::CanonicalPrDecision::Park { candidates } => {
                        let list = candidates
                            .iter()
                            .map(|n| format!("#{n}"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        println!(
                            "  {} {} — {} open PRs but none mergeable ({}); parked for triage",
                            "⏸".yellow(),
                            id,
                            candidates.len(),
                            list
                        );
                        any_parked_or_waited = true;
                        continue;
                    }
                    integrate::CanonicalPrDecision::Integrate { chosen, ignored } => {
                        if !ignored.is_empty() {
                            let list = ignored
                                .iter()
                                .map(|n| format!("#{n}"))
                                .collect::<Vec<_>>()
                                .join(", ");
                            println!(
                                "  {} {} — {} open PRs; integrating newest (#{}), ignoring this pass: {}",
                                crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
                                id,
                                ignored.len() + 1,
                                chosen,
                                list
                            );
                        }
                    }
                    // 0 or 1 candidate is handled by the len()>1 guard above.
                    integrate::CanonicalPrDecision::NoPr => {}
                }
            }

            // TASK-842: dedupe drive + emit the multi-spec completion line. The
            // recognition + dedupe were computed (purely) before the loop; here
            // we (a) skip re-driving a PR already driven this pass for a sibling
            // spec, and (b) emit one legible line naming every spec the merge
            // completes. trace:TASK-842 | ai:claude
            if let Some(pr_num) = pr_numbers.get(id).and_then(|p| *p) {
                if !integrated_pr_numbers.insert(pr_num) {
                    // Already driven this pass for a sibling spec — the
                    // merge-trailer → auto-bump completes this spec too; don't
                    // re-drive the same PR.
                    println!(
                        "  {} {} — same PR (#{}) already integrating this pass for a sibling spec (skip re-drive)",
                        "·".dimmed(),
                        id,
                        pr_num
                    );
                    continue;
                }
                if let Some(completion) = multi_spec_pr.get(&(pr_num as u64)) {
                    println!(
                        "  {} {}",
                        "↩".cyan(),
                        integrate::describe_pr_completion(completion)
                    );
                }
            }

            if dry_run {
                let rebase_note = if rebase { " (would rebase first)" } else { "" };
                println!(
                    "  {} [dry-run] would drive `{}` via `aida queue work {} --auto-complete --from-pr`{}",
                    "→".dimmed(),
                    id,
                    id,
                    rebase_note
                );
                continue;
            }

            // STORY-335: rebase this member's PR branch onto current main before
            // merging it. A deferred batch cuts every branch from the same stale
            // main, so without this they would merge un-rebased. Composes the
            // existing `aida pr rebase <N> --no-smoke` primitive (temp-worktree
            // rebase + force-push-with-lease). A rebase conflict/failure skips
            // this member and continues — punt-and-continue, like the resilient
            // drain; the slice-1 --dry-run forecast previews these. The local
            // smoke is skipped because the --from-pr drive below runs CI.
            // trace:STORY-335 | ai:claude
            if rebase {
                match pr_numbers.get(id).and_then(|p| *p) {
                    Some(pr) => {
                        println!(
                            "{} rebasing `{}` (PR #{}) onto current main…",
                            "↻".cyan(),
                            id,
                            pr
                        );
                        // TASK-1050/BUG-650: rebase in the integrator's own
                        // checkout, not the (possibly advisor-leased) launch
                        // worktree. trace:TASK-1050 | ai:claude
                        let rb = std::process::Command::new(&aida)
                            .current_dir(drive_cwd)
                            .args(build_integrate_rebase_args(pr))
                            .status();
                        match rb {
                            Ok(s) if s.success() => {
                                println!(
                                    "  {} `{}` rebased onto main",
                                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                                    id
                                );
                            }
                            Ok(_) => {
                                println!(
                                    "  {} `{}` rebase failed (conflict?) — skipping; resolve with `aida pr rebase {}` then re-run",
                                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                                    id,
                                    pr
                                );
                                any_parked_or_waited = true; // trace:TASK-836
                                continue;
                            }
                            Err(e) => {
                                println!(
                                    "  {} `{}` rebase could not run ({e}) — skipping",
                                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                                    id
                                );
                                any_parked_or_waited = true; // trace:TASK-836
                                continue;
                            }
                        }
                    }
                    None => {
                        // Defensive: ready members have an open PR, so a number
                        // should resolve. If it somehow doesn't, integrate as-is
                        // rather than skip — the --from-pr drive still gates on CI.
                        println!(
                            "  {} `{}` — --rebase set but no PR number resolved; integrating without rebase",
                            "·".dimmed(),
                            id
                        );
                    }
                }
            }

            println!(
                "{} integrating `{}` (reviewer → CI → merge → pull → build)…",
                "↩".cyan().bold(),
                id
            );
            // TASK-1050/BUG-650: drive in the integrator's OWN checkout so the
            // merge work never sits in (or lease-contends) a live session's
            // worktree. Because the drive runs in a linked worktree, its
            // `find_main_worktree_root()` drain lock targets MAIN — the very lock
            // THIS integrate run already holds. Authorize the delegated sub-drive
            // past that self-conflict with the force escape (env-scoped to the
            // child only); a genuinely independent drain never inherits it and is
            // still refused, so the repo-wide one-authority invariant holds.
            // trace:TASK-1050 trace:BUG-650 | ai:claude
            // ONE per-spec orchestration engine (ADR-7): integrate hands the
            // spec to the SAME `--auto-complete` engine, entering at the
            // reviewer phase via `--from-pr`. The routing argv is a pure helper
            // so the `orchestration_routing` guardrail can assert it.
            // trace:ADR-7 trace:ADR-9 | ai:claude
            let status = std::process::Command::new(&aida)
                .current_dir(drive_cwd)
                .env("AIDA_DRAIN_FORCE", "1")
                // BUG-748: this is an internal child drive launched under the
                // integrator's existing drain lock. Borrow that parent lock so
                // the child does not overwrite and release `.aida/drain.lock`
                // before the parent loop finishes. trace:BUG-748 | ai:codex
                .env("AIDA_DRAIN_BORROW", "1")
                .args(integrate::drive_args(id))
                .status();
            match status {
                Ok(s) if s.success() => {
                    integrated_total += 1;
                    println!(
                        "  {} `{}` integrated",
                        crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                        id
                    );
                }
                Ok(s) => {
                    // A non-zero exit means the drive shelved/refused this spec
                    // (CI red, RequestChanges, already-merged race, …). Leave it
                    // visible and move on — the serial loop must not stall on one
                    // spec. trace:STORY-520 | ai:claude
                    eprintln!(
                        "  {} `{}` did not integrate cleanly (exit {}); leaving for triage and continuing",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                        id,
                        s.code().unwrap_or(-1)
                    );
                    any_parked_or_waited = true; // trace:TASK-836
                }
                Err(e) => {
                    eprintln!(
                        "  {} failed to launch the drive for `{}`: {}",
                        crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                        id,
                        e
                    );
                    any_parked_or_waited = true; // trace:TASK-836
                }
            }
        }

        if !watch {
            break;
        }
        if max != 0 && integrated_total >= max {
            break;
        }
        // TASK-1036: event-driven wake. Instead of a blind `interval` sleep, BLOCK
        // until a focus-relevant actionable drain event lands in
        // `.aida/events.jsonl` (PhaseDonePr / CiTerminal / PrMerged / SpecShelved
        // — the same `events::is_actionable` taxonomy `aida watch` and the advisor
        // loop use), the idle backstop elapses (the old blind timer, now the
        // documented fallback — exactly like `advisor_watch::plan_watch_tick`'s
        // cadence path), or a live drain we were following stops. The store reload
        // at the top of the loop then picks up whatever shipped. trace:TASK-1036
        match event_wait::wait_for_actionable(
            &project_root,
            &mut event_offset,
            idle_backstop_secs,
            focus_subtree.as_ref(),
        ) {
            event_wait::WakeReason::Event(kind) => {
                let label = match &kind {
                    crate::events::EventKind::PhaseDonePr { .. } => "a PR is ready",
                    crate::events::EventKind::CiTerminal { .. } => "CI reached a verdict",
                    crate::events::EventKind::PrMerged { .. } => "a PR merged",
                    crate::events::EventKind::SpecShelved { .. } => "a spec shelved",
                    _ => "a drain event",
                };
                println!(
                    "  {} woke on {} — rescanning",
                    crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
                    label
                );
            }
            event_wait::WakeReason::DrainCrashed => {
                println!(
                    "  {} the drain we were following stopped streaming events — rescanning",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
            }
            event_wait::WakeReason::IdleBackstop => {
                // Quiet idle re-scan — same cadence the old blind timer gave.
            }
        }
    }

    // TASK-836: mirror the resilient-drain exit-code contract — exit 2 when any
    // member was parked (CI red / RequestChanges / conflict / rebase-skip /
    // drive-refused) or made to wait (CI running), so a wrapping script (the
    // solo loop) treats the run as "did its job but triage pending" rather than
    // a clean success. Dry-run never mutates, so it always exits 0. The drain
    // guard is RAII (Drop), and process::exit skips destructors — so drop it
    // explicitly first to release the lock cleanly. trace:TASK-836 | ai:claude
    if !dry_run && any_parked_or_waited {
        println!(
            "{} integration left member(s) parked/waiting — triage with `aida findings list`, then re-run.",
            crate::glyph(crate::glyphs::Glyph::Arrow).yellow().bold()
        );
        drop(_drain_guard);
        std::process::exit(2);
    }

    Ok(())
}

/// STORY-384: short human label for a recovery action (the recommendation
/// headline). The longer "why" is [`queue_recover::RecoverAction::rationale`].
/// trace:STORY-384 | ai:claude
pub(crate) fn recover_action_label(action: queue_recover::RecoverAction) -> &'static str {
    use queue_recover::RecoverAction as A;
    match action {
        A::AlreadyCompleted => "already completed — nothing to recover",
        A::AlreadyMergedPull => "pull to auto-bump the merged PR",
        A::DrivePhasesFromPr => "drive phases 3-6 on the open PR",
        A::PushOpenPrDrive => "push + open PR + drive phases 3-6",
        A::WipCommitPushDrive => "commit WIP + push + open PR + drive phases 3-6",
        A::WipCommitPark => "commit WIP and park for resumption",
        A::EndAndRequeue => "end the lease and re-queue",
    }
}
