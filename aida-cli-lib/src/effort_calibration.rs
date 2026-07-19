//! Four-touchpoint effort calibration — STORY-451.
//!
//! Effort is the quantitative sibling to STORY-439's complexity axis:
//! complexity answers "how hard/class of work", effort answers "how much
//! load/time". Records live at `.aida/effort-calibration/<SPEC>.yaml` and
//! accumulate four lifecycle slots: open, plan, ship, and review.
//!
//! Bucket conversions are intentionally work-time conversions:
//! `15m = 15 minutes`, `1h = 60 minutes`, `4h = 240 minutes`,
//! `1d = 8 work-hours`, and `1w = 5 work-days = 40 work-hours`.
//!
//! trace:STORY-451 | ai:codex

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub enum EffortBucket {
    #[clap(name = "15m", alias = "15min", alias = "15mins")]
    #[serde(rename = "15m")]
    FifteenMinutes,
    #[clap(name = "1h", alias = "hour", alias = "1hr")]
    #[serde(rename = "1h")]
    OneHour,
    #[clap(name = "4h", alias = "half-day", alias = "4hr")]
    #[serde(rename = "4h")]
    FourHours,
    #[clap(name = "1d", alias = "day")]
    #[serde(rename = "1d")]
    OneDay,
    #[clap(name = "1w", alias = "week")]
    #[serde(rename = "1w")]
    OneWeek,
}

impl EffortBucket {
    pub fn as_str(self) -> &'static str {
        match self {
            EffortBucket::FifteenMinutes => "15m",
            EffortBucket::OneHour => "1h",
            EffortBucket::FourHours => "4h",
            EffortBucket::OneDay => "1d",
            EffortBucket::OneWeek => "1w",
        }
    }

    pub fn minutes(self) -> u32 {
        match self {
            EffortBucket::FifteenMinutes => 15,
            EffortBucket::OneHour => 60,
            EffortBucket::FourHours => 240,
            EffortBucket::OneDay => 8 * 60,
            EffortBucket::OneWeek => 5 * 8 * 60,
        }
    }

    pub fn parse_str(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "15m" | "15min" | "15mins" | "15-min" | "15-minute" | "quarter-hour" => {
                Some(EffortBucket::FifteenMinutes)
            }
            "1h" | "h" | "hour" | "1hr" | "1-hour" => Some(EffortBucket::OneHour),
            "4h" | "half-day" | "halfday" | "4hr" | "4-hour" => Some(EffortBucket::FourHours),
            "1d" | "d" | "day" | "1day" | "1-day" => Some(EffortBucket::OneDay),
            "1w" | "w" | "week" | "1week" | "1-week" => Some(EffortBucket::OneWeek),
            _ => None,
        }
    }
}

impl std::fmt::Display for EffortBucket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EffortTouchpoint {
    Open,
    Plan,
    #[serde(rename = "impl")]
    Impl,
    Review,
}

impl EffortTouchpoint {
    pub fn as_str(self) -> &'static str {
        match self {
            EffortTouchpoint::Open => "open",
            EffortTouchpoint::Plan => "plan",
            EffortTouchpoint::Impl => "impl",
            EffortTouchpoint::Review => "review",
        }
    }
}

pub const EFFORT_TAG_PREFIX: &str = "effort:";

pub fn effort_tag_prefix(touchpoint: EffortTouchpoint) -> String {
    format!("{EFFORT_TAG_PREFIX}{}:", touchpoint.as_str())
}

pub fn apply_effort_tag(
    tags: &mut HashSet<String>,
    touchpoint: EffortTouchpoint,
    effort: EffortBucket,
) -> bool {
    let prefix = effort_tag_prefix(touchpoint);
    let target = format!("{prefix}{}", effort.as_str());
    let before = tags.clone();
    tags.retain(|t| !t.starts_with(&prefix));
    tags.insert(target);
    *tags != before
}

