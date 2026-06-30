//! TASK-266: per-invocation telemetry for `aida queue work --auto-complete`.
//!
//! Local-only, append-only JSONL log at `~/.aida/auto-complete.jsonl`. One
//! line per orchestrator run records the phase outcome — never argument
//! values or file contents (the same privacy floor as `aida usage`,
//! STORY-122). The data powers `aida usage --auto-complete`, and on a phase
//! failure `handle_auto_complete` also auto-drafts a Draft BUG so the
//! friction surfaces back to the project instead of dying in scrollback.
//!
//! Opt-out shares the `aida usage` switch — `AIDA_TELEMETRY=0` or
//! `[telemetry] enabled = false` in `.aida/config.toml` (see
//! [`crate::usage::is_enabled`]).
//!
//! trace:TASK-266 | ai:claude

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Wall time spent in one lifecycle phase of an `--auto-complete` run.
/// trace:TASK-266 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PhaseDuration {
    /// 1-based phase index (1 = implementer … 6 = build).
    pub phase: u8,
    /// Stable phase slug (`implementer`, `ci`, `reviewer`, `merge`, `pull`,
    /// `build`).
    pub slug: String,
    /// Wall time the phase took.
    pub elapsed_ms: u64,
}

/// One phase-local auto-rebase decision during an `--auto-complete` run.
/// STORY-429 records this inside the run event so recovery remains correlated
/// with the lifecycle it helped.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutoRebaseEvent {
    /// 1-based phase index. STORY-429 only records phase 3.
    pub phase: u8,
    /// PR number whose head was considered for rebase.
    pub pr_number: u64,
    /// `clean`, `conflict`, `failed`, or `skipped:<reason>`.
    pub outcome: String,
}

/// One `aida queue work --auto-complete` invocation — appended one-per-run
/// to `~/.aida/auto-complete.jsonl`. trace:TASK-266 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutoCompleteEvent {
    /// The spec the orchestrator was driving.
    pub spec_id: String,
    /// RFC3339 — when the orchestrator started.
    pub started_at: String,
    /// RFC3339 — when it finished (success or failure).
    pub completed_at: String,
    /// `success` or `failed`.
    pub outcome: String,
    /// Which variant ran (`full`, `through-ci`, `through-merge`,
    /// `skip-build`).
    pub variant: String,
    /// 1-based index of the phase that failed; `None` on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_phase: Option<u8>,
    /// Stable failure-kind slug (`ci-red`, `no-pr`, `spawn`, …); `None` on
    /// success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<String>,
    /// The one-line failure reason; `None` on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_message: Option<String>,
    /// Per-phase wall time, in run order.
    pub phase_durations: Vec<PhaseDuration>,
    /// Total wall time of the run.
    pub total_ms: u64,
    /// Spec-id of the Draft BUG auto-filed for this failure, if one was
    /// filed. Set after the BUG is drafted, so the verbatim copy embedded
    /// in that BUG's own description carries `None` here — that is not a
    /// circular reference, it is the pre-draft snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drafted_bug: Option<String>,
    /// Short build SHA of the aida binary (release tracking).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_sha: Option<String>,
    /// STORY-429: phase-3 stale-base auto-rebase attempts / skips.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auto_rebase: Vec<AutoRebaseEvent>,
    /// TASK-525: which `lifecycle:*` short-circuits were active for this run
    /// (`no-ci-wait`, `no-review`, `no-build`), for retro analysis of how often
    /// the fast-track tags fire and on what. Empty when none were set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lifecycle_skips: Vec<String>,
}

impl AutoCompleteEvent {
    /// Whether this run did not succeed.
    pub fn is_failure(&self) -> bool {
        self.outcome != "success"
    }
}

/// Resolve `~/.aida/auto-complete.jsonl`. Returns `None` when the home dir
/// can't be located (treat as "telemetry off" — never error out).
pub fn log_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".aida").join("auto-complete.jsonl"))
}

/// Append a single event as JSONL. Errors are intentionally swallowed —
/// telemetry must never break the foreground command.
pub fn append_event(event: &AutoCompleteEvent) {
    let Some(path) = log_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let Ok(json) = serde_json::to_string(event) else {
        return;
    };
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{json}");
    }
}

/// Read every event from the log (insertion order — oldest first).
/// Best-effort: malformed lines are skipped silently.
pub fn read_events() -> Vec<AutoCompleteEvent> {
    let Some(path) = log_path() else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<AutoCompleteEvent>(l).ok())
        .collect()
}

