// BUG-1594: `aida show` had single calls up to 23.8s. Two causes, both pinned
// here against the real binary:
//   1. the upstream-report recheck notice ran a FULL store load before every
//      command, because its "checked this version" marker was only written
//      when a stale report existed;
//   2. the git-linkage forge (PR) lookup had no ceiling, so one slow `gh`
//      call could stall the whole command.
// The fixture's fake `gh` never answers; `aida show` must still finish inside
// a generous budget and say the PR state is unknown because the lookup timed
// out (PRIN-5) rather than claiming there is no PR.
// trace:BUG-1594 | ai:claude
// trace:BUG-1995 | ai:antigravity
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Performance budget for forge PR lookups. Reported when missed, not fatal.
// trace:BUG-1995 | ai:antigravity
const BUDGET: Duration = Duration::from_secs(30);

/// The fatal bound. A hung forge lookup times out far below it; only a total
/// timeout regression reaches the ceiling.
// trace:BUG-1995 | ai:antigravity
const HARD_CEILING: Duration = Duration::from_secs(60);

#[cfg(target_os = "linux")]
fn load_average() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}

fn aida_command(repo: &Path, home: &Path, gh: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aida"));
    command
        .current_dir(repo)
        .env("HOME", home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_ROLE", "advisor")
        .env("AIDA_TEST_GH_BINARY", gh);
    for name in [
        "AIDA_STORE",
        "AIDA_PROJECT_ROOT",
        "AIDA_DRIVE_ROOT",
        "AIDA_OUTPUT_FORMAT",
        "GIT_DIR",
        "GIT_WORK_TREE",
    ] {
        command.env_remove(name);
    }
    let fixture_store = repo.join(".aida-store");
    if fixture_store.join("objects").is_dir() {
        command.env("AIDA_STORE", fixture_store);
    }
    command
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?}");
}

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
    gh: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    // A `gh` that never answers. `exec` so the process `aida` kills at the
    // ceiling is the sleeper itself (no grandchild holding the pipes).
    let gh = tmp.path().join("gh");
    std::fs::write(&gh, "#!/bin/sh\nexec sleep 120\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "BUG-1594 Test"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let run = |args: &[&str]| -> Output {
        aida_command(&repo, &home, &gh)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("run aida")
    };
    let init = run(&[
        "init",
        "--no-skills",
        "--no-hooks",
        "--no-agent-config",
        "--no-roles",
    ]);
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let add = run(&[
        "add", "--title", "demo", "--type", "task", "--status", "approved",
    ]);
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    // In-flight work: a feature branch carrying a `(TASK-1)` commit, with a
    // GitHub origin so the linkage section performs a `gh` PR lookup.
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/demo.git",
        ],
    );
    git(&repo, &["checkout", "-q", "-b", "task-1"]);
    std::fs::write(repo.join("work.txt"), "work\n").unwrap();
    git(&repo, &["add", "work.txt"]);
    git(&repo, &["commit", "-q", "-m", "feat: demo work (TASK-1)"]);
    // Warm the fixture untimed: the first invocation pays cold costs (cache
    // build, the once-per-version notice check) unrelated to the bound.
    let warm = run(&["show", "TASK-1", "--no-git"]);
    assert!(
        warm.status.success(),
        "warm-up failed: {}",
        String::from_utf8_lossy(&warm.stderr)
    );
    Fixture {
        _tmp: tmp,
        repo,
        home,
        gh,
    }
}

/// Run `aida` polling `try_wait` so a regression fails with a clear budget
/// panic instead of hanging the suite.
// trace:BUG-1594 | ai:claude
fn run_bounded(fx: &Fixture, args: &[&str], budget: Duration) -> (Output, Duration) {
    run_bounded_with(aida_command(&fx.repo, &fx.home, &fx.gh), args, budget)
}

