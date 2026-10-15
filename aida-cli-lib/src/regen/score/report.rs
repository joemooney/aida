//! Read-only reporting from committed trial evidence, with diagnostic gap trends.
// trace:TASK-1733 | ai:codex
use super::*;
use std::fmt::Write;
mod cache; // trace:TASK-1734 | ai:codex

/// Latest scored ordinary R1, shared by the statusline and every digest format.
// trace:TASK-1734 | ai:codex
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegenerationSummary {
    pub trial_id: Uuid,
    pub completed_at: DateTime<Utc>,
    pub passed: usize,
    pub total: usize,
    pub pass_rate: f64,
    pub age_seconds: i64,
    pub clean: bool,
    pub flags: Vec<String>,
}

impl RegenerationSummary {
    pub(crate) fn text(&self) -> String {
        format!(
            "regen {:.1}% age {}{}",
            self.pass_rate * 100.0,
            compact_age(self.age_seconds),
            if !self.clean || !self.flags.is_empty() {
                " !"
            } else {
                ""
            }
        )
    }
}

fn compact_age(seconds: i64) -> String {
    let seconds = seconds.max(0);
    for (unit, suffix) in [(86400, "d"), (3600, "h"), (60, "m")] {
        if seconds >= unit {
            return format!("{}{suffix}", seconds / unit);
        }
    }
    format!("{seconds}s")
}

// JSON round trips can move a finite ratio by a few ulps. This display-only
// tolerance never feeds controls or eligibility; render the integer ratio.
fn valid_rate(passed: usize, total: usize, rate: f64) -> bool {
    total > 0
        && passed <= total
        && rate.is_finite()
        && (rate - passed as f64 / total as f64).abs() <= 8.0 * f64::EPSILON
}

fn scored_aggregate(record: &TrialRecord) -> Option<&Aggregate> {
    record
        .discriminating
        .as_ref()
        .filter(|a| record.status == "scored" && valid_rate(a.passed, a.total, a.pass_rate))
}

fn summary(records: &[TrialRecord], now: DateTime<Utc>) -> Option<RegenerationSummary> {
    let latest = records
        .iter()
        .filter(|r| {
            r.format_version == 1
                && r.kind == "trial"
                && r.completed_at <= now
                && scored_aggregate(r).is_some()
        })
        .max_by_key(|r| (r.completed_at, r.trial_id))?;
    let aggregate = scored_aggregate(latest)?;
    Some(RegenerationSummary {
        trial_id: latest.trial_id,
        completed_at: latest.completed_at,
        passed: aggregate.passed,
        total: aggregate.total,
        pass_rate: aggregate.passed as f64 / aggregate.total as f64,
        age_seconds: now
            .signed_duration_since(latest.completed_at)
            .num_seconds()
            .max(0),
        clean: latest.clean,
        flags: latest.flags.clone(),
    })
}

pub(crate) fn read_summary_for_project(
    project: &Path,
    now: DateTime<Utc>,
) -> Option<RegenerationSummary> {
    // Optional display only: local committed evidence and a derived local
    // summary cache; no canonical writes, rebuild, fetch or authority decisions.
    let store = cache::display_store(project)?;
    cache::read(&store, &cache::path(&store)?, now)
}

pub(crate) fn read_report(store: &Path) -> Result<String> {
    Ok(render(&controls::read_records(store)?, Utc::now()))
}

