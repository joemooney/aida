//! Tests for TASK-1522: Intent-capture effort target.
//!
//! Report the criterion-to-test share of newly completed specs against a
//! configurable floor (default 50%).
//! Advisory and report-only: does not block merge, drain, or completion.
//!
//! trace:TASK-1522 | ai:antigravity

use super::*;
use crate::intent_capture::*;
use aida_core::models::{RequirementStatus, RequirementType};
use aida_core::{Requirement, RequirementsStore};
use chrono::{Duration, Utc};
use std::path::Path;

fn spec(id: &str, req_type: RequirementType, description: &str, days_ago: i64) -> Requirement {
    let mut r = Requirement::new(format!("{id} title"), description.to_string());
    r.spec_id = Some(id.to_string());
    r.req_type = req_type;
    r.created_at = Utc::now() - Duration::days(days_ago);
    r
}

fn store_with(reqs: Vec<Requirement>) -> RequirementsStore {
    let mut store = RequirementsStore::new();
    store.requirements = reqs;
    store
}

fn tokens(criterion: &[&str], comments: usize) -> TraceTokens {
    TraceTokens {
        criterion_tokens: criterion.iter().map(|s| s.to_ascii_uppercase()).collect(),
        test_criterion_tokens: criterion.iter().map(|s| s.to_ascii_uppercase()).collect(),
        trace_comments: comments,
        source: "fixture".to_string(),
    }
}

fn git(root: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .env("HOME", root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn init_repo(root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.email", "t@example.invalid"]);
    git(root, &["config", "user.name", "t"]);
    git(root, &["config", "commit.gpgsign", "false"]);
}

fn commit_file(root: &Path, rel: &str, body: &str, subject: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, body).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", subject]);
}

// --- intent_share_counts_only_completed_work_specs_in_window ------------------

#[test]
fn test_intent_share_counts_only_completed_work_specs_in_window() {
    let now = Utc::now();

    // 1. completed 10 d ago traced -> 1/1
    let mut s1 = spec(
        "TASK-1",
        RequirementType::Task,
        "## Acceptance\n- A1. do x\n",
        15,
    );
    s1.status = RequirementStatus::Completed;
    s1.implementation_info = Some(aida_core::ImplementationInfo {
        completed_at: Some(now - Duration::days(10)),
        ..Default::default()
    });

    // 2. completed 10 d ago untraced -> 0/1
    let mut s2 = spec(
        "TASK-2",
        RequirementType::Task,
        "## Acceptance\n- A1. do y\n",
        15,
    );
    s2.status = RequirementStatus::Completed;
    s2.implementation_info = Some(aida_core::ImplementationInfo {
        completed_at: Some(now - Duration::days(10)),
        ..Default::default()
    });

    // 3. completed 60 d ago traced -> outside 30-day window -> excluded
    let mut s3 = spec(
        "STORY-3",
        RequirementType::Story,
        "## Acceptance\n- A1. do z\n",
        70,
    );
    s3.status = RequirementStatus::Completed;
    s3.implementation_info = Some(aida_core::ImplementationInfo {
        completed_at: Some(now - Duration::days(60)),
        ..Default::default()
    });

    // 4. auto-drafted BUG completed 10 d ago -> machine filed -> excluded
    let mut s4 = spec(
        "BUG-4",
        RequirementType::Bug,
        "Auto-drafted by `aida queue work`: failure\n## Acceptance\n- A1. fix\n",
        15,
    );
    s4.status = RequirementStatus::Completed;
    s4.implementation_info = Some(aida_core::ImplementationInfo {
        completed_at: Some(now - Duration::days(10)),
        ..Default::default()
    });

    // 5. in-progress spec with criteria -> not completed -> excluded
    let s5 = spec(
        "TASK-5",
        RequirementType::Task,
        "## Acceptance\n- A1. in progress\n",
        5,
    );

    // 6. Epic completed 10 d ago traced -> not story/task/bug -> excluded
    let mut s6 = spec(
        "EPIC-6",
        RequirementType::Epic,
        "## Acceptance\n- A1. big\n",
        15,
    );
    s6.status = RequirementStatus::Completed;
    s6.implementation_info = Some(aida_core::ImplementationInfo {
        completed_at: Some(now - Duration::days(10)),
        ..Default::default()
    });

    let store = store_with(vec![s1, s2, s3, s4, s5, s6]);
    let toks = tokens(&["TASK-1.A1", "STORY-3.A1", "EPIC-6.A1"], 3);

    let report = coverage_from_parts(&store, now, 90, &[], &[], &toks, None);
    let ic = report
        .intent_capture
        .expect("intent_capture must be present");

    assert_eq!(ic.window_days, 30);
    assert_eq!(ic.specs_with_criteria, 2);
    assert_eq!(ic.specs_with_traced_criterion, 1);
    assert_eq!(ic.floor_pct, 50);
    assert_eq!(ic.verdict, IntentCaptureVerdict::AtOrAboveFloor);
    assert!((ic.share_pct.unwrap() - 50.0).abs() < f64::EPSILON);
    assert_eq!(
        ic.render_report_line(),
        "Intent capture (specs completed, last 30 d): 1 of 2 (50%) vs floor 50%: at or above floor"
    );
}

// --- capture floor config tests (acceptance 2) ---------------------------------

#[test]
fn test_capture_floor_default_is_50_when_unset() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    assert_eq!(intent_capture_floor_pct(root), 50);

    // Config exists but has no [capture]
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[deployment]\nmode = \"distributed\"\n",
    )
    .unwrap();
    assert_eq!(intent_capture_floor_pct(root), 50);
}

