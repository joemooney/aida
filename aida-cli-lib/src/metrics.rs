//! STORY-477: `aida metrics agent-lift` — a reporting layer over the
//! existing dogfood telemetry substrate (`~/.aida/auto-complete.jsonl` +
//! `~/.aida/usage.jsonl`) that surfaces the coordination / agent-lift
//! signals already derivable from recorded data.
//!
//! This is intentionally a *reporting* layer: it computes nothing the
//! substrate doesn't already record. The acceptance criteria on STORY-477
//! enumerate a wider wishlist (brief-to-PR time, trace coverage, …) whose
//! source signals are not yet captured in the telemetry logs; those are
//! documented as limitations rather than approximated. The metrics emitted
//! here are the 2-3 signals that are genuinely derivable today:
//!
//!   - **drain success rate** — fraction of `--auto-complete` orchestrator
//!     runs that reached `success` (the autonomous-lifecycle success metric).
//!   - **autonomous runs / coordinated agent count** — how many autonomous
//!     drain runs ran, over how many distinct specs, across how many distinct
//!     binary builds (the "more than one agent / build coordinated work"
//!     proxy).
//!   - **stale-base recoveries** — phase-3 auto-rebase events that landed
//!     `clean`, i.e. the orchestrator recovered a stale PR base without a
//!     human intervening (a concrete manual-intervention-avoided signal).
//!   - **autonomous-vs-human split** — autonomous drain runs counted against
//!     a coarse human-activity proxy (distinct usage-log command shapes in
//!     the window), so the report can say "N autonomous runs alongside human
//!     CLI activity".
//!
//! The computation is a pure function over event slices so it is fully unit
//! testable from fixtures with no filesystem or clock involvement.
//!
//! trace:STORY-477 | ai:claude

use crate::auto_complete_telemetry::AutoCompleteEvent;
use crate::usage::UsageEvent;
use std::collections::BTreeMap;

/// One commit as read from the git log corpus. This deliberately omits author
/// fields because STORY-783 requires aggregate-only reporting.
// trace:STORY-783 | ai:codex
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitCorpusEntry {
    pub date: String,
    pub message: String,
}

/// Monthly trailer-coverage bucket for the commit corpus.
// trace:STORY-783 | ai:codex
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AiLiftBucket {
    pub month: String,
    pub total_commits: usize,
    pub trailer_commits: usize,
}

impl AiLiftBucket {
    pub fn coverage(&self) -> f64 {
        if self.total_commits == 0 {
            0.0
        } else {
            self.trailer_commits as f64 / self.total_commits as f64
        }
    }
}

/// Aggregate-only AI-lift signals derived from commit messages.
// trace:STORY-783 | ai:codex
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AiLift {
    pub total_commits: usize,
    pub trailer_commits: usize,
    pub buckets: Vec<AiLiftBucket>,
    pub tools: BTreeMap<String, usize>,
    pub confidence: BTreeMap<String, usize>,
}

impl AiLift {
    pub fn convention_adopted(&self) -> bool {
        self.trailer_commits > 0
    }

    pub fn overall_coverage(&self) -> f64 {
        if self.total_commits == 0 {
            0.0
        } else {
            self.trailer_commits as f64 / self.total_commits as f64
        }
    }

    pub fn confidence_tagged_commits(&self) -> usize {
        ["high", "med", "low"]
            .into_iter()
            .filter_map(|band| self.confidence.get(band))
            .sum()
    }

