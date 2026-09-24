//! Mass-change mode: the switch, the clock and the window.
//!
//! A decision record adopted a temporary "mass-change" review mode (the
//! reviewer's verdict splits into blocking defects and deferrable acceptance
//! gaps). This module is only the mechanism that makes such a temporary mode
//! observable and self-expiring:
//!
//! - **Switch** — `[review] mass_change_mode = "on" | "off"` in
//!   `.aida/config.toml`, absent (= off) by default. Turned on/off only through
//!   `aida review mode mass-change on|off`; turning it ON stamps the clock.
//! - **Clock** — the same write records `mass_change_started_at` (RFC 3339 UTC)
//!   and `mass_change_started_by` (the shell user identity).
//! - **Window** — `mass_change_expires_at` is set by the on-verb (default 7
//!   days, `--days N`). Past it the mode reads as EXPIRED — i.e. inactive —
//!   without anyone acting; `aida status` says so instead of going silent.
//!
//! Fail-closed: `mass_change_mode = "on"` with no parseable start stamp is
//! treated as INACTIVE (a mode nobody clocked cannot be trusted to expire).
//!
//! **Consumers** ask exactly one question — [`is_active`] (or [`state`] for the
//! detail). Nothing else should parse the `[review] mass_change_*` keys.
//!
//! Deferred findings (the budget the mode must surface) are open specs tagged
//! [`DEFERRED_FINDING_TAG`]; the split-verdict writer tags the child specs it
//! files with it, and [`count_open_deferred_findings`] counts them straight off
//! the cache (no store load, no network).
// trace:STORY-1415 | ai:claude

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use std::path::{Path, PathBuf};

/// Config section the mode lives in.
pub(crate) const SECTION: &str = "review";
/// The switch key.
pub(crate) const KEY_MODE: &str = "mass_change_mode";
/// When the mode was turned on (RFC 3339, UTC).
pub(crate) const KEY_STARTED_AT: &str = "mass_change_started_at";
/// Who turned it on.
pub(crate) const KEY_STARTED_BY: &str = "mass_change_started_by";
/// When the window closes (RFC 3339, UTC).
pub(crate) const KEY_EXPIRES_AT: &str = "mass_change_expires_at";
/// Default window: one week.
pub(crate) const DEFAULT_WINDOW_DAYS: i64 = 7;
/// Upper bound on `--days`: a temporary mode stays temporary.
pub(crate) const MAX_WINDOW_DAYS: i64 = 30;
/// Tag carried by a finding the review deferred (rather than blocked on)
/// while the mode was on. Open specs with this tag are the deferred-findings
/// budget.
pub(crate) const DEFERRED_FINDING_TAG: &str = "review:deferred-finding";

/// The raw `[review] mass_change_*` keys as found in config.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct MassChangeRecord {
    pub on: bool,
    pub started_at: Option<DateTime<Utc>>,
    pub started_by: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// The effective state at a given instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MassChangeState {
    /// Absent, or explicitly off.
    Off,
    /// On and inside its window.
    Active {
        started_at: DateTime<Utc>,
        started_by: Option<String>,
        expires_at: DateTime<Utc>,
    },
    /// Switched on, but the window has closed. Behaves exactly like `Off` for
    /// every consumer; kept distinct so `aida status` can say it expired.
    Expired {
        started_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    },
    /// Switched on with no recorded start — fail closed (inactive).
    Unclocked,
}

impl MassChangeState {
    pub(crate) fn is_active(&self) -> bool {
        matches!(self, MassChangeState::Active { .. })
    }
}

/// Where the mode's config lives: the MAIN worktree's `.aida/config.toml`, so
/// every linked worktree and drain sees one switch rather than its branch's
/// copy.
pub(crate) fn config_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("config.toml")
}

/// The project root whose config carries the mode.
pub(crate) fn mode_root() -> Result<PathBuf> {
    crate::find_main_worktree_root().or_else(|_| crate::find_project_root())
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s.trim())
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn parse_on(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "on" | "true" | "1" | "yes"
    )
}

