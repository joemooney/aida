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
    let init_ok = crate::test_git::git_command()
        .arg("-C")
        .arg(dir.path())
        .args(["init", "-q", "-b", branch])
        .status();
    assert!(
        init_ok.map(|s| s.success()).unwrap_or(false),
        "git init failed"
    );

    std::fs::write(dir.path().join("file.txt"), "hello").unwrap();
    let add_ok = crate::test_git::git_command()
        .arg("-C")
        .arg(dir.path())
        .args(["add", "file.txt"])
        .status();
    assert!(
        add_ok.map(|s| s.success()).unwrap_or(false),
        "git add failed"
    );

    let commit_ok = crate::test_git::git_command()
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
    let ok = crate::test_git::git_command()
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
            !err_str.contains("aida pr ship: on the default branch; pass a PR number")
                && !err_str.contains("the default branch could not be determined"),
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
        assert!(crate::test_git::git_command()
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
        assert!(crate::test_git::git_command()
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
    assert!(crate::test_git::git_command()
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
    assert!(
        error.to_string().contains("on the default branch"),
        "{error:#}"
    );
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

// trace:TASK-1803 | ai:antigravity
#[test]
fn task_1803_gitlab_feature_branch_without_origin_head_proceeds_via_ls_remote() {
    let origin = create_test_repo_on_branch("trunk");
    let work = create_test_repo_on_branch("feature-work");
    assert!(crate::test_git::git_command()
        .arg("-C")
        .arg(work.path())
        .args(["remote", "add", "origin", origin.path().to_str().unwrap()])
        .status()
        .unwrap()
        .success());
    let gh = fake_default_branch_gh(work.path(), "gitlab", "echo unexpected");
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        work.path(),
        "implementer",
        &[],
        &[("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap()))],
    );

    assert_eq!(
        crate::forge::probe_default_branch(work.path()).as_deref(),
        Some("trunk")
    );
    assert_eq!(
        classify_ship_default_branch(work.path(), "feature-work"),
        ShipDefaultBranchDisposition::FeatureBranch
    );
    assert!(!ship_is_default_branch(work.path(), "feature-work"));
    assert!(!work.path().join("gh-calls").exists());

    // Dry-run ship on the GitLab feature branch proceeds past the default-branch guard.
    let res = pr_ship_handler(
        None, false, false, true, false, None, None, true, false, false, None, false,
    );
    if let Err(err) = res {
        let err_str = err.to_string();
        assert!(
            !err_str.contains("on the default branch")
                && !err_str.contains("the default branch could not be determined"),
            "unexpected default-branch refusal on GitLab feature branch: {err_str}"
        );
    }
}

// trace:TASK-1803 | ai:antigravity
#[test]
fn task_1803_unknown_default_branch_refusal_wording_suggests_set_head_auto() {
    let dir = create_test_repo_on_branch("feature-work");
    let gh = fake_default_branch_gh(dir.path(), "github", "exit 1");
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        dir.path(),
        "implementer",
        &[],
        &[("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap()))],
    );

    assert_eq!(
        classify_ship_default_branch(dir.path(), "feature-work"),
        ShipDefaultBranchDisposition::UnknownDefaultBranch
    );
    let err = pr_ship_handler(
        None, false, false, true, false, None, None, false, false, false, None, false,
    )
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("the default branch could not be determined"),
        "expected unknown default branch wording, got: {err}"
    );
    assert!(
        err.contains("git remote set-head origin --auto"),
        "expected git remote set-head origin --auto suggestion, got: {err}"
    );
    assert!(
        !err.contains("on the default branch"),
        "must not claim to be on the default branch when unknown, got: {err}"
    );
}

// trace:TASK-1803 | ai:antigravity
#[test]
fn task_1803_stripped_path_gh_resolution_finds_fallback_binary() {
    // trace:BUG-1994 | ai:antigravity
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let dir = create_test_repo_on_branch("feature-work");
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    std::fs::write(
        dir.path().join(".aida/config.toml"),
        "[forge]\nprovider = \"github\"\n",
    )
    .unwrap();

    // Build a stripped PATH directory containing only `git` (no `gh`), so `git`
    // subcommands still function while `gh` must be found via Pass 2 fallback.
    let git_only_bin = dir.path().join("git-only-bin");
    std::fs::create_dir_all(&git_only_bin).unwrap();
    let real_git = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|d| d.join("git"))
        .find(|p| p.is_file())
        .unwrap_or_else(|| std::path::PathBuf::from("/usr/bin/git"));
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real_git, git_only_bin.join("git")).unwrap();

    // The fallback directories are replaced by one the test owns, so a gh
    // installed on the host cannot shadow the fixture.
    let fallback_bin = dir.path().join("fallback-bin");
    std::fs::create_dir_all(&fallback_bin).unwrap();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        dir.path(),
        "implementer",
        &[],
        &[
            ("AIDA_TEST_GH_BINARY", None),
            ("PATH", Some(git_only_bin.to_str().unwrap())),
            (
                "AIDA_TEST_FORGE_FALLBACK_DIRS",
                Some(fallback_bin.to_str().unwrap()),
            ),
        ],
    );

    // Install a working gh in the fallback directory (resolved by Pass 2 when PATH has no gh).
    let fallback_gh = fallback_bin.join("gh");
    let marker = dir.path().join("fallback-gh-called");
    crate::test_exec::write_executable(
        &fallback_gh,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  echo 'gh version 2.0.0'\n  exit 0\nfi\necho called >> '{}'\necho trunk\n",
            marker.display()
        ),
    );

    assert_eq!(
        crate::forge::probe_default_branch(dir.path()).as_deref(),
        Some("trunk")
    );
    assert!(marker.exists(), "expected fallback gh to be invoked");
    assert_eq!(
        classify_ship_default_branch(dir.path(), "feature-work"),
        ShipDefaultBranchDisposition::FeatureBranch
    );
}

