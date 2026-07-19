//! Cold-boot vs fork-from-live advisor calibration ledger — STORY-347.
//!
//! When the `[advisor] calibration_mode = "on"` toggle is set (or the
//! per-drain `--calibrate` flag is passed), every punt that the orchestrator
//! routes to the headless advisor (STORY-306) produces **two** verdicts: a
//! cold-boot one (the existing `claude -p` substrate-only path) plus a
//! fork-from-live one (STORY-360 — copy the live advisor's JSONL transcript
//! and `claude --resume` it). The cold-boot still drives the drain; the
//! fork is shadow only.
//!
//! Each pair is recorded to `.aida/punts/<punt-id>/calibration.yaml`. The
//! review surface (`aida findings calibration`) reads the directory back to
//! surface disagreements for live-advisor triage. Disagreements are
//! interpretable: every one is either a substrate gap (a memory that should
//! exist but doesn't), an inherently in-flight framing, or a case where the
//! cold-boot's fresh read of the substrate was actually cleaner than the
//! live advisor's potentially-stale context.
//!
//! trace:STORY-347 | ai:claude

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};

use crate::punt::{PuntResolution, PuntResponse};

/// One side of a calibration pair — a single advisor's verdict.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CalibrationVerdict {
    /// `"resolved"` or `"escalated"` — kept a string in the ledger so a
    /// reader that doesn't have AIDA's enums (a script, a future tool)
    /// still gets a usable record.
    pub resolution: String,
    /// The decision the advisor wrote when it resolved. `None` on an
    /// escalation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// The reasoning trace — always present (every advisor decision is
    /// auditable).
    pub reasoning: String,
    /// The A/B/C calibration class the advisor assigned, when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<String>,
    /// On an escalation, the categorized reason a human is needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalation_reason: Option<String>,
    /// Path to the `claude -p` JSONL log this verdict came from, when one
    /// was captured. Relative to the project root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_path: Option<String>,
}

impl CalibrationVerdict {
    /// Build from a [`PuntResponse`] — what the advisor wrote to the
    /// punt-response file.
    pub fn from_response(response: &PuntResponse, log_path: Option<String>) -> Self {
        Self {
            resolution: match response.resolution {
                PuntResolution::Resolved => "resolved".to_string(),
                PuntResolution::Escalated => "escalated".to_string(),
            },
            answer: response.answer.clone(),
            reasoning: response.reasoning.clone(),
            classification: response.classification.clone(),
            escalation_reason: response.escalation_reason.clone(),
            log_path,
        }
    }
}

/// One calibration record — the pair of verdicts (or just cold-boot when no
/// live advisor was registered). The ledger lives at
/// `.aida/punts/<punt-id>/calibration.yaml`, one file per punt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CalibrationRecord {
    pub punt_id: String,
    pub spec: String,
    pub timestamp: DateTime<Utc>,
    /// The cold-boot advisor's verdict — always present. It is the verdict
    /// that drove the drain.
    pub cold_boot: CalibrationVerdict,
    /// The fork-from-live advisor's verdict, when one ran. `None` when no
    /// live advisor was registered, the fork was infeasible (size cap,
    /// stale JSONL), or the fork process failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork: Option<CalibrationVerdict>,
    /// Reason the fork was skipped, when `fork` is None. `"no-live-advisor"`,
    /// `"fork-failed"`, `"size-cap-exceeded"`, etc. — surfaced in the review
    /// view so a reader can tell "fork ran and agreed" apart from "fork did
    /// not run."
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_skip_reason: Option<String>,
    /// Which verdict drove the drain. With STORY-347 this is always
    /// `"cold-boot"` (the fork is shadow only); the field exists so a future
    /// extension could flip the default without breaking the schema.
    pub drove_drain: String,
    /// Triage annotation — one line per record, written by
    /// `aida findings calibration annotate <punt-id> "..."`. Prefix
    /// conventions (not enforced): `gap → wrote memory <name>`,
    /// `inherently in-flight, accept`, `cold-boot was actually correct`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotation: Option<String>,
}