    pub fn confidence_sparsity(&self) -> &'static str {
        if self.confidence_tagged_commits() == 0 {
            "absent"
        } else if self.total_commits > 0
            && (self.confidence_tagged_commits() as f64 / self.total_commits as f64) < 0.10
        {
            "sparse"
        } else {
            "present"
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct AiTrailer {
    tools: Vec<String>,
    confidence: Option<String>,
}

/// Compute git-corpus AI-lift signals from already-read commits.
// trace:STORY-783 | ai:codex
pub fn compute_ai_lift(commits: &[CommitCorpusEntry]) -> AiLift {
    let mut lift = AiLift {
        total_commits: commits.len(),
        ..AiLift::default()
    };
    let mut buckets: BTreeMap<String, AiLiftBucket> = BTreeMap::new();

    for commit in commits {
        let month = commit_month(&commit.date);
        let bucket = buckets
            .entry(month.clone())
            .or_insert_with(|| AiLiftBucket {
                month,
                ..AiLiftBucket::default()
            });
        bucket.total_commits += 1;

        let trailers = parse_ai_trailers(&commit.message);
        if trailers.is_empty() {
            continue;
        }

        lift.trailer_commits += 1;
        bucket.trailer_commits += 1;

        let mut commit_had_confidence = false;
        for trailer in trailers {
            for tool in trailer.tools {
                *lift.tools.entry(tool).or_insert(0) += 1;
            }
            if let Some(confidence) = trailer.confidence {
                commit_had_confidence = true;
                *lift.confidence.entry(confidence).or_insert(0) += 1;
            }
        }

        if !commit_had_confidence {
            *lift
                .confidence
                .entry("unspecified".to_string())
                .or_insert(0) += 1;
        }
    }

    lift.buckets = buckets.into_values().collect();
    lift
}

// trace:STORY-783 | ai:codex
fn commit_month(date: &str) -> String {
    if date.len() >= 7 && date.as_bytes().get(4) == Some(&b'-') {
        date[..7].to_string()
    } else {
        "unknown".to_string()
    }
}

// trace:STORY-783 | ai:codex
fn parse_ai_trailers(message: &str) -> Vec<AiTrailer> {
    let re = regex::Regex::new(r"^\[AI:([^\]\s]+)\]").expect("static regex compiles");
    re.captures(message)
        .and_then(|cap| parse_ai_trailer_body(&cap[1]))
        .into_iter()
        .collect()
}

// trace:STORY-783 | ai:codex
fn parse_ai_trailer_body(body: &str) -> Option<AiTrailer> {
    let (tools_raw, confidence) = match body.rsplit_once(':') {
        Some((tools, band)) if matches!(band, "high" | "med" | "low") => {
            (tools, Some(band.to_string()))
        }
        _ => (body, None),
    };
    let tools: Vec<String> = tools_raw
        .split('+')
        .map(str::trim)
        .filter(|tool| !tool.is_empty())
        .map(|tool| tool.to_ascii_lowercase())
        .collect();

    if tools.is_empty() {
        None
    } else {
        Some(AiTrailer { tools, confidence })
    }
}

/// The agent-lift signals derived from the telemetry substrate over a window.
/// Every field is a count or ratio computed purely from the input slices.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentLift {
    /// Total `--auto-complete` orchestrator runs in the window.
    pub drain_runs: usize,
    /// Runs that reached `success`.
    pub drain_success: usize,
    /// Runs that did not succeed.
    pub drain_failed: usize,
    /// Distinct specs driven by an autonomous drain in the window — the
    /// "how much work was coordinated autonomously" breadth signal.
    pub distinct_specs: usize,
    /// Distinct binary build SHAs that recorded a drain run — a proxy for
    /// "how many distinct agent builds coordinated work in this window".
    pub distinct_builds: usize,
    /// Phase-3 auto-rebase attempts recorded across all runs.
    pub stale_base_attempts: usize,
    /// Phase-3 auto-rebase attempts that landed `clean` — stale-base
    /// recoveries the orchestrator handled with no human intervention.
    pub stale_base_recoveries: usize,
    /// Distinct human-driven command shapes seen in the usage log over the
    /// window — a coarse "human activity present" proxy for the
    /// autonomous-vs-human framing.
    pub human_command_shapes: usize,
    /// Total human-driven CLI invocations in the window.
    pub human_invocations: usize,
}

impl AgentLift {
    /// Drain success rate in `0.0..=1.0`. `0.0` when no runs.
    pub fn drain_success_rate(&self) -> f64 {
        if self.drain_runs == 0 {
            0.0
        } else {
            self.drain_success as f64 / self.drain_runs as f64
        }
    }

