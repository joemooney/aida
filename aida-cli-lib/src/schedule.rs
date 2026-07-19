//! No-daemon scheduled advisor tasks (STORY-262).
//!
//! The advisor role's proactive work — competitive-analysis refreshes,
//! spec-graph audits, memory curation, telemetry reviews — benefits from a
//! *cadence*, but AIDA deliberately has no background daemon. This module
//! provides the scheduling primitive: a recurring **task template** with a
//! cadence is registered locally, and on every `aida pull` AIDA evaluates
//! which schedules are *due* (cadence elapsed since the last fire) and files
//! a fresh TASK into the target role's queue.
//!
//! Storage is local, per-clone runtime state at `.aida/schedules.toml`
//! (gitignored under the `.aida/*` deny-by-default rule, like
//! `.aida/stacks.json`). It is intentionally NOT synced to the orphan store —
//! schedules are an operator's personal cadence, not a project artifact.
//!
//! The due-calculation (`Schedule::is_due` / `Schedule::next_due`) is a pure
//! function of cadence + last_fired + now, with no I/O, so it is fully
//! unit-testable in isolation (see the tests at the bottom of this file).
//!
//! Out of scope (per the spec): a cron daemon, external-event triggers, team
//! sync, and smart anti-clustering. Pure time-cadence evaluated on `pull`.
//!
//! trace:STORY-262 | ai:claude

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One registered recurring task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    /// Unique short name (the registration key). Becomes the
    /// `scheduled:<name>` tag on every filed TASK so fires are traceable
    /// and batch-drainable.
    pub name: String,
    /// Cadence in the Go-duration-like syntax AIDA already uses elsewhere
    /// (`90d`, `14d`, `30d`, `12h`, `1w`). Stored verbatim; parsed via
    /// [`parse_cadence`] at evaluation time.
    pub cadence: String,
    /// TASK title filed when the schedule fires.
    pub title: String,
    /// TASK description (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Extra tags applied to the filed TASK (in addition to `scheduled:<name>`
    /// and a `batch:scheduled` tag).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Role the filed TASK is routed to. Defaults to `advisor` — these are
    /// the advisor role's proactive responsibilities.
    #[serde(default = "default_role")]
    pub for_role: String,
    /// When this schedule last fired a TASK. `None` = never fired; the
    /// schedule is due immediately on the next evaluation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fired: Option<DateTime<Utc>>,
    /// Whether the schedule participates in auto-evaluation. A disabled
    /// schedule is never fired by `aida pull` but is preserved.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_role() -> String {
    "advisor".to_string()
}

fn default_enabled() -> bool {
    true
}

impl Schedule {
    /// Whether this schedule is due to fire at `now`. A disabled schedule is
    /// never due. A never-fired (`last_fired == None`) enabled schedule is
    /// always due. Otherwise: due when `now - last_fired >= cadence`.
    ///
    /// An unparseable cadence is treated as *not due* (with the error
    /// surfaced separately by the caller) so a single typo'd schedule can't
    /// wedge the whole evaluation.
    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        if !self.enabled {
            return false;
        }
        let cadence = match parse_cadence(&self.cadence) {
            Ok(c) => c,
            Err(_) => return false,
        };
        match self.last_fired {
            None => true,
            Some(last) => now.signed_duration_since(last) >= cadence,
        }
    }

    /// The next time this schedule will be due, given its `last_fired`.
    /// `None` when the cadence is unparseable or the schedule never fired
    /// (it's already due now). When fired, this is `last_fired + cadence`.
    pub fn next_due(&self) -> Option<DateTime<Utc>> {
        let cadence = parse_cadence(&self.cadence).ok()?;
        self.last_fired.map(|last| last + cadence)
    }
}

/// On-disk shape of `.aida/schedules.toml`. A flat list plus the file format
/// version so future migrations are cheap.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScheduleFile {
    #[serde(default)]
    pub schedules: Vec<Schedule>,
}

/// Path on disk: `<project-root>/.aida/schedules.toml`.
pub fn schedules_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("schedules.toml")
}

