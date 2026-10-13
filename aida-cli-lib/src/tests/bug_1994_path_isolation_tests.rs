use super::*;
use std::io::Read;
use std::process::{Command, Stdio};

// Capture to files so a stuck descendant cannot keep an output pipe open after
// the harness is killed. Only this invocation's process group is terminated.
// trace:BUG-1994 | ai:codex
#[cfg(unix)]
fn bounded_output(command: &mut Command, cap: std::time::Duration) -> std::process::Output {
    let dir = tempfile::tempdir().unwrap();
    let stdout_path = dir.path().join("stdout");
    let stderr_path = dir.path().join("stderr");
    command
        .stdout(std::fs::File::create(&stdout_path).unwrap())
        .stderr(std::fs::File::create(&stderr_path).unwrap());
    use std::os::unix::process::CommandExt;
    command.process_group(0);
    let mut child = command.spawn().unwrap();
    let deadline = std::time::Instant::now() + cap;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            // SAFETY: spawn created a distinct process group with this live
            // child's PID; the negative PID selects only its descendants.
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            child.wait().unwrap();
            panic!(
                "stress child exceeded {cap:?}: {command:?}\n{}\n{}",
                std::fs::read_to_string(&stdout_path).unwrap(),
                std::fs::read_to_string(&stderr_path).unwrap()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    std::process::Output {
        status,
        stdout: std::fs::read(stdout_path).unwrap(),
        stderr: std::fs::read(stderr_path).unwrap(),
    }
}

// trace:BUG-1994 | ai:codex
#[test]
#[cfg(unix)]
#[should_panic(expected = "stress child exceeded")]
fn stress_watchdog_refuses_a_stuck_child() {
    bounded_output(
        Command::new("/bin/sh").args(["-c", "sleep 10"]),
        std::time::Duration::from_millis(50),
    );
}

// trace:BUG-1994 | ai:codex
const PATH_TESTS: &[&str] = &[
    "bug_1957_pr_ship_squash_subject_returns_completion_trailer",
    "one_expensive_candidate_cannot_starve_the_scan",
    "bug1780_concurrent_fetch_does_not_break_pull_code_leg",
    "recover_open_change_uses_github_origin_even_with_stale_pure_git_config",
    "run_claude_session_restores_terminal_title_after_child_exit",
    "finds_gh_via_path",
    "rejects_non_executable",
    "rejects_is_executable_but_spawn_fails",
    "falls_back_past_broken_to_working",
    "drain_finish_ci_completes_for_hold_gate_only_red",
    "drain_finish_ci_shelves_real_red_naming_only_the_genuine_check",
    "drain_finish_ci_retries_unavailable_rows_then_shelves_ci_unavailable",
    "bug_1624_apply_session_env_to_process_ignores_non_allowlisted_names",
    "task_1458_drain_merge_passes_the_approved_head_as_match_head_commit",
    "task_1458_drain_merge_with_a_stale_approval_never_calls_gh_merge",
    "init_github_plus_gitlab_creates_both_and_publishes_in_order",
    "non_refusal_push_error_never_creates_project",
    "no_token_prints_manual_recipe_and_keeps_github_repo",
    "mirror_url_creates_missing_gitlab_project",
    "mirror_rerun_with_a_different_url_never_probes_the_existing_remote",
    "mirror_rerun_without_url_stays_offline",
    "mirror_rerun_with_an_equivalent_url_still_probes",
    "create_uses_the_host_scoped_glab_token_over_the_env_token",
    "mirror_rerun_creates_the_project_when_the_remote_already_exists",
    "mirror_non_refusal_probe_error_wires_without_creating",
    "gitlab_only_init_publishes_to_origin_with_upstream",
];

fn child_command(home: &std::path::Path) -> Command {
    std::fs::create_dir_all(home).unwrap();
    std::fs::write(
        home.join(".gitconfig"),
        "[user]\nname = AIDA Test\nemail = test@example.invalid\n[init]\ndefaultBranch = main\n",
    )
    .unwrap();
    let mut command = Command::new(crate::aida_bin::running_executable_fallback());
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1");
    if let Some(path) = std::env::var_os("LD_LIBRARY_PATH") {
        command.env("LD_LIBRARY_PATH", path);
    }
    command
}

// This child installs and actually executes the shim, then holds it live until
// the parent finishes an unlocked Git lookup. Its fixture stays alive through
// the parent's wait, so passing cannot be attributed to an expired shim.
// trace:BUG-1994 | ai:codex
#[test]
fn fake_git_path_probe_child() {
    let Some(root) = std::env::var_os("AIDA_TEST_PATH_PROBE_ROOT") else {
        return;
    };
    assert!(is_path_isolated_process());
    let root = std::path::PathBuf::from(root);
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    crate::test_exec::write_executable(
        &bin.join("git"),
        "#!/bin/sh\nprintf 'isolated-fake-git\\n'\n",
    );
    let _path = EnvVarGuard::prepend_path(&bin);
    let output = Command::new("git").arg("--version").output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "isolated-fake-git"
    );
    std::fs::write(root.join("ready"), "shim active").unwrap();
    // EOF is the release signal. Unwind/parent exit also closes the pipe.
    let mut release = Vec::new();
    std::io::stdin().read_to_end(&mut release).unwrap();
    assert!(bin.join("git").exists(), "shim retained until child exit");
}