/// BUG-657: the spec-id of the Draft BUG already auto-filed for an identical
/// recent failure — same spec, same failed phase, same failure kind, with a
/// `completed_at` at or after `cutoff` — scanning `events` newest-first.
///
/// Pure over a slice so the dedup decision is unit-testable without the global
/// `~/.aida/auto-complete.jsonl`. The auto-draft path consults this first and,
/// on a hit, reuses the existing BUG instead of filing a fresh one — so
/// re-running a still-broken `--auto-complete` does not spam the backlog with
/// one Draft per retry (the BUG-638 → BUG-644..649 incident filed 6 identical
/// drafts in a 6-minute window).
// trace:BUG-657 | ai:claude
pub fn dedup_failure_bug(
    events: &[AutoCompleteEvent],
    spec: &str,
    failed_phase: u8,
    failure_kind: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    events
        .iter()
        .rev()
        .filter(|ev| {
            ev.spec_id == spec
                && ev.failed_phase == Some(failed_phase)
                && ev.failure_kind.as_deref() == Some(failure_kind)
                && ev.drafted_bug.is_some()
        })
        .find(|ev| {
            chrono::DateTime::parse_from_rfc3339(&ev.completed_at)
                .map(|t| t.with_timezone(&chrono::Utc) >= cutoff)
                .unwrap_or(false)
        })
        .and_then(|ev| ev.drafted_bug.clone())
}

/// Success / failure tallies over a slice of events.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub total: usize,
    pub success: usize,
    pub failed: usize,
}

impl Summary {
    /// Fraction of runs that succeeded, in `0.0..=1.0`. `0.0` when empty.
    pub fn success_rate(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.success as f64 / self.total as f64
        }
    }
}

/// Tally outcomes across `events`.
pub fn summarize(events: &[AutoCompleteEvent]) -> Summary {
    let mut s = Summary::default();
    for ev in events {
        s.total += 1;
        if ev.is_failure() {
            s.failed += 1;
        } else {
            s.success += 1;
        }
    }
    s
}

