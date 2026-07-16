// ----------------------------------------------------------------------------
// `aida usage --health` — the Tier-1 deterministic health-metrics catalog.
// STORY-530 / EPIC-36. Six pure metrics over the orchestrator telemetry log
// (`~/.aida/auto-complete.jsonl`), the headless session logs
// (`<root>/.aida/headless-logs/`), and the spec graph. Each metric is computed
// by a pure function in `health_metrics` / `auto_complete_telemetry`; this
// handler is the thin I/O + presentation boundary.
// ----------------------------------------------------------------------------

use anyhow::Result;
use colored::Colorize;

use aida_core::RequirementsStore;

use crate::{auto_complete, auto_complete_telemetry, health_metrics, parse_days_arg};

/// Convert a `DateTime<Utc>` to an ordinal calendar day.
// trace:STORY-530
fn datetime_to_ordinal_day(t: chrono::DateTime<chrono::Utc>) -> i64 {
    use chrono::Datelike;
    t.date_naive().num_days_from_ce() as i64
}

/// STORY-530: render the deterministic Tier-1 health-metrics catalog. Both a
/// human table and `--json`. Pure metrics, thin I/O.
// trace:STORY-530 | ai:claude
pub(crate) fn handle_health_command(
    since_raw: &str,
    json_out: bool,
    store: Option<&RequirementsStore>,
    project_root: Option<&std::path::Path>,
) -> Result<()> {
    let now = chrono::Utc::now();
    let since = now - parse_days_arg(since_raw)?;

    // --- Source 1: the orchestrator telemetry log, windowed. ----------------
    let drain_events: Vec<auto_complete_telemetry::AutoCompleteEvent> =
        auto_complete_telemetry::read_events()
            .into_iter()
            .filter(|ev| {
                chrono::DateTime::parse_from_rfc3339(&ev.completed_at)
                    .map(|t| t.with_timezone(&chrono::Utc) >= since)
                    .unwrap_or(true)
            })
            .collect();
    let drain_summary = auto_complete_telemetry::summarize(&drain_events);

    // Metric 1 — phase-failure distribution (reuses the existing histogram).
    let phase_hist = auto_complete_telemetry::failure_histogram(&drain_events);

    // Metric 2 — reap-vs-genuine-kill breakdown (reuses the session tally).
    let session_tally = project_root
        .map(|root| health_metrics::tally_from_dir(&root.join(".aida").join("headless-logs")))
        .unwrap_or_default();

    // Metric 3 — drain halt-rate, from the windowed failure kinds.
    let halt = health_metrics::halt_breakdown(
        drain_events
            .iter()
            .filter(|e| e.is_failure())
            .map(|e| e.failure_kind.clone()),
    );

    // Metric 4 — recovery latency, from the windowed run timestamps.
    let drain_runs: Vec<health_metrics::DrainRun> = drain_events
        .iter()
        .filter_map(|e| {
            let started = chrono::DateTime::parse_from_rfc3339(&e.started_at).ok()?;
            let completed = chrono::DateTime::parse_from_rfc3339(&e.completed_at).ok()?;
            Some(health_metrics::DrainRun {
                started_at: started.timestamp(),
                completed_at: completed.timestamp(),
                failed: e.is_failure(),
            })
        })
        .collect();
    let recovery = health_metrics::recovery_latency(&drain_runs);

    // --- Source 2: the spec graph (draft-inbox depth + burn-down). ----------
    let (draft_depth, burn): (Option<usize>, Option<health_metrics::BurnDownVelocity>) = match store
    {
        Some(s) => {
            let depth = health_metrics::draft_inbox_depth(s.requirements.iter().map(|r| {
                (
                    r.status == aida_core::models::RequirementStatus::Draft,
                    r.archived,
                )
            }));
            let completed_str = aida_core::models::RequirementStatus::Completed.to_string();
            let specs: Vec<health_metrics::SpecLifecycleDays> = s
                .requirements
                .iter()
                .map(|r| {
                    // "Reached Completed" from the transition history; fall
                    // back to modified_at for currently-Completed specs with
                    // no history row (mirrors find_uncovered_completed_specs).
                    let completed_at = r
                        .history
                        .iter()
                        .filter(|h| {
                            h.changes
                                .iter()
                                .any(|c| c.field_name == "status" && c.new_value == completed_str)
                        })
                        .map(|h| h.timestamp)
                        .max()
                        .or_else(|| {
                            if r.status == aida_core::models::RequirementStatus::Completed {
                                Some(r.modified_at)
                            } else {
                                None
                            }
                        });
                    health_metrics::SpecLifecycleDays {
                        created_day: datetime_to_ordinal_day(r.created_at),
                        completed_day: completed_at.map(datetime_to_ordinal_day),
                    }
                })
                .collect();
            let velocity = health_metrics::burn_down_velocity(
                &specs,
                datetime_to_ordinal_day(since),
                datetime_to_ordinal_day(now),
            );
            (Some(depth), Some(velocity))
        }
        None => (None, None),
    };

    if json_out {
        let phase_arr: Vec<serde_json::Value> = phase_hist
            .iter()
            .map(|(phase, count)| {
                serde_json::json!({
                    "phase": phase,
                    "phase_slug": auto_complete::Phase::from_index(i32::from(*phase))
                        .map(|p| p.slug()),
                    "failures": count,
                })
            })
            .collect();
        let session_arr: Vec<serde_json::Value> = session_tally
            .breakdown()
            .iter()
            .map(|(outcome, count)| {
                serde_json::json!({
                    "outcome": outcome.slug(),
                    "count": count,
                    "counts_as_success": outcome.is_success(),
                })
            })
            .collect();
        let out = serde_json::json!({
            "window": since_raw,
            "phase_failure_distribution": phase_arr,
            "reap_vs_kill": {
                "total": session_tally.total,
                "breakdown": session_arr,
                "success_rate": session_tally.success_rate(),
            },
            "drain_halt_rate": {
                "shelved": halt.shelved,
                "halted": halt.halted,
                "unclassified": halt.unclassified,
                "halt_rate": halt.halt_rate(),
            },
            "recovery_latency": {
                "count": recovery.count(),
                "mean_secs": recovery.mean_secs(),
                "median_secs": recovery.median_secs(),
                "max_secs": recovery.max_secs(),
            },
            "draft_inbox_depth": draft_depth,
            "burn_down_velocity": burn.as_ref().map(|b| serde_json::json!({
                "completed": b.completed,
                "added": b.added,
                "days": b.days,
                "net": b.net(),
                "net_per_day": b.net_per_day(),
            })),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!(
        "{} deterministic project-health catalog — last {}",
        "Health:".bold(),
        since_raw.cyan(),
    );

    // Metric 1 — phase-failure distribution.
    println!();
    println!(
        "  {} ({} failed of {} drain runs)",
        "Phase-failure distribution:".bold(),
        drain_summary.failed,
        drain_summary.total,
    );
    if phase_hist.is_empty() {
        println!("    {}", "no drain failures in the window".green());
    } else {
        let max = phase_hist.iter().map(|(_, c)| *c).max().unwrap_or(1);
        for (phase, count) in &phase_hist {
            let label = auto_complete::Phase::from_index(i32::from(*phase))
                .map(|p| p.slug())
                .unwrap_or("?");
            let width = ((*count * 20) / max).max(1);
            println!(
                "    phase {} {:<12} {} {}",
                phase,
                label,
                "█".repeat(width).red(),
                count,
            );
        }
    }

    // Metric 2 — reap-vs-genuine-kill breakdown.
    println!();
    println!(
        "  {} ({} headless sessions)",
        "Reap-vs-kill breakdown:".bold(),
        session_tally.total,
    );
    if session_tally.total == 0 {
        println!(
            "    {}",
            "no headless session logs in this project".dimmed()
        );
    } else {
        for (outcome, count) in session_tally.breakdown() {
            let marker = if outcome.is_success() {
                crate::glyph(crate::glyphs::Glyph::Check).green()
            } else {
                crate::glyph(crate::glyphs::Glyph::Cross).yellow()
            };
            println!("    {} {:<16} {}", marker, outcome.slug(), count);
        }
    }

    // Metric 3 — drain halt-rate.
    println!();
    println!("  {}", "Drain halt-rate:".bold());
    if halt.total() == 0 {
        println!("    {}", "no drain failures to classify".green());
    } else {
        println!(
            "    {} shelved (park-and-continue), {} halted (batch-stop){} → halt-rate {:.0}%",
            halt.shelved.to_string().green(),
            if halt.halted == 0 {
                halt.halted.to_string().dimmed()
            } else {
                halt.halted.to_string().yellow()
            },
            if halt.unclassified > 0 {
                format!(", {} unclassified", halt.unclassified)
                    .dimmed()
                    .to_string()
            } else {
                String::new()
            },
            halt.halt_rate() * 100.0,
        );
    }

    // Metric 4 — recovery latency.
    println!();
    println!("  {}", "Recovery latency (failure → next drain):".bold());
    if recovery.count() == 0 {
        println!(
            "    {}",
            "no failure-then-recovery pairs in the window".dimmed()
        );
    } else {
        println!(
            "    {} recoveries — median {}, mean {}, max {}",
            recovery.count(),
            recovery
                .median_secs()
                .map(humanize_secs)
                .unwrap_or_else(|| "—".to_string()),
            recovery
                .mean_secs()
                .map(humanize_secs)
                .unwrap_or_else(|| "—".to_string()),
            recovery
                .max_secs()
                .map(|s| humanize_secs(s as f64))
                .unwrap_or_else(|| "—".to_string()),
        );
    }

    // Metric 5 — draft-inbox depth.
    println!();
    print!("  {} ", "Draft-inbox depth:".bold());
    match draft_depth {
        Some(d) => {
            let cell = if d == 0 {
                d.to_string().green()
            } else {
                d.to_string().yellow()
            };
            println!("{} untriaged Draft spec(s) awaiting approve/reject", cell);
        }
        None => println!("{}", "(no store — run inside a project)".dimmed()),
    }

    // Metric 6 — burn-down velocity.
    println!();
    print!("  {} ", "Burn-down velocity:".bold());
    match &burn {
        Some(b) => {
            let net = b.net();
            let net_cell = if net > 0 {
                format!("{net:+}").green()
            } else if net < 0 {
                format!("{net:+}").yellow()
            } else {
                format!("{net:+}").dimmed()
            };
            println!(
                "{} completed, {} added over {} day(s) → net {} ({})",
                b.completed,
                b.added,
                b.days,
                net_cell,
                b.net_per_day()
                    .map(|p| format!("{p:+.2}/day"))
                    .unwrap_or_else(|| "—".to_string()),
            );
        }
        None => println!("{}", "(no store — run inside a project)".dimmed()),
    }

    Ok(())
}

/// Humanize a duration given in seconds into a compact `Xd Yh`, `Xh Ym`,
/// `Xm Ys`, or `Xs` string.
// trace:STORY-530 | ai:claude
fn humanize_secs(secs: f64) -> String {
    let s = secs.round() as i64;
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m {}s", s / 60, s % 60)
    } else if s < 86_400 {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    } else {
        format!("{}d {}h", s / 86_400, (s % 86_400) / 3600)
    }
}
