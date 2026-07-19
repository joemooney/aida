//! `aida advisor schedule` command cluster (STORY-262).
//!
//! The no-daemon scheduling primitive: recurring TASK templates with a cadence,
//! registered under `.aida/schedules.toml`, fired into the target role's queue
//! on `aida pull`. Extracted verbatim from `main.rs` (SPIKE-78); no behavior
//! change.

use anyhow::{Context, Result};
use colored::Colorize;
use std::collections::HashSet;

use aida_core::{Requirement, RequirementStatus, RequirementType, Storage};

use crate::cli::ScheduleCommand;
use crate::schedule;
use crate::{canonical_role_name, current_user_id, get_default_author, is_terminal_status};

/// STORY-262: `aida advisor schedule <add|list|enable|disable|remove|run>`.
/// The no-daemon scheduling primitive — recurring task templates with a
/// cadence, fired into the queue on `aida pull`.
// trace:STORY-262 | ai:claude
pub(crate) fn handle_schedule_command(
    cmd: &ScheduleCommand,
    project_root: &std::path::Path,
    store_path: &std::path::Path,
) -> Result<()> {
    match cmd {
        ScheduleCommand::Add {
            name,
            every,
            template,
            description,
            tags,
            for_role,
        } => {
            // Validate the cadence up front so a bad token errors at
            // registration instead of silently never-firing.
            schedule::parse_cadence(every)
                .with_context(|| format!("invalid --every cadence '{every}'"))?;

            let mut file = schedule::load(project_root);
            if file.schedules.iter().any(|s| &s.name == name) {
                anyhow::bail!(
                    "a schedule named '{name}' already exists — `aida advisor schedule remove {name}` first, or pick a different name"
                );
            }
            let parsed_tags: Vec<String> = tags
                .as_deref()
                .map(|t| {
                    t.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            file.schedules.push(schedule::Schedule {
                name: name.clone(),
                cadence: every.clone(),
                title: template.clone(),
                description: description.clone(),
                tags: parsed_tags,
                for_role: canonical_role_name(for_role),
                last_fired: None,
                enabled: true,
            });
            schedule::save(project_root, &file)?;
            println!(
                "{} schedule '{}' — every {}, routes to {} (fires on next `aida pull`)",
                "Registered".green().bold(),
                name.cyan(),
                every,
                for_role,
            );
            Ok(())
        }
        ScheduleCommand::List { json } => {
            let file = schedule::load(project_root);
            if *json {
                println!("{}", serde_json::to_string_pretty(&file.schedules)?);
                return Ok(());
            }
            if file.schedules.is_empty() {
                println!(
                    "No schedules registered. Add one with:\n  aida advisor schedule add <name> --every 14d --template \"...\""
                );
                return Ok(());
            }
            let now = chrono::Utc::now();
            println!(
                "{:<28} {:<9} {:<13} {:<13} {}",
                "Name".bold(),
                "Cadence".bold(),
                "Last fired".bold(),
                "Next due".bold(),
                "Status".bold(),
            );
            for s in &file.schedules {
                let last = s
                    .last_fired
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| "(never)".to_string());
                let next = s
                    .next_due()
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| "now".to_string());
                let status = if !s.enabled {
                    "disabled".dimmed().to_string()
                } else if s.is_due(now) {
                    "due".yellow().bold().to_string()
                } else {
                    "pending".green().to_string()
                };
                println!(
                    "{:<28} {:<9} {:<13} {:<13} {}",
                    s.name, s.cadence, last, next, status,
                );
            }
            Ok(())
        }
        ScheduleCommand::Enable { name } => set_schedule_enabled(project_root, name, true),
        ScheduleCommand::Disable { name } => set_schedule_enabled(project_root, name, false),
        ScheduleCommand::Remove { name } => {
            let mut file = schedule::load(project_root);
            let before = file.schedules.len();
            file.schedules.retain(|s| &s.name != name);
            if file.schedules.len() == before {
                anyhow::bail!("no schedule named '{name}'");
            }
            schedule::save(project_root, &file)?;
            println!("{} schedule '{}'", "Removed".green().bold(), name.cyan());
            Ok(())
        }
        ScheduleCommand::Run { name } => {
            let fired = fire_schedules(project_root, store_path, name.as_deref(), false)?;
            if fired.is_empty() {
                match name {
                    Some(n) => println!("Schedule '{n}' fired no TASK (see notes above)."),
                    None => println!("No schedules were due."),
                }
            }
            Ok(())
        }
    }
}

/// STORY-262 helper: flip a schedule's `enabled` flag.
// trace:STORY-262 | ai:claude
fn set_schedule_enabled(project_root: &std::path::Path, name: &str, enabled: bool) -> Result<()> {
    let mut file = schedule::load(project_root);
    let s = file
        .schedules
        .iter_mut()
        .find(|s| s.name == name)
        .ok_or_else(|| anyhow::anyhow!("no schedule named '{name}'"))?;
    s.enabled = enabled;
    schedule::save(project_root, &file)?;
    println!(
        "{} schedule '{}'",
        if enabled {
            "Enabled".green().bold()
        } else {
            "Disabled".yellow().bold()
        },
        name.cyan(),
    );
    Ok(())
}

