#![cfg(target_os = "linux")]

//! Black-box coverage for the protocol CLI surfaces introduced by STORY-1221.
//! These tests deliberately execute the binary so Clap dispatch, distributed
//! store discovery, lease discovery, and user-visible output are all covered.
// trace:TASK-1283 | ai:codex

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git");
    assert!(
        status.success(),
        "git {args:?} failed in {}",
        repo.display()
    );
}

fn aida(repo: &Path, home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(repo)
        .env("HOME", home)
        .env("AIDA_TELEMETRY", "0")
        // These assertions exercise the human protocol notice. Pin the format
        // so CI's non-TTY auto-selection cannot silently switch them to TOON.
        .env("AIDA_OUTPUT_FORMAT", "human")
        .env("NO_COLOR", "1")
        .env_remove("AIDA_HEADLESS")
        .env_remove("AIDA_SESSION_ID")
        .env_remove("AIDA_SESSION_ROLE");
    cmd
}

fn run(mut cmd: Command, label: &str) -> Output {
    let out = cmd.output().unwrap_or_else(|e| panic!("run {label}: {e}"));
    assert!(
        out.status.success(),
        "{label} failed ({:?})\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

struct Project {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
}

impl Project {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().canonicalize().expect("canonical tempdir");
        let repo = root.join("repo");
        let home = root.join("home");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Protocol Test"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        run(
            {
                let mut cmd = aida(&repo, &home);
                cmd.args([
                    "init",
                    "--no-skills",
                    "--no-hooks",
                    "--no-agent-config",
                    "--no-roles",
                ]);
                cmd
            },
            "aida init",
        );
        std::fs::write(
            repo.join(".aida/config.toml"),
            "[agents]\nenabled = [\"codex\"]\n",
        )
        .unwrap();
        Self {
            _temp: temp,
            repo,
            home,
        }
    }

    fn add(&self, kind: &str, title: &str) -> String {
        let out = run(
            {
                let mut cmd = aida(&self.repo, &self.home);
                cmd.env("AIDA_SESSION_ROLE", "advisor").args([
                    "add",
                    "--type",
                    kind,
                    "--status",
                    "approved",
                    "--mode",
                    "operator",
                    "--title",
                    title,
                    "--description",
                    "## Acceptance\n- fixture acceptance",
                ]);
                if kind == "spike" {
                    cmd.arg("--no-human-only");
                }
                cmd
            },
            "aida add",
        );
        String::from_utf8_lossy(&out.stdout)
            .split(|c: char| c.is_whitespace() || c == ',')
            .find(|token| {
                token.starts_with("SPIKE-")
                    || token.starts_with("BUG-")
                    || token.starts_with("STORY-")
            })
            .unwrap_or_else(|| panic!("no spec id in {}", text(&out)))
            .to_string()
    }
}

fn assert_pickup_block(output: &str, kind: &str) {
    let heading = format!("protocol: {kind} [META-");
    let start = output
        .find(&heading)
        .unwrap_or_else(|| panic!("missing {heading:?} in:\n{output}"));
    let block = output[start..].split("\n\n").next().unwrap();
    assert!(
        block.contains("META-"),
        "protocol must cite its META row:\n{block}"
    );
    assert!(
        block.to_ascii_lowercase().contains("spec acceptance"),
        "missing precedence label:\n{block}"
    );
    assert!(
        block.lines().count() <= 42,
        "pickup protocol exceeded the 40-line body cap:\n{block}"
    );
}

#[test]
fn pickup_commands_dispatch_and_render_protocols_before_acceptance() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let p = Project::new();
    let spike = p.add("spike", "queue work protocol fixture");
    let bug = p.add("bug", "do protocol fixture");
    let story = p.add("story", "worktree protocol fixture");

    let queue = run(
        {
            let mut cmd = aida(&p.repo, &p.home);
            cmd.args([
                "queue",
                "work",
                &spike,
                "--no-launch",
                "--no-human",
                "--no-pull",
            ]);
            cmd
        },
        "queue work",
    );
    let queue_text = text(&queue);
    assert_pickup_block(&queue_text, "spike");
    assert!(queue_text.contains("lane:research"));

    let do_out = run(
        {
            let mut cmd = aida(&p.repo, &p.home);
            cmd.args(["do", &bug, "--mode", "operator"]);
            cmd
        },
        "do",
    );
    assert_pickup_block(&text(&do_out), "bug");

    let enter = run(
        {
            let mut cmd = aida(&p.repo, &p.home);
            cmd.args(["worktree", "enter", &story]);
            cmd
        },
        "worktree enter",
    );
    assert_pickup_block(&text(&enter), "story");
}

