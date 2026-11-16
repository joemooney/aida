//! Read-only, local throughput evidence. Never infer FULL CI from an untyped red event.
// trace:STORY-2844 | ai:codex
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    process::Command,
};

use aida_core::{RequirementStatus, RequirementType, RequirementsStore};
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

use crate::events::{Event, EventKind};

#[derive(Debug)]
struct Landing {
    at: DateTime<Utc>,
    pr: Option<u64>,
}

#[derive(Default, Debug, Serialize)]
struct SpecCounts {
    opened: usize,
    completed: usize,
    closed: usize,
    missing_completed_at: usize,
    missing_closed_at: usize,
}

#[derive(Debug, Serialize)]
struct Window {
    window: String,
    since: DateTime<Utc>,
    until: DateTime<Utc>,
    landings: usize,
    longest_gap_seconds: Option<i64>,
    gaps_over_60_minutes: usize,
    product_specs: SpecCounts,
    review_stories: SpecCounts,
    landed_prs: usize,
    observed_reworked_prs: usize,
    observed_rework_rate: Option<f64>,
    rework_rate: Option<f64>,
    unknown_rework_prs: usize,
}

fn windows(raw: &str) -> Result<Vec<(String, Duration)>> {
    raw.split(',')
        .map(|part| {
            let part = part.trim();
            anyhow::ensure!(
                part.is_ascii(),
                "throughput windows must use ASCII durations"
            );
            let (n, unit) = part.split_at(part.len().saturating_sub(1));
            let multiplier = match unit {
                "m" => 60,
                "h" => 3600,
                "d" => 86400,
                "w" => 604800,
                _ => anyhow::bail!(
                    "invalid throughput window {part:?}; use positive m/h/d/w durations"
                ),
            };
            let seconds = n
                .parse::<i64>()
                .ok()
                .and_then(|n| n.checked_mul(multiplier));
            let duration = seconds
                .filter(|n| *n > 0)
                .and_then(Duration::try_seconds)
                .context("throughput duration must be positive and within range")?;
            Ok((part.to_owned(), duration))
        })
        .collect()
}

fn read_landings(root: &Path, since: DateTime<Utc>, until: DateTime<Utc>) -> Result<Vec<Landing>> {
    // Explicit main ref and first-parent traversal exclude lane commits and merge-side history.
    // Refuse a missing main rather than silently reporting the current branch or zero activity.
    let output = Command::new("git")
        .current_dir(root)
        .args([
            "log",
            "origin/main",
            "--first-parent",
            "--since-as-filter",
            &since.to_rfc3339(),
            &format!("--until={}", until.to_rfc3339()),
            "--format=%cI%x09%s",
            "--",
        ])
        .output()
        .context("read main landing history")?;
    anyhow::ensure!(
        output.status.success(),
        "cannot read origin/main landing history: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    String::from_utf8(output.stdout)?
        .lines()
        .map(|line| {
            let (at, subject) = line
                .split_once('\t')
                .context("invalid main landing record")?;
            Ok(Landing {
                at: DateTime::parse_from_rfc3339(at)?.with_timezone(&Utc),
                pr: crate::extract_pr_number_from_commit_subject(subject),
            })
        })
        .collect()
}

fn summarize(
    label: String,
    since: DateTime<Utc>,
    until: DateTime<Utc>,
    store: &RequirementsStore,
    landings: &[Landing],
    events: &[Event],
    historical: &HashMap<u64, Option<bool>>,
) -> Window {
    // Windows include both endpoints. Gaps are only between landings inside the window;
    // neither the leading quiet interval nor time since the last landing is an inter-landing gap.
    let contains = |t: DateTime<Utc>| t >= since && t <= until;
    let mut times: Vec<_> = landings
        .iter()
        .filter(|l| contains(l.at))
        .map(|l| l.at)
        .collect();
    times.sort();
    let gaps: Vec<_> = times
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).num_seconds())
        .collect();
    let prs: HashSet<_> = landings
        .iter()
        .filter(|l| contains(l.at))
        .filter_map(|l| l.pr)
        .collect();
    // A refusal before the window still counts for a PR that landed inside it. Repeats count once.
    let mut reworked: HashSet<_> = events
        .iter()
        .filter(|e| e.ts <= until)
        .filter_map(|e| match &e.kind {
            EventKind::ReviewVerdictRecorded {
                pr: Some(pr),
                verdict,
                ..
            } if matches!(
                verdict
                    .to_ascii_lowercase()
                    .replace([' ', '_'], "-")
                    .as_str(),
                "request-changes" | "changes-requested"
            ) =>
            {
                landings
                    .iter()
                    .any(|l| l.pr == Some(u64::from(*pr)) && contains(l.at) && e.ts <= l.at)
                    .then_some(u64::from(*pr))
            }
            _ => None,
        })
        .filter(|pr| prs.contains(pr))
        .collect();
    reworked.extend(
        prs.iter()
            .filter(|pr| historical.get(*pr) == Some(&Some(true)))
            .copied(),
    );
    let unknown_rework_prs = prs
        .iter()
        .filter(|pr| !reworked.contains(*pr) && !matches!(historical.get(*pr), Some(Some(false))))
        .count();
    let mut product_specs = SpecCounts::default();
    let mut review_stories = SpecCounts::default();
    for req in &store.requirements {
        let review = req.req_type == RequirementType::Story
            && ["Review PR-", "Review MR-"].iter().any(|prefix| {
                req.title
                    .strip_prefix(prefix)
                    .and_then(|tail| tail.chars().next())
                    .is_some_and(|c| c.is_ascii_digit())
            });
        let counts = if review {
            &mut review_stories
        } else {
            &mut product_specs
        };
        counts.opened += usize::from(contains(req.created_at));
        let completed = aida_core::terminal_timestamps::completed_at(req);
        counts.completed += usize::from(completed.is_some_and(contains));
        counts.missing_completed_at +=
            usize::from(req.status == RequirementStatus::Completed && completed.is_none());
        let closed = aida_core::terminal_timestamps::closed_at(req);
        counts.closed += usize::from(closed.is_some_and(contains));
        counts.missing_closed_at += usize::from(
            matches!(
                req.status,
                RequirementStatus::Rejected | RequirementStatus::Superseded
            ) && closed.is_none(),
        );
    }
    Window {
        window: label,
        since,
        until,
        landings: times.len(),
        longest_gap_seconds: gaps.iter().max().copied(),
        gaps_over_60_minutes: gaps.iter().filter(|s| **s > 3600).count(),
        product_specs,
        review_stories,
        landed_prs: prs.len(),
        observed_reworked_prs: reworked.len(),
        unknown_rework_prs,
        rework_rate: (unknown_rework_prs == 0 && !prs.is_empty())
            .then(|| reworked.len() as f64 / prs.len() as f64),
        observed_rework_rate: (!prs.is_empty()).then(|| reworked.len() as f64 / prs.len() as f64),
    }
}

