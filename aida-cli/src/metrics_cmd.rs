//! `aida metrics` command cluster (STORY-477).
//!
//! The dogfood-signals surface: `aida metrics agent-lift` reads the two local
//! telemetry logs (`~/.aida/auto-complete.jsonl` + `~/.aida/usage.jsonl`),
//! windows them to `--since`, computes the derivable lift signals via the pure
//! `metrics` module, and renders them as a colorized terminal report, a
//! paste-ready markdown table, or JSON. Reads only; never writes the logs.
//! Extracted verbatim from `main.rs` (SPIKE-78); no behavior change.

use anyhow::Result;
use colored::Colorize;

use crate::{auto_complete_telemetry, metrics, parse_days_arg, usage};

pub(crate) fn handle_metrics_command(cmd: &crate::cli::MetricsCommand) -> Result<()> {
    match cmd {
        crate::cli::MetricsCommand::AgentLift {
            since,
            markdown,
            json,
        } => handle_metrics_agent_lift(since, *markdown, *json),
    }
}

/// Compute + render the `agent-lift` report. Reads the two local telemetry
/// logs, windows them to `--since`, computes the derivable signals, and
/// renders them in the requested format.
// trace:STORY-477 | ai:claude
fn handle_metrics_agent_lift(since_raw: &str, markdown: bool, json_out: bool) -> Result<()> {
    let now = chrono::Utc::now();
    let since = now - parse_days_arg(since_raw)?;

    // Window the autonomous-drain log by completion time (unparseable
    // timestamps are kept rather than dropped, matching `aida usage
    // --auto-complete`).
    let ac_events: Vec<auto_complete_telemetry::AutoCompleteEvent> =
        auto_complete_telemetry::read_events()
            .into_iter()
            .filter(|ev| {
                chrono::DateTime::parse_from_rfc3339(&ev.completed_at)
                    .map(|t| t.with_timezone(&chrono::Utc) >= since)
                    .unwrap_or(true)
            })
            .collect();

    // Window the human usage log by event time.
    let usage_events: Vec<usage::UsageEvent> = usage::read_events()
        .into_iter()
        .filter(|ev| {
            chrono::DateTime::parse_from_rfc3339(&ev.ts)
                .map(|t| t.with_timezone(&chrono::Utc) >= since)
                .unwrap_or(true)
        })
        .collect();

    let lift = metrics::compute_agent_lift(&ac_events, &usage_events);

    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "since": since_raw,
                "drain_runs": lift.drain_runs,
                "drain_success": lift.drain_success,
                "drain_failed": lift.drain_failed,
                "drain_success_rate": lift.drain_success_rate(),
                "distinct_specs": lift.distinct_specs,
                "distinct_builds": lift.distinct_builds,
                "stale_base_attempts": lift.stale_base_attempts,
                "stale_base_recoveries": lift.stale_base_recoveries,
                "stale_base_recovery_rate": lift.stale_base_recovery_rate(),
                "human_invocations": lift.human_invocations,
                "human_command_shapes": lift.human_command_shapes,
            }))?
        );
        return Ok(());
    }

    if markdown {
        render_agent_lift_markdown(&lift, since_raw);
        return Ok(());
    }

    render_agent_lift_terminal(&lift, since_raw);
    Ok(())
}

/// Terminal (colorized) rendering of the agent-lift report.
// trace:STORY-477 | ai:claude
fn render_agent_lift_terminal(lift: &metrics::AgentLift, since_raw: &str) {
    println!(
        "{} dogfood signals over the last {}",
        "Agent-lift:".bold(),
        since_raw.cyan()
    );

    if lift.is_empty() {
        println!(
            "  {}",
            "no telemetry recorded in the window — run drains / commands first".dimmed()
        );
        println!(
            "  {} {} {} {}",
            "logs:".dimmed(),
            auto_complete_telemetry::log_path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "<home dir unavailable>".to_string())
                .dimmed(),
            "+".dimmed(),
            usage::log_path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "<home dir unavailable>".to_string())
                .dimmed(),
        );
        return;
    }

    println!(
        "  {:<28} {} ({} ok / {} failed of {} runs)",
        "autonomous drain success".bold(),
        format!("{:.0}%", lift.drain_success_rate() * 100.0).green(),
        lift.drain_success.to_string().green(),
        if lift.drain_failed == 0 {
            lift.drain_failed.to_string().dimmed()
        } else {
            lift.drain_failed.to_string().yellow()
        },
        lift.drain_runs,
    );
    println!(
        "  {:<28} {} specs across {} build{}",
        "coordinated work".bold(),
        lift.distinct_specs.to_string().cyan(),
        lift.distinct_builds.to_string().cyan(),
        if lift.distinct_builds == 1 { "" } else { "s" },
    );
    println!(
        "  {:<28} {} of {} auto-rebase attempts ({:.0}%)",
        "stale-base recoveries".bold(),
        lift.stale_base_recoveries.to_string().green(),
        lift.stale_base_attempts,
        lift.stale_base_recovery_rate() * 100.0,
    );
    println!(
        "  {:<28} {} drain runs alongside {} human invocations ({} command shapes)",
        "autonomous vs human".bold(),
        lift.drain_runs.to_string().cyan(),
        lift.human_invocations.to_string().cyan(),
        lift.human_command_shapes,
    );
    println!();
    println!(
        "  {}",
        "Limitations: brief-to-PR time, unshipped-commits-caught, and trace \
         coverage are not yet recorded in the telemetry substrate, so they are \
         omitted rather than approximated."
            .dimmed()
    );
}

/// Markdown rendering of the agent-lift report — paste-ready for release
/// notes / case studies.
// trace:STORY-477 | ai:claude
fn render_agent_lift_markdown(lift: &metrics::AgentLift, since_raw: &str) {
    println!("## AIDA agent-lift (last {since_raw})\n");
    if lift.is_empty() {
        println!("_No telemetry recorded in this window._");
        return;
    }
    println!("| Signal | Value |");
    println!("| --- | --- |");
    println!(
        "| Autonomous drain success rate | {:.0}% ({}/{} runs) |",
        lift.drain_success_rate() * 100.0,
        lift.drain_success,
        lift.drain_runs,
    );
    println!(
        "| Coordinated specs / builds | {} specs across {} build(s) |",
        lift.distinct_specs, lift.distinct_builds,
    );
    println!(
        "| Stale-base recoveries | {} of {} auto-rebase attempts ({:.0}%) |",
        lift.stale_base_recoveries,
        lift.stale_base_attempts,
        lift.stale_base_recovery_rate() * 100.0,
    );
    println!(
        "| Autonomous vs human | {} drain runs alongside {} human invocations ({} command shapes) |",
        lift.drain_runs, lift.human_invocations, lift.human_command_shapes,
    );
    println!(
        "\n> Limitations: brief-to-PR time, unshipped-commits-caught, and trace \
         coverage are not yet captured in the telemetry substrate and are \
         therefore omitted rather than approximated."
    );
}
