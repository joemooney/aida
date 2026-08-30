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
        crate::cli::MetricsCommand::AiLift { markdown, json } => {
            handle_metrics_ai_lift(*markdown, *json)
        }
    }
}

/// Compute + render the `ai-lift` report from the git commit corpus.
// trace:STORY-783 | ai:codex
fn handle_metrics_ai_lift(markdown: bool, json_out: bool) -> Result<()> {
    let project_root = crate::find_project_root()?;
    let commits = read_commit_corpus(&project_root)?;
    let lift = metrics::compute_ai_lift(&commits);

    if json_out {
        println!("{}", serde_json::to_string_pretty(&ai_lift_json(&lift))?);
        return Ok(());
    }

    if markdown {
        render_ai_lift_markdown(&lift);
        return Ok(());
    }

    render_ai_lift_terminal(&lift);
    Ok(())
}

/// Read commit date + subject only. Author identity is intentionally never
/// requested so the report cannot grow a per-author projection by accident.
// trace:STORY-783 | ai:codex
fn read_commit_corpus(project_root: &std::path::Path) -> Result<Vec<metrics::CommitCorpusEntry>> {
    let output = std::process::Command::new("git")
        .args(["log", "--date=short", "--format=%H%x00%ad%x00%s%x1e"])
        .current_dir(project_root)
        .output()?;

    if !output.status.success() {
        anyhow::bail!(
            "git log failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .split('\x1e')
        .filter_map(|raw| {
            let raw = raw.trim_matches('\n');
            if raw.is_empty() {
                return None;
            }
            let mut parts = raw.splitn(3, '\0');
            let _sha = parts.next()?;
            let date = parts.next()?.to_string();
            let message = parts.next().unwrap_or_default().to_string();
            Some(metrics::CommitCorpusEntry { date, message })
        })
        .collect())
}

// trace:STORY-783 | ai:codex
fn ai_lift_json(lift: &metrics::AiLift) -> serde_json::Value {
    let coverage = if lift.convention_adopted() {
        Some(lift.overall_coverage())
    } else {
        None
    };
    serde_json::json!({
        "total_commits": lift.total_commits,
        "trailer_commits": lift.trailer_commits,
        "convention": if lift.convention_adopted() { "adopted" } else { "not adopted" },
        "coverage": coverage,
        "time_series": lift.buckets.iter().map(|bucket| {
            serde_json::json!({
                "month": bucket.month,
                "total_commits": bucket.total_commits,
                "trailer_commits": bucket.trailer_commits,
                "coverage": if lift.convention_adopted() { Some(bucket.coverage()) } else { None },
            })
        }).collect::<Vec<_>>(),
        "tools": lift.tools,
        "confidence": {
            "tagged_commits": lift.confidence_tagged_commits(),
            "total_commits": lift.total_commits,
            "trailer_commits": lift.trailer_commits,
            "sparsity": lift.confidence_sparsity(),
            "bands": lift.confidence,
        }
    })
}

/// Terminal rendering of aggregate commit-corpus AI-lift.
// trace:STORY-783 | ai:codex
fn render_ai_lift_terminal(lift: &metrics::AiLift) {
    println!("{}", "AI-lift: git commit corpus".bold());
    println!("  {:<24} {}", "commits scanned".bold(), lift.total_commits);

    if !lift.convention_adopted() {
        println!(
            "  {:<24} {}",
            "trailer convention".bold(),
            "not adopted — no [AI:tool] trailers found".yellow()
        );
        return;
    }

    println!(
        "  {:<24} {:.1}% ({} of {} commits)",
        "trailer coverage".bold(),
        lift.overall_coverage() * 100.0,
        lift.trailer_commits.to_string().cyan(),
        lift.total_commits,
    );
    println!(
        "  {:<24} {}",
        "confidence bands".bold(),
        format!(
            "{} tagged commits / {} total ({})",
            lift.confidence_tagged_commits(),
            lift.total_commits,
            lift.confidence_sparsity()
        )
        .cyan()
    );

    println!();
    println!("  {}", "Coverage over time".bold());
    for bucket in &lift.buckets {
        println!(
            "    {}  {:>6.1}%  ({}/{})",
            bucket.month,
            bucket.coverage() * 100.0,
            bucket.trailer_commits,
            bucket.total_commits,
        );
    }

    println!();
    println!("  {}", "Tool attribution".bold());
    for (tool, count) in &lift.tools {
        println!("    {:<16} {}", tool, count);
    }

    println!();
    println!("  {}", "Confidence bands".bold());
    for (band, count) in &lift.confidence {
        println!("    {:<16} {}", band, count);
    }
    println!(
        "\n  {}",
        "Aggregate-only: no per-author breakdown is computed or emitted.".dimmed()
    );
}

/// Markdown rendering of aggregate commit-corpus AI-lift.
// trace:STORY-783 | ai:codex
fn render_ai_lift_markdown(lift: &metrics::AiLift) {
    println!("## AIDA AI-lift from git history\n");
    println!("- Commits scanned: {}", lift.total_commits);

    if !lift.convention_adopted() {
        println!("- Trailer convention: not adopted — no `[AI:tool]` trailers found");
        return;
    }

    println!(
        "- Trailer coverage: {:.1}% ({} of {} commits)",
        lift.overall_coverage() * 100.0,
        lift.trailer_commits,
        lift.total_commits
    );
    println!(
        "- Confidence bands: {} tagged commits / {} total ({})",
        lift.confidence_tagged_commits(),
        lift.total_commits,
        lift.confidence_sparsity()
    );
    println!("\n### Coverage over time\n");
    println!("| Month | Coverage | AI trailer commits | Total commits |");
    println!("| --- | ---: | ---: | ---: |");
    for bucket in &lift.buckets {
        println!(
            "| {} | {:.1}% | {} | {} |",
            bucket.month,
            bucket.coverage() * 100.0,
            bucket.trailer_commits,
            bucket.total_commits
        );
    }

    println!("\n### Tool attribution\n");
    println!("| Tool | Commit attributions |");
    println!("| --- | ---: |");
    for (tool, count) in &lift.tools {
        println!("| {} | {} |", tool, count);
    }

    println!("\n### Confidence bands\n");
    println!("| Band | Commits |");
    println!("| --- | ---: |");
    for (band, count) in &lift.confidence {
        println!("| {} | {} |", band, count);
    }
    println!("\n> Aggregate-only: no per-author breakdown is computed or emitted.");
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
