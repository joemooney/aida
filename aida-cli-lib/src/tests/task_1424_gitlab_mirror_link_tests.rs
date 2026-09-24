//! TASK-1424: the GitHub-PR <- GitLab-mirror-pipeline link, plus the
//! strict-review follow-up (informational-only status, exact-sha lookup,
//! pinned `gh` target, open-PR gate).
//!
//! Real temp git repo + stubbed `gh`/`glab` (via `AIDA_TEST_GH_BINARY` /
//! `AIDA_TEST_GLAB_BINARY`, same pattern as `task1312_pr_gc_tests.rs`), plus
//! table tests for the pure mappers.
// trace:TASK-1424 | ai:claude

use super::*;

fn git(repo: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("git {:?} failed to spawn: {e}", args));
    assert!(
        out.status.success(),
        "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn init_github_repo(path: &std::path::Path) {
    git(path, &["init", "-b", "main", "--quiet"]);
    git(path, &["config", "user.email", "aida@example.test"]);
    git(path, &["config", "user.name", "AIDA Test"]);
    git(path, &["commit", "--allow-empty", "-m", "base", "--quiet"]);
    git(
        path,
        &["remote", "add", "origin", "https://github.com/joe/aida.git"],
    );
    git(path, &["branch", "feature-x", "main"]);
}

// --- pure mappers ---

#[test]
fn github_status_state_is_always_success_regardless_of_gitlab_result() {
    // TASK-1424 strict-review follow-up: the posted GitHub `state` is FIXED
    // to `success` — a `failure`/`pending` mirror status must never be able
    // to shelve or stall a drain (`parse_ci_probe` reads it). The real
    // result lives only in the description.
    assert_eq!(GITHUB_STATUS_STATE, "success");
}

#[test]
fn mirror_status_description_carries_the_real_result_and_is_short() {
    let cases = [
        (CiState::Success, "GitLab mirror: passed — see link"),
        (CiState::Failed, "GitLab mirror: failed — see link"),
        (CiState::Running, "GitLab mirror: running — see link"),
        (CiState::Pending, "GitLab mirror: running — see link"),
    ];
    for (state, expected) in cases {
        let status = CiStatus {
            state,
            url: None,
            failing_checks: Vec::new(),
        };
        assert_eq!(mirror_status_description(&status), expected);
        assert!(mirror_status_description(&status).len() < 140);
    }
}

// --- fake gh/glab plumbing ---

#[cfg(unix)]
fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

#[cfg(unix)]
fn write_fake_glab_pipelines(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = dir.join("glab");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n\
             if [ \"$1\" = api ] && [ \"$4\" = projects/:id/pipelines ]; then\n\
               cat <<'EOF'\n{body}\nEOF\n\
               exit 0\n\
             fi\n\
             echo unexpected glab args: \"$@\" >&2\n\
             exit 1\n"
        ),
    )
    .unwrap();
    make_executable(&path);
    path
}

/// A fake `glab` that fails outright — simulates "not installed" without
/// relying on env-var-absence (the env-var override always returns
/// `Some(path)`, so an unresolvable/broken binary is what "unavailable"
/// actually looks like in this harness).
#[cfg(unix)]
fn write_failing_glab(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("glab");
    std::fs::write(&path, "#!/bin/sh\nexit 1\n").unwrap();
    make_executable(&path);
    path
}

/// A fake `gh` that answers `pr list` with `pr_line` (empty = no open PR)
/// and records every OTHER invocation's argv (one line per call,
/// space-joined) to `record_path` — so a test can assert whether, and with
/// what fields, the mirror status was posted.
#[cfg(unix)]
fn write_fake_gh(
    dir: &std::path::Path,
    pr_line: &str,
    record_path: &std::path::Path,
) -> std::path::PathBuf {
    let path = dir.join("gh");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n\
             if [ \"$1\" = pr ] && [ \"$2\" = list ]; then\n\
               printf '%s\\n' '{pr_line}'\n\
               exit 0\n\
             fi\n\
             printf '%s\\n' \"$*\" >> '{}'\n\
             exit 0\n",
            record_path.display()
        ),
    )
    .unwrap();
    make_executable(&path);
    path
}

/// The `pr list --json number,title,url,headRefName -q '...'` line shape
/// `gh_pr_list_first` parses: `number\ttitle\turl\theadRefName`.
fn pr_line_open(branch: &str) -> String {
    format!("42\tTest PR\thttps://github.com/joe/aida/pull/42\t{branch}")
}

