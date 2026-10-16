//! Read-only reporting from committed trial evidence, with diagnostic gap trends.
// trace:TASK-1733 | ai:codex
use super::*;
use std::fmt::Write;

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
    let mut out = match latest.discriminating.as_ref().filter(|a| {
        latest.status == "scored"
            && a.total > 0
            && a.passed <= a.total
            && a.pass_rate == a.passed as f64 / a.total as f64
    }) {
        Some(a) => format!(
            "Discriminating oracle pass rate (R1): {:.1}% ({}/{}), age {}s",
            a.pass_rate * 100.0,
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

    fn trial(id: u128, seconds: i64) -> TrialRecord {
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