#[test]
fn test_capture_floor_reads_config_and_changes_verdict() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida")).unwrap();

    // Floor 20%
    std::fs::write(
        root.join(".aida/config.toml"),
        "[capture]\ncriterion_test_floor_pct = 20\n",
    )
    .unwrap();
    assert_eq!(intent_capture_floor_pct(root), 20);

    let now = Utc::now();
    let specs = vec![
        SpecCoverage {
            spec_id: "TASK-1".into(),
            spec_type: RequirementType::Task,
            status: "Completed".into(),
            created_at: now - Duration::days(5),
            completed_at: Some(now - Duration::days(5)),
            criteria: 1,
            traced_criteria: 1,
        },
        SpecCoverage {
            spec_id: "TASK-2".into(),
            spec_type: RequirementType::Task,
            status: "Completed".into(),
            created_at: now - Duration::days(5),
            completed_at: Some(now - Duration::days(5)),
            criteria: 1,
            traced_criteria: 0,
        },
        SpecCoverage {
            spec_id: "TASK-3".into(),
            spec_type: RequirementType::Task,
            status: "Completed".into(),
            created_at: now - Duration::days(5),
            completed_at: Some(now - Duration::days(5)),
            criteria: 1,
            traced_criteria: 0,
        },
        SpecCoverage {
            spec_id: "TASK-4".into(),
            spec_type: RequirementType::Task,
            status: "Completed".into(),
            created_at: now - Duration::days(5),
            completed_at: Some(now - Duration::days(5)),
            criteria: 1,
            traced_criteria: 0,
        },
    ];

    // 1 of 4 = 25%
    // At floor 20%, 25% >= 20% -> AtOrAboveFloor
    let ic_20 = intent_capture_share(&specs, now, 20);
    assert_eq!(ic_20.verdict, IntentCaptureVerdict::AtOrAboveFloor);
    assert_eq!(
        ic_20.render_report_line(),
        "Intent capture (specs completed, last 30 d): 1 of 4 (25%) vs floor 20%: at or above floor"
    );

    // Floor 80%
    std::fs::write(
        root.join(".aida/config.toml"),
        "[capture]\ncriterion_test_floor_pct = 80\n",
    )
    .unwrap();
    assert_eq!(intent_capture_floor_pct(root), 80);

    // At floor 80%, 25% < 80% -> BelowFloor
    let ic_80 = intent_capture_share(&specs, now, 80);
    assert_eq!(ic_80.verdict, IntentCaptureVerdict::BelowFloor);
    assert_eq!(
        ic_80.render_report_line(),
        "Intent capture (specs completed, last 30 d): 1 of 4 (25%) vs floor 80%: below floor"
    );
}