pub(crate) fn run(
    raw: &str,
    format: crate::digest::DigestFormat,
    out: Option<&Path>,
    store: &RequirementsStore,
) -> Result<()> {
    let until = Utc::now();
    let ranges: Vec<_> = windows(raw)?
        .into_iter()
        .map(|(label, duration)| {
            Ok((
                label,
                until
                    .checked_sub_signed(duration)
                    .context("throughput window is outside timestamp range")?,
            ))
        })
        .collect::<Result<_>>()?;
    let root = crate::find_project_root()?;
    let since = ranges
        .iter()
        .map(|(_, since)| *since)
        .min()
        .context("no throughput windows")?;
    let landings = read_landings(&root, since, until)?;
    let event_root = crate::worktree_gc::find_main_worktree_root().unwrap_or(root);
    let events = crate::integrate_view::read_events(&event_root);
    let mut historical = HashMap::new();
    let mut errors = Vec::new();
    for landing in &landings {
        if let Some(pr) = landing.pr {
            if historical.contains_key(&pr) {
                continue;
            }
            eprintln!("Reading historical rework evidence for PR #{pr}");
            match crate::forge::github_rework(&event_root, pr, landing.at) {
                Ok(reworked) => {
                    historical.insert(pr, Some(reworked));
                }
                Err(error) => {
                    errors.push(format!("PR #{pr}: {error:#}"));
                    historical.insert(pr, None);
                }
            }
        }
    }
    let reports: Vec<_> = ranges
        .into_iter()
        .map(|(label, since)| {
            summarize(label, since, until, store, &landings, &events, &historical)
        })
        .collect();
    let note = "Rework counts historical requested changes or a failed FULL-tier run on a PR commit before landing. API failures leave rework_rate null and retain the observed lower bound. Missing terminal timestamps are excluded, never replaced by modified_at.";
    let rendered = if crate::output_format::output_format_is_json()
        || matches!(format, crate::digest::DigestFormat::Json)
    {
        serde_json::to_string_pretty(
            &serde_json::json!({"schema_version": 1, "view": "digest-throughput", "rework_coverage": if errors.is_empty() { "complete" } else { "partial" }, "notes": [note], "errors": errors, "windows": reports}),
        )? + "\n"
    } else {
        let mut text = String::new();
        for r in reports {
            text.push_str(&format!(
                "{}: {} landings; longest gap {}s; gaps >60m {}\n",
                r.window,
                r.landings,
                r.longest_gap_seconds
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "n/a".into()),
                r.gaps_over_60_minutes
            ));
            for (label, counts) in [
                ("Product specs", r.product_specs),
                ("Review stories", r.review_stories),
            ] {
                text.push_str(&format!("  {label}: opened {}, completed {}, closed {} (missing completion/closure timestamps: {}/{})\n", counts.opened, counts.completed, counts.closed, counts.missing_completed_at, counts.missing_closed_at));
            }
            text.push_str(&format!(
                "  Rework: {}/{} landed PRs ({}; unknown PRs: {})\n",
                r.observed_reworked_prs,
                r.landed_prs,
                r.rework_rate
                    .map(|rate| format!("{:.1}%", rate * 100.0))
                    .unwrap_or_else(|| "unknown; observed lower bound only".into()),
                r.unknown_rework_prs
            ));
        }
        for error in &errors {
            text.push_str(&format!("  Rework evidence unavailable: {error}\n"));
        }
        text.push_str(note);
        text.push('\n');
        text
    };
    if let Some(path) = out {
        aida_core::write_atomic(path, &rendered)?;
    } else {
        print!("{rendered}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aida_core::{FieldChange, HistoryEntry, Requirement};

    #[test]
    fn throughput_json_synthetic_store() {
        let until = DateTime::parse_from_rfc3339("2026-11-15T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let since = until - Duration::hours(24);
        let mut product = Requirement::new("Product".into(), String::new());
        product.created_at = since;
        product.status = RequirementStatus::Completed;
        product.implementation_info = Some(aida_core::ImplementationInfo {
            completed_at: Some(until - Duration::hours(2)),
            ..Default::default()
        });
        product.modified_at = until + Duration::days(1); // never use last edit for completion
        let mut review = Requirement::new("Review PR-42: example".into(), String::new());
        review.req_type = RequirementType::Story;
        review.created_at = until;
        review.status = RequirementStatus::Superseded;
        let mut history = HistoryEntry::new(
            "fixture".into(),
            vec![FieldChange {
                field_name: "status".into(),
                old_value: "Draft".into(),
                new_value: "Superseded".into(),
            }],
        );
        history.timestamp = until;
        review.history.push(history);
        let mut store = RequirementsStore::new();
        store.requirements = vec![product, review];
        // Exercise the same serialized store shape used by the command boundary.
        let store = serde_json::from_value(serde_json::to_value(store).unwrap()).unwrap();
        let landings = vec![
            Landing {
                at: until - Duration::hours(3),
                pr: Some(42),
            },
            Landing {
                at: until - Duration::hours(2),
                pr: Some(43),
            },
            Landing {
                at: until,
                pr: Some(44),
            },
        ];
        let mut refusal = Event::new(
            None,
            "",
            EventKind::ReviewVerdictRecorded {
                pr: Some(42),
                verdict: "request-changes".into(),
                reviewed_sha: None,
            },
        );
        refusal.ts = since - Duration::hours(1);
        let result = serde_json::to_value(summarize(
            "24h".into(),
            since,
            until,
            &store,
            &landings,
            &[refusal.clone(), refusal],
            &HashMap::from([(42, Some(true)), (43, Some(false)), (44, Some(false))]),
        ))
        .unwrap();
        assert_eq!(result["landings"], 3);
        assert_eq!(result["longest_gap_seconds"], 7200);
        assert_eq!(result["gaps_over_60_minutes"], 1);
        assert_eq!(result["product_specs"]["opened"], 1);
        assert_eq!(result["product_specs"]["completed"], 1);
        assert_eq!(result["review_stories"]["opened"], 1);
        assert_eq!(result["review_stories"]["closed"], 1);
        assert_eq!(result["observed_reworked_prs"], 1);
        assert_eq!(result["observed_rework_rate"], 1.0 / 3.0);
        let empty = summarize(
            "1h".into(),
            until + Duration::hours(1),
            until + Duration::hours(2),
            &store,
            &[],
            &[],
            &HashMap::new(),
        );
        assert!(empty.longest_gap_seconds.is_none());
        assert!(empty.observed_rework_rate.is_none());
    }

    #[test]
    fn throughput_unknown_evidence_never_reports_clean_rate() {
        let until = Utc::now();
        let landings = [Landing {
            at: until,
            pr: Some(42),
        }];
        let report = summarize(
            "1h".into(),
            until - Duration::hours(1),
            until,
            &RequirementsStore::new(),
            &landings,
            &[],
            &HashMap::from([(42, None)]),
        );
        let json = serde_json::to_value(report).unwrap();
        assert_eq!(json["unknown_rework_prs"], 1);
        assert!(json["rework_rate"].is_null());
        assert_eq!(json["observed_rework_rate"], 0.0);
    }

    #[test]
    fn throughput_cli_accepts_global_json_and_rejects_conflicts() {
        assert!(crate::TEST_CLI
            .try_parse_from([
                "aida",
                "digest",
                "--throughput",
                "1h,24h,7d",
                "--format",
                "json"
            ])
            .is_ok());
        assert!(crate::TEST_CLI
            .try_parse_from(["aida", "digest", "--throughput", "1h", "--reset"])
            .is_err());
    }

    #[test]
    fn throughput_windows_reject_invalid_input() {
        for invalid in ["", "0h", "-1h", "24h,", "1y", "é", "999999999999999999999d"] {
            assert!(windows(invalid).is_err(), "{invalid}");
        }
        assert_eq!(windows("1h,24h,7d").unwrap().len(), 3);
    }
}
