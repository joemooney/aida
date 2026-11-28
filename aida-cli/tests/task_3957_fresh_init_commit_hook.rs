#![cfg(target_os = "linux")]
//! A new user runs `aida init` and then makes an ordinary commit ("Configurable
//! greeting": no conventional type, no spec ID, no trace marker). The default
//! commit-msg hook is permissive, so the commit goes through; it must not look
//! like a failure. At most one plain hint line, no red, no "failed".
//! Driven through the real binary and the hooks `aida init` installs.
// trace:TASK-3957 | ai:claude

#[path = "support/test_git.rs"]
mod test_git;

use std::path::Path;
use std::process::{Command, Output};

fn path_with_aida() -> String {
    let bin = Path::new(env!("CARGO_BIN_EXE_aida"));
    format!("{}:/usr/bin:/bin", bin.parent().unwrap().display())
}

fn isolated(mut command: Command, root: &Path, repo: &Path) -> Command {
    let home = root.join("home");
    command
        .current_dir(repo)
        .env_clear()
        .env("PATH", path_with_aida())
        .env("HOME", &home)
        .env("USER", "fixture-human")
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("XDG_RUNTIME_DIR", home.join("run"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.com")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.com")
        .env("AIDA_TELEMETRY", "0")
        .stdin(std::process::Stdio::null());
    command
}

fn text(out: &Output) -> String {
    format!(
        "exit={:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn git(root: &Path, repo: &Path, args: &[&str]) -> Output {
    isolated(test_git::git_command(), root, repo)
        .args(args)
        .output()
        .expect("run git")
}

#[test]
fn an_ordinary_commit_after_init_shows_no_hook_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(root.join("home")).unwrap();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["commit", "-q", "--allow-empty", "-m", "initial"][..],
    ] {
        let out = git(&root, &repo, args);
        assert!(out.status.success(), "git {args:?}\n{}", text(&out));
    }
    test_git::enroll_synthetic_home(&repo);

    // Hooks are installed: this is the default a new user gets.
    let init = isolated(Command::new(env!("CARGO_BIN_EXE_aida")), &root, &repo)
        .env("AIDA_OUTPUT_FORMAT", "human")
        .args([
            "init",
            "--force",
            "--no-skills",
            "--no-agent-config",
            "--no-roles",
        ])
        .output()
        .expect("run aida init");
    assert!(init.status.success(), "aida init\n{}", text(&init));
    let hook = repo.join(".git/hooks/commit-msg");
    assert!(hook.is_file(), "aida init must install the commit-msg hook");

    std::fs::write(repo.join("greeting.txt"), "hello\n").unwrap();
    let add = git(&root, &repo, &["add", "greeting.txt"]);
    assert!(add.status.success(), "git add\n{}", text(&add));
    let commit = git(&root, &repo, &["commit", "-m", "Configurable greeting"]);
    let report = text(&commit);

    assert!(commit.status.success(), "the commit must succeed\n{report}");
    // Hook output reaches the user on stderr.
    let stderr = String::from_utf8_lossy(&commit.stderr);
    let lowered = stderr.to_lowercase();
    for alarming in ["failed", "fail", "error", "rejected", "✗"] {
        assert!(
            !lowered.contains(alarming),
            "no failure wording ({alarming:?}) on an ordinary commit\n{report}"
        );
    }
    assert!(
        !stderr.contains("\x1b[0;31m"),
        "no red output on an ordinary commit\n{report}"
    );
    // Exactly the one hint: its presence proves the installed hook ran (so the
    // checks above saw its output), and nothing else was printed.
    let lines: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(
        lines.len() == 1 && lines[0].starts_with("hint: "),
        "a single neutral hint line\n{report}"
    );
}
