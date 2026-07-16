//! `aida field-study` command cluster (SPIKE-67 / TASK-891).
//!
//! The opt-in field-study sensor's presentation boundary: `scan` harvests the
//! git log into observations, `report` renders the span/vendor/drain/type
//! controls (human table + `--json`), and `violations` surfaces stated-rule
//! breaks observed in real drains. Pure I/O + formatting over the
//! `field_study` and `rule_violation` domain modules. Extracted verbatim from
//! `main.rs` (SPIKE-78); no behavior change.

use anyhow::Result;
use colored::Colorize;

use crate::field_study;
use crate::rule_violation;

/// Dispatch the `aida metrics` subcommands.
/// SPIKE-67 field-study command dispatch. `scan` harvests verdicts from the git
/// log into the local study log; `report` aggregates it.
// trace:STORY-477 | ai:claude trace:SPIKE-67
pub(crate) fn handle_field_study_command(cmd: &crate::cli::FieldStudyCommand) -> Result<()> {
    let root =
        crate::find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    match cmd {
        crate::cli::FieldStudyCommand::Scan { since, limit } => {
            if !field_study::is_enabled(Some(&root)) {
                println!(
                    "Field study is off (nothing recorded). Opt in to plant the sensor:\n  \
                     export AIDA_FIELD_STUDY=1\n  \
                     (or add an `enabled = true` line under a `[field_study]` section in .aida/config.toml)\n\
                     Then re-run `aida field-study scan`. Local-only; honors AIDA_TELEMETRY=0."
                );
                return Ok(());
            }
            let outcome = field_study::scan(&root, since.as_deref(), *limit);
            println!(
                "{} scanned {} commit(s) → {} new observation(s) recorded ({} already on file).",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                outcome.commits_scanned,
                outcome.observations_added,
                outcome.already_recorded
            );
            if outcome.observations_added > 0 {
                println!("Review with `aida field-study report`.");
            }
            Ok(())
        }
        crate::cli::FieldStudyCommand::Report { json } => {
            let obs = field_study::read_observations();
            let summaries = field_study::summarize(&obs);
            // The drain-vs-interactive join set (slice 2): which specs were ever
            // driven by an --auto-complete orchestrator. trace:TASK-891
            let drain_specs = field_study::drain_spec_set();
            let controls: Vec<field_study::RuleControls> = summaries
                .iter()
                .map(|s| field_study::controls_for(&obs, &s.rule, &drain_specs))
                .collect();
            if *json {
                let payload = serde_json::json!({
                    "observations": obs.len(),
                    "drain_specs_known": drain_specs.len(),
                    "rules": summaries.iter().zip(controls.iter())
                        .map(|(s, c)| field_study_rule_json(s, c))
                        .collect::<Vec<_>>(),
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
                return Ok(());
            }
            if obs.is_empty() {
                println!(
                    "No field-study observations yet. Run `aida field-study scan` (opt-in) to harvest the git log."
                );
                return Ok(());
            }
            println!(
                "{} Field study — {} observation(s) over {} rule(s)",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                obs.len(),
                summaries.len()
            );
            for (s, c) in summaries.iter().zip(controls.iter()) {
                println!(
                    "\n  {} — would-block {}/{} ({:.0}%)",
                    s.rule.bold(),
                    s.would_block,
                    s.total,
                    rate(s.would_block, s.total) * 100.0
                );
                println!("    by task span (code files changed):");
                for (bucket, total, wb) in &s.by_span {
                    println!(
                        "      {:>4} files: {:>3}/{:<3} would-block ({:.0}%)",
                        bucket,
                        wb,
                        total,
                        rate(*wb, *total) * 100.0
                    );
                }
                render_field_study_controls(c);
            }
            println!(
                "\n  Hypothesis lens: a rising would-block rate as span grows is the field signal \
                 the controlled ablations could not reach (SPIKE-67). The three controls below ask \
                 whether that span effect SURVIVES — a `flat` verdict is a valid null result."
            );
            Ok(())
        }
        crate::cli::FieldStudyCommand::Violations { json } => {
            let events = rule_violation::read_events();
            let by_rule = rule_violation::by_rule(&events);
            let (headless, supervised) = rule_violation::headless_split(&events);
            if *json {
                let payload = serde_json::json!({
                    "violations": events.len(),
                    "headless": headless,
                    "supervised": supervised,
                    "by_rule": by_rule.iter()
                        .map(|(rule, count)| serde_json::json!({ "rule": rule, "count": count }))
                        .collect::<Vec<_>>(),
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
                return Ok(());
            }
            if events.is_empty() {
                println!(
                    "No stated-rule violations recorded yet. The sensor logs during real \
                     `aida queue work --auto-complete` drains when CI / a punt / a reviewer trips \
                     a stated rule (fmt, clippy, the /// SPEC-ID leak, …) with no gate to stop it, \
                     and the post-commit hook logs a `no-verify-bypass` whenever a commit skips the \
                     pre-commit hook (git commit --no-verify). Opt in with AIDA_FIELD_STUDY=1; \
                     honors AIDA_TELEMETRY=0."
                );
                return Ok(());
            }
            println!(
                "{} Stated-rule violations observed in real drains — {} event(s) \
                 ({} headless / {} supervised)",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                events.len(),
                headless,
                supervised,
            );
            println!("\n  by rule (which stated rule needs a gate?):");
            for (rule, count) in &by_rule {
                println!("    {:>18}: {:>3}", rule.bold(), count);
            }
            println!(
                "\n  Gate-vs-rule lens: each event is a stated rule a confident agent broke that \
                 a programmatic GATE would have caught before the commit (SPIKE-67). A rule \
                 recurring here is a substrate-as-bouncer candidate."
            );
            Ok(())
        }
    }
}

/// Safe ratio that returns 0.0 for an empty denominator.
fn rate(num: usize, den: usize) -> f64 {
    if den == 0 {
        0.0
    } else {
        num as f64 / den as f64
    }
}

/// Render the three slice-2 controls (vendor / drain / type) for one rule under
/// the field-study report. Each control prints the cut plus a mechanical
/// `rises`/`flat`/`falls` verdict so the reader can see whether the span effect
/// survives the control.
// trace:TASK-891 | ai:claude
fn render_field_study_controls(c: &field_study::RuleControls) {
    let trend_str = |t: &Option<field_study::SpanTrend>| -> String {
        match t {
            Some(t) => format!(
                "span {} ({}: {:.0}% → {}: {:.0}%)",
                t.verdict(),
                t.first_bucket,
                t.first_rate * 100.0,
                t.last_bucket,
                t.last_rate * 100.0
            ),
            None => "span trend n/a (one bucket)".to_string(),
        }
    };

    // (a) vendor — does adherence-under-load differ by vendor? (EPIC-48)
    if !c.vendors.is_empty() {
        println!("    by vendor (do prose rules port across vendors?):");
        for v in &c.vendors {
            println!(
                "      {:>18}: {:>3}/{:<3} would-block ({:.0}%) · {}",
                v.vendor,
                v.would_block,
                v.total,
                rate(v.would_block, v.total) * 100.0,
                trend_str(&v.trend),
            );
        }
    }

    // (b) drain-vs-interactive — the context-pressure axis.
    let d = &c.drain;
    println!("    by autonomy (drain-vs-interactive):");
    println!(
        "            drained: {:>3}/{:<3} would-block ({:.0}%)",
        d.drain_block,
        d.drain_total,
        rate(d.drain_block, d.drain_total) * 100.0
    );
    println!(
        "        interactive: {:>3}/{:<3} would-block ({:.0}%)",
        d.interactive_block,
        d.interactive_total,
        rate(d.interactive_block, d.interactive_total) * 100.0
    );
    if d.unattributed_total > 0 {
        println!(
            "       unattributed: {:>3}/{:<3} would-block ({:.0}%, no SPEC-ID to join)",
            d.unattributed_block,
            d.unattributed_total,
            rate(d.unattributed_block, d.unattributed_total) * 100.0
        );
    }
    println!(
        "        (spec-level join; headless-vs-supervised is not recorded in auto-complete.jsonl)"
    );

    // (c) type control — does span survive once we hold commit-type to feat/fix?
    let tc = &c.type_control;
    println!("    type control (does span survive within feat/fix only?):");
    println!("        all types: {}", trend_str(&tc.all_trend));
    println!(
        "      feat/fix only: {} [{}/{} would-block]",
        trend_str(&tc.featfix_trend),
        tc.featfix_block,
        tc.featfix_total
    );
}

/// Build one rule's JSON object for `field-study report --json`: the existing
/// overall + by_span figures plus the three slice-2 controls.
// trace:TASK-891
fn field_study_rule_json(
    s: &field_study::RuleSummary,
    c: &field_study::RuleControls,
) -> serde_json::Value {
    let by_span: Vec<serde_json::Value> = s
        .by_span
        .iter()
        .map(|(b, t, wb)| {
            serde_json::json!({
                "span": b, "total": t, "would_block": wb, "would_block_rate": rate(*wb, *t),
            })
        })
        .collect();
    let by_vendor: Vec<serde_json::Value> = c
        .vendors
        .iter()
        .map(|v| {
            serde_json::json!({
                "vendor": v.vendor,
                "total": v.total,
                "would_block": v.would_block,
                "would_block_rate": rate(v.would_block, v.total),
                "span_trend": field_study_trend_json(&v.trend),
            })
        })
        .collect();
    let d = &c.drain;
    let by_drain = serde_json::json!({
        "drain": { "total": d.drain_total, "would_block": d.drain_block, "would_block_rate": rate(d.drain_block, d.drain_total) },
        "interactive": { "total": d.interactive_total, "would_block": d.interactive_block, "would_block_rate": rate(d.interactive_block, d.interactive_total) },
        "unattributed": { "total": d.unattributed_total, "would_block": d.unattributed_block, "would_block_rate": rate(d.unattributed_block, d.unattributed_total) },
        "note": "spec-level join; headless-vs-supervised not recorded in auto-complete.jsonl",
    });
    let tc = &c.type_control;
    let featfix_by_span: Vec<serde_json::Value> = tc
        .featfix_curve
        .iter()
        .map(|(b, t, wb)| {
            serde_json::json!({
                "span": b, "total": t, "would_block": wb, "would_block_rate": rate(*wb, *t),
            })
        })
        .collect();
    let type_control = serde_json::json!({
        "all_span_trend": field_study_trend_json(&tc.all_trend),
        "featfix_total": tc.featfix_total,
        "featfix_would_block": tc.featfix_block,
        "featfix_span_trend": field_study_trend_json(&tc.featfix_trend),
        "featfix_by_span": featfix_by_span,
    });
    serde_json::json!({
        "rule": s.rule,
        "total": s.total,
        "would_block": s.would_block,
        "would_block_rate": rate(s.would_block, s.total),
        "by_span": by_span,
        "by_vendor": by_vendor,
        "by_drain": by_drain,
        "type_control": type_control,
    })
}

/// JSON-encode a [`field_study::SpanTrend`] (or `null`) for the report payload.
// trace:TASK-891 | ai:claude
fn field_study_trend_json(t: &Option<field_study::SpanTrend>) -> serde_json::Value {
    match t {
        Some(t) => serde_json::json!({
            "verdict": t.verdict(),
            "first_bucket": t.first_bucket,
            "first_rate": t.first_rate,
            "last_bucket": t.last_bucket,
            "last_rate": t.last_rate,
            "delta": t.delta(),
        }),
        None => serde_json::Value::Null,
    }
}
