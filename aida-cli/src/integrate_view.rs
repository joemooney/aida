//! `aida integrate` (bare) — the read-only integrator throughput view.
//!
//! # Why this exists
//!
//! The integrator seat answers one question on a glance: *what would I work
//! next, and is `main` actually moving?* This module is the read-only VISIBILITY
//! half of that seat (the drain/merge half is a separate slice). It assembles
//! three already-existing reads into one screen:
//!
//! 1. the focus-scoped queue (what's routed and waiting),
//! 2. live throughput (time since the last merge to `origin/main`, recent-merge
//!    counts, and a main-idle indicator), and
//! 3. active fan-out work (which sessions/agents hold which specs right now).
//!
//! It writes nothing, touches no network beyond a local `git log` read, and runs
//! cache-backed — a cheap status read, never a drain.
//!
//! The CLI handler in `main.rs` does the impure gathering (queue read, the
//! running-work table, the `git log`/event reads) and hands the pure parts in
//! here. The helpers below — the throughput summary and the main-idle verdict —
//! are side-effect-free so the decision logic is exhaustively unit-testable from
//! fixtures, the same discipline `events`/`watch` follow.
//!
//! trace:TASK-1034 trace:STORY-718 | ai:claude

use std::path::Path;

use chrono::{DateTime, Duration, Utc};

use crate::events::{Event, EventKind};

/// Default main-idle threshold: no merge to `origin/main` in this many minutes
/// flips the indicator from "moving" to "idle". A glanceable default — long
/// enough that a healthy drain never trips it mid-cycle, short enough that a
/// stalled `main` is noticed.
pub const DEFAULT_IDLE_THRESHOLD_MINS: i64 = 30;

/// How many recent commits on `origin/main` to scan for merge timing. Bounds the
/// `git log` read so the view stays cheap even on a long-lived repo.
pub const GIT_LOG_SCAN_LIMIT: usize = 100;

/// A glanceable throughput summary derived from a list of merge timestamps
/// (the commit times on `origin/main`, or the event stream's `pr-merged` times
/// when no remote ref is reachable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThroughputSummary {
    /// The most recent merge timestamp, if any merges are known.
    pub last_merge: Option<DateTime<Utc>>,
    /// Merges within the trailing hour.
    pub merges_last_hour: usize,
    /// Merges within the trailing 24 hours.
    pub merges_last_day: usize,
}

/// Count merges within the trailing 1h / 24h windows and find the most recent.
/// PURE — the windows are computed off the passed-in `now`, so the summary is
/// unit-testable from a fixture timestamp list with no clock dependency.
pub fn summarize_throughput(
    merge_times: &[DateTime<Utc>],
    now: DateTime<Utc>,
) -> ThroughputSummary {
    let one_hour_ago = now - Duration::hours(1);
    let one_day_ago = now - Duration::hours(24);
    let last_merge = merge_times.iter().copied().max();
    let merges_last_hour = merge_times
        .iter()
        .filter(|t| **t >= one_hour_ago && **t <= now)
        .count();
    let merges_last_day = merge_times
        .iter()
        .filter(|t| **t >= one_day_ago && **t <= now)
        .count();
    ThroughputSummary {
        last_merge,
        merges_last_hour,
        merges_last_day,
    }
}

/// The main-idle verdict: whether `origin/main` has gone quiet, plus the elapsed
/// minutes since the last merge (when one is known).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainIdleVerdict {
    /// True when no merge has landed within the threshold window.
    pub idle: bool,
    /// Minutes since the last merge; `None` when no merge is known at all (a
    /// fresh / empty history is treated as idle).
    pub minutes_since_last_merge: Option<i64>,
}

