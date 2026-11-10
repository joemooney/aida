//! Observational metrics only; no gate decisions or persisted report state.
// trace:STORY-2699 | ai:codex
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::{CommandFactory, FromArgMatches};
use serde_json::{json, Value};
#[path = "flow_cmd_args.rs"]
mod args;
use args::*;

fn timestamp(v: &Value) -> Option<DateTime<Utc>> {
    v.as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.with_timezone(&Utc))
}
fn bound(s: &str) -> Result<DateTime<Utc>> {
    timestamp(&json!(s))
        .or_else(|| timestamp(&json!(format!("{s}T00:00:00Z"))))
        .context("expected RFC3339 timestamp or YYYY-MM-DD")
}
pub(crate) fn run(matches: &clap::ArgMatches, output_json: bool) -> Result<()> {
    let FlowCommand::Report { since, until, json } = FlowArgs::from_arg_matches(matches)?.command;
    let since = bound(&since)?;
    let until = until
        .as_deref()
        .map(bound)
        .transpose()?
        .unwrap_or_else(Utc::now);
    anyhow::ensure!(since <= until, "since must not be later than until");
    let report = crate::flow_runtime::report(since, until)?;
    let rows = report["prs"]
        .as_array()
        .context("AIDA report missing prs")?;
    if json
        || matches
            .try_get_one::<String>("format")
            .ok()
            .flatten()
            .is_some_and(|f| f == "json")
        || output_json
    {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("PR     open_h pushes first_review_h approval_land_h train_h verdicts / CI runs / findings");
        for r in rows {
            println!(
                "{:<6} {:>6} {:>6} {:>14} {:>15} {:>7} {} / {} / {}",
                r["pr"],
                display(&r["open_hours"]),
                r["pushes"],
                display(&r["first_review_hours"]),
                display(&r["approval_to_land_hours"]),
                display(&r["train_hours"]),
                r["verdict_counts"],
                r["ci_runs"],
                r["findings"]
            );
        }
        println!("Aggregate (hours except pushes): {}", report["aggregate"]);
        for note in report["notes"].as_array().unwrap() {
            println!("{}", note.as_str().unwrap());
        }
    }
    Ok(())
}
fn display(v: &Value) -> String {
    v.as_f64().map_or("unknown".into(), |n| format!("{n:.2}"))
}

pub(crate) fn command() -> clap::Command {
    FlowArgs::command().version(env!("CARGO_PKG_VERSION"))
}