/// Parse the record from a config.toml body. Malformed TOML or absent keys
/// read as the default (off) — config errors never switch the mode on.
pub(crate) fn record_from_toml_str(body: &str) -> MassChangeRecord {
    let Ok(doc) = body.parse::<toml_edit::DocumentMut>() else {
        return MassChangeRecord::default();
    };
    let Some(tbl) = doc.get(SECTION).and_then(|i| i.as_table_like()) else {
        return MassChangeRecord::default();
    };
    let s = |k: &str| tbl.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let on = match tbl.get(KEY_MODE) {
        Some(v) => match (v.as_str(), v.as_bool()) {
            (Some(s), _) => parse_on(s),
            (None, Some(b)) => b,
            _ => false,
        },
        None => false,
    };
    MassChangeRecord {
        on,
        started_at: s(KEY_STARTED_AT).as_deref().and_then(parse_ts),
        started_by: s(KEY_STARTED_BY).filter(|v| !v.trim().is_empty()),
        expires_at: s(KEY_EXPIRES_AT).as_deref().and_then(parse_ts),
    }
}

/// Read the record from `<project_root>/.aida/config.toml`.
pub(crate) fn read_record(project_root: &Path) -> MassChangeRecord {
    std::fs::read_to_string(config_path(project_root))
        .map(|b| record_from_toml_str(&b))
        .unwrap_or_default()
}

/// Project a record onto an instant. A missing expiry falls back to the
/// default one-week window from the recorded start.
pub(crate) fn state_at(rec: &MassChangeRecord, now: DateTime<Utc>) -> MassChangeState {
    if !rec.on {
        return MassChangeState::Off;
    }
    let Some(started_at) = rec.started_at else {
        return MassChangeState::Unclocked;
    };
    let expires_at = rec
        .expires_at
        .unwrap_or(started_at + Duration::days(DEFAULT_WINDOW_DAYS));
    if now >= expires_at {
        MassChangeState::Expired {
            started_at,
            expires_at,
        }
    } else {
        MassChangeState::Active {
            started_at,
            started_by: rec.started_by.clone(),
            expires_at,
        }
    }
}

/// The effective state for a project right now.
pub(crate) fn state(project_root: &Path) -> MassChangeState {
    state_at(&read_record(project_root), Utc::now())
}

/// THE consumer query: is mass-change mode in force right now? False when
/// off, expired, or on-without-a-clock.
#[allow(dead_code)] // the split-verdict consumer lands separately
pub(crate) fn is_active(project_root: &Path) -> bool {
    state(project_root).is_active()
}

/// Count open specs tagged [`DEFERRED_FINDING_TAG`] from the sqlite cache.
/// Read-only, no store load; a missing/unreadable cache counts zero.
pub(crate) fn count_open_deferred_findings(cache_path: &Path) -> usize {
    if !cache_path.exists() {
        return 0;
    }
    let Ok(conn) = rusqlite::Connection::open_with_flags(
        cache_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return 0;
    };
    let Ok(mut stmt) =
        conn.prepare("SELECT status, tags_json FROM requirements_cache WHERE archived = 0")
    else {
        return 0;
    };
    let Ok(rows) = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }) else {
        return 0;
    };
    rows.flatten()
        .filter(|(status, tags_json)| {
            is_open_status(status)
                && serde_json::from_str::<Vec<String>>(tags_json)
                    .map(|tags| tags.iter().any(|t| t == DEFERRED_FINDING_TAG))
                    .unwrap_or(false)
        })
        .count()
}

fn is_open_status(status: &str) -> bool {
    let norm: String = status
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    !matches!(
        norm.as_str(),
        "done" | "completed" | "released" | "rejected" | "superseded"
    )
}

/// "day N of M" — day 1 is the first 24h. Totals round up so a 36h window
/// reads "of 2".
fn day_of(started_at: DateTime<Utc>, expires_at: DateTime<Utc>, now: DateTime<Utc>) -> (i64, i64) {
    let secs_per_day = 86_400;
    let total_secs = (expires_at - started_at).num_seconds().max(1);
    let total = (total_secs + secs_per_day - 1) / secs_per_day;
    let elapsed = (now - started_at).num_seconds().max(0);
    let day = (elapsed / secs_per_day + 1).min(total.max(1));
    (day, total.max(1))
}