impl CalibrationRecord {
    /// Did the two verdicts agree on the drain-driving decision? `None`
    /// when the fork did not run (there is no pair to compare).
    ///
    /// Agreement = same resolution AND (for a resolved pair) the answers
    /// match. We compare answers literally — a future extension could
    /// embedding-compare them, but byte-exact catches the common case where
    /// both advisors pick from a small set of recorded options.
    pub fn agreement(&self) -> Option<bool> {
        let fork = self.fork.as_ref()?;
        if self.cold_boot.resolution != fork.resolution {
            return Some(false);
        }
        // Both resolved → answers must match (whitespace-normalised).
        if self.cold_boot.resolution == "resolved" {
            let a = self
                .cold_boot
                .answer
                .as_deref()
                .map(str::trim)
                .unwrap_or_default();
            let b = fork.answer.as_deref().map(str::trim).unwrap_or_default();
            Some(a == b)
        } else {
            // Both escalated → counts as agreement (both said "ask a human").
            Some(true)
        }
    }

    /// The category prefix this record's annotation starts with — the
    /// histogram axis for `--stats`. Returns `None` when the annotation is
    /// missing or doesn't start with one of the recommended prefixes.
    pub fn annotation_category(&self) -> Option<&'static str> {
        let raw = self.annotation.as_deref()?.trim().to_ascii_lowercase();
        if raw.starts_with("gap") {
            Some("gap")
        } else if raw.starts_with("inherently in-flight") || raw.starts_with("in-flight") {
            Some("in-flight")
        } else if raw.starts_with("cold-boot was") || raw.starts_with("cold-boot correct") {
            Some("cold-boot-correct")
        } else {
            None
        }
    }
}

/// Build a punt-id from a spec + the wall-clock time. The format is
/// stable and human-sortable: `<SPEC>-<unix-seconds>`. Stable enough to be
/// useful in `aida findings calibration annotate <punt-id> ...`; unique
/// enough that two punts on the same spec within the same drain still get
/// distinct directories (one second is more than enough for the
/// orchestrator's serial advisor calls).
pub fn build_punt_id(spec: &str, now: DateTime<Utc>) -> String {
    format!("{spec}-{}", now.timestamp())
}

/// `.aida/punts/<punt-id>/` — the directory the calibration ledger writes
/// its `calibration.yaml` into, alongside any future per-punt artefacts.
pub fn punt_dir(project_root: &Path, punt_id: &str) -> PathBuf {
    project_root.join(".aida").join("punts").join(punt_id)
}

/// `.aida/punts/<punt-id>/calibration.yaml` — the ledger file.
pub fn calibration_path(project_root: &Path, punt_id: &str) -> PathBuf {
    punt_dir(project_root, punt_id).join("calibration.yaml")
}

/// Write a calibration record, creating the per-punt directory if needed.
pub fn write_calibration(project_root: &Path, record: &CalibrationRecord) -> Result<()> {
    let dir = punt_dir(project_root, &record.punt_id);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = calibration_path(project_root, &record.punt_id);
    let body = serde_yaml::to_string(record).context("serialising calibration record")?;
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Read a single calibration record by punt-id. `None` when the file is
/// absent or unparseable — the review surface reads it as "no record."
pub fn read_calibration(project_root: &Path, punt_id: &str) -> Option<CalibrationRecord> {
    let path = calibration_path(project_root, punt_id);
    let body = std::fs::read_to_string(&path).ok()?;
    serde_yaml::from_str::<CalibrationRecord>(&body).ok()
}

/// Walk `.aida/punts/*/calibration.yaml` and return every record, newest
/// first. Bad/forward-incompatible files are skipped rather than aborting
/// the read — the worst case is a file gets dropped from one triage view
/// but the others still surface.
pub fn read_all_calibrations(project_root: &Path) -> Vec<CalibrationRecord> {
    let dir = project_root.join(".aida").join("punts");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut records: Vec<CalibrationRecord> = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let cal = p.join("calibration.yaml");
        let Ok(body) = std::fs::read_to_string(&cal) else {
            continue;
        };
        if let Ok(record) = serde_yaml::from_str::<CalibrationRecord>(&body) {
            records.push(record);
        }
    }
    records.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    records
}

/// Update only the `annotation` field of a record on disk. Read + rewrite
/// the YAML in place so the rest of the record survives byte-for-byte.
pub fn annotate_calibration(project_root: &Path, punt_id: &str, note: &str) -> Result<()> {
    let mut record = read_calibration(project_root, punt_id)
        .with_context(|| format!("no calibration record for punt-id {punt_id}"))?;
    record.annotation = Some(note.trim().to_string());
    write_calibration(project_root, &record)
}

/// Filters the review surface accepts.
#[derive(Debug, Clone, Default)]
pub struct CalibrationFilter {
    /// `<N>d` / `<N>h` / `<N>w` — restrict to records within the window.
    pub since: Option<ChronoDuration>,
    /// Restrict to one outcome bucket. None = both.
    pub bucket: Option<AgreementBucket>,
}