#[cfg(unix)]
#[test]
fn posts_informational_success_status_even_for_a_failed_pipeline() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_github_repo(root);
    let sha = git(root, &["rev-parse", "feature-x"]);

    let glab = write_fake_glab_pipelines(
        root,
        &format!(
            r#"[{{"id":7,"status":"failed","ref":"feature-x","sha":"{sha}","web_url":"https://gitlab.joemooney.com/ai/aida/-/pipelines/7"}}]"#
        ),
    );
    let record = root.join("gh-calls.log");
    let gh = write_fake_gh(root, &pr_line_open("feature-x"), &record);

    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_TEST_GLAB_BINARY", glab.to_str().unwrap()),
        ("AIDA_TEST_GH_BINARY", gh.to_str().unwrap()),
    ]);

    sync_mirror_ci_link(root, "feature-x");

    let logged = std::fs::read_to_string(&record).unwrap_or_default();
    assert!(
        logged.contains(&format!("statuses/{sha}")),
        "expected a status posted on {sha}, got: {logged}"
    );
    // TASK-1424 strict-review: the posted `state` is ALWAYS success —
    // informational only — even though the pipeline itself failed.
    assert!(logged.contains("state=success"), "{logged}");
    assert!(
        logged.contains("description=GitLab mirror: failed — see link"),
        "{logged}"
    );
    assert!(
        logged.contains("target_url=https://gitlab.joemooney.com/ai/aida/-/pipelines/7"),
        "{logged}"
    );
    assert!(logged.contains("context=ci/gitlab-mirror"), "{logged}");
    assert!(
        logged.contains("repos/joe/aida/statuses/"),
        "expected the pinned owner/repo path, got: {logged}"
    );
}

#[cfg(unix)]
#[test]
fn looks_up_the_exact_pushed_sha_not_just_the_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_github_repo(root);
    let sha = git(root, &["rev-parse", "feature-x"]);

    // The fake glab only serves a pipeline when queried with `sha=<sha>` —
    // a `ref=`-only query (the old, reverted behavior) gets the "unexpected
    // args" failure path instead, which would make this test fail loudly if
    // the sha-filter regressed back to a branch-ref lookup.
    let path = root.join("glab");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n\
             case \"$*\" in\n\
               *'sha={sha}'*)\n\
                 cat <<'EOF'\n\
[{{\"id\":9,\"status\":\"success\",\"sha\":\"{sha}\",\"web_url\":\"https://gitlab.joemooney.com/ai/aida/-/pipelines/9\"}}]\n\
EOF\n\
                 exit 0\n\
                 ;;\n\
               *)\n\
                 echo unexpected glab args: \"$*\" >&2\n\
                 exit 1\n\
                 ;;\n\
             esac\n"
        ),
    )
    .unwrap();
    make_executable(&path);

    let record = root.join("gh-calls.log");
    let gh = write_fake_gh(root, &pr_line_open("feature-x"), &record);

    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_TEST_GLAB_BINARY", path.to_str().unwrap()),
        ("AIDA_TEST_GH_BINARY", gh.to_str().unwrap()),
    ]);

    sync_mirror_ci_link(root, "feature-x");

    let logged = std::fs::read_to_string(&record).unwrap_or_default();
    assert!(logged.contains("state=success"), "{logged}");
    assert!(
        logged.contains("pipelines/9"),
        "expected the sha-matched pipeline's url, got: {logged}"
    );
}

#[cfg(unix)]
#[test]
fn no_pipeline_for_the_exact_sha_posts_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_github_repo(root);

    // Empty pipeline list — GitLab has no pipeline for this exact sha yet
    // (an older pipeline for the branch, if any, must not be reported).
    let glab = write_fake_glab_pipelines(root, "[]");
    let record = root.join("gh-calls.log");
    let gh = write_fake_gh(root, &pr_line_open("feature-x"), &record);

    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_TEST_GLAB_BINARY", glab.to_str().unwrap()),
        ("AIDA_TEST_GH_BINARY", gh.to_str().unwrap()),
    ]);

    sync_mirror_ci_link(root, "feature-x");

    assert!(
        !record.exists(),
        "gh api must never be invoked when GitLab has no pipeline for this sha yet"
    );
}

#[cfg(unix)]
#[test]
fn glab_unavailable_is_a_silent_no_op() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_github_repo(root);

    let glab = write_failing_glab(root);
    let record = root.join("gh-calls.log");
    let gh = write_fake_gh(root, &pr_line_open("feature-x"), &record);

    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_TEST_GLAB_BINARY", glab.to_str().unwrap()),
        ("AIDA_TEST_GH_BINARY", gh.to_str().unwrap()),
    ]);

    sync_mirror_ci_link(root, "feature-x");

    assert!(
        !record.exists(),
        "gh api must never be invoked when glab fails"
    );
}

#[cfg(unix)]
#[test]
fn no_open_pr_skips_entirely_without_ever_asking_gitlab() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_github_repo(root);

    // glab would answer with a real pipeline if asked — but it must never be
    // asked, because there is no open PR to carry the status.
    let glab_path = root.join("glab");
    std::fs::write(
        &glab_path,
        "#!/bin/sh\necho glab must not be called >&2\nexit 1\n",
    )
    .unwrap();
    make_executable(&glab_path);

    let record = root.join("gh-calls.log");
    // Empty pr_line == `gh pr list` found nothing (no open PR).
    let gh = write_fake_gh(root, "", &record);

    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_TEST_GLAB_BINARY", glab_path.to_str().unwrap()),
        ("AIDA_TEST_GH_BINARY", gh.to_str().unwrap()),
    ]);

    sync_mirror_ci_link(root, "feature-x");

    let logged = std::fs::read_to_string(&record).unwrap_or_default();
    assert!(
        !logged.contains("api"),
        "gh api must never be invoked without an open PR: {logged}"
    );
}