fn fmt_ts(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%d %H:%M UTC").to_string()
}

fn findings_phrase(n: usize) -> String {
    if n == 1 {
        "1 deferred finding open".to_string()
    } else {
        format!("{n} deferred findings open")
    }
}

/// The single `aida status` line. `None` when the mode is off — a mode that
/// prints nothing when off costs nothing when off.
pub(crate) fn status_line(
    st: &MassChangeState,
    now: DateTime<Utc>,
    deferred_open: usize,
) -> Option<String> {
    match st {
        MassChangeState::Off => None,
        MassChangeState::Active {
            started_at,
            expires_at,
            ..
        } => {
            let (day, total) = day_of(*started_at, *expires_at, now);
            Some(format!(
                "mass-change mode: ON, day {day} of {total} (expires {}), {}",
                fmt_ts(*expires_at),
                findings_phrase(deferred_open)
            ))
        }
        MassChangeState::Expired { expires_at, .. } => Some(format!(
            "mass-change mode: EXPIRED {} — review verdicts block again, {}; clear with `aida review mode mass-change off`",
            fmt_ts(*expires_at),
            findings_phrase(deferred_open)
        )),
        MassChangeState::Unclocked => Some(
            "mass-change mode: set on but never clocked — INACTIVE; turn it on with `aida review mode mass-change on`"
                .to_string(),
        ),
    }
}

/// Turn the mode on: one write sets the switch, the start stamp, the actor and
/// the expiry. Refuses while already active (the on-verb must not silently
/// extend a running window — turn it off first to restart the clock).
pub(crate) fn turn_on(
    project_root: &Path,
    actor: &str,
    days: i64,
    now: DateTime<Utc>,
) -> Result<MassChangeState> {
    if !(1..=MAX_WINDOW_DAYS).contains(&days) {
        anyhow::bail!("--days must be between 1 and {MAX_WINDOW_DAYS} (got {days})");
    }
    if let MassChangeState::Active { expires_at, .. } = state_at(&read_record(project_root), now) {
        anyhow::bail!(
            "mass-change mode is already on (expires {}); turn it off first to restart the clock",
            fmt_ts(expires_at)
        );
    }
    let expires_at = now + Duration::days(days);
    let path = config_path(project_root);
    crate::config_edit::set_kvs(
        &path,
        SECTION,
        &[
            (KEY_MODE, toml_edit::Value::from("on")),
            (
                KEY_STARTED_AT,
                toml_edit::Value::from(now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            ),
            (KEY_STARTED_BY, toml_edit::Value::from(actor)),
            (
                KEY_EXPIRES_AT,
                toml_edit::Value::from(
                    expires_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                ),
            ),
        ],
    )
    .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(state_at(&read_record(project_root), now))
}

/// Turn the mode off. Returns the state it was in before. Safe at any time:
/// it only stops future deferrals; nothing already merged is re-blocked.
pub(crate) fn turn_off(project_root: &Path, now: DateTime<Utc>) -> Result<MassChangeState> {
    let prev = state_at(&read_record(project_root), now);
    let path = config_path(project_root);
    crate::config_edit::set_kv(&path, SECTION, KEY_MODE, toml_edit::Value::from("off"))?;
    crate::config_edit::remove_keys(
        &path,
        SECTION,
        &[KEY_STARTED_AT, KEY_STARTED_BY, KEY_EXPIRES_AT],
    )?;
    Ok(prev)
}

fn state_json(st: &MassChangeState, now: DateTime<Utc>, deferred_open: usize) -> serde_json::Value {
    let mut v = match st {
        MassChangeState::Off => serde_json::json!({ "state": "off", "active": false }),
        MassChangeState::Unclocked => {
            serde_json::json!({ "state": "unclocked", "active": false })
        }
        MassChangeState::Active {
            started_at,
            started_by,
            expires_at,
        } => {
            let (day, total) = day_of(*started_at, *expires_at, now);
            serde_json::json!({
                "state": "on",
                "active": true,
                "started_at": started_at.to_rfc3339(),
                "started_by": started_by,
                "expires_at": expires_at.to_rfc3339(),
                "day": day,
                "window_days": total,
            })
        }
        MassChangeState::Expired {
            started_at,
            expires_at,
        } => serde_json::json!({
            "state": "expired",
            "active": false,
            "started_at": started_at.to_rfc3339(),
            "expires_at": expires_at.to_rfc3339(),
        }),
    };
    v["deferred_findings_open"] = serde_json::json!(deferred_open);
    v
}

/// `aida review mode mass-change on|off|status`.
pub(crate) fn handle_mass_change_command(
    action: &str,
    days: Option<i64>,
    json: bool,
) -> Result<()> {
    let root = mode_root()?;
    let now = Utc::now();
    let deferred = || count_open_deferred_findings(&root.join(".aida").join("cache.db"));
    match action {
        "on" => {
            let actor = crate::current_user_id(None);
            let st = turn_on(&root, &actor, days.unwrap_or(DEFAULT_WINDOW_DAYS), now)?;
            let n = deferred();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&state_json(&st, now, n))?
                );
            } else if let MassChangeState::Active { expires_at, .. } = st {
                println!(
                    "mass-change mode: ON — started {} by {actor}, expires {}.",
                    fmt_ts(now),
                    fmt_ts(expires_at)
                );
                println!(
                    "Acceptance gaps may be deferred instead of blocking until then; it turns itself off at expiry. {}.",
                    findings_phrase(n)
                );
            }
        }
        "off" => {
            if days.is_some() {
                anyhow::bail!("--days applies only to `on`");
            }
            let prev = turn_off(&root, now)?;
            let n = deferred();
            if json {
                let mut v = state_json(&MassChangeState::Off, now, n);
                v["was"] = state_json(&prev, now, n)["state"].clone();
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                let was = match prev {
                    MassChangeState::Active { started_at, .. } => {
                        let hours = (now - started_at).num_hours();
                        format!(" (was on for {}d {}h)", hours / 24, hours % 24)
                    }
                    MassChangeState::Expired { .. } => " (it had already expired)".to_string(),
                    _ => " (it was not on)".to_string(),
                };
                println!("mass-change mode: OFF{was}.");
                println!(
                    "Acceptance gaps block again; work already merged is not re-blocked. Left behind: {} — `aida list --tags {DEFERRED_FINDING_TAG}`.",
                    findings_phrase(n)
                );
            }
        }
        "status" => {
            if days.is_some() {
                anyhow::bail!("--days applies only to `on`");
            }
            let st = state_at(&read_record(&root), now);
            let n = deferred();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&state_json(&st, now, n))?
                );
            } else {
                match status_line(&st, now, n) {
                    Some(line) => println!("{line}"),
                    None => println!("mass-change mode: OFF, {}", findings_phrase(n)),
                }
            }
        }
        other => anyhow::bail!("unknown action `{other}` (expected on, off or status)"),
    }
    Ok(())
}

