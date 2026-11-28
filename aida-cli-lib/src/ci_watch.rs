// trace:TASK-3428 | ai:codex
use std::time::{Duration, Instant};

use crate::{
    ci_probe_with_forge, detect_open_pr_for_branch, find_project_root, first_run_id_from_gh_json,
    wait_for_ci_terminal, CiProbe, Colorize, PrLookup, RetryEtxtbsy,
};

/// TASK-233: `--watch-ci` — stream live CI progress via `gh run watch`
/// rather than silently polling, then re-probe for the terminal state so
/// the end-session decision tree (green proceeds / red prompts) runs
/// exactly as it does for `--wait-ci`. Falls back to the silent poll
/// loop when `gh` is missing or no run id resolves. trace:TASK-233
// trace:TASK-1165 | ai:claude
pub(crate) fn watch_ci_terminal(project_root: Option<&std::path::Path>, branch: &str) -> CiProbe {
    stream_ci(project_root, branch).unwrap_or_else(|| wait_for_ci_terminal(project_root, branch))
}

pub(crate) fn stream_ci(project_root: Option<&std::path::Path>, branch: &str) -> Option<CiProbe> {
    let root = project_root.unwrap_or_else(|| std::path::Path::new("."));
    let command = crate::forge::watcher_command(root)?;
    let backoff = || {
        crate::forge::watcher_api_backoff(root, crate::forge::ForgeKind::GitHub, || Some(command()))
    };
    if let Some(reason) = backoff() {
        eprintln!("CI watcher: {reason}");
        return None;
    }
    // Resolve the latest workflow run id for this branch.
    let run_id = command()
        .args([
            "run",
            "list",
            "--branch",
            branch,
            "--limit",
            "1",
            "--json",
            "databaseId",
        ])
        .output_retrying_etxtbsy()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| first_run_id_from_gh_json(&String::from_utf8_lossy(&o.stdout)))?;
    eprintln!(
        "  {} streaming `gh run watch {}`",
        crate::glyph(crate::glyphs::Glyph::FlowActive).cyan(),
        run_id.dimmed()
    );
    // Inherit stdio so gh's live-updating display renders straight to
    // the user's terminal. `gh run watch` returns when the run reaches
    // a terminal state.
    if let Ok(mut stream) = command()
        .args(["run", "watch", &run_id])
        .spawn_retrying_etxtbsy()
    {
        let mut checked = Instant::now();
        while matches!(stream.try_wait(), Ok(None)) {
            std::thread::sleep(Duration::from_secs(1));
            if checked.elapsed() >= Duration::from_secs(60) {
                if let Some(reason) = backoff() {
                    eprintln!("CI watcher: {reason}");
                    let _ = stream.kill();
                    let _ = stream.wait();
                    return None;
                }
                checked = Instant::now();
            }
        }
    }
    // Re-probe to classify Green/Red for the end-session decision tree.
    // Keep the re-probe on the caller-injected root. Re-deriving from the
    // process cwd can silently route a GitLab branch through GitHub when the
    // driven worktree is not the agent's cwd. Only the `None` arm — where the
    // caller supplied no root — keeps the historical cwd discovery.
    // trace:TASK-1273 | ai:claude
    let probe_root = project_root
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| find_project_root().unwrap_or_else(|_| std::path::PathBuf::from(".")));
    let forge_kind = crate::forge::resolve_forge_kind(&probe_root);
    if let Some(reason) = crate::forge::watcher_backoff(&probe_root, forge_kind, None) {
        eprintln!("CI watcher: {reason}");
        return None;
    }
    Some(ci_probe_with_forge(&probe_root, forge_kind, branch)) // STORY-516: forge-routed
}

#[cfg(all(test, unix))]
mod story1163_forge_dispatch_tests {
    use super::*;
    use tempfile::TempDir;

    fn gitlab_project() -> TempDir {
        let tmp = TempDir::new().unwrap();
        let aida = tmp.path().join(".aida");
        std::fs::create_dir_all(&aida).unwrap();
        std::fs::write(aida.join("config.toml"), "[forge]\nprovider = \"gitlab\"\n").unwrap();
        tmp
    }

    #[test]
    fn detect_open_pr_for_branch_dispatches_gitlab_to_glab() {
        let project = gitlab_project();
        let bin_dir = TempDir::new().unwrap();
        let log = bin_dir.path().join("glab.log");
        let glab = bin_dir.path().join("glab");
        crate::test_exec::write_executable(
            &glab,
            &format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = \"--version\" ]; then echo glab fake; exit 0; fi\n\
                 printf '%s\\n' \"$*\" >> '{}'\n\
                 printf '%s\\n' '[{{\"iid\":7,\"web_url\":\"https://gitlab.example.com/g/p/-/merge_requests/7\",\"source_branch\":\"feature/x\",\"target_branch\":\"main\",\"title\":\"GitLab MR\"}}]'\n",
                log.display()
            ),
        );
        let _g = crate::test_env::EnvVarGuard::set("AIDA_TEST_GLAB_BINARY", glab.to_str().unwrap());

        let found = match detect_open_pr_for_branch(project.path(), "feature/x") {
            PrLookup::Found(pr) => pr,
            _ => panic!("expected GitLab MR lookup"),
        };

        assert_eq!(found.number, 7);
        assert_eq!(found.head_branch.as_deref(), Some("feature/x"));
        let logged = std::fs::read_to_string(log).unwrap();
        assert!(logged.contains("api -X GET projects/:id/merge_requests"));
        assert!(logged.contains("-f source_branch=feature/x"));
        assert!(logged.contains("-f state=opened"));
    }

    #[test]
    fn diff_change_dispatches_gitlab_to_glab() {
        let project = gitlab_project();
        let bin_dir = TempDir::new().unwrap();
        let log = bin_dir.path().join("glab.log");
        let glab = bin_dir.path().join("glab");
        crate::test_exec::write_executable(
            &glab,
            &format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = \"--version\" ]; then echo glab fake; exit 0; fi\n\
                 printf '%s\\n' \"$*\" >> '{}'\n\
                 exit 0\n",
                log.display()
            ),
        );
        let _g = crate::test_env::EnvVarGuard::set("AIDA_TEST_GLAB_BINARY", glab.to_str().unwrap());

        crate::forge::forge_for_kind(project.path(), crate::forge::ForgeKind::GitLab)
            .diff_change(7)
            .expect("GitLab diff should dispatch through glab");

        let logged = std::fs::read_to_string(log).unwrap();
        assert!(logged.contains("mr diff 7"));
    }
}
