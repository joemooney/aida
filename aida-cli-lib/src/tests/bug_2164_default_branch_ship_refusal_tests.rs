//! Tests for BUG-2164: `aida pr ship` with no PR number on the default branch
//! must refuse before any push or `gh` call, with suggestions if open approved
//! PRs exist.
//!
//! trace:BUG-2164 | ai:antigravity

use super::*;
use crate::pr_ship::{
    format_default_branch_ship_refusal, parse_open_approved_prs_json, OpenApprovedPr,
};

#[test]
fn parse_open_approved_prs_json_handles_empty_and_malformed() {
    assert_eq!(
        parse_open_approved_prs_json(""),
        Vec::<OpenApprovedPr>::new()
    );
    assert_eq!(
        parse_open_approved_prs_json("not json"),
        Vec::<OpenApprovedPr>::new()
    );
    assert_eq!(
        parse_open_approved_prs_json("{}"),
        Vec::<OpenApprovedPr>::new()
    );
    assert_eq!(
        parse_open_approved_prs_json("[]"),
        Vec::<OpenApprovedPr>::new()
    );
}

#[test]
fn parse_open_approved_prs_json_filters_and_extracts_approved_prs() {
    let json = r#"[
        {"number": 1, "title": "Draft PR", "reviewDecision": ""},
        {"number": 2, "title": "Needs work", "reviewDecision": "CHANGES_REQUESTED"},
        {"number": 3, "title": "Ready PR", "reviewDecision": "APPROVED"},
        {"number": 4, "title": "Also approved", "reviewDecision": "approved"},
        {"number": 5, "title": "Waiting review", "reviewDecision": "REVIEW_REQUIRED"},
        {"number": 6, "title": "No number", "reviewDecision": "APPROVED"}
    ]"#;

    let res = parse_open_approved_prs_json(json);
    assert_eq!(
        res,
        vec![
            OpenApprovedPr {
                number: 3,
                title: "Ready PR".to_string(),
            },
            OpenApprovedPr {
                number: 4,
                title: "Also approved".to_string(),
            },
            OpenApprovedPr {
                number: 6,
                title: "No number".to_string(),
            },
        ]
    );
}

#[test]
fn format_default_branch_ship_refusal_renders_expected_formats() {
    let empty: [OpenApprovedPr; 0] = [];
    assert_eq!(
        format_default_branch_ship_refusal(&empty),
        "aida pr ship: on the default branch; pass a PR number (e.g. aida pr ship 8)"
    );

    let approved = vec![
        OpenApprovedPr {
            number: 8,
            title: "Add feature X".to_string(),
        },
        OpenApprovedPr {
            number: 12,
            title: "".to_string(),
        },
    ];
    let msg = format_default_branch_ship_refusal(&approved);
    assert!(msg.starts_with(
        "aida pr ship: on the default branch; pass a PR number (e.g. aida pr ship 8)"
    ));
    assert!(msg.contains("Open approved PRs:"));
    assert!(msg.contains("  PR-8 — Add feature X"));
    assert!(msg.contains("  PR-12"));
}

fn create_test_repo_on_branch(branch: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let init_ok = std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["init", "-q", "-b", branch])
        .status();
    assert!(
        init_ok.map(|s| s.success()).unwrap_or(false),
        "git init failed"
    );

    std::fs::write(dir.path().join("file.txt"), "hello").unwrap();
    let add_ok = std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["add", "file.txt"])
        .status();
    assert!(
        add_ok.map(|s| s.success()).unwrap_or(false),
        "git add failed"
    );

    let commit_ok = std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-qm",
            "initial commit",
        ])
        .status();
    assert!(
        commit_ok.map(|s| s.success()).unwrap_or(false),
        "git commit failed"
    );

    dir
}

#[test]
fn pr_ship_handler_refuses_on_default_branch_without_pr_number() {
    let dir = create_test_repo_on_branch("main");
    let _ambient = crate::test_env::AmbientGuard::hermetic(dir.path(), None);

    let err = pr_ship_handler(
        None, false, false, false, false, None, None, false, false, false, None, false,
    )
    .unwrap_err();

    let err_str = err.to_string();
    assert!(
        err_str.contains(
            "aida pr ship: on the default branch; pass a PR number (e.g. aida pr ship 8)"
        ),
        "expected default branch refusal, got: {err_str}"
    );
}

#[test]
fn pr_ship_handler_refuses_on_default_branch_in_dry_run_without_pr_number() {
    let dir = create_test_repo_on_branch("main");
    let _ambient = crate::test_env::AmbientGuard::hermetic(dir.path(), None);

    let err = pr_ship_handler(
        None, false, false, true, false, None, None, false, false, false, None, false,
    )
    .unwrap_err();

    let err_str = err.to_string();
    assert!(
        err_str.contains(
            "aida pr ship: on the default branch; pass a PR number (e.g. aida pr ship 8)"
        ),
        "expected default branch refusal in dry run, got: {err_str}"
    );
}

#[test]
fn pr_ship_handler_bypasses_default_branch_guard_when_pr_number_provided() {
    let dir = create_test_repo_on_branch("main");
    let _ambient = crate::test_env::AmbientGuard::hermetic(dir.path(), None);

    // When explicit PR number is provided, default branch guard must not refuse.
    // It proceeds to step 1 (dry-run to avoid git network operations).
    let res = pr_ship_handler(
        Some(42),
        false,
        false,
        true,
        false,
        None,
        None,
        true,
        false,
        false,
        None,
        false,
    );

    // Whether it succeeds in dry-run or fails later, it MUST NOT fail with default branch refusal.
    if let Err(err) = res {
        let err_str = err.to_string();
        assert!(
            !err_str.contains("aida pr ship: on the default branch; pass a PR number"),
            "should not fail with default branch refusal when PR number is given, got: {err_str}"
        );
    }
}

#[test]
fn pr_ship_handler_bypasses_default_branch_guard_on_feature_branch() {
    let dir = create_test_repo_on_branch("main");
    // Create feature branch
    let ok = std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["checkout", "-qb", "feature-work"])
        .status();
    assert!(
        ok.map(|s| s.success()).unwrap_or(false),
        "git checkout failed"
    );

    let _ambient = crate::test_env::AmbientGuard::hermetic(dir.path(), None);

    // On feature branch without PR number in dry run, it proceeds to step 1 (would create new PR)
    let res = pr_ship_handler(
        None, false, false, true, false, None, None, true, false, false, None, false,
    );

    if let Err(err) = res {
        let err_str = err.to_string();
        assert!(
            !err_str.contains("aida pr ship: on the default branch; pass a PR number"),
            "should not fail with default branch refusal on feature branch, got: {err_str}"
        );
    }
}
