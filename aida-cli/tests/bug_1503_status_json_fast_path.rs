// BUG-1503: bare `aida status --format json` / `aida status --json` must take
// the same sub-second, cache-only path as bare human `aida status` instead of
// silently falling through to the heavy `--full`-equivalent report (which
// warms `gh` network probes and builds the full "Awaiting you" report). Drives
// the real binary so a regression back to the heavy dispatch path is caught by
// both an output-shape check and a hard wall-clock budget, never a hang.
// trace:BUG-1503 | ai:claude
#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

fn aida_command(repo: &Path, home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aida"));
    command
        .current_dir(repo)
        .env("HOME", home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_ROLE", "advisor");
    for name in [
        "AIDA_STORE",
        "AIDA_PROJECT_ROOT",
        "AIDA_DRIVE_ROOT",
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

fn aida(repo: &Path, home: &Path, args: &[&str]) -> Output {
    aida_command(repo, home)
        .args(args)
        .output()
        .expect("run aida")
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?}");
}

fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "BUG-1503 Test"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let init = aida(
        &repo,
        &home,
        &[
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ],
    );
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    // Warm the fixture with one untimed `aida status` before any timed run.
    // The very first invocation against a freshly-`init`ed store pays cold
    // costs (cache build/verify, first git reads) that have nothing to do
    // with the fast-vs-heavy dispatch this test suite is pinning — without
    // this warm-up the budgeted runs below flake under CPU contention
    // (observed: 3 parallel fixtures on a loaded box each cold-starting).
    let warm = aida(&repo, &home, &["status"]);
    assert!(
        warm.status.success(),
        "warm-up `aida status` failed: {}",
        String::from_utf8_lossy(&warm.stderr)
    );
    (tmp, repo, home)
}

/// Run `aida status <extra args>` as a child process, polling `try_wait`
/// instead of blocking on `.output()`. A regression to the heavy dispatch
/// path must fail this test with a clear budget-exceeded panic rather than
/// hanging the suite the way the bare defect hung an interactive agent.
// trace:BUG-1503 | ai:claude
fn run_status_bounded(
    repo: &Path,
    home: &Path,
    extra: &[&str],
    budget: Duration,
) -> (Output, Duration) {
    let mut args = vec!["status"];
    args.extend_from_slice(extra);
    let mut child = aida_command(repo, home)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn aida status");
    let started = Instant::now();
    let kill_deadline = started + budget + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().expect("poll aida status") {
            let elapsed = started.elapsed();
            let stdout_handle = child.stdout.take();
            let stderr_handle = child.stderr.take();
            use std::io::Read;
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            if let Some(mut h) = stdout_handle {
                let _ = h.read_to_end(&mut stdout);
            }
            if let Some(mut h) = stderr_handle {
                let _ = h.read_to_end(&mut stderr);
            }
            return (
                Output {
                    status,
                    stdout,
                    stderr,
                },
                elapsed,
            );
        }
        if Instant::now() >= kill_deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "`aida status {}` did not return within {:?} — regressed onto the heavy path",
                extra.join(" "),
                kill_deadline - started
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Acceptance 1 + 4: bare `--format json` completes within a generous bound
/// (well under the heavy path's observed tens-of-seconds cost) and parses as
/// JSON.
#[test]
fn status_format_json_completes_fast_and_parses() {
    let (_tmp, repo, home) = fixture();
    let budget = Duration::from_secs(30);
    let (out, elapsed) = run_status_bounded(&repo, &home, &["--format", "json"], budget);
    assert!(
        out.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        elapsed < budget,
        "bare `aida status --format json` took {elapsed:?}, exceeding the {budget:?} budget"
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|error| {
        panic!(
            "bare `aida status --format json` did not parse as JSON: {error}\n{}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    assert!(value.is_object(), "expected a JSON object: {value}");
}

/// Acceptance 2: the bare json branch must reuse the SAME cache-only fast
/// snapshot as the human default, not the heavy report. Pin the shape rather
/// than only timing — a shape that grows the heavy-only keys (`awaiting`,
/// `session`, `pr`, `agents`) is the regression signature even if the fixture
/// repo happens to stay fast.
#[test]
fn status_format_json_uses_fast_snapshot_shape_not_heavy_report() {
    let (_tmp, repo, home) = fixture();
    let (out, _elapsed) =
        run_status_bounded(&repo, &home, &["--format", "json"], Duration::from_secs(30));
    assert!(out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
    for field in ["role", "branch", "queue", "cache_present", "counts"] {
        assert!(
            value.get(field).is_some(),
            "fast-snapshot field `{field}` missing from bare status json: {value}"
        );
    }
    for heavy_only_field in ["awaiting", "session", "agents"] {
        assert!(
            value.get(heavy_only_field).is_none(),
            "bare `aida status --format json` pulled in heavy-report field `{heavy_only_field}`: {value}"
        );
    }
    assert!(value["counts"]["open"].is_u64());
    assert!(value["counts"]["total"].is_u64());
    assert!(value["queue"]["depth"].is_u64());
    // Monitor contract (monitor_contract.rs / docs/monitor-contract-fixtures/
    // status.json) promises `requirements.total` (integer) and
    // `requirements.by_status` (object) from `aida status --json`; the fast
    // path must not drop `by_status` the way it once did. trace:BUG-1503
    assert!(
        value["requirements"]["total"].is_u64(),
        "requirements.total missing or not an integer: {value}"
    );
    assert!(
        value["requirements"]["by_status"].is_object(),
        "requirements.by_status missing or not an object: {value}"
    );
}

/// Acceptance 3: `--format json` and `--json` must agree on the bare command,
/// same as every other surface honouring the global pin.
#[test]
fn status_format_json_and_json_flag_agree() {
    let (_tmp, repo, home) = fixture();
    let budget = Duration::from_secs(30);
    let (via_format, _) = run_status_bounded(&repo, &home, &["--format", "json"], budget);
    let (via_flag, _) = run_status_bounded(&repo, &home, &["--json"], budget);
    assert!(via_format.status.success());
    assert!(via_flag.status.success());
    let format_value: serde_json::Value =
        serde_json::from_slice(&via_format.stdout).expect("valid json via --format json");
    let flag_value: serde_json::Value =
        serde_json::from_slice(&via_flag.stdout).expect("valid json via --json");
    assert_eq!(format_value, flag_value);
}