/// What `--agreement` / `--disagreement` filter to. The default review view
/// shows disagreements only; `--all` widens to both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgreementBucket {
    Agreement,
    Disagreement,
    /// "No pair to compare" — calibration ran with no live advisor or the
    /// fork failed. Surfaced separately so a reader can see "how many punts
    /// found no live advisor" without it muddying the agreement metric.
    #[allow(dead_code)] // surfaced via tests + future `--no-fork` flag
    NoFork,
}

/// Filter a record set by the given filter, preserving the newest-first order.
pub fn filter(records: &[CalibrationRecord], filter: &CalibrationFilter) -> Vec<CalibrationRecord> {
    let cutoff = filter.since.map(|d| Utc::now() - d);
    records
        .iter()
        .filter(|r| match cutoff {
            Some(c) => r.timestamp >= c,
            None => true,
        })
        .filter(|r| match filter.bucket {
            None => true,
            Some(AgreementBucket::Agreement) => matches!(r.agreement(), Some(true)),
            Some(AgreementBucket::Disagreement) => matches!(r.agreement(), Some(false)),
            Some(AgreementBucket::NoFork) => r.agreement().is_none(),
        })
        .cloned()
        .collect()
}

/// Parse a `--since` window of the form `<N>{d,h,w,m}` (days, hours, weeks,
/// minutes). `None` on a missing window; `Err` on a malformed one.
pub fn parse_since(s: &str) -> Result<ChronoDuration, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty --since window".to_string());
    }
    let (num_str, unit) = s.split_at(s.len() - 1);
    let n: i64 = num_str
        .parse()
        .map_err(|_| format!("invalid --since window `{s}` — expected e.g. `7d`, `24h`"))?;
    let dur = match unit {
        "d" => ChronoDuration::days(n),
        "h" => ChronoDuration::hours(n),
        "w" => ChronoDuration::weeks(n),
        "m" => ChronoDuration::minutes(n),
        _ => {
            return Err(format!(
                "invalid --since unit `{unit}` in `{s}` — expected d/h/w/m"
            ));
        }
    };
    Ok(dur)
}

