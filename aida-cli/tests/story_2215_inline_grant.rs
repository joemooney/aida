// Binary-driving PTY suite; Linux-only like its siblings (BUG-688).
#![cfg(target_os = "linux")]
//! STORY-2215 acceptance (1)(2): plain `aida agent new` at a human terminal
//! with no parent grant offers a one-time grant. Answering `y` launches the
//! child under a child grant derived from an `agent-new-inline` parent, and
//! the shell that ran it is left without a grant; answering `n` refuses with
//! today's text.
// trace:STORY-2215 | ai:claude

#[path = "support/test_git.rs"]
mod test_git;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod support;

const TEST_USER: &str = "story-2215-user";
const PROMPT: &str = "under a one-time grant from you";

/// Whether the outer environment variable `name` must not reach the child:
/// session state, and every marker the offer treats as headless or an agent
/// harness (this suite itself often runs under one).
fn scrubbed(name: &str) -> bool {
    name.starts_with("AIDA_")
        || name.starts_with("CODEX_")
        || name == "CLAUDECODE"
        || name.starts_with("CLAUDE_CODE")
}

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
    bin: PathBuf,
    calls: PathBuf,
}

fn git(repo: &Path, args: &[&str]) {
    let out = test_git::git_command()
        .current_dir(repo)
        .args(args)
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?} failed: {out:?}");
}

fn path_env(bin: &Path) -> std::ffi::OsString {
    std::env::join_paths(
        std::iter::once(bin.to_path_buf())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap()
}

fn fixture() -> Fixture {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let (repo, home, bin) = (base.join("repo"), base.join("home"), base.join("bin"));
    for dir in [&repo, &home, &bin] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let calls = base.join("claude-calls");
    // The fake `claude` records the grant it was launched with, then exits.
    let claude = bin.join("claude");
    std::fs::write(
        &claude,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo '2.1.0 (Claude Code)'; exit 0; fi\necho \"GRANT=$AIDA_SESSION_GRANT\" >> '{}'\nexit 0\n",
            calls.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    let gh = bin.join("gh");
    std::fs::write(&gh, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();

    git(&repo, &["init", "-q", "-b", "main"]);
    test_git::enroll_synthetic_home(&repo);
    git(&repo, &["config", "user.email", "t@t.t"]);
    git(&repo, &["config", "user.name", "t"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let mut init = std::process::Command::new(env!("CARGO_BIN_EXE_aida"));
    for (name, _) in std::env::vars() {
        if scrubbed(&name) {
            init.env_remove(name);
        }
    }
    let init = init
        .current_dir(&repo)
        .env("HOME", &home)
        .env("AIDA_HOME", &home)
        .env("AIDA_USER", TEST_USER)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_HEADLESS", "1")
        .env("PATH", path_env(&bin))
        .args([
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ])
        .output()
        .unwrap();
    assert!(init.status.success(), "{init:?}");
    std::fs::write(
        repo.join(".aida/config.toml"),
        "[agents]\nenabled = [\"claude\"]\n",
    )
    .unwrap();
    // The roster ceiling (advisor, delegating implementer). The fixture also
    // writes a grant record, which this suite never exports.
    support::ensure_seat_for(&home, &repo, TEST_USER, "advisor", &["implementer"]);
    Fixture {
        _tmp: tmp,
        repo,
        home,
        bin,
        calls,
    }
}

/// Run `aida agent new claude --role implementer` in a shell on a PTY,
/// answer the offer with `answer`, then have the SAME shell print its own
/// `AIDA_SESSION_GRANT`. Returns (agent-new exit code, full terminal output).
fn run_agent_new(fx: &Fixture, answer: &str) -> (i32, String) {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 40,
            cols: 200,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut cmd = CommandBuilder::new("sh");
    for (name, _) in std::env::vars() {
        if scrubbed(&name) {
            cmd.env_remove(name);
        }
    }
    cmd.env("HOME", &fx.home);
    cmd.env("AIDA_HOME", &fx.home);
    cmd.env("AIDA_USER", TEST_USER);
    cmd.env("AIDA_TELEMETRY", "0");
    cmd.env("PATH", path_env(&fx.bin));
    cmd.cwd(&fx.repo);
    cmd.arg("-c");
    cmd.arg(
        "\"$0\" agent new claude --role implementer --no-resume --no-context --no-title \
         --no-prompt --no-duplicate-check --no-default-flags; \
         code=$?; echo \"AGENT_NEW_EXIT=$code\"; echo \"SHELL_GRANT=[$AIDA_SESSION_GRANT]\"",
    );
    cmd.arg(env!("CARGO_BIN_EXE_aida"));
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);

    let output = Arc::new(Mutex::new(String::new()));
    let mut reader = pair.master.try_clone_reader().unwrap();
    let sink = Arc::clone(&output);
    let reading = std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            sink.lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&buf[..n]));
        }
    });
    let mut writer = pair.master.take_writer().unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while !output.lock().unwrap().contains(PROMPT) {
        assert!(
            Instant::now() < deadline,
            "the one-time grant offer never appeared:\n{}",
            output.lock().unwrap()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_millis(200));
    writer.write_all(answer.as_bytes()).unwrap();
    writer.write_all(b"\r").unwrap();
    writer.flush().unwrap();

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "agent new did not finish:\n{}",
            output.lock().unwrap()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(writer);
    drop(pair.master);
    let _ = reading.join();
    let text = output.lock().unwrap().clone();
    let code = text
        .split("AGENT_NEW_EXIT=")
        .nth(1)
        .and_then(|rest| {
            rest.trim_start()
                .split(|c: char| !c.is_ascii_digit())
                .next()
        })
        .and_then(|digits| digits.parse().ok())
        .unwrap_or_else(|| panic!("no exit code in output:\n{text}"));
    (code, text)
}

