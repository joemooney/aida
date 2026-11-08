// trace:TASK-3426 | ai:codex
use super::*;

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

pub(crate) fn handle_queue_progress(
    storage: &Storage,
    session: Option<&str>,
    batch: Option<&str>,
    since: Option<&str>,
    verbose: bool,
    // trace:BUG-1289 | ai:claude
    json: bool,
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

    // trace:BUG-1289 | ai:claude
    // A valid, parseable JSON document for the "nothing to report" edge cases
    // below — `--format json` / `--json` must never fall back to the human
    // prose lines on these early-return paths.
    let empty_progress_json = |note: &str| -> Result<()> {
        println!(
            "{}",
            crate::cache_output::json_pretty(&serde_json::json!({
                "queued": 0,
                "in_progress": 0,
                "done": 0,
                "shipped": 0,
                "total": 0,
                "unresolved": 0,
                "shelved": 0,
                "buckets": {},
                "note": note,
            }))?
        );
        Ok(())
    };

    let source: Source = if let Some(name) = batch {
        let tag = format!("batch:{}", name);
        let specs: Vec<String> = store
            .requirements
            .iter()
            .filter(|r| r.tags.iter().any(|t| t.eq_ignore_ascii_case(&tag)))
            .map(|r| r.display_id())
            .collect();
        if specs.is_empty() {
            if json {
                return empty_progress_json(&format!("no requirements tagged `batch:{name}`"));
            }
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
            if json {
                return empty_progress_json(&format!(
                    "no requirements modified since {}",
                    cutoff.to_rfc3339()
                ));
            }
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
            if json {
                return empty_progress_json(
                    "no active sessions; pass --batch NAME or --since 2d for a non-session view",
                );
            }
            println!(
                "{} (no active sessions; pass --batch NAME or --since 2d for a non-session view)",
                "Progress:".bold()
            );
            return Ok(());
        };
        let manifest_path = session_manifest::manifest_path(&project_root, &lease.id);
        if !manifest_path.exists() {
            if json {
                return empty_progress_json(&format!(
                    "session {} ({}) has no planned-cluster manifest yet",
                    lease.id, lease.scope
                ));
            }
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

    // trace:BUG-1289 | ai:claude
    // The contract-promised shape (`aida queue progress --json` /
    // `--format json`): `queued` = Remaining, `in_progress` = Working now,
    // `done` = In flight (status Done — work finished on a branch, not yet
    // merged; distinct from `shipped`, which is Completed/Rejected on the
    // default branch). Mirrors `docs/internal/monitor-contract-fixtures/queue-progress.json`.
    if json {
        let shipped = bucket_count("Shipped");
        let in_flight = bucket_count("In flight");
        let working = bucket_count("Working now");
        let remaining = bucket_count("Remaining");
        let total = shipped + in_flight + working + remaining + unresolved.len();
        let buckets_json: serde_json::Value = bucket_order
            .iter()
            .map(|(label, _)| {
                let items: Vec<serde_json::Value> = buckets[label]
                    .iter()
                    .map(|(id, title)| serde_json::json!({"id": id, "title": title}))
                    .collect();
                (label.to_string(), serde_json::Value::Array(items))
            })
            .collect::<serde_json::Map<_, _>>()
            .into();
        println!(
            "{}",
            crate::cache_output::json_pretty(&serde_json::json!({
                "queued": remaining,
                "in_progress": working,
                "done": in_flight,
                "shipped": shipped,
                "total": total,
                "unresolved": unresolved,
                "shelved": shelved_count,
                "buckets": buckets_json,
            }))?
        );
        return Ok(());
    }

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

    // A groomed overlap verdict is only useful after grooming if the operator
    // can still discover the matching coupled-drain command here.
    // trace:TASK-1268 | ai:codex
    if let Source::Batch { name, .. } = &source {
        let tag = format!("batch:{name}");
        let members: Vec<aida_core::Requirement> = store
            .requirements
            .iter()
            .filter(|req| req.tags.iter().any(|t| t.eq_ignore_ascii_case(&tag)))
            .cloned()
            .collect();
        if let Some(command) = backlog::serialize_batch_command(&members, &project_root, name) {
            let mut next = Vec::new();
            crate::help_next::push_serialize_cluster(&mut next, Some(command));
            if agent_output_mode() {
                if let Some(block) = crate::help_next::render(&next) {
                    println!("{block}");
                }
            } else if let Some(block) = crate::help_next::render_human(&next) {
                println!("{block}");
            }
        }
    }

    Ok(())
}

// trace:TASK-3426 | ai:codex
pub(super) fn handle_queue_command(
    cmd: &QueueCommand,
    storage: &Storage,
    _store_path: &std::path::Path,
) -> Result<()> {
    match cmd {
        QueueCommand::Progress {
            session,
            batch,
            since,
            verbose,
            json,
        } => {
            handle_queue_progress(
                storage,
                session.as_deref(),
                // TASK-270: strip a redundant `batch:` prefix off `--batch`
                // so `--batch batch:NAME` == `--batch NAME`. trace:TASK-270
                batch.as_deref().map(normalize_batch_name),
                since.as_deref(),
                *verbose,
                // trace:BUG-1289 | ai:claude
                *json || crate::output_format_is_json(),
            )?;
        }
        _ => unreachable!("dispatcher selected another queue subcommand"),
    }
    Ok(())
}
