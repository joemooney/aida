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
    assert!(msg.contains("  aida pr ship 8 — Add feature X"));
    assert!(msg.contains("  aida pr ship 12"));
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

    set_origin_head(dir.path(), "main");
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

// trace:TASK-1795 | ai:codex
fn set_origin_head(root: &std::path::Path, default: &str) {
    let target = format!("refs/remotes/origin/{default}");
    for args in [
        vec!["update-ref", target.as_str(), "HEAD"],
        vec!["symbolic-ref", "refs/remotes/origin/HEAD", target.as_str()],
    ] {
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
}

// trace:TASK-1795 | ai:codex
fn fake_default_branch_gh(
    root: &std::path::Path,
    provider: &str,
    body: &str,
) -> std::path::PathBuf {
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        format!("[forge]\nprovider = \"{provider}\"\n"),
    )
    .unwrap();
    let script = root.join("fake-gh");
    crate::test_exec::write_executable(
        &script,
        format!(
            "#!/bin/sh\necho called >> '{}'\n{body}\n",
            root.join("gh-calls").display()
        ),
    );
    script
}

// trace:TASK-1795 | ai:codex
#[test]
fn task_1795_local_mainlines_and_feature_resume_skip_gh() {
    for branch in ["main", "master"] {
        let dir = create_test_repo_on_branch(branch);
        let gh = fake_default_branch_gh(dir.path(), "github", "echo unexpected");
        let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
            dir.path(),
            "implementer",
            &[],
            &[("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap()))],
        );
        assert!(ship_is_default_branch(dir.path(), branch));
        assert!(!dir.path().join("gh-calls").exists());
        set_origin_head(dir.path(), branch);
        assert!(!ship_is_default_branch(dir.path(), "feature-work"));
        assert!(!dir.path().join("gh-calls").exists());
    }
}

// trace:TASK-1795 | ai:codex
#[test]
fn task_1795_origin_head_custom_mainline_skips_gh() {
    let dir = create_test_repo_on_branch("trunk");
    for args in [
        vec!["update-ref", "refs/remotes/origin/release/mainline", "HEAD"],
        vec![
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/release/mainline",
        ],
    ] {
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    let gh = fake_default_branch_gh(dir.path(), "github", "echo unexpected");
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        dir.path(),
        "implementer",
        &[],
        &[("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap()))],
    );
    assert!(ship_is_default_branch(dir.path(), "release/mainline"));
    assert!(!ship_is_default_branch(dir.path(), "feature-work"));
    assert!(!dir.path().join("gh-calls").exists());
}

// trace:TASK-1795 | ai:codex
#[test]
fn task_1795_failed_gh_falls_back_to_master_without_origin_head() {
    let dir = create_test_repo_on_branch("master");
    let gh = fake_default_branch_gh(dir.path(), "github", "exit 1");
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        dir.path(),
        "implementer",
        &[],
        &[("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap()))],
    );
    assert_eq!(crate::forge::default_branch_of(dir.path()), "master");
    assert!(dir.path().join("gh-calls").exists());
    let err = pr_ship_handler(
        None, false, false, true, false, None, None, false, false, false, None, false,
    )
    .unwrap_err();
    assert!(err.to_string().contains("on the default branch"), "{err:#}");
}

// trace:TASK-1795 | ai:codex
#[test]
fn task_1795_hung_gh_default_branch_probe_is_bounded() {
    let dir = create_test_repo_on_branch("master");
    let gh = fake_default_branch_gh(dir.path(), "github", "/bin/sleep 10\necho wrong");
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        dir.path(),
        "implementer",
        &[],
        &[("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap()))],
    );
    let started = std::time::Instant::now();
    assert_eq!(crate::forge::default_branch_of(dir.path()), "master");
    assert!(started.elapsed() < std::time::Duration::from_secs(8));
    assert!(dir.path().join("gh-calls").exists());
}

// trace:TASK-1795 | ai:codex
#[test]
fn task_1795_non_github_default_branch_probe_skips_gh() {
    for provider in ["gitlab", "pure-git"] {
        let dir = create_test_repo_on_branch("master");
        let gh = fake_default_branch_gh(dir.path(), provider, "echo wrong");
        let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
            dir.path(),
            "implementer",
            &[],
            &[("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap()))],
        );
        assert_eq!(crate::forge::default_branch_of(dir.path()), "master");
        assert!(!dir.path().join("gh-calls").exists());
    }
}

// trace:TASK-1795 | ai:codex
#[test]
fn task_1795_unknown_local_default_uses_forge_fallback() {
    let dir = create_test_repo_on_branch("trunk");
    let gh = fake_default_branch_gh(dir.path(), "github", "echo trunk");
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        dir.path(),
        "implementer",
        &[],
        &[("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap()))],
    );
    assert!(ship_is_default_branch(dir.path(), "trunk"));
    assert!(!ship_is_default_branch(dir.path(), "feature-work"));
    assert!(dir.path().join("gh-calls").exists());
}

// trace:TASK-1795 | ai:codex
#[test]
fn task_1795_leftover_main_ref_is_not_default_branch_evidence() {
    let dir = create_test_repo_on_branch("develop");
    assert!(std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["update-ref", "refs/remotes/origin/main", "HEAD"])
        .status()
        .unwrap()
        .success());
    let gh = fake_default_branch_gh(dir.path(), "github", "echo develop");
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        dir.path(),
        "implementer",
        &[],
        &[("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap()))],
    );
    let error = pr_ship_handler(
        None, false, false, true, false, None, None, false, false, false, None, false,
    )
    .unwrap_err();
    assert!(error.to_string().contains("pass a PR number"), "{error:#}");
    assert!(dir.path().join("gh-calls").exists());
}

// trace:TASK-1795 | ai:codex
#[test]
fn task_1795_hung_probe_without_evidence_refuses_instead_of_guessing_main() {
    let dir = create_test_repo_on_branch("trunk");
    let gh = fake_default_branch_gh(
        dir.path(),
        "github",
        "case \"$1\" in repo) exec /bin/sleep 10;; *) printf '[]\\n';; esac",
    );
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        dir.path(),
        "implementer",
        &[],
        &[("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap()))],
    );
    let start = std::time::Instant::now();
    let error = pr_ship_handler(
        None, false, false, true, false, None, None, false, false, false, None, false,
    )
    .unwrap_err();
    assert!(error.to_string().contains("pass a PR number"), "{error:#}");
    assert!(start.elapsed() >= std::time::Duration::from_secs(2));
    assert!(start.elapsed() < std::time::Duration::from_secs(8));
    assert!(dir.path().join("gh-calls").exists());
}

// trace:TASK-1795 | ai:codex
#[test]
fn task_1795_probe_errors_and_bad_answers_remain_unknown() {
    for body in [
        "exit 1",
        "echo 'bad branch'",
        "printf 'trunk\\nmain\\n'",
        "printf ''",
    ] {
        let dir = create_test_repo_on_branch("trunk");
        let gh = fake_default_branch_gh(dir.path(), "github", body);
        let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
            dir.path(),
            "implementer",
            &[],
            &[("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap()))],
        );
        assert!(crate::forge::probe_default_branch(dir.path()).is_none());
        assert!(ship_is_default_branch(dir.path(), "trunk"), "{body}");
    }
}