/// Decide whether `main` is idle. PURE over `(last_merge, now, threshold)` so
/// the matrix — moving / idle / never-merged — is testable without a clock or a
/// git repo, the same discipline as `ci_idle_timeout::ci_wait_verdict`.
///
/// No known merge at all reads as **idle** (a stalled or never-started main),
/// never as "moving".
pub fn main_idle_verdict(
    last_merge: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    threshold_mins: i64,
) -> MainIdleVerdict {
    match last_merge {
        Some(ts) => {
            let mins = (now - ts).num_minutes().max(0);
            MainIdleVerdict {
                idle: mins >= threshold_mins,
                minutes_since_last_merge: Some(mins),
            }
        }
        None => MainIdleVerdict {
            idle: true,
            minutes_since_last_merge: None,
        },
    }
}

/// Extract merge timestamps from the drain event stream — the `pr-merged` verbs.
/// PURE. Used as the throughput source when no `origin/main` ref is reachable
/// (offline clone), so the view still shows recent activity from the local
/// drain's own record.
pub fn merge_times_from_events(events: &[Event]) -> Vec<DateTime<Utc>> {
    events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::PrMerged { .. }))
        .map(|e| e.ts)
        .collect()
}

/// Extract the timestamps of completed drains (`queue-drained` verbs) from the
/// event stream. PURE. A secondary throughput signal — how many unattended
/// drains have finished — surfaced alongside the merge counts.
pub fn drain_times_from_events(events: &[Event]) -> Vec<DateTime<Utc>> {
    events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::QueueDrained { .. }))
        .map(|e| e.ts)
        .collect()
}

/// Read `.aida/events.jsonl` and parse each line into an [`Event`], tolerantly
/// skipping blank / malformed lines (the same forgiving follow `watch` uses). A
/// missing file is not an error — it yields an empty list (no drain has emitted
/// yet). Best-effort: any IO error degrades to empty.
pub fn read_events(project_root: &Path) -> Vec<Event> {
    let path = crate::events::events_path(project_root);
    let body = match std::fs::read_to_string(&path) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    body.lines()
        .filter_map(|line| {
            let t = line.trim();
            if t.is_empty() {
                return None;
            }
            serde_json::from_str::<Event>(t).ok()
        })
        .collect()
}

/// Read recent merge-commit timestamps from `git log origin/main`. Best-effort:
/// any failure (no remote ref, git missing, parse error) degrades to an empty
/// list so the caller can fall back to the event stream. The repo squash-merges
/// PRs, so each commit on `origin/main` is effectively one landed PR — its
/// committer timestamp is the merge time.
pub fn read_git_merge_times(project_root: &Path, limit: usize) -> Vec<DateTime<Utc>> {
    let out = match std::process::Command::new("git")
        .current_dir(project_root)
        .args(["log", "origin/main", "--format=%cI", &format!("-n{limit}")])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    parse_git_iso_times(&String::from_utf8_lossy(&out.stdout))
}

