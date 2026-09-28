//! Intent-capture effort target (TASK-1522).
//!
//! Report the criterion-to-test share of newly completed specs against a
//! configurable floor (default 50%).
//! Advisory and report-only: does not block merge, drain, or completion.
//!
//! trace:TASK-1522 | ai:antigravity

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::criteria_coverage::{load_fresh_coverage_cache, SpecCoverage};
use aida_core::models::RequirementType;

/// Default lookback window for completed specs.
pub(crate) const DEFAULT_WINDOW_DAYS: u64 = 30;

/// Default floor percentage for intent capture (50%).
pub(crate) const DEFAULT_FLOOR_PCT: u8 = 50;

/// Verdict on whether intent capture meets the configured floor.
// trace:TASK-1522 | ai:antigravity
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum IntentCaptureVerdict {
    #[serde(rename = "below floor", alias = "below-floor", alias = "below_floor")]
    BelowFloor,
    #[serde(
        rename = "at or above floor",
        alias = "at-or-above-floor",
        alias = "at_or_above_floor"
    )]
    AtOrAboveFloor,
}

impl std::fmt::Display for IntentCaptureVerdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BelowFloor => write!(f, "below floor"),
            Self::AtOrAboveFloor => write!(f, "at or above floor"),
        }
    }
}

/// The intent capture target calculation.
// trace:TASK-1522 | ai:antigravity
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct IntentCapture {
    pub(crate) window_days: u64,
    pub(crate) specs_with_criteria: usize,
    pub(crate) specs_with_traced_criterion: usize,
    pub(crate) share_pct: Option<f64>,
    pub(crate) floor_pct: u8,
    pub(crate) verdict: IntentCaptureVerdict,
}

impl IntentCapture {
    /// Format for the `aida criteria coverage` report output.
    pub(crate) fn render_report_line(&self) -> String {
        let pct_str = match self.share_pct {
            Some(p) => {
                if (p - p.round()).abs() < 0.05 {
                    format!("{:.0}%", p.round())
                } else {
                    format!("{:.1}%", p)
                }
            }
            None => "(nothing to measure)".to_string(),
        };
        let ratio_str = if self.share_pct.is_some() {
            format!(
                "{} of {} ({})",
                self.specs_with_traced_criterion, self.specs_with_criteria, pct_str
            )
        } else {
            format!(
                "{} of {} (nothing to measure)",
                self.specs_with_traced_criterion, self.specs_with_criteria
            )
        };
        format!(
            "Intent capture (specs completed, last {} d): {} vs floor {}%: {}",
            self.window_days, ratio_str, self.floor_pct, self.verdict
        )
    }

    /// One-line indicator for `aida status` when below floor.
    pub(crate) fn render_status_line(&self) -> Option<String> {
        if self.verdict != IntentCaptureVerdict::BelowFloor {
            return None;
        }
        let pct_str = match self.share_pct {
            Some(p) => {
                if (p - p.round()).abs() < 0.05 {
                    format!("{:.0}%", p.round())
                } else {
                    format!("{:.1}%", p)
                }
            }
            None => "0%".to_string(),
        };
        Some(format!(
            "  Intent capture: {} of {} completed specs ({}) traced to tests; below the {}% floor. Run aida criteria coverage",
            self.specs_with_traced_criterion, self.specs_with_criteria, pct_str, self.floor_pct
        ))
    }
}

/// Read the floor percentage from `[capture] criterion_test_floor_pct` in `.aida/config.toml`.
/// Defaults to 50 if unset. If invalid (non-integer or out of 0..=100), emits a warning to stderr
/// and falls back to 50.
// trace:TASK-1522 | ai:antigravity
pub(crate) fn intent_capture_floor_pct(project_root: &Path) -> u8 {
    let cfg = crate::read_project_config_value(project_root);
    let val = crate::config_lookup(cfg.as_ref(), "capture", "criterion_test_floor_pct");
    let Some(val) = val else {
        return DEFAULT_FLOOR_PCT;
    };
    match val.as_integer() {
        Some(n) if (0..=100).contains(&n) => n as u8,
        Some(n) => {
            eprintln!(
                "warning: [capture] criterion_test_floor_pct must be between 0 and 100, got {n}; using default {DEFAULT_FLOOR_PCT}%"
            );
            DEFAULT_FLOOR_PCT
        }
        None => {
            eprintln!(
                "warning: [capture] criterion_test_floor_pct must be an integer between 0 and 100, got {val}; using default {DEFAULT_FLOOR_PCT}%"
            );
            DEFAULT_FLOOR_PCT
        }
    }
}

/// Calculate the intent-capture share for specs completed in the last 30 days.
// trace:TASK-1522 | ai:antigravity
pub(crate) fn intent_capture_share(
    specs: &[SpecCoverage],
    now: DateTime<Utc>,
    floor_pct: u8,
) -> IntentCapture {
    let window_days = DEFAULT_WINDOW_DAYS;
    let since = now - chrono::Duration::days(window_days as i64);

    let mut specs_with_criteria = 0usize;
    let mut specs_with_traced_criterion = 0usize;

    for s in specs {
        if !matches!(
            s.spec_type,
            RequirementType::Story | RequirementType::Task | RequirementType::Bug
        ) {
            continue;
        }
        if s.criteria == 0 {
            continue;
        }
        let Some(completed_at) = s.completed_at else {
            continue;
        };
        if completed_at < since || completed_at > now {
            continue;
        }

        specs_with_criteria += 1;
        if s.traced_criteria >= 1 {
            specs_with_traced_criterion += 1;
        }
    }

    let share_pct = if specs_with_criteria > 0 {
        Some(100.0 * specs_with_traced_criterion as f64 / specs_with_criteria as f64)
    } else {
        None
    };

    let verdict = if specs_with_criteria == 0 {
        IntentCaptureVerdict::AtOrAboveFloor
    } else {
        let is_at_or_above = match share_pct {
            Some(_) => {
                (specs_with_traced_criterion * 100) >= (specs_with_criteria * floor_pct as usize)
            }
            None => true,
        };
        if is_at_or_above {
            IntentCaptureVerdict::AtOrAboveFloor
        } else {
            IntentCaptureVerdict::BelowFloor
        }
    };

    IntentCapture {
        window_days,
        specs_with_criteria,
        specs_with_traced_criterion,
        share_pct,
        floor_pct,
        verdict,
    }
}

/// Computes the status line if the capture share is below the floor, or `None` if at or above floor,
/// or if the cache is absent/stale.
// trace:TASK-1522 | ai:antigravity
pub(crate) fn status_intent_capture_line(project_root: &Path) -> Option<String> {
    let now = Utc::now();
    let cache = load_fresh_coverage_cache(project_root, now, chrono::Duration::hours(24))?;
    let floor_pct = intent_capture_floor_pct(project_root);
    let ic = intent_capture_share(&cache.specs, now, floor_pct);
    ic.render_status_line()
}

/// Print the status intent-capture line if below the configured floor.
// trace:TASK-1522 | ai:antigravity
pub(crate) fn print_status_intent_capture_line(project_root: &Path) {
    if let Some(line) = status_intent_capture_line(project_root) {
        println!("{line}");
    }
}