/// The fast `aida status` hook: print the one line when the mode is not off.
/// Config + cache reads only.
pub(crate) fn print_status_line(project_root: &Path) {
    let root = crate::find_main_worktree_root().unwrap_or_else(|_| project_root.to_path_buf());
    let now = Utc::now();
    let st = state_at(&read_record(&root), now);
    if matches!(st, MassChangeState::Off) {
        return;
    }
    let n = count_open_deferred_findings(&root.join(".aida").join("cache.db"));
    if let Some(line) = status_line(&st, now, n) {
        use colored::Colorize;
        println!("{}", line.yellow().bold());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap()
    }

    #[test]
    fn absent_config_is_off_and_prints_nothing() {
        let rec = record_from_toml_str("[review]\nmode = \"local\"\n");
        let st = state_at(&rec, t0());
        assert_eq!(st, MassChangeState::Off);
        assert!(!st.is_active());
        assert_eq!(status_line(&st, t0(), 12), None);
    }

    #[test]
    fn on_stamps_the_clock_and_status_shows_day_of_window() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
        std::fs::write(
            config_path(dir.path()),
            "# keep me\n[review]\nmode = \"local\"\n",
        )
        .unwrap();
        let st = turn_on(dir.path(), "joe", 7, t0()).unwrap();
        assert!(st.is_active());
        let body = std::fs::read_to_string(config_path(dir.path())).unwrap();
        assert!(body.contains("# keep me") && body.contains("mode = \"local\""));
        assert!(body.contains("mass_change_mode = \"on\""));
        let rec = read_record(dir.path());
        assert_eq!(rec.started_at, Some(t0()));
        assert_eq!(rec.started_by.as_deref(), Some("joe"));

        let later = t0() + Duration::days(2) + Duration::hours(1);
        let line = status_line(&state_at(&rec, later), later, 12).unwrap();
        assert!(line.contains("ON, day 3 of 7"), "{line}");
        assert!(line.contains("12 deferred findings open"), "{line}");
    }

    #[test]
    fn on_refuses_to_extend_a_running_window() {
        let dir = tempfile::tempdir().unwrap();
        turn_on(dir.path(), "joe", 7, t0()).unwrap();
        let err = turn_on(dir.path(), "joe", 7, t0() + Duration::days(3)).unwrap_err();
        assert!(err.to_string().contains("already on"));
        assert!(turn_on(dir.path(), "joe", 0, t0()).is_err());
        assert!(turn_on(dir.path(), "joe", MAX_WINDOW_DAYS + 1, t0()).is_err());
    }

    #[test]
    fn expires_by_itself_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        turn_on(dir.path(), "joe", 7, t0()).unwrap();
        let after = t0() + Duration::days(7) + Duration::minutes(1);
        let st = state_at(&read_record(dir.path()), after);
        assert!(matches!(st, MassChangeState::Expired { .. }));
        assert!(!st.is_active());
        let line = status_line(&st, after, 3).unwrap();
        assert!(line.contains("EXPIRED"), "{line}");
        // An expired window can be restarted.
        assert!(turn_on(dir.path(), "joe", 7, after).unwrap().is_active());
    }

    #[test]
    fn off_clears_the_clock_and_reports_prior_state() {
        let dir = tempfile::tempdir().unwrap();
        turn_on(dir.path(), "joe", 7, t0()).unwrap();
        let prev = turn_off(dir.path(), t0() + Duration::days(1)).unwrap();
        assert!(prev.is_active());
        let rec = read_record(dir.path());
        assert_eq!(rec, MassChangeRecord::default());
        let body = std::fs::read_to_string(config_path(dir.path())).unwrap();
        assert!(body.contains("mass_change_mode = \"off\""));
        assert!(!body.contains(KEY_STARTED_AT));
    }

    #[test]
    fn on_without_a_start_fails_closed() {
        let rec = record_from_toml_str("[review]\nmass_change_mode = \"on\"\n");
        let st = state_at(&rec, t0());
        assert_eq!(st, MassChangeState::Unclocked);
        assert!(!st.is_active());
        assert!(status_line(&st, t0(), 0).unwrap().contains("INACTIVE"));
    }

    #[test]
    fn missing_expiry_defaults_to_one_week() {
        let body = format!(
            "[review]\nmass_change_mode = \"on\"\nmass_change_started_at = \"{}\"\n",
            t0().to_rfc3339()
        );
        let rec = record_from_toml_str(&body);
        assert!(state_at(&rec, t0() + Duration::days(6)).is_active());
        assert!(!state_at(&rec, t0() + Duration::days(7)).is_active());
    }

    #[test]
    fn counts_only_open_deferred_findings_from_cache() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("cache.db");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE requirements_cache (status TEXT, tags_json TEXT, archived INTEGER);
             INSERT INTO requirements_cache VALUES ('Draft', '[\"review:deferred-finding\"]', 0);
             INSERT INTO requirements_cache VALUES ('In Progress', '[\"x\",\"review:deferred-finding\"]', 0);
             INSERT INTO requirements_cache VALUES ('Completed', '[\"review:deferred-finding\"]', 0);
             INSERT INTO requirements_cache VALUES ('Draft', '[\"review:deferred-finding\"]', 1);
             INSERT INTO requirements_cache VALUES ('Draft', '[\"other\"]', 0);",
        )
        .unwrap();
        drop(conn);
        assert_eq!(count_open_deferred_findings(&db), 2);
        assert_eq!(count_open_deferred_findings(&dir.path().join("nope.db")), 0);
    }
}
