// trace:TASK-3426 | ai:codex
use super::*;

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
            store.get_requirement_unambiguous(target)? // trace:TASK-1468 | ai:claude
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

        let choice = match crate::prompt_select::select(
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

/// BUG-1147: queue GC and fleet queue rendering need to resolve archived
/// targets before deciding whether to hide, label, or prune them. The default
/// list lens hides archived/deferred rows, which makes archived corpses look
/// like unknown live specs.
// trace:BUG-1147 | ai:codex
pub(crate) fn queue_dead_target_summary_filter() -> aida_core::ListFilter {
    aida_core::ListFilter {
        archive: aida_core::ArchiveFilter::Both,
        defer: aida_core::DeferFilter::Both,
        ..aida_core::ListFilter::default()
    }
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

    let aida = crate::aida_exe_path();
    let project_root = store_path.parent().map(|p| p.to_path_buf());

    match action {
        AdvanceAction::Review => {
            // In-process review (handles the closed-PR reopen offer itself,
            // AC-4/AC-5). On the operator's confirmation that it was approved,
            // offer to drop `review:draft-only` so it drains.
            let backend = advance_backend(store_path)?;
            if let Err(e) = handle_review_spec(
                &backend, store_path, display, /* no_agent */ false,
                /* allow_stale_base */ false, /* target_branch */ None,
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
                // trace:STORY-809 | ai:claude
                let card = crate::context_prompt::ContextCard {
                    decision: format!(
                        "whether {display} is operator-approved, clearing its `review:draft-only` hold"
                    ),
                    provenance: vec![format!(
                        "{display} carries `review:draft-only` — filed as a draft awaiting an explicit approval decision"
                    )],
                    answers: vec![
                        "y: drop the tag — the spec becomes drainable by autonomous runs".to_string(),
                        "n: keep the hold; nothing changes (re-runnable any time)".to_string(),
                    ],
                    recommended_default: "n — only approve when you have actually reviewed the spec text".to_string(),
                };
                let approved = crate::context_prompt::confirm_with_context(
                    &format!("Was {display} approved? (drop `review:draft-only` so it drains)"),
                    false,
                    &card,
                )
                .unwrap_or(false);
                if approved {
                    let mut req = backend
                        .get_requirement_unambiguous(display)? // trace:TASK-1468 | ai:claude
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
            // trace:BUG-1735 | ai:claude
            let status = cmd
                .args(["queue", "work", display, "--zen"])
                .status_retrying_etxtbsy();
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
            // trace:BUG-1735 | ai:claude
            let status = cmd
                .args(["queue", "work", display])
                .status_retrying_etxtbsy();
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
                .get_requirement_unambiguous(display)? // trace:TASK-1468 | ai:claude
                .ok_or_else(|| not_found::requirement_not_found(display, Some(store_path)))?;
            // Same advisor-authority gate as `aida edit` / `aida queue add`:
            // approving is an advisor call. Closes the `--yes` side-door where a
            // non-advisor / non-TTY run could auto-approve drafts. trace:STORY-566
            let new_status = RequirementStatus::Approved;
            if status_advance_requires_advisor_authority(&req.status, &new_status)
                && !crate::gate::role_authority::has_advisor_authority()
            {
                println!(
                    "  {} approving {} needs advisor authority. Enter the advisor role at an interactive TTY with `aida role enter advisor`, or ask an advisor session to do it.{}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    display.bold(),
                    roleless_recovery_sentence()
                );
                return Ok(());
            }
            // BUG-1637: through the one shared history helper, under the
            // caller (same identity this path used before). trace:BUG-1637 | ai:claude
            let old = req.status.clone();
            req.set_status_from_str("Approved");
            aida_core::conflict::record_status_transition(&mut req, &current_user_id(None), &old);
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
                // trace:STORY-809 | ai:claude
                let card = crate::context_prompt::ContextCard {
                    decision: format!("whether to move {display} to Rejected (declined — nothing adopted)"),
                    provenance: vec![format!(
                        "{display} is parked and the advance flow judged rejection the plausible resolution"
                    )],
                    answers: vec![
                        "y: status → Rejected; it leaves every open/candidate view (reversible via `aida edit --status`)".to_string(),
                        "n: leave it parked as-is".to_string(),
                    ],
                    recommended_default: "n — reject only when you are sure the work should not happen".to_string(),
                };
                crate::context_prompt::confirm_with_context(
                    &format!("Reject {display} (resolve it out)?"),
                    false,
                    &card,
                )
                .unwrap_or(false)
            } else {
                false
            };
            if do_reject {
                let backend = advance_backend(store_path)?;
                let mut req = backend
                    .get_requirement_unambiguous(display)? // trace:TASK-1468 | ai:claude
                    .ok_or_else(|| not_found::requirement_not_found(display, Some(store_path)))?;
                let new_status = RequirementStatus::Rejected;
                if status_advance_requires_advisor_authority(&req.status, &new_status)
                    && !crate::gate::role_authority::has_advisor_authority()
                {
                    println!(
                        "  {} rejecting {} needs advisor authority. Enter the advisor role at an interactive TTY with `aida role enter advisor`, or ask an advisor session to do it.{}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                        display.bold(),
                        roleless_recovery_sentence()
                    );
                    return Ok(());
                }
                // BUG-1637: through the one shared history helper, under the
                // caller (same identity this path used before). trace:BUG-1637 | ai:claude
                let old = req.status.clone();
                req.set_status_from_str("Rejected");
                aida_core::conflict::record_status_transition(
                    &mut req,
                    &current_user_id(None),
                    &old,
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
                // trace:STORY-809 | ai:claude
                let card = crate::context_prompt::ContextCard {
                    decision: format!("whether to close {display} now that every child is Completed"),
                    provenance: vec![format!(
                        "the rollup for {display} reports all children Completed — the epic is fully delivered"
                    )],
                    answers: vec![
                        "y: status → Completed; it leaves the open lens (reversible via `aida edit --status`)".to_string(),
                        "n: leave it open (it will be offered again next pass)".to_string(),
                    ],
                    recommended_default: "y — a fully-delivered epic staying open just clutters the open view".to_string(),
                };
                crate::context_prompt::confirm_with_context(
                    &format!("Close {display}? (all children completed → status completed)"),
                    true,
                    &card,
                )
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
            advance_dispatch_close(display, store_path)?;
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

/// BUG-1996: execute the Close action for `display` in `store_path`, transitioning
/// it to Completed and printing the completion-linkage warning if no commit was shipped.
// trace:BUG-1996 | ai:antigravity
pub(crate) fn advance_dispatch_close(display: &str, store_path: &std::path::Path) -> Result<()> {
    let backend = advance_backend(store_path)?;
    let mut req = backend
        .get_requirement_unambiguous(display)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(display, Some(store_path)))?;
    // Completing is an advisor-authority act (same gate as approve/
    // reject); an interactive operator clears it via the TTY branch of
    // `has_advisor_authority`. trace:BUG-543
    if status_advance_requires_advisor_authority(&req.status, &RequirementStatus::Completed)
        && !crate::gate::role_authority::has_advisor_authority()
    {
        println!(
            "  {} closing {} needs advisor authority. Enter the advisor role at an interactive TTY with `aida role enter advisor`, or ask an advisor session to do it.{}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            display.bold(),
            roleless_recovery_sentence()
        );
        return Ok(());
    }
    // BUG-1286 F1: the queue's completion path reaches Completed without
    // going through auto-bump or reconcile, so it must emit the ship
    // record itself. STORY-1418: stamp + persist + emit via the seam.
    // trace:BUG-1286 trace:STORY-1418 | ai:claude
    let into_completed = crate::completion::transition_to_completed(
        &mut req,
        store_path.parent(),
        display,
        "",
        "queue-done",
        |req, prior| {
            // BUG-1637: through the one shared history helper, under
            // the caller. trace:BUG-1637 | ai:claude
            aida_core::conflict::record_status_transition(req, &current_user_id(None), prior);
            req.modified_at = chrono::Utc::now();
            backend.update_requirement(req)?;
            Ok(())
        },
    )?;
    println!(
        "  {} closed {} — all children were completed.",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        display.bold()
    );
    // BUG-1996: warn if completed without a shipped commit on the
    // default branch (same check as `aida edit --status completed` and
    // `aida done`).
    // trace:BUG-1996 | ai:antigravity
    if into_completed {
        if let Some(root) = store_path.parent() {
            crate::shipped_linkage::warn_if_completed_without_shipped_commit(root, &req);
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

// trace:TASK-3426 | ai:codex
pub(super) fn handle_queue_command(
    cmd: &QueueCommand,
    storage: &Storage,
    _store_path: &std::path::Path,
) -> Result<()> {
    match cmd {
        QueueCommand::Advance { id, yes, user } => {
            handle_queue_advance(storage, id.as_deref(), *yes, user.as_deref())?
        }
        _ => unreachable!("dispatcher selected another queue subcommand"),
    }
    Ok(())
}