fn grant_record(home: &Path, id: &str) -> serde_json::Value {
    let path = home
        .join(".aida")
        .join("session-grants")
        .join(format!("{id}.json"));
    serde_json::from_slice(&std::fs::read(&path).unwrap_or_else(|e| panic!("{path:?}: {e}")))
        .unwrap()
}

#[test]
fn answering_yes_launches_under_a_child_of_a_one_time_grant() {
    let fx = fixture();
    let (code, text) = run_agent_new(&fx, "y");
    assert_eq!(code, 0, "agent new failed:\n{text}");
    let calls = std::fs::read_to_string(&fx.calls)
        .unwrap_or_else(|e| panic!("the child never launched ({e}):\n{text}"));
    let child_id = calls
        .lines()
        .find_map(|l| l.strip_prefix("GRANT="))
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| panic!("the child had no grant: {calls}"))
        .to_string();
    let child = grant_record(&fx.home, &child_id);
    assert_eq!(child["seat"], "implementer");
    assert_eq!(child["principal"], TEST_USER);
    let parent_id = child["parent_grant_id"]
        .as_str()
        .expect("child has a parent");
    let parent = grant_record(&fx.home, parent_id);
    assert_eq!(parent["origin"], "agent-new-inline", "{parent}");
    assert_eq!(parent["principal"], TEST_USER);
    assert_eq!(parent["seat"], "advisor");
    assert_eq!(
        parent["delegable_seats"],
        serde_json::json!(["implementer"])
    );
    // The parent is bound to the launcher process alone, never the shell.
    let binding = parent["binding"].as_array().unwrap();
    assert_eq!(binding.len(), 1, "{parent}");
    assert!(
        binding[0].as_str().unwrap().starts_with("proc:"),
        "{parent}"
    );
    // The shell that ran `agent new` holds no grant afterwards.
    assert!(text.contains("SHELL_GRANT=[]"), "{text}");
}

#[test]
fn answering_no_refuses_with_todays_text() {
    let fx = fixture();
    let (code, text) = run_agent_new(&fx, "n");
    assert_ne!(code, 0, "{text}");
    assert!(
        text.contains("no valid parent seat grant is active"),
        "{text}"
    );
    assert!(!fx.calls.exists(), "no child may launch on `n`");
    assert!(text.contains("SHELL_GRANT=[]"), "{text}");
}

#[test]
fn grant_inline_no_refuses_without_prompting() {
    let fx = fixture();
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_aida"));
    for (name, _) in std::env::vars() {
        if scrubbed(&name) {
            cmd.env_remove(name);
        }
    }
    let out = cmd
        .current_dir(&fx.repo)
        .env("HOME", &fx.home)
        .env("AIDA_HOME", &fx.home)
        .env("AIDA_USER", TEST_USER)
        .env("AIDA_TELEMETRY", "0")
        .env("PATH", path_env(&fx.bin))
        .args([
            "agent",
            "new",
            "claude",
            "--role",
            "implementer",
            "--no-resume",
            "--grant-inline",
            "no",
        ])
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "{text}");
    assert!(!text.contains(PROMPT), "{text}");
    assert!(
        text.contains("no valid parent seat grant is active"),
        "{text}"
    );
}
