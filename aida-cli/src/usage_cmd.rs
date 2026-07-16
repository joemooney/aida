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
    auto_complete, auto_complete_telemetry, bug_status, find_project_root, handle_health_command,
    health_metrics, humanize_relative, parse_days_arg, usage,
};

// ----------------------------------------------------------------------------
// `aida usage --slowest` + `aida usage --events` — the performance lens
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

/// `aida usage --slowest`: rank command shapes by latency, slowest-first.
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
        "{} slowest commands in the last {} (by p95 latency)",
        "Usage:".bold(),
        since_raw.cyan()
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

/// `aida usage --events`: stream raw recent events, newest-first, filterable.
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
        "{} recent events in the last {}",
        "Usage:".bold(),
        since_raw.cyan()
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

    // STORY-530: `--health` renders the deterministic Tier-1 health catalog
    // (six pure metrics over the telemetry logs + the spec graph). It reads
    // its own sources, so it gets its own handler.
    if health {
        let project_root = find_project_root().ok();
        return handle_health_command(since_raw, json_out, store, project_root.as_deref());
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
        let cutoff_window = parse_days_arg(raw)?;
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
                "{} commands NOT used in the last {} (deprecation candidates):",
                "Usage:".bold(),
                raw.cyan()
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
            "{} commands with errors in the last {}",
            "Usage:".bold(),
            since_raw.cyan()
        )
    } else {
        format!(
            "{} top commands in the last {}",
            "Usage:".bold(),
            since_raw.cyan()
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
                        "recent 7d: {} — the row above is the {} aggregate (a since-resolved batch)",
                        parts.join(", "),
                        since_raw
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
        "{} trace-read-rate audit over the last {}",
        "Usage:".bold(),
        since_raw.cyan()
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
// `aida usage --auto-complete` — orchestrator telemetry views (TASK-266).
// Reads `~/.aida/auto-complete.jsonl` (written one line per `--auto-complete`
// run by `record_auto_complete_run`).
// ----------------------------------------------------------------------------

/// TASK-266: the `aida usage --auto-complete` view family. Bare
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
                "{} (no --auto-complete runs recorded in the last {})",
                "Auto-complete:".bold(),
                since_raw.cyan()
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
        "{} {} runs in the last {} — {} ok, {} failed ({:.0}% success)",
        "Auto-complete:".bold(),
        summary.total,
        since_raw.cyan(),
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
        let bug_cell = match &ev.drafted_bug {
            Some(bug) => {
                let status = store.and_then(|s| bug_status(s, bug));
                match status {
                    Some(st) => format!("→ {} [{}]", bug.cyan(), st),
                    None => format!("→ {}", bug.cyan()),
                }
            }
            None => "(no BUG drafted)".dimmed().to_string(),
        };
        println!(
            "    {}  {:<12} phase {} ({})  {}",
            when.dimmed(),
            ev.spec_id.bold(),
            phase_n,
            phase_label,
            bug_cell,
        );
    }

    if failures.len() > cap {
        let more = failures.len() - cap;
        if overview {
            println!(
                "    {} {} more — `aida usage --auto-complete --failures`",
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
            "`aida usage --auto-complete --pattern` — which phases fail most often".dimmed()
        );
    }
    Ok(())
}

/// Render the per-phase failure histogram — the signal for where to invest
/// orchestrator fixes.
// trace:TASK-266 | ai:claude
fn render_auto_complete_pattern(
    events: &[auto_complete_telemetry::AutoCompleteEvent],
    json_out: bool,
) -> Result<()> {
    let hist = auto_complete_telemetry::failure_histogram(events);
    let summary = auto_complete_telemetry::summarize(events);

    if json_out {
        let arr: Vec<serde_json::Value> = hist
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
        println!("{}", serde_json::to_string_pretty(&arr)?);
        return Ok(());
    }

    println!(
        "{} phase-failure frequency — {} failed of {} runs",
        "Auto-complete:".bold(),
        summary.failed,
        summary.total,
    );
    if hist.is_empty() {
        println!("  {}", "no failures recorded — nothing to pattern".green());
        return Ok(());
    }
    let max = hist.iter().map(|(_, c)| *c).max().unwrap_or(1);
    for (phase, count) in &hist {
        let label = auto_complete::Phase::from_index(i32::from(*phase))
            .map(|p| p.slug())
            .unwrap_or("?");
        // Scale the bar to a 24-column field; never empty for a nonzero count.
        let width = ((*count * 24) / max).max(1);
        println!(
            "  phase {} {:<12} {} {}",
            phase,
            label,
            "█".repeat(width).red(),
            count,
        );
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