    /// Fraction of stale-base auto-rebase attempts that recovered cleanly.
    /// `0.0` when no attempts.
    pub fn stale_base_recovery_rate(&self) -> f64 {
        if self.stale_base_attempts == 0 {
            0.0
        } else {
            self.stale_base_recoveries as f64 / self.stale_base_attempts as f64
        }
    }

    /// Whether the window has anything worth reporting at all.
    pub fn is_empty(&self) -> bool {
        self.drain_runs == 0 && self.human_invocations == 0
    }
}

/// Compute the agent-lift signals from already-windowed event slices.
///
/// Both slices are assumed pre-filtered to the reporting window by the
/// caller — this function does no time filtering of its own so it stays a
/// pure fixtures→numbers transform for testing. trace:STORY-477 | ai:claude
pub fn compute_agent_lift(
    auto_complete_events: &[AutoCompleteEvent],
    usage_events: &[UsageEvent],
) -> AgentLift {
    let mut lift = AgentLift::default();

    let mut specs: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut builds: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for ev in auto_complete_events {
        lift.drain_runs += 1;
        if ev.is_failure() {
            lift.drain_failed += 1;
        } else {
            lift.drain_success += 1;
        }
        if !ev.spec_id.is_empty() {
            specs.insert(ev.spec_id.as_str());
        }
        if let Some(sha) = ev.binary_sha.as_deref() {
            if !sha.is_empty() {
                builds.insert(sha);
            }
        }
        for reb in &ev.auto_rebase {
            lift.stale_base_attempts += 1;
            if reb.outcome == "clean" {
                lift.stale_base_recoveries += 1;
            }
        }
    }
    lift.distinct_specs = specs.len();
    lift.distinct_builds = builds.len();

    let mut shapes: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for ev in usage_events {
        lift.human_invocations += 1;
        if !ev.cmd.is_empty() {
            shapes.insert(ev.cmd.as_str());
        }
    }
    lift.human_command_shapes = shapes.len();

    lift
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auto_complete_telemetry::{AutoRebaseEvent, PhaseDuration};

    fn ac_event(
        spec: &str,
        outcome: &str,
        sha: Option<&str>,
        rebases: Vec<&str>,
    ) -> AutoCompleteEvent {
        AutoCompleteEvent {
            spec_id: spec.to_string(),
            started_at: "2026-06-06T10:00:00Z".to_string(),
            completed_at: "2026-06-06T10:05:00Z".to_string(),
            outcome: outcome.to_string(),
            variant: "full".to_string(),
            failed_phase: if outcome == "success" { None } else { Some(2) },
            failure_kind: if outcome == "success" {
                None
            } else {
                Some("ci-red".to_string())
            },
            failure_message: None,
            phase_durations: vec![PhaseDuration {
                phase: 1,
                slug: "implementer".to_string(),
                elapsed_ms: 1000,
            }],
            total_ms: 5000,
            drafted_bug: None,
            binary_sha: sha.map(|s| s.to_string()),
            auto_rebase: rebases
                .into_iter()
                .map(|o| AutoRebaseEvent {
                    phase: 3,
                    pr_number: 42,
                    outcome: o.to_string(),
                })
                .collect(),
            lifecycle_skips: Vec::new(),
        }
    }

    fn usage_event(cmd: &str) -> UsageEvent {
        UsageEvent {
            ts: "2026-06-06T10:00:00Z".to_string(),
            cmd: cmd.to_string(),
            args_count: 1,
            exit_code: 0,
            duration_ms: 12,
            binary_sha: None,
            role: None,
            scope: None,
        }
    }

    fn commit(date: &str, message: &str) -> CommitCorpusEntry {
        CommitCorpusEntry {
            date: date.to_string(),
            message: message.to_string(),
        }
    }

    #[test]
    fn ai_lift_reports_not_adopted_when_no_trailers_exist() {
        let lift = compute_ai_lift(&[
            commit("2026-01-02", "initial commit"),
            commit("2026-01-03", "docs update"),
        ]);

        assert_eq!(lift.total_commits, 2);
        assert_eq!(lift.trailer_commits, 0);
        assert!(!lift.convention_adopted());
        assert_eq!(lift.buckets[0].month, "2026-01");
        assert_eq!(lift.buckets[0].coverage(), 0.0);
        assert!(lift.tools.is_empty());
    }

    #[test]
    fn ai_lift_splits_multi_tool_trailers_and_buckets_by_month() {
        let lift = compute_ai_lift(&[
            commit("2026-01-02", "[AI:codex+claude] feat: one"),
            commit("2026-01-03", "human commit"),
            commit("2026-02-01", "[AI:codex] fix: two"),
        ]);

        assert!(lift.convention_adopted());
        assert_eq!(lift.total_commits, 3);
        assert_eq!(lift.trailer_commits, 2);
        assert_eq!(lift.tools.get("codex"), Some(&2));
        assert_eq!(lift.tools.get("claude"), Some(&1));
        assert_eq!(lift.buckets.len(), 2);
        assert_eq!(lift.buckets[0].month, "2026-01");
        assert_eq!(lift.buckets[0].total_commits, 2);
        assert_eq!(lift.buckets[0].trailer_commits, 1);
        assert_eq!(lift.buckets[1].month, "2026-02");
        assert_eq!(lift.buckets[1].coverage(), 1.0);
    }

    #[test]
    fn ai_lift_reports_confidence_band_sparsity() {
        let lift = compute_ai_lift(&[
            commit("2026-01-01", "[AI:codex:high] confident"),
            commit("2026-01-02", "[AI:claude:med] medium"),
            commit("2026-01-03", "[AI:codex] unspecified"),
            commit("2026-01-04", "human"),
        ]);

        assert_eq!(lift.confidence.get("high"), Some(&1));
        assert_eq!(lift.confidence.get("med"), Some(&1));
        assert_eq!(lift.confidence.get("unspecified"), Some(&1));
        assert_eq!(lift.confidence_tagged_commits(), 2);
        assert_eq!(lift.confidence_sparsity(), "present");
    }

    #[test]
    fn ai_lift_marks_low_confidence_adoption_as_sparse() {
        let mut commits = vec![commit("2026-01-01", "[AI:codex:low] one")];
        for day in 2..=20 {
            commits.push(commit(&format!("2026-01-{day:02}"), "human"));
        }

        let lift = compute_ai_lift(&commits);

        assert_eq!(lift.confidence_tagged_commits(), 1);
        assert_eq!(lift.confidence_sparsity(), "sparse");
    }

    #[test]
    fn ai_trailer_parser_ignores_empty_tool_names() {
        assert!(parse_ai_trailer_body("").is_none());
        assert_eq!(
            parse_ai_trailer_body("codex+:high").unwrap(),
            AiTrailer {
                tools: vec!["codex".to_string()],
                confidence: Some("high".to_string())
            }
        );
    }

    #[test]
    fn ai_trailer_parser_uses_only_leading_commit_trailer() {
        let lift = compute_ai_lift(&[
            commit(
                "2026-01-01",
                "[AI:claude] fix: mention [AI:tool] as an example",
            ),
            commit("2026-01-02", "docs: mention [AI:codex] as prose"),
        ]);

        assert_eq!(lift.trailer_commits, 1);
        assert_eq!(lift.tools.get("claude"), Some(&1));
        assert_eq!(lift.tools.get("tool"), None);
        assert_eq!(lift.tools.get("codex"), None);
    }

    #[test]
    fn empty_inputs_yield_empty_report() {
        let lift = compute_agent_lift(&[], &[]);
        assert!(lift.is_empty());
        assert_eq!(lift.drain_runs, 0);
        assert_eq!(lift.drain_success_rate(), 0.0);
        assert_eq!(lift.stale_base_recovery_rate(), 0.0);
    }

    #[test]
    fn drain_success_rate_counts_outcomes() {
        let events = vec![
            ac_event("STORY-1", "success", Some("aaa"), vec![]),
            ac_event("STORY-2", "success", Some("aaa"), vec![]),
            ac_event("STORY-3", "failed", Some("aaa"), vec![]),
        ];
        let lift = compute_agent_lift(&events, &[]);
        assert_eq!(lift.drain_runs, 3);
        assert_eq!(lift.drain_success, 2);
        assert_eq!(lift.drain_failed, 1);
        assert!((lift.drain_success_rate() - 2.0 / 3.0).abs() < 1e-9);
        assert!(!lift.is_empty());
    }

    #[test]
    fn distinct_specs_and_builds_dedupe() {
        let events = vec![
            ac_event("STORY-1", "success", Some("aaa"), vec![]),
            // same spec re-driven, different build
            ac_event("STORY-1", "failed", Some("bbb"), vec![]),
            ac_event("STORY-2", "success", Some("bbb"), vec![]),
        ];
        let lift = compute_agent_lift(&events, &[]);
        assert_eq!(lift.distinct_specs, 2, "STORY-1 + STORY-2");
        assert_eq!(lift.distinct_builds, 2, "aaa + bbb");
        assert_eq!(lift.drain_runs, 3);
    }

    #[test]
    fn empty_spec_and_sha_do_not_inflate_distinct_counts() {
        let events = vec![
            ac_event("", "success", Some(""), vec![]),
            ac_event("", "success", None, vec![]),
        ];
        let lift = compute_agent_lift(&events, &[]);
        assert_eq!(lift.distinct_specs, 0);
        assert_eq!(lift.distinct_builds, 0);
        assert_eq!(lift.drain_runs, 2);
    }

    #[test]
    fn stale_base_recoveries_count_only_clean() {
        let events = vec![
            ac_event("STORY-1", "success", Some("aaa"), vec!["clean", "clean"]),
            ac_event(
                "STORY-2",
                "failed",
                Some("aaa"),
                vec!["conflict", "skipped:allow-stale", "failed"],
            ),
        ];
        let lift = compute_agent_lift(&events, &[]);
        assert_eq!(lift.stale_base_attempts, 5);
        assert_eq!(lift.stale_base_recoveries, 2);
        assert!((lift.stale_base_recovery_rate() - 2.0 / 5.0).abs() < 1e-9);
    }

    #[test]
    fn human_activity_counts_invocations_and_distinct_shapes() {
        let usage = vec![
            usage_event("queue list"),
            usage_event("queue list"),
            usage_event("show"),
            usage_event(""),
        ];
        let lift = compute_agent_lift(&[], &usage);
        assert_eq!(lift.human_invocations, 4);
        assert_eq!(
            lift.human_command_shapes, 2,
            "queue list + show; empty skipped"
        );
    }

    #[test]
    fn combined_report_blends_both_sources() {
        let ac = vec![
            ac_event("STORY-1", "success", Some("aaa"), vec!["clean"]),
            ac_event("STORY-2", "success", Some("aaa"), vec![]),
        ];
        let usage = vec![usage_event("list"), usage_event("show")];
        let lift = compute_agent_lift(&ac, &usage);
        assert_eq!(lift.drain_runs, 2);
        assert_eq!(lift.drain_success, 2);
        assert_eq!(lift.drain_success_rate(), 1.0);
        assert_eq!(lift.distinct_specs, 2);
        assert_eq!(lift.distinct_builds, 1);
        assert_eq!(lift.stale_base_recoveries, 1);
        assert_eq!(lift.human_invocations, 2);
        assert_eq!(lift.human_command_shapes, 2);
        assert!(!lift.is_empty());
    }
}