pub fn effort_from_tags(tags: &[String], touchpoint: EffortTouchpoint) -> Option<EffortBucket> {
    let prefix = effort_tag_prefix(touchpoint);
    tags.iter()
        .find_map(|t| t.strip_prefix(&prefix))
        .and_then(EffortBucket::parse_str)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EffortSlot {
    pub effort: EffortBucket,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimator: Option<String>,
    pub ts: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EffortCapture {
    pub spec: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<EffortSlot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<EffortSlot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ship: Option<EffortSlot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<EffortSlot>,
}

impl EffortCapture {
    fn new(spec: &str) -> Self {
        Self {
            spec: spec.to_string(),
            open: None,
            plan: None,
            ship: None,
            review: None,
        }
    }

    pub fn latest_ts(&self) -> Option<DateTime<Utc>> {
        [
            self.open.as_ref().map(|s| s.ts),
            self.plan.as_ref().map(|s| s.ts),
            self.ship.as_ref().map(|s| s.ts),
            self.review.as_ref().map(|s| s.ts),
        ]
        .into_iter()
        .flatten()
        .max()
    }

    pub fn latest_effort(&self) -> Option<(EffortTouchpoint, EffortBucket)> {
        self.review
            .as_ref()
            .map(|s| (EffortTouchpoint::Review, s.effort))
            .or_else(|| {
                self.ship
                    .as_ref()
                    .map(|s| (EffortTouchpoint::Impl, s.effort))
            })
            .or_else(|| {
                self.plan
                    .as_ref()
                    .map(|s| (EffortTouchpoint::Plan, s.effort))
            })
            .or_else(|| {
                self.open
                    .as_ref()
                    .map(|s| (EffortTouchpoint::Open, s.effort))
            })
    }
}

pub fn capture_dir(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("effort-calibration")
}

pub fn capture_path(project_root: &Path, spec: &str) -> PathBuf {
    capture_dir(project_root).join(format!("{spec}.yaml"))
}

pub fn read_capture(project_root: &Path, spec: &str) -> Option<EffortCapture> {
    let body = std::fs::read_to_string(capture_path(project_root, spec)).ok()?;
    serde_yaml::from_str(&body).ok()
}

pub fn write_capture(project_root: &Path, record: &EffortCapture) -> Result<()> {
    let dir = capture_dir(project_root);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = capture_path(project_root, &record.spec);
    let body = serde_yaml::to_string(record).context("serialising effort capture")?;
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

pub fn read_all_captures(project_root: &Path) -> Vec<EffortCapture> {
    let dir = capture_dir(project_root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut records = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(record) = serde_yaml::from_str::<EffortCapture>(&body) {
            records.push(record);
        }
    }
    records.sort_by_key(|r| std::cmp::Reverse(r.latest_ts()));
    records
}

fn upsert_slot(
    project_root: &Path,
    spec: &str,
    touchpoint: EffortTouchpoint,
    effort: Option<EffortBucket>,
    estimator: Option<String>,
) -> Result<()> {
    if !crate::usage::is_enabled(Some(project_root)) {
        return Ok(());
    }
    let Some(effort) = effort else {
        return Ok(());
    };
    let mut record = read_capture(project_root, spec).unwrap_or_else(|| EffortCapture::new(spec));
    let slot = Some(EffortSlot {
        effort,
        estimator,
        ts: Utc::now(),
    });
    match touchpoint {
        EffortTouchpoint::Open => record.open = slot,
        EffortTouchpoint::Plan => record.plan = slot,
        EffortTouchpoint::Impl => record.ship = slot,
        EffortTouchpoint::Review => record.review = slot,
    }
    write_capture(project_root, &record)
}

pub fn upsert_open(
    project_root: &Path,
    spec: &str,
    effort: Option<EffortBucket>,
    estimator: Option<String>,
) -> Result<()> {
    upsert_slot(
        project_root,
        spec,
        EffortTouchpoint::Open,
        effort,
        estimator,
    )
}

pub fn upsert_plan(
    project_root: &Path,
    spec: &str,
    effort: Option<EffortBucket>,
    estimator: Option<String>,
) -> Result<()> {
    upsert_slot(
        project_root,
        spec,
        EffortTouchpoint::Plan,
        effort,
        estimator,
    )
}

pub fn upsert_ship(
    project_root: &Path,
    spec: &str,
    effort: Option<EffortBucket>,
    estimator: Option<String>,
) -> Result<()> {
    upsert_slot(
        project_root,
        spec,
        EffortTouchpoint::Impl,
        effort,
        estimator,
    )
}

pub fn upsert_review(
    project_root: &Path,
    spec: &str,
    effort: Option<EffortBucket>,
    estimator: Option<String>,
) -> Result<()> {
    upsert_slot(
        project_root,
        spec,
        EffortTouchpoint::Review,
        effort,
        estimator,
    )
}

pub fn format_minutes(total: u32) -> String {
    if total == 0 {
        return "0m".to_string();
    }
    let week = EffortBucket::OneWeek.minutes();
    let day = EffortBucket::OneDay.minutes();
    let hour = EffortBucket::OneHour.minutes();
    let mut rest = total;
    let mut parts = Vec::new();
    let weeks = rest / week;
    if weeks > 0 {
        parts.push(format!("{weeks}w"));
        rest %= week;
    }
    let days = rest / day;
    if days > 0 {
        parts.push(format!("{days}d"));
        rest %= day;
    }
    let hours = rest / hour;
    if hours > 0 {
        parts.push(format!("{hours}h"));
        rest %= hour;
    }
    if rest > 0 {
        parts.push(format!("{rest}m"));
    }
    parts.join(" ")
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CalibrationDelta {
    pub spec: String,
    pub touchpoint: EffortTouchpoint,
    pub estimate: EffortBucket,
    pub actual: EffortBucket,
    pub delta_minutes: i32,
    pub ts: DateTime<Utc>,
}

pub fn calibration_deltas(
    records: &[EffortCapture],
    since: Option<ChronoDuration>,
) -> Vec<CalibrationDelta> {
    let cutoff = since.map(|d| Utc::now() - d);
    let mut rows = Vec::new();
    for record in records {
        let Some(ship) = record.ship.as_ref() else {
            continue;
        };
        if cutoff
            .map(|c| record.latest_ts().map(|t| t < c).unwrap_or(true))
            .unwrap_or(false)
        {
            continue;
        }
        for (touchpoint, slot) in [
            (EffortTouchpoint::Open, record.open.as_ref()),
            (EffortTouchpoint::Plan, record.plan.as_ref()),
            (EffortTouchpoint::Review, record.review.as_ref()),
        ] {
            if let Some(slot) = slot {
                rows.push(CalibrationDelta {
                    spec: record.spec.clone(),
                    touchpoint,
                    estimate: slot.effort,
                    actual: ship.effort,
                    delta_minutes: slot.effort.minutes() as i32 - ship.effort.minutes() as i32,
                    ts: slot.ts,
                });
            }
        }
    }
    rows.sort_by(|a, b| {
        b.delta_minutes
            .abs()
            .cmp(&a.delta_minutes.abs())
            .then_with(|| b.ts.cmp(&a.ts))
    });
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_bucket_parse_display_and_minutes() {
        assert_eq!(
            EffortBucket::parse_str("15min"),
            Some(EffortBucket::FifteenMinutes)
        );
        assert_eq!(EffortBucket::parse_str("hour"), Some(EffortBucket::OneHour));
        assert_eq!(EffortBucket::parse_str("day"), Some(EffortBucket::OneDay));
        assert_eq!(EffortBucket::OneDay.minutes(), 480);
        assert_eq!(EffortBucket::OneWeek.minutes(), 2400);
        assert_eq!(EffortBucket::FourHours.to_string(), "4h");
    }

    #[test]
    fn effort_tag_replaces_one_touchpoint_only() {
        let mut tags = HashSet::new();
        tags.insert("effort:open:1h".to_string());
        tags.insert("effort:plan:4h".to_string());
        tags.insert("metrics".to_string());
        assert!(apply_effort_tag(
            &mut tags,
            EffortTouchpoint::Open,
            EffortBucket::OneDay
        ));
        assert!(tags.contains("effort:open:1d"));
        assert!(tags.contains("effort:plan:4h"));
        assert!(tags.contains("metrics"));
        assert!(!tags.contains("effort:open:1h"));
    }

    #[test]
    fn upserts_preserve_other_slots_and_latest_precedence() {
        let dir = tempfile::tempdir().unwrap();
        upsert_open(dir.path(), "TASK-1", Some(EffortBucket::OneHour), None).unwrap();
        upsert_plan(dir.path(), "TASK-1", Some(EffortBucket::FourHours), None).unwrap();
        upsert_ship(dir.path(), "TASK-1", Some(EffortBucket::OneDay), None).unwrap();
        let record = read_capture(dir.path(), "TASK-1").unwrap();
        assert_eq!(record.open.as_ref().unwrap().effort, EffortBucket::OneHour);
        assert_eq!(
            record.plan.as_ref().unwrap().effort,
            EffortBucket::FourHours
        );
        assert_eq!(
            record.latest_effort(),
            Some((EffortTouchpoint::Impl, EffortBucket::OneDay))
        );
    }

    #[test]
    fn format_minutes_uses_work_day_and_week() {
        assert_eq!(format_minutes(15), "15m");
        assert_eq!(format_minutes(60), "1h");
        assert_eq!(format_minutes(480), "1d");
        assert_eq!(format_minutes(2400 + 480 + 60 + 15), "1w 1d 1h 15m");
    }

    #[test]
    fn calibration_deltas_compare_estimates_to_ship_actual() {
        let now = Utc::now();
        let records = vec![EffortCapture {
            spec: "TASK-1".to_string(),
            open: Some(EffortSlot {
                effort: EffortBucket::OneHour,
                estimator: None,
                ts: now,
            }),
            plan: None,
            ship: Some(EffortSlot {
                effort: EffortBucket::FourHours,
                estimator: None,
                ts: now,
            }),
            review: Some(EffortSlot {
                effort: EffortBucket::OneDay,
                estimator: None,
                ts: now,
            }),
        }];
        let rows = calibration_deltas(&records, None);
        assert_eq!(rows.len(), 2);
        assert!(rows
            .iter()
            .any(|r| r.touchpoint == EffortTouchpoint::Open && r.delta_minutes == -180));
        assert!(rows
            .iter()
            .any(|r| r.touchpoint == EffortTouchpoint::Review && r.delta_minutes == 240));
    }
}
