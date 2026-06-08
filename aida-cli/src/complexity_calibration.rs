//! Three-way complexity calibration — STORY-439.
//!
//! Per-spec calibration record (`.aida/complexity-calibration/<SPEC>.yaml`)
//! accumulates three slots over a spec's lifecycle: a **pickup** estimate
//! when `aida queue work --complexity X --assist-est Y` runs, a **ship**
//! self-assessment when `aida pr ship --complexity Z` runs (with the
//! actual punt count read from `.aida/punts.jsonl`), and a **review**
//! assessment when the reviewer's verdict file carries the new
//! `implementation_complexity` / `complexity_agreement` fields.
//!
//! The whole point is to make the substrate's self-knowledge measurable:
//! when pickup-predicted complexity consistently diverges from
//! reviewer-assessed complexity, the gap names a class of work the
//! agents systematically misjudge. The substrate-gap signal surfaces
//! via `aida autonomy calibration mismatches`.
//!
//! Layout + idioms mirror STORY-347 (`crate::calibration`): one file per
//! record, parse-tolerant directory walk, YAML on disk. Telemetry writes
//! gate on `crate::usage::is_enabled` so `AIDA_TELEMETRY=0` /
//! `[telemetry] enabled = false` suppresses every capture — the same
//! kill-switch the punt ledger uses.
//!
//! trace:STORY-439 | ai:claude

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};

// --- Levels ---------------------------------------------------------------

/// Three-step implementation complexity. Operator-set at pickup, reviewer
/// may override based on the diff.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub enum ComplexityLevel {
    Low,
    Med,
    High,
}

impl ComplexityLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            ComplexityLevel::Low => "low",
            ComplexityLevel::Med => "med",
            ComplexityLevel::High => "high",
        }
    }

    /// Parse a tag value or freeform string. Tolerant of casing and a
    /// `medium` alias for `med`.
    pub fn parse_str(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "low" | "l" => Some(ComplexityLevel::Low),
            "med" | "medium" | "m" => Some(ComplexityLevel::Med),
            "high" | "h" => Some(ComplexityLevel::High),
            _ => None,
        }
    }

    /// 0/1/2 — the rank used by the mismatch view's `delta_steps`.
    fn rank(self) -> i32 {
        match self {
            ComplexityLevel::Low => 0,
            ComplexityLevel::Med => 1,
            ComplexityLevel::High => 2,
        }
    }
}

impl std::fmt::Display for ComplexityLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Three-step expected assistance level. Records *what the pickup thinks
/// will be needed*; the actual count of advisor / human interventions
/// comes from the STORY-325 punt ledger via [`punt_count_for_spec`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum AssistanceLevel {
    None,
    Advisor,
    Human,
}

impl AssistanceLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            AssistanceLevel::None => "none",
            AssistanceLevel::Advisor => "advisor",
            AssistanceLevel::Human => "human",
        }
    }

    // TASK-552: kept for STORY-439 tag/import consumers; current write paths
    // use clap `ValueEnum`, while persisted/freeform inputs still need this.
    #[allow(dead_code)]
    pub fn parse_str(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "none" | "no" => Some(AssistanceLevel::None),
            "advisor" => Some(AssistanceLevel::Advisor),
            "human" => Some(AssistanceLevel::Human),
            _ => None,
        }
    }
}

impl std::fmt::Display for AssistanceLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The reviewer's call on whether the implementer's ship-side
/// complexity estimate matched what the diff actually demanded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComplexityAgreement {
    Matched,
    ImplementerUnderestimated,
    ImplementerOverestimated,
}