/// STORY-262: evaluate schedules and fire the due ones (the logic `aida pull`
/// invokes, and `aida advisor schedule run` too). Returns the names of
/// schedules that actually filed a TASK.
///
/// - `only`: when `Some(name)`, restrict to that one schedule and FORCE-fire
///   it regardless of cadence (the `schedule run <name>` semantics). When
///   `None`, fire every currently-due, enabled schedule.
/// - `quiet`: suppress the per-fire stdout lines (used by `--quiet` pulls).
///
/// Skip rule (acceptance): if a schedule already has an open (non-terminal)
/// TASK from a prior fire, SKIP and surface a note rather than piling up.
// trace:STORY-262 | ai:claude
pub(crate) fn fire_schedules(
    project_root: &std::path::Path,
    store_path: &std::path::Path,
    only: Option<&str>,
    quiet: bool,
) -> Result<Vec<String>> {
    let mut file = schedule::load(project_root);
    if file.schedules.is_empty() {
        if let Some(n) = only {
            anyhow::bail!("no schedule named '{n}'");
        }
        return Ok(vec![]);
    }
    if let Some(n) = only {
        if !file.schedules.iter().any(|s| s.name == n) {
            anyhow::bail!("no schedule named '{n}'");
        }
    }

    let now = chrono::Utc::now();
    // Load the store once to detect open prior fires by `scheduled:<name>` tag.
    let storage = Storage::new(store_path);
    let store = storage.load().ok();

    let mut fired: Vec<String> = vec![];
    let mut changed = false;

    for s in file.schedules.iter_mut() {
        // Targeting: when `only` is set, force-fire just that one.
        let should_consider = match only {
            Some(n) => s.name == n,
            None => s.is_due(now),
        };
        if !should_consider {
            continue;
        }
        // A forced run still respects `enabled = false` only when not
        // explicitly named; an explicit `run <name>` overrides.
        if only.is_none() && !s.enabled {
            continue;
        }

        // Skip if a prior fire is still open (non-terminal).
        let scheduled_tag = format!("scheduled:{}", s.name);
        let has_open_prior = store
            .as_ref()
            .map(|st| {
                st.requirements
                    .iter()
                    .any(|r| r.tags.contains(&scheduled_tag) && !is_terminal_status(&r.status))
            })
            .unwrap_or(false);
        if has_open_prior {
            if !quiet {
                eprintln!(
                    "  {} schedule '{}' has an open TASK from a prior fire — skipping (finish it first)",
                    "Note:".dimmed(),
                    s.name,
                );
            }
            continue;
        }

        match file_task_for_schedule(&storage, s) {
            Ok(spec_id) => {
                s.last_fired = Some(now);
                changed = true;
                fired.push(s.name.clone());
                if !quiet {
                    println!(
                        "  {} {} from schedule '{}' → {} queue",
                        "Filed".green().bold(),
                        spec_id.cyan(),
                        s.name,
                        s.for_role,
                    );
                }
            }
            Err(e) => {
                eprintln!(
                    "  {} schedule '{}' failed to file a TASK: {} (last_fired not updated; retries next pull)",
                    "Warning:".yellow().bold(),
                    s.name,
                    e,
                );
            }
        }
    }

    if changed {
        schedule::save(project_root, &file)?;
    }
    Ok(fired)
}

/// STORY-262: file one TASK from a schedule template into the target role's
/// queue. Returns the new spec id (or the UUID if no spec id was assigned).
/// Tags the TASK `scheduled:<name>` + `batch:scheduled` so it's traceable and
/// drainable via `aida queue work --batch scheduled --auto-complete`.
// trace:STORY-262 | ai:claude
fn file_task_for_schedule(storage: &Storage, s: &schedule::Schedule) -> Result<String> {
    let mut store = storage.load()?;

    let mut requirement =
        Requirement::new(s.title.clone(), s.description.clone().unwrap_or_default());
    requirement.req_type = RequirementType::Task;
    // Approved so the routed role can pick it up immediately; these are
    // operator-registered cadences, not unvetted intake.
    requirement.status = RequirementStatus::Approved;
    requirement.owner = get_default_author();

    let mut tags: HashSet<String> = HashSet::new();
    tags.insert(format!("scheduled:{}", s.name));
    tags.insert("batch:scheduled".to_string());
    for t in &s.tags {
        tags.insert(t.clone());
    }
    requirement.tags = tags;

    let id = requirement.id;
    let type_prefix = store.get_type_prefix(&requirement.req_type);
    store.add_requirement_with_id(requirement, None, type_prefix.as_deref());
    storage.save(&store)?;

    let spec_id = store
        .get_requirement_by_id(&id)
        .and_then(|r| r.spec_id.clone())
        .unwrap_or_else(|| id.to_string());

    // Route into the target role's queue.
    let user_id = current_user_id(None);
    let entry = aida_core::QueueEntry {
        user_id: user_id.clone(),
        requirement_id: id,
        position: i64::MAX,
        added_by: user_id,
        note: Some(format!("Scheduled task from '{}'", s.name)),
        added_at: chrono::Utc::now(),
        for_role: Some(canonical_role_name(&s.for_role)),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    };
    storage
        .queue_add(entry)
        .with_context(|| format!("failed to queue scheduled TASK {spec_id}"))?;

    Ok(spec_id)
}
