//! `aida usage` command cluster (STORY-122 / STORY-709 / TASK-266 / TASK-872 /
//! STORY-530 / EPIC-36).
//!
//! The usage-telemetry query surface: the top-N default view, `--errors`,
//! `--unused`, `--slowest` + `--events` (the performance lens), `--read-write`
//! (the trace-read-rate audit), and `--auto-complete` (orchestrator telemetry
//! views). Reads the per-command usage log (`~/.aida/usage.jsonl`) and the
//! orchestrator log (`~/.aida/auto-complete.jsonl`); never writes them.
//! Extracted verbatim from `main.rs` (SPIKE-78); no behavior change.

use anyhow::Result;
use colored::Colorize;

use aida_core::RequirementsStore;

use crate::{
    auto_complete, auto_complete_telemetry, bug_status, find_project_root, health_metrics,
    humanize_relative, parse_days_arg, usage,
};

/// How a usage header names its window: `("in the last", "7d")` for a
/// compact relative duration, otherwise `("since", <resolved local time>)`
/// so an absolute or `... ago` value reads naturally.
// trace:TASK-1509 | ai:claude
fn window_label(raw: &str) -> (&'static str, String) {
    window_label_at(raw, chrono::Utc::now(), &chrono::Local)
}

/// [`window_label`] rendered for a header: `in the last 7d` or
/// `since 2026-09-01 00:00 -07:00`, with the value highlighted.
// trace:TASK-1509 | ai:claude
fn window_phrase(raw: &str) -> String {
    let (lead, value) = window_label(raw);
    format!("{} {}", lead, value.cyan())
}

/// [`window_label`] against an explicit `now` and timezone, for tests.
// trace:TASK-1509 | ai:claude
pub(crate) fn window_label_at<Tz: chrono::TimeZone>(
    raw: &str,
    now: chrono::DateTime<chrono::Utc>,
    tz: &Tz,
) -> (&'static str, String)
where
    Tz::Offset: std::fmt::Display,
{
    let trimmed = raw.trim();
    let is_compact_duration = trimmed
        .char_indices()
        .next_back()
        .is_some_and(|(idx, unit)| {
            matches!(unit, 'm' | 'h' | 'd' | 'w')
                && idx > 0
                && trimmed[..idx].chars().all(|c| c.is_ascii_digit())
        });
    if is_compact_duration {
        return ("in the last", trimmed.to_string());
    }
    match crate::queue_cmd::parse_since_arg_at(trimmed, now, tz) {
        Ok(at) => (
            "since",
            at.with_timezone(tz)
                .format("%Y-%m-%d %H:%M %:z")
                .to_string(),
        ),
        Err(_) => ("since", trimmed.to_string()),
    }
}

// ----------------------------------------------------------------------------
// `aida usage slowest` + `aida usage events` — the performance lens
// (STORY-709). Both read the SAME `~/.aida/usage.jsonl` log via
// `usage::read_events()` and the same `UsageEvent` struct (cmd + duration_ms +
// exit_code + ts) — no new capture, no new file. `--slowest` aggregates
// latency per command shape; `--events` streams the raw rows.
// ----------------------------------------------------------------------------

/// Latency aggregate for one command shape over a telemetry window.
// trace:STORY-709 | ai:claude
struct LatencyRow {
    cmd: String,
    count: u64,
    p50: u64,
    p95: u64,
    max: u64,
}

/// Nearest-rank percentile over a slice of `duration_ms` samples.
///
/// `pct` is a fraction in `[0.0, 1.0]` (e.g. 0.5 for p50, 0.95 for p95).
/// Returns 0 for an empty input. The samples are sorted internally; the
/// nearest-rank index is `ceil(pct * n) - 1`, clamped into bounds — so
/// `percentile(.., 1.0)` is the max and `percentile(.., 0.0)` is the min.
/// Pure over its input so it can be unit-tested.
// trace:STORY-709 | ai:claude
fn percentile(durations: &[u64], pct: f64) -> u64 {
    if durations.is_empty() {
        return 0;
    }
    let mut sorted = durations.to_vec();
    sorted.sort_unstable();
    let n = sorted.len();
    // Nearest-rank: rank = ceil(pct * n), 1-indexed, clamped to [1, n].
    let rank = (pct * n as f64).ceil() as usize;
    let idx = rank.clamp(1, n) - 1;
    sorted[idx]
}

/// Group windowed events by command shape and compute p50/p95/max + count
/// of `duration_ms` per shape. Pure over its inputs (testable). Sorted
/// slowest-first by p95, tie-broken by max.
// trace:STORY-709 | ai:claude
fn aggregate_latency(
    events: &[usage::UsageEvent],
    since: chrono::DateTime<chrono::Utc>,
) -> Vec<LatencyRow> {
    let mut by_cmd: std::collections::HashMap<String, Vec<u64>> = std::collections::HashMap::new();
    for ev in events {
        let Ok(ts) = chrono::DateTime::parse_from_rfc3339(&ev.ts) else {
            continue;
        };
        if ts.with_timezone(&chrono::Utc) < since {
            continue;
        }
        by_cmd
            .entry(ev.cmd.clone())
            .or_default()
            .push(ev.duration_ms);
    }
    let mut rows: Vec<LatencyRow> = by_cmd
        .into_iter()
        .map(|(cmd, durations)| {
            let max = durations.iter().copied().max().unwrap_or(0);
            LatencyRow {
                cmd,
                count: durations.len() as u64,
                p50: percentile(&durations, 0.50),
                p95: percentile(&durations, 0.95),
                max,
            }
        })
        .collect();
    // Slowest-first: rank by p95, tie-break by max, then count (stable enough).
    rows.sort_by(|a, b| {
        b.p95
            .cmp(&a.p95)
            .then_with(|| b.max.cmp(&a.max))
            .then_with(|| b.count.cmp(&a.count))
    });
    rows
}

/// Filter raw events to a window + optional command-shape + optional
/// duration-threshold, then return them NEWEST-first. Pure over its inputs
/// (testable).
// trace:STORY-709 | ai:claude
fn filter_events<'a>(
    events: &'a [usage::UsageEvent],
    since: chrono::DateTime<chrono::Utc>,
    cmd_filter: Option<&str>,
    slower_than: Option<u64>,
) -> Vec<&'a usage::UsageEvent> {
    let mut matched: Vec<&usage::UsageEvent> = events
        .iter()
        .filter(|ev| {
            let Ok(ts) = chrono::DateTime::parse_from_rfc3339(&ev.ts) else {
                return false;
            };
            if ts.with_timezone(&chrono::Utc) < since {
                return false;
            }
            if let Some(want) = cmd_filter {
                if ev.cmd != want {
                    return false;
                }
            }
            if let Some(threshold) = slower_than {
                if ev.duration_ms < threshold {
                    return false;
                }
            }
            true
        })
        .collect();
    // Newest-first. The log is append-order; sort by ts descending so we don't
    // assume strict monotonicity (clock skew / concurrent writers).
    matched.sort_by(|a, b| b.ts.cmp(&a.ts));
    matched
}

