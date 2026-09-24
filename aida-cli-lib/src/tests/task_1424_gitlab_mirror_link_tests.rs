//! TASK-1424: the GitHub-PR <- GitLab-mirror-pipeline link.
//!
//! Real temp git repo + stubbed `gh`/`glab` (via `AIDA_TEST_GH_BINARY` /
//! `AIDA_TEST_GLAB_BINARY`, same pattern as `task1312_pr_gc_tests.rs`), plus
//! table tests for the two pure mappers.
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
}

// --- pure mappers ---

#[test]
fn github_status_state_maps_every_ci_state() {
    assert_eq!(github_status_state(CiState::Success), "success");
    assert_eq!(github_status_state(CiState::Failed), "failure");
    assert_eq!(github_status_state(CiState::Running), "pending");
    assert_eq!(github_status_state(CiState::Pending), "pending");
    assert_eq!(github_status_state(CiState::None), "pending");
}

#[test]
fn mirror_status_description_is_short_and_state_specific() {
    let success = CiStatus {
        state: CiState::Success,
        url: None,
        failing_checks: Vec::new(),
    };
    assert_eq!(
        mirror_status_description(&success),
        "GitLab mirror pipeline passed"
    );

    let failed = CiStatus {
        state: CiState::Failed,
        url: None,
        failing_checks: vec!["pipeline failed".to_string()],
    };
    assert_eq!(
        mirror_status_description(&failed),
        "GitLab mirror pipeline failed"
    );

    let running = CiStatus {
        state: CiState::Running,
        url: None,
        failing_checks: Vec::new(),
    };
    assert_eq!(
        mirror_status_description(&running),
        "GitLab mirror pipeline running"
    );

    // Every description stays well under GitHub's 140-char statuses cap.
    for status in [success, failed, running] {
        assert!(mirror_status_description(&status).len() < 140);
    }
}

// --- fake gh/glab plumbing ---

#[cfg(unix)]
fn write_fake_glab_pipelines(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
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
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// A fake `gh` that records every invocation's argv (one line per call,
/// space-joined) to `record_path`, so a test can assert whether — and with
/// what fields — the mirror status was posted.
#[cfg(unix)]
fn write_fake_gh_recording(
    dir: &std::path::Path,
    record_path: &std::path::Path,
) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("gh");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n\
             printf '%s\\n' \"$*\" >> '{}'\n\
             exit 0\n",
            record_path.display()
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn posts_success_status_pointing_at_the_gitlab_pipeline() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_github_repo(root);
    let sha = git(root, &["rev-parse", "main"]);

    let glab = write_fake_glab_pipelines(
        root,
        r#"[{"id":42,"status":"success","ref":"main","web_url":"https://gitlab.joemooney.com/ai/aida/-/pipelines/42"}]"#,
    );
    let record = root.join("gh-calls.log");
    let gh = write_fake_gh_recording(root, &record);

    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_TEST_GLAB_BINARY", glab.to_str().unwrap()),
        ("AIDA_TEST_GH_BINARY", gh.to_str().unwrap()),
    ]);

    sync_mirror_ci_link(root, "main");

    let logged = std::fs::read_to_string(&record).unwrap_or_default();
    assert!(
        logged.contains(&format!("statuses/{sha}")),
        "expected a status posted on {sha}, got: {logged}"
    );
    assert!(logged.contains("state=success"), "{logged}");
    assert!(
        logged.contains("target_url=https://gitlab.joemooney.com/ai/aida/-/pipelines/42"),
        "{logged}"
    );
    assert!(logged.contains("context=ci/gitlab-mirror"), "{logged}");
}

#[cfg(unix)]
#[test]
fn posts_failure_status_for_a_failed_pipeline() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_github_repo(root);

    let glab = write_fake_glab_pipelines(
        root,
        r#"[{"id":7,"status":"failed","ref":"main","web_url":"https://gitlab.joemooney.com/ai/aida/-/pipelines/7"}]"#,
    );
    let record = root.join("gh-calls.log");
    let gh = write_fake_gh_recording(root, &record);

    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_TEST_GLAB_BINARY", glab.to_str().unwrap()),
        ("AIDA_TEST_GH_BINARY", gh.to_str().unwrap()),
    ]);

    sync_mirror_ci_link(root, "main");

    let logged = std::fs::read_to_string(&record).unwrap_or_default();
    assert!(logged.contains("state=failure"), "{logged}");
    assert!(
        logged.contains("description=GitLab mirror pipeline failed"),
        "{logged}"
    );
}

#[cfg(unix)]
#[test]
fn no_pipeline_yet_posts_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_github_repo(root);

    // Empty pipeline list — GitLab has no pipeline for this branch yet.
    let glab = write_fake_glab_pipelines(root, "[]");
    let record = root.join("gh-calls.log");
    let gh = write_fake_gh_recording(root, &record);

    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_TEST_GLAB_BINARY", glab.to_str().unwrap()),
        ("AIDA_TEST_GH_BINARY", gh.to_str().unwrap()),
    ]);

    sync_mirror_ci_link(root, "main");

    assert!(
        !record.exists(),
        "gh must never be invoked when GitLab has no pipeline yet"
    );
}

#[cfg(unix)]
#[test]
fn glab_unavailable_is_a_silent_no_op() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_github_repo(root);

    let record = root.join("gh-calls.log");
    let gh = write_fake_gh_recording(root, &record);

    // A path that resolves but does not exist — the env-var override always
    // returns Some(path), so this exercises the "spawn fails" branch rather
    // than "no override configured", matching what a project without `glab`
    // installed looks like.
    let _env = crate::test_env::EnvVarsGuard::set(&[
        (
            "AIDA_TEST_GLAB_BINARY",
            "/nonexistent/glab-binary-for-task-1424",
        ),
        ("AIDA_TEST_GH_BINARY", gh.to_str().unwrap()),
    ]);

    sync_mirror_ci_link(root, "main");

    assert!(
        !record.exists(),
        "gh must never be invoked when glab can't be spawned"
    );
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
        r#"[{"id":1,"status":"success","ref":"main","web_url":"https://gitlab.joemooney.com/ai/aida/-/pipelines/1"}]"#,
    );
    let record = root.join("gh-calls.log");
    let gh = write_fake_gh_recording(root, &record);

    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_TEST_GLAB_BINARY", glab.to_str().unwrap()),
        ("AIDA_TEST_GH_BINARY", gh.to_str().unwrap()),
    ]);

    sync_mirror_ci_link(root, "main");

    assert!(
        !record.exists(),
        "a GitLab-primary project has no GitHub PR to link back to"
    );
}