#[test]
fn awaiting_notice_tracks_real_lease_through_session_end() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let p = Project::new();
    let bug = p.add("bug", "notice lifecycle fixture");

    let before = run(
        {
            let mut cmd = aida(&p.repo, &p.home);
            cmd.args(["awaiting", "--notice"]);
            cmd
        },
        "awaiting before lease",
    );
    assert!(!text(&before).contains("protocol: bug [META-"));

    let enter = run(
        {
            let mut cmd = aida(&p.repo, &p.home);
            cmd.args(["worktree", "enter", &bug]);
            cmd
        },
        "worktree enter for notice",
    );
    let shell = String::from_utf8_lossy(&enter.stdout);
    let worktree = shell
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("cd '")
                .and_then(|s| s.strip_suffix('\''))
        })
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("no worktree cd payload in:\n{shell}"));
    let session_id = shell
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("export AIDA_SESSION_ID='")
                .and_then(|s| s.strip_suffix('\''))
        })
        .unwrap_or_else(|| panic!("no session id export in:\n{shell}"));

    // `worktree enter` is itself a short-lived child process. In a Docker
    // executor its creator PID cannot be resolved back through the runner's
    // process tree, so explicitly attach the still-live test harness to the
    // real lease before exercising notice and session-end behavior.
    let lease_path = p
        .repo
        .join(".aida/sessions")
        .join(format!("{session_id}.toml"));
    let mut lease = std::fs::OpenOptions::new()
        .append(true)
        .open(&lease_path)
        .unwrap_or_else(|e| panic!("open lease {}: {e}", lease_path.display()));
    writeln!(lease, "active_pid = {}", std::process::id()).unwrap();

    let held = run(
        {
            let mut cmd = aida(&worktree, &p.home);
            cmd.env("AIDA_SESSION_ID", session_id)
                // This is the one call in this file that needs to observe a
                // real `backend.load()` finish rather than race the
                // production 1s fail-open bound (BUG-1239) — see
                // `notice_deadline` (aida-cli-lib/src/lib.rs, TASK-1274) for
                // why that bound must stay short for every other caller,
                // `awaiting_notice_does_not_read_an_open_stdin_pipe` included.
                .env("AIDA_TEST_NOTICE_DEADLINE_MS", "10000")
                .args(["awaiting", "--notice"]);
            cmd
        },
        "awaiting with lease",
    );
    assert!(
        text(&held).contains("protocol: bug [META-"),
        "{}",
        text(&held)
    );

    run(
        {
            let mut cmd = aida(&worktree, &p.home);
            cmd.env("AIDA_SESSION_ID", session_id).args([
                "session",
                "end",
                session_id,
                "--yes",
                "--skip-ci",
            ]);
            cmd
        },
        "session end",
    );
    let after = run(
        {
            let mut cmd = aida(&p.repo, &p.home);
            cmd.env("AIDA_SESSION_ID", session_id)
                .args(["awaiting", "--notice"]);
            cmd
        },
        "awaiting after session end",
    );
    assert!(
        !text(&after).contains("protocol: bug [META-"),
        "{}",
        text(&after)
    );
}

#[test]
fn awaiting_notice_does_not_read_an_open_stdin_pipe() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let p = Project::new();
    let mut child = aida(&p.repo, &p.home)
        .args(["awaiting", "--notice"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn awaiting --notice");
    // Keep the writer alive without sending data or EOF. Before BUG-1239 the
    // child drained non-TTY stdin and remained blocked here indefinitely.
    // trace:BUG-1239 | ai:codex
    let _held_open = child.stdin.take().expect("piped stdin");
    let started = Instant::now();
    let contract_budget = Duration::from_secs(2);
    // Give the test harness one extra second to reap and diagnose a late child,
    // while separately asserting the command met its two-second contract. This
    // avoids a scheduler race exactly at the assertion/kill boundary.
    let reap_deadline = started + Duration::from_secs(3);
    loop {
        if let Some(status) = child.try_wait().expect("poll awaiting --notice") {
            let elapsed = started.elapsed();
            assert!(status.success(), "awaiting --notice failed: {status}");
            assert!(
                elapsed < contract_budget,
                "awaiting --notice exceeded its two-second budget: {elapsed:?}"
            );
            let stdout = child.stdout.take().expect("piped stdout");
            let output = std::io::read_to_string(stdout).expect("read notice stdout");
            assert!(
                output.starts_with("Current date/time:"),
                "notice did not emit its fail-open time line: {output:?}"
            );
            break;
        }
        if Instant::now() >= reap_deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "awaiting --notice remained alive for {:?} with an open stdin pipe",
                started.elapsed()
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn protocol_seed_dispatch_adds_four_and_is_idempotent() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let p = Project::new();
    let store = p.repo.join(".aida-store");
    let mut protocol_files = Vec::new();
    fn files_below(dir: &Path, files: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files_below(&path, files);
            } else {
                files.push(path);
            }
        }
    }
    let mut object_files = Vec::new();
    files_below(&store.join("objects"), &mut object_files);
    for path in object_files {
        let body = std::fs::read_to_string(&path).unwrap();
        if [
            "protocol:story",
            "protocol:task",
            "protocol:decision",
            "protocol:doc",
        ]
        .iter()
        .any(|tag| body.contains(tag))
        {
            protocol_files.push(path);
        }
    }
    assert_eq!(
        protocol_files.len(),
        4,
        "expected four removable protocol rows"
    );
    for path in &protocol_files {
        std::fs::remove_file(path).unwrap();
    }
    git(&store, &["add", "-u"]);
    git(
        &store,
        &["commit", "-q", "-m", "test: remove four protocols"],
    );

    let first = run(
        {
            let mut cmd = aida(&p.repo, &p.home);
            cmd.args(["protocol", "seed"]);
            cmd
        },
        "protocol seed first",
    );
    assert!(
        text(&first).contains("seeded 4 missing type protocol(s)"),
        "{}",
        text(&first)
    );
    let second = run(
        {
            let mut cmd = aida(&p.repo, &p.home);
            cmd.args(["protocol", "seed"]);
            cmd
        },
        "protocol seed second",
    );
    assert!(
        text(&second).contains("seeded 0 missing type protocol(s)"),
        "{}",
        text(&second)
    );
}