// trace:TASK-1951 trace:TASK-1660 | ai:claude
#[test]
fn task_1951_parse_ls_remote_symref_head_refuses_ambiguous_or_unsafe_answers() {
    use crate::forge::parse_ls_remote_symref_head as parse;
    let sha = "0123456789abcdef0123456789abcdef01234567";
    assert_eq!(
        parse(&format!("ref: refs/heads/trunk\tHEAD\n{sha}\tHEAD\n")).as_deref(),
        Some("trunk")
    );
    // Symrefs for other refs are ignored, not mistaken for HEAD.
    assert_eq!(
        parse("ref: refs/heads/other\trefs/remotes/x/HEAD\nref: refs/heads/trunk\tHEAD\n")
            .as_deref(),
        Some("trunk")
    );
    for bad in [
        // No symref line at all (server without symref capability).
        format!("{sha}\tHEAD\n"),
        String::new(),
        // A symref line without the tab separator is malformed.
        "ref: refs/heads/trunk HEAD\n".to_owned(),
        // Two HEAD symrefs: ambiguous evidence.
        "ref: refs/heads/trunk\tHEAD\nref: refs/heads/main\tHEAD\n".to_owned(),
        // HEAD pointing outside refs/heads.
        "ref: refs/tags/v1\tHEAD\n".to_owned(),
        // Branch names the shell-safety guard rejects.
        "ref: refs/heads/-x\tHEAD\n".to_owned(),
        "ref: refs/heads/a;rm -rf\tHEAD\n".to_owned(),
        "ref: refs/heads/\tHEAD\n".to_owned(),
    ] {
        assert_eq!(parse(&bad), None, "{bad:?}");
    }
}