impl ComplexityAgreement {
    pub fn as_str(self) -> &'static str {
        match self {
            ComplexityAgreement::Matched => "matched",
            ComplexityAgreement::ImplementerUnderestimated => "implementer-underestimated",
            ComplexityAgreement::ImplementerOverestimated => "implementer-overestimated",
        }
    }

    pub fn parse_str(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "matched" | "match" => Some(ComplexityAgreement::Matched),
            "implementer-underestimated" | "underestimated" | "under" => {
                Some(ComplexityAgreement::ImplementerUnderestimated)
            }
            "implementer-overestimated" | "overestimated" | "over" => {
                Some(ComplexityAgreement::ImplementerOverestimated)
            }
            _ => None,
        }
    }
}

impl std::fmt::Display for ComplexityAgreement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Compute the agreement the reviewer *would* assign by comparing two
/// levels mechanically (pickup-or-ship vs reviewer). Useful when the
/// reviewer fills `implementation_complexity` but skips
/// `complexity_agreement` — the orchestrator derives it.
pub fn compute_agreement(
    estimate: ComplexityLevel,
    reviewer: ComplexityLevel,
) -> ComplexityAgreement {
    match reviewer.rank() - estimate.rank() {
        0 => ComplexityAgreement::Matched,
        d if d > 0 => ComplexityAgreement::ImplementerUnderestimated,
        _ => ComplexityAgreement::ImplementerOverestimated,
    }
}

// --- Tag conventions ------------------------------------------------------

/// Tag prefix carrying the spec's predicted-or-actual complexity:
/// `complexity:low|med|high`.
pub const COMPLEXITY_TAG_PREFIX: &str = "complexity:";

/// Tag prefix carrying the pickup-time assistance estimate:
/// `estimated-assistance:none|advisor|human`.
pub const ASSISTANCE_TAG_PREFIX: &str = "estimated-assistance:";

/// Parse the first `complexity:*` tag, when present. Mirrors the scan
/// idiom in `crate::findings::pr_number_from_tag`.
// TASK-552: reserved for STORY-439 calibration readers that derive estimates
// from existing spec tags during mismatch/reporting flows.
#[allow(dead_code)]
pub fn complexity_from_tags(tags: &[String]) -> Option<ComplexityLevel> {
    tags.iter()
        .find_map(|t| t.strip_prefix(COMPLEXITY_TAG_PREFIX))
        .and_then(ComplexityLevel::parse_str)
}

/// Parse the first `estimated-assistance:*` tag, when present.
// TASK-552: reserved alongside `complexity_from_tags` for calibration
// reporting/import paths that consume existing `estimated-assistance:*` tags.
#[allow(dead_code)]
pub fn assistance_est_from_tags(tags: &[String]) -> Option<AssistanceLevel> {
    tags.iter()
        .find_map(|t| t.strip_prefix(ASSISTANCE_TAG_PREFIX))
        .and_then(AssistanceLevel::parse_str)
}

/// Replace any existing `complexity:*` tag with `level`. Returns `true`
/// when the tag set was actually mutated (so the caller can short-circuit
/// a no-op write to the spec).
pub fn apply_complexity_tag(tags: &mut HashSet<String>, level: ComplexityLevel) -> bool {
    apply_prefixed_tag(tags, COMPLEXITY_TAG_PREFIX, level.as_str())
}

/// Replace any existing `estimated-assistance:*` tag with `level`.
pub fn apply_assistance_tag(tags: &mut HashSet<String>, level: AssistanceLevel) -> bool {
    apply_prefixed_tag(tags, ASSISTANCE_TAG_PREFIX, level.as_str())
}

/// Replace every tag starting with `prefix` with the single concatenated
/// `prefix + value`. Returns whether the set was mutated.
fn apply_prefixed_tag(tags: &mut HashSet<String>, prefix: &str, value: &str) -> bool {
    let target = format!("{prefix}{value}");
    let before_len = tags.len();
    let had_target = tags.contains(&target);
    let other_prefixed_count = tags
        .iter()
        .filter(|t| t.starts_with(prefix) && t.as_str() != target)
        .count();
    if had_target && other_prefixed_count == 0 {
        return false;
    }
    tags.retain(|t| !t.starts_with(prefix));
    tags.insert(target);
    // Mutated unless the post-set equals the pre-set (size + target
    // membership pin it — retain stripped exactly `other_prefixed_count
    // + (had_target as usize)` tags, then inserted 1).
    !(had_target && other_prefixed_count == 0 && tags.len() == before_len)
}

