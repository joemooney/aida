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
        std::env::var("PATH").unwrap_or_default()
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