// trace:BUG-1994 | ai:codex
#[test]
fn fake_git_is_scoped_to_one_child_and_real_git_reader_is_unaffected() {
    let root = tempfile::tempdir().unwrap();
    let path = std::env::var_os("PATH");
    let name = "test_env::bug_1994_path_isolation_tests::fake_git_path_probe_child";
    let mut child = child_command(&root.path().join("home"))
        .env("AIDA_TEST_PATH_CHILD", name)
        .env("AIDA_TEST_PATH_PROBE_ROOT", root.path())
        .args(["--exact", "--nocapture", "--test-threads=1", name])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !root.path().join("ready").exists() {
        if child.try_wait().unwrap().is_some() {
            let output = child.wait_with_output().unwrap();
            panic!(
                "probe child exited before readiness: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        if std::time::Instant::now() >= deadline {
            drop(child.stdin.take());
            child.wait().unwrap();
            panic!("probe child did not become ready");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let output = Command::new("git").arg("--version").output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("git version "));
    assert_eq!(std::env::var_os("PATH"), path);
    assert!(root.path().join("bin/git").exists());
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line.starts_with("test result: ok. 1 passed; 0 failed;")));
}

// trace:BUG-1994 | ai:codex
#[test]
#[should_panic(expected = "PATH mutation requires run_path_isolated_test")]
fn unisolated_path_mutation_is_refused_before_setting_environment() {
    require_path_mutation_scope("PATH");
}

// libtest accepts a union of filters in one process:
// https://github.com/rust-lang/rust/blob/master/library/test/src/cli.rs
// Each round exercises history readers with all formerly shared PATH writers
// scheduled by one 16-thread harness, rather than separate serial cargo runs.
// trace:BUG-1994 | ai:codex
#[test]
#[cfg(unix)]
fn history_and_path_mutators_pass_15_rounds_at_16_threads() {
    let root = tempfile::tempdir().unwrap();
    let mut list = child_command(&root.path().join("home"));
    // The existing weekly-only random DAG sweep stays on its own schedule.
    // Exclude it from BOTH discovery and execution so all selected tests must pass.
    let skip = "task_1507_random_dag_sweep_fixed_seeds";
    list.args(["--list", "--skip", skip, "task_1507_history_cache_tests"])
        .args(PATH_TESTS);
    let output = bounded_output(&mut list, std::time::Duration::from_secs(120));
    assert!(output.status.success());
    let names: Vec<_> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_suffix(": test").map(str::to_owned))
        .collect();
    assert!(names
        .iter()
        .any(|name| name.contains("task_1507_history_cache_tests")));
    for filter in PATH_TESTS {
        assert!(
            names.iter().any(|name| name.contains(filter)),
            "missing stress member {filter}"
        );
    }
    for round in 1..=15 {
        let mut command = child_command(&root.path().join(format!("home-{round}")));
        command
            .args([
                "--nocapture",
                "--test-threads=16",
                "--skip",
                skip,
                "task_1507_history_cache_tests",
            ])
            .args(PATH_TESTS);
        let output = bounded_output(&mut command, std::time::Duration::from_secs(120));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "round {round} failed:\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let expected = format!("test result: ok. {} passed; 0 failed;", names.len());
        assert!(
            stdout.lines().any(|line| line.starts_with(&expected)),
            "stress round {round} did not run all {} members:\n{stdout}",
            names.len()
        );
        println!(
            "PATH/history stress round {round}/15: {} members, 16 threads, zero failures",
            names.len()
        );
    }
}