#[cfg(unix)]
#[test]
fn default_branch_is_always_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_github_repo(root);

    let glab_path = root.join("glab");
    std::fs::write(
        &glab_path,
        "#!/bin/sh\necho glab must not be called >&2\nexit 1\n",
    )
    .unwrap();
    make_executable(&glab_path);
    let gh_path = root.join("gh");
    std::fs::write(
        &gh_path,
        "#!/bin/sh\necho gh must not be called >&2\nexit 1\n",
    )
    .unwrap();
    make_executable(&gh_path);

    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_TEST_GLAB_BINARY", glab_path.to_str().unwrap()),
        ("AIDA_TEST_GH_BINARY", gh_path.to_str().unwrap()),
    ]);

    // main IS this repo's default branch — never linked, no forge CLI call
    // of any kind (both fakes fail loudly if invoked).
    sync_mirror_ci_link(root, "main");
}

#[cfg(unix)]
#[test]
fn unresolvable_origin_skips_without_posting() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // No `origin` remote at all — the pin can never be resolved.
    git(root, &["init", "-b", "main", "--quiet"]);
    git(root, &["config", "user.email", "aida@example.test"]);
    git(root, &["config", "user.name", "AIDA Test"]);
    git(root, &["commit", "--allow-empty", "-m", "base", "--quiet"]);

    // `resolve_forge_kind` with no origin degrades to `ForgeKind::None`, so
    // this also exercises the earlier GitHub-forge guard — belt and braces
    // with the pin-resolution guard for a project with no origin at all.
    let glab = write_fake_glab_pipelines(
        root,
        r#"[{"id":1,"status":"success","web_url":"https://gitlab.joemooney.com/ai/aida/-/pipelines/1"}]"#,
    );
    let record = root.join("gh-calls.log");
    let gh = write_fake_gh(root, &pr_line_open("main"), &record);

    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_TEST_GLAB_BINARY", glab.to_str().unwrap()),
        ("AIDA_TEST_GH_BINARY", gh.to_str().unwrap()),
    ]);

    sync_mirror_ci_link(root, "main");

    assert!(!record.exists());
}

#[cfg(unix)]
#[test]
fn non_github_forge_skips_entirely() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-b", "main", "--quiet"]);
    git(root, &["config", "user.email", "aida@example.test"]);
    git(root, &["config", "user.name", "AIDA Test"]);
    git(root, &["commit", "--allow-empty", "-m", "base", "--quiet"]);
    git(root, &["branch", "feature-x", "main"]);
    // GitLab-primary project — nothing to link FROM.
    git(
        root,
        &[
            "remote",
            "add",
            "origin",
            "ssh://git@gitlab.joemooney.com:2222/ai/aida.git",
        ],
    );

    let glab = write_fake_glab_pipelines(
        root,
        r#"[{"id":1,"status":"success","web_url":"https://gitlab.joemooney.com/ai/aida/-/pipelines/1"}]"#,
    );
    let record = root.join("gh-calls.log");
    let gh = write_fake_gh(root, &pr_line_open("feature-x"), &record);

    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_TEST_GLAB_BINARY", glab.to_str().unwrap()),
        ("AIDA_TEST_GH_BINARY", gh.to_str().unwrap()),
    ]);

    sync_mirror_ci_link(root, "feature-x");

    assert!(
        !record.exists(),
        "a GitLab-primary project has no GitHub PR to link back to"
    );
}

// --- parse_ci_probe exclusion (drain safety net) ---

#[test]
fn parse_ci_probe_ignores_the_gitlab_mirror_context_status_context_shape() {
    // A rollup with ONLY the mirror status (StatusContext shape, as GitHub
    // reports a plain commit status) must read as "no checks" — not
    // in-progress, not red — even if it somehow carried FAILURE/PENDING.
    let stdout = format!(
        r#"[{{"number":7,"statusCheckRollup":[{{"state":"FAILURE","context":"{MIRROR_STATUS_CONTEXT}"}}]}}]"#
    );
    assert_eq!(
        crate::parse_ci_probe(&stdout),
        crate::CiProbe::PrNoChecks { pr_number: 7 }
    );
}

#[test]
fn parse_ci_probe_ignores_the_gitlab_mirror_context_alongside_a_real_check() {
    // A real failing check alongside our (informational, always-success, but
    // excluded-anyway-as-a-second-safety-net) mirror context still reports
    // the REAL check's failure, unaffected by the mirror context's presence.
    let stdout = format!(
        r#"[{{"number":7,"statusCheckRollup":[
            {{"state":"PENDING","context":"{MIRROR_STATUS_CONTEXT}"}},
            {{"status":"COMPLETED","conclusion":"SUCCESS","name":"build"}}
        ]}}]"#
    );
    assert_eq!(
        crate::parse_ci_probe(&stdout),
        crate::CiProbe::Green { pr_number: 7 }
    );
}