/// Count failures per phase, descending by count then ascending phase
/// index. The signal for "which phases of the orchestrator break most
/// often" — used by `aida usage --auto-complete --pattern`.
pub fn failure_histogram(events: &[AutoCompleteEvent]) -> Vec<(u8, usize)> {
    let mut counts: std::collections::HashMap<u8, usize> = std::collections::HashMap::new();
    for ev in events {
        if let Some(phase) = ev.failed_phase {
            *counts.entry(phase).or_insert(0) += 1;
        }
    }
    let mut rows: Vec<(u8, usize)> = counts.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(spec: &str, outcome: &str, failed_phase: Option<u8>) -> AutoCompleteEvent {
        AutoCompleteEvent {
            spec_id: spec.to_string(),
            started_at: "2026-05-16T14:20:00Z".to_string(),
            completed_at: "2026-05-16T14:23:00Z".to_string(),
            outcome: outcome.to_string(),
            variant: "full".to_string(),
            failed_phase,
            failure_kind: failed_phase.map(|_| "ci-red".to_string()),
            failure_message: failed_phase.map(|_| "CI red".to_string()),
            phase_durations: vec![PhaseDuration {
                phase: 1,
                slug: "implementer".to_string(),
                elapsed_ms: 1000,
            }],
            total_ms: 5000,
            drafted_bug: failed_phase.map(|_| "BUG-200".to_string()),
            binary_sha: Some("abc1234".to_string()),
            auto_rebase: Vec::new(),
            lifecycle_skips: Vec::new(),
        }
    }

    #[test]
    fn event_round_trips_through_json() {
        let original = ev("TASK-259", "failed", Some(2));
        let line = serde_json::to_string(&original).unwrap();
        let parsed: AutoCompleteEvent = serde_json::from_str(&line).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn auto_rebase_events_round_trip_inside_run_event() {
        let mut original = ev("STORY-429", "success", None);
        original.auto_rebase.push(AutoRebaseEvent {
            phase: 3,
            pr_number: 234,
            outcome: "clean".to_string(),
        });
        let line = serde_json::to_string(&original).unwrap();
        assert!(line.contains("\"auto_rebase\""));
        let parsed: AutoCompleteEvent = serde_json::from_str(&line).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn success_event_omits_failure_fields_from_json() {
        let line = serde_json::to_string(&ev("TASK-1", "success", None)).unwrap();
        // `skip_serializing_if = Option::is_none` keeps success lines lean.
        assert!(!line.contains("failed_phase"));
        assert!(!line.contains("failure_kind"));
        assert!(!line.contains("drafted_bug"));
    }

    #[test]
    fn is_failure_distinguishes_outcome() {
        assert!(ev("X", "failed", Some(1)).is_failure());
        assert!(!ev("X", "success", None).is_failure());
    }

    #[test]
    fn summarize_tallies_success_and_failure() {
        let events = vec![
            ev("A", "success", None),
            ev("B", "failed", Some(2)),
            ev("C", "success", None),
            ev("D", "failed", Some(2)),
        ];
        let s = summarize(&events);
        assert_eq!(s.total, 4);
        assert_eq!(s.success, 2);
        assert_eq!(s.failed, 2);
        assert!((s.success_rate() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn summary_success_rate_is_zero_when_empty() {
        assert_eq!(summarize(&[]).success_rate(), 0.0);
    }

    #[test]
    fn failure_histogram_ranks_phases_by_count() {
        let events = vec![
            ev("A", "failed", Some(2)),
            ev("B", "failed", Some(2)),
            ev("C", "failed", Some(1)),
            ev("D", "success", None),
        ];
        let hist = failure_histogram(&events);
        // phase 2 (×2) ranks above phase 1 (×1); successes are ignored.
        assert_eq!(hist, vec![(2, 2), (1, 1)]);
    }

    #[test]
    fn failure_histogram_breaks_count_ties_by_phase_index() {
        let events = vec![ev("A", "failed", Some(5)), ev("B", "failed", Some(3))];
        // Equal counts → lower phase index first.
        assert_eq!(failure_histogram(&events), vec![(3, 1), (5, 1)]);
    }

    fn far_cutoff() -> chrono::DateTime<chrono::Utc> {
        // Well before every helper event's `completed_at` (2026-05-16T14:23).
        chrono::DateTime::parse_from_rfc3339("2026-05-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    /// BUG-657: an identical recent failure (same spec / phase / kind, within
    /// the cutoff, with a drafted BUG) is found and its BUG reused.
    #[test]
    fn dedup_finds_matching_recent_failure_bug() {
        let events = vec![ev("BUG-638", "failed", Some(1))];
        let hit = dedup_failure_bug(&events, "BUG-638", 1, "ci-red", far_cutoff());
        assert_eq!(hit, Some("BUG-200".to_string()));
    }

    /// BUG-657: the dedup key is (spec, phase, kind) — any mismatch means no
    /// reuse, so a genuinely different failure still files its own BUG.
    #[test]
    fn dedup_misses_on_a_different_signature() {
        let events = vec![ev("BUG-638", "failed", Some(1))];
        assert_eq!(
            dedup_failure_bug(&events, "BUG-639", 1, "ci-red", far_cutoff()),
            None,
            "different spec"
        );
        assert_eq!(
            dedup_failure_bug(&events, "BUG-638", 2, "ci-red", far_cutoff()),
            None,
            "different phase"
        );
        assert_eq!(
            dedup_failure_bug(&events, "BUG-638", 1, "no-pr", far_cutoff()),
            None,
            "different kind"
        );
    }

    /// BUG-657: an event older than the cutoff is ignored — the BUG would have
    /// aged out, so a fresh one is filed rather than reviving a stale link.
    #[test]
    fn dedup_ignores_events_older_than_cutoff() {
        let events = vec![ev("BUG-638", "failed", Some(1))];
        // Cutoff AFTER the helper event's completed_at (14:23) → too old.
        let cutoff = chrono::DateTime::parse_from_rfc3339("2026-05-16T18:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(
            dedup_failure_bug(&events, "BUG-638", 1, "ci-red", cutoff),
            None
        );
    }

    /// BUG-657: with several matching events the NEWEST drafted BUG wins
    /// (newest-first scan), so retries converge on the latest tracking BUG.
    #[test]
    fn dedup_returns_the_newest_matching_bug() {
        let mut older = ev("BUG-638", "failed", Some(1));
        older.drafted_bug = Some("BUG-100".to_string());
        let mut newer = ev("BUG-638", "failed", Some(1));
        newer.drafted_bug = Some("BUG-300".to_string());
        // Insertion order is oldest-first; the scan reverses it.
        let events = vec![older, newer];
        assert_eq!(
            dedup_failure_bug(&events, "BUG-638", 1, "ci-red", far_cutoff()),
            Some("BUG-300".to_string())
        );
    }

    /// BUG-657: an event with no drafted BUG (the auto-file failed, or it was an
    /// environmental suppression) does not count as a dedup target.
    #[test]
    fn dedup_skips_events_with_no_drafted_bug() {
        let mut ev_no_bug = ev("BUG-638", "failed", Some(1));
        ev_no_bug.drafted_bug = None;
        assert_eq!(
            dedup_failure_bug(&[ev_no_bug], "BUG-638", 1, "ci-red", far_cutoff()),
            None
        );
    }

    #[test]
    fn log_path_ends_with_auto_complete_jsonl() {
        if let Some(p) = log_path() {
            assert!(p.ends_with("auto-complete.jsonl"));
            assert!(p.to_string_lossy().contains(".aida"));
        }
    }
}