#[test]
fn test_capture_floor_invalid_value_falls_back_to_default() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida")).unwrap();

    // String value "abc"
    std::fs::write(
        root.join(".aida/config.toml"),
        "[capture]\ncriterion_test_floor_pct = \"abc\"\n",
    )
    .unwrap();
    assert_eq!(intent_capture_floor_pct(root), 50);

    // Out of range (150)
    std::fs::write(
        root.join(".aida/config.toml"),
        "[capture]\ncriterion_test_floor_pct = 150\n",
    )
    .unwrap();
    assert_eq!(intent_capture_floor_pct(root), 50);

    // Negative out of range (-5)
    std::fs::write(
        root.join(".aida/config.toml"),
        "[capture]\ncriterion_test_floor_pct = -5\n",
    )
    .unwrap();
    assert_eq!(intent_capture_floor_pct(root), 50);
}

// --- status line tests (acceptance 3) -------------------------------------------

#[test]
fn test_status_prints_intent_line_below_floor_and_nothing_at_or_above() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root);
    std::fs::create_dir_all(root.join(".aida")).unwrap();

    // Commit file with 1 traced test
    commit_file(
        root,
        "tests/t.rs",
        "// trace:TASK-1.A1 | ai:antigravity\n#[test]\nfn t1() {\n    assert!(true);\n}\n",
        "test: traced (TASK-1)",
    );

    let now = Utc::now();
    let mut s1 = spec(
        "TASK-1",
        RequirementType::Task,
        "## Acceptance\n- A1. x\n",
        5,
    );
    s1.status = RequirementStatus::Completed;
    s1.implementation_info = Some(aida_core::ImplementationInfo {
        completed_at: Some(now - Duration::days(5)),
        ..Default::default()
    });

    let mut s2 = spec(
        "TASK-2",
        RequirementType::Task,
        "## Acceptance\n- A1. y\n",
        5,
    );
    s2.status = RequirementStatus::Completed;
    s2.implementation_info = Some(aida_core::ImplementationInfo {
        completed_at: Some(now - Duration::days(5)),
        ..Default::default()
    });

    let mut s3 = spec(
        "TASK-3",
        RequirementType::Task,
        "## Acceptance\n- A1. z\n",
        5,
    );
    s3.status = RequirementStatus::Completed;
    s3.implementation_info = Some(aida_core::ImplementationInfo {
        completed_at: Some(now - Duration::days(5)),
        ..Default::default()
    });

    let mut s4 = spec(
        "TASK-4",
        RequirementType::Task,
        "## Acceptance\n- A1. w\n",
        5,
    );
    s4.status = RequirementStatus::Completed;
    s4.implementation_info = Some(aida_core::ImplementationInfo {
        completed_at: Some(now - Duration::days(5)),
        ..Default::default()
    });

    // 1 traced out of 4 = 25%
    let store = store_with(vec![s1, s2, s3, s4]);

    // Run report to write cache
    handle_criteria_coverage(root, &store, 90, false).unwrap();
    assert!(root.join(CACHE_REL_PATH).exists());

    // Default floor 50%: 25% < 50% -> below floor -> prints line
    let line = status_intent_capture_line(root);
    assert_eq!(
        line,
        Some(
            "  Intent capture: 1 of 4 completed specs (25%) traced to tests; below the 50% floor. Run aida criteria coverage"
                .to_string()
        )
    );

    // Floor 20%: 25% >= 20% -> at or above floor -> prints nothing (None)
    std::fs::write(
        root.join(".aida/config.toml"),
        "[capture]\ncriterion_test_floor_pct = 20\n",
    )
    .unwrap();
    assert_eq!(status_intent_capture_line(root), None);

    // Stale cache (HEAD moved) -> prints nothing (None)
    std::fs::remove_file(root.join(".aida/config.toml")).unwrap();
    commit_file(root, "src/new.rs", "fn f() {}\n", "feat: new file");
    assert_eq!(status_intent_capture_line(root), None);
}

// --- store head cache invalidation ---------------------------------------------

