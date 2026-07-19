//! `aida autonomy` command cluster (STORY-439 / TASK-340).
//!
//! The autonomy-maturity surface: the calibration mismatch report
//! (pickup-predicted vs reviewer-assessed complexity) and the human-
//! intervention maturity report (count of escalate-to-human punt decisions
//! per day). Extracted verbatim from `main.rs` (SPIKE-78, pure movement) —
//! the display/config layer only. The `AutonomyMode` enum + drain-shared
//! resolution logic (ADR-10) stays near the drain machinery.

use anyhow::Result;
use colored::Colorize;

use crate::cli::{AutonomyCommand, CalibrationSubcommand};
use crate::{
    calibration, complexity_calibration, find_project_root, main_worktree_root_from, punt,
};

pub(crate) fn handle_autonomy_command(cmd: &AutonomyCommand) -> Result<()> {
    match cmd {
        AutonomyCommand::Calibration(sub) => match sub {
            CalibrationSubcommand::Mismatches { since, last, json } => {
                let project_root = find_project_root()?;
                let main_root = main_worktree_root_from(&project_root);
                let records = complexity_calibration::read_all_captures(&main_root);
                let since_dur = match since {
                    Some(s) => Some(calibration::parse_since(s).map_err(|e| anyhow::anyhow!(e))?),
                    None => None,
                };
                let rows = complexity_calibration::mismatches(&records, since_dur);
                let capped: Vec<&complexity_calibration::MismatchRow> =
                    rows.iter().take(*last).collect();

                if *json {
                    println!("{}", serde_json::to_string_pretty(&capped)?);
                    return Ok(());
                }

                if capped.is_empty() {
                    println!(
                        "{} no pickup-vs-reviewer divergences recorded{}",
                        "Calibration:".bold(),
                        match since {
                            Some(s) => format!(" in the last {s}"),
                            None => String::new(),
                        }
                    );
                    println!(
                        "  {}",
                        "captures live under .aida/complexity-calibration/ — \
                          set --complexity at pickup/ship and add \
                          `implementation_complexity` to the reviewer's verdict \
                          file to populate them"
                            .dimmed()
                    );
                    return Ok(());
                }

                println!(
                    "{} {} record{} (pickup-predicted vs reviewer-assessed){}",
                    "Calibration mismatches:".bold(),
                    capped.len(),
                    if capped.len() == 1 { "" } else { "s" },
                    match since {
                        Some(s) => format!(", window {s}"),
                        None => String::new(),
                    },
                );
                println!(
                    "  {:<14} {:<10} {:<10} {:<8} {}",
                    "SPEC".dimmed(),
                    "PICKUP".dimmed(),
                    "REVIEWER".dimmed(),
                    "DELTA".dimmed(),
                    "AGREEMENT".dimmed(),
                );
                for row in &capped {
                    let delta = match row.delta_steps.cmp(&0) {
                        std::cmp::Ordering::Greater => {
                            format!("+{}", row.delta_steps).red().to_string()
                        }
                        std::cmp::Ordering::Less => {
                            row.delta_steps.to_string().yellow().to_string()
                        }
                        std::cmp::Ordering::Equal => "0".dimmed().to_string(),
                    };
                    let agree = match row.agreement {
                        complexity_calibration::ComplexityAgreement::ImplementerUnderestimated => {
                            row.agreement.as_str().red().to_string()
                        }
                        complexity_calibration::ComplexityAgreement::ImplementerOverestimated => {
                            row.agreement.as_str().yellow().to_string()
                        }
                        complexity_calibration::ComplexityAgreement::Matched => {
                            row.agreement.as_str().dimmed().to_string()
                        }
                    };
                    println!(
                        "  {:<14} {:<10} {:<10} {:<8} {}",
                        row.spec.cyan().bold(),
                        row.pickup_complexity.as_str(),
                        row.reviewer_complexity.as_str(),
                        delta,
                        agree,
                    );
                }
                println!();
                println!(
                    "  {}",
                    "each row names a class of work the agents misjudged at pickup time; \
                      a recurring gap is a memory candidate (the substrate-gap signal)"
                        .dimmed()
                );
                Ok(())
            }
        },
        // Human-intervention maturity report: count of escalate-to-human punt
        // records, rolled up per day. The honest maturity signal — the count
        // trending toward zero shows the autonomy investment paying off.
        // (Operator decision 2026-06-06: ship the intervention-count only;
        // the availability-polluted duration fraction is skipped.)
        // trace:TASK-340 | ai:claude
        AutonomyCommand::Report { last, json } => {
            let project_root = find_project_root()?;
            let records = punt::read_ledger(&project_root);
            let days = punt::human_interventions_by_day(&records);
            let total = punt::total_human_interventions(&records);

            if *json {
                let capped: Vec<&punt::AutonomyDay> = days.iter().take(*last).collect();
                let payload = serde_json::json!({
                    "total_human_interventions": total,
                    "days": capped,
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
                return Ok(());
            }

            println!("{}", "Autonomy maturity — human interventions".bold());
            println!(
                "  {}",
                "count of escalate-to-human punt decisions, per day (newest first)".dimmed()
            );
            println!();

            if days.is_empty() {
                println!(
                    "  {}",
                    "No human interventions recorded — no drain has escalated to a human yet."
                        .green()
                );
                println!(
                    "  {}",
                    "interventions are escalate-to-human records in .aida/punts.jsonl".dimmed()
                );
                return Ok(());
            }

            println!("  {:<12} {}", "DATE".dimmed(), "INTERVENTIONS".dimmed());
            for day in days.iter().take(*last) {
                println!(
                    "  {:<12} {}",
                    day.date,
                    day.interventions.to_string().bold()
                );
            }
            println!();
            println!(
                "  {} {} across {} day{}",
                "Total:".bold(),
                total.to_string().bold(),
                days.len(),
                if days.len() == 1 { "" } else { "s" },
            );
            println!(
                "  {}",
                "the count trending toward zero is the maturity signal; \
                 it is NOT polluted by human-availability latency the way a \
                 raw duration fraction would be"
                    .dimmed()
            );
            Ok(())
        }
    }
}