/// `aida usage slowest`: rank command shapes by latency, slowest-first.
// trace:STORY-709 | ai:claude
fn handle_usage_slowest(since_raw: &str, json_out: bool, limit: usize) -> Result<()> {
    let now = chrono::Utc::now();
    let since = now - parse_days_arg(since_raw)?;
    let events = usage::read_events();
    let rows = aggregate_latency(&events, since);

    if json_out {
        let arr: Vec<serde_json::Value> = rows
            .iter()
            .take(limit)
            .map(|r| {
                serde_json::json!({
                    "cmd": r.cmd,
                    "count": r.count,
                    "p50_ms": r.p50,
                    "p95_ms": r.p95,
                    "max_ms": r.max,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
        return Ok(());
    }

    println!(
        "{} slowest commands {} (by p95 latency)",
        "Usage:".bold(),
        window_phrase(since_raw)
    );
    println!(
        "  {:<24} {:>6} {:>8} {:>8} {:>8}",
        "cmd".dimmed(),
        "count".dimmed(),
        "p50_ms".dimmed(),
        "p95_ms".dimmed(),
        "max_ms".dimmed()
    );
    if rows.is_empty() {
        println!("  (no qualifying events in the window — try `--since 90d` or run more commands)");
        return Ok(());
    }
    for row in rows.iter().take(limit) {
        println!(
            "  {:<24} {:>6} {:>8} {:>8} {:>8}",
            row.cmd.bold(),
            row.count,
            row.p50,
            row.p95,
            row.max
        );
    }
    if rows.len() > limit {
        println!(
            "  {} {} more (pass --limit N to expand)",
            "…".dimmed(),
            (rows.len() - limit).to_string().dimmed()
        );
    }
    Ok(())
}

/// `aida usage events`: stream raw recent events, newest-first, filterable.
// trace:STORY-709 | ai:claude
fn handle_usage_events(
    since_raw: &str,
    json_out: bool,
    limit: usize,
    cmd_filter: Option<&str>,
    slower_than: Option<u64>,
) -> Result<()> {
    let now = chrono::Utc::now();
    let since = now - parse_days_arg(since_raw)?;
    let events = usage::read_events();
    let matched = filter_events(&events, since, cmd_filter, slower_than);

    if json_out {
        let arr: Vec<serde_json::Value> = matched
            .iter()
            .take(limit)
            .map(|ev| {
                serde_json::json!({
                    "ts": ev.ts,
                    "cmd": ev.cmd,
                    "duration_ms": ev.duration_ms,
                    "exit_code": ev.exit_code,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
        return Ok(());
    }

    let mut header = format!(
        "{} recent events {}",
        "Usage:".bold(),
        window_phrase(since_raw)
    );
    if let Some(c) = cmd_filter {
        header.push_str(&format!(" (cmd = {})", c.cyan()));
    }
    if let Some(t) = slower_than {
        header.push_str(&format!(" (slower than {}ms)", t.to_string().cyan()));
    }
    println!("{}", header);
    println!(
        "  {:<25} {:<24} {:>10} {:>5}",
        "ts".dimmed(),
        "cmd".dimmed(),
        "duration_ms".dimmed(),
        "exit".dimmed()
    );
    if matched.is_empty() {
        println!("  (no matching events in the window — widen --since or relax the filters)");
        return Ok(());
    }
    for ev in matched.iter().take(limit) {
        let exit_cell = if ev.exit_code == 0 {
            "0".dimmed().to_string()
        } else {
            ev.exit_code.to_string().yellow().to_string()
        };
        println!(
            "  {:<25} {:<24} {:>10} {:>5}",
            ev.ts.dimmed(),
            ev.cmd.bold(),
            ev.duration_ms,
            exit_cell
        );
    }
    if matched.len() > limit {
        println!(
            "  {} {} more (pass --limit N to expand)",
            "…".dimmed(),
            (matched.len() - limit).to_string().dimmed()
        );
    }
    Ok(())
}

// ----------------------------------------------------------------------------
// `aida usage timeline` — the compact scan-first companion to `--events`
// (TASK-1481). Same source, same filter/sort as `filter_events` above; only
// the rendering differs: one dense line per invocation (local time, compact
// duration, a width-capped command shape, a pass/fail mark) instead of
// `--events`'s raw-field table. `--events` stays available, unchanged, for
// when the exact ts/duration_ms/exit_code fields are needed.
// ----------------------------------------------------------------------------

/// Command-shape column width for the timeline's human rendering. Long
/// shapes are ellipsized rather than left to blow out the line — the whole
/// point of this view is staying scannable.
// trace:TASK-1481 | ai:claude
const TIMELINE_CMD_WIDTH: usize = 32;

/// Ellipsize `s` to at most `max` characters (character-count, not bytes, so
/// multi-byte command args don't panic on a mid-char split).
// trace:TASK-1481 | ai:claude
fn truncate_cmd(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

/// Compact human duration: `<1s` as milliseconds, `<1min` as one-decimal
/// seconds, otherwise `MmSSs`. Kept short and fixed-ish width so the column
/// stays scannable next to slow (multi-second/minute) outliers.
// trace:TASK-1481 | ai:claude
fn compact_duration_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{}m{:02}s", ms / 60_000, (ms % 60_000) / 1000)
    }
}

/// Format an RFC3339 UTC timestamp as a compact LOCAL `MM-DD HH:MM:SS`
/// string (14 chars). Unlike `--events`'s raw UTC `ts`, this view exists to
/// correlate "what ran just before this" against a human's own wall clock.
/// An unparseable timestamp falls back to the raw string rather than erroring
/// — a report is more useful with one odd cell than with none.
// trace:TASK-1481 | ai:claude
fn format_timeline_ts(ts: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|_| ts.to_string())
}

/// One rendered (but uncolored) timeline row — pure data, no ANSI, no I/O —
/// so the rendering logic is unit-testable without capturing stdout. The
/// print loop wraps `cmd`/`when` in `.bold()`/`.dimmed()` and derives the
/// pass/fail glyph from `exit_code` at print time.
// trace:TASK-1481 | ai:claude
#[derive(Debug, PartialEq, Eq)]
struct TimelineRow {
    when: String,
    cmd: String,
    duration: String,
    exit_code: i32,
}

/// Build the printable rows for the human timeline view: local timestamp,
/// duration, and the width-truncated command shape, for at most `limit` of
/// `matched` (already windowed/filtered/sorted newest-first by
/// [`filter_events`]). Pure over its inputs.
// trace:TASK-1481 | ai:claude
fn build_timeline_rows(matched: &[&usage::UsageEvent], limit: usize) -> Vec<TimelineRow> {
    matched
        .iter()
        .take(limit)
        .map(|ev| TimelineRow {
            when: format_timeline_ts(&ev.ts),
            cmd: truncate_cmd(&ev.cmd, TIMELINE_CMD_WIDTH),
            duration: compact_duration_ms(ev.duration_ms),
            exit_code: ev.exit_code,
        })
        .collect()
}

/// Build the `--json` payload for the timeline view — the same schema as
/// `--events` (this is the same underlying stream; only the human rendering
/// differs), so a machine consumer gets the raw fields either way. Pure over
/// its inputs.
// trace:TASK-1481 | ai:claude
fn build_timeline_json(matched: &[&usage::UsageEvent], limit: usize) -> Vec<serde_json::Value> {
    matched
        .iter()
        .take(limit)
        .map(|ev| {
            serde_json::json!({
                "ts": ev.ts,
                "cmd": ev.cmd,
                "duration_ms": ev.duration_ms,
                "exit_code": ev.exit_code,
            })
        })
        .collect()
}

/// `aida usage timeline`: one compact line per invocation, newest-first —
/// local timestamp, duration, command shape, and a pass/fail mark. Reuses
/// [`filter_events`] verbatim (same --since/--cmd/--slower-than/--limit
/// semantics as `--events`); only the rendering is denser.
// trace:TASK-1481 | ai:claude
fn handle_usage_timeline(
    since_raw: &str,
    json_out: bool,
    limit: usize,
    cmd_filter: Option<&str>,
    slower_than: Option<u64>,
) -> Result<()> {
    let now = chrono::Utc::now();
    let since = now - parse_days_arg(since_raw)?;
    let events = usage::read_events();
    let matched = filter_events(&events, since, cmd_filter, slower_than);

    if json_out {
        let arr = build_timeline_json(&matched, limit);
        println!("{}", serde_json::to_string_pretty(&arr)?);
        return Ok(());
    }

    // Agent-mode TOON: the same compact tabular convention `aida list` uses
    // (raw field values, not the colored/truncated human cells) so a
    // non-interactive caller gets a token-cheap table rather than the
    // colored/ellipsized rendering below. trace:TASK-1481 | ai:claude
    if crate::agent_output_mode() {
        let rows: Vec<Vec<String>> = matched
            .iter()
            .take(limit)
            .map(|ev| {
                vec![
                    ev.ts.clone(),
                    ev.cmd.clone(),
                    ev.duration_ms.to_string(),
                    ev.exit_code.to_string(),
                ]
            })
            .collect();
        println!(
            "{}",
            crate::toon::table_raw(
                "timeline",
                &["ts", "cmd", "duration_ms", "exit_code"],
                &rows
            )
        );
        return Ok(());
    }

    let mut header = format!(
        "{} recent invocations {}",
        "Usage:".bold(),
        window_phrase(since_raw)
    );
    if let Some(c) = cmd_filter {
        header.push_str(&format!(" (cmd = {})", c.cyan()));
    }
    if let Some(t) = slower_than {
        header.push_str(&format!(" (slower than {}ms)", t.to_string().cyan()));
    }
    println!("{}", header);

    if matched.is_empty() {
        println!("  (no matching events in the window — widen --since or relax the filters)");
        return Ok(());
    }

    println!(
        "  {:<14} {:<w$} {:>7}  {}",
        "ts".dimmed(),
        "cmd".dimmed(),
        "dur".dimmed(),
        "exit".dimmed(),
        w = TIMELINE_CMD_WIDTH
    );
    for row in build_timeline_rows(&matched, limit) {
        let when_cell = format!("{:<14}", row.when);
        let cmd_cell = format!("{:<w$}", row.cmd, w = TIMELINE_CMD_WIDTH);
        let status_cell = if row.exit_code == 0 {
            crate::glyph(crate::glyphs::Glyph::Check)
                .green()
                .to_string()
        } else {
            format!(
                "{} exit {}",
                crate::glyph(crate::glyphs::Glyph::Cross).red(),
                row.exit_code
            )
        };
        println!(
            "  {} {} {:>7}  {}",
            when_cell.dimmed(),
            cmd_cell.bold(),
            row.duration,
            status_cell
        );
    }
    if matched.len() > limit {
        println!(
            "  {} {} more (pass --limit N to expand)",
            "…".dimmed(),
            (matched.len() - limit).to_string().dimmed()
        );
    }
    Ok(())
}

#[cfg(test)]
mod usage_timeline_tests {
    // trace:TASK-1481 | ai:claude
    use super::*;
    use tempfile::tempdir;

    fn ev(ts: &str, cmd: &str, duration_ms: u64, exit_code: i32) -> usage::UsageEvent {
        usage::UsageEvent {
            ts: ts.to_string(),
            cmd: cmd.to_string(),
            args_count: 0,
            exit_code,
            duration_ms,
            binary_sha: None,
            role: None,
            scope: None,
            schedule_source: None,
        }
    }

    #[test]
    fn truncate_cmd_leaves_short_names_untouched() {
        assert_eq!(truncate_cmd("queue list", 32), "queue list");
        assert_eq!(truncate_cmd("", 32), "");
    }

    #[test]
    fn truncate_cmd_ellipsizes_long_names() {
        let long = "a".repeat(50);
        let out = truncate_cmd(&long, 32);
        assert_eq!(out.chars().count(), 32);
        assert!(out.ends_with('…'));
        assert!(out.starts_with(&"a".repeat(31)));
    }

    #[test]
    fn compact_duration_ms_buckets() {
        assert_eq!(compact_duration_ms(0), "0ms");
        assert_eq!(compact_duration_ms(245), "245ms");
        assert_eq!(compact_duration_ms(999), "999ms");
        assert_eq!(compact_duration_ms(1_000), "1.0s");
        assert_eq!(compact_duration_ms(26_400), "26.4s");
        assert_eq!(compact_duration_ms(65_000), "1m05s");
    }

    #[test]
    fn format_timeline_ts_falls_back_on_bad_input() {
        assert_eq!(format_timeline_ts("not-a-timestamp"), "not-a-timestamp");
    }

    #[test]
    fn format_timeline_ts_is_compact_and_local() {
        let out = format_timeline_ts("2026-03-14T09:26:53+00:00");
        // "MM-DD HH:MM:SS" — always 14 chars, whatever the local offset.
        assert_eq!(out.chars().count(), 14);
        assert_eq!(out.chars().nth(2), Some('-'));
        assert_eq!(out.chars().nth(5), Some(' '));
    }

    #[test]
    fn build_timeline_rows_success_row() {
        let e = ev("2026-03-14T09:26:53+00:00", "queue list", 120, 0);
        let rows = build_timeline_rows(&[&e], 20);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].cmd, "queue list");
        assert_eq!(rows[0].duration, "120ms");
        assert_eq!(rows[0].exit_code, 0);
    }

    #[test]
    fn build_timeline_rows_failure_row() {
        let e = ev("2026-03-14T09:26:53+00:00", "status", 26_400, 1);
        let rows = build_timeline_rows(&[&e], 20);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].duration, "26.4s");
        assert_eq!(rows[0].exit_code, 1);
    }

    #[test]
    fn build_timeline_rows_truncates_long_command_names() {
        let long_cmd = "doctor verify-relationships-and-then-some-extra-words-here";
        let e = ev("2026-03-14T09:26:53+00:00", long_cmd, 5, 0);
        let rows = build_timeline_rows(&[&e], 20);
        assert_eq!(rows[0].cmd.chars().count(), TIMELINE_CMD_WIDTH);
        assert!(rows[0].cmd.ends_with('…'));
    }

    #[test]
    fn build_timeline_rows_empty_window_is_empty() {
        assert!(build_timeline_rows(&[], 20).is_empty());
    }

    #[test]
    fn build_timeline_rows_honors_limit() {
        let events: Vec<usage::UsageEvent> = (0..5)
            .map(|i| ev("2026-03-14T09:26:53+00:00", "list", i, 0))
            .collect();
        let refs: Vec<&usage::UsageEvent> = events.iter().collect();
        assert_eq!(build_timeline_rows(&refs, 2).len(), 2);
    }

    #[test]
    fn build_timeline_json_matches_events_schema() {
        let e = ev("2026-03-14T09:26:53+00:00", "queue work", 800, 1);
        let arr = build_timeline_json(&[&e], 20);
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["ts"], "2026-03-14T09:26:53+00:00");
        assert_eq!(arr[0]["cmd"], "queue work");
        assert_eq!(arr[0]["duration_ms"], 800);
        assert_eq!(arr[0]["exit_code"], 1);
    }

    #[test]
    fn build_timeline_json_empty_window_is_empty_array() {
        assert!(build_timeline_json(&[], 20).is_empty());
    }

    /// End-to-end wiring test: a fixture `usage.jsonl` under a temp `HOME`
    /// (never the real `~/.aida/usage.jsonl`) round-trips through
    /// `usage::read_events()` → `filter_events()` → the timeline builders,
    /// covering a success row, a failure row, and the newest-first order.
    // trace:TASK-1481 | ai:claude
    #[test]
    fn timeline_reads_fixture_usage_log_not_real_home() {
        let home = tempdir().unwrap();
        let aida_dir = home.path().join(".aida");
        std::fs::create_dir_all(&aida_dir).unwrap();
        let now = chrono::Utc::now();
        let earlier = (now - chrono::Duration::seconds(30)).to_rfc3339();
        let later = now.to_rfc3339();
        let fixture = format!(
            "{}\n{}\n",
            serde_json::json!({
                "ts": earlier, "cmd": "queue list", "args_count": 0,
                "exit_code": 0, "duration_ms": 120
            }),
            serde_json::json!({
                "ts": later, "cmd": "status", "args_count": 0,
                "exit_code": 1, "duration_ms": 26_000
            }),
        );
        std::fs::write(aida_dir.join("usage.jsonl"), fixture).unwrap();

        // `AIDA_HOME` too: on Windows `dirs::home_dir()` ignores `HOME`, so the
        // fixture home is only reachable through the `AIDA_HOME` override.
        // trace:BUG-1646 | ai:claude
        let _env = crate::test_env::EnvVarsGuard::set(&[
            ("HOME", home.path().to_str().unwrap()),
            ("AIDA_HOME", home.path().to_str().unwrap()),
        ]);
        let events = usage::read_events();
        assert_eq!(events.len(), 2, "must read the fixture, not the real home");

        let since = now - chrono::Duration::days(1);
        let matched = filter_events(&events, since, None, None);
        assert_eq!(matched.len(), 2);
        // Newest-first: the failing "status" row comes before "queue list".
        assert_eq!(matched[0].cmd, "status");
        assert_eq!(matched[1].cmd, "queue list");

        let rows = build_timeline_rows(&matched, 20);
        assert_eq!(rows[0].exit_code, 1);
        assert_eq!(rows[1].exit_code, 0);

        let json = build_timeline_json(&matched, 20);
        assert_eq!(json.len(), 2);
    }
}

#[cfg(test)]
mod usage_perf_lens_tests {
    // trace:STORY-709 | ai:claude
    use super::*;

    fn ev(ts: &str, cmd: &str, duration_ms: u64, exit_code: i32) -> usage::UsageEvent {
        usage::UsageEvent {
            ts: ts.to_string(),
            cmd: cmd.to_string(),
            args_count: 0,
            exit_code,
            duration_ms,
            binary_sha: None,
            role: None,
            scope: None,
            schedule_source: None,
        }
    }

    #[test]
    fn percentile_basic_p50_p95_max() {
        // 1..=10: nearest-rank.
        let d: Vec<u64> = (1..=10).collect();
        // p50 → ceil(0.5*10)=5 → index 4 → value 5.
        assert_eq!(percentile(&d, 0.50), 5);
        // p95 → ceil(0.95*10)=10 → index 9 → value 10.
        assert_eq!(percentile(&d, 0.95), 10);
        // p100 → max.
        assert_eq!(percentile(&d, 1.0), 10);
        // p0 → ceil(0)=0 → clamped to rank 1 → min.
        assert_eq!(percentile(&d, 0.0), 1);
    }

    #[test]
    fn percentile_empty_is_zero() {
        assert_eq!(percentile(&[], 0.5), 0);
        assert_eq!(percentile(&[], 0.95), 0);
    }

    #[test]
    fn percentile_single_sample() {
        assert_eq!(percentile(&[42], 0.50), 42);
        assert_eq!(percentile(&[42], 0.95), 42);
        assert_eq!(percentile(&[42], 1.0), 42);
    }

    #[test]
    fn percentile_unsorted_input() {
        // Same multiset as 1..=10, scrambled — must match the sorted result.
        let d = vec![10u64, 3, 7, 1, 9, 2, 8, 4, 6, 5];
        assert_eq!(percentile(&d, 0.50), 5);
        assert_eq!(percentile(&d, 0.95), 10);
    }

    #[test]
    fn aggregate_latency_ranks_slowest_first() {
        let since = chrono::Utc::now() - chrono::Duration::days(30);
        let now = chrono::Utc::now().to_rfc3339();
        let mut events = Vec::new();
        // "slow" — three calls, max 9000.
        for ms in [1000u64, 5000, 9000] {
            events.push(ev(&now, "slow cmd", ms, 0));
        }
        // "fast" — three calls, max 300.
        for ms in [100u64, 200, 300] {
            events.push(ev(&now, "fast cmd", ms, 0));
        }
        let rows = aggregate_latency(&events, since);
        assert_eq!(rows.len(), 2);
        // Slowest (higher p95) first.
        assert_eq!(rows[0].cmd, "slow cmd");
        assert_eq!(rows[0].count, 3);
        assert_eq!(rows[0].max, 9000);
        assert_eq!(rows[1].cmd, "fast cmd");
        assert_eq!(rows[1].max, 300);
    }

    #[test]
    fn aggregate_latency_excludes_out_of_window() {
        let since = chrono::Utc::now() - chrono::Duration::days(7);
        let recent = chrono::Utc::now().to_rfc3339();
        let old = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339();
        let events = vec![
            ev(&recent, "in window", 500, 0),
            ev(&old, "out of window", 9999, 0),
        ];
        let rows = aggregate_latency(&events, since);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].cmd, "in window");
    }

    #[test]
    fn filter_events_by_cmd_and_slower_than() {
        let since = chrono::Utc::now() - chrono::Duration::days(30);
        let t = chrono::Utc::now().to_rfc3339();
        let events = vec![
            ev(&t, "queue list", 100, 0),
            ev(&t, "queue list", 5000, 0),
            ev(&t, "status", 26000, 0),
            ev(&t, "queue work", 800, 1),
        ];
        // --cmd "queue list" → two events.
        let by_cmd = filter_events(&events, since, Some("queue list"), None);
        assert_eq!(by_cmd.len(), 2);
        assert!(by_cmd.iter().all(|e| e.cmd == "queue list"));

        // --slower-than 1000 → the 5000 and 26000 ones.
        let slow = filter_events(&events, since, None, Some(1000));
        assert_eq!(slow.len(), 2);
        assert!(slow.iter().all(|e| e.duration_ms >= 1000));

        // Both together: "queue list" AND >= 1000 → just the 5000 one.
        let both = filter_events(&events, since, Some("queue list"), Some(1000));
        assert_eq!(both.len(), 1);
        assert_eq!(both[0].duration_ms, 5000);
    }

    #[test]
    fn filter_events_newest_first() {
        let since = chrono::Utc::now() - chrono::Duration::days(30);
        let older = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        let newer = (chrono::Utc::now() - chrono::Duration::minutes(2)).to_rfc3339();
        // Insert older first to prove the sort, not insertion order, governs.
        let events = vec![ev(&older, "a", 1, 0), ev(&newer, "b", 1, 0)];
        let out = filter_events(&events, since, None, None);
        assert_eq!(out[0].ts, newer);
        assert_eq!(out[1].ts, older);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_usage_command(
    since_raw: &str,
    unused_raw: Option<&str>,
    errors_only: bool,
    json_out: bool,
    limit: usize,
    auto_complete: bool,
    failures: bool,
    pattern: bool,
    health: bool,
    read_write: bool,
    slowest: bool,
    events_lens: bool,
    timeline: bool,
    cmd_filter: Option<&str>,
    slower_than: Option<u64>,
    store: Option<&RequirementsStore>,
) -> Result<()> {
    // TASK-872: `--read-write` runs the trace-read-rate audit — classify the
    // logged command shapes into graph reads vs writes and report the ratio.
    // Its own source-shape (the usage log, classified), so its own handler.
    if read_write {
        return handle_usage_read_write(since_raw, json_out, limit);
    }

    // STORY-709: `--slowest` is the performance lens — aggregate per-command
    // latency (p50/p95/max + count) from the same usage log and rank
    // slowest-first. Reuses the existing read/parse path; no new capture.
    if slowest {
        return handle_usage_slowest(since_raw, json_out, limit);
    }

    // STORY-709: `--events` streams the raw recent event log (ts, cmd,
    // duration_ms, exit_code), newest-first, filterable by --cmd and
    // --slower-than. The "aida events" raw log, delivered as a usage lens.
    if events_lens {
        return handle_usage_events(since_raw, json_out, limit, cmd_filter, slower_than);
    }

    // TASK-1481: `timeline` is the scan-first companion to `--events` — same
    // filtered/sorted event stream, but a denser one-line-per-invocation
    // rendering (local time, compact duration, truncated command shape, a
    // pass/fail mark) so a human can eyeball what ran immediately before a
    // slow command without the raw-field table's width.
    if timeline {
        return handle_usage_timeline(since_raw, json_out, limit, cmd_filter, slower_than);
    }

    // STORY-530: `--health` renders the deterministic Tier-1 health catalog
    // (six pure metrics over the telemetry logs + the spec graph). It reads
    // its own sources, so it gets its own handler.
    if health {
        let project_root = find_project_root().ok();
        return crate::health_cmd::handle_health_command(
            since_raw,
            json_out,
            store,
            project_root.as_deref(),
        );
    }

    // TASK-266: `--auto-complete` switches to the orchestrator telemetry
    // log (`~/.aida/auto-complete.jsonl`) — a different source from the
    // per-command usage log, so it gets its own handler.
    if auto_complete {
        // trace:EPIC-36 — the session-vs-drain gap reads the project's
        // `.aida/headless-logs/`; resolve the root best-effort (None when not
        // inside a project, in which case the gap is simply omitted).
        let project_root = find_project_root().ok();
        return handle_auto_complete_usage(
            since_raw,
            failures,
            pattern,
            json_out,
            limit,
            store,
            project_root.as_deref(),
        );
    }

    let now = chrono::Utc::now();
    let since_window = parse_days_arg(since_raw)?;
    let since = now - since_window;

    let events = usage::read_events();
    if events.is_empty() {
        if json_out {
            println!("[]");
        } else {
            println!(
                "{} (no events yet; the log fills as `aida ...` commands run)",
                "Usage:".bold()
            );
            println!(
                "  {} {}",
                "log:".dimmed(),
                usage::log_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "<home dir unavailable>".to_string())
                    .dimmed()
            );
        }
        return Ok(());
    }

    // --unused: show commands present in `events` but with no record
    // since the cutoff. (A "command not in events at all" is invisible
    // here — we can only report what we've seen.)
    if let Some(raw) = unused_raw {
        // trace:TASK-1509 | ai:claude
        let cutoff_window = crate::queue_cmd::parse_lookback(raw, "--unused")?;
        let cutoff = now - cutoff_window;
        let mut last_seen: std::collections::HashMap<String, chrono::DateTime<chrono::Utc>> =
            std::collections::HashMap::new();
        for ev in &events {
            if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(&ev.ts) {
                let ts = ts.with_timezone(&chrono::Utc);
                let cur = last_seen.entry(ev.cmd.clone()).or_insert(ts);
                if *cur < ts {
                    *cur = ts;
                }
            }
        }
        let mut stale: Vec<(String, chrono::DateTime<chrono::Utc>)> = last_seen
            .into_iter()
            .filter(|(_, ts)| *ts < cutoff)
            .collect();
        stale.sort_by_key(|(_, ts)| *ts);
        if json_out {
            let arr: Vec<serde_json::Value> = stale
                .iter()
                .map(|(cmd, ts)| {
                    serde_json::json!({
                        "cmd": cmd,
                        "last_seen": ts.to_rfc3339(),
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&arr)?);
        } else {
            println!(
                "{} commands NOT used {} (deprecation candidates):",
                "Usage:".bold(),
                window_phrase(raw)
            );
            if stale.is_empty() {
                println!("  (none — everything we've seen has been used recently)");
            } else {
                for (cmd, ts) in stale.iter().take(limit) {
                    let age = humanize_relative(*ts);
                    println!("  {:<24} {}", cmd.bold(), format!("last {}", age).dimmed());
                }
                if stale.len() > limit {
                    println!(
                        "  {} {} more (pass --limit N to expand)",
                        "…".dimmed(),
                        (stale.len() - limit).to_string().dimmed()
                    );
                }
            }
        }
        return Ok(());
    }

    // BUG-699: a recent sub-window (7d, or the whole window if it's shorter) so
    // a stale aggregate can't read as current.
    let recent_since = std::cmp::max(since, now - chrono::Duration::days(7));
    let by_cmd = crate::aggregate_events(&events, since, recent_since);
    let mut rows: Vec<crate::UsageRow> = by_cmd.into_values().collect();
    if errors_only {
        rows.retain(|r| r.errors > 0);
        rows.sort_by(|a, b| {
            b.error_rate()
                .partial_cmp(&a.error_rate())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.count.cmp(&a.count))
        });
    } else {
        rows.sort_by(|a, b| b.count.cmp(&a.count));
    }

    if json_out {
        let arr: Vec<serde_json::Value> = rows
            .iter()
            .take(limit)
            .map(|r| {
                serde_json::json!({
                    "cmd": r.cmd,
                    "count": r.count,
                    "errors": r.errors,
                    "avg_ms": r.avg_ms(),
                    "error_rate": r.error_rate(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
        return Ok(());
    }

    let header = if errors_only {
        format!(
            "{} commands with errors {}",
            "Usage:".bold(),
            window_phrase(since_raw)
        )
    } else {
        format!(
            "{} top commands {}",
            "Usage:".bold(),
            window_phrase(since_raw)
        )
    };
    println!("{}", header);
    println!(
        "  {:<24} {:>6} {:>6} {:>8}",
        "cmd".dimmed(),
        "count".dimmed(),
        "errs".dimmed(),
        "avg_ms".dimmed()
    );
    if rows.is_empty() {
        println!("  (no qualifying events in the window — try `--since 90d` or run more commands)");
        return Ok(());
    }
    for row in rows.iter().take(limit) {
        let err_cell = if row.errors == 0 {
            "0".dimmed().to_string()
        } else {
            row.errors.to_string().yellow().to_string()
        };
        println!(
            "  {:<24} {:>6} {:>6} {:>8}",
            row.cmd.bold(),
            row.count,
            err_cell,
            row.avg_ms()
        );
        // BUG-699: flag a STALE aggregate — errors or latency the recent 7d
        // window no longer shows, so a since-resolved batch doesn't read as a
        // live problem (what misled the advisor into a wrong call).
        if row.recent_count > 0 {
            let stale_errs = row.errors > 0 && row.recent_errors == 0;
            let stale_slow =
                row.avg_ms() > 2000 && row.recent_avg_ms().saturating_mul(3) < row.avg_ms();
            if stale_errs || stale_slow {
                let mut parts = Vec::new();
                if stale_errs {
                    parts.push("0 errors".to_string());
                }
                if stale_slow {
                    parts.push(format!("{}ms avg", row.recent_avg_ms()));
                }
                println!(
                    "    {}",
                    format!(
                        "recent 7d: {} — the row above is the whole-window aggregate (a since-resolved batch)",
                        parts.join(", "),
                    )
                    .dimmed()
                );
            }
        }
    }
    if rows.len() > limit {
        println!(
            "  {} {} more (pass --limit N to expand)",
            "…".dimmed(),
            (rows.len() - limit).to_string().dimmed()
        );
    }

    Ok(())
}

// ----------------------------------------------------------------------------
// `aida usage --read-write` — trace-read-rate audit (TASK-872).
//
// The cheap INTERNAL falsifier for P2b: is AIDA's typed intent graph
// CONSULTED (read) or merely WRITTEN? Classify every logged command shape
// into graph READS vs WRITES vs NEITHER (plumbing), then report total reads,
// total writes, and the read:write ratio over the window. A high ratio is
// direct evidence the rich layer earns its keep; written-but-near-zero-read
// would falsify P2b cleanly.
//
// Honest cap (spec section 10 confound): a high read rate could be operator
// discipline rather than the graph paying its way — but the asymmetry means
// even a positive result is informative, and a *negative* (writes ≫ reads)
// is a clean falsification regardless of the confound.
//
// Measures CLI telemetry only. MCP read tools (show_requirement / query_graph
// / list_requirements) are not in usage.jsonl today; an MCP read counter is a
// follow-up (see the spec's "add an MCP read counter" note), not this slice.
// ----------------------------------------------------------------------------

/// Aggregated graph reads vs writes over a telemetry window.
// trace:TASK-872 | ai:claude
struct ReadWriteTally {
    reads: u64,
    writes: u64,
    read_by_cmd: std::collections::HashMap<String, u64>,
    write_by_cmd: std::collections::HashMap<String, u64>,
}

impl ReadWriteTally {
    fn ratio(&self) -> Option<f64> {
        if self.writes == 0 {
            None
        } else {
            Some(self.reads as f64 / self.writes as f64)
        }
    }
}

/// Walk windowed events, classify each command shape, and tally reads/writes.
/// Pure over its inputs so it can be unit-tested.
// trace:TASK-872 | ai:claude
fn tally_read_write(
    events: &[usage::UsageEvent],
    since: chrono::DateTime<chrono::Utc>,
) -> ReadWriteTally {
    let mut tally = ReadWriteTally {
        reads: 0,
        writes: 0,
        read_by_cmd: std::collections::HashMap::new(),
        write_by_cmd: std::collections::HashMap::new(),
    };
    for ev in events {
        let Ok(ts) = chrono::DateTime::parse_from_rfc3339(&ev.ts) else {
            continue;
        };
        if ts.with_timezone(&chrono::Utc) < since {
            continue;
        }
        match usage::classify_access(&ev.cmd) {
            usage::GraphAccess::Read => {
                tally.reads += 1;
                *tally.read_by_cmd.entry(ev.cmd.clone()).or_insert(0) += 1;
            }
            usage::GraphAccess::Write => {
                tally.writes += 1;
                *tally.write_by_cmd.entry(ev.cmd.clone()).or_insert(0) += 1;
            }
            usage::GraphAccess::Neither => {}
        }
    }
    tally
}

fn handle_usage_read_write(since_raw: &str, json_out: bool, limit: usize) -> Result<()> {
    let now = chrono::Utc::now();
    let since = now - parse_days_arg(since_raw)?;
    let events = usage::read_events();

    let tally = tally_read_write(&events, since);
    let ratio = tally.ratio();

    if json_out {
        let top = |m: &std::collections::HashMap<String, u64>| -> Vec<serde_json::Value> {
            let mut rows: Vec<(String, u64)> = m.iter().map(|(k, v)| (k.clone(), *v)).collect();
            rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            rows.into_iter()
                .take(limit)
                .map(|(cmd, count)| serde_json::json!({ "cmd": cmd, "count": count }))
                .collect()
        };
        let obj = serde_json::json!({
            "window": since_raw,
            "reads": tally.reads,
            "writes": tally.writes,
            "read_write_ratio": ratio,
            "top_reads": top(&tally.read_by_cmd),
            "top_writes": top(&tally.write_by_cmd),
            "note": "CLI telemetry only; MCP read tools (show_requirement/query_graph/list_requirements) are not logged in usage.jsonl — an MCP read counter is a follow-up.",
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    println!(
        "{} trace-read-rate audit {}",
        "Usage:".bold(),
        window_phrase(since_raw)
    );
    println!(
        "  {} is the intent graph consulted, or just written?",
        "Question:".dimmed()
    );
    println!();
    println!("  {:<14} {:>10}", "graph reads".dimmed(), tally.reads);
    println!("  {:<14} {:>10}", "graph writes".dimmed(), tally.writes);
    let ratio_cell = match ratio {
        Some(r) => format!("{:.2} : 1", r).bold().to_string(),
        None if tally.reads > 0 => "∞ (no writes in window)".bold().to_string(),
        None => "n/a (no graph activity in window)".dimmed().to_string(),
    };
    println!("  {:<14} {:>10}", "read:write".dimmed(), ratio_cell);

    if let Some(r) = ratio {
        let verdict = if r >= 1.0 {
            "reads ≥ writes — evidence the typed graph is consulted, not just written".green()
        } else {
            "writes > reads — the graph may be written more than read (watch P2b)".yellow()
        };
        println!("  {} {}", "→".dimmed(), verdict);
    }

    let print_top = |label: &str, m: &std::collections::HashMap<String, u64>| {
        if m.is_empty() {
            return;
        }
        let mut rows: Vec<(&String, &u64)> = m.iter().collect();
        rows.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        println!();
        println!("  {}", label.dimmed());
        for (cmd, count) in rows.into_iter().take(limit.min(10)) {
            println!("    {:<24} {:>6}", cmd.bold(), count);
        }
    };
    print_top("top reads", &tally.read_by_cmd);
    print_top("top writes", &tally.write_by_cmd);

    println!();
    println!(
        "  {} CLI telemetry only. MCP read tools (show_requirement / query_graph /",
        "Note:".dimmed()
    );
    println!(
        "        list_requirements) aren't in usage.jsonl yet — an MCP read counter is a follow-up."
    );

    Ok(())
}

// ----------------------------------------------------------------------------
// `aida usage drains` — orchestrator telemetry views (TASK-266).
// Reads `~/.aida/auto-complete.jsonl` (written one line per `--auto-complete`
// run by `record_auto_complete_run`).
// ----------------------------------------------------------------------------

/// TASK-266: the `aida usage drains` view family. Bare
/// `--auto-complete` prints a success/failure summary plus the most recent
/// failures; `--failures` expands the full failure list; `--pattern` shows
/// the per-phase failure histogram.
// trace:TASK-266 | ai:claude
fn handle_auto_complete_usage(
    since_raw: &str,
    failures: bool,
    pattern: bool,
    json_out: bool,
    limit: usize,
    store: Option<&RequirementsStore>,
    project_root: Option<&std::path::Path>,
) -> Result<()> {
    let now = chrono::Utc::now();
    let since = now - parse_days_arg(since_raw)?;
    let events: Vec<auto_complete_telemetry::AutoCompleteEvent> =
        auto_complete_telemetry::read_events()
            .into_iter()
            .filter(|ev| {
                // Keep events whose completion falls inside the window;
                // an unparseable timestamp is kept rather than dropped.
                chrono::DateTime::parse_from_rfc3339(&ev.completed_at)
                    .map(|t| t.with_timezone(&chrono::Utc) >= since)
                    .unwrap_or(true)
            })
            .collect();

    if events.is_empty() {
        if json_out {
            println!("[]");
        } else {
            println!(
                "{} (no --auto-complete runs recorded {})",
                "Auto-complete:".bold(),
                window_phrase(since_raw)
            );
            println!(
                "  {} {}",
                "log:".dimmed(),
                auto_complete_telemetry::log_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "<home dir unavailable>".to_string())
                    .dimmed()
            );
        }
        return Ok(());
    }

    if pattern {
        return render_auto_complete_pattern(&events, json_out);
    }
    // `--failures` expands the full list; bare `--auto-complete` is the
    // compact overview that caps the list and points at `--failures`.
    render_auto_complete_failures(
        &events,
        json_out,
        limit,
        !failures,
        store,
        since_raw,
        project_root,
    )
}

/// EPIC-36: compose the session-vs-drain misclassification gap from the drain
/// summary (already computed over the auto-complete log) and the headless
/// session logs under `<project_root>/.aida/headless-logs/`. Returns `None`
/// when there's no project root (gap can't be located). Carries the session
/// tally back so the caller can render the per-class breakdown.
// trace:EPIC-36
fn compute_session_drain_gap(
    drain_summary: &auto_complete_telemetry::Summary,
    project_root: Option<&std::path::Path>,
) -> Option<(
    health_metrics::MisclassificationGap,
    health_metrics::SessionTally,
)> {
    let root = project_root?;
    let logs_dir = root.join(".aida").join("headless-logs");
    let sessions = health_metrics::tally_from_dir(&logs_dir);
    let gap =
        health_metrics::compute_gap(&sessions, drain_summary.success_rate(), drain_summary.total);
    Some((gap, sessions))
}

/// Render the summary header + recent-failures list. `overview` (bare
/// `--auto-complete`) caps the list short and adds navigation hints;
/// `--failures` shows the full list up to `limit`.
// trace:TASK-266 | ai:claude
fn render_auto_complete_failures(
    events: &[auto_complete_telemetry::AutoCompleteEvent],
    json_out: bool,
    limit: usize,
    overview: bool,
    store: Option<&RequirementsStore>,
    since_raw: &str,
    project_root: Option<&std::path::Path>,
) -> Result<()> {
    let summary = auto_complete_telemetry::summarize(events);
    // trace:EPIC-36 — session-vs-drain misclassification gap over the headless
    // session logs, alongside the drain success rate computed above.
    let gap = compute_session_drain_gap(&summary, project_root);
    let mut failures: Vec<&auto_complete_telemetry::AutoCompleteEvent> =
        events.iter().filter(|e| e.is_failure()).collect();
    // Newest first — RFC3339 sorts lexically.
    failures.sort_by(|a, b| b.completed_at.cmp(&a.completed_at));
    let cap = if overview { 5 } else { limit };

    if json_out {
        let arr: Vec<serde_json::Value> = failures
            .iter()
            .take(cap)
            .map(|ev| {
                serde_json::json!({
                    "spec_id": ev.spec_id,
                    "completed_at": ev.completed_at,
                    "failed_phase": ev.failed_phase,
                    "failure_kind": ev.failure_kind,
                    "failure_message": ev.failure_message,
                    "drafted_bug": ev.drafted_bug,
                })
            })
            .collect();
        let gap_json = gap.as_ref().map(|(g, tally)| {
            let breakdown: Vec<serde_json::Value> = tally
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
            serde_json::json!({
                "session_success_rate": g.session_success_rate,
                "drain_success_rate": g.drain_success_rate,
                "gap": g.gap(),
                "session_total": g.session_total,
                "drain_total": g.drain_total,
                "insufficient_data": g.has_zero_denominator(),
                "session_breakdown": breakdown,
            })
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "total": summary.total,
                "success": summary.success,
                "failed": summary.failed,
                "success_rate": summary.success_rate(),
                "misclassification_gap": gap_json,
                "failures": arr,
            }))?
        );
        return Ok(());
    }

    println!(
        "{} {} runs {} — {} ok, {} failed ({:.0}% success)",
        "Auto-complete:".bold(),
        summary.total,
        window_phrase(since_raw),
        summary.success.to_string().green(),
        if summary.failed == 0 {
            summary.failed.to_string().dimmed()
        } else {
            summary.failed.to_string().yellow()
        },
        summary.success_rate() * 100.0,
    );

    // trace:EPIC-36 — surface the session-vs-drain misclassification gap: how
    // much work the headless sessions actually completed that the drain scored
    // as a failure.
    if let Some((g, _)) = gap {
        if g.has_zero_denominator() {
            let why = if g.session_total == 0 {
                "no headless session logs"
            } else {
                "no drain runs"
            };
            println!(
                "  {} {}",
                "Misclassification gap:".bold(),
                format!("insufficient data ({why})").dimmed()
            );
        } else {
            let gap_pct = g.gap() * 100.0;
            let gap_cell = format!("{gap_pct:+.0}%");
            let gap_cell = if g.gap() > 0.001 {
                gap_cell.yellow()
            } else if g.gap() < -0.001 {
                gap_cell.red()
            } else {
                gap_cell.dimmed()
            };
            println!(
                "  {} session {:.0}% vs drain {:.0}% → gap {} ({} sessions)",
                "Misclassification gap:".bold(),
                g.session_success_rate * 100.0,
                g.drain_success_rate * 100.0,
                gap_cell,
                g.session_total,
            );
            if g.gap() > 0.001 {
                println!(
                    "  {}",
                    format!(
                        "{} work sessions finished but the orchestrator scored as failed",
                        crate::glyph(crate::glyphs::Glyph::SubArrow)
                    )
                    .dimmed()
                );
            }
        }
    }

    if failures.is_empty() {
        println!(
            "  {}",
            "no failures in the window — the orchestrator is green".green()
        );
        return Ok(());
    }

    println!();
    println!("  {}", "Recent --auto-complete failures:".bold());
    for ev in failures.iter().take(cap) {
        let when = chrono::DateTime::parse_from_rfc3339(&ev.completed_at)
            .map(|t| {
                t.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string()
            })
            .unwrap_or_else(|_| ev.completed_at.clone());
        let phase_n = ev.failed_phase.unwrap_or(0);
        let phase_label = auto_complete::Phase::from_index(i32::from(phase_n))
            .map(|p| p.slug())
            .unwrap_or("?");
        let cause = auto_complete_telemetry::failure_cause_label(ev.failure_kind.as_deref());
        let detail =
            auto_complete_telemetry::failure_detail_first_line(ev.failure_message.as_deref());
        let bug_cell = ev.drafted_bug.as_ref().map(|bug| {
            let status = store.and_then(|s| bug_status(s, bug));
            match status {
                Some(st) => format!(" · {}", format!("{} [{}]", bug, st).cyan()),
                None => format!(" · {}", bug.cyan()),
            }
        });
        println!(
            "    {}  {:<12} phase {} ({})  {} — {}{}",
            when.dimmed(),
            ev.spec_id.bold(),
            phase_n,
            phase_label,
            cause.yellow(),
            detail,
            bug_cell.unwrap_or_default(),
        );
    }

    if failures.len() > cap {
        let more = failures.len() - cap;
        if overview {
            println!(
                "    {} {} more — `aida usage drains --failures`",
                "…".dimmed(),
                more
            );
        } else {
            println!(
                "    {} {} more (pass --limit N to expand)",
                "…".dimmed(),
                more
            );
        }
    }

    if overview {
        println!();
        println!(
            "  {}",
            "`aida usage drains --pattern` — which causes fail most often".dimmed()
        );
    }
    Ok(())
}

/// Render the per-cause failure histogram — the signal for where to invest
/// orchestrator fixes.
// trace:TASK-266 trace:STORY-974 | ai:codex
fn render_auto_complete_pattern(
    events: &[auto_complete_telemetry::AutoCompleteEvent],
    json_out: bool,
) -> Result<()> {
    let hist = auto_complete_telemetry::failure_histogram(events);
    let summary = auto_complete_telemetry::summarize(events);

    if json_out {
        let arr: Vec<serde_json::Value> = hist
            .iter()
            .map(|(cause, count)| {
                serde_json::json!({
                    "cause": cause,
                    "failures": count,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
        return Ok(());
    }

    println!(
        "{} failure-cause frequency — {} failed of {} runs",
        "Auto-complete:".bold(),
        summary.failed,
        summary.total,
    );
    if hist.is_empty() {
        println!("  {}", "no failures recorded — nothing to pattern".green());
        return Ok(());
    }
    let max = hist.iter().map(|(_, c)| *c).max().unwrap_or(1);
    for (cause, count) in &hist {
        // Scale the bar to a 24-column field; never empty for a nonzero count.
        let width = ((*count * 24) / max).max(1);
        println!("  {:<24} {} {}", cause, "█".repeat(width).red(), count,);
    }
    Ok(())
}

#[cfg(test)]
mod task_872_read_write_audit_tests {
    use super::*;
    use crate::usage::UsageEvent;

    fn ev(cmd: &str, ts: &str) -> UsageEvent {
        UsageEvent {
            ts: ts.to_string(),
            cmd: cmd.to_string(),
            args_count: 1,
            exit_code: 0,
            duration_ms: 5,
            binary_sha: None,
            role: None,
            scope: None,
            schedule_source: None,
        }
    }

    #[test]
    fn tally_counts_reads_writes_and_skips_neither() {
        // trace:TASK-872 | ai:claude
        let since = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00+00:00")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let events = vec![
            ev("list", "2026-02-01T00:00:00+00:00"),
            ev("show", "2026-02-01T00:00:00+00:00"),
            ev("queue list", "2026-02-01T00:00:00+00:00"),
            ev("add", "2026-02-01T00:00:00+00:00"),
            ev("pull", "2026-02-01T00:00:00+00:00"), // NEITHER — excluded
            ev("statusline", "2026-02-01T00:00:00+00:00"), // NEITHER — excluded
        ];
        let tally = tally_read_write(&events, since);
        assert_eq!(tally.reads, 3);
        assert_eq!(tally.writes, 1);
        assert_eq!(tally.ratio(), Some(3.0));
    }

    #[test]
    fn tally_windows_out_old_events() {
        // trace:TASK-872 | ai:claude
        let since = chrono::DateTime::parse_from_rfc3339("2026-06-01T00:00:00+00:00")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let events = vec![
            ev("list", "2026-01-01T00:00:00+00:00"), // before window — dropped
            ev("add", "2026-07-01T00:00:00+00:00"),  // in window
        ];
        let tally = tally_read_write(&events, since);
        assert_eq!(tally.reads, 0);
        assert_eq!(tally.writes, 1);
        assert_eq!(tally.ratio(), Some(0.0));
    }

    #[test]
    fn tally_ratio_is_none_when_no_writes() {
        // trace:TASK-872 | ai:claude
        let since = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00+00:00")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let events = vec![ev("list", "2026-02-01T00:00:00+00:00")];
        let tally = tally_read_write(&events, since);
        assert_eq!(tally.reads, 1);
        assert_eq!(tally.writes, 0);
        assert_eq!(tally.ratio(), None);
    }

    #[test]
    fn tally_empty_log_is_graceful() {
        // trace:TASK-872 | ai:claude
        let since = chrono::Utc::now() - chrono::Duration::days(30);
        let tally = tally_read_write(&[], since);
        assert_eq!(tally.reads, 0);
        assert_eq!(tally.writes, 0);
        assert_eq!(tally.ratio(), None);
    }
}