#[test]
fn test_cache_invalidated_when_store_head_moves() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root);
    commit_file(root, "README.md", "# test\n", "initial commit");
    std::fs::create_dir_all(root.join(".aida")).unwrap();

    // Create a mock .aida-store git repository
    let store_dir = root.join(".aida-store");
    init_repo(&store_dir);
    commit_file(
        &store_dir,
        "metadata.yaml",
        "mode: distributed\n",
        "initial store",
    );

    let now = Utc::now();
    let mut s1 = spec(
        "TASK-1",
        RequirementType::Task,
        "## Acceptance\n- A1. x\n",
        5,
    );
    s1.status = RequirementStatus::Completed;
    s1.implementation_info = Some(aida_core::ImplementationInfo {
        completed_at: Some(now - Duration::days(5)),
        ..Default::default()
    });
    let store = store_with(vec![s1]);

    // Write report and cache
    handle_criteria_coverage(root, &store, 90, false).unwrap();

    // Cache should be fresh
    let cache = load_fresh_coverage_cache(root, Utc::now(), Duration::hours(24));
    assert!(cache.is_some());
    let cache = cache.unwrap();
    assert!(cache.store_head.is_some());

    // Modify a spec and commit in .aida-store
    commit_file(
        &store_dir,
        "objects/task.yaml",
        "title: edit\n",
        "edit spec in store",
    );

    // Cache should now be invalid because store HEAD moved
    let stale = load_fresh_coverage_cache(root, Utc::now(), Duration::hours(24));
    assert!(stale.is_none(), "store HEAD change must invalidate cache");
}

// --- completion does not block (acceptance 4) ----------------------------------

#[test]
fn test_completion_ignores_intent_capture_floor() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida")).unwrap();

    // Configure 100% floor
    std::fs::write(
        root.join(".aida/config.toml"),
        "[capture]\ncriterion_test_floor_pct = 100\n",
    )
    .unwrap();
    assert_eq!(intent_capture_floor_pct(root), 100);

    // Spec with criteria but NO traced tests
    let mut req = aida_core::Requirement::new(
        "untraced task".into(),
        "## Acceptance\n- A1. untraced requirement\n".into(),
    );
    req.req_type = RequirementType::Task;
    req.set_status_from_str("Done");

    let into = crate::completion::transition_to_completed(
        &mut req,
        Some(root),
        "TASK-9999",
        "",
        "test",
        |r, prior| {
            assert!(matches!(r.status, RequirementStatus::Completed));
            assert!(matches!(prior, RequirementStatus::Done));
            Ok(())
        },
    )
    .unwrap();

    assert!(
        into,
        "transition to Completed must succeed despite 100% floor and 0 traces"
    );
    assert!(matches!(req.status, RequirementStatus::Completed));
}

// --- report json carries intent_capture object ---------------------------------

#[test]
fn test_report_json_carries_intent_capture_object() {
    let store = store_with(vec![spec(
        "TASK-1",
        RequirementType::Task,
        "## Acceptance\n- A1. x\n",
        5,
    )]);
    let report = coverage_from_parts(&store, Utc::now(), 90, &[], &[], &tokens(&[], 0), None);

    let json = serde_json::to_value(&report).unwrap();
    assert!(json.get("intent_capture").is_some());
    let ic_json = &json["intent_capture"];
    assert_eq!(ic_json["window_days"], 30);
    assert_eq!(ic_json["floor_pct"], 50);
    assert_eq!(ic_json["verdict"], "at or above floor");
}

// --- zero completed specs edge case -------------------------------------------

#[test]
fn test_zero_completed_specs_reports_at_or_above_floor() {
    let now = Utc::now();
    let ic = intent_capture_share(&[], now, 50);
    assert_eq!(ic.specs_with_criteria, 0);
    assert_eq!(ic.specs_with_traced_criterion, 0);
    assert_eq!(ic.share_pct, None);
    assert_eq!(ic.floor_pct, 50);
    assert_eq!(ic.verdict, IntentCaptureVerdict::AtOrAboveFloor);
    assert_eq!(
        ic.render_report_line(),
        "Intent capture (specs completed, last 30 d): 0 of 0 (nothing to measure) vs floor 50%: at or above floor"
    );
    assert_eq!(ic.render_status_line(), None);
}
