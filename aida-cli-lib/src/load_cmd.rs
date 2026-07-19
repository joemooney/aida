//! `aida load` command cluster — effort-load reporting over the queue / backlog
//! / in-flight sets plus the effort-calibration deltas view.
//!
//! Surfaces the estimated effort load for the queued, backlog, and in-flight
//! requirement sets (and the combined report), and the `Calibration` subcommand
//! that diffs recorded estimates against actuals. Extracted verbatim from
//! `main.rs` (SPIKE-78); no behavior change. Shared effort helpers
//! (`print_effort_load_for_requirements`, `queued_requirement_ids`,
//! `effort_display_id`, `find_project_root`, `main_worktree_root_from`,
//! `current_user_id`) stay in `main.rs` and are reached via `crate::`.

use anyhow::Result;
use colored::Colorize;

use aida_core::{RequirementStatus, Storage};

use crate::cli::LoadCommand;
use crate::*;

fn is_backlog_status(status: &RequirementStatus) -> bool {
    matches!(
        status,
        RequirementStatus::Draft | RequirementStatus::Approved | RequirementStatus::Planned
    )
}

pub(crate) fn handle_load_command(cmd: &LoadCommand, storage: &Storage) -> Result<()> {
    let store = storage.load()?;
    let project_root = find_project_root()
        .map(|p| main_worktree_root_from(&p))
        .unwrap_or_else(|_| {
            storage
                .path()
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from("."))
        });
    let user_id = current_user_id(None);
    let queued = queued_requirement_ids(storage, &user_id).unwrap_or_default();
    match cmd {
        LoadCommand::Queue => {
            print_effort_load_for_requirements(
                &project_root,
                "Queue load",
                store.requirements.iter().filter(|r| queued.contains(&r.id)),
            );
        }
        LoadCommand::Backlog => {
            print_effort_load_for_requirements(
                &project_root,
                "Backlog load",
                store
                    .requirements
                    .iter()
                    .filter(|r| is_backlog_status(&r.status) && !queued.contains(&r.id)),
            );
        }
        LoadCommand::Report => {
            print_effort_load_for_requirements(
                &project_root,
                "Queue load",
                store.requirements.iter().filter(|r| queued.contains(&r.id)),
            );
            print_effort_load_for_requirements(
                &project_root,
                "Backlog load",
                store
                    .requirements
                    .iter()
                    .filter(|r| is_backlog_status(&r.status) && !queued.contains(&r.id)),
            );
            print_effort_load_for_requirements(
                &project_root,
                "In-flight load",
                store.requirements.iter().filter(|r| {
                    matches!(
                        r.status,
                        RequirementStatus::InProgress | RequirementStatus::Done
                    )
                }),
            );
        }
        LoadCommand::Calibration {
            since,
            by_type,
            json,
        } => {
            let since_dur = match since {
                Some(s) => Some(calibration::parse_since(s).map_err(|e| anyhow::anyhow!(e))?),
                None => None,
            };
            let records = effort_calibration::read_all_captures(&project_root);
            let rows = effort_calibration::calibration_deltas(&records, since_dur);
            if *json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
                return Ok(());
            }
            if rows.is_empty() {
                println!("No effort calibration deltas found.");
                return Ok(());
            }
            if *by_type {
                let by_spec_type: std::collections::HashMap<String, String> = store
                    .requirements
                    .iter()
                    .map(|r| (effort_display_id(r).to_string(), r.req_type.to_string()))
                    .collect();
                let mut groups: std::collections::BTreeMap<String, (usize, i32)> =
                    std::collections::BTreeMap::new();
                for row in &rows {
                    let typ = by_spec_type
                        .get(&row.spec)
                        .cloned()
                        .unwrap_or_else(|| "unknown".to_string());
                    let entry = groups.entry(typ).or_insert((0, 0));
                    entry.0 += 1;
                    entry.1 += row.delta_minutes;
                }
                println!("{}", "Effort calibration by type".bold());
                for (typ, (count, delta)) in groups {
                    println!(
                        "  {:<16} {:>3} rows  net delta {}",
                        typ,
                        count,
                        effort_calibration::format_minutes(delta.unsigned_abs())
                    );
                }
            } else {
                println!("{}", "Effort calibration deltas".bold());
                for row in rows.iter().take(50) {
                    let sign = if row.delta_minutes >= 0 { "+" } else { "-" };
                    println!(
                        "  {:<12} {:<6} est {:<3} actual {:<3} delta {}{}",
                        row.spec,
                        row.touchpoint.as_str(),
                        row.estimate,
                        row.actual,
                        sign,
                        effort_calibration::format_minutes(row.delta_minutes.unsigned_abs())
                    );
                }
            }
        }
    }
    Ok(())
}