/// Load the schedule file. Missing / unreadable / malformed → empty list, so
/// a bad file can never block `aida pull` (best-effort, matching `stacks::load`).
pub fn load(project_root: &Path) -> ScheduleFile {
    let path = schedules_path(project_root);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return ScheduleFile::default();
    };
    toml::from_str(&text).unwrap_or_default()
}

/// Persist the schedule file, creating `.aida/` if needed.
pub fn save(project_root: &Path, file: &ScheduleFile) -> Result<()> {
    let path = schedules_path(project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let text = toml::to_string_pretty(file).context("failed to serialize schedules")?;
    aida_core::write_atomic(&path, text.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// Parse a Go-duration-like cadence string into a `chrono::Duration`.
///
/// Supports a single `<number><unit>` token where unit is one of:
///   - `m` minutes
///   - `h` hours
///   - `d` days
///   - `w` weeks
///
/// Examples: `90d`, `14d`, `30d`, `12h`, `1w`. Whitespace is trimmed. A bare
/// number, an empty string, a non-positive value, or an unknown unit is an
/// error.
pub fn parse_cadence(s: &str) -> Result<Duration> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("empty cadence");
    }
    // Split into the leading numeric run and the trailing unit.
    let split = s
        .find(|c: char| !c.is_ascii_digit())
        .ok_or_else(|| anyhow::anyhow!("cadence '{s}' has no unit (expected e.g. 90d, 12h, 1w)"))?;
    if split == 0 {
        anyhow::bail!("cadence '{s}' has no leading number (expected e.g. 90d)");
    }
    let (num_part, unit) = s.split_at(split);
    let n: i64 = num_part
        .parse()
        .with_context(|| format!("cadence '{s}': '{num_part}' is not a number"))?;
    if n <= 0 {
        anyhow::bail!("cadence '{s}': value must be positive");
    }
    let dur = match unit {
        "m" => Duration::minutes(n),
        "h" => Duration::hours(n),
        "d" => Duration::days(n),
        "w" => Duration::weeks(n),
        other => anyhow::bail!("cadence '{s}': unknown unit '{other}' (use m, h, d, or w)"),
    };
    Ok(dur)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, mo: u32, d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, 0, 0, 0).unwrap()
    }

    fn sched(cadence: &str, last_fired: Option<DateTime<Utc>>, enabled: bool) -> Schedule {
        Schedule {
            name: "test".to_string(),
            cadence: cadence.to_string(),
            title: "Test task".to_string(),
            description: None,
            tags: vec![],
            for_role: "advisor".to_string(),
            last_fired,
            enabled,
        }
    }

    // ---- parse_cadence ----

    #[test]
    fn parse_cadence_units() {
        assert_eq!(parse_cadence("90d").unwrap(), Duration::days(90));
        assert_eq!(parse_cadence("14d").unwrap(), Duration::days(14));
        assert_eq!(parse_cadence("12h").unwrap(), Duration::hours(12));
        assert_eq!(parse_cadence("30m").unwrap(), Duration::minutes(30));
        assert_eq!(parse_cadence("1w").unwrap(), Duration::weeks(1));
    }

    #[test]
    fn parse_cadence_trims_whitespace() {
        assert_eq!(parse_cadence("  7d  ").unwrap(), Duration::days(7));
    }

    #[test]
    fn parse_cadence_rejects_bad_input() {
        assert!(parse_cadence("").is_err(), "empty");
        assert!(parse_cadence("90").is_err(), "no unit");
        assert!(parse_cadence("d").is_err(), "no number");
        assert!(parse_cadence("90y").is_err(), "unknown unit");
        assert!(parse_cadence("0d").is_err(), "zero");
        assert!(
            parse_cadence("-5d").is_err(),
            "negative (leading '-' is non-digit)"
        );
        assert!(parse_cadence("xyz").is_err(), "garbage");
    }

    // ---- is_due: the core cadence + last-run → due? calculation ----

    #[test]
    fn never_fired_enabled_is_due() {
        let s = sched("14d", None, true);
        assert!(
            s.is_due(at(2026, 6, 6)),
            "never-fired enabled schedule is due now"
        );
    }

    #[test]
    fn never_fired_disabled_is_not_due() {
        let s = sched("14d", None, false);
        assert!(!s.is_due(at(2026, 6, 6)), "disabled schedules never fire");
    }

    #[test]
    fn fired_within_cadence_is_not_due() {
        // Fired 10 days ago, cadence 14d → not yet due.
        let s = sched("14d", Some(at(2026, 6, 1)), true);
        assert!(
            !s.is_due(at(2026, 6, 11)),
            "10 days < 14d cadence → not due"
        );
    }

    #[test]
    fn fired_exactly_at_cadence_is_due() {
        // Fired exactly 14 days ago → due (>= boundary is inclusive).
        let s = sched("14d", Some(at(2026, 6, 1)), true);
        assert!(
            s.is_due(at(2026, 6, 15)),
            "exactly 14d elapsed → due (inclusive boundary)"
        );
    }

    #[test]
    fn fired_past_cadence_is_due() {
        // Fired 20 days ago, cadence 14d → overdue, due.
        let s = sched("14d", Some(at(2026, 6, 1)), true);
        assert!(s.is_due(at(2026, 6, 21)), "20 days > 14d cadence → due");
    }

    #[test]
    fn fired_past_cadence_but_disabled_is_not_due() {
        let s = sched("14d", Some(at(2026, 6, 1)), false);
        assert!(
            !s.is_due(at(2026, 6, 30)),
            "disabled wins even when overdue"
        );
    }

    #[test]
    fn unparseable_cadence_is_not_due() {
        // A typo'd cadence must not wedge evaluation — treated as not-due.
        let s = sched("90y", None, true);
        assert!(
            !s.is_due(at(2026, 6, 6)),
            "bad cadence → not due, doesn't panic"
        );
    }

    #[test]
    fn hour_cadence_due_calculation() {
        let base = Utc.with_ymd_and_hms(2026, 6, 6, 0, 0, 0).unwrap();
        let s = sched("12h", Some(base), true);
        let eleven_h = base + Duration::hours(11);
        let twelve_h = base + Duration::hours(12);
        assert!(!s.is_due(eleven_h), "11h < 12h → not due");
        assert!(s.is_due(twelve_h), "12h == 12h → due");
    }

    // ---- next_due ----

    #[test]
    fn next_due_is_last_plus_cadence() {
        let s = sched("14d", Some(at(2026, 6, 1)), true);
        assert_eq!(s.next_due(), Some(at(2026, 6, 15)));
    }

    #[test]
    fn next_due_none_when_never_fired() {
        let s = sched("14d", None, true);
        assert_eq!(s.next_due(), None, "never fired → due now, no next_due");
    }

    #[test]
    fn next_due_none_when_cadence_bad() {
        let s = sched("nope", Some(at(2026, 6, 1)), true);
        assert_eq!(s.next_due(), None);
    }

    // ---- file round-trip ----

    #[test]
    fn schedule_file_round_trips_through_toml() {
        let file = ScheduleFile {
            schedules: vec![
                sched("90d", Some(at(2026, 5, 16)), true),
                sched("14d", None, true),
            ],
        };
        let text = toml::to_string_pretty(&file).unwrap();
        let back: ScheduleFile = toml::from_str(&text).unwrap();
        assert_eq!(back.schedules.len(), 2);
        assert_eq!(back.schedules[0].cadence, "90d");
        assert_eq!(back.schedules[0].last_fired, Some(at(2026, 5, 16)));
        assert_eq!(back.schedules[1].last_fired, None);
        assert!(back.schedules[1].enabled);
    }

    #[test]
    fn defaults_fill_in_on_load() {
        // A minimal entry (just name/cadence/title) should default role +
        // enabled.
        let text = r#"
[[schedules]]
name = "audit-spec-graph"
cadence = "14d"
title = "Audit the spec graph"
"#;
        let file: ScheduleFile = toml::from_str(text).unwrap();
        assert_eq!(file.schedules.len(), 1);
        assert_eq!(file.schedules[0].for_role, "advisor");
        assert!(file.schedules[0].enabled);
        assert_eq!(file.schedules[0].last_fired, None);
    }
}