/// Parse the `%cI` (strict-ISO committer time) lines `git log` emits into UTC
/// timestamps, skipping any unparseable line. PURE — split out so the parsing
/// is unit-testable without spawning git.
pub fn parse_git_iso_times(stdout: &str) -> Vec<DateTime<Utc>> {
    stdout
        .lines()
        .filter_map(|line| {
            let t = line.trim();
            if t.is_empty() {
                return None;
            }
            DateTime::parse_from_rfc3339(t)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Event;

    fn ts(mins_ago: i64, now: DateTime<Utc>) -> DateTime<Utc> {
        now - Duration::minutes(mins_ago)
    }

    #[test]
    fn summarize_counts_each_window_and_finds_latest() {
        let now = Utc::now();
        // merges at 5m, 40m, 90m, and 30h ago.
        let merges = vec![ts(5, now), ts(40, now), ts(90, now), ts(30 * 60, now)];
        let s = summarize_throughput(&merges, now);
        // Last hour: only the 5m and 40m ones.
        assert_eq!(s.merges_last_hour, 2);
        // Last day: all but the 30h one.
        assert_eq!(s.merges_last_day, 3);
        // Latest is the 5m-ago merge.
        assert_eq!(s.last_merge, Some(ts(5, now)));
    }

    #[test]
    fn summarize_empty_history_has_no_last_merge() {
        let now = Utc::now();
        let s = summarize_throughput(&[], now);
        assert_eq!(s.last_merge, None);
        assert_eq!(s.merges_last_hour, 0);
        assert_eq!(s.merges_last_day, 0);
    }

    #[test]
    fn idle_verdict_moving_when_recent() {
        let now = Utc::now();
        let v = main_idle_verdict(Some(ts(10, now)), now, DEFAULT_IDLE_THRESHOLD_MINS);
        assert!(!v.idle, "a 10m-old merge is well within the 30m threshold");
        assert_eq!(v.minutes_since_last_merge, Some(10));
    }

    #[test]
    fn idle_verdict_idle_when_stale_past_threshold() {
        let now = Utc::now();
        let v = main_idle_verdict(Some(ts(45, now)), now, DEFAULT_IDLE_THRESHOLD_MINS);
        assert!(v.idle, "a 45m-old merge trips the 30m idle threshold");
        assert_eq!(v.minutes_since_last_merge, Some(45));
    }

    #[test]
    fn idle_verdict_exactly_at_threshold_is_idle() {
        let now = Utc::now();
        let v = main_idle_verdict(Some(ts(30, now)), now, 30);
        assert!(
            v.idle,
            "at exactly the threshold the indicator flips to idle"
        );
    }

    #[test]
    fn idle_verdict_never_merged_reads_idle() {
        let now = Utc::now();
        let v = main_idle_verdict(None, now, DEFAULT_IDLE_THRESHOLD_MINS);
        assert!(v.idle, "an empty history is treated as idle, never moving");
        assert_eq!(v.minutes_since_last_merge, None);
    }

    #[test]
    fn merge_and_drain_times_extracted_from_event_stream() {
        let now = Utc::now();
        let mut merged = Event::new(None, "", EventKind::PrMerged { pr: 1207 });
        merged.ts = ts(3, now);
        let phase = Event::new(
            Some("STORY-1".into()),
            "",
            EventKind::PhaseEntered {
                idx: 1,
                slug: "implementer".into(),
            },
        );
        let mut drained = Event::new(
            None,
            "",
            EventKind::QueueDrained {
                shipped: 4,
                shelved: 1,
            },
        );
        drained.ts = ts(1, now);
        let events = vec![merged, phase, drained];

        let merges = merge_times_from_events(&events);
        assert_eq!(merges, vec![ts(3, now)], "only the pr-merged event counts");
        let drains = drain_times_from_events(&events);
        assert_eq!(
            drains,
            vec![ts(1, now)],
            "only the queue-drained event counts"
        );
    }

    #[test]
    fn parse_git_iso_times_skips_blank_and_garbage() {
        let stdout = "2026-06-29T10:15:00+00:00\n\nnot-a-date\n2026-06-29T09:00:00-04:00\n";
        let times = parse_git_iso_times(stdout);
        assert_eq!(times.len(), 2, "two valid lines, two skipped");
        // Both normalize to UTC; the second was -04:00 so its UTC hour is 13.
        assert_eq!(times[1].to_rfc3339(), "2026-06-29T13:00:00+00:00");
    }

    #[test]
    fn read_events_tolerates_missing_file_and_bad_lines() {
        let dir = tempfile::tempdir().unwrap();
        // No events file yet.
        assert!(read_events(dir.path()).is_empty());

        // Write a mix of one good line and noise.
        let path = crate::events::events_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let good =
            serde_json::to_string(&Event::new(None, "", EventKind::PrMerged { pr: 9 })).unwrap();
        std::fs::write(&path, format!("{good}\n\n{{not json\n")).unwrap();
        let evs = read_events(dir.path());
        assert_eq!(evs.len(), 1, "the malformed and blank lines are skipped");
        assert!(matches!(evs[0].kind, EventKind::PrMerged { .. }));
    }
}
