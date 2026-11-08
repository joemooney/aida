// BUG-688: binary-driving e2e suites are Linux-gated like their siblings.
#![cfg(target_os = "linux")]
//! Exercise declared Git policy and linked-worktree roots through the real CLI.
// trace:STORY-1486 | ai:codex

#[path = "support/test_git.rs"]
mod test_git;

use std::path::Path;
use std::process::{Command, Output};

fn aida(repo: &Path, home: &Path, path: &str, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(repo)
        .env("HOME", home)
        .env("AIDA_HOME", home)
        .env("AIDA_TELEMETRY", "0")
        .env("PATH", path)
        .args(args);
    for var in [
        "AIDA_SESSION_ROLE",
        "AIDA_SESSION_GRANT",
        "AIDA_PERMISSION_MODE",
        "AIDA_AGENT_OUTPUT",
        "AIDA_OUTPUT_FORMAT",
        "AIDA_HEADLESS_VENDOR",
        "AIDA_STORE",
        "AIDA_STORE_PATH",
        "AIDA_PROJECT_ROOT",
        "AIDA_SESSION_PROJECT",
        "AIDA_CONTAINED",
        "AIDA_CONTAINMENT_POSTURE",
        "GIT_DIR",
        "GIT_COMMON_DIR",
        "GIT_WORK_TREE",
    ] {
        cmd.env_remove(var);
    }
    cmd.output().expect("run aida")
}