// trace:TASK-1951 trace:TASK-1659 | ai:claude
#[test]
fn task_1951_default_branch_of_prefers_local_origin_head_over_network_probe() {
    let origin = create_test_repo_on_branch("trunk");
    let work = create_test_repo_on_branch("develop");
    for args in [
        vec!["remote", "add", "origin", origin.path().to_str().unwrap()],
        vec!["update-ref", "refs/remotes/origin/develop", "HEAD"],
        vec![
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/develop",
        ],
    ] {
        assert!(crate::test_git::git_command()
            .arg("-C")
            .arg(work.path())
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    let gh = fake_default_branch_gh(work.path(), "pure-git", "echo unexpected");
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        work.path(),
        "implementer",
        &[],
        &[("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap()))],
    );
    // The remote answers "trunk"; local origin/HEAD says "develop" and must
    // win without a network round-trip.
    assert_eq!(
        crate::forge::probe_default_branch(work.path()).as_deref(),
        Some("trunk")
    );
    assert_eq!(crate::forge::default_branch_of(work.path()), "develop");
}

// trace:TASK-1951 trace:TASK-1663 | ai:claude
#[test]
fn task_1951_origin_head_probe_keeps_operator_ssh_command() {
    for via_config in [false, true] {
        let work = create_test_repo_on_branch("feature-work");
        let log = work.path().join("ssh-calls");
        let ssh = work.path().join("fake-ssh");
        crate::test_exec::write_executable(
            &ssh,
            format!("#!/bin/sh\necho \"$*\" >> '{}'\nexit 1\n", log.display()),
        );
        let user_command = format!("'{}' --operator-wrapper", ssh.display());
        let mut git_args = vec![vec![
            "remote".to_owned(),
            "add".to_owned(),
            "origin".to_owned(),
            "ssh://forge.invalid/repo.git".to_owned(),
        ]];
        if via_config {
            git_args.push(vec![
                "config".to_owned(),
                "core.sshCommand".to_owned(),
                user_command.clone(),
            ]);
        }
        for args in git_args {
            assert!(crate::test_git::git_command()
                .arg("-C")
                .arg(work.path())
                .args(&args)
                .status()
                .unwrap()
                .success());
        }
        let gh = fake_default_branch_gh(work.path(), "pure-git", "echo unexpected");
        let env_command = (!via_config).then_some(user_command.as_str());
        let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
            work.path(),
            "implementer",
            &[],
            &[
                ("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap())),
                ("GIT_SSH_COMMAND", env_command),
                ("GIT_SSH", None),
            ],
        );
        assert_eq!(crate::forge::probe_default_branch(work.path()), None);
        let calls = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(
            calls
                .lines()
                .any(|l| l.contains("--operator-wrapper") && l.contains("BatchMode=yes")),
            "via_config={via_config}: {calls:?}"
        );
    }
}

// trace:TASK-1951 trace:TASK-1663 | ai:claude
#[test]
fn task_1951_ssh_command_with_batch_mode_appends_to_configured_command() {
    use crate::forge::ssh_command_with_batch_mode as batch;
    assert_eq!(batch(None), "ssh -o BatchMode=yes");
    assert_eq!(
        batch(Some("ssh -i ~/.ssh/deploy")),
        "ssh -i ~/.ssh/deploy -o BatchMode=yes"
    );
}

/// TASK-2099 + TASK-2100: with only GIT_SSH set (a bare program, here in a
/// directory whose name has a space and a quote), the probe quotes it and
/// adds the batch flag of the variant git will use: OpenSSH's
/// `-o BatchMode=yes`, or `-batch` for a program named plink.
// trace:TASK-1951 trace:TASK-2099 trace:TASK-2100 | ai:claude
#[test]
fn task_1951_origin_head_probe_quotes_git_ssh_and_uses_the_variant_batch_flag() {
    for (program, want, refuse) in [
        ("fake-ssh", "BatchMode=yes", "-batch"),
        ("plink", "-batch", "BatchMode"),
    ] {
        let work = create_test_repo_on_branch("feature-work");
        let log = work.path().join("ssh-calls");
        let dir = work.path().join("op's tools");
        std::fs::create_dir_all(&dir).unwrap();
        let ssh = dir.join(program);
        crate::test_exec::write_executable(
            &ssh,
            format!("#!/bin/sh\necho \"$*\" >> '{}'\nexit 1\n", log.display()),
        );
        assert!(crate::test_git::git_command()
            .arg("-C")
            .arg(work.path())
            .args(["remote", "add", "origin", "ssh://forge.invalid/repo.git"])
            .status()
            .unwrap()
            .success());
        let gh = fake_default_branch_gh(work.path(), "pure-git", "echo unexpected");
        let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
            work.path(),
            "implementer",
            &[],
            &[
                ("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap())),
                ("GIT_SSH_COMMAND", None),
                ("GIT_SSH_VARIANT", None),
                ("GIT_SSH", Some(ssh.to_str().unwrap())),
            ],
        );
        assert_eq!(crate::forge::probe_default_branch(work.path()), None);
        let calls = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(
            calls
                .lines()
                .any(|l| l.contains(want) && !l.contains(refuse)),
            "{program}: {calls:?}"
        );
    }
}

// trace:TASK-1951 trace:TASK-2100 | ai:claude
#[test]
fn task_1951_batch_flag_follows_the_ssh_variant() {
    use crate::forge::ssh_command_with_batch_mode_for as batch;
    assert_eq!(
        batch(Some("'/opt/putty/plink.exe'"), None),
        "'/opt/putty/plink.exe' -batch"
    );
    assert_eq!(
        batch(Some("tortoiseplink -v"), None),
        "tortoiseplink -v -batch"
    );
    assert_eq!(
        batch(Some("my-wrapper"), Some("plink")),
        "my-wrapper -batch"
    );
    assert_eq!(batch(Some("my-wrapper"), Some("simple")), "my-wrapper");
    assert_eq!(
        batch(Some("ssh -i k"), Some("auto")),
        "ssh -i k -o BatchMode=yes"
    );
    assert_eq!(batch(None, None), "ssh -o BatchMode=yes");
}