fn render(records: &[TrialRecord], now: DateTime<Utc>) -> String {
    let mut trials: Vec<_> = records
        .iter()
        .filter(|r| r.format_version == 1 && r.kind == "trial")
        .collect();
    trials.sort_by_key(|r| std::cmp::Reverse((r.completed_at, r.trial_id)));
    let Some(&latest) = trials.first() else {
        return "Discriminating oracle pass rate: none (no ordinary trial recorded)\nGap diagnostics: no gap data".into();
    };
    let age = now
        .signed_duration_since(latest.completed_at)
        .num_seconds()
        .max(0);
    let mut out = match scored_aggregate(latest) {
        Some(a) => format!(
            "Discriminating oracle pass rate (R1): {:.1}% ({}/{}), age {}s",
            a.passed as f64 / a.total as f64 * 100.0,
            a.passed,
            a.total,
            age
        ),
        None => format!("Discriminating oracle pass rate: unavailable, age {age}s"),
    };
    // Make unsuccessful/contaminated attempts visible; never fall back to an
    // older passing trial or treat an adapter score as the raw headline.
    let _ = write!(
        out,
        "\nTrial: {}; status {}; clean {}",
        latest.trial_id,
        latest.status.escape_debug(),
        latest.clean
    );
    if !latest.flags.is_empty() {
        let _ = write!(
            out,
            "; flags {}",
            latest
                .flags
                .iter()
                .map(|f| f.escape_debug().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let previous = trials.into_iter().skip(1).find(|r| {
        r.oracle_sha == latest.oracle_sha
            && r.oracle_bundle_sha256 == latest.oracle_bundle_sha256
            && r.content_sha256 == latest.content_sha256
    });
    let current_counts = latest.gaps.as_ref().and_then(adapter::gap_counts);
    let Some(current_counts) = current_counts else {
        out.push_str("\nGap diagnostics: no gap data (missing or invalid metadata)");
        return out;
    };
    out.push_str("\nGap diagnostics (category counts; not a quality score):");
    let prior_counts = previous
        .and_then(|r| r.gaps.as_ref())
        .and_then(adapter::gap_counts);
    for (i, category) in ["interface", "missing", "behavior", "other"]
        .iter()
        .enumerate()
    {
        let _ = write!(out, "\n  {category}: {}", current_counts[i]);
        if let Some(prior) = prior_counts {
            let delta = current_counts[i] as i128 - prior[i] as i128;
            let _ = write!(out, " (delta {delta:+})");
        }
    }
    match (previous, prior_counts) {
        (Some(prior), Some(_)) => {
            let _ = write!(
                out,
                "\nDiagnostic deltas versus {} (matching oracle, bundle and corpus)",
                prior.trial_id
            );
        }
        (Some(_), None) => {
            out.push_str("\nDiagnostic deltas unavailable: no gap data for previous matching trial")
        }
        (None, _) => out.push_str("\nDiagnostic deltas unavailable: no previous matching trial"),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub(super) fn trial(id: u128, seconds: i64) -> TrialRecord {
        serde_json::from_value(json!({
            "format_version": 1, "trial_id": Uuid::from_u128(id),
            "content_sha256": "corpus", "store_sha": "store",
            "oracle_sha": "oracle", "oracle_bundle_sha256": "bundle",
            "model": "model", "vendor": "vendor", "harness": "harness",
            "started_at": DateTime::from_timestamp(seconds, 0).unwrap(),
            "completed_at": DateTime::from_timestamp(seconds, 0).unwrap(),
            "checks": [{"name": "check", "passed": true}],
            "aggregate": {"passed": 10, "total": 10, "pass_rate": 1.0},
            "discriminating": {"passed": 3, "total": 4, "pass_rate": 0.75},
            "install_exit": 0, "oracle_exit": 0, "status": "scored",
            "clean": true, "guard_hits": [], "unconfined": false,
            "flags": [], "error": null
        }))
        .unwrap()
    }

    fn gaps(rows: &str) -> serde_json::Value {
        let root = tempfile::TempDir::new().unwrap();
        std::fs::write(
            root.path().join("GAPS.toml"),
            format!("format_version=1\n{rows}"),
        )
        .unwrap();
        let captured = adapter::CapturedAdapter::load(root.path())
            .unwrap()
            .unwrap();
        // Persisted metadata survives disappearance of the original input.
        drop(root);
        captured.gap_snapshot().unwrap()
    }

    // trace:TASK-1734 | ai:codex
    #[test]
    fn regeneration_summary_selects_scored_ordinary_by_completion_then_uuid() {
        let now = DateTime::from_timestamp(200, 0).unwrap();
        assert!(summary(&[], now).is_none());
        let old = trial(1, 100);
        let mut latest = trial(3, 150);
        latest.clean = false;
        latest.flags = vec!["oracle_nonzero_exit".into()];
        let mut tied = trial(2, 150);
        tied.discriminating.as_mut().unwrap().pass_rate = 0.0; // invalid aggregate
        let mut control = trial(4, 190);
        control.kind = "control-reference".into();
        let mut adapted = trial(5, 195);
        adapted.kind = "trial-adapted".into();
        let mut failed = trial(6, 199);
        failed.status = "error".into();
        let selected = summary(&[old, latest, tied, control, adapted, failed], now).unwrap();
        assert_eq!(selected.trial_id, Uuid::from_u128(3));
        assert_eq!(selected.text(), "regen 75.0% age 50s !");
        let selected = summary(&[trial(9, 150), trial(8, 150)], now).unwrap();
        assert_eq!(selected.trial_id, Uuid::from_u128(9));
        assert!(summary(&[trial(1, 250)], now).is_none());
    }

    #[test]
    fn regeneration_age_is_compact_and_future_cutoffs_select_only_completed_trials() {
        for (seconds, expected) in [
            (0, "0s"),
            (59, "59s"),
            (60, "1m"),
            (12 * 60 + 5, "12m"),
            (3 * 3600 + 40, "3h"),
            (14 * 86400 + 23, "14d"),
        ] {
            assert_eq!(compact_age(seconds), expected);
        }
        let rows = [trial(1, 100), trial(2, 150), trial(3, 250)];
        assert_eq!(
            summary(&rows, DateTime::from_timestamp(120, 0).unwrap())
                .unwrap()
                .trial_id,
            Uuid::from_u128(1)
        );
        assert_eq!(
            summary(&rows, DateTime::from_timestamp(200, 0).unwrap())
                .unwrap()
                .trial_id,
            Uuid::from_u128(2)
        );
        assert!(summary(&rows, DateTime::from_timestamp(99, 0).unwrap()).is_none());
    }

    // Both display paths use committed evidence and omit absent/corrupt data.
    // trace:TASK-1734 | ai:codex
    #[test]
    fn regeneration_statusline_and_digest_cover_absent_flagged_and_invalid_records() {
        use crate::digest::{DigestAudience, DigestFormat, DigestOptions};
        let project = tempfile::tempdir().unwrap();
        let git = |root: &Path, args: &[&str]| super::super::super::git(root, args).unwrap();
        git(project.path(), &["init", "-b", "main"]);
        git(project.path(), &["config", "user.name", "Test"]);
        git(
            project.path(),
            &["config", "user.email", "test@example.com"],
        );
        std::fs::create_dir(project.path().join(".aida")).unwrap();
        std::fs::write(
            project.path().join(".aida/config.toml"),
            "[deployment]\nmode='distributed'\nstore_path='.aida-store'\n",
        )
        .unwrap();
        git(project.path(), &["add", "."]);
        git(project.path(), &["commit", "-m", "project"]);
        let store = project.path().join(".aida-store");
        std::fs::create_dir(&store).unwrap();
        git(&store, &["init", "-b", "aida-store"]);
        git(&store, &["config", "user.name", "Test"]);
        git(&store, &["config", "user.email", "test@example.com"]);
        git(&store, &["commit", "--allow-empty", "-m", "store"]);
        let now = DateTime::from_timestamp(200, 0).unwrap();
        let requirements = aida_core::RequirementsStore::default();
        let check_surfaces = |expected: Option<&str>, clean: bool, flags: &[&str]| {
            assert_eq!(
                crate::statusline_cmd::regeneration_statusline_segment(project.path(), now)
                    .as_deref(),
                expected
            );
            for audience in [
                DigestAudience::Customer,
                DigestAudience::Team,
                DigestAudience::Slf,
                DigestAudience::Operator,
            ] {
                for format in [
                    DigestFormat::Markdown,
                    DigestFormat::Plain,
                    DigestFormat::Brief,
                    DigestFormat::Json,
                ] {
                    let opts = DigestOptions {
                        since: DateTime::from_timestamp(0, 0).unwrap(),
                        until: now,
                        audience,
                        format,
                        include_next: false,
                        include_process: false,
                        out: None,
                    };
                    let out =
                        crate::digest::render_string(&opts, project.path(), &requirements).unwrap();
                    if format == DigestFormat::Json {
                        let json: serde_json::Value = serde_json::from_str(&out).unwrap();
                        if expected.is_some() {
                            assert_eq!(json["regeneration"]["pass_rate"], 0.75);
                            assert_eq!(json["regeneration"]["age_seconds"], 50);
                            assert_eq!(json["regeneration"]["clean"], clean);
                            assert_eq!(json["regeneration"]["flags"], json!(flags));
                        } else {
                            assert!(json.get("regeneration").is_none(), "{out}");
                        }
                    } else if let Some(text) = expected {
                        assert!(out.contains(text), "{out}");
                    } else {
                        assert!(!out.contains("regen"), "{out}");
                    }
                }
            }
        };
        check_surfaces(None, true, &[]);
        std::fs::create_dir_all(store.join("regen/trials")).unwrap();
        let path = store.join(format!("regen/trials/{}.json", Uuid::from_u128(1)));
        for (clean, flags, text) in [
            (false, vec![], "regen 75.0% age 50s !"),
            (true, vec!["oracle_nonzero_exit"], "regen 75.0% age 50s !"),
            (true, vec![], "regen 75.0% age 50s"),
        ] {
            let mut row = trial(1, 150);
            row.clean = clean;
            row.flags = flags.iter().map(|f| (*f).to_owned()).collect();
            std::fs::write(&path, serde_json::to_vec(&row).unwrap()).unwrap();
            git(&store, &["add", "."]);
            git(&store, &["commit", "-m", "scored trial"]);
            std::fs::write(&path, "dirty invalid JSON").unwrap();
            check_surfaces(Some(text), clean, &flags);
        }
        git(&store, &["add", "."]);
        git(&store, &["commit", "-m", "invalid committed record"]);
        check_surfaces(None, true, &[]);
        assert!(!project.path().join(".aida/cache.db").exists());
    }

    #[test]
    fn report_headline_and_category_deltas_use_latest_matching_ordinary_pair() {
        let now = DateTime::from_timestamp(200, 0).unwrap();
        let mut old = trial(1, 100);
        old.gaps = Some(gaps(
            "[[gaps]]\nid='rename'\ncategory='interface'\nnote='plain $text'\n",
        ));
        let mut current = trial(2, 150);
        current.gaps = Some(gaps("[[gaps]]\nid='new'\ncategory='missing'\n"));
        let mut unrelated = trial(3, 140);
        unrelated.oracle_bundle_sha256 = "different".into();
        unrelated.gaps = Some(gaps(""));
        let mut control = trial(4, 190);
        control.kind = "control-reference".into();
        let mut nested = trial(5, 195);
        nested.kind = "trial-adapted".into();
        let out = render(&[current, control, unrelated, old, nested], now);
        assert_eq!(out, concat!(
            "Discriminating oracle pass rate (R1): 75.0% (3/4), age 50s\n",
            "Trial: 00000000-0000-0000-0000-000000000002; status scored; clean true\n",
            "Gap diagnostics (category counts; not a quality score):\n",
            "  interface: 0 (delta -1)\n  missing: 1 (delta +1)\n",
            "  behavior: 0 (delta +0)\n  other: 0 (delta +0)\n",
            "Diagnostic deltas versus 00000000-0000-0000-0000-000000000001 (matching oracle, bundle and corpus)"
        ));
    }

    #[test]
    fn report_missing_invalid_and_empty_gap_data_are_distinct_from_zero_inference() {
        let now = Utc::now();
        assert!(render(&[], now).contains("none (no ordinary trial recorded)"));
        let invalids = [
            None,
            Some(json!({})),
            Some(json!({
                "format_version": 2, "input_sha256": "a".repeat(64), "rows": []
            })),
            Some(json!({
                "format_version": 1, "input_sha256": "a".repeat(64),
                "rows": [{"id": "Bad", "category": "interface"}]
            })),
            Some(json!({
                "format_version": 1, "input_sha256": "a".repeat(64),
                "rows": [{"id": "good", "category": "guessed"}]
            })),
        ];
        for metadata in invalids {
            let mut row = trial(1, 100);
            row.gaps = metadata;
            let out = render(&[row], now);
            assert!(out.contains("75.0%"));
            assert!(out.contains("no gap data"));
            assert!(!out.contains("interface: 0"));
        }
        let mut empty = trial(2, 150);
        empty.gaps = Some(gaps(""));
        let out = render(&[trial(1, 100), empty], now);
        assert!(out.contains("interface: 0"));
        assert!(out.contains("no gap data for previous matching trial"));
        assert!(!out.contains("delta +0"));
    }

    #[test]
    fn report_does_not_skip_failed_latest_or_hide_contamination() {
        let mut latest = trial(2, 150);
        latest.status = "adapter_refused".into();
        latest.clean = false;
        latest.flags = vec!["guard_hits".into()];
        let out = render(&[trial(1, 100), latest], Utc::now());
        assert!(out.starts_with("Discriminating oracle pass rate: unavailable"));
        assert!(out.contains("status adapter_refused; clean false; flags guard_hits"));
        assert!(!out.contains("75.0%"));
        let mut zero = trial(1, 100);
        zero.discriminating.as_mut().unwrap().total = 0;
        assert!(render(&[zero], Utc::now()).contains("unavailable"));
    }

    #[test]
    fn report_provenance_matches_each_hash_and_never_skips_missing_previous_data() {
        let now = Utc::now();
        for mismatch in 0..3 {
            let mut earlier = trial(1, 100);
            match mismatch {
                0 => earlier.oracle_sha = "different".into(),
                1 => earlier.oracle_bundle_sha256 = "different".into(),
                _ => earlier.content_sha256 = "different".into(),
            }
            let mut latest = trial(2, 150);
            latest.gaps = Some(gaps(""));
            assert!(render(&[earlier, latest], now).contains("no previous matching trial"));
        }
        let mut oldest = trial(1, 90);
        oldest.gaps = Some(gaps(""));
        let mut latest = trial(3, 150);
        latest.gaps = Some(gaps(""));
        let out = render(&[oldest, trial(2, 100), latest], now);
        assert!(out.contains("no gap data for previous matching trial"));
        assert!(!out.contains("delta +0"));
    }

    #[test]
    fn report_reads_only_committed_evidence_and_does_not_require_caller_paths_or_cache() {
        let store = tempfile::TempDir::new().unwrap();
        super::super::super::git(store.path(), &["init"]).unwrap();
        super::super::super::git(store.path(), &["config", "user.name", "Test"]).unwrap();
        super::super::super::git(store.path(), &["config", "user.email", "test@example.com"])
            .unwrap();
        super::super::super::git(store.path(), &["commit", "--allow-empty", "-m", "fixture"])
            .unwrap();
        let mut row = trial(1, 100);
        row.gaps = Some(gaps("[[gaps]]\nid='one'\ncategory='behavior'\n"));
        super::super::persist(store.path(), &row).unwrap();
        // Dirty edits and untracked records must not alter the report.
        let path = store
            .path()
            .join(format!("regen/trials/{}.json", row.trial_id));
        std::fs::write(&path, "invalid dirty JSON").unwrap();
        std::fs::write(path.with_file_name("untracked.json"), "{}").unwrap();
        let out = read_report(store.path()).unwrap();
        assert!(out.contains("75.0%"));
        assert!(out.contains("behavior: 1"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "invalid dirty JSON");
    }
}