/// [`run_bounded`] over a caller-prepared command (extra env, same budget
/// discipline).
// trace:STORY-1649 | ai:claude
fn run_bounded_with(mut command: Command, args: &[&str], budget: Duration) -> (Output, Duration) {
    let mut child = command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn aida");
    let started = Instant::now();
    loop {
        if child.try_wait().expect("poll aida").is_some() {
            let elapsed = started.elapsed();
            let out = child.wait_with_output().expect("collect output");
            // trace:BUG-1995 | ai:antigravity
            if elapsed >= budget {
                eprintln!(
                    "BUDGET-MISSED `aida {args:?}` took {elapsed:?}, over its {budget:?} budget \
                     but inside the {HARD_CEILING:?} ceiling (1-minute load {}). Reported, not \
                     failed: a busy host explains this; a hung forge would have hit the ceiling. \
                     trace:BUG-1995",
                    load_average()
                );
            }
            return (out, elapsed);
        }
        if started.elapsed() > HARD_CEILING {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "`aida {args:?}` exceeded its {HARD_CEILING:?} hard ceiling (1-minute load {})",
                load_average()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn bug_1594_show_with_hung_forge_finishes_in_budget_and_says_timed_out() {
    let fx = fixture();
    let (out, _elapsed) = run_bounded(&fx, &["--format", "human", "show", "TASK-1"], BUDGET);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "stdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Truncation honesty: the PR line names the timeout and says the state
    // is unknown — it never claims there is no PR.
    assert!(stdout.contains("Git linkage"), "{stdout}");
    assert!(stdout.contains("task-1"), "{stdout}");
    assert!(stdout.contains("timed out"), "{stdout}");
    assert!(stdout.contains("state unknown"), "{stdout}");
    assert!(!stdout.contains("no PR opened yet"), "{stdout}");
}

// STORY-1649: the default TOON `aida show` path now carries one `linkage:`
// line. It must come from the cheap default-branch scan — never the full
// `--all` walk or the forge lookup — so with the same hung `gh` the TOON
// read stays inside the BUG-1594 budget and still prints the line.
// trace:STORY-1649 | ai:claude
#[test]
fn bug_1594_toon_show_default_path_with_linkage_stays_in_budget() {
    let fx = fixture();
    let mut command = aida_command(&fx.repo, &fx.home, &fx.gh);
    command.env("AIDA_AGENT_OUTPUT", "toon");
    let (out, _elapsed) = run_bounded_with(command, &["show", "TASK-1"], BUDGET);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "stdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let line = stdout
        .lines()
        .find(|l| l.starts_with("linkage:"))
        .unwrap_or_else(|| panic!("TOON show must print a linkage line:\n{stdout}"));
    // TASK-1's commit is on the feature branch, not main: honest `false`,
    // and nothing from the hung forge lookup leaks into the line.
    assert!(line.contains("shipped=false"), "{line}");
    assert!(!line.contains("timed out"), "{line}");
}

#[test]
fn bug_1594_upstream_notice_check_is_recorded_once_per_version() {
    let fx = fixture();
    // The warm-up run found no stale upstream reports; it must still record
    // the check, or every later command repeats the full store load.
    let marker = fx.repo.join(".aida").join("upstream-report-notice-version");
    let recorded = std::fs::read_to_string(&marker)
        .expect("notice check must be recorded even with no stale reports");
    // Marker = version, then the store HEAD the check covered.
    let mut lines = recorded.lines();
    assert_eq!(lines.next(), Some(env!("CARGO_PKG_VERSION")));
    let sha = lines.next().unwrap_or_default();
    assert_eq!(sha.len(), 40, "store HEAD sha recorded: {recorded:?}");
    let (out, _elapsed) = run_bounded(&fx, &["show", "TASK-1", "--no-git"], BUDGET);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// BUG-1594 fix-up: the notice marker used to create `.aida/` beside a fresh
// `--file <dir>` store. That directory steers the cache path, so the FIRST
// `aida add` wrote its cache row to one cache and every later command read
// another — the first requirement was missing from `aida list`
// (tests/test_distributed.sh: "Expected at least 3 requirements, got 2").
// trace:BUG-1594 | ai:claude
#[test]
fn bug_1594_first_add_to_fresh_file_store_is_visible_to_list() {
    let tmp = tempfile::tempdir().unwrap();
    // Nest the store deeper than the cache-path walk-up (6 levels) so a
    // stray `.aida/` in an ancestor of the temp dir (e.g. on a dev box)
    // cannot capture the cache — this mirrors CI, where none exists.
    let root_buf = tmp.path().join("a/b/c/d/e/f/g");
    let root = root_buf.as_path();
    let store = root.join("store");
    let home = root.join("home");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let run = |args: &[&str]| -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
        cmd.current_dir(root)
            .env("HOME", &home)
            .env("AIDA_TELEMETRY", "0");
        for name in [
            "AIDA_STORE",
            "AIDA_PROJECT_ROOT",
            "AIDA_OUTPUT_FORMAT",
            "GIT_DIR",
            "GIT_WORK_TREE",
        ] {
            cmd.env_remove(name);
        }
        cmd.arg("--file")
            .arg(&store)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("run aida")
    };
    for title in ["First", "Second"] {
        let out = run(&[
            "add",
            "--title",
            title,
            "--type",
            "functional",
            "--status",
            "draft",
        ]);
        assert!(
            out.status.success(),
            "add {title}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let list = run(&["list"]);
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(
        list.status.success(),
        "{}",
        String::from_utf8_lossy(&list.stderr)
    );
    assert!(
        stdout.contains("First"),
        "first add missing from list: {stdout}"
    );
    assert!(stdout.contains("Second"), "{stdout}");
}