fn git(repo: &Path, args: &[&str]) {
    let status = test_git::git_command()
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

// Exercise the real TTY picker rather than supplying --role before dispatch.
// trace:STORY-1486 | ai:codex
fn picked_role_policy(repo: &Path, home: &Path, path: &str, role: &str) {
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};
    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_aida"));
    cmd.cwd(repo);
    cmd.args(["agent", "new", "claude", "--no-exec", "--verbose"]);
    cmd.env("HOME", home);
    cmd.env("AIDA_HOME", home);
    cmd.env("PATH", path);
    cmd.env("TERM", "xterm");
    for key in [
        "AIDA_SESSION_ROLE",
        "AIDA_SESSION_GRANT",
        "AIDA_CONTAINED",
        "AIDA_PERMISSION_MODE",
        "AIDA_CONTAINMENT_POSTURE",
        "AIDA_HEADLESS",
        "AIDA_STORE",
        "AIDA_STORE_PATH",
        "AIDA_PROJECT_ROOT",
        "AIDA_SESSION_PROJECT",
        "GIT_DIR",
        "GIT_COMMON_DIR",
        "GIT_WORK_TREE",
    ] {
        cmd.env_remove(key);
    }
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 40,
            cols: 160,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let read = std::thread::spawn(move || {
        let mut buf = [0; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0
                || send
                    .send(String::from_utf8_lossy(&buf[..n]).into_owned())
                    .is_err()
            {
                break;
            }
        }
    });
    let started = Instant::now();
    let mut output = String::new();
    let mut selected = false;
    let status = loop {
        while let Ok(chunk) = receive.try_recv() {
            output.push_str(&chunk);
            if chunk.contains("\x1b[6n") {
                writer.write_all(b"\x1b[1;1R").unwrap();
            }
        }
        if !selected && output.contains("Select a role for the new claude agent:") {
            assert!(
                !output.contains("git_policy:"),
                "policy preceded picker: {output}"
            );
            writer.write_all(format!("{role}\r").as_bytes()).unwrap();
            selected = true;
        }
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(15) {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("role picker did not finish: {output}");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    drop(writer);
    drop(pair.master);
    read.join().unwrap();
    for chunk in receive.try_iter() {
        output.push_str(&chunk);
    }
    assert!(status.success(), "{output}");
    assert!(selected, "{output}");
    assert!(output.contains("git_policy: read-only"), "{output}");
    assert!(
        output.contains(&format!("requested seat: {role}")),
        "{output}"
    );
    assert!(!output.contains("git_policy: write"), "{output}");
}

// trace:STORY-1486.ac325a99 | ai:codex
// trace:STORY-1486.ac91839b | ai:codex
#[test]
fn noexec_verbose_prints_stakeholder_and_explicit_git_downgrades() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path().canonicalize().expect("canonical tempdir");
    let (repo, home, bin) = (base.join("repo"), base.join("home"), base.join("bin"));
    for dir in [&repo, &home, &bin] {
        std::fs::create_dir_all(dir).unwrap();
    }
    // A stand-in `claude` so the launcher resolves a binary; --no-exec never
    // runs it.
    let claude = bin.join("claude");
    std::fs::write(
        &claude,
        "#!/bin/sh\necho unexpected-agent-execution >&2\nexit 97\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        aida_core::exec_context::Environment::capture()
            .var("PATH")
            .unwrap_or_default()
    );

    git(&repo, &["init", "-q", "-b", "main"]);
    // trace:BUG-2017 | ai:codex — explicit positive global-ID fixture.
    test_git::enroll_synthetic_home(&repo);
    std::fs::set_permissions(
        repo.join(".git")
            .join(aida_core::allocation_authority::CONFIG_FILE),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Launch Profile Test"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let init = aida(
        &repo,
        &home,
        &path,
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
        "init: {}\n{}",
        String::from_utf8_lossy(&init.stderr),
        String::from_utf8_lossy(&init.stdout)
    );

    // Availability detection deliberately ignores our non-executable preview stub.
    // Pin the fixture's selected vendor rather than inheriting host inventory.
    let config_path = repo.join(".aida/config.toml");
    let mut config: toml::Value =
        toml::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    config["agents"]["enabled"] = toml::Value::Array(vec![toml::Value::String("claude".into())]);
    std::fs::write(config_path, toml::to_string(&config).unwrap()).unwrap();
    std::fs::write(
        repo.join(".aida/launch-profile.toml"),
        aida_core::scaffolding::launch_profile::RECIPE,
    )
    .unwrap();
    // trace:STORY-1486 | ai:codex
    for (extra, header) in [
        (
            vec!["--show-prompt"],
            "# AIDA agent launch preview — initial prompt only;",
        ),
        (
            vec!["--show-prompt", "--verbose"],
            "# AIDA agent launch preview — initial prompt only;",
        ),
        (
            vec!["--show-context"],
            "# DRY PREVIEW — no session was started.",
        ),
        (
            vec!["--show-context", "--verbose"],
            "# DRY PREVIEW — no session was started.",
        ),
    ] {
        let mut args = vec!["agent", "new", "claude"];
        args.extend(extra);
        let out = aida(&repo, &home, &path, &args);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(stdout.starts_with(header), "{stdout}");
        assert!(!stdout.contains("git_policy:"), "{stdout}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stderr.contains("git_policy:"), "{stderr}");
        assert!(!stderr.contains("unexpected-agent-execution"), "{stderr}");
    }
    // Explicit downgrades stay visible even without verbose output.
    // trace:STORY-1486 | ai:codex
    let out = aida(
        &repo,
        &home,
        &path,
        &[
            "agent",
            "new",
            "claude",
            "--role",
            "implementer",
            "--no-exec",
            "--git-read-only",
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("git_policy: read-only (explicit --git-read-only downgrade"),
        "{stdout}"
    );
    let roles = aida(&repo, &home, &path, &["role", "scaffold"]);
    assert!(
        roles.status.success(),
        "{}",
        String::from_utf8_lossy(&roles.stderr)
    );
    for role in ["guest", "requester"] {
        picked_role_policy(&repo, &home, &path, role);
    }
    // Unresolvable declarations remain visible without preventing the preview.
    std::os::unix::fs::symlink(repo.join("missing"), repo.join("target")).unwrap();
    let out = aida(
        &repo,
        &home,
        &path,
        &["agent", "new", "claude", "--no-exec", "--verbose"],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("build root unavailable"), "{stdout}");
    let linked = base.join("linked");
    git(
        &repo,
        &["worktree", "add", "--detach", linked.to_str().unwrap()],
    );
    for (parent, requested, downgrade, expected) in [
        ("implementer", Some("implementer"), false, "write"),
        ("guest", Some("implementer"), false, "read-only"),
        ("guest", None, false, "read-only"),
        ("requester", Some("implementer"), false, "read-only"),
        ("implementer", Some("requester"), false, "read-only"),
        ("implementer", Some("implementer"), true, "read-only"),
    ] {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
        cmd.current_dir(&repo)
            .env("HOME", &home)
            .env("AIDA_HOME", &home)
            .env("AIDA_TELEMETRY", "0")
            .env("PATH", &path)
            .env("AIDA_SESSION_ROLE", parent);
        for key in [
            "AIDA_SESSION_GRANT",
            "AIDA_CONTAINED",
            "AIDA_PERMISSION_MODE",
            "AIDA_CONTAINMENT_POSTURE",
            "AIDA_HEADLESS_VENDOR",
            "AIDA_STORE",
            "AIDA_STORE_PATH",
            "AIDA_PROJECT_ROOT",
            "AIDA_SESSION_PROJECT",
            "GIT_DIR",
            "GIT_COMMON_DIR",
            "GIT_WORK_TREE",
        ] {
            cmd.env_remove(key);
        }
        cmd.args([
            "agent",
            "new",
            "claude",
            "--no-exec",
            "--verbose",
            "--cwd",
            linked.to_str().unwrap(),
        ]);
        if let Some(role) = requested {
            cmd.args(["--role", role]);
        }
        if downgrade {
            cmd.arg("--git-read-only");
        }
        let out = cmd.output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{parent}/{requested:?}: {stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            stdout.contains(&format!("git_policy: {expected}")),
            "{stdout}"
        );
        assert!(stdout.contains("declared, not enforced"), "{stdout}");
        assert!(!String::from_utf8_lossy(&out.stderr).contains("unexpected-agent-execution"));
        assert!(
            stdout.contains(&format!("project: {}", linked.display())),
            "{stdout}"
        );
        assert!(
            stdout.contains(&format!(
                "git-dir: {}",
                repo.join(".git/worktrees/linked").display()
            )),
            "{stdout}"
        );
    }
}