/// Rolling-metric stats over the last `n` records (default 50 in the CLI),
/// plus a 4-week trend bucket and an annotation-category histogram.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CalibrationStats {
    /// Total records considered (`min(n, all_records)`).
    pub considered: usize,
    /// How many of those produced both verdicts (the denominator of the
    /// agreement rate).
    pub paired: usize,
    /// How many of the paired records agreed.
    pub agreed: usize,
    /// How many of the paired records disagreed.
    pub disagreed: usize,
    /// How many calibration runs found no live advisor / had a fork
    /// failure — counted separately so the agreement rate isn't polluted.
    pub no_fork: usize,
    /// Bucketed counts over the last 4 weeks (most recent first). Each
    /// entry is one week.
    pub weekly: Vec<WeeklyBucket>,
    /// Annotation-category histogram: gap / in-flight / cold-boot-correct /
    /// unannotated.
    pub categories: CategoryHistogram,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WeeklyBucket {
    /// Inclusive start of the week (UTC).
    pub week_start: DateTime<Utc>,
    pub paired: usize,
    pub agreed: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CategoryHistogram {
    pub gap: usize,
    pub in_flight: usize,
    pub cold_boot_correct: usize,
    pub unannotated: usize,
}

/// Aggregate stats from a record set. Pure — caller supplies the records.
/// Pass `now` so tests can pin "today" deterministically.
pub fn compute_stats(
    records: &[CalibrationRecord],
    last_n: usize,
    now: DateTime<Utc>,
) -> CalibrationStats {
    let mut stats = CalibrationStats::default();
    let window = records.iter().take(last_n);
    let window_vec: Vec<&CalibrationRecord> = window.collect();
    stats.considered = window_vec.len();

    for r in &window_vec {
        match r.agreement() {
            Some(true) => {
                stats.paired += 1;
                stats.agreed += 1;
            }
            Some(false) => {
                stats.paired += 1;
                stats.disagreed += 1;
            }
            None => stats.no_fork += 1,
        }
        match r.annotation_category() {
            Some("gap") => stats.categories.gap += 1,
            Some("in-flight") => stats.categories.in_flight += 1,
            Some("cold-boot-correct") => stats.categories.cold_boot_correct += 1,
            _ => {
                if r.agreement() == Some(false) && r.annotation.is_none() {
                    stats.categories.unannotated += 1;
                }
            }
        }
    }

    // 4-week trend, most recent week first.
    for week_idx in 0..4 {
        let week_end = now - ChronoDuration::weeks(week_idx as i64);
        let week_start = week_end - ChronoDuration::weeks(1);
        let mut bucket = WeeklyBucket {
            week_start,
            paired: 0,
            agreed: 0,
        };
        for r in records.iter() {
            if r.timestamp >= week_start && r.timestamp < week_end {
                if let Some(agreed) = r.agreement() {
                    bucket.paired += 1;
                    if agreed {
                        bucket.agreed += 1;
                    }
                }
            }
        }
        stats.weekly.push(bucket);
    }

    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::punt::PuntResolution;

    fn resolved(answer: &str) -> CalibrationVerdict {
        CalibrationVerdict {
            resolution: "resolved".to_string(),
            answer: Some(answer.to_string()),
            reasoning: format!("reasoning for {answer}"),
            classification: Some("A".to_string()),
            escalation_reason: None,
            log_path: None,
        }
    }

    fn escalated(reason: &str) -> CalibrationVerdict {
        CalibrationVerdict {
            resolution: "escalated".to_string(),
            answer: None,
            reasoning: reason.to_string(),
            classification: Some("C".to_string()),
            escalation_reason: Some("strategy".to_string()),
            log_path: None,
        }
    }

    fn record_with(
        spec: &str,
        cold: CalibrationVerdict,
        fork: Option<CalibrationVerdict>,
        ts: DateTime<Utc>,
    ) -> CalibrationRecord {
        CalibrationRecord {
            punt_id: build_punt_id(spec, ts),
            spec: spec.to_string(),
            timestamp: ts,
            cold_boot: cold,
            fork,
            fork_skip_reason: None,
            drove_drain: "cold-boot".to_string(),
            annotation: None,
        }
    }

    #[test]
    fn build_punt_id_is_sortable_and_unique_per_second() {
        let t1 = DateTime::parse_from_rfc3339("2026-05-21T18:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let t2 = t1 + ChronoDuration::seconds(1);
        let a = build_punt_id("STORY-347", t1);
        let b = build_punt_id("STORY-347", t2);
        assert_ne!(a, b);
        assert!(a.starts_with("STORY-347-"), "{a}");
        assert!(b > a, "later ts sorts later");
    }

    #[test]
    fn agreement_resolved_same_answer() {
        let t = Utc::now();
        let r = record_with(
            "STORY-347",
            resolved("use OAuth"),
            Some(resolved("use OAuth")),
            t,
        );
        assert_eq!(r.agreement(), Some(true));
    }

    #[test]
    fn agreement_resolved_different_answer_disagrees() {
        let t = Utc::now();
        let r = record_with(
            "STORY-347",
            resolved("use OAuth"),
            Some(resolved("use sessions")),
            t,
        );
        assert_eq!(r.agreement(), Some(false));
    }

    #[test]
    fn agreement_one_resolved_one_escalated_disagrees() {
        let t = Utc::now();
        let r = record_with(
            "STORY-347",
            resolved("ship the default"),
            Some(escalated("ask a human about the strategy")),
            t,
        );
        assert_eq!(r.agreement(), Some(false));
    }

    #[test]
    fn agreement_both_escalated_agrees() {
        let t = Utc::now();
        let r = record_with(
            "STORY-347",
            escalated("strategy call"),
            Some(escalated("strategy call")),
            t,
        );
        assert_eq!(r.agreement(), Some(true));
    }

    #[test]
    fn agreement_none_when_no_fork() {
        let t = Utc::now();
        let r = record_with("STORY-347", resolved("ship"), None, t);
        assert_eq!(r.agreement(), None);
    }

    #[test]
    fn annotation_category_recognises_prefixes() {
        let t = Utc::now();
        let mut r = record_with("STORY-347", resolved("a"), Some(resolved("b")), t);
        r.annotation = Some("gap → wrote memory feedback_x".to_string());
        assert_eq!(r.annotation_category(), Some("gap"));
        r.annotation = Some("inherently in-flight, accept".to_string());
        assert_eq!(r.annotation_category(), Some("in-flight"));
        r.annotation = Some("cold-boot was actually correct here".to_string());
        assert_eq!(r.annotation_category(), Some("cold-boot-correct"));
        r.annotation = Some("Mystery note that doesn't match".to_string());
        assert_eq!(r.annotation_category(), None);
        r.annotation = None;
        assert_eq!(r.annotation_category(), None);
    }

    #[test]
    fn roundtrip_through_yaml_preserves_record() {
        let t = DateTime::parse_from_rfc3339("2026-05-21T18:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut r = record_with(
            "STORY-347",
            resolved("use OAuth"),
            Some(escalated("strategy call")),
            t,
        );
        r.annotation = Some("gap → wrote memory feedback_x".to_string());
        let body = serde_yaml::to_string(&r).unwrap();
        let parsed: CalibrationRecord = serde_yaml::from_str(&body).unwrap();
        assert_eq!(parsed, r);
    }

    #[test]
    fn write_then_read_roundtrip_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let t = Utc::now();
        let r = record_with("STORY-347", resolved("answer"), Some(resolved("answer")), t);
        write_calibration(dir.path(), &r).unwrap();
        let read = read_calibration(dir.path(), &r.punt_id).expect("roundtrip");
        assert_eq!(read, r);
        // The on-disk path matches the documented layout.
        let path = calibration_path(dir.path(), &r.punt_id);
        assert!(path.ends_with(format!("punts/{}/calibration.yaml", r.punt_id)));
    }

    #[test]
    fn read_all_calibrations_returns_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let older = DateTime::parse_from_rfc3339("2026-05-19T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let newer = DateTime::parse_from_rfc3339("2026-05-21T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        write_calibration(
            dir.path(),
            &record_with("STORY-1", resolved("a"), Some(resolved("a")), older),
        )
        .unwrap();
        write_calibration(
            dir.path(),
            &record_with("STORY-2", resolved("b"), Some(resolved("c")), newer),
        )
        .unwrap();
        let all = read_all_calibrations(dir.path());
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].spec, "STORY-2");
        assert_eq!(all[1].spec, "STORY-1");
    }

    #[test]
    fn read_all_calibrations_skips_garbage_files() {
        let dir = tempfile::tempdir().unwrap();
        // A good record.
        write_calibration(
            dir.path(),
            &record_with("STORY-1", resolved("a"), Some(resolved("a")), Utc::now()),
        )
        .unwrap();
        // A directory with a malformed YAML file — must be skipped, not panic.
        std::fs::create_dir_all(dir.path().join(".aida/punts/bogus")).unwrap();
        std::fs::write(
            dir.path().join(".aida/punts/bogus/calibration.yaml"),
            "not: valid: yaml: at: all:",
        )
        .unwrap();
        let all = read_all_calibrations(dir.path());
        assert_eq!(all.len(), 1, "garbage file is skipped");
        assert_eq!(all[0].spec, "STORY-1");
    }

    #[test]
    fn annotate_preserves_existing_fields() {
        let dir = tempfile::tempdir().unwrap();
        let t = Utc::now();
        let r = record_with(
            "STORY-347",
            resolved("use OAuth"),
            Some(resolved("use sessions")),
            t,
        );
        write_calibration(dir.path(), &r).unwrap();
        annotate_calibration(dir.path(), &r.punt_id, "gap → wrote memory feedback_oauth").unwrap();
        let read = read_calibration(dir.path(), &r.punt_id).unwrap();
        assert_eq!(read.spec, "STORY-347");
        assert_eq!(read.cold_boot.answer.as_deref(), Some("use OAuth"));
        assert_eq!(
            read.fork.as_ref().unwrap().answer.as_deref(),
            Some("use sessions")
        );
        assert_eq!(
            read.annotation.as_deref(),
            Some("gap → wrote memory feedback_oauth")
        );
    }

    #[test]
    fn filter_by_bucket_partitions_records() {
        let t = Utc::now();
        let agree = record_with("STORY-A", resolved("x"), Some(resolved("x")), t);
        let disagree = record_with("STORY-B", resolved("y"), Some(resolved("z")), t);
        let no_fork = record_with("STORY-C", resolved("w"), None, t);
        let all = vec![agree.clone(), disagree.clone(), no_fork.clone()];

        let agreements = filter(
            &all,
            &CalibrationFilter {
                bucket: Some(AgreementBucket::Agreement),
                ..Default::default()
            },
        );
        assert_eq!(agreements.len(), 1);
        assert_eq!(agreements[0].spec, "STORY-A");

        let disagreements = filter(
            &all,
            &CalibrationFilter {
                bucket: Some(AgreementBucket::Disagreement),
                ..Default::default()
            },
        );
        assert_eq!(disagreements.len(), 1);
        assert_eq!(disagreements[0].spec, "STORY-B");

        let no_forks = filter(
            &all,
            &CalibrationFilter {
                bucket: Some(AgreementBucket::NoFork),
                ..Default::default()
            },
        );
        assert_eq!(no_forks.len(), 1);
        assert_eq!(no_forks[0].spec, "STORY-C");
    }

    #[test]
    fn filter_by_since_window() {
        let now = Utc::now();
        let recent = record_with(
            "STORY-NEW",
            resolved("x"),
            Some(resolved("x")),
            now - ChronoDuration::hours(2),
        );
        let old = record_with(
            "STORY-OLD",
            resolved("y"),
            Some(resolved("y")),
            now - ChronoDuration::days(30),
        );
        let all = vec![recent.clone(), old.clone()];
        let last_day = filter(
            &all,
            &CalibrationFilter {
                since: Some(ChronoDuration::days(1)),
                ..Default::default()
            },
        );
        assert_eq!(last_day.len(), 1);
        assert_eq!(last_day[0].spec, "STORY-NEW");
    }

    #[test]
    fn parse_since_accepts_units() {
        assert_eq!(parse_since("7d").unwrap(), ChronoDuration::days(7));
        assert_eq!(parse_since("12h").unwrap(), ChronoDuration::hours(12));
        assert_eq!(parse_since("2w").unwrap(), ChronoDuration::weeks(2));
        assert_eq!(parse_since("30m").unwrap(), ChronoDuration::minutes(30));
        assert!(parse_since("xy").is_err());
        assert!(parse_since("7y").is_err());
        assert!(parse_since("").is_err());
    }

    #[test]
    fn compute_stats_counts_agreement_and_no_fork() {
        let now = Utc::now();
        let records = vec![
            record_with("STORY-1", resolved("a"), Some(resolved("a")), now),
            record_with(
                "STORY-2",
                resolved("b"),
                Some(resolved("c")),
                now - ChronoDuration::hours(1),
            ),
            record_with(
                "STORY-3",
                resolved("d"),
                None,
                now - ChronoDuration::hours(2),
            ),
            record_with(
                "STORY-4",
                resolved("e"),
                Some(resolved("e")),
                now - ChronoDuration::days(3),
            ),
        ];
        let stats = compute_stats(&records, 50, now);
        assert_eq!(stats.considered, 4);
        assert_eq!(stats.paired, 3);
        assert_eq!(stats.agreed, 2);
        assert_eq!(stats.disagreed, 1);
        assert_eq!(stats.no_fork, 1);
        // First weekly bucket (most recent week) catches three (now, -1h, -2h)
        // — all paired.
        assert_eq!(stats.weekly[0].paired, 2);
        assert_eq!(stats.weekly[0].agreed, 1);
    }

    #[test]
    fn compute_stats_respects_last_n_window() {
        let now = Utc::now();
        let mut records = Vec::new();
        for i in 0..10 {
            records.push(record_with(
                &format!("STORY-{i}"),
                resolved("a"),
                Some(resolved("a")),
                now - ChronoDuration::hours(i),
            ));
        }
        // Records are newest-first by `read_all_calibrations` convention; here
        // they happen to be timestamped that way already.
        let stats = compute_stats(&records, 3, now);
        assert_eq!(stats.considered, 3);
        assert_eq!(stats.paired, 3);
        assert_eq!(stats.agreed, 3);
    }

    #[test]
    fn from_response_maps_punt_resolution_to_string() {
        let resp = PuntResponse {
            resolution: PuntResolution::Resolved,
            answer: Some("ship".to_string()),
            reasoning: "the recorded convention".to_string(),
            classification: Some("A".to_string()),
            escalation_reason: None,
        };
        let v = CalibrationVerdict::from_response(&resp, Some("logs/advise.jsonl".into()));
        assert_eq!(v.resolution, "resolved");
        assert_eq!(v.answer.as_deref(), Some("ship"));
        assert_eq!(v.log_path.as_deref(), Some("logs/advise.jsonl"));

        let resp = PuntResponse {
            resolution: PuntResolution::Escalated,
            answer: None,
            reasoning: "ask a human".to_string(),
            classification: None,
            escalation_reason: Some("strategy".to_string()),
        };
        let v = CalibrationVerdict::from_response(&resp, None);
        assert_eq!(v.resolution, "escalated");
        assert!(v.answer.is_none());
        assert_eq!(v.escalation_reason.as_deref(), Some("strategy"));
    }
}