// --- Record -------------------------------------------------------------

/// One pickup-time slot on a [`CalibrationCapture`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PickupSlot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub complexity: Option<ComplexityLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistance: Option<AssistanceLevel>,
    pub ts: DateTime<Utc>,
}

/// One ship-time slot on a [`CalibrationCapture`]. `punt_count` is the
/// count of `.aida/punts.jsonl` records whose `spec` field matched at
/// the moment `aida pr ship` ran — pulled from [`punt_count_for_spec`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShipSlot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub complexity: Option<ComplexityLevel>,
    pub punt_count: usize,
    pub ts: DateTime<Utc>,
}

/// One reviewer-time slot on a [`CalibrationCapture`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewSlot {
    pub complexity: ComplexityLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agreement: Option<ComplexityAgreement>,
    pub ts: DateTime<Utc>,
}

/// Per-spec calibration record. Three slots, each optional — a spec
/// captured at only one or two of the three lifecycle points still
/// produces a valid record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CalibrationCapture {
    pub spec: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pickup: Option<PickupSlot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ship: Option<ShipSlot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewSlot>,
}

impl CalibrationCapture {
    fn new(spec: &str) -> Self {
        Self {
            spec: spec.to_string(),
            pickup: None,
            ship: None,
            review: None,
        }
    }

    /// The newest timestamp across the three slots. Used by the mismatch
    /// view's `--since` window.
    pub fn latest_ts(&self) -> Option<DateTime<Utc>> {
        [
            self.pickup.as_ref().map(|s| s.ts),
            self.ship.as_ref().map(|s| s.ts),
            self.review.as_ref().map(|s| s.ts),
        ]
        .into_iter()
        .flatten()
        .max()
    }
}

// --- File IO --------------------------------------------------------------

/// `.aida/complexity-calibration/` — directory that holds every per-spec
/// capture record. Gitignored by the deny-by-default `.aida/*` rule.
pub fn capture_dir(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("complexity-calibration")
}

/// `.aida/complexity-calibration/<SPEC>.yaml` — the per-spec record.
pub fn capture_path(project_root: &Path, spec: &str) -> PathBuf {
    capture_dir(project_root).join(format!("{spec}.yaml"))
}

/// Read one spec's capture record. `None` when the file is absent or
/// unparseable — the absent case fronts the upsert path's "start fresh"
/// branch.
pub fn read_capture(project_root: &Path, spec: &str) -> Option<CalibrationCapture> {
    let path = capture_path(project_root, spec);
    let body = std::fs::read_to_string(&path).ok()?;
    serde_yaml::from_str(&body).ok()
}

/// Write a capture record verbatim — creates the per-project directory
/// as needed.
pub fn write_capture(project_root: &Path, record: &CalibrationCapture) -> Result<()> {
    let dir = capture_dir(project_root);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = capture_path(project_root, &record.spec);
    let body = serde_yaml::to_string(record).context("serialising calibration capture")?;
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Walk `.aida/complexity-calibration/*.yaml` and return every record;
/// newest-first by `latest_ts()`. Garbage / forward-incompatible files
/// are skipped rather than aborting the read — the worst case is a
/// dropped row from one triage view, not a crashed CLI.
pub fn read_all_captures(project_root: &Path) -> Vec<CalibrationCapture> {
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
        if let Ok(record) = serde_yaml::from_str::<CalibrationCapture>(&body) {
            records.push(record);
        }
    }
    records.sort_by_key(|r| std::cmp::Reverse(r.latest_ts()));
    records
}

// --- Upserts --------------------------------------------------------------

