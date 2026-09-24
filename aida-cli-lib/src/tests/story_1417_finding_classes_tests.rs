//! STORY-1417: typed finding classes — vocabulary, positional pairing,
//! backward-compatible storage, and the corpus count.
// trace:STORY-1417 | ai:claude

use super::*;
use crate::review_verdict::{build_verdict_object, write_verdict_object};
use tempfile::TempDir;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn record(
    root: &Path,
    key: &str,
    sha: &str,
    by: &str,
    findings: &[String],
    classes: &[String],
) -> serde_json::Map<String, serde_json::Value> {
    let path = crate::review_verdict::verdict_path(root, key);
    let mut obj = build_verdict_object(
        root,
        &path,
        Some("request-changes"),
        Some(sha),
        Some("b"),
        None,
        findings,
        by,
    )
    .unwrap();
    let (resolved, _) = resolve_finding_classes(findings, classes);
    apply_finding_classes(&mut obj, findings, &resolved);
    write_verdict_object(&path, &obj).unwrap();
    obj
}

#[test]
fn class_names_normalize_and_aliases_resolve() {
    assert_eq!(parse_class("Fail_Open"), ClassArg::Class("fail-open"));
    assert_eq!(
        parse_class("missing-test"),
        ClassArg::Class("untested-path")
    );
    assert_eq!(
        parse_class("contract-break"),
        ClassArg::Class("contract-drift")
    );
    assert_eq!(parse_class("-"), ClassArg::Skip);
    assert_eq!(parse_class("none"), ClassArg::Skip);
    assert_eq!(parse_class("vibes"), ClassArg::Unknown("vibes".to_string()));
    for (name, _) in FINDING_CLASSES {
        assert_eq!(parse_class(name), ClassArg::Class(name));
    }
}

#[test]
fn classes_pair_by_position_and_never_fail() {
    let (classes, warnings) =
        resolve_finding_classes(&s(&["a", "b", "c"]), &s(&["race", "-", "bogus", "perf"]));
    assert_eq!(classes, vec![Some("race".to_string()), None, None]);
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings[0].contains("bogus"));
    assert!(warnings[1].contains("perf"));
}

#[test]
fn classes_are_stored_alongside_findings_and_old_files_parse() {
    let dir = TempDir::new().unwrap();
    let obj = record(
        dir.path(),
        "TASK-1",
        "abcdef1234567",
        "reviewer",
        &s(&["left two sites", "no test"]),
        &s(&["incomplete-fix", "untested-path"]),
    );
    assert_eq!(obj["findings"].as_array().unwrap().len(), 2);
    assert_eq!(
        obj["finding_classes"],
        serde_json::json!(["incomplete-fix", "untested-path"])
    );
    // The existing reader is unaffected by the new field.
    let body =
        std::fs::read_to_string(dir.path().join(".aida/review-verdicts/TASK-1.json")).unwrap();
    let v = crate::review_verdict::parse_recorded_verdict(&body).unwrap();
    assert_eq!(v.findings.len(), 2);

    // Unclassified findings write no `finding_classes` at all, and new
    // findings drop stale classes (positional alignment would be wrong).
    let obj = record(
        dir.path(),
        "TASK-1",
        "abcdef1234568",
        "reviewer",
        &s(&["something else"]),
        &[],
    );
    assert!(!obj.contains_key("finding_classes"));
}

#[test]
fn report_counts_across_current_archives_and_rounds_once_each() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    // Pre-schema file: findings, no classes — counted as unclassified.
    let vd = root.join(".aida/review-verdicts");
    std::fs::create_dir_all(&vd).unwrap();
    std::fs::write(
        vd.join("OLD-1.json"),
        r#"{"verdict":"approved","findings":["covers all three sites"],"recorded_at":"2026-01-01T00:00:00Z"}"#,
    )
    .unwrap();
    record(
        root,
        "TASK-1",
        "abcdef1234567",
        "r1",
        &s(&["left a sibling"]),
        &s(&["incomplete-fix"]),
    );
    // A second round at a new sha retains the first in `rounds` and in the
    // per-sha archive; neither copy may be counted twice.
    record(
        root,
        "TASK-1",
        "abcdef7654321",
        "r1",
        &s(&["still left a sibling", "unwrap on error"]),
        &s(&["incomplete-fix", "fail-open"]),
    );
    record(
        root,
        "TASK-2",
        "1234567abcdef",
        "r2",
        &s(&["fails open on missing file"]),
        &s(&["fail-open"]),
    );

    let report = collect_class_report(&vd, None);
    let get = |c: &str| report.classes.iter().find(|x| x.class == c).cloned();
    let inc = get("incomplete-fix").unwrap();
    assert_eq!(inc.findings, 2);
    assert_eq!(inc.subjects, vec!["TASK-1".to_string()]);
    let fo = get("fail-open").unwrap();
    assert_eq!(fo.findings, 2);
    assert_eq!(fo.subjects, s(&["TASK-1", "TASK-2"]));
    assert_eq!(report.classified, 4);
    assert_eq!(report.unclassified, 1);

    // `--since` in the future excludes everything.
    let future = chrono::Utc::now() + chrono::Duration::days(1);
    let empty = collect_class_report(&vd, Some(future));
    assert!(empty.classes.is_empty());
    assert_eq!(empty.classified + empty.unclassified, 0);
}

#[test]
fn report_on_missing_dir_is_empty() {
    let dir = TempDir::new().unwrap();
    let r = collect_class_report(&dir.path().join("nope"), None);
    assert_eq!(r, ClassReport::default());
}