/// Write the pickup slot for `spec`. Telemetry kill-switch: when
/// `usage::is_enabled` reports off, the call is a graceful no-op (matches
/// `punt::append_to_ledger`'s gate). Best-effort — write errors return
/// `Err` but the caller can ignore them so a transient FS issue never
/// breaks `aida queue work`.
pub fn upsert_pickup(
    project_root: &Path,
    spec: &str,
    complexity: Option<ComplexityLevel>,
    assistance: Option<AssistanceLevel>,
) -> Result<()> {
    if !crate::usage::is_enabled(Some(project_root)) {
        return Ok(());
    }
    if complexity.is_none() && assistance.is_none() {
        return Ok(());
    }
    let mut record =
        read_capture(project_root, spec).unwrap_or_else(|| CalibrationCapture::new(spec));
    record.pickup = Some(PickupSlot {
        complexity,
        assistance,
        ts: Utc::now(),
    });
    write_capture(project_root, &record)
}

/// Write the ship slot for `spec`. Punt count comes from
/// [`punt_count_for_spec`] at the same instant — the ledger is the only
/// source of truth for "how many forks did this spec actually hit."
pub fn upsert_ship(
    project_root: &Path,
    spec: &str,
    complexity: Option<ComplexityLevel>,
    punt_count: usize,
) -> Result<()> {
    if !crate::usage::is_enabled(Some(project_root)) {
        return Ok(());
    }
    let mut record =
        read_capture(project_root, spec).unwrap_or_else(|| CalibrationCapture::new(spec));
    record.ship = Some(ShipSlot {
        complexity,
        punt_count,
        ts: Utc::now(),
    });
    write_capture(project_root, &record)
}

/// Write the reviewer slot for `spec`. When `agreement` is `None` and a
/// ship-or-pickup complexity slot exists, the orchestrator derives one
/// mechanically via [`compute_agreement`] — the reviewer can override by
/// writing the field explicitly in the verdict file.
pub fn upsert_review(
    project_root: &Path,
    spec: &str,
    complexity: ComplexityLevel,
    agreement: Option<ComplexityAgreement>,
) -> Result<()> {
    if !crate::usage::is_enabled(Some(project_root)) {
        return Ok(());
    }
    let mut record =
        read_capture(project_root, spec).unwrap_or_else(|| CalibrationCapture::new(spec));
    let derived = match agreement {
        Some(a) => Some(a),
        None => record
            .ship
            .as_ref()
            .and_then(|s| s.complexity)
            .or_else(|| record.pickup.as_ref().and_then(|s| s.complexity))
            .map(|est| compute_agreement(est, complexity)),
    };
    record.review = Some(ReviewSlot {
        complexity,
        agreement: derived,
        ts: Utc::now(),
    });
    write_capture(project_root, &record)
}

// --- Punt count -----------------------------------------------------------

/// Count of `.aida/punts.jsonl` records whose `spec` matches. A single
/// spec has at most one drain in flight, so the all-time count for that
/// spec is the count of forks during its implementation. Reads-tolerant
/// of an absent ledger (returns `0`).
pub fn punt_count_for_spec(project_root: &Path, spec: &str) -> usize {
    crate::punt::read_ledger(project_root)
        .into_iter()
        .filter(|r| r.spec == spec)
        .count()
}

// --- Mismatch view --------------------------------------------------------

/// One row in `aida autonomy calibration mismatches`. Surfaces specs
/// where pickup-predicted complexity diverged from reviewer-assessed
/// complexity — the substrate-gap signal.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MismatchRow {
    pub spec: String,
    pub pickup_complexity: ComplexityLevel,
    pub reviewer_complexity: ComplexityLevel,
    /// Signed step delta: `reviewer_rank - pickup_rank`. Positive ⇒
    /// implementer underestimated; negative ⇒ overestimated.
    pub delta_steps: i32,
    pub agreement: ComplexityAgreement,
    pub ts: DateTime<Utc>,
}

/// Build the mismatches view from a set of captures.
///
/// Keeps only records with both a pickup-complexity AND a reviewer
/// slot. `since`, when set, filters by `latest_ts()`. Output is sorted
/// by `|delta_steps|` descending (biggest gap first), then by `ts`
/// descending so newer rows lead within a tie.
pub fn mismatches(
    records: &[CalibrationCapture],
    since: Option<ChronoDuration>,
) -> Vec<MismatchRow> {
    let cutoff = since.map(|d| Utc::now() - d);
    let mut rows: Vec<MismatchRow> = records
        .iter()
        .filter(|r| match cutoff {
            Some(c) => r.latest_ts().map(|t| t >= c).unwrap_or(false),
            None => true,
        })
        .filter_map(|r| {
            let pickup = r.pickup.as_ref()?.complexity?;
            let review = r.review.as_ref()?;
            if pickup == review.complexity {
                return None;
            }
            let delta = review.complexity.rank() - pickup.rank();
            Some(MismatchRow {
                spec: r.spec.clone(),
                pickup_complexity: pickup,
                reviewer_complexity: review.complexity,
                delta_steps: delta,
                agreement: review
                    .agreement
                    .unwrap_or_else(|| compute_agreement(pickup, review.complexity)),
                ts: review.ts,
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        b.delta_steps
            .abs()
            .cmp(&a.delta_steps.abs())
            .then_with(|| b.ts.cmp(&a.ts))
    });
    rows
}

// --- Tests ----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(spec: &str) -> CalibrationCapture {
        CalibrationCapture::new(spec)
    }

    #[test]
    fn complexity_level_parse_and_display_round_trip() {
        for &lvl in &[
            ComplexityLevel::Low,
            ComplexityLevel::Med,
            ComplexityLevel::High,
        ] {
            assert_eq!(ComplexityLevel::parse_str(lvl.as_str()), Some(lvl));
            assert_eq!(format!("{lvl}"), lvl.as_str());
        }
        assert_eq!(
            ComplexityLevel::parse_str("Medium"),
            Some(ComplexityLevel::Med)
        );
        assert_eq!(
            ComplexityLevel::parse_str("HIGH"),
            Some(ComplexityLevel::High)
        );
        assert!(ComplexityLevel::parse_str("nope").is_none());
        assert!(ComplexityLevel::parse_str("").is_none());
    }

    #[test]
    fn assistance_level_parse_and_display_round_trip() {
        for &lvl in &[
            AssistanceLevel::None,
            AssistanceLevel::Advisor,
            AssistanceLevel::Human,
        ] {
            assert_eq!(AssistanceLevel::parse_str(lvl.as_str()), Some(lvl));
            assert_eq!(format!("{lvl}"), lvl.as_str());
        }
        assert_eq!(
            AssistanceLevel::parse_str("HUMAN"),
            Some(AssistanceLevel::Human)
        );
        assert!(AssistanceLevel::parse_str("magic").is_none());
    }

    #[test]
    fn apply_complexity_tag_replaces_existing_complexity_tag() {
        let mut tags = HashSet::new();
        tags.insert("complexity:low".to_string());
        tags.insert("from-user-direction".to_string());
        let mutated = apply_complexity_tag(&mut tags, ComplexityLevel::High);
        assert!(mutated, "tag set differs from previous");
        assert!(tags.contains("complexity:high"));
        assert!(!tags.contains("complexity:low"));
        assert!(tags.contains("from-user-direction"));
        // Re-applying the same tag is a no-op (no mutation reported).
        let again = apply_complexity_tag(&mut tags, ComplexityLevel::High);
        assert!(!again, "re-apply is a no-op");
    }

    #[test]
    fn apply_complexity_tag_preserves_unrelated_tags() {
        let mut tags = HashSet::new();
        tags.insert("metrics".to_string());
        tags.insert("batch:overnight".to_string());
        apply_complexity_tag(&mut tags, ComplexityLevel::Med);
        assert!(tags.contains("metrics"));
        assert!(tags.contains("batch:overnight"));
        assert!(tags.contains("complexity:med"));
    }

    #[test]
    fn apply_assistance_tag_replaces_existing_assistance_tag() {
        let mut tags = HashSet::new();
        tags.insert("estimated-assistance:none".to_string());
        let mutated = apply_assistance_tag(&mut tags, AssistanceLevel::Advisor);
        assert!(mutated);
        assert!(tags.contains("estimated-assistance:advisor"));
        assert!(!tags.contains("estimated-assistance:none"));
    }

    #[test]
    fn complexity_from_tags_returns_none_when_absent() {
        let tags = vec!["metrics".to_string(), "batch:x".to_string()];
        assert_eq!(complexity_from_tags(&tags), None);
        assert_eq!(assistance_est_from_tags(&tags), None);
    }

    #[test]
    fn complexity_from_tags_reads_first_matching() {
        let tags = vec!["complexity:high".to_string(), "complexity:low".to_string()];
        assert_eq!(complexity_from_tags(&tags), Some(ComplexityLevel::High));
    }

    #[test]
    fn compute_agreement_returns_matched_when_levels_equal() {
        assert_eq!(
            compute_agreement(ComplexityLevel::Med, ComplexityLevel::Med),
            ComplexityAgreement::Matched
        );
    }

    #[test]
    fn compute_agreement_returns_under_when_pickup_lower_than_review() {
        assert_eq!(
            compute_agreement(ComplexityLevel::Low, ComplexityLevel::High),
            ComplexityAgreement::ImplementerUnderestimated
        );
    }

    #[test]
    fn compute_agreement_returns_over_when_pickup_higher_than_review() {
        assert_eq!(
            compute_agreement(ComplexityLevel::High, ComplexityLevel::Low),
            ComplexityAgreement::ImplementerOverestimated
        );
    }

    #[test]
    fn pickup_capture_writes_yaml_and_round_trips_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        upsert_pickup(
            dir.path(),
            "STORY-439",
            Some(ComplexityLevel::Med),
            Some(AssistanceLevel::Advisor),
        )
        .unwrap();
        let read = read_capture(dir.path(), "STORY-439").expect("file written");
        assert_eq!(read.spec, "STORY-439");
        let p = read.pickup.expect("pickup slot");
        assert_eq!(p.complexity, Some(ComplexityLevel::Med));
        assert_eq!(p.assistance, Some(AssistanceLevel::Advisor));
        assert!(read.ship.is_none());
        assert!(read.review.is_none());
    }

    #[test]
    fn pickup_upsert_noops_when_both_args_absent() {
        let dir = tempfile::tempdir().unwrap();
        upsert_pickup(dir.path(), "STORY-439", None, None).unwrap();
        assert!(read_capture(dir.path(), "STORY-439").is_none());
    }

    #[test]
    fn ship_upsert_preserves_pickup_slot() {
        let dir = tempfile::tempdir().unwrap();
        upsert_pickup(dir.path(), "S1", Some(ComplexityLevel::Low), None).unwrap();
        upsert_ship(dir.path(), "S1", Some(ComplexityLevel::Med), 2).unwrap();
        let read = read_capture(dir.path(), "S1").unwrap();
        assert!(read.pickup.is_some(), "pickup preserved");
        let ship = read.ship.unwrap();
        assert_eq!(ship.complexity, Some(ComplexityLevel::Med));
        assert_eq!(ship.punt_count, 2);
    }

    #[test]
    fn review_upsert_preserves_pickup_and_ship_slots_and_derives_agreement() {
        let dir = tempfile::tempdir().unwrap();
        upsert_pickup(dir.path(), "S2", Some(ComplexityLevel::Low), None).unwrap();
        upsert_ship(dir.path(), "S2", Some(ComplexityLevel::Med), 0).unwrap();
        upsert_review(dir.path(), "S2", ComplexityLevel::High, None).unwrap();
        let read = read_capture(dir.path(), "S2").unwrap();
        assert!(read.pickup.is_some());
        assert!(read.ship.is_some());
        let review = read.review.unwrap();
        assert_eq!(review.complexity, ComplexityLevel::High);
        // Derived from the ship slot (most recent prior estimate): med → high.
        assert_eq!(
            review.agreement,
            Some(ComplexityAgreement::ImplementerUnderestimated)
        );
    }

    #[test]
    fn upsert_pickup_no_ops_when_telemetry_disabled() {
        // Use the project-config kill-switch (not the env var) so this test
        // doesn't race with sibling tests under `cargo test`'s thread pool.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
        std::fs::write(
            dir.path().join(".aida").join("config.toml"),
            "[telemetry]\nenabled = false\n",
        )
        .unwrap();
        let res = upsert_pickup(
            dir.path(),
            "S3",
            Some(ComplexityLevel::Med),
            Some(AssistanceLevel::None),
        );
        assert!(res.is_ok());
        assert!(
            read_capture(dir.path(), "S3").is_none(),
            "kill-switch suppresses write"
        );
    }

    #[test]
    fn read_all_captures_skips_garbage_files() {
        let dir = tempfile::tempdir().unwrap();
        upsert_pickup(dir.path(), "S-OK", Some(ComplexityLevel::Low), None).unwrap();
        std::fs::create_dir_all(capture_dir(dir.path())).unwrap();
        std::fs::write(
            capture_dir(dir.path()).join("garbage.yaml"),
            "this is not: valid: yaml: :",
        )
        .unwrap();
        // A non-yaml file is skipped on extension.
        std::fs::write(capture_dir(dir.path()).join("notes.txt"), "ignore me").unwrap();
        let all = read_all_captures(dir.path());
        assert_eq!(
            all.len(),
            1,
            "{:?}",
            all.iter().map(|c| &c.spec).collect::<Vec<_>>()
        );
        assert_eq!(all[0].spec, "S-OK");
    }

    #[test]
    fn read_all_captures_returns_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        // Two captures; the second is written with a touch-newer pickup ts.
        upsert_pickup(dir.path(), "OLD", Some(ComplexityLevel::Low), None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        upsert_pickup(dir.path(), "NEW", Some(ComplexityLevel::Med), None).unwrap();
        let all = read_all_captures(dir.path());
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].spec, "NEW");
        assert_eq!(all[1].spec, "OLD");
    }

    fn record_with(
        spec: &str,
        pickup: Option<ComplexityLevel>,
        review: Option<ComplexityLevel>,
        ts: DateTime<Utc>,
    ) -> CalibrationCapture {
        let mut c = cap(spec);
        if let Some(p) = pickup {
            c.pickup = Some(PickupSlot {
                complexity: Some(p),
                assistance: None,
                ts,
            });
        }
        if let Some(r) = review {
            c.review = Some(ReviewSlot {
                complexity: r,
                agreement: None,
                ts,
            });
        }
        c
    }

    #[test]
    fn mismatches_drops_records_missing_review_half() {
        let now = Utc::now();
        let records = vec![record_with("S1", Some(ComplexityLevel::Low), None, now)];
        assert!(mismatches(&records, None).is_empty());
    }

    #[test]
    fn mismatches_drops_records_missing_pickup_complexity() {
        let now = Utc::now();
        // Pickup slot exists but no complexity recorded — only an
        // assistance estimate. Mismatch needs the complexity to compare.
        let mut c = cap("S1");
        c.pickup = Some(PickupSlot {
            complexity: None,
            assistance: Some(AssistanceLevel::None),
            ts: now,
        });
        c.review = Some(ReviewSlot {
            complexity: ComplexityLevel::High,
            agreement: None,
            ts: now,
        });
        assert!(mismatches(&[c], None).is_empty());
    }

    #[test]
    fn mismatches_drops_records_where_levels_match() {
        let now = Utc::now();
        let r = record_with(
            "S1",
            Some(ComplexityLevel::Med),
            Some(ComplexityLevel::Med),
            now,
        );
        assert!(mismatches(&[r], None).is_empty());
    }

    #[test]
    fn mismatches_surfaces_pickup_low_vs_review_high_as_underestimate() {
        let now = Utc::now();
        let r = record_with(
            "S1",
            Some(ComplexityLevel::Low),
            Some(ComplexityLevel::High),
            now,
        );
        let rows = mismatches(&[r], None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].spec, "S1");
        assert_eq!(rows[0].delta_steps, 2);
        assert_eq!(
            rows[0].agreement,
            ComplexityAgreement::ImplementerUnderestimated
        );
    }

    #[test]
    fn mismatches_ranks_biggest_gap_first() {
        let now = Utc::now();
        let records = vec![
            record_with(
                "S-small",
                Some(ComplexityLevel::Med),
                Some(ComplexityLevel::High),
                now,
            ),
            record_with(
                "S-big",
                Some(ComplexityLevel::Low),
                Some(ComplexityLevel::High),
                now,
            ),
            record_with(
                "S-over",
                Some(ComplexityLevel::High),
                Some(ComplexityLevel::Low),
                now,
            ),
        ];
        let rows = mismatches(&records, None);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].delta_steps.abs(), 2);
        assert_eq!(rows[2].delta_steps.abs(), 1);
    }

    #[test]
    fn mismatches_filters_by_since_window() {
        let now = Utc::now();
        let recent = record_with(
            "S-RECENT",
            Some(ComplexityLevel::Low),
            Some(ComplexityLevel::High),
            now - ChronoDuration::hours(2),
        );
        let old = record_with(
            "S-OLD",
            Some(ComplexityLevel::Low),
            Some(ComplexityLevel::High),
            now - ChronoDuration::days(30),
        );
        let filtered = mismatches(
            &[recent.clone(), old.clone()],
            Some(ChronoDuration::days(1)),
        );
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].spec, "S-RECENT");
    }

    #[test]
    fn punt_count_for_spec_aggregates_only_matching_records() {
        use crate::punt::{append_to_ledger, PuntRecord};
        use aida_core::PuntCategory;
        let dir = tempfile::tempdir().unwrap();
        // No `.aida/config.toml` in the tempdir + no `AIDA_TELEMETRY=0` set by
        // this test means `is_enabled` returns the default `true`. Sibling
        // tests that mutate `AIDA_TELEMETRY` use `.aida/config.toml` instead
        // (see `upsert_pickup_no_ops_when_telemetry_disabled`).
        let rec = |spec: &str| PuntRecord {
            timestamp: Utc::now(),
            spec: spec.to_string(),
            category: PuntCategory::DesignFork,
            detail: "smoke".to_string(),
            lean: None,
            raised_by: None,
            resolution_path: "punted".to_string(),
            classification: None,
            escalation_reason: None,
            answer: None,
            answered_by: None,
            decision: None,
            principle_link: None,
            calibration_pair: None,
            paused_at: None,
            resolved_at: None,
        };
        append_to_ledger(dir.path(), &rec("STORY-100")).unwrap();
        append_to_ledger(dir.path(), &rec("STORY-100")).unwrap();
        append_to_ledger(dir.path(), &rec("STORY-200")).unwrap();
        assert_eq!(punt_count_for_spec(dir.path(), "STORY-100"), 2);
        assert_eq!(punt_count_for_spec(dir.path(), "STORY-200"), 1);
        assert_eq!(punt_count_for_spec(dir.path(), "STORY-404"), 0);
    }
}
